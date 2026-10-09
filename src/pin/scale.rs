// SPDX-License-Identifier: GPL-3.0-only
//! 贴图缩放与像素变换（PIN-3）：draw_pixmap 双线性重采样、透明度合成、BGRA 预乘导出。
//! 全部纯函数，无窗口交互，便于单测。

use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

/// 把原始像素图缩放到目标尺寸。
/// 缩小（dw<=sw 且 dh<=sh）用整数区域平均：无混叠、纯整型快（性能优化，M2b 审查后整改）；
/// 放大用双线性插值。源/目标均为 premultiplied 语义（tiny-skia 内部约定）。
pub(super) fn resample(src: &Pixmap, w: u32, h: u32) -> Option<Pixmap> {
    if w == src.width() && h == src.height() {
        return Some(src.clone());
    }
    if w <= src.width() && h <= src.height() {
        resample_box(src, w, h)
    } else {
        resample_bilinear(src, w, h)
    }
}

/// 最近邻放大（滚动预览用，极快）：定点整数映射，无插值；静止后由双线性精修（zoom.rs）。
pub(super) fn resample_nearest(src: &Pixmap, w: u32, h: u32) -> Option<Pixmap> {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (dw, dh) = (w as usize, h as usize);
    let src_data = src.data();
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        let sy = (y as u64 * sh as u64 / dh as u64) as usize;
        let src_row = &src_data[sy * sw * 4..(sy + 1) * sw * 4];
        let dst = &mut out[y * dw * 4..(y + 1) * dw * 4];
        for x in 0..dw {
            let sx = (x as u64 * sw as u64 / dw as u64) as usize;
            dst[x * 4..x * 4 + 4].copy_from_slice(&src_row[sx * 4..sx * 4 + 4]);
        }
    }
    Pixmap::from_vec(out, tiny_skia::IntSize::from_wh(w, h)?)
}

/// 双线性插值缩放（tiny-skia draw_pixmap）。
fn resample_bilinear(src: &Pixmap, w: u32, h: u32) -> Option<Pixmap> {
    let mut out = Pixmap::new(w, h)?;
    out.fill(tiny_skia::Color::TRANSPARENT);
    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..Default::default()
    };
    let tr = Transform::from_scale(w as f32 / src.width() as f32, h as f32 / src.height() as f32);
    out.draw_pixmap(0, 0, src.as_ref(), &paint, tr, None);
    Some(out)
}

/// 整数区域平均降采样：先水平再垂直，两遍各 O(源+目标)；每输出像素 = 源区域均值。
/// premultiplied 域求平均（alpha 一并平均，边缘正确）。
fn resample_box(src: &Pixmap, dw_u: u32, dh_u: u32) -> Option<Pixmap> {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (dw, dh) = (dw_u as usize, dh_u as usize);
    // 水平 pass：每行 sw→dw，中间缓冲
    let mut mid = vec![0u8; sh * dw * 4];
    for y in 0..sh {
        let row = &src.data()[y * sw * 4..(y + 1) * sw * 4];
        box_axis(row, sw, dw, &mut mid[y * dw * 4..(y + 1) * dw * 4]);
    }
    // 垂直 pass：每列 sh→dh
    let mut out = vec![0u8; dw * dh * 4];
    let mut col = vec![0u8; sh * 4];
    let mut tmp = vec![0u8; dh * 4];
    for x in 0..dw {
        for y in 0..sh {
            let o = (y * dw + x) * 4;
            col[y * 4..y * 4 + 4].copy_from_slice(&mid[o..o + 4]);
        }
        box_axis(&col, sh, dh, &mut tmp);
        for y in 0..dh {
            let o = (y * dw + x) * 4;
            out[o..o + 4].copy_from_slice(&tmp[y * 4..y * 4 + 4]);
        }
    }
    Pixmap::from_vec(out, tiny_skia::IntSize::from_wh(dw_u, dh_u)?)
}

/// 单轴 box 平均：把 n 个交错 RGBA 像素平均为 m 个（m <= n）。
fn box_axis(src: &[u8], n: usize, m: usize, dst: &mut [u8]) {
    debug_assert!(m <= n, "放大请走双线性路径");
    let mut s = 0usize;
    for d in 0..m {
        let mut e = (((d + 1) as u64 * n as u64) / m as u64) as usize;
        e = e.max(s + 1).min(n); // 至少覆盖 1 像素，防退化区间
        let mut acc = [0u32; 4];
        for i in s..e {
            for c in 0..4 {
                acc[c] += src[i * 4 + c] as u32;
            }
        }
        let k = (e - s) as u32;
        for c in 0..4 {
            dst[d * 4 + c] = (acc[c] / k) as u8;
        }
        s = e;
    }
}

