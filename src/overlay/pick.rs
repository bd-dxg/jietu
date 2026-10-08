// SPDX-License-Identifier: GPL-3.0-only
//! 命中测试与选中状态（EDT-3/EDT-7）：选区八向手柄、已有对象、曲线控制柄。
//! 控制柄位置即曲线控制点 P1/P2（直线箭头时落在 1/3、2/3 处）。

use crate::editor::{Kind, Object, Point};

use super::geometry::{SelRect, expand_rect, union_rect};
use super::selection::DIRTY_PAD;

/// 手柄命中半径。
pub(super) const PICK_RADIUS: i32 = 6;

/// 选区手柄类型（八向 + 移动 + 对象移动）。
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Handle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
    Move,
    /// 拖动文本对象（EDT-4）。
    MoveObj,
}

/// 八向手柄/移动命中（CAP-2）。
pub(super) fn hit_handle(sel: SelRect, x: i32, y: i32) -> Option<Handle> {
    let (x0, y0, x1, y1) = (sel.x, sel.y, sel.x + sel.w, sel.y + sel.h);
    let cx = x0 + sel.w / 2;
    let cy = y0 + sel.h / 2;
    for (hx, hy, h) in [
        (x0, y0, Handle::NW),
        (cx, y0, Handle::N),
        (x1, y0, Handle::NE),
        (x0, cy, Handle::W),
        (x1, cy, Handle::E),
        (x0, y1, Handle::SW),
        (cx, y1, Handle::S),
        (x1, y1, Handle::SE),
    ] {
        if (x - hx).abs() <= PICK_RADIUS && (y - hy).abs() <= PICK_RADIUS {
            return Some(h);
        }
    }
    None
}

/// 误触判定：长宽均不足 2px 的对象丢弃（EDT-1/EDT-2）。
pub(super) fn is_degenerate(obj: &Object) -> bool {
    match &obj.kind {
        Kind::Rect { a, b, .. } => (a.x - b.x).abs() < 2.0 && (a.y - b.y).abs() < 2.0,
        Kind::Arrow { from, to, .. } => (from.x - to.x).abs() < 2.0 && (from.y - to.y).abs() < 2.0,
        // 区域类（高亮/模糊）：过小视为误触
        Kind::Highlight { a, b } => (a.x - b.x).abs() < 2.0 && (a.y - b.y).abs() < 2.0,
        Kind::Blur { a, b, .. } => (a.x - b.x).abs() < 2.0 && (a.y - b.y).abs() < 2.0,
        // 文本：空文本在提交时已拦截，进入文档的均为有效对象
        Kind::Text { .. } => false,
    }
}

impl super::Overlay {
    /// 已选对象。
    pub(super) fn selected_object(&self) -> Option<&Object> {
        self.doc.objects().get(self.selected?)
    }

    /// 切换选中对象；控制柄与辅助线随之变化，故重绘整个选区（只在点选时触发）。
    pub(super) fn set_selected(&mut self, new: Option<usize>) {
        if self.selected == new {
            return;
        }
        let had = self.selected.is_some();
        self.selected = new;
        if (had || new.is_some())
            && let Some(sel) = self.selection
        {
            self.repaint(expand_rect(sel, DIRTY_PAD));
        }
    }

    /// 命中已有对象：从上层往下找（列表顺序即 z 序，EDT-7）。
    pub(super) fn hit_object(&self, x: i32, y: i32) -> Option<usize> {
        let q = Point::new(x as f32, y as f32);
        self.doc.objects().iter().rposition(|o| o.hit(q, PICK_RADIUS as f32))
    }

    /// 命中已选箭头的控制柄（EDT-3），返回 0（尾部侧）或 1（头部侧）。
    pub(super) fn hit_control(&self, x: i32, y: i32) -> Option<usize> {
        let cps = self.selected_object()?.kind.control_points()?;
        cps.iter()
            .position(|p| (p.x - x as f32).abs() <= PICK_RADIUS as f32 && (p.y - y as f32).abs() <= PICK_RADIUS as f32)
    }

    /// 按下控制柄：记录起点（用于「未移动则不进撤销栈」）。
    pub(super) fn begin_control_drag(&mut self, index: usize) {
        let Some(p) = self
            .selected_object()
            .and_then(|o| o.kind.control_points())
            .map(|c| c[index])
        else {
            return;
        };
        self.doc.begin();
        self.ctrl_drag = Some((index, p));
    }

    /// 拖动控制柄：控制点跟随鼠标（限制在选区内），重绘受影响区域。
    pub(super) fn drag_control(&mut self, x: i32, y: i32) {
        let Some((index, _)) = self.ctrl_drag else {
            return;
        };
        let Some(sel) = self.selection else {
            return;
        };
        let Some(i) = self.selected else {
            return;
        };
        let before = self.selected_clip();
        let cx = x.clamp(sel.x, (sel.x + sel.w - 1).max(sel.x)) as f32;
        let cy = y.clamp(sel.y, (sel.y + sel.h - 1).max(sel.y)) as f32;
        self.doc.objects_mut()[i].kind.set_control(index, Point::new(cx, cy));
        if let Some(d) = union_rect(before, self.selected_clip()) {
            self.repaint(expand_rect(d, 2));
        }
    }

