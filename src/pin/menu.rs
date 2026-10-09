// SPDX-License-Identifier: GPL-3.0-only
//! 贴图右键菜单（PIN-4）：复制、保存、关闭、关闭全部。
//! 导出像素：去除阴影扩展、premultiplied → straight 还原（经 `pixmap_of`）。

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use super::window::PinWindow;

const IDM_PIN_COPY: usize = 41001;
const IDM_PIN_SAVE: usize = 41002;
const IDM_PIN_CLOSE: usize = 41003;
const IDM_PIN_CLOSE_ALL: usize = 41004;

/// 显示右键菜单并分发动作（TrackPopupMenu 模态，内部自带消息循环）。
pub(super) fn on_context_menu(win: &mut PinWindow) {
    let hwnd = win.hwnd();
    let selected = unsafe {
        let mut pt = std::mem::zeroed();
        let _ = GetCursorPos(&mut pt);
        let menu = CreatePopupMenu().ok();
        let Some(menu) = menu else { return };
        let _ = AppendMenuW(menu, MF_STRING, IDM_PIN_COPY, w!("复制"));
        let _ = AppendMenuW(menu, MF_STRING, IDM_PIN_SAVE, w!("保存"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, IDM_PIN_CLOSE, w!("关闭"));
        let _ = AppendMenuW(menu, MF_STRING, IDM_PIN_CLOSE_ALL, w!("关闭全部"));
        let cmd = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    };
    match selected {
        IDM_PIN_COPY => {
            if let Some(pm) = win.export_content() {
                std::thread::spawn(move || {
                    if let Err(e) = crate::output::copy_to_clipboard(&pm) {
                        crate::app::message_box(None, &format!("复制失败：{e}"), MB_OK | MB_ICONERROR);
                    }
                });
            }
        }
        IDM_PIN_SAVE => {
            if let Some(pm) = win.export_content() {
                std::thread::spawn(move || {
                    let dir = crate::output::default_save_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    let path = dir.join(crate::output::timestamped_filename("png"));
                    if let Err(e) = crate::output::save_png(&pm, &path) {
                        crate::app::message_box(None, &format!("保存失败：{e}"), MB_OK | MB_ICONERROR);
                    }
                });
            }
        }
        IDM_PIN_CLOSE => {
            let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        IDM_PIN_CLOSE_ALL => super::close_all(),
        _ => {}
    }
}
