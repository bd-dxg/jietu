// SPDX-License-Identifier: GPL-3.0-only
//! 贴图（PIN-1~6）：分层置顶窗口、拖拽/滚轮缩放、透明度、数量管理。
//! 窗口统一在主线程创建（贴图生命周期超出截图/覆盖层线程），
//! 覆盖层线程经 `from_payload` 跨线程传递裁剪图（GlobalAlloc 载荷）。
//!
//! 文件分工：`window` 窗口状态与上屏、`wndproc` 消息分发、`border` 边框烘焙、
//! `menu` 右键菜单、`clipboard` 剪贴板读图、`scale` 像素变换、`dib` DIB 辅助。

mod border;
pub mod clipboard; // tests 用
mod dib;
mod menu;
mod scale;
mod window;
mod wndproc;
mod zoom;

use std::sync::Mutex;

use tiny_skia::Pixmap;
use windows::Win32::Foundation::{GlobalFree, HGLOBAL, HINSTANCE, HWND};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, MB_ICONERROR, MB_OK, PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, WM_CLOSE,
};

use crate::app::message_box;
use crate::settings::PinPosition;

/// 贴图数量上限（PIN-5）。
pub const MAX_PINS: usize = 20;

/// 贴图内边框切换快捷键（非全局；贴图窗口聚焦时生效，M2b 修正：避免无修饰键抢占全局）。
static BORDER_KEY: Mutex<(u32, u32)> = Mutex::new((0x72, 2)); // 默认 Ctrl+F3

/// 设置贴图边框快捷键（App 启动/改键时下发）。
pub fn set_border_key(vk: u32, mods: u32) {
    *BORDER_KEY.lock().unwrap_or_else(|e| e.into_inner()) = (vk, mods);
}

/// 当前贴图边框快捷键。
pub(super) fn border_key() -> (u32, u32) {
    *BORDER_KEY.lock().unwrap_or_else(|e| e.into_inner())
}

/// 存活贴图窗口句柄（close_all / 数量统计）。HWND 非 Send，存原生指针值。
static PINS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn pins() -> std::sync::MutexGuard<'static, Vec<usize>> {
    PINS.lock().unwrap_or_else(|e| e.into_inner())
}

fn register_hwnd(hwnd: HWND) {
    pins().push(hwnd.0 as usize);
}

/// 窗口销毁时从列表移除（由 window.rs WM_NCDESTROY 调用）。
pub(super) fn unregister_hwnd(hwnd: HWND) {
    pins().retain(|&h| h != hwnd.0 as usize);
}

/// 从像素图创建贴图窗口。
/// `place` 决定定位：Original 时用 `at`（选区屏幕位置），Center 或无 `at` 时居中。
/// `show_border`：是否烘焙 1px 蓝色边框。
pub fn create(
    pixmap: Pixmap,
    hinstance: HINSTANCE,
    place: PinPosition,
    at: Option<(i32, i32)>,
    show_border: bool,
) -> Result<HWND, String> {
    if pins().len() >= MAX_PINS {
        message_box(
            None,
            &format!("贴图数量已达上限（{MAX_PINS} 张）。\n请关闭部分贴图后再试。"),
            MB_OK | MB_ICONERROR,
        );
        return Err("数量上限".into());
    }
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let (vx, vy, vw, vh) = virtual_screen();
    let (px, py) = match (place, at) {
        // 原位置：选区坐标（坐标已含虚拟屏偏移），仅限幅回屏幕内
        (PinPosition::Original, Some((sx, sy))) => (
            sx.clamp(vx + 4, (vx + vw - w - 4).max(vx + 4)),
            sy.clamp(vy + 4, (vy + vh - h - 4).max(vy + 4)),
        ),
        // 居中（剪贴板贴图 / 设置了居中）：级联偏移避免完全重叠
        _ => {
            let n = pins().len() as i32;
            let x = (vx + vw / 2 - w / 2 + n * 20).clamp(vx + 4, (vx + vw - w - 4).max(vx + 4));
            let y = (vy + vh / 2 - h / 2 + n * 20).clamp(vy + 4, (vy + vh - h - 4).max(vy + 4));
            (x, y)
        }
    };
    let hwnd = window::spawn(pixmap, hinstance, px, py, show_border)?;
    register_hwnd(hwnd);
    Ok(hwnd)
}

