// SPDX-License-Identifier: GPL-3.0-only
//! DirectWrite 文本测量与栅格化（EDT-4）：`measure` 得到文字尺寸（写入对象测量缓存），
//! `draw` 把文本栅格化进 tiny_skia 像素图。
//!
//! 渲染路径：IDWriteTextLayout 布局 → 自定义 `IDWriteTextRenderer` 接收纵向字形 →
//! `IDWriteBitmapRenderTarget` 栅格化进内存 DIB（黑字白底）→ 反色取覆盖度与目标色混合。
//! 全程纯 CPU、无窗口依赖；DirectWrite 初始化失败时返回估算测量并静默跳过绘制（不崩溃）。

use tiny_skia::{Pixmap, Transform};
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::{DeleteDC, DeleteObject, GdiFlush, HBITMAP, HDC, PatBlt, WHITENESS};
use windows::core::{BOOL, IUnknown, Ref, Result, implement, w};

use crate::editor::Point;

/// 中文字体（Win10/11 自带；EDT-4 一期固定，预留字体字段扩展）。
/// 布局构造的宽高上限：单行文本，禁止自动换行。
const MAX_LAYOUT: f32 = 100_000.0;

/// 共享工厂缓存（DWrite SHARED 工厂线程安全，`IDWriteFactory` 实现了 Send+Sync）。
fn factory() -> Option<&'static IDWriteFactory> {
    static F: std::sync::OnceLock<Option<IDWriteFactory>> = std::sync::OnceLock::new();
    F.get_or_init(|| unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok() })
        .as_ref()
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// DirectWrite 不可用时的估算（供 bounds 使用，保证不崩溃）。
fn approx(text: &str, font_size: f32) -> (f32, f32) {
    let n = text.encode_utf16().count() as f32;
    ((n * font_size * 0.9 + font_size).max(1.0), font_size * 1.4)
}

fn create_format(f: &IDWriteFactory, font_size: f32) -> Option<IDWriteTextFormat> {
    unsafe {
        f.CreateTextFormat(
            w!("Microsoft YaHei UI"),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            font_size,
            w!("zh-CN"),
        )
    }
    .ok()
}

/// 测量文本尺寸（物理像素，单行）。失败时给估算值。
pub fn measure(text: &str, font_size: f32) -> (f32, f32) {
    let Some(f) = factory() else {
        return approx(text, font_size);
    };
    let Some(format) = create_format(f, font_size) else {
        return approx(text, font_size);
    };
    let Ok(layout) = (unsafe { f.CreateTextLayout(&utf16(text), &format, MAX_LAYOUT, MAX_LAYOUT) }) else {
        return approx(text, font_size);
    };
    let mut m = DWRITE_TEXT_METRICS::default();
    if unsafe { layout.GetMetrics(&mut m) }.is_ok() {
        // 多行（\n 换行）时 width 为布局宽度（最长行），height 为总行高
        return (m.width.max(1.0), m.height.max(1.0));
    }
    approx(text, font_size)
}

/// 自定义文字器：只把字形转发给位图渲染目标（黑字白底），忽略下划线/删除线。
// 实现目标为 `Renderer_Impl`（windows-implement 0.60 约定），字段经 `this` 访问。
#[implement(IDWriteTextRenderer)]
struct Renderer {
    target: IDWriteBitmapRenderTarget,
    params: IDWriteRenderingParams,
}

impl IDWritePixelSnapping_Impl for Renderer_Impl {
    fn IsPixelSnappingDisabled(&self, _: *const core::ffi::c_void) -> Result<BOOL> {
        Ok(false.into())
    }

    fn GetCurrentTransform(&self, _: *const core::ffi::c_void, transform: *mut DWRITE_MATRIX) -> Result<()> {
        unsafe {
            *transform = DWRITE_MATRIX {
                m11: 1.0,
                m12: 0.0,
                m21: 0.0,
                m22: 1.0,
                dx: 0.0,
                dy: 0.0,
            };
        }
        Ok(())
    }

    fn GetPixelsPerDip(&self, _: *const core::ffi::c_void) -> Result<f32> {
        Ok(1.0)
    }
}

impl IDWriteTextRenderer_Impl for Renderer_Impl {
    fn DrawGlyphRun(
        &self,
        _ctx: *const core::ffi::c_void,
        x: f32,
        y: f32,
        mode: DWRITE_MEASURING_MODE,
        run: *const DWRITE_GLYPH_RUN,
        _desc: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _effect: Ref<IUnknown>,
    ) -> Result<()> {
        unsafe {
            self.this
                .target
                .DrawGlyphRun(x, y, mode, run, &self.this.params, COLORREF(0), None)
        }?;
        Ok(())
    }

    fn DrawUnderline(
        &self,
        _: *const core::ffi::c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_UNDERLINE,
        _: Ref<IUnknown>,
    ) -> Result<()> {
        Ok(())
    }

    fn DrawStrikethrough(
        &self,
        _: *const core::ffi::c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_STRIKETHROUGH,
        _: Ref<IUnknown>,
    ) -> Result<()> {
        Ok(())
    }

    fn DrawInlineObject(
        &self,
        _: *const core::ffi::c_void,
        _: f32,
        _: f32,
        _: Ref<IDWriteInlineObject>,
        _: BOOL,
        _: BOOL,
        _: Ref<IUnknown>,
    ) -> Result<()> {
        Ok(())
    }
}

