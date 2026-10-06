// SPDX-License-Identifier: GPL-3.0-only
//! 矢量渲染（PRD 6.3.2/6.3.3）：把标注对象绘制到 tiny-skia 像素图上。
//! 只负责「对象 → 像素」，不含上屏与脏区逻辑（见 `crate::overlay`）。

use tiny_skia::{FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::editor::{Bounds, Kind, Object, Point, Style, arrow_head_len};

/// 像素区域（x0, y0, x1, y1 半开区间，物理像素）。
pub type Clip = (i32, i32, i32, i32);

/// 绘制对象列表；`draft` 是正在拖拽的临时对象，绘制在最上层。
/// 只有与 `clip` 相交的对象会被绘制，调用方需保证 `clip` 已包含它们的包围盒（否则会污染区域外的像素）。
pub fn draw_objects(pixmap: &mut Pixmap, objects: &[Object], draft: Option<&Object>, clip: Clip) {
    for obj in objects.iter().chain(draft) {
        if intersects(&obj.bounds(), clip) {
            draw_object(pixmap, obj);
        }
    }
}

/// 绘制单个对象。
pub fn draw_object(pixmap: &mut Pixmap, obj: &Object) {
    draw_object_with(pixmap, obj, Transform::identity());
}

/// 绘制单个对象，并施加坐标系变换（导出烘焙时用于平移到裁剪图坐标）。
pub fn draw_object_with(pixmap: &mut Pixmap, obj: &Object, transform: Transform) {
    match obj.kind {
        Kind::Rect { a, b, radius, filled } => draw_rect(pixmap, a, b, radius, filled, &obj.style, transform),
        Kind::Arrow { from, to } => draw_arrow(pixmap, from, to, &obj.style, transform),
    }
}

/// 导出烘焙（OUT-1/OUT-2）：把标注对象栅格化进裁剪结果图。
/// `origin` 是裁剪区域在覆盖层坐标系中的左上角；对象坐标减去它即为裁剪图坐标。
/// 超出裁剪范围的对象由像素图边界自然裁掉。
pub fn bake_objects(pixmap: &mut Pixmap, objects: &[Object], origin: (i32, i32)) {
    let transform = Transform::from_translate(-origin.0 as f32, -origin.1 as f32);
    for obj in objects {
        draw_object_with(pixmap, obj, transform);
    }
}

/// 对象包围盒是否与像素区域相交。
pub fn intersects(b: &Bounds, clip: Clip) -> bool {
    b.x0 < clip.2 as f32 && b.x1 > clip.0 as f32 && b.y0 < clip.3 as f32 && b.y1 > clip.1 as f32
}

/// 对象包围盒向外取整为像素区域，供脏区计算。
pub fn bounds_to_clip(b: &Bounds) -> Clip {
    (
        b.x0.floor() as i32,
        b.y0.floor() as i32,
        b.x1.ceil() as i32 + 1,
        b.y1.ceil() as i32 + 1,
    )
}

fn paint_of(color: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint
}

/// EDT-1 矩形：描边 + 可选填充、圆角、透明度（由颜色 alpha 控制）。
fn draw_rect(pixmap: &mut Pixmap, a: Point, b: Point, radius: f32, filled: bool, style: &Style, tr: Transform) {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    let (x1, y1) = (a.x.max(b.x), a.y.max(b.y));
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return;
    }
    let Some(path) = rounded_rect_path(x0, y0, x1, y1, radius) else {
        return;
    };
    let paint = paint_of(style.color);

    if filled {
        // 填充用半透明色（与描边同色），透明度由颜色 alpha 表达
        pixmap.fill_path(&path, &paint, FillRule::Winding, tr, None);
        return;
    }
    let stroke = Stroke {
        width: style.width,
        ..Default::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, tr, None);
}

/// 圆角矩形路径，圆角用三次贝塞尔近似圆弧；`radius <= 0.5` 时退化为直角矩形。
fn rounded_rect_path(x0: f32, y0: f32, x1: f32, y1: f32, radius: f32) -> Option<Path> {
    let r = radius.min((x1 - x0) * 0.5).min((y1 - y0) * 0.5).max(0.0);
    let mut pb = PathBuilder::new();
    if r < 0.5 {
        pb.push_rect(Rect::from_ltrb(x0, y0, x1, y1)?);
        return pb.finish();
    }
    let k = r * 0.552_284_8;
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.cubic_to(x1 - r + k, y0, x1, y0 + r - k, x1, y0 + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x0 + r, y1);
    pb.cubic_to(x0 + r - k, y1, x0, y1 - r + k, x0, y1 - r);
    pb.line_to(x0, y0 + r);
    pb.cubic_to(x0, y0 + r - k, x0 + r - k, y0, x0 + r, y0);
    pb.close();
    pb.finish()
}

