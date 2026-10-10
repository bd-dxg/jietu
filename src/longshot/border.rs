// SPDX-License-Identifier: GPL-3.0-only
//! 长截图视口边框（M4）：在待滚动视口四周画 2px 高亮框，让用户看清截取范围。
//! 分层窗口 + 颜色键：边框不透明、内部完全透明（可点击穿透，不挡目标窗口）。

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetDC, HBRUSH, ReleaseDC,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

/// 窗口类名。
const CLASS: windows::core::PCWSTR = w!("jietu.longshot.border");
/// 边框厚度（像素）。
const EDGE: i32 = 2;
/// 内部透明用的颜色键（极罕见颜色，LWA_COLORKEY）。
const KEY: COLORREF = COLORREF(0x00030201); // RGB(1,2,3)
/// 边框颜色（主题蓝）。
const BORDER: COLORREF = COLORREF(0x00E8731A); // RGB(26,115,232)

/// 在视口四周创建边框窗口（覆盖 x-EDGE .. x+w+EDGE 区域）。
/// 返回窗口句柄；失败返回 None。调用方在结束后 `destroy`。
pub fn spawn(x: i32, y: i32, w: u32, h: u32, hinstance: HINSTANCE) -> Option<HWND> {
    unsafe {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let bx = x - EDGE;
        let by = y - EDGE;
        let bw = w as i32 + EDGE * 2;
        let bh = h as i32 + EDGE * 2;
        let hwnd = CreateWindowExW(
            // WS_EX_TRANSPARENT：鼠标点击/滚轮穿透到下面的目标窗口；
            // 颜色键区域本身命中后也穿透（LWA_COLORKEY）。
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            CLASS,
            w!("长截图边框"),
            WS_POPUP,
            bx,
            by,
            bw,
            bh,
            None,
            None,
            Some(hinstance),
            None,
        )
        .ok()?;
        let _ = SetLayeredWindowAttributes(hwnd, KEY, 255, LWA_COLORKEY);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), bx, by, bw, bh, SWP_SHOWWINDOW);
        Some(hwnd)
    }
}

/// 销毁边框窗口。
pub fn destroy(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            unsafe {
                let mut ps = Default::default();
                let _ = BeginPaint(hwnd, &mut ps);
                draw(hwnd);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 画内部透明 + 四边边框。
fn draw(hwnd: HWND) {
    unsafe {
        let hdc = GetDC(Some(hwnd));
        if hdc.0.is_null() {
            return;
        }
        let mut rc: RECT = Default::default();
        let _ = GetClientRect(hwnd, &mut rc);
        // 先铺满颜色键（透明）
        let key_brush = CreateSolidBrush(KEY);
        let _ = FillRect(hdc, &rc, key_brush);
        let _ = DeleteObject(HBRUSH(key_brush.0).into());
        // 四条边画边框色
        let border_brush = CreateSolidBrush(BORDER);
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        let strips = [
            RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: EDGE,
            }, // 上
            RECT {
                left: 0,
                top: h - EDGE,
                right: w,
                bottom: h,
            }, // 下
            RECT {
                left: 0,
                top: 0,
                right: EDGE,
                bottom: h,
            }, // 左
            RECT {
                left: w - EDGE,
                top: 0,
                right: w,
                bottom: h,
            }, // 右
        ];
        for s in strips {
            let _ = FillRect(hdc, &s, border_brush);
        }
        let _ = DeleteObject(HBRUSH(border_brush.0).into());
        let _ = ReleaseDC(Some(hwnd), hdc);
    }
}