/// 导出 UpdateLayeredWindow 需要的 BGRA premultiplied 像素：
/// straight RGBA → 每通道预乘（×a/255）→ 乘窗口透明度 alpha 系数 → BGRA。
/// 透明度在 premultiplied 域乘法，RGB 自动按比例收缩，无黑边。
pub(super) fn to_bgra_premul(pixmap: &Pixmap, alpha: u8) -> Vec<u8> {
    let a = alpha as u16;
    let mut out = Vec::with_capacity(pixmap.data().len());
    for px in pixmap.data().chunks_exact(4) {
        let r = px[0] as u16; // 理论上是 premultiplied，这里先乘透明度系数
        let g = px[1] as u16;
        let b = px[2] as u16;
        let pa = px[3] as u16;
        let ka = a * pa / 255; // 合成后的真实 alpha
        out.push((b * ka / 255) as u8);
        out.push((g * ka / 255) as u8);
        out.push((r * ka / 255) as u8);
        out.push(ka as u8);
    }
    out
}

/// straight RGBA → premultiplied RGBA（贴图入库时调用，抵消失真来源）。
pub(super) fn premultiply_rgba(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for px in data.chunks_exact(4) {
        let [r, g, b, a] = [px[0], px[1], px[2], px[3]];
        let ka = a as u16;
        out.push((r as u16 * ka / 255) as u8);
        out.push((g as u16 * ka / 255) as u8);
        out.push((b as u16 * ka / 255) as u8);
        out.push(a);
    }
    out
}