/// EDT-2 直线箭头：头部大小随线宽缩放；线身在头部底边截断，避免粗线从箭尖穿出（EDT-3a）。
fn draw_arrow(pixmap: &mut Pixmap, from: Point, to: Point, style: &Style, tr: Transform) {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.5 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let head = arrow_head_len(style.width).min(len);
    let half = head * 0.45;
    let base = Point::new(to.x - ux * head, to.y - uy * head);
    let paint = paint_of(style.color);

    // 线身：尾 → 头部底边（底边处多画半个线宽，避免抗锯齿缝隙）
    let line_end = Point::new(base.x + ux * style.width * 0.5, base.y + uy * style.width * 0.5);
    let mut line = PathBuilder::new();
    line.move_to(from.x, from.y);
    line.line_to(line_end.x, line_end.y);
    if let Some(path) = line.finish() {
        let stroke = Stroke {
            width: style.width,
            line_cap: LineCap::Butt,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, tr, None);
    }

    // 箭头三角（填充），底边垂直于线身
    let (px, py) = (-uy, ux);
    let mut tri = PathBuilder::new();
    tri.move_to(to.x, to.y);
    tri.line_to(base.x + px * half, base.y + py * half);
    tri.line_to(base.x - px * half, base.y - py * half);
    tri.close();
    if let Some(path) = tri.finish() {
        pixmap.fill_path(&path, &paint, FillRule::Winding, tr, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Kind, Style};

    fn new_pixmap(w: u32, h: u32) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        p.fill(tiny_skia::Color::TRANSPARENT);
        p
    }

    fn alpha_at(p: &Pixmap, x: u32, y: u32) -> u8 {
        p.data()[((y * p.width() + x) * 4 + 3) as usize]
    }

    #[test]
    fn arrow_line_is_drawn_between_endpoints() {
        let mut p = new_pixmap(200, 60);
        let obj = Object {
            kind: Kind::Arrow {
                from: Point::new(10.0, 30.0),
                to: Point::new(180.0, 30.0),
            },
            style: Style::new([255, 0, 0, 255], 4.0),
        };
        draw_object(&mut p, &obj);

        assert!(alpha_at(&p, 50, 30) > 200, "线身中心应有像素");
        assert!(alpha_at(&p, 10, 30) > 200, "起点应有像素");
        assert_eq!(alpha_at(&p, 5, 5), 0, "远离箭头的区域应保持透明");
    }

    #[test]
    fn arrow_head_scales_with_width() {
        let head_thin = arrow_head_len(2.0);
        let head_thick = arrow_head_len(8.0);
        assert!(head_thick > head_thin * 2.0);

        // 粗箭头在尖端附近的垂直覆盖范围应更大
        let mut thin = new_pixmap(200, 120);
        let mut thick = new_pixmap(200, 120);
        let mk = |w: f32| Object {
            kind: Kind::Arrow {
                from: Point::new(10.0, 60.0),
                to: Point::new(180.0, 60.0),
            },
            style: Style::new([0, 0, 0, 255], w),
        };
        draw_object(&mut thin, &mk(2.0));
        draw_object(&mut thick, &mk(8.0));

        // 三角形头部的角度相同（按线宽等比缩放），因此比较「最宽处」而非固定列
        let wide = |p: &Pixmap| {
            (0..200)
                .map(|x| (0..120).filter(|y| alpha_at(p, x, *y) > 32).count())
                .max()
                .unwrap_or(0)
        };
        assert!(
            wide(&thick) > wide(&thin) * 2,
            "粗箭头的头部应明显更宽：thin={} thick={}",
            wide(&thin),
            wide(&thick)
        );
    }

    #[test]
    fn rounded_rect_stroke_covers_corners_but_not_center() {
        let mut p = new_pixmap(120, 120);
        let obj = Object {
            kind: Kind::Rect {
                a: Point::new(10.0, 10.0),
                b: Point::new(110.0, 110.0),
                radius: 12.0,
                filled: false,
            },
            style: Style::new([0, 0, 255, 255], 3.0),
        };
        draw_object(&mut p, &obj);

        assert!(alpha_at(&p, 10, 60) > 128, "左边框应有像素");
        assert!(alpha_at(&p, 60, 10) > 128, "上边框应有像素");
        assert_eq!(alpha_at(&p, 60, 60), 0, "未填充的矩形内部应保持透明");
        assert_eq!(alpha_at(&p, 10, 10), 0, "圆角处（r=12）应被切掉");
    }

    #[test]
    fn filled_rect_covers_interior_with_alpha() {
        let mut p = new_pixmap(120, 120);
        let obj = Object {
            kind: Kind::Rect {
                a: Point::new(10.0, 10.0),
                b: Point::new(110.0, 110.0),
                radius: 0.0,
                filled: true,
            },
            style: Style::new([0, 200, 0, 60], 2.0), // 半透明填充（EDT-1 透明度）
        };
        draw_object(&mut p, &obj);

        let a = alpha_at(&p, 60, 60);
        assert!((50..=70).contains(&a), "内部应为半透明填充，实际 alpha={a}");
    }

    #[test]
    fn bake_translates_objects_into_crop_coordinates() {
        let mut p = new_pixmap(300, 300);
        let obj = Object {
            kind: Kind::Rect {
                a: Point::new(100.0, 100.0),
                b: Point::new(160.0, 160.0),
                radius: 0.0,
                filled: false,
            },
            style: Style::new([255, 0, 0, 255], 4.0),
        };
        // 裁剪区域左上角在覆盖层坐标 (100,100)：对象边框应落在裁剪图坐标 0 附近
        bake_objects(&mut p, &[obj], (100, 100));

        assert!(alpha_at(&p, 1, 30) > 128, "左边框应平移到裁剪图 x≈0");
        assert!(alpha_at(&p, 30, 1) > 128, "上边框应平移到裁剪图 y≈0");
        assert_eq!(alpha_at(&p, 101, 101), 0, "原坐标处不应有像素");
    }

    #[test]
    fn bake_draws_nothing_without_objects() {
        let mut p = new_pixmap(20, 20);
        bake_objects(&mut p, &[], (0, 0));
        assert!(p.data().iter().all(|b| *b == 0));
    }

    #[test]
    fn clip_skips_objects_outside_region() {
        let b = Object {
            kind: Kind::Rect {
                a: Point::new(10.0, 10.0),
                b: Point::new(20.0, 20.0),
                radius: 0.0,
                filled: false,
            },
            style: Style::new([0, 0, 0, 255], 2.0),
        };
        assert!(intersects(&b.bounds(), (0, 0, 100, 100)));
        assert!(!intersects(&b.bounds(), (200, 200, 300, 300)));
    }
}
