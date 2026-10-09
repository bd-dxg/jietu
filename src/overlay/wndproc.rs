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
    if msg == WM_NCCREATE {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // 窗口销毁（DestroyWindow 同步发送，不经消息队列）：必须 PostQuitMessage(0)
    // 让消息循环的 GetMessageW 返回 0 并退出，否则覆盖层线程永久阻塞在消息循环，
    // Overlay（含 4 份全屏位图）永不释放，每次截图都泄漏一份（实测每张 +57MB）。
    if msg == WM_DESTROY {
        unsafe {
            let _ = PostQuitMessage(0);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
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
        WM_MOUSEWHEEL => {
            // lparam 为屏幕坐标（非客户区），转覆盖层坐标由 on_mouse_wheel 完成
            let sx = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let sy = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16; // ±120 单位
            overlay.on_mouse_wheel(sx, sy, delta, ctrl_down());
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            overlay.on_cancel();
            LRESULT(0)
        }
        // 双击：命中对象 → 编辑文本/恢复直线（EDT-3/EDT-4）；
        // 否则选区空白内双击 = 完成截图（复制，M3，替代回车）
        WM_LBUTTONDBLCLK => {
            let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let edited = overlay
                .hit_object(x, y)
                .map(|i| overlay.begin_edit_text(i))
                .unwrap_or(false);
            let handled = if edited { true } else { overlay.reset_curve_at(x, y) };
            if !handled {
                // 双击第一击在选区内空白处按下（start_draw），作废草稿并完成截图
                if let Some(sel) = overlay.selection {
                    if sel.contains(x, y) && overlay.down_started_draw {
                        overlay.draft = None;
                        overlay.on_copy();
                    }
                }
            }
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
            // 文本输入中：只处理输入相关按键（光标移动/退格/Enter 提交/Esc 取消），
            // 拦截全局快捷键与空格抓手切换（空格作为普通字符经 WM_CHAR 进入文本）
            if overlay.is_editing_text() {
                overlay.on_text_key(wparam.0 as u32, shift_down(), ctrl_down());
                return LRESULT(0);
            }
            // 空格按下立即切抓手光标：系统只在鼠标移动时发 WM_SETCURSOR，不能等它
            if wparam.0 as u32 == 0x20 {
                apply_cursor(hwnd, true);
                return LRESULT(0);
            }
            match wparam.0 as u32 {
                0x1B => overlay.on_cancel(),              // Esc
                0x72 => overlay.on_pin(),                 // PIN-1：F3 贴选区（截图期间全局 F3 已撤销）
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
                _ => {
                    // 工具切换键（M3）：任意键都查映射（不限于数字键），
                    // 以便设置面板可把工具键改为字母等
                    if let Some(t) = overlay.tool_by_key(wparam.0 as u32) {
                        overlay.switch_tool(t);
                    }
                }
            }
            LRESULT(0)
        }
        // 文本字符（EDT-4）：普通字符经 TranslateMessage 转 WM_CHAR；IME 候选经组合消息
        WM_CHAR => {
            if overlay.is_editing_text() {
                overlay.on_text_char(wparam.0 as u16);
            }
            LRESULT(0)
        }
        // 中文 IME（EDT-4）：组合开始无需处理；组合更新读取候选/结果；结束清空候选
        WM_IME_STARTCOMPOSITION => LRESULT(0),
        WM_IME_COMPOSITION => {
            if overlay.is_editing_text() {
                overlay.on_text_composition(lparam.0 as u32);
            }
            LRESULT(0)
        }
        WM_IME_ENDCOMPOSITION => {
            if overlay.is_editing_text() {
                overlay.on_text_composition_end();
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
