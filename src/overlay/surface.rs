// SPDX-License-Identifier: GPL-3.0-only
//! 像素缓冲与上屏（CAP-3、OUT-2）：RGBA↔BGRA 转换、暗化、DIB section、
//! BitBlt 局部上屏、选区裁剪输出。GDI 资源集中创建与释放。

use std::mem::size_of;

use tiny_skia::Pixmap;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection, DEFAULT_GUI_FONT,
    DIB_RGB_COLORS, DeleteDC, GetDC, GetStockObject, HBITMAP, HDC, ReleaseDC, SRCCOPY, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT, TextOutW,
};

use super::SelRect;
use super::geometry::text_rect;

impl super::Overlay {
    /// 上屏指定区域：局部 RGBA → BGRA 写入 DIB section，再用 BitBlt 刷到窗口。
    /// 用 BitBlt 而非 SetDIBitsToDevice：后者对 top-down DIB 的源行语义易出错。
    pub(super) fn present_region(&mut self, r: SelRect) {
        let w = self.display.width() as i32;
        let h = self.display.height() as i32;
        let x0 = r.x.clamp(0, w);
        let y0 = r.y.clamp(0, h);
        let x1 = (r.x + r.w).clamp(0, w);
        let y1 = (r.y + r.h).clamp(0, h);
        let rw = x1 - x0;
        let rh = y1 - y0;
        if rw <= 0 || rh <= 0 {
            return;
        }

        // 局部 RGBA → BGRA，直接写入 DIB section（大面积多线程、小面积串行）
        let area = rw as usize * rh as usize;
        let workers = if area >= 1 << 18 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .clamp(1, 8)
        } else {
            1
        };
        convert_rgba_to_bgra(self.display.data(), self.dib_bits as usize, w, x0, y0, rw, rh, workers);

        unsafe {
            let hdc = GetDC(Some(self.hwnd));
            if hdc.0.is_null() {
                return;
            }
            let _ = BitBlt(hdc, x0, y0, rw, rh, Some(self.mem_dc), x0, y0, SRCCOPY);
            if let Some(sel) = self.selection {
                draw_info_text(hdc, sel, w);
            }
            let _ = ReleaseDC(Some(self.hwnd), hdc);
        }
    }
}

/// GDI 文本：显示选区尺寸与坐标（物理像素），位置由 text_rect 确定。
pub(super) fn draw_info_text(hdc: HDC, sel: SelRect, screen_w: i32) {
    let r = text_rect(sel, screen_w);
    unsafe {
        let font = GetStockObject(DEFAULT_GUI_FONT);
        let _ = SelectObject(hdc, font);
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(0x00FFFFFF));
        let text: Vec<u16> = format!("{} × {}   @({},{})", sel.w, sel.h, sel.x, sel.y)
            .encode_utf16()
            .collect();
        let _ = TextOutW(hdc, r.x + 4, r.y + 4, &text);
    }
}

/// 将 src 的 r 区域拷贝到 dst（原地回贴）。
pub(super) fn blit_region(dst: &mut Pixmap, src: &Pixmap, r: SelRect) {
    let w = dst.width() as i32;
    let h = dst.height() as i32;
    let x0 = r.x.clamp(0, w);
    let y0 = r.y.clamp(0, h);
    let x1 = (r.x + r.w).clamp(0, w);
    let y1 = (r.y + r.h).clamp(0, h);
    let rw = (x1 - x0).max(0);
    let rh = (y1 - y0).max(0);
    for row in 0..rh {
        let off = ((y0 + row) as usize * dst.width() as usize + x0 as usize) * 4;
        dst.data_mut()[off..off + rw as usize * 4].copy_from_slice(&src.data()[off..off + rw as usize * 4]);
    }
}

/// 区域从 src 拷贝到 dst（用于从暗图恢复旧选区区域）。
pub(super) fn restore_region(dst: &mut Pixmap, src: &Pixmap, r: SelRect) {
    blit_region(dst, src, r);
}

/// 暗化像素图：逐通道乘以 k/256，多线程分块并行。
pub(super) fn dim_pixmap(pixmap: &mut Pixmap, k: u32) {
    let data = pixmap.data_mut();
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    if workers == 1 || data.len() < 1 << 20 {
        dim_chunk(data, k);
        return;
    }
    // 每块按 4 字节对齐切分（保证不跨像素）
    let chunk = ((data.len() / workers) + 3) & !3;
    std::thread::scope(|s| {
        for part in data.chunks_mut(chunk) {
            s.spawn(move || dim_chunk(part, k));
        }
    });
}

