// SPDX-License-Identifier: GPL-3.0-only
//! 工具栏绘制：把工具栏渲染到覆盖层像素图。

use tiny_skia::{Paint, PathBuilder, Pixmap, Rect, Transform};

use crate::editor::{Kind, Object, PALETTE, Point, Style, Tool, WIDTH_PRESETS};
use crate::overlay::SelRect;
use crate::overlay::toolbar::{ACCENT, ACCENT_SOFT, Action, BG, BORDER, GAP, H, ICON, ICON_DIM, State, Toolbar};
use crate::render;

/// 绘制工具栏到像素图。
pub fn render(pixmap: &mut Pixmap, bar: &Toolbar, state: &State) {
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
        kind: Kind::arrow(
            Point::new(r.x as f32 + inset, (r.y + r.h) as f32 - inset),
            Point::new((r.x + r.w) as f32 - inset, r.y as f32 + inset),
        ),
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
