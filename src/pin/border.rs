// SPDX-License-Identifier: GPL-3.0-only
//! 贴图边框（M2b）：内容四周 1px 蓝色描边，烘焙进窗口像素（分层窗口无系统边框可用）。
//! 替代早期的发光阴影方案 —— 用户反馈阴影难以调出自然效果，改为简洁的 1px 蓝框。

use tiny_skia::Pixmap;

/// 四周扩展像素（边框 1px，内容区在画布 (1,1) 起）。
pub const BORDER_PAD: u32 = 1;

/// 边框蓝（DodgerBlue，深色/浅色桌面上都清晰）。
const BORDER_COLOR: [u8; 4] = [0x1E, 0x90, 0xFF, 255];

/// 给 premultiplied 内容图生成带 1px 蓝色边框的扩展像素图；
/// `on == false` 时原样返回（边框关闭）。
/// 返回图尺寸 = (w + 2, h + 2)，内容位于左上角 (1, 1)，边框为外圈一行像素。
pub(super) fn with_border(src: &Pixmap, on: bool) -> Option<Pixmap> {
    if !on {
        return Some(src.clone());
    }
    let (w, h) = (src.width(), src.height());
    let (ow, oh) = (w + 2, h + 2);
    let mut out = Pixmap::new(ow, oh)?;
    out.fill(tiny_skia::Color::TRANSPARENT);
    let data = out.data_mut();

    // 上边 / 下边整行
    for off in [0usize, (oh - 1) as usize * ow as usize] {
        for x in 0..ow as usize {
            let o = (off + x) * 4;
            data[o..o + 4].copy_from_slice(&BORDER_COLOR);
        }
    }
    // 左边 / 右边（跳过已填的角落行）
    for y in 1..(oh - 1) as usize {
        let row = y * ow as usize;
        for x in [0usize, ow as usize - 1] {
            let o = (row + x) * 4;
            data[o..o + 4].copy_from_slice(&BORDER_COLOR);
        }
    }

    // 内容盖回（src 已 premultiplied，直接拷贝）
    for y in 0..h {
        let src_row = &src.data()[y as usize * w as usize * 4..(y + 1) as usize * w as usize * 4];
        let dst = &mut out.data_mut()[(1 + y) as usize * ow as usize * 4..(2 + y) as usize * ow as usize * 4];
        dst[4..(w + 1) as usize * 4].copy_from_slice(src_row);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Color;

    #[test]
    fn off_returns_same_size() {
        let mut p = Pixmap::new(4, 4).unwrap();
        p.fill(Color::from_rgba8(10, 20, 30, 255));
        let out = with_border(&p, false).unwrap();
        assert_eq!((out.width(), out.height()), (4, 4));
    }

    #[test]
    fn border_expands_and_frames_content() {
        let mut p = Pixmap::new(8, 8).unwrap();
        p.fill(Color::from_rgba8(200, 100, 50, 255));
        let out = with_border(&p, true).unwrap();
        assert_eq!((out.width(), out.height()), (10, 10));
        let d = out.data();
        // 外圈四条边是蓝色
        for pos in [0usize, 9, 9 * 10, 9 * 10 + 9] {
            let o = pos * 4;
            assert_eq!(&d[o..o + 4], &[0x1E, 0x90, 0xFF, 255], "边框角 (pos {pos}) 应为蓝");
        }
        let side = 5 * 10 + 0; // 左边中点
        assert_eq!(&d[side * 4..side * 4 + 4], &[0x1E, 0x90, 0xFF, 255]);
        // 内容中心保持原色
        let cx = (1 + 4) * 10 + (1 + 4);
        assert_eq!(d[cx * 4 + 3], 255);
        assert_eq!(d[cx * 4], 200); // R
    }

    #[test]
    fn transparent_content_still_framed() {
        let mut p = Pixmap::new(8, 8).unwrap();
        p.fill(tiny_skia::Color::TRANSPARENT);
        let out = with_border(&p, true).unwrap();
        // 边框仍在，内容区透明
        let d = out.data();
        assert_eq!(&d[0..4], &[0x1E, 0x90, 0xFF, 255]);
        let cx = 5 * 10 + 5;
        assert_eq!(d[cx * 4 + 3], 0);
    }
}
