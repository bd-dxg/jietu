// SPDX-License-Identifier: GPL-3.0-only
//! 工具栏图标（EDT-7）：工具按钮与撤销/重做图标，以及绘制共用的小工具
//! （颜色构造、圆角矩形）。

use tiny_skia::{Paint, PathBuilder, Pixmap, Rect, Transform};

use crate::editor::{Kind, Object, Point, Style};
use crate::overlay::SelRect;

/// 矩形按钮上的图标（内缩 7px），`radius` 与 `filled` 对应 EDT-1 的圆角/填充。
pub(super) fn rect_icon(r: SelRect, radius: f32, filled: bool, color: [u8; 4]) -> Object {
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
pub(super) fn arrow_icon(r: SelRect, color: [u8; 4]) -> Object {
    let inset = 5.0;
    Object {
        kind: Kind::arrow(
            Point::new(r.x as f32 + inset, (r.y + r.h) as f32 - inset),
            Point::new((r.x + r.w) as f32 - inset, r.y as f32 + inset),
        ),
        style: Style::new(color, 2.0),
    }
}

/// 文本按钮图标：字母 T（横杠 + 竖杠，EDT-4）。
pub(super) fn text_icon(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4]) {
    let (x0, x1) = (r.x as f32 + 8.0, (r.x + r.w) as f32 - 8.0);
    let y_top = r.y as f32 + 8.0;
    let bottom = (r.y + r.h) as f32 - 12.0;
    let bar = tiny_skia::Stroke {
        width: 2.2,
        line_cap: tiny_skia::LineCap::Round,
        ..Default::default()
    };
    let paint = paint_of(color);
    // 横杠
    let mut h = PathBuilder::new();
    h.move_to(x0, y_top);
    h.line_to(x1, y_top);
    if let Some(p) = h.finish() {
        pixmap.stroke_path(&p, &paint, &bar, Transform::identity(), None);
    }
    // 竖杠（落到基线处）
    let cx = (x0 + x1) * 0.5;
    let mut v = PathBuilder::new();
    v.move_to(cx, y_top);
    v.line_to(cx, bottom.max(y_top + 1.0));
    if let Some(p) = v.finish() {
        pixmap.stroke_path(&p, &paint, &bar, Transform::identity(), None);
    }
}