/// PIN-2：全局 F3 → 剪贴板图片贴到屏幕（始终居中）。
pub fn from_clipboard(hinstance: HINSTANCE, place: PinPosition, show_border: bool) {
    match clipboard::read_image() {
        Ok(pm) => {
            if let Err(e) = create(pm, hinstance, place, None, show_border) {
                // 数量上限已提示过，静默其余失败
                let _ = e;
            }
        }
        Err(e) => {
            message_box(None, &e, MB_OK | MB_ICONERROR);
        }
    }
}

/// PIN-1：覆盖层线程跨线程传递的裁剪图载荷（GlobalAlloc: u32 w + u32 h + i32 x + i32 y + RGBA）。
/// 由主线程 wndproc 调用；读取后负责释放。
pub unsafe fn from_payload(hg: HGLOBAL, hinstance: HINSTANCE, place: PinPosition, show_border: bool) {
    unsafe {
        // 1) 锁内仅拷贝载荷，随即释放句柄；后续创建/弹窗不再持有锁（审查 P2-2：
        //    模态 MessageBox 重入消息循环，锁着 HGLOBAL 是坏味道）。
        let payload: Option<(Pixmap, (i32, i32))> = (|| {
            let ptr = GlobalLock(hg);
            if ptr.is_null() {
                return None;
            }
            let hp = ptr as *const u32;
            let w = *hp;
            let h = *hp.add(1);
            let sp = hp.add(2) as *const i32;
            let x = *sp;
            let y = *sp.add(1);
            let total = (w as usize) * (h as usize) * 4;
            let size = tiny_skia::IntSize::from_wh(w, h)?;
            let src = std::slice::from_raw_parts((ptr as *const u8).add(16), total);
            let pm = Pixmap::from_vec(src.to_vec(), size)?;
            Some((pm, (x, y)))
        })();
        let _ = GlobalUnlock(hg);
        let _ = GlobalFree(Some(hg));

        let Some((pm, (x, y))) = payload else {
            message_box(None, "贴图载荷尺寸非法", MB_OK | MB_ICONERROR);
            return;
        };
        if let Err(e) = create(pm, hinstance, place, Some((x, y)), show_border) {
            // 数量上限已提示；窗口创建失败必须让用户知道（M2b 静默缺陷）
            message_box(None, &format!("贴图创建失败：{e}"), MB_OK | MB_ICONERROR);
        }
    }
}

/// PIN-4：关闭全部贴图。
pub fn close_all() {
    for h in pins().clone() {
        let _ = unsafe {
            PostMessageW(
                Some(HWND(h as *mut _)),
                WM_CLOSE,
                Default::default(),
                Default::default(),
            )
        };
    }
}

/// 切换全部贴图的边框显示（贴图窗口内快捷键 → 主窗口消息触发），返回切换后的状态；无贴图时返回 None。
pub fn toggle_border() -> Option<bool> {
    let hwnds = pins().clone();
    if hwnds.is_empty() {
        return None;
    }
    // 以第一张的当前状态决定目标方向：开 → 关 / 关 → 开
    let on = !unsafe { dib::pin_from(HWND(hwnds[0] as *mut _)) }
        .map(|w| w.pad > 0)
        .unwrap_or(false);
    for h in hwnds {
        unsafe {
            if let Some(w) = dib::pin_from(HWND(h as *mut _)) {
                w.toggle_border(on);
            }
        }
    }
    Some(on)
}

/// 虚拟屏幕矩形（多显示器统一坐标）。
fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}
