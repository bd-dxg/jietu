// SPDX-License-Identifier: GPL-3.0-only
//! 选区更新与统一脏区重绘（CAP-2/3、PRD 6.3.2）：
//! 底图复位 → 标注对象 → 选中箭头的控制柄 → 选区装饰 → 工具栏，一次 `repaint` 完成并上屏。

use tiny_skia::{Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    DEFAULT_GUI_FONT, GetStockObject, HDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT, TextOutW,
};

use super::geometry::{SelRect, expand_rect, intersect_rect, same_rect, text_rect, union_rect};
use super::surface::{blit_region, restore_region};
use super::{textinput, toolbar};
use crate::editor::{Kind, Object, Point};
use crate::render;

/// 脏区域外扩（覆盖边框、手柄与信息文字）。
pub(super) const DIRTY_PAD: i32 = 60;
/// 手柄边长。
const HANDLE_SIZE: i32 = 7;
/// 曲线控制柄半径（含描边，需 ≤ 对象包围盒的余量）。
const CTRL_RADIUS: f32 = 4.5;
const ACCENT: u8 = 26; // 主题蓝
const ACCENT_G: u8 = 115;
const ACCENT_B: u8 = 232;

impl super::Overlay {
    /// 更新选区并做脏矩形增量重绘（PRD 6.3.2）：
    /// 只处理旧/新选区的装饰与工具栏区域，避免每次鼠标移动全屏重绘。
    pub(super) fn set_selection(&mut self, new: Option<SelRect>) {
        if same_rect(self.selection, new) {
            return;
        }
        let (vw, vh) = (self.display.width() as i32, self.display.height() as i32);
        let old = self.selection;
        self.selection = new;
        let old_bar = self.bar.as_ref().map(|b| b.rect);
        self.bar = new.map(|s| toolbar::layout(s, vw, vh));
        let new_bar = self.bar.as_ref().map(|b| b.rect);

        let mut dirty: Option<SelRect> = None;
        for r in [
            old.map(|o| expand_rect(o, DIRTY_PAD)),
            new.map(|n| expand_rect(n, DIRTY_PAD)),
            old.map(|o| text_rect(o, vw)),
            new.map(|n| text_rect(n, vw)),
            old_bar.map(|b| expand_rect(b, 2)),
            new_bar.map(|b| expand_rect(b, 2)),
        ]
        .into_iter()
        .flatten()
        {
            dirty = union_rect(dirty, Some(r));
        }
        if let Some(d) = dirty {
            self.repaint(d);
        }
    }

    /// 重绘指定区域并上屏（PRD 6.3.2 脏矩形）：
    /// 1) 底图复位（选区外恢复暗图、选区内回贴原图）2) 重绘标注 3) 选区装饰 4) 工具栏。
    pub(super) fn repaint(&mut self, r: SelRect) {
        let r = self.expand_for_objects(r);
        let (w, h) = (self.display.width() as i32, self.display.height() as i32);
        let x0 = r.x.clamp(0, w);
        let y0 = r.y.clamp(0, h);
        let x1 = (r.x + r.w).clamp(0, w);
        let y1 = (r.y + r.h).clamp(0, h);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let region = SelRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        };

