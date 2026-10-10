// SPDX-License-Identifier: GPL-3.0-only
//! 长截图状态条绘制（M4）：与截图工具栏同风格 —— tiny-skia 圆角矩形 + 主题调色板 +
//! DirectWrite 文字。分层窗口提交（见 `window`）。
//!
//! 布局：[模式按钮] [进度文字 ......] [撤销] [完成] [取消]

use tiny_skia::{Pixmap, Transform};

use crate::editor::Point;
use crate::overlay::geometry::SelRect;
use crate::overlay::toolbar::icons::{paint_of, rounded_path, rounded_rect};
use crate::render::text as dwrite;
use crate::theme::Palette;

/// 状态条高度（物理像素）。
pub const BAR_H: i32 = 40;
/// 状态条最小宽度。
pub const BAR_MIN_W: i32 = 420;
/// 状态条最大宽度。
pub const BAR_MAX_W: i32 = 760;
/// 按钮边长。
const BTN: i32 = 28;
/// 内边距。
const PAD: i32 = 6;
/// 圆角半径。
const RADIUS: f32 = 8.0;
/// 文字字号。
const FONT: f32 = 14.0;

/// 状态条上的按钮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// 切换自动/手动。
    Mode,
    /// 撤销最后一段。
    Undo,
    /// 完成。
    Done,
    /// 取消。
    Cancel,
}

/// 状态条命中区信息。
pub struct Bar {
    /// 整条矩形（屏幕坐标）。
    pub rect: SelRect,
    /// 按钮命中区：(矩形, 按钮)。
    hits: Vec<(SelRect, Button)>,
}

impl Bar {
    /// 命中测试。
    pub fn hit(&self, x: i32, y: i32) -> Option<Button> {
        let lx = x - self.rect.x;
        let ly = y - self.rect.y;
        self.hits
            .iter()
            .find(|(r, _)| lx >= r.x && lx < r.x + r.w && ly >= r.y && ly < r.y + r.h)
            .map(|(_, b)| *b)
    }
}

/// 状态条要显示的状态。
pub struct State {
    /// 是否自动模式。
    pub auto: bool,
    /// 已拼接像素高度。
    pub height: u32,
    /// 是否可撤销。
    pub can_undo: bool,
    /// 提示文字（拼接失败等）。
    pub hint: &'static str,
}

/// 计算状态条几何（条放视口下方，空间不足则上方）。
pub fn layout(vp_x: i32, vp_w: u32, vp_y: i32, vp_h: u32, screen_h: i32) -> Bar {
    let bar_w = (vp_w as i32).clamp(BAR_MIN_W, BAR_MAX_W);
    let x = vp_x + ((vp_w as i32) - bar_w) / 2;
    let below = vp_y + vp_h as i32 + 6;
    let y = if below + BAR_H <= screen_h {
        below
    } else {
        (vp_y - BAR_H - 6).max(0)
    };
    // 按钮从右往左排：取消、完成、撤销、模式
    let mut hits = Vec::new();
    let mut bx = bar_w - PAD - BTN;
    let by = (BAR_H - BTN) / 2;
    for b in [Button::Cancel, Button::Done, Button::Undo] {
        hits.push((
            SelRect {
                x: bx,
                y: by,
                w: BTN,
                h: BTN,
            },
            b,
        ));
        bx -= BTN + 4;
    }
    // 模式按钮在最左侧
    hits.push((
        SelRect {
            x: PAD,
            y: by,
            w: BTN,
            h: BTN,
        },
        Button::Mode,
    ));
    Bar {
        rect: SelRect {
            x,
            y,
            w: bar_w,
            h: BAR_H,
        },
        hits,
    }
}

