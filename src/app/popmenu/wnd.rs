// SPDX-License-Identifier: GPL-3.0-only
//! 弹出菜单窗口：创建、窗口过程与消息分发。

use windows::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use super::draw::present;
use super::{Ctx, WINDOW_CLASS};

/// 创建无边框置顶弹出窗口（CS_DBLCLKS：双击执行菜单项）。
pub(super) fn create_window(owner: HWND, x: i32, y: i32, w: i32, h: i32) -> Result<HWND, String> {
    unsafe {
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .unwrap_or_default()
                .into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: WINDOW_CLASS,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 && GetLastError().0 != 1410 {
            return Err("菜单窗口类注册失败".into());
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            WINDOW_CLASS,
            w!("jietu.menu"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            Some(owner),
            None,
            Some(GetModuleHandleW(None).unwrap_or_default().into()),
            None,
        )
        .map_err(|e| format!("创建菜单窗口失败：{e}"))?;
        Ok(hwnd)
    }
}

pub(super) unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    if msg == WM_DESTROY {
        unsafe {
            let _ = PostQuitMessage(0);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
    }
    let Some(ctx) = app_from(hwnd) else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    let mouse = |lparam: LPARAM| {
        (
            (lparam.0 & 0xFFFF) as u16 as i16 as i32,
            ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
        )
    };
    match msg {
        // 单击即执行（标准菜单交互）；点击菜单外关闭
        WM_LBUTTONDOWN => {
            let (x, y) = mouse(lparam);
            if let Some(i) = ctx.hit_item(x, y) {
                ctx.result = Some(ctx.items[i].id);
                ctx.close();
            } else {
                ctx.close();
            }
            LRESULT(0)
        }
        // 双击与单击一致（快速连点不产生歧义）
        WM_LBUTTONDBLCLK => {
            let (x, y) = mouse(lparam);
            if let Some(i) = ctx.hit_item(x, y) {
                ctx.result = Some(ctx.items[i].id);
                ctx.close();
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = mouse(lparam);
            let hit = ctx.hit_item(x, y);
            if hit != ctx.hover {
                ctx.hover = hit;
                present(ctx);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            match wparam.0 as u32 {
                0x1B => ctx.close(), // Esc
                0x0D => {
                    // Enter：执行悬停项
                    if let Some(i) = ctx.hover {
                        ctx.result = Some(ctx.items[i].id);
                        ctx.close();
                    }
                }
                0x26 | 0x28 => {
                    // 上下方向键导航（悬停高亮移动）
                    let len = ctx.items.len();
                    let cur = ctx.hover.map(|i| i as i32).unwrap_or(-1);
                    let next = if wparam.0 as u32 == 0x26 {
                        (cur - 1).max(0)
                    } else {
                        (cur + 1).min(len as i32 - 1)
                    };
                    if len > 0 {
                        ctx.hover = Some(next as usize);
                        present(ctx);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        // 失去焦点（点击外部）：关闭菜单
        WM_ACTIVATE => {
            if wparam.0 as u16 == 0 {
                ctx.close();
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn app_from(hwnd: HWND) -> Option<&'static mut Ctx> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if ptr == 0 { None } else { Some(&mut *(ptr as *mut Ctx)) }
    }
}
