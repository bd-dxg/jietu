// SPDX-License-Identifier: GPL-3.0-only
//! 文本对象拖动（EDT-4）：按下已提交文本对象可拖拽移动位置。
//! 无位移的按下退化为单击选中；按下即建撤销快照，松开时有位移才入住撤销栈。

use crate::editor::{Kind, Point};

use super::super::geometry::{SelRect, expand_rect, union_rect};
use super::super::pick::Handle;
use super::Drag;

/// 文字左上角在选区内的活动范围：左边缘到「右边缘 − 文字宽」；
/// 选区小于文字时上下界互换，保证仍可拖动（尽量留在选区内）。
fn range_of(sel_start: i32, sel_len: i32, obj_len: f32) -> (f32, f32) {
    let lo = sel_start as f32;
    let hi = (sel_start + sel_len) as f32 - obj_len;
    (lo.min(hi), lo.max(hi))
}

impl super::super::Overlay {
    /// 按下文本对象：选中并为可能的拖动建立撤销快照。
    pub(super) fn begin_object_move(&mut self, i: usize, sel: SelRect, x: i32, y: i32, pos: Point) {
        self.set_selected(Some(i));
        self.doc.begin();
        self.drag = Some(Drag {
            handle: Handle::MoveObj,
            orig: sel,
            px: x,
            py: y,
            obj: Some((i, pos)),
        });
    }

    /// 拖动中：以按下时位置为基准增量移动，文字内容贴住选区边缘（padding 不参与限制）。
    pub(super) fn drag_object(&mut self, i: usize, orig: Point, dx: i32, dy: i32) {
        let Some(sel) = self.selection else {
            return;
        };
        let before = self.selected_clip();
        // 文字内容尺寸（不含内边距）；padding 仅为命中余量，不做移动限制
        let (tw, th) = match &self.doc.objects()[i].kind {
            Kind::Text { size, .. } => *size,
            _ => return,
        };
        let (min_x, max_x) = range_of(sel.x, sel.w, tw);
        let (min_y, max_y) = range_of(sel.y, sel.h, th);
        {
            let obj = &mut self.doc.objects_mut()[i];
            let Kind::Text { pos, .. } = &mut obj.kind else {
                return;
            };
            pos.x = (orig.x + dx as f32).clamp(min_x, max_x);
            pos.y = (orig.y + dy as f32).clamp(min_y, max_y);
        }
        if let Some(d) = union_rect(before, self.selected_clip()) {
            self.repaint(expand_rect(d, 2));
        }
    }

    /// 松开：有位移入撤销栈，否则丢弃快照（单击仅选中）。
    pub(super) fn end_object_move(&mut self) {
        let moved = self
            .drag
            .and_then(|d| d.obj)
            .and_then(|(i, orig)| match &self.doc.objects()[i].kind {
                Kind::Text { pos, .. } => Some(pos.x != orig.x || pos.y != orig.y),
                _ => None,
            })
            .unwrap_or(false);
        self.drag = None;
        self.doc.commit(moved);
        if moved && let Some(bar) = &self.bar {
            // 撤销按钮可用性变化
            self.repaint(expand_rect(bar.rect, 2));
        }
    }
}
