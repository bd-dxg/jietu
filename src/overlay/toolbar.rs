// SPDX-License-Identifier: GPL-3.0-only
//! 覆盖层工具栏（EDT-7）：工具切换、颜色、线宽、填充/圆角开关、撤销/重做。
//! 自绘（tiny-skia 直接画进覆盖层像素图），布局与命中测试集中在此模块。

use tiny_skia::{Paint, PathBuilder, Pixmap, Rect, Transform};

use crate::editor::{Kind, Object, PALETTE, Point, Style, Tool, WIDTH_PRESETS};
use crate::overlay::{INFO_OFFSET, INFO_TEXT_H, SelRect};
use crate::render;

/// 工具栏高度。
const H: i32 = 34;
/// 内边距。
const PAD: i32 = 5;
/// 方形按钮边长（工具、撤销、开关）。
const BTN: i32 = 26;
/// 色块边长。
const SW: i32 = 22;
/// 线宽按钮宽度。
const WW: i32 = 24;
/// 元素间距。
const GAP: i32 = 4;
/// 组分隔区宽度。
const SEP: i32 = 9;
/// 与选区的最小间隔。
const OFFSET: i32 = INFO_OFFSET;

const BG: [u8; 4] = [32, 32, 32, 240];
const BORDER: [u8; 4] = [255, 255, 255, 60];
const ICON: [u8; 4] = [255, 255, 255, 235];
const ICON_DIM: [u8; 4] = [255, 255, 255, 70];
const ACCENT: [u8; 4] = [26, 115, 232, 255];
const ACCENT_SOFT: [u8; 4] = [26, 115, 232, 90];

/// 工具栏当前状态（决定高亮与可用性）。
#[derive(Clone, Copy, PartialEq)]
pub struct State {
    pub tool: Tool,
    pub color: usize,
    pub width: usize,
    /// 矩形是否填充（EDT-1）。
    pub filled: bool,
    /// 矩形是否圆角（EDT-1）。
    pub round: bool,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// 工具栏动作。
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    Tool(Tool),
    Color(usize),
    Width(usize),
    ToggleFill,
    ToggleRound,
    Undo,
    Redo,
}

/// 工具栏布局：整体矩形 + 各元素命中区。
#[derive(Clone)]
pub struct Toolbar {
    pub rect: SelRect,
    hits: Vec<(SelRect, Action)>,
}

impl Toolbar {
    /// 命中测试：返回落在哪个元素上。
    pub fn hit(&self, x: i32, y: i32) -> Option<Action> {
        self.hits
            .iter()
            .find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .map(|(_, a)| *a)
    }
}

/// 布局游标：依次排布元素，返回元素左边界。
struct Cursor {
    x: i32,
}

impl Cursor {
    fn next(&mut self, w: i32) -> i32 {
        let x = self.x;
        self.x += w + GAP;
        x
    }

    fn sep(&mut self) {
        self.x += SEP - GAP;
    }
}

