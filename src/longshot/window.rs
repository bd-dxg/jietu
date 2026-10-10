// SPDX-License-Identifier: GPL-3.0-only
//! 长截图执行窗口（LNG-2/3/6）：置顶**分层**状态条，风格与截图工具栏一致
//! （tiny-skia 绘制，见 `bar`）。键位：Enter 完成、Esc 取消、Ctrl+Z 撤销、空格 切换模式；
//! 也可用鼠标点条上的按钮。定时器每 ~90ms 驱动 `Session::tick`。
//!
//! 执行窗口用 `WS_EX_NOACTIVATE` 不抢焦点（滚轮需发给目标页面），键盘走注册到本窗口的全局热键。

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, DeleteDC, DeleteObject, HBITMAP, HDC};
use windows::Win32::UI::Input::Ime::{HIMC, ImmAssociateContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, HOT_KEY_MODIFIERS, MOD_CONTROL, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use super::bar::{self, Bar, Button, State};
use super::border;
use super::session::Session;
use crate::theme::Palette;

/// 定时器 ID。
const TIMER_ID: usize = 1;

/// 全局热键 ID（执行窗口无焦点，键盘走全局热键；Enter/Esc/空格/Ctrl+Z）。
const HK_ENTER: i32 = 301;
const HK_ESC: i32 = 302;
const HK_SPACE: i32 = 303;
const HK_UNDO: i32 = 304;
/// 每拍间隔（毫秒）。
const TICK_MS: u32 = 90;

/// 注册/注销执行窗口的全局热键。
fn register_hotkeys(hwnd: HWND) {
    unsafe {
        let _ = RegisterHotKey(Some(hwnd), HK_ENTER, HOT_KEY_MODIFIERS(0), 0x0D);
        let _ = RegisterHotKey(Some(hwnd), HK_ESC, HOT_KEY_MODIFIERS(0), 0x1B);
        let _ = RegisterHotKey(Some(hwnd), HK_SPACE, HOT_KEY_MODIFIERS(0), 0x20);
        let _ = RegisterHotKey(Some(hwnd), HK_UNDO, MOD_CONTROL, 0x5A);
    }
}

fn unregister_hotkeys(hwnd: HWND) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HK_ENTER);
        let _ = UnregisterHotKey(Some(hwnd), HK_ESC);
        let _ = UnregisterHotKey(Some(hwnd), HK_SPACE);
        let _ = UnregisterHotKey(Some(hwnd), HK_UNDO);
    }
}

/// 执行窗口运行时状态。
struct Win {
    session: Box<Session>,
    /// 视口边框窗口（M4，让用户看清截取范围）。
    border_hwnd: Option<HWND>,
    /// 状态条几何。
    bar: Bar,
    /// 主题调色板（与工具栏一致）。
    palette: &'static Palette,
    /// 上屏用内存 DC 与其 DIB。
    mem_dc: HDC,
    dib_bmp: HBITMAP,
    dib_bits: *mut u8,
    /// 状态条像素尺寸。
    w: i32,
    h: i32,
    /// 是否有待上屏的变更。
    dirty: bool,
}

impl Drop for Win {
    fn drop(&mut self) {
        unsafe {
            if !self.dib_bmp.0.is_null() {
                let _ = DeleteObject(self.dib_bmp.into());
            }
            if !self.mem_dc.0.is_null() {
                let _ = DeleteDC(self.mem_dc);
            }
        }
    }
}