/// 从含边框扩展的显示像素中裁剪内容区并还原 straight（右键复制/保存用）。
/// `pad` 为扩展像素（边框宽），`scale` 为当前缩放；内容偏移按四舍五入取整，
/// 避免缩放后向零截断把边框带进导出（M2b 审查 P1-1）。
pub(super) fn extract_content(scaled: &Pixmap, pad: u32, scale: f32) -> Option<Pixmap> {
    let off = (pad as f32 * scale).round() as u32;
    let cw = scaled.width().saturating_sub(off * 2);
    let ch = scaled.height().saturating_sub(off * 2);
    let mut out = Pixmap::new(cw, ch)?;
    for y in 0..ch {
        let src = &scaled.data()[((y + off) as usize * scaled.width() as usize + off as usize) * 4..];
        let dst = &mut out.data_mut()[y as usize * cw as usize * 4..(y + 1) as usize * cw as usize * 4];
        dst.copy_from_slice(&src[..cw as usize * 4]);
    }
    // premultiplied → straight
    for px in out.data_mut().chunks_exact_mut(4) {
        let a = px[3] as u16;
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else {
            px[0] = (px[0] as u16 * 255 / a) as u8;
            px[1] = (px[1] as u16 * 255 / a) as u8;
            px[2] = (px[2] as u16 * 255 / a) as u8;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_identity_returns_clone() {
        let p = Pixmap::new(10, 8).unwrap();
        let s = resample(&p, 10, 8).unwrap();
        assert_eq!(s.width(), 10);
        assert_eq!(s.height(), 8);
    }

    #[test]
    fn resample_up_and_down() {
        let mut p = Pixmap::new(4, 4).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let big = resample(&p, 16, 16).unwrap();
        assert_eq!((big.width(), big.height()), (16, 16));
        let small = resample(&big, 2, 2).unwrap();
        assert_eq!((small.width(), small.height()), (2, 2));
        // 不透明红：缩放后 alpha 全 255
        assert!(small.data().iter().step_by(4).all(|&a| a == 255));
    }

    #[test]
    fn bgra_premul_opaque_unchanged() {
        let mut p = Pixmap::new(1, 1).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(1, 2, 3, 255));
        let out = to_bgra_premul(&p, 255);
        assert_eq!(out, [3, 2, 1, 255]);
    }

    #[test]
    fn bgra_premul_window_alpha_scales() {
        let mut p = Pixmap::new(1, 1).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(200, 100, 0, 255));
        let out = to_bgra_premul(&p, 128);
        // alpha=128：R 通道 200*128/255≈100，B=0 保持 0
        assert_eq!(out[0], 0);
        assert_eq!(out[1], 50); // 100*128/255 ≈ 50
        assert_eq!(out[2], 100); // 200*128/255 ≈ 100
        assert_eq!(out[3], 128);
    }

    #[test]
    fn premultiply_undoes_straight() {
        let data = [10u8, 20, 30, 128];
        let pm = premultiply_rgba(&data);
        assert_eq!(pm[0], 5); // 10*128/255
        assert_eq!(pm[1], 10); // 20*128/255
        assert_eq!(pm[3], 128);
    }

    /// P1-1 回归：缩放到 0.9 后导出不应带 1px 边框（round 而非截断）。
    #[test]
    fn extract_content_removes_border_after_scale() {
        use crate::pin::border::with_border;
        // 8x8 纯红内容 → 加边框（10x10）→ 0.9 缩放（9x9）→ 导出
        let mut content = Pixmap::new(8, 8).unwrap();
        content.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let framed = with_border(&content, true).unwrap();
        let scaled = resample(&framed, 9, 9).unwrap(); // round(10 * 0.9)
        let out = extract_content(&scaled, 1, 0.9).unwrap();
        // 导出不应有边框蓝：抽查角与中心皆为红色系（R 主导、无蓝边最多容忍插值杂色）
        assert!(
            !out.data().chunks_exact(4).any(|px| px[2] > 200 && px[0] < 40),
            "导出含蓝色边框"
        );
        let center = (out.height() as usize / 2) * out.width() as usize + out.width() as usize / 2;
        let d = out.data();
        assert!(d[center * 4] > 200, "内容中心应红色，实际 {}", d[center * 4]);
    }

    /// 未缩放导出应精确裁剪边框。
    #[test]
    fn extract_content_exact_at_scale_one() {
        use crate::pin::border::with_border;
        let mut content = Pixmap::new(4, 4).unwrap();
        content.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let framed = with_border(&content, true).unwrap(); // 6x6 含边框
        let out = extract_content(&framed, 1, 1.0).unwrap();
        assert_eq!((out.width(), out.height()), (4, 4));
        assert!(
            !out.data().chunks_exact(4).any(|px| px[2] > 200 && px[0] < 40),
            "导出含蓝色边框"
        );
    }

    /// box 降采样：全纯色保持，尺寸正确。
    #[test]
    fn box_downsample_keeps_solid_color() {
        let mut p = Pixmap::new(100, 80).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(30, 160, 90, 255));
        let s = resample(&p, 50, 40).unwrap();
        assert_eq!((s.width(), s.height()), (50, 40));
        assert!(
            s.data()
                .chunks_exact(4)
                .all(|px| px[0] == 30 && px[1] == 160 && px[2] == 90 && px[3] == 255)
        );
    }

    /// box 降采样：四象限区域平均正确（而非点采样）。
    #[test]
    fn box_downsample_averages_region() {
        let mut p = Pixmap::new(4, 4).unwrap();
        p.fill(tiny_skia::Color::TRANSPARENT);
        for y in 0..4usize {
            for x in 0..4usize {
                let o = (y * 4 + x) * 4;
                p.data_mut()[o..o + 4].copy_from_slice(match (y < 2, x < 2) {
                    (true, true) => &[255, 0, 0, 255],       // 左上红
                    (true, false) => &[0, 255, 0, 255],      // 右上绿
                    (false, true) => &[0, 0, 255, 255],      // 左下蓝
                    (false, false) => &[255, 255, 255, 255], // 右下白
                });
            }
        }
        let s = resample(&p, 2, 2).unwrap();
        let d = s.data();
        assert_eq!(&d[0..4], &[255, 0, 0, 255]);
        assert_eq!(&d[4..8], &[0, 255, 0, 255]);
        assert_eq!(&d[8..12], &[0, 0, 255, 255]);
        assert_eq!(&d[12..16], &[255, 255, 255, 255]);
    }

    /// 最近邻放大：2x 时每个源像素复制成 2x2 块。
    #[test]
    fn nearest_up_duplicates_blocks() {
        let mut p = Pixmap::new(2, 2).unwrap();
        let (r, g) = ([10u8, 20, 30, 255], [40, 50, 60, 255]);
        p.data_mut()[0..4].copy_from_slice(&r); // src(0,0)
        p.data_mut()[4..8].copy_from_slice(&g); // src(1,0)
        p.data_mut()[8..12].copy_from_slice(&g); // src(0,1)
        p.data_mut()[12..16].copy_from_slice(&r); // src(1,1)
        let s = resample_nearest(&p, 4, 4).unwrap();
        let d = s.data();
        // 4x4 = 每个源像素 2x2 块；坐标 out(x,y)=src(x/2,y/2)
        assert_eq!(&d[0..4], &r); // out(0,0)
        assert_eq!(&d[4..8], &r); // out(1,0) 同块
        assert_eq!(&d[8..12], &g); // out(2,0)
        assert_eq!(&d[16..20], &r); // out(0,1) 同块
        assert_eq!(&d[4 * 4 + 4..4 * 4 + 8], &r); // out(1,1) 同块（源 0,0）
    }
}
