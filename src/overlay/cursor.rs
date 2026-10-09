// SPDX-License-Identifier: GPL-3.0-only
//! 光标（CAP-2）：白十字（默认）与自绘抓手（按住空格移动选区时）。

use std::mem::size_of;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GCLP_HCURSOR, GetClassLongPtrW, HCURSOR, ICONINFO, SetCursor,
};

/// 创建白十字 + 黑描边彩色光标（32bpp alpha），保证在暗化画面上清晰可见。
/// 结果缓存进 static：窗口类可多次注册（截图每次重注册），直接新建会泄漏 GDI 图标句柄。
pub(super) fn create_cross_cursor() -> HCURSOR {
    let cached = CROSS_CURSOR.load(Ordering::Relaxed);
    if cached != 0 {
        return HCURSOR(cached as *mut core::ffi::c_void);
    }
    let cur = build_cross_cursor();
    CROSS_CURSOR.store(cur.0 as isize, Ordering::Relaxed);
    cur
}

/// 十字光标句柄缓存（进程内只创建一次，避免每次截图泄漏 GDI 句柄）。
static CROSS_CURSOR: AtomicIsize = AtomicIsize::new(0);

/// 实际创建十字光标。
fn build_cross_cursor() -> HCURSOR {
    const S: i32 = 32;
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: S,
                biHeight: -S,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [Default::default()],
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let hbm_color = match CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(_) => return HCURSOR::default(),
        };

        let px = std::slice::from_raw_parts_mut(bits as *mut u8, (S * S * 4) as usize);
        let mut set = |x: i32, y: i32, r: u8, g: u8, b: u8, a: u8| {
            if x < 0 || y < 0 || x >= S || y >= S {
                return;
            }
            let i = ((y * S + x) * 4) as usize;
            px[i] = b;
            px[i + 1] = g;
            px[i + 2] = r;
            px[i + 3] = a;
        };
        let (cx, cy, arm) = (S / 2, S / 2, 14);
        // 黑色描边（白线两侧各 1px）
        for d in -arm..=arm {
            for w in -1i32..=1 {
                set(cx + d, cy + w, 0, 0, 0, 255);
                set(cx + w, cy + d, 0, 0, 0, 255);
            }
        }
        // 白色细十字（1px 宽）
        for d in -arm..=arm {
            set(cx + d, cy, 255, 255, 255, 255);
            set(cx, cy + d, 255, 255, 255, 255);
        }

        let mask = CreateBitmap(S, S, 1, 1, None);
        let info = ICONINFO {
            fIcon: false.into(),
            xHotspot: cx as u32,
            yHotspot: cy as u32,
            hbmMask: mask,
            hbmColor: hbm_color,
        };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(hbm_color.into());
        if !mask.0.is_null() {
            let _ = DeleteObject(mask.into());
        }
        HCURSOR(icon.0)
    }
}

/// 切换当前光标：按住空格时用自绘抓手，否则用窗口类光标（十字）。
pub(super) fn apply_cursor(hwnd: HWND, space: bool) {
    unsafe {
        let cur = if space {
            hand_cursor()
        } else {
            HCURSOR(GetClassLongPtrW(hwnd, GCLP_HCURSOR) as *mut _)
        };
        let _ = SetCursor(Some(cur));
    }
}

/// 抓手光标句柄缓存（进程内只创建一次，避免每次截图泄漏 GDI 句柄）。
static HAND_CURSOR: AtomicIsize = AtomicIsize::new(0);

/// 手掌形状的字符画掩码，`#` 为实心（三指 + 拇指 + 掌心）。
const HAND_MASK: [&str; 16] = [
    ".....##........",
    ".....##........",
    ".....##..##....",
    ".....##..##....",
    ".....##..##.##.",
    ".....##..##.##.",
    "..#..##..##.##.",
    "..#####..##.##.",
    "..############.",
    ".#############.",
    "..############.",
    "..############.",
    "...###########.",
    "...##########..",
    "....#########..",
    ".....#######...",
];

