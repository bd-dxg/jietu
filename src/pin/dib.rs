// SPDX-License-Identifier: GPL-3.0-only
//! 贴图窗口 DIB 与句柄辅助（M2b）：DIB section 创建、窗口中心、PinWindow 查表。

use std::mem::size_of;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, HBITMAP, HDC,
    HGDIOBJ, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{GWLP_USERDATA, GetWindowLongPtrW, GetWindowRect};

use super::window::PinWindow;

/// 创建 32bpp top-down DIB section，返回 (内存DC, 位图句柄, 像素指针)。
pub(super) unsafe fn create_dib(w: u32, h: u32) -> Result<(HDC, HBITMAP, *mut u8), String> {
    unsafe {
        let hdc = CreateCompatibleDC(None);
        if hdc.0.is_null() {
            return Err("CreateCompatibleDC 失败".into());
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [Default::default()],
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = match CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(e) => {
                let _ = DeleteDC(hdc);
                return Err(format!("CreateDIBSection 失败：{e}"));
            }
        };
        let _ = SelectObject(hdc, HGDIOBJ(bmp.0));
        Ok((hdc, bmp, bits as *mut u8))
    }
}

/// 窗口中心（缩小时保持）。
pub(super) unsafe fn window_center(hwnd: HWND) -> (i32, i32) {
    unsafe {
        let mut r = std::mem::zeroed();
        let _ = GetWindowRect(hwnd, &mut r);
        ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
    }
}

/// 从窗口句柄取 PinWindow（GWLP_USERDATA 装箱指针）。窗口销毁后调用为 UB，调用方保证生命周期。
pub(super) unsafe fn pin_from(hwnd: HWND) -> Option<&'static mut PinWindow> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if ptr == 0 {
            None
        } else {
            Some(&mut *(ptr as *mut PinWindow))
        }
    }
}