/// 计算工具栏布局：优先放在选区下方，空间不足则放上方。
pub fn layout(sel: SelRect, screen_w: i32, screen_h: i32) -> Toolbar {
    let mut c = Cursor { x: PAD };
    let mut hits: Vec<(SelRect, Action)> = Vec::new();
    let mut item = |c: &mut Cursor, w: i32, h: i32, action: Action| {
        let x = c.next(w);
        let y = (H - h) / 2;
        hits.push((SelRect { x, y, w, h }, action));
    };

    for tool in Tool::ALL {
        item(&mut c, BTN, BTN, Action::Tool(tool));
    }
    c.sep();
    for i in 0..PALETTE.len() {
        item(&mut c, SW, SW, Action::Color(i));
    }
    c.sep();
    for i in 0..WIDTH_PRESETS.len() {
        item(&mut c, WW, BTN, Action::Width(i));
    }
    c.sep();
    item(&mut c, BTN, BTN, Action::ToggleFill);
    item(&mut c, BTN, BTN, Action::ToggleRound);
    c.sep();
    item(&mut c, BTN, BTN, Action::Undo);
    item(&mut c, BTN, BTN, Action::Redo);

    let w = c.x - GAP + PAD;
    let x = sel.x.clamp(4, (screen_w - w - 4).max(4));
    // 信息文字占位（见 overlay::text_rect）：与文字同侧时需再让出一个文字高度，避免重叠
    let text_above = sel.y > INFO_TEXT_H + INFO_OFFSET;
    let below = sel.y + sel.h + OFFSET + if text_above { 0 } else { INFO_TEXT_H };
    let y = if below + H + 4 <= screen_h {
        below
    } else if text_above {
        (sel.y - INFO_OFFSET - INFO_TEXT_H - OFFSET - H).max(4)
    } else {
        (sel.y - OFFSET - H).max(4)
    };
    let bar = SelRect { x, y, w, h: H };

    // 相对坐标 → 屏幕坐标
    let hits = hits
        .into_iter()
        .map(|(r, a)| {
            (
                SelRect {
                    x: r.x + x,
                    y: r.y + y,
                    w: r.w,
                    h: r.h,
                },
                a,
            )
        })
        .collect();
    Toolbar { rect: bar, hits }
}

/// 把工具栏绘制到覆盖层像素图。
pub fn draw(pixmap: &mut Pixmap, bar: &Toolbar, state: &State) {
    rounded_rect(pixmap, bar.rect, 7.0, BG, Some(BORDER));

    // 组分隔线：画在每个组首个元素左侧的空白处
    for &index in &[2usize, 7, 10, 12] {
        let Some((first, _)) = bar.hits.get(index) else {
            continue;
        };
        let x = first.x - GAP / 2 - 1;
        if let Some(rect) = Rect::from_xywh(x as f32, (bar.rect.y + 7) as f32, 1.0, (H - 14) as f32) {
            pixmap.fill_rect(rect, &paint([255, 255, 255, 40]), Transform::identity(), None);
        }
    }

    for (r, action) in &bar.hits {
        match *action {
            Action::Tool(tool) => {
                let active = tool == state.tool;
                if active {
                    rounded_rect(pixmap, *r, 5.0, ACCENT_SOFT, None);
                }
                let icon = match tool {
                    Tool::Rect => rect_icon(*r, 0.0, false, if active { ICON } else { ICON_DIM }),
                    Tool::Arrow => arrow_icon(*r, if active { ICON } else { ICON_DIM }),
                };
                render::draw_object(pixmap, &icon);
            }
            Action::Color(i) => {
                let inset = 2;
                let swatch = SelRect {
                    x: r.x + inset,
                    y: r.y + inset,
                    w: r.w - inset * 2,
                    h: r.h - inset * 2,
                };
                rounded_rect(pixmap, swatch, 4.0, PALETTE[i], Some([0, 0, 0, 120]));
                if i == state.color {
                    rounded_rect_outline(pixmap, *r, 4.0, ACCENT);
                }
            }
            Action::Width(i) => {
                if i == state.width {
                    rounded_rect(pixmap, *r, 5.0, ACCENT_SOFT, None);
                }
                let lw = WIDTH_PRESETS[i];
                let len = (r.w as f32 * 0.66).max(4.0);
                let yc = r.y as f32 + r.h as f32 / 2.0;
                let xc = r.x as f32 + r.w as f32 / 2.0;
                if let Some(rect) = Rect::from_xywh(xc - len / 2.0, yc - lw / 2.0, len, lw) {
                    pixmap.fill_rect(rect, &paint(ICON), Transform::identity(), None);
                }
            }
            Action::ToggleFill => {
                if state.filled {
                    rounded_rect(pixmap, *r, 5.0, ACCENT_SOFT, None);
                }
                let icon = rect_icon(*r, 0.0, state.filled, ICON);
                render::draw_object(pixmap, &icon);
            }
            Action::ToggleRound => {
                if state.round {
                    rounded_rect(pixmap, *r, 5.0, ACCENT_SOFT, None);
                }
                let icon = rect_icon(*r, 6.0, false, ICON);
                render::draw_object(pixmap, &icon);
            }
            Action::Undo => draw_undo_icon(pixmap, *r, if state.can_undo { ICON } else { ICON_DIM }, false),
            Action::Redo => draw_undo_icon(pixmap, *r, if state.can_redo { ICON } else { ICON_DIM }, true),
        }
    }
}

