// SPDX-License-Identifier: GPL-3.0-only
//! 剪贴板读图（PIN-2）：优先 "PNG" 注册格式 → CF_DIB → CF_BITMAP 三级回退。
//! 只读不改剪贴板内容；解码在调用线程同步完成（贴图数据量小，无需后台）。

use std::io::Cursor;
use std::mem::size_of;

use tiny_skia::Pixmap;
use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteObject, GetDIBits,
    GetObjectW, HBITMAP, HGDIOBJ, SelectObject,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize};
use windows::Win32::System::Ole::{CF_BITMAP, CF_DIB};
use windows::core::w;

/// 读取剪贴板图片，失败返回错误信息（格式不支持 / 内容损坏）。
pub(super) fn read_image() -> Result<Pixmap, String> {
    unsafe {
        if OpenClipboard(None).is_err() {
            return Err("无法打开剪贴板".into());
        }
        let fail = |e: String| {
            let _ = CloseClipboard();
            e
        };

        // 1) "PNG" 注册格式（现代程序偏好，直接 PNG 字节解码）
        let png_fmt = RegisterClipboardFormatW(w!("PNG"));
        if png_fmt != 0
            && IsClipboardFormatAvailable(png_fmt).is_ok()
            && let Ok(h) = GetClipboardData(png_fmt)
            && let Some(pm) = try_png(h)
        {
            let _ = CloseClipboard();
            return Ok(pm);
        }

        // 2) CF_DIB（BITMAPINFOHEADER + 像素）
        if IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok()
            && let Ok(h) = GetClipboardData(CF_DIB.0 as u32)
            && let Some(pm) = try_dib(h)
        {
            let _ = CloseClipboard();
            return Ok(pm);
        }

        // 3) CF_BITMAP（GDI 位图句柄）
        if IsClipboardFormatAvailable(CF_BITMAP.0 as u32).is_ok()
            && let Ok(h) = GetClipboardData(CF_BITMAP.0 as u32)
            && let Some(pm) = try_gdi_bitmap(h)
        {
            let _ = CloseClipboard();
            return Ok(pm);
        }

        Err(fail("剪贴板中没有图片".into()))
    }
}

/// 锁内临时读取剪贴板数据并计算（数据仅在调用返回前有效，故用闭包而非返回切片；审查 P3-2）。
unsafe fn with_data<T>(h: HANDLE, f: impl FnOnce(&[u8]) -> Option<T>) -> Option<T> {
    unsafe {
        let hg = HGLOBAL(h.0);
        let ptr = GlobalLock(hg);
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr as *const u8, GlobalSize(hg) as usize);
        let out = f(bytes);
        // 剪贴板数据由 CloseClipboard 统一释放，此处无需 GlobalUnlock
        out
    }
}

/// PNG 字节解码（"PNG" 注册格式）。
unsafe fn try_png(h: HANDLE) -> Option<Pixmap> {
    unsafe {
        with_data(h, |bytes| {
            let mut dec = png::Decoder::new(Cursor::new(bytes));
            dec.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::EXPAND);
            let mut reader = dec.read_info().ok()?;
            let (w, h) = reader.info().size();
            let w = w as u32;
            let h = h as u32;
            let needed = w as usize * h as usize * 4;
            let mut data = vec![0u8; reader.output_buffer_size().unwrap_or(needed)];
            reader.next_frame(&mut data).ok()?;
            data.truncate(needed);
            let mut pm = Pixmap::new(w, h)?;
            match reader.info().color_type {
                png::ColorType::Rgba => pm.data_mut().copy_from_slice(&data),
                png::ColorType::Rgb => {
                    for (dst, src) in pm.data_mut().chunks_exact_mut(4).zip(data.chunks_exact(3)) {
                        dst[0] = src[0];
                        dst[1] = src[1];
                        dst[2] = src[2];
                        dst[3] = 255;
                    }
                }
                _ => return None,
            }
            Some(pm)
        })
    }
}

/// CF_DIB：解析 BITMAPINFOHEADER + 像素（32/24bpp、BI_RGB、上下行皆支持）。
unsafe fn try_dib(h: HANDLE) -> Option<Pixmap> {
    unsafe {
        with_data(h, |bytes| {
            if bytes.len() < size_of::<BITMAPINFOHEADER>() {
                return None;
            }
            let header: BITMAPINFOHEADER = std::ptr::read_unaligned(bytes.as_ptr() as *const BITMAPINFOHEADER);
            decode_dib(bytes, &header)
        })
    }
}

/// 从 DIB 字节解码为 Pixmap（无剪贴板依赖，便于单测）。
fn decode_dib(bytes: &[u8], header: &BITMAPINFOHEADER) -> Option<Pixmap> {
    let bi_size = header.biSize as usize;
    if bytes.len() < bi_size + 4 {
        return None;
    }
    let w = header.biWidth.max(0) as u32;
    let h_abs = header.biHeight.abs().max(0) as u32;
    if w == 0 || h_abs == 0 || header.biPlanes != 1 {
        return None;
    }
    let top_down = header.biHeight < 0;
    let bpp = header.biBitCount;
    if !matches!(bpp, 32 | 24) || (bpp == 24 && header.biCompression != BI_RGB.0) {
        return None; // BITFIELDS/RLE 等压缩位图不支持（24bpp 无压缩）
    }
    let px_bytes = bpp as usize / 8;
    let row_bytes = align4(w as usize * px_bytes);
    if bi_size + row_bytes * h_abs as usize > bytes.len() {
        return None; // 长度不符，防越界
    }

    // 32bpp：alpha 大面积全 0 时按不透明处理（部分程序写入 DIB 但 alpha 无意义）
    let mut use_alpha = bpp == 32;
    if use_alpha {
        use_alpha = false;
        for x in 0..w.min(64) as usize {
            if bytes[bi_size + x * 4 + 3] != 0 {
                use_alpha = true;
                break;
            }
        }
    }

    let mut pm = Pixmap::new(w, h_abs)?;
    let stride = w as usize * 4;
    for y in 0..h_abs as usize {
        let src_row = if top_down { y } else { h_abs as usize - 1 - y };
        let row = &bytes[bi_size + src_row * row_bytes..];
        let dst = &mut pm.data_mut()[y * stride..(y + 1) * stride];
        for (i, px) in dst.chunks_exact_mut(4).enumerate() {
            let off = i * px_bytes;
            px[0] = row[off + 2];
            px[1] = row[off + 1];
            px[2] = row[off];
            px[3] = if use_alpha { row[off + 3] } else { 255 };
        }
    }
    Some(pm)
}

