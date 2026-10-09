// SPDX-License-Identifier: GPL-3.0-only
//! 标注状态与输出（EDT-1/2/7、OUT-1/2/3）：当前工具与样式、工具栏动作、
//! 撤销/重做、复制与保存。

use windows::Win32::Foundation::{GlobalFree, LPARAM, WPARAM};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, PostMessageW, WM_CLOSE};

use super::{
    geometry::{SelRect, expand_rect},
    surface::crop_pixmap,
    toolbar,
};
use crate::editor::{self, Style};
use crate::{output, render};

impl super::Overlay {
    /// 当前样式（EDT-7：颜色 + 连续线宽）。
    pub(super) fn current_style(&self) -> Style {
        Style::new(editor::PALETTE[self.color_index], self.line_width)
    }

    /// 按下键对应的标注工具（工具切换键配置，M3）。
    pub(super) fn tool_by_key(&self, vk: u32) -> Option<crate::editor::Tool> {
        let k = &self.tool_keys;
        if vk == k.rect {
            Some(crate::editor::Tool::Rect)
        } else if vk == k.arrow {
            Some(crate::editor::Tool::Arrow)
        } else if vk == k.text {
            Some(crate::editor::Tool::Text)
        } else if vk == k.blur {
            Some(crate::editor::Tool::Blur)
        } else if vk == k.highlight {
            Some(crate::editor::Tool::Highlight)
        } else if vk == k.glow {
            Some(crate::editor::Tool::Glow)
        } else {
            None
        }
    }

    /// 切换到指定工具（经工具栏动作统一处理，使其重绘）。
    pub(super) fn switch_tool(&mut self, t: crate::editor::Tool) {
        self.apply_action(toolbar::Action::Tool(t));
    }

    pub(super) fn bar_state(&self) -> toolbar::State {
        toolbar::State {
            tool: self.tool,
            color: self.color_index,
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
            toolbar::Action::ToggleFill => {
                // 文本工具无背景功能：填充开关仅对矩形有效，文本下忽略
                if self.tool != crate::editor::Tool::Text {
                    self.filled = !self.filled;
                }
            }
            // 圆角开关仅对矩形有效（EDT-4 文本工具下点击无效果）
            toolbar::Action::ToggleRound => {
                if self.tool != crate::editor::Tool::Text {
                    self.round = !self.round;
                }
            }
            toolbar::Action::Undo => return self.on_undo(),
            toolbar::Action::Redo => return self.on_redo(),
            // PIN-1：贴图按钮 → 选区贴图后覆盖层自行关闭（不复绘）
            toolbar::Action::Pin => return self.on_pin(),
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
            self.selected = None; // 索引已变，控制柄不再对应
            self.repaint_after_history();
        }
    }

    /// 重做（Ctrl+Y / Ctrl+Shift+Z）。
    pub(super) fn on_redo(&mut self) {
        if self.doc.redo() {
            self.selected = None;
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

    /// 输出用像素图：裁剪选区 → 把标注对象烘焙进结果图（OUT-1/OUT-2）。
    /// 标注坐标是覆盖层坐标系，需按裁剪原点平移到裁剪图坐标。
    fn baked_selection(&self, sel: SelRect) -> Option<tiny_skia::Pixmap> {
        let mut cropped = crop_pixmap(&self.original, sel)?;
        render::bake_objects(&mut cropped, self.doc.objects(), (sel.x, sel.y));
        Some(cropped)
    }

    /// PIN-1：F3 / 工具栏贴图按钮 → 选区（含标注）跨线程交给主窗口贴图，并关闭覆盖层。
    /// 载荷：GlobalAlloc[u32 w][u32 h][RGBA]，主窗口创建贴图窗口后释放（app/mod.rs WM_PIN_FROM_EDITOR）。
    pub(super) fn on_pin(&mut self) {
        if let Some(sel) = self.selection
            && let Some(cropped) = self.baked_selection(sel)
        {
            let (w, h) = (cropped.width(), cropped.height());
            let px_total = w as usize * h as usize * 4;
            unsafe {
                // 载荷头：u32 w + u32 h + i32 x + i32 y（选区在屏幕上的位置，PIN 定位用）
                let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, 16 + px_total) else {
                    // 分配失败：提示并保留覆盖层（用户可重按 F3 重试，审查 P2-1）
                    crate::app::message_box(None, "贴图内存分配失败，请关闭部分贴图后重试。", MB_OK | MB_ICONERROR);
                    return;
                };
                let ptr = GlobalLock(hg);
                if ptr.is_null() {
                    let _ = GlobalFree(Some(hg));
                    crate::app::message_box(None, "贴图内存锁失败。", MB_OK | MB_ICONERROR);
                    return;
                }
                let hp = ptr as *mut u32;
                *hp = w;
                *hp.add(1) = h;
                let sp = hp.add(2) as *mut i32;
                *sp = sel.x;
                *sp.add(1) = sel.y;
                std::ptr::copy_nonoverlapping(cropped.data().as_ptr(), (ptr as *mut u8).add(16), px_total);
                let _ = GlobalUnlock(hg);
                let _ = PostMessageW(
                    Some(self.main_hwnd),
                    crate::app::WM_PIN_FROM_EDITOR,
                    WPARAM(hg.0 as usize),
                    LPARAM(0),
                );
            }
        }
        // 与复制/保存一致：贴图即完成，关闭覆盖层
        self.cancelled = false;
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    /// Enter / Ctrl+C：复制到剪贴板并关闭（OUT-1）。
    pub(super) fn on_copy(&mut self) {
        if let Some(sel) = self.selection {
            let Some(cropped) = self.baked_selection(sel) else {
                return;
            };
            std::thread::spawn(move || {
                if let Err(e) = output::copy_to_clipboard(&cropped) {
                    crate::app::message_box(None, &format!("复制失败：{e}"), MB_OK | MB_ICONERROR);
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
            let Some(cropped) = self.baked_selection(sel) else {
                return;
            };
            std::thread::spawn(move || {
                let dir = output::default_save_dir();
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join(output::timestamped_filename("png"));
                if let Err(e) = output::save_png(&cropped, &path) {
                    // 保存失败必须提示：静默吞掉会让用户以为截图已保存（OUT-2）
                    crate::app::message_box(None, &format!("保存失败：{e}"), MB_OK | MB_ICONERROR);
                }
            });
        }
        self.cancelled = false;
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}