/// 矩形按钮上的图标（内缩 7px），`radius` 与 `filled` 对应 EDT-1 的圆角/填充。
fn rect_icon(r: SelRect, radius: f32, filled: bool, color: [u8; 4]) -> Object {
    let inset = 7.0;
    Object {
        kind: Kind::Rect {
            a: Point::new(r.x as f32 + inset, r.y as f32 + inset),
            b: Point::new((r.x + r.w) as f32 - inset, (r.y + r.h) as f32 - inset),
            radius,
            filled,
        },
        style: Style::new(color, 2.0),
    }
}

/// 箭头按钮上的图标（左下 → 右上）。
fn arrow_icon(r: SelRect, color: [u8; 4]) -> Object {
    let inset = 5.0;
    Object {
        kind: Kind::Arrow {
            from: Point::new(r.x as f32 + inset, (r.y + r.h) as f32 - inset),
            to: Point::new((r.x + r.w) as f32 - inset, r.y as f32 + inset),
        },
        style: Style::new(color, 2.0),
    }
}

fn paint(color: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], color[3]);
    p
}

/// 填充圆角矩形（可选描边）。
fn rounded_rect(pixmap: &mut Pixmap, r: SelRect, radius: f32, fill: [u8; 4], border: Option<[u8; 4]>) {
    let Some(path) = rounded_path(r, radius) else {
        return;
    };
    pixmap.fill_path(
        &path,
        &paint(fill),
        tiny_skia::FillRule::Winding,
        Transform::identity(),
        None,
    );
    if let Some(bc) = border {
        let stroke = tiny_skia::Stroke {
            width: 1.0,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint(bc), &stroke, Transform::identity(), None);
    }
}

/// 仅描边的圆角矩形（选中态外框）。
fn rounded_rect_outline(pixmap: &mut Pixmap, r: SelRect, radius: f32, color: [u8; 4]) {
    let Some(path) = rounded_path(r, radius) else {
        return;
    };
    let stroke = tiny_skia::Stroke {
        width: 2.0,
        ..Default::default()
    };
    pixmap.stroke_path(&path, &paint(color), &stroke, Transform::identity(), None);
}

fn rounded_path(r: SelRect, radius: f32) -> Option<tiny_skia::Path> {
    let rect = Rect::from_ltrb(
        r.x as f32 + 0.5,
        r.y as f32 + 0.5,
        (r.x + r.w) as f32 - 0.5,
        (r.y + r.h) as f32 - 0.5,
    )?;
    let rad = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let mut pb = PathBuilder::new();
    if rad < 0.5 {
        pb.push_rect(rect);
        return pb.finish();
    }
    let k = rad * 0.552_284_8;
    let (x0, y0, x1, y1) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    pb.move_to(x0 + rad, y0);
    pb.line_to(x1 - rad, y0);
    pb.cubic_to(x1 - rad + k, y0, x1, y0 + rad - k, x1, y0 + rad);
    pb.line_to(x1, y1 - rad);
    pb.cubic_to(x1, y1 - rad + k, x1 - rad + k, y1, x1 - rad, y1);
    pb.line_to(x0 + rad, y1);
    pb.cubic_to(x0 + rad - k, y1, x0, y1 - rad + k, x0, y1 - rad);
    pb.line_to(x0, y0 + rad);
    pb.cubic_to(x0, y0 + rad - k, x0 + rad - k, y0, x0 + rad, y0);
    pb.close();
    pb.finish()
}

