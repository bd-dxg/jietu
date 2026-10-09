// SPDX-License-Identifier: GPL-3.0-only
//! 自绘弹出菜单（托盘右键菜单用）：
//! 无边框 + 柔和阴影（替代系统菜单的固定边框）；交互：单击选中高亮、双击执行、
//! Esc/方向键、点击外部关闭。整窗带 UpdateLayeredWindow 一次上屏。

use tiny_skia::{Pixmap, Transform};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, DEFAULT_GUI_FONT, DeleteDC, DeleteObject, GetDC, GetStockObject,
    GetTextExtentPoint32W, HBITMAP, HDC, ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::editor::Point as FPoint;
use crate::overlay::geometry::SelRect;
use crate::overlay::surface::{convert_rgba_to_bgra, create_dib_surface};
use crate::overlay::toolbar::icons::rounded_rect;
use crate::render::text as dwrite;
use crate::theme::Palette;

const WINDOW_CLASS: PCWSTR = w!("jietu.popmenu");
/// 单位（未缩放）行高。
const ITEM_H: i32 = 34;
/// 文字左右内边距。
const PAD_X: i32 = 18;
/// 分隔线占位高度。
const SEP_H: i32 = 9;
/// 内容圆角。
const CORNER: f32 = 8.0;
/// 菜单最小宽度。
const MIN_W: i32 = 150;

/// 菜单项。
pub struct Item {
    pub id: usize,
    pub text: &'static str,
    /// true 时在本项上方绘制分隔线。
    pub separator_before: bool,
}

/// 弹出菜单运行上下文。
struct Ctx {
    items: Vec<Item>,
    palette: &'static Palette,
    scale: f32,
    w: i32,
    h: i32,
    /// 每项内容区 y（含分隔线占位，已缩放）。
    ys: Vec<i32>,
    hover: Option<usize>,
    result: Option<usize>,
    hwnd: HWND,
    mem_dc: HDC,
    dib_bmp: HBITMAP,
    dib_bits: *mut u8,
}

impl Drop for Ctx {
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

/// 弹出菜单并阻塞，返回双击选择的菜单项 id（未选择返回 None）。
pub fn popup(owner: HWND, items: Vec<Item>, palette: &'static Palette) -> Option<usize> {
    let scale = unsafe { GetDpiForWindow(owner) as f32 / 96.0 };
    let item_h = (ITEM_H as f32 * scale) as i32;
    let sep_h = (SEP_H as f32 * scale) as i32;
    let pad_x = (PAD_X as f32 * scale) as i32;

    // 测量文本宽度（默认 UI 字体）决定菜单宽
    let hdc = unsafe { GetDC(Some(owner)) };
    let max_w = {
        let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
        unsafe {
            let _ = SelectObject(hdc, font.into());
        }
        let mut mw = 0i32;
        for it in &items {
            if it.text.is_empty() {
                continue;
            }
            let buf: Vec<u16> = it.text.encode_utf16().collect();
            let mut sz = SIZE::default();
            unsafe {
                let _ = GetTextExtentPoint32W(hdc, &buf, &mut sz);
            }
            mw = mw.max(sz.cx);
        }
        mw
    };
    unsafe {
        let _ = ReleaseDC(Some(owner), hdc);
    }
    let w = (max_w + pad_x * 2).max(MIN_W);
    let mut ys = Vec::with_capacity(items.len());
    let mut y = 0i32;
    for it in &items {
        if it.separator_before {
            y += sep_h;
        }
        ys.push(y);
        y += item_h;
    }
    let h = y;

    // 窗口位置：光标左上方（托盘菜单向上弹出），限定在屏幕内
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let sh = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let x = (pt.x - w + 8).clamp(0, (sw - w).max(0));
    let ypos = (pt.y - h + 8).clamp(0, (sh - h).max(0));

    let hwnd = match create_window(owner, x, ypos, w, h) {
        Ok(h) => h,
        Err(_) => return None,
    };
    let mut ctx = Ctx {
        items,
        palette,
        scale,
        w,
        h,
        ys,
        hover: None,
        result: None,
        hwnd,
        mem_dc: Default::default(),
        dib_bmp: Default::default(),
        dib_bits: std::ptr::null_mut(),
    };
    // 上屏 DIB
    match create_dib_surface(w as u32, h as u32) {
        Ok((dc, bmp, bits)) => {
            ctx.mem_dc = dc;
            ctx.dib_bmp = bmp;
            ctx.dib_bits = bits;
        }
        Err(_) => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return None;
        }
    }
    let raw = Box::into_raw(Box::new(ctx));
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let _ = SetFocus(Some(hwnd));
    }
    unsafe {
        present(&*raw);
    }

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 != 0 {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }
    let ctx = unsafe { Box::from_raw(raw) };
    let result = ctx.result;
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    result
}

