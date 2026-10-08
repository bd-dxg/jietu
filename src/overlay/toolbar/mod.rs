// SPDX-License-Identifier: GPL-3.0-only
//! 覆盖层工具栏（EDT-7）：布局与命中测试；绘制见 `draw` 子模块。

mod draw;
mod icons;

use tiny_skia::Pixmap;

use crate::editor::{PALETTE, Tool};
use crate::overlay::geometry::{INFO_OFFSET, INFO_TEXT_H, SelRect};

/// 工具栏高度（绘制与布局共用）。
pub const H: i32 = 34;
/// 方形按钮边长（工具、撤销、开关）。
pub(crate) const BTN: i32 = 26;
/// 色块边长。
pub(crate) const SW: i32 = 22;
/// 元素间距。
pub(crate) const GAP: i32 = 4;
/// 组分隔区宽度。
pub(crate) const SEP: i32 = 9;
/// 与选区的最小间隔。
pub(crate) const OFFSET: i32 = INFO_OFFSET;
/// 内边距。
pub(crate) const PAD: i32 = 5;

/// 架构风格：颜色常量。
pub(crate) const BG: [u8; 4] = [32, 32, 32, 240];
pub(crate) const BORDER: [u8; 4] = [255, 255, 255, 60];
pub(crate) const ICON: [u8; 4] = [255, 255, 255, 235];
pub(crate) const ICON_DIM: [u8; 4] = [255, 255, 255, 70];
pub(crate) const ACCENT: [u8; 4] = [26, 115, 232, 255];
pub(crate) const ACCENT_SOFT: [u8; 4] = [26, 115, 232, 90];

/// 工具栏当前状态（决定高亮与可用性）。
#[derive(Clone, Copy, PartialEq)]
pub struct State {
    pub tool: Tool,
    pub color: usize,
    /// 矩形是否填充（EDT-1）。
    pub filled: bool,
    /// 矩形是否圆角（EDT-1）。
    pub round: bool,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// 工具栏动作。
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    Tool(Tool),
    Color(usize),
    ToggleFill,
    ToggleRound,
    Undo,
    Redo,
}

/// 工具栏布局：整体矩形 + 各元素命中区。
#[derive(Clone)]
pub struct Toolbar {
    pub rect: SelRect,
    /// 命中区（draw 子模块读取所有元素）。
    pub(crate) hits: Vec<(SelRect, Action)>,
}

impl Toolbar {
    /// 命中测试：返回落在哪个元素上。
    pub fn hit(&self, x: i32, y: i32) -> Option<Action> {
        self.hits
            .iter()
            .find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .map(|(_, a)| *a)
    }
}

/// 布局游标：依次排布元素，返回元素左边界。
struct Cursor {
    x: i32,
}

impl Cursor {
    fn next(&mut self, w: i32) -> i32 {
        let x = self.x;
        self.x += w + GAP;
        x
    }

    fn sep(&mut self) {
        self.x += SEP - GAP;
    }
}

