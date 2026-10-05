// SPDX-License-Identifier: GPL-3.0-only
//! 覆盖层窗口过程：消息分发、修饰键状态（Ctrl/Shift/空格）。

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::Overlay;
use super::cursor::apply_cursor;
use super::geometry::SelRect;

pub(super) unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_DESTROY || msg == WM_NCCREATE {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let Some(overlay) = app_from(hwnd) else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            overlay.on_lbutton_down(x, y);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            overlay.on_mouse_move(x, y);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            overlay.on_lbutton_up();
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            overlay.on_cancel();
            LRESULT(0)
        }
        // 主动绘制，不靠系统擦除；只重绘无效区域
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                let _ = BeginPaint(hwnd, &mut ps);
            }
            let rc = ps.rcPaint;
            let w = overlay.display.width() as i32;
            let h = overlay.display.height() as i32;
            let x0 = rc.left.clamp(0, w);
            let y0 = rc.top.clamp(0, h);
            let x1 = rc.right.clamp(0, w);
            let y1 = rc.bottom.clamp(0, h);
            if x1 > x0 && y1 > y0 {
                overlay.present_region(SelRect {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                });
            }
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        // 按住空格时切换为自绘抓手光标，提示「空格 + 拖动可移动选区」
        WM_SETCURSOR => {
            apply_cursor(hwnd, space_down());
            LRESULT(1)
        }
        WM_KEYDOWN => {
            // 空格按下立即切抓手光标：系统只在鼠标移动时发 WM_SETCURSOR，不能等它
            if wparam.0 as u32 == 0x20 {
                apply_cursor(hwnd, true);
                return LRESULT(0);
            }
            match wparam.0 as u32 {
                0x1B => overlay.on_cancel(),              // Esc
                0x0D => overlay.on_copy(),                // Enter
                0x43 if ctrl_down() => overlay.on_copy(), // Ctrl+C
                0x53 if ctrl_down() => overlay.on_save(), // Ctrl+S
                0x5A if ctrl_down() => {
                    // Ctrl+Z 撤销 / Ctrl+Shift+Z 重做（EDT-7）
                    if shift_down() {
                        overlay.on_redo()
                    } else {
                        overlay.on_undo()
                    }
                }
                0x59 if ctrl_down() => overlay.on_redo(), // Ctrl+Y
                _ => {}
            }
            LRESULT(0)
        }
        WM_KEYUP => {
            if wparam.0 as u32 == 0x20 {
                apply_cursor(hwnd, false);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn app_from(hwnd: HWND) -> Option<&'static mut Overlay> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if ptr == 0 {
            None
        } else {
            Some(&mut *(ptr as *mut Overlay))
        }
    }
}

/// 修饰键是否按下（`GetKeyState` 高位为 1 表示按下）。
fn key_down(vk: i32) -> bool {
    unsafe { GetKeyState(vk) as u16 & 0x8000 != 0 }
}

fn ctrl_down() -> bool {
    key_down(0x11)
}

fn shift_down() -> bool {
    key_down(0x10)
}

/// 空格键是否按下（按住空格拖动可移动整个选区，CAP-2）。
pub(super) fn space_down() -> bool {
    key_down(0x20)
}
