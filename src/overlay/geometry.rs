// SPDX-License-Identifier: GPL-3.0-only
//! 矩形几何（CAP-2）：`SelRect` 与交集/并集/外扩等运算，供选区、脏区与工具栏共用。

/// 信息文字高度（`text_rect` 与工具栏布局共用）。
pub(super) const INFO_TEXT_H: i32 = 22;
/// 信息文字/工具栏与选区的间隔。
pub(super) const INFO_OFFSET: i32 = 8;

/// 矩形选区（相对虚拟屏幕左上角的物理像素坐标）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl SelRect {
    /// 保证 w/h 非负。
    pub(super) fn normalized(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let x = x0.min(x1);
        let y = y0.min(y1);
        SelRect {
            x,
            y,
            w: x0.max(x1) - x,
            h: y0.max(y1) - y,
        }
    }

    pub(super) fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// 点是否在矩形内（半开区间）。
pub(crate) fn in_rect(r: SelRect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

/// 两矩形交集；不相交时返回 None。
pub(super) fn intersect_rect(a: SelRect, b: SelRect) -> Option<SelRect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    if x1 <= x0 || y1 <= y0 {
        None
    } else {
        Some(SelRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

/// 两可选矩形的并集。
pub(super) fn union_rect(a: Option<SelRect>, b: Option<SelRect>) -> Option<SelRect> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let x0 = a.x.min(b.x);
            let y0 = a.y.min(b.y);
            let x1 = (a.x + a.w).max(b.x + b.w);
            let y1 = (a.y + a.h).max(b.y + b.h);
            Some(SelRect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            })
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// 矩形外扩 pad 像素。
pub(super) fn expand_rect(r: SelRect, pad: i32) -> SelRect {
    SelRect {
        x: r.x - pad,
        y: r.y - pad,
        w: r.w + pad * 2,
        h: r.h + pad * 2,
    }
}

/// 两个可选矩形是否相同。
pub(super) fn same_rect(a: Option<SelRect>, b: Option<SelRect>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.x == y.x && x.y == y.y && x.w == y.w && x.h == y.h,
        _ => false,
    }
}

/// 估算信息文本占位矩形（供脏区域与工具栏避让使用，宽度保守）。
pub(super) fn text_rect(sel: SelRect, screen_w: i32) -> SelRect {
    let len = format!("{} × {}   @({},{})", sel.w, sel.h, sel.x, sel.y)
        .chars()
        .count() as i32;
    let tw = (len * 10 + 16).min(screen_w.max(0));
    let ty = if sel.y > INFO_TEXT_H + INFO_OFFSET {
        sel.y - INFO_TEXT_H - INFO_OFFSET
    } else {
        sel.y + sel.h + INFO_OFFSET
    };
    SelRect {
        x: sel.x.min((screen_w - tw - 8).max(4)).max(4),
        y: ty,
        w: tw,
        h: INFO_TEXT_H,
    }
}
