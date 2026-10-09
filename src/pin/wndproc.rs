// SPDX-License-Identifier: GPL-3.0-only
//! 贴图窗口消息分发（PIN-3/4）：拖动、滚轮缩放、Ctrl+滚轮透明度、
//! 双击/Esc 关闭、右键菜单入口。

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::dib::pin_from;
use super::window::ALPHA_STEP;

pub(super) unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let win = match unsafe { pin_from(hwnd) } {
        Some(w) => w,
        None => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    };
    match msg {
        // 单击记录拖动起点（窗口同时被系统激活，Esc 之后可用）。
        // 用 GetCursorPos 屏幕坐标而非客户区坐标：窗口移动会改变鼠标相对客户区位置，
        // 按客户区增量计算会与窗口移动互相反馈 → 拖动抖动（M2b 实测缺陷）。
        WM_LBUTTONDOWN => {
            win.dragging = true;
            win.drag_cursor = unsafe { cursor_pos() };
            let rect = unsafe { window_rect(hwnd) };
            win.win_origin = (rect.left, rect.top);
            let _ = unsafe { SetCapture(hwnd) };
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if win.dragging && wparam.0 as u32 & 0x1 != 0 {
                // MK_LBUTTON
                let (cx, cy) = unsafe { cursor_pos() };
                let nx = win.win_origin.0 + (cx - win.drag_cursor.0);
                let ny = win.win_origin.1 + (cy - win.drag_cursor.1);
                let _ = unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), nx, ny, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE) };
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            win.dragging = false;
            let _ = unsafe { ReleaseCapture() };
            LRESULT(0)
        }
        // 双击关闭（PIN-4）
        WM_LBUTTONDBLCLK => {
            let _ = unsafe { post_close(hwnd) };
            LRESULT(0)
        }
        // 右键菜单（PIN-4）
        WM_RBUTTONUP => {
            super::menu::on_context_menu(win);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as u16 as i16;
            let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
            if ctrl {
                win.adjust_alpha(if delta > 0 { ALPHA_STEP } else { -ALPHA_STEP });
            } else {
                // 预览缩放（最近邻，快）+ 静止后精修
                win.rescale_preview(1.0 + delta as f32 / 1200.0);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 as usize == super::zoom::TIMER_FINAL {
                win.finalize_preview();
            }
            LRESULT(0)
        }
        // Alt+字母产生 WM_SYSKEYDOWN 而非 WM_KEYDOWN：边框快捷键需两者都收（审查 P1-2）；
        // Esc 关闭不受 Alt 影响（Alt+Esc 罕见，一并处理无害）
        WM_SYSKEYDOWN | WM_KEYDOWN => {
            let vk = wparam.0 as u32;
            if vk == 0x1B {
                let _ = unsafe { post_close(hwnd) };
            } else if let (bk, bm) = super::border_key()
                && bk != 0
                && vk == bk
                && held_modifiers() == bm
            {
                // 贴图内边框快捷键（M2b）：交主窗口统一切全部贴图 + 写默认配置
                let _ = unsafe { notify_border_toggled() };
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_NCDESTROY => unsafe {
            let win = Box::from_raw(pin_from(hwnd).unwrap());
            drop(win);
            super::unregister_hwnd(hwnd);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn post_close(hwnd: HWND) -> windows_core::Result<()> {
    unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }
}

unsafe fn window_rect(hwnd: HWND) -> RECT {
    unsafe {
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        r
    }
}

unsafe fn cursor_pos() -> (i32, i32) {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        (pt.x, pt.y)
    }
}

/// 当前按下的修饰键（MOD_ALT=1 / MOD_CONTROL=2 / MOD_SHIFT=4 / MOD_WIN=8）。
fn held_modifiers() -> u32 {
    let mut m = 0u32;
    if unsafe { GetKeyState(0x11) } < 0 {
        m |= 2; // VK_CONTROL
    }
    if unsafe { GetKeyState(0x12) } < 0 {
        m |= 1; // VK_MENU
    }
    if unsafe { GetKeyState(0x10) } < 0 {
        m |= 4; // VK_SHIFT
    }
    if unsafe { GetKeyState(0x5B) } < 0 || unsafe { GetKeyState(0x5C) } < 0 {
        m |= 8; // VK_LWIN / VK_RWIN
    }
    m
}

/// 通知主窗口：贴图边框已在贴图窗口内切换（更新默认配置）。
unsafe fn notify_border_toggled() -> windows_core::Result<()> {
    unsafe {
        let Ok(main) = FindWindowW(crate::app::WINDOW_CLASS, None) else {
            return Ok(());
        };
        PostMessageW(Some(main), crate::app::WM_PIN_BORDER_CHANGED, WPARAM(0), LPARAM(0))
    }
}