fn create_window(owner: HWND, x: i32, y: i32, w: i32, h: i32) -> Result<HWND, String> {
    unsafe {
        let wc = WNDCLASSW {
            // CS_DBLCLKS：双击执行菜单项
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .unwrap_or_default()
                .into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: WINDOW_CLASS,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 && windows::Win32::Foundation::GetLastError().0 != 1410 {
            return Err("菜单窗口类注册失败".into());
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            WINDOW_CLASS,
            w!("jietu.menu"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            Some(owner),
            None,
            Some(GetModuleHandleW(None).unwrap_or_default().into()),
            None,
        )
        .map_err(|e| format!("创建菜单窗口失败：{e}"))?;
        Ok(hwnd)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    if msg == WM_DESTROY {
        unsafe {
            let _ = PostQuitMessage(0);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
    }
    let Some(ctx) = app_from(hwnd) else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    let mouse = |lparam: LPARAM| {
        (
            (lparam.0 & 0xFFFF) as u16 as i16 as i32,
            ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
        )
    };
    match msg {
        // 单击即执行（标准菜单交互）；点击菜单外关闭
        WM_LBUTTONDOWN => {
            let (x, y) = mouse(lparam);
            if let Some(i) = ctx.hit_item(x, y) {
                ctx.result = Some(ctx.items[i].id);
                ctx.close();
            } else {
                ctx.close();
            }
            LRESULT(0)
        }
        // 双击与单击一致（快速连点不产生歧义）
        WM_LBUTTONDBLCLK => {
            let (x, y) = mouse(lparam);
            if let Some(i) = ctx.hit_item(x, y) {
                ctx.result = Some(ctx.items[i].id);
                ctx.close();
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = mouse(lparam);
            let hit = ctx.hit_item(x, y);
            if hit != ctx.hover {
                ctx.hover = hit;
                present(ctx);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            match wparam.0 as u32 {
                0x1B => ctx.close(), // Esc
                0x0D => {
                    // Enter：执行悬停项
                    if let Some(i) = ctx.hover {
                        ctx.result = Some(ctx.items[i].id);
                        ctx.close();
                    }
                }
                0x26 | 0x28 => {
                    // 上下方向键导航（悬停高亮移动）
                    let len = ctx.items.len();
                    let cur = ctx.hover.map(|i| i as i32).unwrap_or(-1);
                    let next = if wparam.0 as u32 == 0x26 {
                        (cur - 1).max(0)
                    } else {
                        (cur + 1).min(len as i32 - 1)
                    };
                    if len > 0 {
                        ctx.hover = Some(next as usize);
                        present(ctx);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        // 失去焦点（点击外部）：关闭菜单
        WM_ACTIVATE => {
            if wparam.0 as u16 == 0 {
                ctx.close();
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn app_from(hwnd: HWND) -> Option<&'static mut Ctx> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if ptr == 0 { None } else { Some(&mut *(ptr as *mut Ctx)) }
    }
}

impl Ctx {
    /// 命中菜单项（内容区外返回 None）。
    fn hit_item(&self, x: i32, y: i32) -> Option<usize> {
        let item_h = (ITEM_H as f32 * self.scale) as i32;
        if x < 0 || x >= self.w || y < 0 || y >= self.h {
            return None;
        }
        self.ys.iter().position(|yy| y >= *yy && y < *yy + item_h)
    }

    fn close(&mut self) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

/// 全量重绘：内容 → premultiplied BGRA → UpdateLayeredWindow 上屏。
fn present(ctx: &Ctx) {
    let Some(mut pix) = Pixmap::new(ctx.w as u32, ctx.h as u32) else {
        return;
    };
    let p = ctx.palette;
    let s = ctx.scale;
    let item_h = (ITEM_H as f32 * s) as i32;
    let pad_x = (PAD_X as f32 * s) as i32;
    let font_size = 14.0 * s;
    let text_h = (font_size * 1.2) as i32;

    // 内容背景（圆角矩形，窗口四角透明）+ 1px 细边框提供对比度（替代阴影）
    rounded_rect(
        &mut pix,
        SelRect {
            x: 0,
            y: 0,
            w: ctx.w,
            h: ctx.h,
        },
        CORNER,
        p.bg,
        Some(p.border),
    );

    for (i, it) in ctx.items.iter().enumerate() {
        let y = ctx.ys[i];
        if it.separator_before {
            let line_y = y - (SEP_H as f32 * s) as i32 / 2;
            let rect = SelRect {
                x: pad_x,
                y: line_y,
                w: ctx.w - pad_x * 2,
                h: 1,
            };
            rounded_rect(&mut pix, rect, 0.5, p.divider, None);
        }
        // 悬停高亮整行占满（不留左右空隙）
        let rect = SelRect {
            x: 0,
            y,
            w: ctx.w,
            h: item_h,
        };
        if ctx.hover == Some(i) {
            rounded_rect(&mut pix, rect, 0.0, p.row_hover, None);
        }
        let color = p.text;
        let ty = y + (item_h - text_h) / 2;
        dwrite::draw(
            &mut pix,
            FPoint::new(pad_x as f32, ty as f32),
            it.text,
            font_size,
            color,
            Transform::identity(),
        );
    }

    // premultiplied RGBA → BGRA 上屏
    convert_rgba_to_bgra(pix.data(), ctx.dib_bits as usize, ctx.w, 0, 0, ctx.w, ctx.h, 1);
    unsafe {
        let hdc_screen = GetDC(None);
        if hdc_screen.0.is_null() {
            return;
        }
        // 窗口当前位置（UpdateLayeredWindow 的 pptdst 会把窗口移动过去，必须传真实坐标）
        let mut wr = RECT::default();
        let _ = GetWindowRect(ctx.hwnd, &mut wr);
        let pt = POINT { x: wr.left, y: wr.top };
        let size = SIZE { cx: ctx.w, cy: ctx.h };
        let pt_src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            ctx.hwnd,
            Some(hdc_screen),
            Some(&pt as *const POINT),
            Some(&size as *const SIZE),
            Some(ctx.mem_dc),
            Some(&pt_src as *const POINT),
            COLORREF(0),
            Some(&blend as *const BLENDFUNCTION),
            ULW_ALPHA,
        );
        let _ = ReleaseDC(None, hdc_screen);
    }
}