/// 掩码在光标画布中的偏移。
const HAND_OFFSET: (i32, i32) = (8, 6);

/// 把抓手掩码绘制到 BGRA 缓冲区：形状白色 + 外侧 1px 黑边（不侵入形状，细手指也保持白色）。
fn paint_hand(px: &mut [u8], s: i32) {
    let filled = |x: i32, y: i32| -> bool {
        let (gx, gy) = (x - HAND_OFFSET.0, y - HAND_OFFSET.1);
        if gx < 0 || gy < 0 || gy >= HAND_MASK.len() as i32 {
            return false;
        }
        let row = HAND_MASK[gy as usize].as_bytes();
        gx < row.len() as i32 && row[gx as usize] == b'#'
    };
    for y in 0..s {
        for x in 0..s {
            let inside = filled(x, y);
            let halo = !inside && (-1..=1).any(|dy| (-1..=1).any(|dx| filled(x + dx, y + dy)));
            if !inside && !halo {
                continue;
            }
            let i = ((y * s + x) * 4) as usize;
            let (r, g, b) = if inside { (255, 255, 255) } else { (0, 0, 0) };
            px[i] = b;
            px[i + 1] = g;
            px[i + 2] = r;
            px[i + 3] = 255;
        }
    }
}

/// 抓手光标（空格移动选区时使用）。
fn hand_cursor() -> HCURSOR {
    let cached = HAND_CURSOR.load(Ordering::Relaxed);
    if cached != 0 {
        return HCURSOR(cached as *mut core::ffi::c_void);
    }
    let cur = create_hand_cursor();
    HAND_CURSOR.store(cur.0 as isize, Ordering::Relaxed);
    cur
}

/// 自绘抓手光标：Win32 无内置抓手，用字符画掩码 + 外侧 1px 黑边生成 32bpp 彩色光标。
fn create_hand_cursor() -> HCURSOR {
    const S: i32 = 32;
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: S,
                biHeight: -S,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [Default::default()],
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let hbm_color = match CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(_) => return HCURSOR::default(),
        };
        let px = std::slice::from_raw_parts_mut(bits as *mut u8, (S * S * 4) as usize);
        paint_hand(px, S);

        let mask = CreateBitmap(S, S, 1, 1, None);
        let info = ICONINFO {
            fIcon: false.into(),
            xHotspot: (HAND_OFFSET.0 + 7) as u32,
            yHotspot: (HAND_OFFSET.1 + 11) as u32,
            hbmMask: mask,
            hbmColor: hbm_color,
        };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(hbm_color.into());
        if !mask.0.is_null() {
            let _ = DeleteObject(mask.into());
        }
        HCURSOR(icon.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 抓手光标创建并缓存（空格移动选区用）。
    #[test]
    fn hand_cursor_is_cached() {
        let a = hand_cursor();
        let b = hand_cursor();
        assert!(!a.0.is_null(), "抓手光标创建失败");
        assert_eq!(a.0, b.0, "抓手光标应复用缓存句柄");
    }

    /// 抓手掩码应同时有白色掌心/手指与黑色轮廓（否则在浅色或深色背景上会看不见）。
    #[test]
    fn hand_mask_has_white_shape_and_black_outline() {
        const S: i32 = 32;
        let mut px = vec![0u8; (S * S * 4) as usize];
        paint_hand(&mut px, S);
        let count = |rgb: [u8; 3]| {
            px.chunks_exact(4)
                .filter(|p| p[0] == rgb[2] && p[1] == rgb[1] && p[2] == rgb[0] && p[3] == 255)
                .count()
        };
        let white = count([255, 255, 255]);
        let black = count([0, 0, 0]);
        assert!(white > 60, "白色形状过小：{white}");
        assert!(black > 60, "黑色轮廓过小：{black}");
        assert!(white < 400 && black < 400, "形状过大：white={white} black={black}");
    }
}