/// 启动长截图执行并阻塞到完成/取消。完成后返回拼接结果；取消返回 None。
pub fn run(
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hinstance: windows::Win32::Foundation::HINSTANCE,
    max_height: u32,
    palette: &'static Palette,
) -> Option<tiny_skia::Pixmap> {
    let session = match Session::new(x, y, w, h, max_height) {
        Ok(s) => s,
        Err(e) => {
            crate::app::message_box(None, &format!("长截图初始化失败：{e}"), MB_OK | MB_ICONERROR);
            return None;
        }
    };

    // 视口边框：提前创建让用户立刻看到截取范围。
    let border_hwnd = border::spawn(x, y, w, h, hinstance);

    unsafe {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("jietu.longshot"),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        let screen_h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let bar = bar::layout(x, w, y, h, screen_h);
        let (bw, bh) = (bar.rect.w, bar.rect.h);
        let (bx, by) = (bar.rect.x, bar.rect.y);

        let hwnd = CreateWindowExW(
            // WS_EX_LAYERED：UpdateLayeredWindow 提交带圆角透明的像素（与工具栏一致）；
            // WS_EX_NOACTIVATE：不抢焦点（滚轮须发给目标页面，键盘走全局热键）。
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("jietu.longshot"),
            w!("长截图"),
            WS_POPUP,
            bx,
            by,
            bw,
            bh,
            None,
            None,
            Some(hinstance),
            None,
        )
        .map_err(|e| {
            crate::app::message_box(None, &format!("创建长截图窗口失败：{e}"), MB_OK | MB_ICONERROR);
        })
        .ok()?;
        // 执行窗口禁输入法：中文 IME 会截走 Enter/Esc/Space 等按键。
        let _ = ImmAssociateContext(hwnd, HIMC(std::ptr::null_mut()));

        let (mem_dc, dib_bmp, dib_bits) = crate::overlay::surface::create_dib_surface(bw as u32, bh as u32).ok()?;
        let win = Box::new(Win {
            session: Box::new(session),
            border_hwnd,
            bar,
            palette,
            mem_dc,
            dib_bmp,
            dib_bits,
            w: bw,
            h: bh,
            dirty: true,
        });
        let raw = Box::into_raw(win);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), bx, by, bw, bh, SWP_SHOWWINDOW);
        register_hotkeys(hwnd);
        let _ = SetTimer(Some(hwnd), TIMER_ID, TICK_MS, None);
        // 首帧立即上屏。
        redraw(hwnd);

        let mut msg = MSG::default();
        loop {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 == 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
            if msg.hwnd == hwnd && msg.message == WM_DESTROY {
                break;
            }
        }

        // 局部持有 Box 指针，不重新查询窗口 userdata（销毁后不可靠，见 app 模块教训）。
        let result = if !(*raw).session.cancelled {
            (*raw).session.result().ok()
        } else {
            None
        };
        unregister_hotkeys(hwnd);
        if let Some(bh) = (*raw).border_hwnd {
            border::destroy(bh);
        }
        let _ = Box::from_raw(raw);
        result
    }
}

/// 绘制状态条并提交到分层窗口。
fn redraw(hwnd: HWND) {
    let Some(win) = win_from(hwnd) else {
        return;
    };
    let (w, h) = (win.w, win.h);
    let mut pixmap = match tiny_skia::Pixmap::new(w as u32, h as u32) {
        Some(p) => p,
        None => return,
    };
    let state = State {
        auto: win.session.auto,
        height: win.session.height(),
        can_undo: win.session.can_undo(),
        hint: win.session.hint(),
    };
    bar::render(&mut pixmap, &win.bar, &state, win.palette);
    // RGBA → premultiplied BGRA 写入 DIB（tiny-skia 像素本就是 premultiplied）。
    let bgra = premul_bgra(pixmap.data());
    unsafe {
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), win.dib_bits, bgra.len());
        present(hwnd, win.mem_dc);
    }
    win.dirty = false;
}

/// UpdateLayeredWindow 提交（窗口位置不变，只更新像素）。
unsafe fn present(hwnd: HWND, mem_dc: HDC) {
    unsafe {
        let mut rect = windows::Win32::Foundation::RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        let ppt_dst = POINT {
            x: rect.left,
            y: rect.top,
        };
        let psize = SIZE {
            cx: rect.right - rect.left,
            cy: rect.bottom - rect.top,
        };
        let ppt_src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            None,
            Some(&ppt_dst),
            Some(&psize),
            Some(mem_dc),
            Some(&ppt_src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
    }
}

/// RGBA（premultiplied）→ BGRA。
fn premul_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    out
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_ID);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if let Some(win) = win_from(hwnd) {
                let changed = win.session.tick();
                if changed {
                    win.dirty = true;
                }
                if win.dirty {
                    redraw(hwnd);
                }
                if win.session.finished || win.session.cancelled {
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if let Some(win) = win_from(hwnd) {
                let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
                match win.bar.hit(x, y) {
                    Some(Button::Mode) => win.session.toggle_mode(),
                    Some(Button::Undo) => win.session.undo(),
                    Some(Button::Done) => win.session.finished = true,
                    Some(Button::Cancel) => win.session.cancelled = true,
                    None => {}
                }
                win.dirty = true;
                redraw(hwnd);
                if win.session.finished || win.session.cancelled {
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if let Some(win) = win_from(hwnd) {
                match wparam.0 as u32 {
                    0x0D => win.session.finished = true,
                    0x1B => win.session.cancelled = true,
                    0x20 => win.session.toggle_mode(),
                    0x5A if ctrl_down() => win.session.undo(),
                    _ => {}
                }
                win.dirty = true;
                redraw(hwnd);
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            if let Some(win) = win_from(hwnd) {
                match wparam.0 as i32 {
                    HK_ENTER => win.session.finished = true,
                    HK_ESC => win.session.cancelled = true,
                    HK_SPACE => win.session.toggle_mode(),
                    HK_UNDO => win.session.undo(),
                    _ => {}
                }
                win.dirty = true;
                redraw(hwnd);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn ctrl_down() -> bool {
    unsafe { GetKeyState(0x11) as u16 & 0x8000 != 0 }
}

fn win_from(hwnd: HWND) -> Option<&'static mut Win> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if ptr == 0 { None } else { Some(&mut *(ptr as *mut Win)) }
    }
}