/// 撤销/重做图标：顶部半圆弧 + 端点三角（`mirror` 为 true 时镜像成重做）。
fn draw_undo_icon(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4], mirror: bool) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let rad = 6.5;
    let flip = |x: f32| if mirror { 2.0 * cx - x } else { x };

    // 顶弧：θ 从 0°（右）扫到 180°（左）；y 轴向下，故取负
    let mut pb = PathBuilder::new();
    let steps = 18;
    for i in 0..=steps {
        let t = std::f32::consts::PI * i as f32 / steps as f32;
        let (x, y) = (flip(cx + rad * t.cos()), cy - rad * t.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    if let Some(path) = pb.finish() {
        let stroke = tiny_skia::Stroke {
            width: 2.0,
            line_cap: tiny_skia::LineCap::Round,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint(color), &stroke, Transform::identity(), None);
    }

    // 弧末端（左侧）向下的箭头三角
    let tip_x = flip(cx - rad);
    let h = 4.5;
    let mut tri = PathBuilder::new();
    tri.move_to(tip_x, cy + h);
    tri.line_to(tip_x - h * 0.75, cy - h * 0.55);
    tri.line_to(tip_x + h * 0.75, cy - h * 0.55);
    tri.close();
    if let Some(path) = tri.finish() {
        pixmap.fill_path(
            &path,
            &paint(color),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(x: i32, y: i32, w: i32, h: i32) -> SelRect {
        SelRect { x, y, w, h }
    }

    /// 工具栏不能与信息文字重叠，否则 GDI 文字会盖在工具栏上（见 overlay::text_rect）。
    #[test]
    fn toolbar_never_overlaps_info_text() {
        let (vw, vh) = (2560, 1440);
        let cases = [
            sel(100, 100, 400, 300),   // 常规：文字在上、工具栏在下
            sel(100, 4, 400, 300),     // 贴顶：文字在下，工具栏再往下让一个文字高度
            sel(100, 100, 900, 1300),  // 贴底：工具栏翻到文字上方
            sel(100, 4, 900, 1420),    // 贴顶贴底：工具栏翻到上方、文字在下方
            sel(2400, 100, 150, 1200), // 靠右：工具栏贴右边
        ];
        for s in cases {
            let bar = layout(s, vw, vh).rect;
            let text = super::super::text_rect(s, vw);
            assert!(
                super::super::intersect_rect(bar, text).is_none(),
                "工具栏 {bar:?} 与信息文字 {text:?} 重叠（选区 {s:?}）"
            );
            assert!(bar.y >= 0 && bar.y + bar.h <= vh, "工具栏应在屏幕内：{bar:?}");
            assert!(bar.x >= 0 && bar.x + bar.w <= vw, "工具栏应在屏幕内：{bar:?}");
        }
    }

    #[test]
    fn toolbar_hit_maps_each_element() {
        let bar = layout(sel(100, 100, 400, 300), 2560, 1440);
        let y = bar.rect.y + H / 2;
        let center = |offset: i32| bar.hit(bar.rect.x + offset, y);
        assert_eq!(center(18), Some(Action::Tool(Tool::Rect)));
        assert_eq!(center(48), Some(Action::Tool(Tool::Arrow)));
        assert_eq!(center(81), Some(Action::Color(0)));
        assert_eq!(center(107), Some(Action::Color(1)));
        assert_eq!(center(217), Some(Action::Width(0)));
        assert_eq!(center(307), Some(Action::ToggleFill));
        assert_eq!(center(337), Some(Action::ToggleRound));
        assert_eq!(center(372), Some(Action::Undo));
        assert_eq!(center(402), Some(Action::Redo));
    }
}