/// CF_BITMAP：GDI 位图句柄 → GetDIBits 提取 32bpp 像素。
unsafe fn try_gdi_bitmap(h: HANDLE) -> Option<Pixmap> {
    unsafe {
        let hbmp = HBITMAP(h.0);
        let mut bm = BITMAP::default();
        if GetObjectW(
            HGDIOBJ(hbmp.0),
            size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut _ as *mut core::ffi::c_void),
        ) == 0
        {
            return None;
        }
        let (w, h) = (bm.bmWidth.max(0) as u32, bm.bmHeight.abs() as u32);
        if w == 0 || h == 0 {
            return None;
        }
        let mem_dc = CreateCompatibleDC(None);
        if mem_dc.0.is_null() {
            return None;
        }
        let _ = SelectObject(mem_dc, HGDIOBJ(hbmp.0));
        let mut pm = Pixmap::new(w, h)?;
        let mut bmi = BITMAPINFO {
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
        let got = GetDIBits(
            mem_dc,
            hbmp,
            0,
            h,
            Some(pm.data_mut().as_mut_ptr() as *mut core::ffi::c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        if got == 0 || got as u32 != h {
            return None;
        }
        // GetDIBits 输出 BGRA → RGBA，alpha 无意义置 255
        for px in pm.data_mut().chunks_exact_mut(4) {
            let (b, g, r) = (px[0], px[1], px[2]);
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = 255;
        }
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        Some(pm)
    }
}

fn align4(n: usize) -> usize {
    (n + 3) / 4 * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(w: i32, h: i32, bpp: u16, comp: u32) -> BITMAPINFOHEADER {
        BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: h,
            biPlanes: 1,
            biBitCount: bpp,
            biCompression: comp,
            ..Default::default()
        }
    }

    fn to_bytes(hdr: &BITMAPINFOHEADER) -> Vec<u8> {
        let mut out = vec![0u8; hdr.biSize as usize];
        unsafe {
            std::ptr::copy_nonoverlapping(hdr as *const _ as *const u8, out.as_mut_ptr(), hdr.biSize as usize);
        }
        out
    }

    #[test]
    fn dib_24bpp_bottom_up() {
        // 3x2 24bpp：bottom-up，每行 12 字节（9 像素 + 3 对齐）。
        // 数据序：行0=蓝绿红+pad，行1=红绿蓝+pad；图上首行应为红绿蓝。
        let hdr = header(3, -2, 24, BI_RGB.0);
        let mut bytes = to_bytes(&hdr);
        bytes.extend_from_slice(&[0, 0, 255, 0, 255, 0, 255, 0, 0, 0, 0, 0]); // 行0（图底）：蓝 绿 红
        bytes.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0]); // 行1（图顶）：红 绿 蓝
        let pm = decode_dib(&bytes, &hdr).unwrap();
        assert_eq!((pm.width(), pm.height()), (3, 2));
        let d = pm.data();
        assert_eq!(&d[0..4], &[255, 0, 0, 255]); // 首行红
        assert_eq!(&d[4..8], &[0, 255, 0, 255]);
        assert_eq!(&d[8..12], &[0, 0, 255, 255]);
    }

    #[test]
    fn dib_32bpp_alpha_probe() {
        let hdr = header(2, 1, 32, BI_RGB.0);
        let mut bytes = to_bytes(&hdr);
        bytes.extend_from_slice(&[1, 2, 3, 0, 4, 5, 6, 0]); // alpha 全 0 → 不透明
        let pm = decode_dib(&bytes, &hdr).unwrap();
        assert_eq!(&pm.data()[0..4], &[3, 2, 1, 255]);
        assert_eq!(&pm.data()[4..8], &[6, 5, 4, 255]);

        let hdr2 = header(1, 1, 32, BI_RGB.0);
        let mut bytes2 = to_bytes(&hdr2);
        bytes2.extend_from_slice(&[1, 2, 3, 128]); // 有真实 alpha → 保留
        let pm2 = decode_dib(&bytes2, &hdr2).unwrap();
        assert_eq!(&pm2.data()[0..4], &[3, 2, 1, 128]);
    }

    #[test]
    fn dib_24bpp_row_padding() {
        let hdr = header(2, 1, 24, BI_RGB.0);
        let mut bytes = to_bytes(&hdr);
        bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6, 0, 0]); // BGR BGR pad pad
        let pm = decode_dib(&bytes, &hdr).unwrap();
        assert_eq!(&pm.data()[0..4], &[3, 2, 1, 255]);
        assert_eq!(&pm.data()[4..8], &[6, 5, 4, 255]);
    }

    #[test]
    fn dib_truncated_rejected() {
        let hdr = header(100, 100, 32, BI_RGB.0);
        let bytes = vec![0u8; 64];
        assert!(decode_dib(&bytes, &hdr).is_none());
    }
}
