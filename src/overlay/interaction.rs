// SPDX-License-Identifier: GPL-3.0-only
//! 鼠标与绘制交互（CAP-2、EDT-1/2/7）：框选、八向缩放、空格移动选区、选区内绘制标注。

use crate::editor::{Kind, Object, Point, Tool};
use crate::render;

use super::geometry::{SelRect, expand_rect, union_rect};
use super::wndproc::space_down;

/// 手柄命中半径。
const PICK_RADIUS: i32 = 6;

/// 手柄类型（八向 + 移动）。
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
}

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
        let new = match self.selection {
            None => Some(self.begin_select(x, y)),
            Some(sel) => {
                if let Some(handle) = hit_handle(sel, x, y) {
                    self.drag = Some(Drag {
                        handle,
                        orig: sel,
                        px: x,
                        py: y,
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
                        });
                        self.phase = Phase::Adjusting;
                        Some(sel)
                    } else {
                        // 选区内按下：用当前工具绘制标注（EDT-1/EDT-2）
                        self.start_draw(x, y);
                        return;
                    }
                } else {
                    // 远处点击：开始新选区
                    Some(self.begin_select(x, y))
                }
            }
        };
        self.set_selection(new);
    }

    pub(super) fn on_mouse_move(&mut self, x: i32, y: i32) {
        // 绘制中：草稿终点跟随鼠标（限制在选区内）
        if self.draft.is_some() {
            let Some(sel) = self.selection else {
                return;
            };
            let cx = x.clamp(sel.x, (sel.x + sel.w - 1).max(sel.x));
            let cy = y.clamp(sel.y, (sel.y + sel.h - 1).max(sel.y));
            let before = self.draft_region();
            if let Some(draft) = self.draft.as_mut() {
                match &mut draft.kind {
                    Kind::Rect { b, .. } => {
                        b.x = cx as f32;
                        b.y = cy as f32;
                    }
                    Kind::Arrow { to, .. } => {
                        to.x = cx as f32;
                        to.y = cy as f32;
                    }
                }
            }
            if let Some(d) = union_rect(before, self.draft_region()) {
                self.repaint(expand_rect(d, 2));
            }
            return;
        }
        let Some(drag) = &self.drag else {
            return;
        };
        let (dx, dy) = (x - drag.px, y - drag.py);
        let handle = drag.handle;
        let orig = drag.orig;
        let phase = self.phase;
        let (vw, vh) = (self.display.width() as i32, self.display.height() as i32);
        let max_x = vw.saturating_sub(1);
        let max_y = vh.saturating_sub(1);
        let x = x.clamp(0, max_x);
        let y = y.clamp(0, max_y);

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
            }
        };
        self.set_selection(Some(new));
    }

    pub(super) fn on_lbutton_up(&mut self) {
        // 结束绘制：过短视为误触，丢弃；提交后进入撤销栈
        if let Some(draft) = self.draft.take() {
            let kept = !is_degenerate(&draft);
            if kept {
                self.doc.push(draft);
            }
            self.doc.commit(kept);
            let c = render::bounds_to_clip(&draft.bounds());
            let mut region = SelRect {
                x: c.0,
                y: c.1,
                w: c.2 - c.0,
                h: c.3 - c.1,
            };
            if let Some(bar) = self.bar.as_ref() {
                // 撤销/重做按钮的可用性随之变化
                region = union_rect(Some(region), Some(expand_rect(bar.rect, 2))).unwrap_or(region);
            }
            self.repaint(expand_rect(region, 2));
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
        });
        sel
    }

    /// 在选区内开始拖拽绘制标注（EDT-1/EDT-2）。
    fn start_draw(&mut self, x: i32, y: i32) {
        let p = Point::new(x as f32, y as f32);
        let kind = match self.tool {
            Tool::Rect => Kind::Rect {
                a: p,
                b: p,
                radius: if self.round { 12.0 } else { 0.0 },
                filled: self.filled,
            },
            Tool::Arrow => Kind::Arrow { from: p, to: p },
        };
        self.doc.begin();
        self.draft = Some(Object {
            kind,
            style: self.current_style(),
        });
    }

    /// 草稿对象当前占据的像素区域。
    fn draft_region(&self) -> Option<SelRect> {
        let c = render::bounds_to_clip(&self.draft.as_ref()?.bounds());
        Some(SelRect {
            x: c.0,
            y: c.1,
            w: c.2 - c.0,
            h: c.3 - c.1,
        })
    }
}

/// 命中检测：返回落在哪个手柄/移动区域。
fn hit_handle(sel: SelRect, x: i32, y: i32) -> Option<Handle> {
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
fn is_degenerate(obj: &Object) -> bool {
    match obj.kind {
        Kind::Rect { a, b, .. } => (a.x - b.x).abs() < 2.0 && (a.y - b.y).abs() < 2.0,
        Kind::Arrow { from, to } => (from.x - to.x).abs() < 2.0 && (from.y - to.y).abs() < 2.0,
    }
}