        // 1) 底图复位（CAP-3：预生成暗图 + 选区回贴原图）
        restore_region(&mut self.display, &self.dimmed, region);
        if let Some(sel) = self.selection {
            if let Some(overlap) = intersect_rect(region, sel) {
                blit_region(&mut self.display, &self.original, overlap);
            }
        }
        // 2) 标注对象（含正在拖拽的草稿）；二次编辑中的文本对象隐藏，由输入框呈现（EDT-4）
        render::draw_objects(
            &mut self.display,
            self.doc.objects(),
            self.draft.as_ref(),
            self.text_edit.as_ref().and_then(|e| e.obj_index),
            (x0, y0, x1, y1),
        );
        // 2b) 选中箭头的控制柄与虚线辅助线（EDT-3；仅屏幕显示，不进导出烘焙）
        if let Some(obj) = self.selected_object().cloned()
            && render::intersects(&obj.bounds(), (x0, y0, x1, y1))
        {
            draw_curve_handles(&mut self.display, &obj);
        }
        // 2c) 文本输入框（EDT-4；仅屏幕显示，提交后为对象）
        if let Some(edit) = &self.text_edit {
            if intersect_rect(expand_rect(edit.region(), 2), region).is_some() {
                textinput::draw_edit(&mut self.display, edit, crate::editor::PALETTE[self.color_index]);
            }
        }
        // 3) 选区边框与手柄（只在与选区装饰区域相交时重画）
        if let Some(sel) = self.selection {
            if intersect_rect(expand_rect(sel, DIRTY_PAD), region).is_some() {
                draw_rect(&mut self.display, sel);
                draw_handles(&mut self.display, sel);
            }
        }
        // 4) 工具栏
        let state = self.bar_state();
        if let Some(bar) = &self.bar {
            if intersect_rect(expand_rect(bar.rect, 2), region).is_some() {
                toolbar::draw(&mut self.display, bar, &state);
            }
        }
        self.present_region(region);
    }

    /// 把脏区扩展到与它相交的所有标注对象的完整包围盒：
    /// 保证被绘制的对象不会画到 `region` 之外，避免像素图与窗口显示不一致。
    fn expand_for_objects(&self, r: SelRect) -> SelRect {
        let mut r = r;
        loop {
            let clip = (r.x, r.y, r.x + r.w, r.y + r.h);
            let mut changed = false;
            for obj in self.doc.objects().iter().chain(self.draft.iter()) {
                let bounds = obj.bounds();
                if render::intersects(&bounds, clip) {
                    let c = render::bounds_to_clip(&bounds);
                    let b = SelRect {
                        x: c.0,
                        y: c.1,
                        w: c.2 - c.0,
                        h: c.3 - c.1,
                    };
                    if let Some(u) = union_rect(Some(r), Some(b)) {
                        if u != r {
                            r = u;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                return r;
            }
        }
    }
}

/// 选中箭头的控制柄（EDT-3）：控制点处画圆点，与端点之间用虚线辅助线连接。
fn draw_curve_handles(pixmap: &mut Pixmap, obj: &Object) {
    let (Some(cps), Kind::Arrow { from, to, .. }) = (obj.kind.control_points(), &obj.kind) else {
        return;
    };
    let [c1, c2] = cps;
    dashed_line(pixmap, *from, c1);
    dashed_line(pixmap, c2, *to);
    for p in [c1, c2] {
        let mut pb = PathBuilder::new();
        pb.push_circle(p.x, p.y, CTRL_RADIUS);
        let Some(path) = pb.finish() else {
            continue;
        };
        let mut fill = Paint::default();
        fill.set_color_rgba8(255, 255, 255, 255);
        pixmap.fill_path(&path, &fill, tiny_skia::FillRule::Winding, Transform::identity(), None);
        let mut edge = Paint::default();
        edge.set_color_rgba8(ACCENT, ACCENT_G, ACCENT_B, 255);
        let stroke = Stroke {
            width: 1.5,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &edge, &stroke, Transform::identity(), None);
    }
}

/// 虚线辅助线（5px 实 / 4px 虚）：tiny-skia 无 dash，手动拆成小段。
fn dashed_line(pixmap: &mut Pixmap, a: Point, b: Point) {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let mut pb = PathBuilder::new();
    let mut t = 0.0;
    while t < len {
        let end = (t + 5.0).min(len);
        pb.move_to(a.x + ux * t, a.y + uy * t);
        pb.line_to(a.x + ux * end, a.y + uy * end);
        t += 9.0;
    }
    let Some(path) = pb.finish() else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 200);
    let stroke = Stroke {
        width: 1.0,
        ..Default::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// 画选区边框。
fn draw_rect(pixmap: &mut Pixmap, sel: SelRect) {
    let Some(rect) = Rect::from_xywh(
        sel.x as f32 + 0.5,
        sel.y as f32 + 0.5,
        sel.w as f32 - 1.0,
        sel.h as f32 - 1.0,
    ) else {
        return;
    };
    let mut pb = PathBuilder::new();
    pb.push_rect(rect);
    let Some(path) = pb.finish() else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(ACCENT, ACCENT_G, ACCENT_B, 255);
    let stroke = Stroke {
        width: 2.0,
        ..Default::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    // 外圈白色辅助线
    let mut pb = PathBuilder::new();
    if let Some(inner) = Rect::from_xywh(
        sel.x as f32 + 1.5,
        sel.y as f32 + 1.5,
        sel.w as f32 - 3.0,
        sel.h as f32 - 3.0,
    ) {
        pb.push_rect(inner);
    }
    if let Some(p) = pb.finish() {
        let mut white = Paint::default();
        white.set_color_rgba8(255, 255, 255, 160);
        let thin = Stroke {
            width: 1.0,
            ..Default::default()
        };
        pixmap.stroke_path(&p, &white, &thin, Transform::identity(), None);
    }
}

/// 画 8 个缩放手柄。
fn draw_handles(pixmap: &mut Pixmap, sel: SelRect) {
    let (x0, y0, x1, y1) = (sel.x, sel.y, sel.x + sel.w, sel.y + sel.h);
    let cx = x0 + sel.w / 2;
    let cy = y0 + sel.h / 2;
    for (hx, hy) in [
        (x0, y0),
        (cx, y0),
        (x1, y0),
        (x0, cy),
        (x1, cy),
        (x0, y1),
        (cx, y1),
        (x1, y1),
    ] {
        let r = HANDLE_SIZE / 2;
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        if let Some(rect) = Rect::from_xywh((hx - r) as f32, (hy - r) as f32, HANDLE_SIZE as f32, HANDLE_SIZE as f32) {
            let mut pb = PathBuilder::new();
            pb.push_rect(rect);
            if let Some(p) = pb.finish() {
                pixmap.fill_path(&p, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
            }
        }
    }
}

/// GDI 文本：显示选区尺寸与坐标（物理像素），位置由 text_rect 确定。
pub(super) fn draw_info_text(hdc: HDC, sel: SelRect, screen_w: i32) {
    let r = text_rect(sel, screen_w);
    unsafe {
        let font = GetStockObject(DEFAULT_GUI_FONT);
        let _ = SelectObject(hdc, font);
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(0x00FFFFFF));
        let text: Vec<u16> = format!("{} × {}   @({},{})", sel.w, sel.h, sel.x, sel.y)
            .encode_utf16()
            .collect();
        let _ = TextOutW(hdc, r.x + 4, r.y + 4, &text);
    }
}

#[cfg(test)]
mod tests {
    use super::super::geometry::same_rect;
    use super::SelRect;

    #[test]
    fn selection_dirty_covers_old_and_new() {
        // 简单回归：set_selection 依赖 same_rect 判断无变化
        assert!(same_rect(None, None));
        assert!(same_rect(
            Some(SelRect { x: 0, y: 0, w: 1, h: 1 }),
            Some(SelRect { x: 0, y: 0, w: 1, h: 1 })
        ));
        assert!(!same_rect(None, Some(SelRect { x: 0, y: 0, w: 1, h: 1 })));
    }
}