/// 聚光高亮按钮图标（M3）：外框圆角矩形 + 中心光斑（区域恢复亮度）
pub(super) fn glow_icon(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4]) {
    let inset = 6.0;
    let box_rect = SelRect {
        x: (r.x as f32 + inset) as i32,
        y: (r.y as f32 + inset) as i32,
        w: (r.w as f32 - inset * 2.0) as i32,
        h: (r.h as f32 - inset * 2.0) as i32,
    };
    rounded_rect_outline(pixmap, box_rect, 3.0, color);
    // 中心光斑：外晕 + 实心圆
    let cx = r.x as f32 + r.w as f32 / 2.0;
    let cy = r.y as f32 + r.h as f32 / 2.0;
    let rad = (r.w as f32 * 0.18).clamp(2.0, 5.0);
    let mut halo = PathBuilder::new();
    halo.push_circle(cx, cy, rad + 2.2);
    if let Some(p) = halo.finish() {
        let fill = paint_of([color[0], color[1], color[2], (color[3] as u32 / 2) as u8]);
        pixmap.fill_path(&p, &fill, tiny_skia::FillRule::Winding, Transform::identity(), None);
    }
    let mut core = PathBuilder::new();
    core.push_circle(cx, cy, rad);
    if let Some(p) = core.finish() {
        pixmap.fill_path(
            &p,
            &paint_of(color),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// 高亮按钮图标：斜置半透明荧光黄色块（模拟画笔笔迹）。
pub(super) fn highlight_icon(pixmap: &mut Pixmap, r: SelRect, alpha: u8) {
    let (x0, x1) = (r.x as f32 + 6.0, (r.x + r.w) as f32 - 6.0);
    let (y_top, y_bot) = (r.y as f32 + 7.0, (r.y + r.h) as f32 - 7.0);
    let skew = 4.0;
    let mut pb = PathBuilder::new();
    pb.move_to(x0, y_top + skew);
    pb.line_to(x1, y_bot + skew);
    pb.line_to(x1, y_bot - skew);
    pb.line_to(x0, y_top - skew);
    pb.close();
    if let Some(path) = pb.finish() {
        pixmap.fill_path(
            &path,
            &paint_of([255, 230, 0, alpha]),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// 模糊按钮图标：中心实心圆 + 断线圆环（表示虚化）。
pub(super) fn blur_icon(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4]) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let mut inner = PathBuilder::new();
    inner.push_circle(cx, cy, 3.5);
    if let Some(path) = inner.finish() {
        pixmap.fill_path(
            &path,
            &paint_of(color),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let rad = 8.5;
    let steps = 20;
    let mut ring = PathBuilder::new();
    for i in (0..steps).step_by(2) {
        let a0 = std::f32::consts::TAU * i as f32 / steps as f32;
        let a1 = std::f32::consts::TAU * (i + 1) as f32 / steps as f32;
        ring.move_to(cx + rad * a0.cos(), cy + rad * a0.sin());
        ring.line_to(cx + rad * a1.cos(), cy + rad * a1.sin());
    }
    if let Some(path) = ring.finish() {
        let stroke = tiny_skia::Stroke {
            width: 1.5,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint_of(color), &stroke, Transform::identity(), None);
    }
}

pub(crate) fn paint_of(color: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], color[3]);
    p
}

/// 填充圆角矩形（可选描边）。
pub(crate) fn rounded_rect(pixmap: &mut Pixmap, r: SelRect, radius: f32, fill: [u8; 4], border: Option<[u8; 4]>) {
    let Some(path) = rounded_path(r, radius) else {
        return;
    };
    pixmap.fill_path(
        &path,
        &paint_of(fill),
        tiny_skia::FillRule::Winding,
        Transform::identity(),
        None,
    );
    if let Some(bc) = border {
        let stroke = tiny_skia::Stroke {
            width: 1.0,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint_of(bc), &stroke, Transform::identity(), None);
    }
}

/// 仅描边的圆角矩形（选中态外框）。
pub(crate) fn rounded_rect_outline(pixmap: &mut Pixmap, r: SelRect, radius: f32, color: [u8; 4]) {
    let Some(path) = rounded_path(r, radius) else {
        return;
    };
    let stroke = tiny_skia::Stroke {
        width: 2.0,
        ..Default::default()
    };
    pixmap.stroke_path(&path, &paint_of(color), &stroke, Transform::identity(), None);
}

pub(crate) fn rounded_path(r: SelRect, radius: f32) -> Option<tiny_skia::Path> {
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
pub(super) fn draw_undo_icon(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4], mirror: bool) {
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
        pixmap.stroke_path(&path, &paint_of(color), &stroke, Transform::identity(), None);
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
            &paint_of(color),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// 贴图按钮图标（PIN-1，M2b）：图钉（圆头 + 杆 + 底部锥形）。
pub(super) fn pin_icon(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4]) {
    let paint = paint_of(color);
    let cx = r.x as f32 + r.w as f32 / 2.0;
    let head_y = r.y as f32 + 9.0;
    let head_r = 3.4;
    // 圆头
    let mut head = PathBuilder::new();
    head.push_circle(cx, head_y, head_r);
    if let Some(path) = head.finish() {
        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
    }
    // 杆（头到底部锥之间的竖线）
    let stroke = tiny_skia::Stroke {
        width: 1.8,
        line_cap: tiny_skia::LineCap::Round,
        ..Default::default()
    };
    let mut shaft = PathBuilder::new();
    shaft.move_to(cx, head_y + head_r);
    shaft.line_to(cx, (r.y + r.h) as f32 - 9.0);
    if let Some(path) = shaft.finish() {
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    // 底部锥形（倒三角）
    let base_y = (r.y + r.h) as f32 - 6.5;
    let mut tri = PathBuilder::new();
    tri.move_to(cx - 4.2, base_y - 2.8);
    tri.line_to(cx + 4.2, base_y - 2.8);
    tri.line_to(cx, base_y + 2.8);
    tri.close();
    if let Some(path) = tri.finish() {
        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
    }
}

/// 长截图按钮图标（M4）：上下两层页面 + 向下箭头，表示纵向延伸拼接。
pub(super) fn longshot_icon(pixmap: &mut Pixmap, r: SelRect, color: [u8; 4]) {
    let paint = paint_of(color);
    // 上层页面（小矩形）
    let top = Rect::from_xywh(r.x as f32 + 6.0, r.y as f32 + 5.0, 12.0, 9.0);
    if let Some(top) = top {
        let mut pb = PathBuilder::new();
        pb.push_rect(top);
        if let Some(path) = pb.finish() {
            pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
        }
    }
    // 下层页面（大矩形）
    let bottom = Rect::from_xywh(r.x as f32 + 6.0, r.y as f32 + 13.0, 14.0, 8.0);
    if let Some(bottom) = bottom {
        let mut pb = PathBuilder::new();
        pb.push_rect(bottom);
        if let Some(path) = pb.finish() {
            let stroke = tiny_skia::Stroke {
                width: 1.6,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
    // 向下箭头（页面下方）
    let (ax, ay) = (r.x as f32 + 13.0, r.y as f32 + 21.0);
    let mut tri = PathBuilder::new();
    tri.move_to(ax - 3.5, ay - 2.5);
    tri.line_to(ax + 3.5, ay - 2.5);
    tri.line_to(ax, ay + 2.5);
    tri.close();
    if let Some(path) = tri.finish() {
        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
    }
}
