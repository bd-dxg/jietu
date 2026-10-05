// SPDX-License-Identifier: GPL-3.0-only
//! 标注状态与输出（EDT-1/2/7、OUT-1/2/3）：当前工具与样式、工具栏动作、
//! 撤销/重做、复制与保存。

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW, PostMessageW, WM_CLOSE};
use windows::core::w;

use super::{
    geometry::{SelRect, expand_rect},
    surface::crop_pixmap,
    toolbar, wide,
};
use crate::editor::{self, Style};
use crate::output;

impl super::Overlay {
    /// 当前样式（EDT-7：颜色 + 线宽）。
    pub(super) fn current_style(&self) -> Style {
        Style::new(
            editor::PALETTE[self.color_index],
            editor::WIDTH_PRESETS[self.width_index],
        )
    }

    pub(super) fn bar_state(&self) -> toolbar::State {
        toolbar::State {
            tool: self.tool,
            color: self.color_index,
            width: self.width_index,
            filled: self.filled,
            round: self.round,
            can_undo: self.doc.can_undo(),
            can_redo: self.doc.can_redo(),
        }
    }

    /// 执行工具栏动作（EDT-7）。
    pub(super) fn apply_action(&mut self, action: toolbar::Action) {
        match action {
            toolbar::Action::Tool(t) => self.tool = t,
            toolbar::Action::Color(i) => self.color_index = i,
            toolbar::Action::Width(i) => self.width_index = i,
            toolbar::Action::ToggleFill => self.filled = !self.filled,
            toolbar::Action::ToggleRound => self.round = !self.round,
            toolbar::Action::Undo => return self.on_undo(),
            toolbar::Action::Redo => return self.on_redo(),
        }
        // 仅工具栏自身外观发生变化
        let bar = self.bar.as_ref().map(|b| b.rect);
        if let Some(rect) = bar {
            self.repaint(expand_rect(rect, 2));
        }
    }

    /// 撤销（Ctrl+Z）。
    pub(super) fn on_undo(&mut self) {
        if self.doc.undo() {
            self.repaint_after_history();
        }
    }

    /// 重做（Ctrl+Y / Ctrl+Shift+Z）。
    pub(super) fn on_redo(&mut self) {
        if self.doc.redo() {
            self.repaint_after_history();
        }
    }

    /// 撤销/重做后重绘：被撤销的对象可能位于选区之外，而另一份历史快照里的
    /// 对象包围盒不可得，故整屏重绘（仅按键/按钮触发，不在拖拽路径上）。
    fn repaint_after_history(&mut self) {
        let (vw, vh) = (self.display.width() as i32, self.display.height() as i32);
        self.repaint(SelRect {
            x: 0,
            y: 0,
            w: vw,
            h: vh,
        });
    }

    /// 右键取消（OUT-3）。
    pub(super) fn on_cancel(&mut self) {
        self.cancelled = true;
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    /// Enter / Ctrl+C：复制到剪贴板并关闭（OUT-1）。
    pub(super) fn on_copy(&mut self) {
        if let Some(sel) = self.selection {
            let Some(cropped) = crop_pixmap(&self.original, sel) else {
                return;
            };
            std::thread::spawn(move || {
                if let Err(e) = output::copy_to_clipboard(&cropped) {
                    unsafe {
                        let _ = MessageBoxW(None, wide(&format!("复制失败：{e}")), w!("jietu"), MB_OK | MB_ICONERROR);
                    }
                }
            });
        }
        self.cancelled = false;
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    /// Ctrl+S：保存 PNG 到默认目录并关闭（OUT-2）。
    pub(super) fn on_save(&mut self) {
        if let Some(sel) = self.selection {
            let Some(cropped) = crop_pixmap(&self.original, sel) else {
                return;
            };
            std::thread::spawn(move || {
                let dir = output::default_save_dir();
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join(output::timestamped_filename("png"));
                let _ = output::save_png(&cropped, &path);
            });
        }
        self.cancelled = false;
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}