/// 对一段 RGBA 像素做暗化。
fn dim_chunk(data: &mut [u8], k: u32) {
    for px in data.chunks_exact_mut(4) {
        px[0] = ((px[0] as u32 * k) >> 8) as u8;
        px[1] = ((px[1] as u32 * k) >> 8) as u8;
        px[2] = ((px[2] as u32 * k) >> 8) as u8;
    }
}

/// 创建上屏用 DIB section（BGRA、top-down），返回内存 DC、位图句柄与像素指针。
pub(crate) fn create_dib_surface(w: u32, h: u32) -> Result<(HDC, HBITMAP, *mut u8), String> {
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
        let hbmp = match CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(e) => {
                let _ = DeleteDC(hdc);
                return Err(format!("CreateDIBSection 失败：{e}"));
            }
        };
        let _ = SelectObject(hdc, hbmp.into());
        Ok((hdc, hbmp, bits as *mut u8))
    }
}

/// RGBA → BGRA 写入目标缓冲区：转换 (x0,y0,rw,rh) 区域，按行多线程分块。
/// 目标地址用 usize 传递（裸指针不满足 Send）。
pub(crate) fn convert_rgba_to_bgra(
    src: &[u8],
    dst_addr: usize,
    w: i32,
    x0: i32,
    y0: i32,
    rw: i32,
    rh: i32,
    workers: usize,
) {
    let stride = w as usize * 4;
    let total_rows = rh as usize;
    let convert_row = move |row: usize| {
        let off = (y0 as usize + row) * stride + x0 as usize * 4;
        unsafe {
            let sp = src.as_ptr().add(off) as *const u32;
            let dp = (dst_addr as *mut u8).add(off) as *mut u32;
            for i in 0..rw as usize {
                let v = *sp.add(i);
                *dp.add(i) = (v & 0xFF00_FF00) | ((v & 0x00FF_0000) >> 16) | ((v & 0x0000_00FF) << 16);
            }
        }
    };

    if workers <= 1 {
        for row in 0..total_rows {
            convert_row(row);
        }
        return;
    }
    let row_chunk = total_rows.div_ceil(workers);
    std::thread::scope(|s| {
        for start in (0..total_rows).step_by(row_chunk.max(1)) {
            let rows = (total_rows - start).min(row_chunk);
            s.spawn(move || {
                for row in start..start + rows {
                    convert_row(row);
                }
            });
        }
    });
}

/// 裁剪出选区像素；尺寸非法时返回 None。
pub(super) fn crop_pixmap(src: &Pixmap, r: SelRect) -> Option<Pixmap> {
    let w = (r.w.max(1)) as u32;
    let h = (r.h.max(1)) as u32;
    let mut out = Pixmap::new(w, h)?;
    for (dy, row) in (0..h as i32).enumerate() {
        let sy = r.y + row;
        if sy < 0 || sy >= src.height() as i32 {
            continue;
        }
        let off_src = (sy as usize * src.width() as usize + r.x.max(0) as usize) * 4;
        let off_dst = dy * w as usize * 4;
        out.data_mut()[off_dst..off_dst + w as usize * 4]
            .copy_from_slice(&src.data()[off_src..off_src + w as usize * 4]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Pixmap;

    /// 测量热键到覆盖层可显示前的 CPU 耗时（抓屏 + Pixmap 构建 + 暗化）。
    #[test]
    fn capture_pipeline_timing() {
        let t0 = std::time::Instant::now();
        let screen = crate::capture::capture_virtual_screen().expect("抓屏失败");
        let t1 = std::time::Instant::now();
        let size = tiny_skia::IntSize::from_wh(screen.width, screen.height).unwrap();
        let original = Pixmap::from_vec(screen.rgba, size).unwrap();
        let t2 = std::time::Instant::now();
        let mut dimmed = original.clone();
        dim_pixmap(&mut dimmed, 255 - 120);
        let t3 = std::time::Instant::now();
        println!(
            "抓屏 {:?} | 建 Pixmap {:?} | 暗化 {:?} | 合计 {:?}",
            t1 - t0,
            t2 - t1,
            t3 - t2,
            t3 - t0
        );
    }
}
