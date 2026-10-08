// SPDX-License-Identifier: GPL-3.0-only
//! 文本位图缓冲（EDT-4）：DirectWrite 栅格化用的内存 DIB 与字形像素混合。
//! 字形先以黑字白底画进 DIB，按反色覆盖度把目标色混合进 tiny_skia 像素图。

use tiny_skia::{Pixmap, Transform};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    GetCurrentObject, GetDIBits, HBITMAP, HDC, OBJ_BITMAP, SelectObject,
};

use crate::editor::Point;

/// 创建位图渲染目标使用的（w×h 32bpp top-down）内存 DIB 的 DC 与位图。
pub(super) fn create_dib(w: u32, h: u32) -> Option<(HDC, HBITMAP)> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.0.is_null() {
            return None;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
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
        match CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bmp) => {
                let _ = SelectObject(dc, bmp.into());
                Some((dc, bmp))
            }
            Err(_) => {
                let _ = DeleteDC(dc);
                None
            }
        }
    }
}

/// 从位图渲染目标的内存 DC 读回 32bpp 像素（BGRX，黑色字形）。
pub(super) fn read_glyphs(dc: HDC, w: u32, h: u32) -> Option<Vec<u8>> {
    unsafe {
        let bmp = GetCurrentObject(dc, OBJ_BITMAP);
        if bmp.0.is_null() {
            return None;
        }
        let hbmp = HBITMAP(bmp.0);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [Default::default()],
        };
        let mut out = vec![0u8; (w as usize * h as usize * 4).max(4)];
        let n = GetDIBits(
            dc,
            hbmp,
            0,
            h,
            Some(out.as_mut_ptr() as *mut core::ffi::c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        if n == 0 {
            return None;
        }
        Some(out)
    }
}

/// 从字形缓冲（32bpp BGRX，黑字白底）按覆盖度混合进目标像素。
#[allow(clippy::too_many_arguments)]
pub(super) fn blend_glyphs(
    pixmap: &mut Pixmap,
    tr: Transform,
    pos: Point,
    pad: u32,
    cw: u32,
    ch: u32,
    px: &[u8],
    color: [u8; 4],
) {
    let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
    let stride = cw as usize * 4;
    let ox = pos.x - pad as f32;
    let oy = pos.y - pad as f32;
    let data = pixmap.data_mut();
    for y in 0..ch {
        let sy = oy + y as f32;
        for x in 0..cw {
            let cov = 255 - px[y as usize * stride + x as usize * 4];
            if cov == 0 {
                continue;
            }
            let sx = ox + x as f32;
            let tx = (tr.sx * sx + tr.kx * sy + tr.tx).round() as i32;
            let ty = (tr.ky * sx + tr.sy * sy + tr.ty).round() as i32;
            if tx < 0 || ty < 0 || tx >= pw || ty >= ph {
                continue;
            }
            let off = (ty as usize * pw as usize + tx as usize) * 4;
            blend_one(&mut data[off..off + 4], color, cov);
        }
    }
}

/// 单像素混合：`cov`（0-255）为字形覆盖度；目标 alpha 恒为不透明。
fn blend_one(d: &mut [u8], color: [u8; 4], cov: u8) {
    let a = color[3] as u32 * cov as u32 / 255;
    let inv = 255 - a;
    for c in 0..3 {
        d[c] = ((d[c] as u32 * inv + color[c] as u32 * a) / 255) as u8;
    }
    d[3] = 255;
}
