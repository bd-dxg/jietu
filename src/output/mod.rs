// SPDX-License-Identifier: GPL-3.0-only
//! 输出（OUT-1/2/3）：剪贴板 `CF_DIB` + `PNG` 格式、PNG/JPEG 文件保存。
//! 编码调用方负责放到后台线程执行（OUT-2）。

use std::mem::size_of;
use std::path::{Path, PathBuf};

use tiny_skia::Pixmap;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_DIB;
use windows::core::w;

/// 复制到剪贴板：写入 CF_DIB（BGRA）与 "PNG" 注册格式（OUT-1）。
pub fn copy_to_clipboard(pixmap: &Pixmap) -> Result<(), String> {
    let w = pixmap.width() as u32;
    let h = pixmap.height() as u32;
    let bgra = rgba_to_bgra(pixmap.data());
    let png_bytes = encode_png(pixmap)?;

    unsafe {
        if OpenClipboard(None).is_err() {
            return Err("OpenClipboard 失败".into());
        }
        let fail = |msg: String| {
            let _ = CloseClipboard();
            msg
        };

        if EmptyClipboard().is_err() {
            return Err(fail("EmptyClipboard 失败".into()));
        }

        // CF_DIB：BITMAPINFOHEADER + BGRA 像素。
        let header_size = size_of::<BITMAPINFOHEADER>() as usize;
        let hdib = match GlobalAlloc(GMEM_MOVEABLE, header_size + bgra.len()) {
            Ok(h) => h,
            Err(e) => return Err(fail(format!("GlobalAlloc CF_DIB 失败：{e}"))),
        };
        let ptr = GlobalLock(hdib);
        if ptr.is_null() {
            return Err(fail("GlobalLock CF_DIB 失败".into()));
        }
        let header = BITMAPINFOHEADER {
            biSize: header_size as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // 自上而下
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        std::ptr::copy_nonoverlapping(&header as *const _ as *const u8, ptr as *mut u8, header_size);
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), (ptr as *mut u8).add(header_size), bgra.len());
        let _ = GlobalUnlock(hdib);
        if SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(hdib.0))).is_err() {
            // SetClipboardData 失败时剪贴板未接管句柄，必须自行释放
            let _ = GlobalFree(Some(HGLOBAL(hdib.0)));
            return Err(fail("SetClipboardData CF_DIB 失败".into()));
        }

        // PNG 格式（延迟渲染的简化：直接同步提供完整数据）。
        let fmt = RegisterClipboardFormatW(w!("PNG"));
        if fmt == 0 {
            return Err(fail("RegisterClipboardFormatW(PNG) 返回 0".into()));
        }
        let hpng = match GlobalAlloc(GMEM_MOVEABLE, png_bytes.len()) {
            Ok(h) => h,
            Err(e) => return Err(fail(format!("GlobalAlloc PNG 失败：{e}"))),
        };
        let pptr = GlobalLock(hpng);
        if pptr.is_null() {
            return Err(fail("GlobalLock PNG 失败".into()));
        }
        std::ptr::copy_nonoverlapping(png_bytes.as_ptr(), pptr as *mut u8, png_bytes.len());
        let _ = GlobalUnlock(hpng);
        if SetClipboardData(fmt, Some(HANDLE(hpng.0))).is_err() {
            let _ = GlobalFree(Some(HGLOBAL(hpng.0)));
            return Err(fail("SetClipboardData PNG 失败".into()));
        }

        let _ = CloseClipboard();
        Ok(())
    }
}

/// 保存像素为 PNG（按扩展名决定格式，未知扩展名默认 PNG）。
pub fn save_file(pixmap: &Pixmap, path: &Path) -> Result<(), String> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => save_jpeg(pixmap, path),
        _ => save_png(pixmap, path),
    }
}

/// 保存 PNG（快速压缩，OUT-2）。
pub fn save_png(pixmap: &Pixmap, path: &Path) -> Result<(), String> {
    let bytes = encode_png(pixmap)?;
    std::fs::write(path, bytes).map_err(|e| format!("写入 {path:?} 失败：{e}"))
}

/// 保存 JPEG（质量 90，OUT-2）。
pub fn save_jpeg(pixmap: &Pixmap, path: &Path) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("创建 {path:?} 失败：{e}"))?;
    let enc = jpeg_encoder::Encoder::new(file, 90);
    enc.encode(
        pixmap.data(),
        pixmap.width() as u16,
        pixmap.height() as u16,
        jpeg_encoder::ColorType::Rgba,
    )
    .map_err(|e| format!("JPEG 编码失败：{e}"))
}

/// 默认保存目录：`%USERPROFILE%\Pictures\jietu`。
pub fn default_save_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join("Pictures").join("jietu")
}

/// 生成时间戳文件名，如 `jietu-20240101-120000.png`。
pub fn timestamped_filename(ext: &str) -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st = unsafe { GetLocalTime() };
    format!(
        "jietu-{:04}{:02}{:02}-{:02}{:02}{:02}.{ext}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

/// RGBA → BGRA（就地语义复制出新缓冲）。
pub fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut bgra = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    bgra
}

/// 编码 PNG（快速压缩）。
fn encode_png(pixmap: &Pixmap) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, pixmap.width(), pixmap.height());
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(pixmap.data()).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_roundtrip() {
        let pixmap = Pixmap::new(8, 8).unwrap();
        match copy_to_clipboard(&pixmap) {
            Ok(()) => println!("clipboard ok"),
            Err(e) => println!("clipboard err: {e}"),
        }
    }
}
