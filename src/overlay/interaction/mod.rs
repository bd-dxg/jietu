// SPDX-License-Identifier: GPL-3.0-only
//! 鼠标与绘制交互（CAP-2、EDT-1/2/3/7）：框选、八向缩放、空格移动选区、选区内绘制标注。
//! 手柄/对象命中判定与选中状态见 `pick`；文本对象拖动见 `move_obj`，绘制草稿见 `draft`。

mod draft;
mod move_obj;

use crate::editor::{Kind, Point};

use super::geometry::SelRect;
use super::pick::{Handle, hit_handle};
use super::wndproc::space_down;

/// 交互阶段。
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Phase {
    /// 拖框选择中
    Selecting,
    /// 选区确认后，可移动/缩放/输出
    Adjusting,
}

/// 拖拽中的上下文。
#[derive(Clone, Copy)]
pub(super) struct Drag {
    handle: Handle,
    /// 按下时的选区（用于计算增量）。
    orig: SelRect,
    /// 按下时的鼠标位置。
    px: i32,
    py: i32,
    /// Handle::MoveObj：按下命中的对象索引与按下时的位置（EDT-4）。
    obj: Option<(usize, Point)>,
}

impl super::Overlay {
    pub(super) fn on_lbutton_down(&mut self, x: i32, y: i32) {
        // 工具栏优先（EDT-7）：点在工具栏上只切换状态，不开始绘制
        let bar = self.bar.clone();
        if let Some(bar) = &bar {
            if super::geometry::in_rect(bar.rect, x, y) {
                if let Some(action) = bar.hit(x, y) {
                    self.apply_action(action);
                }
                return;
            }
        }
        // 文本输入中：任何左键先提交当前输入（EDT-4），再按常规流程（Text 工具点击空白会开启新输入）
        if self.text_edit.is_some() {
            self.commit_text();
        }
        self.down_started_draw = false;
        let new = match self.selection {
            None => Some(self.begin_select(x, y)),
            Some(sel) => {
                // 控制柄优先于缩放/绘制（EDT-3）：它可能落在对象描边上
                if let Some(index) = self.hit_control(x, y) {
                    self.begin_control_drag(index);
                    return;
                }
                if let Some(handle) = hit_handle(sel, x, y) {
                    self.drag = Some(Drag {
                        handle,
                        orig: sel,
                        px: x,
                        py: y,
                        obj: None,
                    });
                    self.phase = Phase::Adjusting;
                    Some(sel)
                } else if sel.contains(x, y) {
                    if space_down() {
                        // 按住空格拖动：移动整个选区（CAP-2）
                        self.drag = Some(Drag {
                            handle: Handle::Move,
                            orig: sel,
                            px: x,
                            py: y,
                            obj: None,
                        });
                        self.phase = Phase::Adjusting;
                        Some(sel)
                    } else if let Some(i) = self.hit_object(x, y) {
                        // 文本对象：按下即进入拖拽移动（无位移退化为单击选中，双击仍进编辑，EDT-4）
                        let pos = match &self.doc.objects()[i].kind {
                            Kind::Text { pos, .. } => Some(*pos),
                            _ => None,
                        };
                        if let Some(pos) = pos {
                            self.begin_object_move(i, sel, x, y, pos);
                            return;
                        }
                        // 其它对象：仅选中（EDT-7），不新建
                        self.set_selected(Some(i));
                        return;
                    } else {
                        // 空白处按下：用当前工具绘制标注（EDT-1/EDT-2/EDT-3）
                        self.set_selected(None);
                        self.down_started_draw = true;
                        self.start_draw(x, y);
                        return;
                    }
                } else {
                    // 远处点击：开始新选区，并取消选中
                    self.set_selected(None);
                    Some(self.begin_select(x, y))
                }
            }
        };
        self.set_selection(new);
    }

