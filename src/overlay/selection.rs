// SPDX-License-Identifier: GPL-3.0-only
//! 选区更新与统一脏区重绘（CAP-2/3、PRD 6.3.2）：
//! 底图复位 → 标注对象 → 选中箭头的控制柄 → 选区装饰 → 工具栏，一次 `repaint` 完成并上屏。

use tiny_skia::{Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use super::geometry::{SelRect, expand_rect, intersect_rect, same_rect, text_rect, union_rect};
use super::pick::draw_curve_handles;
use super::surface::{blit_region, restore_region};
use super::{textinput, toolbar};
use crate::render;

/// 脏区域外扩（覆盖边框、手柄与信息文字）。
pub(super) const DIRTY_PAD: i32 = 60;
/// 手柄边长。
const HANDLE_SIZE: i32 = 7;
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
        // 聚光模式：选区内保持暗图，仅聚光对象/草稿区域恢复原图亮度（其余区域继续暗化）。
        // 判断包含正在拖拽的草稿，保证第一笔拖动时即进入暗化状态。
        restore_region(&mut self.display, &self.dimmed, region);
        if let Some(sel) = self.selection {
            if let Some(overlap) = intersect_rect(region, sel) {
                let glow_on = self
                    .doc
                    .objects()
                    .iter()
                    .chain(self.draft.iter())
                    .any(|o| matches!(o.kind, crate::editor::Kind::Glow { .. }));
                if glow_on {
                    // 逐个恢复聚光区域（草稿也包含在内；与 overlap 求交后回贴原图）
                    for obj in self.doc.objects().iter().chain(self.draft.iter()) {
                        if let crate::editor::Kind::Glow { a, b } = obj.kind {
                            // 重新计算区域四边（floor/ceil 取整，不能用截断：浮点值可能为负或非整数）
                            let (gx0, gy0) = (a.x.min(b.x).floor(), a.y.min(b.y).floor());
                            let (gx1, gy1) = (a.x.max(b.x).ceil(), a.y.max(b.y).ceil());
                            let gr = SelRect {
                                x: gx0 as i32,
                                y: gy0 as i32,
                                w: (gx1 - gx0) as i32,
                                h: (gy1 - gy0) as i32,
                            };
                            if let Some(part) = intersect_rect(overlap, gr) {
                                blit_region(&mut self.display, &self.original, part);
                            }
                        }
                    }
                } else {
                    blit_region(&mut self.display, &self.original, overlap);
                }
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
                toolbar::draw(&mut self.display, bar, &state, self.palette);
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