/// 把状态条绘制到像素图（尺寸 = 条宽 × 条高）。
pub fn render(pixmap: &mut Pixmap, bar: &Bar, state: &State, p: &Palette) {
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let full = SelRect { x: 0, y: 0, w, h };
    rounded_rect(pixmap, full, RADIUS, p.bg, Some(p.border));
    // 顶部强调条（2px，与工具栏选中色呼应）
    let accent = SelRect { x: 0, y: 0, w, h: 3 };
    if let Some(path) = rounded_path(accent, RADIUS) {
        pixmap.fill_path(
            &path,
            &paint_of(p.accent),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    // 模式按钮
    let mode_rect = bar.hits.iter().find(|(_, b)| *b == Button::Mode).map(|(r, _)| *r);
    if let Some(r) = mode_rect {
        draw_mode_icon(pixmap, r, state.auto, p);
    }
    // 撤销
    if let Some(r) = bar.hits.iter().find(|(_, b)| *b == Button::Undo).map(|(r, _)| *r) {
        let c = if state.can_undo { p.icon } else { p.icon_dim };
        draw_undo_icon(pixmap, r, c);
    }
    // 完成（对勾）
    if let Some(r) = bar.hits.iter().find(|(_, b)| *b == Button::Done).map(|(r, _)| *r) {
        draw_check(pixmap, r, p.icon);
    }
    // 取消（叉）
    if let Some(r) = bar.hits.iter().find(|(_, b)| *b == Button::Cancel).map(|(r, _)| *r) {
        draw_cross(pixmap, r, p.icon);
    }

    // 中间进度文字
    let text = if state.hint.is_empty() {
        format!("已拼接 {} px", state.height)
    } else {
        format!("{} · {} px", state.hint, state.height)
    };
    let tx = (mode_rect.map(|r| r.x + r.w + 8).unwrap_or(PAD)) as f32;
    let (tw, th) = dwrite::measure(&text, FONT);
    let ty = (h as f32 - th) / 2.0;
    // 文字不超出撤销按钮左侧
    let undo_x = bar
        .hits
        .iter()
        .find(|(_, b)| *b == Button::Undo)
        .map(|(r, _)| r.x)
        .unwrap_or(w);
    if tx + tw < undo_x as f32 - 4.0 {
        dwrite::draw(pixmap, Point::new(tx, ty), &text, FONT, p.text, Transform::identity());
    }
}

/// 模式图标：自动 = 循环箭头，手动 = 手形指针（简化为方向箭头）。
fn draw_mode_icon(pixmap: &mut Pixmap, r: SelRect, auto: bool, p: &Palette) {
    // 按钮底色
    rounded_rect(pixmap, r, 5.0, p.accent_soft, None);
    let color = p.accent;
    if auto {
        draw_auto(pixmap, r, color);
    } else {
        draw_hand(pixmap, r, color);
    }
}

/// 自动模式：圆形循环箭头。
fn draw_auto(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4]) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let rad = 6.0;
    let mut pb = tiny_skia::PathBuilder::new();
    let steps = 20;
    // 3/4 圆弧
    for i in 0..=steps {
        let t = std::f32::consts::PI * 1.5 * i as f32 / steps as f32 - std::f32::consts::FRAC_PI_2;
        let (x, y) = (cx + rad * t.cos(), cy + rad * t.sin());
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
        pixmap.stroke_path(&path, &paint_of(color), &stroke, Transform::identity(), None);
    }
    // 箭头
    let mut tri = tiny_skia::PathBuilder::new();
    tri.move_to(cx + rad, cy - 3.0);
    tri.line_to(cx + rad + 2.5, cy + 2.0);
    tri.line_to(cx + rad - 3.0, cy + 1.0);
    tri.close();
    if let Some(path) = tri.finish() {
        pixmap.fill_path(
            &path,
            &paint_of(color),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// 手动模式：手形指针（简化为向下箭头 + 手掌矩形）。
fn draw_hand(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4]) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(cx - 4.0, cy - 6.0);
    pb.line_to(cx - 4.0, cy + 4.0);
    pb.line_to(cx - 1.0, cy + 2.0);
    pb.line_to(cx + 4.0, cy + 5.0);
    pb.line_to(cx + 5.0, cy + 2.5);
    pb.line_to(cx + 2.0, cy + 0.5);
    pb.line_to(cx + 5.0, cy - 1.0);
    pb.close();
    if let Some(path) = pb.finish() {
        pixmap.fill_path(
            &path,
            &paint_of(color),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// 撤销图标：左上弧 + 箭头。
fn draw_undo_icon(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4]) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0 + 1.0);
    let rad = 6.0;
    let mut pb = tiny_skia::PathBuilder::new();
    let steps = 18;
    for i in 0..=steps {
        let t = std::f32::consts::PI * i as f32 / steps as f32;
        let (x, y) = (cx + rad * t.cos(), cy - rad * t.sin());
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
        pixmap.stroke_path(&path, &paint_of(color), &stroke, Transform::identity(), None);
    }
    let tip_x = cx - rad;
    let mut tri = tiny_skia::PathBuilder::new();
    tri.move_to(tip_x, cy + 4.0);
    tri.line_to(tip_x - 3.0, cy - 2.0);
    tri.line_to(tip_x + 3.0, cy - 2.0);
    tri.close();
    if let Some(path) = tri.finish() {
        pixmap.fill_path(
            &path,
            &paint_of(color),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// 完成图标：对勾。
fn draw_check(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4]) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(cx - 6.0, cy);
    pb.line_to(cx - 1.5, cy + 5.0);
    pb.line_to(cx + 6.0, cy - 5.0);
    if let Some(path) = pb.finish() {
        let stroke = tiny_skia::Stroke {
            width: 2.2,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint_of(color), &stroke, Transform::identity(), None);
    }
}

/// 取消图标：叉。
fn draw_cross(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4]) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let d = 5.0;
    let stroke = tiny_skia::Stroke {
        width: 2.0,
        line_cap: tiny_skia::LineCap::Round,
        ..Default::default()
    };
    for (a, b) in [
        ((cx - d, cy - d), (cx + d, cy + d)),
        ((cx + d, cy - d), (cx - d, cy + d)),
    ] {
        let mut pb = tiny_skia::PathBuilder::new();
        pb.move_to(a.0, a.1);
        pb.line_to(b.0, b.1);
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(&path, &paint_of(color), &stroke, Transform::identity(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_hits_all_buttons() {
        let bar = layout(100, 800, 100, 600, 1080);
        let cy = bar.rect.y + BAR_H / 2;
        // 右侧三个按钮：取消在最右
        let cancel = bar.hit(bar.rect.x + bar.rect.w - PAD - BTN / 2, cy);
        assert_eq!(cancel, Some(Button::Cancel));
        // 模式按钮在最左
        let mode = bar.hit(bar.rect.x + PAD + BTN / 2, cy);
        assert_eq!(mode, Some(Button::Mode));
        // 中间空白无命中
        assert_eq!(bar.hit(bar.rect.x + bar.rect.w / 2, cy), None);
    }

    #[test]
    fn bar_falls_back_above_when_no_room_below() {
        let bar = layout(100, 800, 900, 160, 1080);
        assert!(bar.rect.y + BAR_H <= 900, "视口下方无空间时应放上方");
    }

    #[test]
    fn render_produces_opaque_panel() {
        let bar = layout(0, 600, 0, 400, 1080);
        let mut px = Pixmap::new(bar.rect.w as u32, bar.rect.h as u32).unwrap();
        let state = State {
            auto: true,
            height: 1234,
            can_undo: true,
            hint: "",
        };
        render(&mut px, &bar, &state, &crate::theme::LIGHT);
        // 中心像素应为不透明背景
        let idx = ((BAR_H / 2) as u32 * px.width() + (px.width() / 2)) as usize * 4;
        assert!(px.data()[idx + 3] > 0, "状态条中心应有不透明像素");
    }
}