/// 计算工具栏布局：优先放在选区下方，空间不足则放上方。
pub fn layout(sel: SelRect, screen_w: i32, screen_h: i32) -> Toolbar {
    let mut c = Cursor { x: PAD };
    let mut hits: Vec<(SelRect, Action)> = Vec::new();
    let mut item = |c: &mut Cursor, w: i32, h: i32, action: Action| {
        let x = c.next(w);
        let y = (H - h) / 2;
        hits.push((SelRect { x, y, w, h }, action));
    };

    for tool in Tool::ALL {
        item(&mut c, BTN, BTN, Action::Tool(tool));
    }
    c.sep();
    for i in 0..PALETTE.len() {
        item(&mut c, SW, SW, Action::Color(i));
    }
    c.sep();
    item(&mut c, BTN, BTN, Action::ToggleFill);
    item(&mut c, BTN, BTN, Action::ToggleRound);
    c.sep();
    item(&mut c, BTN, BTN, Action::Undo);
    item(&mut c, BTN, BTN, Action::Redo);

    let w = c.x - GAP + PAD;
    let x = sel.x.clamp(4, (screen_w - w - 4).max(4));
    // 信息文字占位（见 overlay::text_rect）：与文字同侧时需再让出一个文字高度，避免重叠
    let text_above = sel.y > INFO_TEXT_H + INFO_OFFSET;
    let below = sel.y + sel.h + OFFSET + if text_above { 0 } else { INFO_TEXT_H };
    let y = if below + H + 4 <= screen_h {
        below
    } else if text_above {
        (sel.y - INFO_OFFSET - INFO_TEXT_H - OFFSET - H).max(4)
    } else {
        (sel.y - OFFSET - H).max(4)
    };
    let bar = SelRect { x, y, w, h: H };

    // 相对坐标 → 屏幕坐标
    let hits = hits
        .into_iter()
        .map(|(r, a)| {
            (
                SelRect {
                    x: r.x + x,
                    y: r.y + y,
                    w: r.w,
                    h: r.h,
                },
                a,
            )
        })
        .collect();
    Toolbar { rect: bar, hits }
}

/// 把工具栏绘制到覆盖层像素图（委托给 draw 子模块）。
pub fn draw(pixmap: &mut Pixmap, bar: &Toolbar, state: &State) {
    draw::render(pixmap, bar, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::overlay::geometry::{intersect_rect, text_rect};

    fn sel(x: i32, y: i32, w: i32, h: i32) -> SelRect {
        SelRect { x, y, w, h }
    }

    /// 工具栏不能与信息文字重叠，否则 GDI 文字会盖在工具栏上（见 overlay::text_rect）。
    #[test]
    fn toolbar_never_overlaps_info_text() {
        let (vw, vh) = (2560, 1440);
        let cases = [
            sel(100, 100, 400, 300),   // 常规：文字在上、工具栏在下
            sel(100, 4, 400, 300),     // 贴顶：文字在下，工具栏再往下让一个文字高度
            sel(100, 100, 900, 1300),  // 贴底：工具栏翻到文字上方
            sel(100, 4, 900, 1420),    // 贴顶贴底：工具栏翻到上方、文字在下方
            sel(2400, 100, 150, 1200), // 靠右：工具栏贴右边
        ];
        for s in cases {
            let bar = layout(s, vw, vh).rect;
            let text = text_rect(s, vw);
            assert!(
                intersect_rect(bar, text).is_none(),
                "工具栏 {bar:?} 与信息文字 {text:?} 重叠（选区 {s:?}）"
            );
            assert!(bar.y >= 0 && bar.y + bar.h <= vh, "工具栏应在屏幕内：{bar:?}");
            assert!(bar.x >= 0 && bar.x + bar.w <= vw, "工具栏应在屏幕内：{bar:?}");
        }
    }

    #[test]
    fn toolbar_hit_maps_each_element() {
        let bar = layout(sel(100, 100, 400, 300), 2560, 1440);
        let y = bar.rect.y + H / 2;
        let center = |offset: i32| bar.hit(bar.rect.x + offset, y);
        assert_eq!(center(18), Some(Action::Tool(Tool::Rect)));
        assert_eq!(center(48), Some(Action::Tool(Tool::Arrow)));
        assert_eq!(center(78), Some(Action::Tool(Tool::Text)));
        assert_eq!(center(108), Some(Action::Tool(Tool::Highlight)));
        assert_eq!(center(138), Some(Action::Tool(Tool::Blur)));
        assert_eq!(center(171), Some(Action::Color(0)));
        assert_eq!(center(197), Some(Action::Color(1)));
        assert_eq!(center(295), Some(Action::ToggleFill));
        assert_eq!(center(325), Some(Action::ToggleRound));
        assert_eq!(center(373), Some(Action::Undo));
        assert_eq!(center(403), Some(Action::Redo));
    }
}
