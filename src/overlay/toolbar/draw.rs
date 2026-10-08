// SPDX-License-Identifier: GPL-3.0-only
//! 工具栏绘制：把工具栏渲染到覆盖层像素图。

use tiny_skia::{Pixmap, Rect, Transform};

use crate::editor::{PALETTE, Tool};
use crate::overlay::SelRect;
use crate::overlay::toolbar::{ACCENT, ACCENT_SOFT, Action, BG, BORDER, GAP, H, ICON, ICON_DIM, State, Toolbar};
use crate::render;

use super::icons::paint_of as paint;
use super::icons::*;

/// 绘制工具栏到像素图。
pub fn render(pixmap: &mut Pixmap, bar: &Toolbar, state: &State) {
    rounded_rect(pixmap, bar.rect, 7.0, BG, Some(BORDER));

    // 组分隔线：画在每个组首个元素左侧的空白处
    for &index in &[4usize, 9, 11] {
        let Some((first, _)) = bar.hits.get(index) else {
            continue;
        };
        let x = first.x - GAP / 2 - 1;
        if let Some(rect) = Rect::from_xywh(x as f32, (bar.rect.y + 7) as f32, 1.0, (H - 14) as f32) {
            pixmap.fill_rect(rect, &paint([255, 255, 255, 40]), Transform::identity(), None);
        }
    }

    for (r, action) in &bar.hits {
        match *action {
            Action::Tool(tool) => {
                let active = tool == state.tool;
                if active {
                    rounded_rect(pixmap, *r, 5.0, ACCENT_SOFT, None);
                }
                match tool {
                    Tool::Rect => {
                        let icon = rect_icon(*r, 0.0, false, if active { ICON } else { ICON_DIM });
                        render::draw_object(pixmap, &icon);
                    }
                    Tool::Arrow => {
                        let icon = arrow_icon(*r, if active { ICON } else { ICON_DIM });
                        render::draw_object(pixmap, &icon);
                    }
                    // 文本图标：字母 T（EDT-4）
                    Tool::Text => text_icon(pixmap, *r, if active { ICON } else { ICON_DIM }),
                    // 高亮图标固定荧光黄（激活时更浓）
                    Tool::Highlight => highlight_icon(pixmap, *r, if active { 230 } else { 150 }),
                    Tool::Blur => blur_icon(pixmap, *r, if active { ICON } else { ICON_DIM }),
                }
            }
            Action::Color(i) => {
                let inset = 2;
                let swatch = SelRect {
                    x: r.x + inset,
                    y: r.y + inset,
                    w: r.w - inset * 2,
                    h: r.h - inset * 2,
                };
                rounded_rect(pixmap, swatch, 4.0, PALETTE[i], Some([0, 0, 0, 120]));
                if i == state.color {
                    rounded_rect_outline(pixmap, *r, 4.0, ACCENT);
                }
            }
            Action::ToggleFill => {
                // 文本工具无背景功能：填充开关置灰禁用（同圆角）
                let enabled = state.tool != Tool::Text;
                if enabled && state.filled {
                    rounded_rect(pixmap, *r, 5.0, ACCENT_SOFT, None);
                }
                let icon = rect_icon(*r, 0.0, state.filled, if enabled { ICON } else { ICON_DIM });
                render::draw_object(pixmap, &icon);
            }
            Action::ToggleRound => {
                // 文本工具下圆角开关无意义：置灰禁用
                let enabled = state.tool != Tool::Text;
                if enabled && state.round {
                    rounded_rect(pixmap, *r, 5.0, ACCENT_SOFT, None);
                }
                let color = if enabled { ICON } else { ICON_DIM };
                let icon = rect_icon(*r, 6.0, false, color);
                render::draw_object(pixmap, &icon);
            }
            Action::Undo => draw_undo_icon(pixmap, *r, if state.can_undo { ICON } else { ICON_DIM }, false),
            Action::Redo => draw_undo_icon(pixmap, *r, if state.can_redo { ICON } else { ICON_DIM }, true),
        }
    }
}