    /// 松开控制柄：未移动则丢弃本次历史快照。
    pub(super) fn end_control_drag(&mut self) {
        let Some((index, orig)) = self.ctrl_drag.take() else {
            return;
        };
        let moved = self
            .selected_object()
            .and_then(|o| o.kind.control_points())
            .map(|c| c[index] != orig)
            .unwrap_or(false);
        self.doc.commit(moved);
        if moved && let Some(bar) = &self.bar {
            // 撤销按钮可用性可能变化
            self.repaint(expand_rect(bar.rect, 2));
        }
    }

    /// 双击控制柄恢复直线（EDT-3）。
    pub(super) fn reset_curve_at(&mut self, x: i32, y: i32) {
        if self.hit_control(x, y).is_none() {
            return;
        }
        self.ctrl_drag = None; // 双击前那次按下启动的拖动作废
        let Some(i) = self.selected else {
            return;
        };
        let before = self.selected_clip();
        self.doc.begin();
        let changed = self.doc.objects_mut()[i].kind.reset_curve();
        self.doc.commit(changed);
        if changed {
            if let Some(d) = union_rect(before, self.selected_clip()) {
                self.repaint(expand_rect(d, 2));
            }
            if let Some(bar) = &self.bar {
                self.repaint(expand_rect(bar.rect, 2));
            }
        }
    }

    /// 选中对象占据的像素区域。
    pub(super) fn selected_clip(&self) -> Option<SelRect> {
        let c = crate::render::bounds_to_clip(&self.selected_object()?.bounds());
        Some(SelRect {
            x: c.0,
            y: c.1,
            w: c.2 - c.0,
            h: c.3 - c.1,
        })
    }

    /// 鼠标滚轮（EDT-7/EDT-4）：悬停在线条/矩形边框 / 模糊区域 / 文本上时，
    /// 滚轮连续调整线宽、模糊半径或字号（Ctrl 3 倍步进）；无悬停对象时调整当前工具默认值。
    /// `sx`/`sy` 为屏幕坐标，需减虚拟屏幕原点转覆盖层坐标。
    pub(super) fn on_mouse_wheel(&mut self, sx: i32, sy: i32, delta: i16, ctrl: bool) {
        let (x, y) = (sx - self.capture.origin_x, sy - self.capture.origin_y);
        let step = if ctrl { 3.0 } else { 1.0 } * if delta > 0 { 1.0 } else { -1.0 };
        // 文本输入中：滚轮调整输入字号（EDT-4）
        if let Some(edit) = self.text_edit.as_mut() {
            edit.font_size = (edit.font_size + step).clamp(10.0, 96.0);
            self.font_size = edit.font_size;
            self.repaint_text_edit();
            return;
        }
        if let Some(i) = self.hit_object(x, y) {
            self.adjust_object(i, step);
        } else {
            // 空白处滚动：只改默认值，供下一次绘制沿用
            match self.tool {
                crate::editor::Tool::Rect | crate::editor::Tool::Arrow => {
                    self.line_width = (self.line_width + step).clamp(1.0, 32.0);
                }
                crate::editor::Tool::Text => {
                    self.font_size = (self.font_size + step).clamp(10.0, 96.0);
                }
                crate::editor::Tool::Blur => self.blur_radius = (self.blur_radius + step).clamp(4.0, 64.0),
                crate::editor::Tool::Highlight => {}
            }
        }
    }

    /// 调整指定对象的线宽 / 模糊半径 / 字号（差分入撤销栈），并同步全局默认值。
    fn adjust_object(&mut self, i: usize, step: f32) {
        let before = self.selected_clip();
        self.doc.begin();
        let changed = {
            let obj = &mut self.doc.objects_mut()[i];
            match &mut obj.kind {
                Kind::Rect { .. } | Kind::Arrow { .. } => {
                    let w = (obj.style.width + step).clamp(1.0, 32.0);
                    let changed = (w - obj.style.width).abs() > 0.01;
                    if changed {
                        obj.style.width = w;
                        self.line_width = w;
                    }
                    changed
                }
                Kind::Blur { radius, .. } => {
                    let r = (*radius + step).clamp(4.0, 64.0);
                    let changed = (r - *radius).abs() > 0.01;
                    if changed {
                        *radius = r;
                        self.blur_radius = r;
                    }
                    changed
                }
                // 文本：字号连续可调，同步重建测量缓存（EDT-4）
                Kind::Text {
                    font_size, size, text, ..
                } => {
                    let fs = (*font_size + step).clamp(10.0, 96.0);
                    let changed = (*font_size - fs).abs() > 0.01;
                    if changed {
                        *font_size = fs;
                        *size = crate::render::text::measure(text, fs);
                        self.font_size = fs;
                    }
                    changed
                }
                Kind::Highlight { .. } => false,
            }
        };
        self.doc.commit(changed);
        if !changed {
            return;
        }
        let after = self.selected_clip();
        let dirty = union_rect(before, after);
        if let Some(d) = dirty {
            self.repaint(expand_rect(d, 4));
        }
        if let Some(bar) = &self.bar {
            // 撤销可用性变化
            self.repaint(expand_rect(bar.rect, 2));
        }
    }
}
