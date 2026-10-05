// SPDX-License-Identifier: GPL-3.0-only
//! 抓屏（CAP-1）：一次性抓取全部虚拟屏幕（含负坐标/混合 DPI 布局，CAP-5）。
//! 以 `BitBlt`（`CAPTUREBLT`）为主，输出 BGRA 像素与 RGBA 预乘副本供 tiny-skia 使用。

use std::mem::size_of;
use std::ptr::null_mut;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// 一次抓屏结果：整个虚拟屏幕的像素与原点。
pub struct CapturedScreen {
    /// 虚拟屏幕宽度（像素）。
    pub width: u32,
    /// 虚拟屏幕高度（像素）。
    pub height: u32,
    /// 虚拟屏幕左上角物理坐标 X（可为负，多显示器负坐标布局）。
    pub origin_x: i32,
    /// 虚拟屏幕左上角物理坐标 Y。
    pub origin_y: i32,
    /// BGRA 像素，长度 width * height * 4。
    pub bgra: Vec<u8>,
    /// RGBA 预乘像素（供 tiny-skia），长度 width * height * 4。
    pub rgba: Vec<u8>,
}

/// 抓取整个虚拟屏幕（一次，冻结画面）。
pub fn capture_virtual_screen() -> Result<CapturedScreen, String> {
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN) as u32;
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN) as u32;
        if vw == 0 || vh == 0 {
            return Err("虚拟屏幕尺寸为 0，无法抓屏".into());
        }

        let hdc_screen = GetDC(None);
        if hdc_screen.0.is_null() {
            return Err("GetDC 失败".into());
        }
        let hdc_mem = CreateCompatibleDC(None);
        if hdc_mem.0.is_null() {
            let _ = ReleaseDC(None, hdc_screen);
            return Err("CreateCompatibleDC 失败".into());
        }

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: vw as i32,
                biHeight: -(vh as i32), // 自上而下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [Default::default()],
        };
        let mut bits: *mut core::ffi::c_void = null_mut();
        let hbmp = match CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(hbmp) => hbmp,
            Err(e) => {
                let _ = DeleteDC(hdc_mem);
                let _ = ReleaseDC(None, hdc_screen);
                return Err(format!("CreateDIBSection 失败：{e}"));
            }
        };

        let _ = SelectObject(hdc_mem, hbmp.into());
        if BitBlt(
            hdc_mem,
            0,
            0,
            vw as i32,
            vh as i32,
            Some(hdc_screen),
            vx,
            vy,
            SRCCOPY | CAPTUREBLT,
        )
        .is_err()
        {
            let _ = DeleteObject(hbmp.into());
            let _ = DeleteDC(hdc_mem);
            let _ = ReleaseDC(None, hdc_screen);
            return Err("BitBlt 失败".into());
        }

        let len = (vw * vh * 4) as usize;
        let mut bgra = vec![0u8; len];
        std::ptr::copy_nonoverlapping(bits as *const u8, bgra.as_mut_ptr(), len);

        // 转换 BGRA → RGBA 预乘（截图 alpha 恒为不透明，预乘与直通相同）。
        let mut rgba = bgra.clone();
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        let _ = DeleteObject(hbmp.into());
        let _ = DeleteDC(hdc_mem);
        let _ = ReleaseDC(None, hdc_screen);

        Ok(CapturedScreen {
            width: vw,
            height: vh,
            origin_x: vx,
            origin_y: vy,
            bgra,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_screen_works() {
        let screen = capture_virtual_screen().expect("抓屏失败");
        assert!(screen.width > 0 && screen.height > 0);
        assert_eq!(screen.bgra.len(), (screen.width * screen.height * 4) as usize);
        assert_eq!(screen.rgba.len(), screen.bgra.len());
        // RGBA 与 BGRA 的 R/B 通道应交换对应
        let rgba = &screen.rgba[..4];
        let bgra = &screen.bgra[..4];
        assert_eq!(rgba[0], bgra[2]); // R
        assert_eq!(rgba[2], bgra[0]); // B
        assert_eq!(rgba[1], bgra[1]); // G
        assert_eq!(rgba[3], bgra[3]); // A
    }
}