/// 栅格化文本并混合进 `pixmap`（`tr` 用于导出烘焙的坐标平移）。
/// `pos` 为文字左上角。
pub(crate) fn draw(pixmap: &mut Pixmap, pos: Point, text: &str, font_size: f32, color: [u8; 4], tr: Transform) {
    let Some(f) = factory() else { return };
    let (w, h) = measure(text, font_size);
    let pad = crate::editor::text::pad(font_size).ceil() as u32;
    let cw = (w.ceil() as u32 + pad * 2).max(4);
    let ch = (h.ceil() as u32 + pad * 2).max(4);

    // 内存 DIB（top-down BGRA），DirectWrite 栅格化画布
    let Some((dc, bmp)) = super::text_bitmap::create_dib(cw, ch) else {
        return;
    };
    let cleanup = |dc: HDC, bmp: HBITMAP| unsafe {
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
    };
    let target = match unsafe {
        f.GetGdiInterop()
            .and_then(|i| i.CreateBitmapRenderTarget(Some(dc), cw, ch))
    } {
        Ok(t) => t,
        Err(_) => {
            cleanup(dc, bmp);
            return;
        }
    };
    if unsafe { target.SetPixelsPerDip(1.0) }.is_err() {
        cleanup(dc, bmp);
        return;
    }
    // 渲染目标的内存 DC：白化 + 读回共用这一个句柄
    let rt_dc = unsafe { target.GetMemoryDC() };
    unsafe {
        let _ = PatBlt(rt_dc, 0, 0, cw as i32, ch as i32, WHITENESS);
    }
    let (Some(format), Ok(params)) = (create_format(f, font_size), unsafe { f.CreateRenderingParams() }) else {
        cleanup(dc, bmp);
        return;
    };
    let Ok(layout) = (unsafe { f.CreateTextLayout(&utf16(text), &format, MAX_LAYOUT, MAX_LAYOUT) }) else {
        cleanup(dc, bmp);
        return;
    };
    let renderer = Renderer { target, params };
    let renderer: IDWriteTextRenderer = renderer.into();
    // origin 取 (pad, pad)：字形在 DIB 中相对四周内边距居中，blend 时映射回 pos 即为文字左上角
    if unsafe { layout.Draw(None, &renderer, pad as f32, pad as f32) }.is_err() {
        cleanup(dc, bmp);
        return;
    }
    let _ = unsafe { GdiFlush() };
    // 字形绘制在 render target 内部位图上，用 GetDIBits 读回像素
    let Some(px) = super::text_bitmap::read_glyphs(rt_dc, cw, ch) else {
        cleanup(dc, bmp);
        return;
    };
    super::text_bitmap::blend_glyphs(pixmap, tr, pos, pad, cw, ch, &px, color);
    cleanup(dc, bmp);
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::DirectWrite::DWRITE_FACTORY_TYPE_SHARED;

    #[test]
    fn measure_handles_chinese_and_latin() {
        let (w1, h1) = measure("挺好", 24.0);
        let (w2, h2) = measure("ab", 24.0);
        assert!(w1 > 0.0 && h1 > 0.0, "中文测量应非零：{w1}x{h1}");
        assert!(w2 > 0.0, "拉丁字符测量应非零");
        assert!(w1 >= w2, "两个全角字应不窄于两个半角字符");
        assert!((h1 - h2).abs() < 8.0, "同行字号高度应接近：{h1} vs {h2}");
    }

    #[test]
    fn larger_font_measures_larger() {
        let (w1, _) = measure("测试", 16.0);
        let (w2, _) = measure("测试", 48.0);
        assert!(w2 > w1 * 2.0, "字号 3 倍宽度应近似 3 倍：{w1} vs {w2}");
    }

    #[test]
    fn draw_produces_white_pixels_on_black() {
        let mut p = Pixmap::new(120, 80).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(0, 0, 0, 255));
        draw(
            &mut p,
            Point::new(10.0, 10.0),
            "测试",
            24.0,
            [255, 255, 255, 255],
            Transform::identity(),
        );
        let any = p.data().chunks_exact(4).any(|px| px[0] > 0);
        assert!(any, "黑底上应绘出白色字形");
    }

    #[test]
    fn draw_translates_with_transform() {
        let mut p = Pixmap::new(200, 100).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(0, 0, 0, 255));
        let tr = Transform::from_translate(50.0, 20.0);
        draw(&mut p, Point::new(10.0, 10.0), "测试", 24.0, [255, 255, 255, 255], tr);
        // 平移后字形应出现在右侧区域（x ≥ 50），原位置（x < 50）应无像素
        let left = p
            .data()
            .chunks_exact(4)
            .enumerate()
            .filter(|(i, px)| {
                let x = (i % 200) as i32;
                x < 50 && px[0] > 0
            })
            .count();
        let right = p
            .data()
            .chunks_exact(4)
            .enumerate()
            .filter(|(i, px)| {
                let x = (i % 200) as i32;
                x >= 50 && px[0] > 0
            })
            .count();
        assert_eq!(left, 0, "平移后左侧不应有字形");
        assert!(right > 0, "平移后右侧应有字形");
    }

    #[test]
    fn direct_write_factory_available() {
        let f = unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) };
        assert!(f.is_ok(), "桌面会话应能创建 DirectWrite 工厂");
    }
}