    pub(super) fn on_mouse_move(&mut self, x: i32, y: i32) {
        // 拖动曲线控制柄（EDT-3）
        if self.ctrl_drag.is_some() {
            self.drag_control(x, y);
            return;
        }
        // 绘制中：草稿终点跟随鼠标（限制在选区内）
        if self.draft.is_some() {
            self.update_draft(x, y);
            return;
        }
        let Some(drag) = &self.drag else {
            return;
        };
        let (dx, dy) = (x - drag.px, y - drag.py);
        let handle = drag.handle;
        let orig = drag.orig;
        let obj = drag.obj;
        let phase = self.phase;
        let (vw, vh) = (self.display.width() as i32, self.display.height() as i32);
        let max_x = vw.saturating_sub(1);
        let max_y = vh.saturating_sub(1);
        let x = x.clamp(0, max_x);
        let y = y.clamp(0, max_y);

        // 拖动文本对象（EDT-4）
        if let Some((i, orig_pos)) = obj {
            self.drag_object(i, orig_pos, dx, dy);
            return;
        }

        let new = if phase == Phase::Selecting {
            // 框选阶段：以按下点为锚点拉伸矩形
            SelRect::normalized(orig.x, orig.y, x, y)
        } else {
            match handle {
                Handle::Move => {
                    let lim_x = (max_x - orig.w).max(0);
                    let lim_y = (max_y - orig.h).max(0);
                    SelRect {
                        x: (orig.x + dx).clamp(0, lim_x),
                        y: (orig.y + dy).clamp(0, lim_y),
                        w: orig.w,
                        h: orig.h,
                    }
                }
                Handle::N => SelRect::normalized(orig.x, y, orig.x + orig.w, orig.y + orig.h),
                Handle::S => SelRect {
                    x: orig.x,
                    y: orig.y,
                    w: orig.w,
                    h: (y - orig.y).max(1),
                },
                Handle::W => SelRect::normalized(x, orig.y, orig.x + orig.w, orig.y + orig.h),
                Handle::E => SelRect {
                    x: orig.x,
                    y: orig.y,
                    w: (x - orig.x).max(1),
                    h: orig.h,
                },
                Handle::NE => SelRect::normalized(orig.x, y, orig.x + orig.w, orig.y + orig.h),
                Handle::NW => SelRect::normalized(x, y, orig.x + orig.w, orig.y + orig.h),
                Handle::SE => SelRect::normalized(orig.x, orig.y, x.max(orig.x + 1), y.max(orig.y + 1)),
                Handle::SW => SelRect::normalized(x, orig.y, orig.x + orig.w, orig.y + orig.h),
                // 对象移动已在上方提前返回，此处只做模式穷尽
                Handle::MoveObj => return,
            }
        };
        self.set_selection(Some(new));
    }

    pub(super) fn on_lbutton_up(&mut self) {
        // 结束控制柄拖动（EDT-3）
        if self.ctrl_drag.is_some() {
            self.end_control_drag();
            return;
        }
        // 结束文本对象拖动（EDT-4）：无位移视为单击（仅选中），不进撤销栈
        if let Some(d) = self.drag
            && d.handle == Handle::MoveObj
        {
            self.end_object_move();
            return;
        }
        // 结束绘制：过短视为误触，丢弃；提交后进入撤销栈
        if self.draft.is_some() {
            self.commit_draft();
            return;
        }
        self.drag = None;
        let new = match self.selection {
            Some(sel) if sel.w < 2 || sel.h < 2 => None, // 误触，放弃
            other => {
                if other.is_some() {
                    self.phase = Phase::Adjusting;
                }
                other
            }
        };
        self.set_selection(new);
    }

    /// 开始新的框选（返回初始 1px 选区）。
    fn begin_select(&mut self, x: i32, y: i32) -> SelRect {
        let sel = SelRect { x, y, w: 1, h: 1 };
        self.phase = Phase::Selecting;
        self.drag = Some(Drag {
            handle: Handle::Move,
            orig: sel,
            px: x,
            py: y,
            obj: None,
        });
        sel
    }
}
