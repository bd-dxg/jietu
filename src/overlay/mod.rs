// SPDX-License-Identifier: GPL-3.0-only
//! 覆盖层（CAP-2/3）：覆盖整个虚拟屏幕的无边框置顶窗口，
//! 冻结画面 + 选区暗化（预生成暗图 + 选区回贴原图），支持框选、移动与八向缩放。

use std::mem::size_of;

use tiny_skia::{Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DEFAULT_GUI_FONT, DIB_RGB_COLORS, GetDC, GetStockObject,
    GetTextExtentPoint32W, ReleaseDC, SelectObject, SetBkMode, SetDIBitsToDevice, SetTextColor, TRANSPARENT, TextOutW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::capture::CapturedScreen;
use crate::output;

const WINDOW_CLASS: PCWSTR = w!("jietu.overlay");
const HANDLE_SIZE: i32 = 7; // 手柄边长
const PICK_RADIUS: i32 = 6; // 命中判定半径
const ACCENT: u8 = 26; // 主题蓝
const ACCENT_G: u8 = 115;
const ACCENT_B: u8 = 232;

/// 矩形选区（相对虚拟屏幕左上角的物理像素坐标）。
#[derive(Clone, Copy, Debug)]
pub struct SelRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl SelRect {
    /// 保证 w/h 非负。
    fn normalized(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let x = x0.min(x1);
        let y = y0.min(y1);
        SelRect {
            x,
            y,
            w: x0.max(x1) - x,
            h: y0.max(y1) - y,
        }
    }
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// 手柄类型（八向 + 移动）。
#[derive(Clone, Copy, PartialEq)]
enum Handle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
    Move,
}

/// 交互阶段。
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    /// 拖框选择中
    Selecting,
    /// 选区确认后，可移动/缩放/输出
    Adjusting,
}

/// 拖拽中的上下文。
struct Drag {
    handle: Handle,
    /// 按下时的选区（用于计算增量）。
    orig: SelRect,
    /// 按下时的鼠标位置。
    px: i32,
    py: i32,
}

/// 覆盖层运行时状态。
pub struct Overlay {
    pub capture: CapturedScreen,
    pub original: Pixmap,
    pub dimmed: Pixmap,
    pub display: Pixmap,
    pub bgra_buffer: Vec<u8>,
    pub selection: Option<SelRect>,
    phase: Phase,
    drag: Option<Drag>,
    pub hwnd: HWND,
    pub cancelled: bool,
}

impl Overlay {
    fn new(capture: CapturedScreen) -> Result<Self, String> {
        let (w, h) = (capture.width, capture.height);
        let original = Pixmap::from_vec(capture.rgba.clone(), tiny_skia::IntSize::from_wh(w, h).unwrap())
            .ok_or("创建原图像素图失败")?;
        let mut dimmed = original.clone();
        // 暗化：整屏覆盖半透明黑（CAP-3：预生成暗图）。
        let mut paint = Paint::default();
        paint.set_color_rgba8(0, 0, 0, 120);
        dimmed.fill_rect(
            Rect::from_xywh(0.0, 0.0, w as f32, h as f32).unwrap(),
            &paint,
            Transform::identity(),
            None,
        );
        let display = dimmed.clone();
        Ok(Self {
            capture,
            original,
            dimmed,
            display,
            bgra_buffer: Vec::new(),
            selection: None,
            phase: Phase::Adjusting,
            drag: None,
            hwnd: HWND::default(),
            cancelled: false,
        })
    }

    /// 注册窗口类并创建覆盖窗口。
    fn create_window(hinstance: windows::Win32::Foundation::HINSTANCE) -> Result<HWND, String> {
        unsafe {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
                hbrBackground: Default::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: WINDOW_CLASS,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                WINDOW_CLASS,
                w!("jietu.overlay"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinstance),
                None,
            )
            .map_err(|e| format!("创建覆盖层窗口失败：{e}"))?;
            Ok(hwnd)
        }
    }

    /// 显示覆盖层并阻塞运行，直到截图完成或取消。
    /// 返回是否完成（false = 取消）。
    pub fn run(capture: CapturedScreen, hinstance: HINSTANCE) -> bool {
        let (origin_x, origin_y) = (capture.origin_x, capture.origin_y);
        let (vw, vh) = (capture.width as i32, capture.height as i32);
        let hwnd = match Self::create_window(hinstance) {
            Ok(h) => h,
            Err(e) => {
                unsafe {
                    let _ = MessageBoxW(None, wide(&e), w!("jietu"), MB_OK | MB_ICONERROR);
                }
                return false;
            }
        };

        let mut overlay = match Self::new(capture) {
            Ok(o) => o,
            Err(_) => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return false;
            }
        };
        overlay.hwnd = hwnd;
        let raw = Box::into_raw(Box::new(overlay));

        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), origin_x, origin_y, vw, vh, SWP_SHOWWINDOW);
            let _ = SetForegroundWindow(hwnd);
        }

        // 局部消息循环：直到覆盖层窗口销毁（WM_DESTROY）。
        // 不调用 PostQuitMessage，避免污染应用主消息队列。
        unsafe {
            let mut msg = MSG::default();
            loop {
                let ret = GetMessageW(&mut msg, None, 0, 0);
                if ret.0 == 0 || msg.message == WM_QUIT {
                    break;
                }
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
                if msg.hwnd == hwnd && msg.message == WM_DESTROY {
                    break;
                }
            }
        }

        let cancelled = unsafe { (*raw).cancelled };
        unsafe {
            let _ = Box::from_raw(raw);
            let _ = DestroyWindow(hwnd);
        }
        !cancelled
    }

    /// 全量重绘（暗图 → 回贴选区 → 边框 → 上屏 + 文本）。
    fn redraw(&mut self) {
        self.display.data_mut().copy_from_slice(self.dimmed.data());
        if let Some(sel) = self.selection {
            blit_region(&mut self.display, &self.original, sel);
            draw_rect(&mut self.display, sel);
            draw_handles(&mut self.display, sel);
        }
        self.present();
    }

    /// 上屏：RGBA → BGRA → SetDIBitsToDevice + GDI 文本（尺寸/坐标）。
    fn present(&mut self) {
        let w = self.display.width() as i32;
        let h = self.display.height() as i32;
        self.bgra_buffer.clear();
        self.bgra_buffer = output::rgba_to_bgra(self.display.data());
        unsafe {
            let hdc = GetDC(Some(self.hwnd));
            if hdc.0.is_null() {
                return;
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                bmiColors: [Default::default()],
            };
            let _ = SetDIBitsToDevice(
                hdc,
                0,
                0,
                w as u32,
                h as u32,
                0,
                0,
                0,
                h as u32,
                self.bgra_buffer.as_ptr() as *const _ as *const _,
                &bmi,
                DIB_RGB_COLORS,
            );
            if let Some(sel) = self.selection {
                draw_info_text(hdc, sel, self.display.width() as i32);
            }
            let _ = ReleaseDC(Some(self.hwnd), hdc);
        }
    }

    fn on_lbutton_down(&mut self, x: i32, y: i32) {
        match self.selection {
            None => {
                // 开始框选
                self.selection = Some(SelRect { x, y, w: 1, h: 1 });
                self.phase = Phase::Selecting;
                self.drag = Some(Drag {
                    handle: Handle::Move,
                    orig: self.selection.unwrap(),
                    px: x,
                    py: y,
                });
            }
            Some(sel) => {
                if let Some(handle) = hit_handle(sel, x, y) {
                    self.drag = Some(Drag {
                        handle,
                        orig: sel,
                        px: x,
                        py: y,
                    });
                    self.phase = Phase::Adjusting;
                } else if sel.contains(x, y) {
                    self.drag = Some(Drag {
                        handle: Handle::Move,
                        orig: sel,
                        px: x,
                        py: y,
                    });
                    self.phase = Phase::Adjusting;
                } else {
                    // 外部点击：开始新选区
                    self.selection = Some(SelRect { x, y, w: 1, h: 1 });
                    self.phase = Phase::Selecting;
                    self.drag = Some(Drag {
                        handle: Handle::Move,
                        orig: self.selection.unwrap(),
                        px: x,
                        py: y,
                    });
                }
            }
        }
        self.redraw();
    }

    fn on_mouse_move(&mut self, x: i32, y: i32) {
        let Some(drag) = &self.drag else {
            return;
        };
        let (dx, dy) = (x - drag.px, y - drag.py);
        let handle = drag.handle;
        let orig = drag.orig;
        let (vw, vh) = (self.display.width() as i32, self.display.height() as i32);
        let max_x = vw.saturating_sub(1);
        let max_y = vh.saturating_sub(1);
        let x = x.clamp(0, max_x);
        let y = y.clamp(0, max_y);
        let sel = self.selection.unwrap_or(orig);
        match handle {
            Handle::Move => {
                let nx = (drag.orig.x + dx).clamp(0, max_x - sel.w.min(vw).max(0));
                let ny = (drag.orig.y + dy).clamp(0, max_y - sel.h.min(vh).max(0));
                self.selection = Some(SelRect {
                    x: nx,
                    y: ny,
                    w: drag.orig.w,
                    h: drag.orig.h,
                });
            }
            Handle::N => {
                let y1 = drag.orig.y + drag.orig.h;
                self.selection = Some(SelRect::normalized(drag.orig.x, y, drag.orig.x + drag.orig.w, y1));
            }
            Handle::S => {
                self.selection = Some(SelRect {
                    x: drag.orig.x,
                    y: drag.orig.y,
                    w: drag.orig.w,
                    h: (y - drag.orig.y).max(1),
                });
            }
            Handle::W => {
                let x1 = drag.orig.x + drag.orig.w;
                self.selection = Some(SelRect::normalized(x, drag.orig.y, x1, drag.orig.y + drag.orig.h));
            }
            Handle::E => {
                self.selection = Some(SelRect {
                    x: drag.orig.x,
                    y: drag.orig.y,
                    w: (x - drag.orig.x).max(1),
                    h: drag.orig.h,
                });
            }
            Handle::NE => {
                let x1 = drag.orig.x + drag.orig.w;
                let y1 = drag.orig.y + drag.orig.h;
                self.selection = Some(SelRect::normalized(drag.orig.x, y, x1, y1));
            }
            Handle::NW => {
                let x1 = drag.orig.x + drag.orig.w;
                let y1 = drag.orig.y + drag.orig.h;
                self.selection = Some(SelRect::normalized(x, y, x1, y1));
            }
            Handle::SE => {
                let x1 = (x).max(drag.orig.x + 1);
                let y1 = (y).max(drag.orig.y + 1);
                self.selection = Some(SelRect::normalized(drag.orig.x, drag.orig.y, x1, y1));
            }
            Handle::SW => {
                let x1 = drag.orig.x + drag.orig.w;
                let y1 = drag.orig.y + drag.orig.h;
                self.selection = Some(SelRect::normalized(x, drag.orig.y, x1, y1));
            }
        }
        if self.phase == Phase::Selecting && self.selection.is_some_and(|s| s.w > 2 && s.h > 2) {
            self.phase = Phase::Adjusting;
        }
        self.redraw();
    }

    fn on_lbutton_up(&mut self) {
        self.drag = None;
        if let Some(sel) = self.selection {
            if sel.w < 2 || sel.h < 2 {
                self.selection = None; // 误触，放弃
            } else {
                self.phase = Phase::Adjusting;
            }
        }
        self.redraw();
    }

    /// 右键取消（OUT-3）。
    fn on_cancel(&mut self) {
        self.cancelled = true;
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    /// Enter / Ctrl+C：复制到剪贴板并关闭（OUT-1）。
    fn on_copy(&mut self) {
        if let Some(sel) = self.selection {
            let cropped = crop_pixmap(&self.original, sel);
            std::thread::spawn(move || {
                if let Err(e) = output::copy_to_clipboard(&cropped) {
                    unsafe {
                        let _ = MessageBoxW(None, wide(&format!("复制失败：{e}")), w!("jietu"), MB_OK | MB_ICONERROR);
                    }
                }
            });
        }
        self.cancelled = false;
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    /// Ctrl+S：保存 PNG 到默认目录并关闭（OUT-2）。
    fn on_save(&mut self) {
        if let Some(sel) = self.selection {
            let cropped = crop_pixmap(&self.original, sel);
            std::thread::spawn(move || {
                let dir = output::default_save_dir();
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join(output::timestamped_filename("png"));
                let _ = output::save_png(&cropped, &path);
            });
        }
        self.cancelled = false;
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_DESTROY || msg == WM_NCCREATE {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let Some(overlay) = app_from(hwnd) else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            overlay.on_lbutton_down(x, y);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            overlay.on_mouse_move(x, y);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            overlay.on_lbutton_up();
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            overlay.on_cancel();
            LRESULT(0)
        }
        WM_KEYDOWN => {
            match wparam.0 as u32 {
                0x1B => overlay.on_cancel(),                                                    // Esc
                0x0D => overlay.on_copy(),                                                      // Enter
                0x43 if unsafe { GetKeyState(0x11) } as u16 & 0x8000 != 0 => overlay.on_copy(), // Ctrl+C
                0x53 if unsafe { GetKeyState(0x11) } as u16 & 0x8000 != 0 => overlay.on_save(), // Ctrl+S
                _ => {}
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn app_from(hwnd: HWND) -> Option<&'static mut Overlay> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if ptr == 0 {
            None
        } else {
            Some(&mut *(ptr as *mut Overlay))
        }
    }
}

/// 命中检测：返回落在哪个手柄/移动区域。
fn hit_handle(sel: SelRect, x: i32, y: i32) -> Option<Handle> {
    let (x0, y0, x1, y1) = (sel.x, sel.y, sel.x + sel.w, sel.y + sel.h);
    let cx = x0 + sel.w / 2;
    let cy = y0 + sel.h / 2;
    for (hx, hy, h) in [
        (x0, y0, Handle::NW),
        (cx, y0, Handle::N),
        (x1, y0, Handle::NE),
        (x0, cy, Handle::W),
        (x1, cy, Handle::E),
        (x0, y1, Handle::SW),
        (cx, y1, Handle::S),
        (x1, y1, Handle::SE),
    ] {
        if (x - hx).abs() <= PICK_RADIUS && (y - hy).abs() <= PICK_RADIUS {
            return Some(h);
        }
    }
    None
}

/// 将 src 的 r 区域拷贝到 dst（原地回贴）。
fn blit_region(dst: &mut Pixmap, src: &Pixmap, r: SelRect) {
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

/// 裁剪出选区像素。
fn crop_pixmap(src: &Pixmap, r: SelRect) -> Pixmap {
    let w = (r.w.max(1)) as u32;
    let h = (r.h.max(1)) as u32;
    let mut out = Pixmap::new(w, h).unwrap();
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
    out
}

/// 画选区边框。
fn draw_rect(pixmap: &mut Pixmap, sel: SelRect) {
    let Some(rect) = Rect::from_xywh(
        sel.x as f32 + 0.5,
        sel.y as f32 + 0.5,
        sel.w as f32 - 1.0,
        sel.h as f32 - 1.0,
    ) else {
        return;
    };
    let mut pb = PathBuilder::new();
    pb.push_rect(rect);
    let Some(path) = pb.finish() else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(ACCENT, ACCENT_G, ACCENT_B, 255);
    let stroke = Stroke {
        width: 2.0,
        ..Default::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    // 外圈白色辅助线
    let mut pb = PathBuilder::new();
    pb.push_rect(
        Rect::from_xywh(
            sel.x as f32 + 1.5,
            sel.y as f32 + 1.5,
            sel.w as f32 - 3.0,
            sel.h as f32 - 3.0,
        )
        .unwrap(),
    );
    if let Some(p) = pb.finish() {
        let mut white = Paint::default();
        white.set_color_rgba8(255, 255, 255, 160);
        let thin = Stroke {
            width: 1.0,
            ..Default::default()
        };
        pixmap.stroke_path(&p, &white, &thin, Transform::identity(), None);
    }
}

/// 画 8 个缩放手柄。
fn draw_handles(pixmap: &mut Pixmap, sel: SelRect) {
    let (x0, y0, x1, y1) = (sel.x, sel.y, sel.x + sel.w, sel.y + sel.h);
    let cx = x0 + sel.w / 2;
    let cy = y0 + sel.h / 2;
    for (hx, hy) in [
        (x0, y0),
        (cx, y0),
        (x1, y0),
        (x0, cy),
        (x1, cy),
        (x0, y1),
        (cx, y1),
        (x1, y1),
    ] {
        let r = HANDLE_SIZE / 2;
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        if let Some(rect) = Rect::from_xywh((hx - r) as f32, (hy - r) as f32, HANDLE_SIZE as f32, HANDLE_SIZE as f32) {
            let mut pb = PathBuilder::new();
            pb.push_rect(rect);
            if let Some(p) = pb.finish() {
                pixmap.fill_path(&p, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
            }
        }
    }
}

/// GDI 文本：显示选区尺寸与坐标（物理像素）。
fn draw_info_text(hdc: windows::Win32::Graphics::Gdi::HDC, sel: SelRect, screen_w: i32) {
    unsafe {
        let font = GetStockObject(DEFAULT_GUI_FONT);
        let _ = SelectObject(hdc, font);
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, windows::Win32::Foundation::COLORREF(0x00FFFFFF));
        let text: Vec<u16> = format!("{} × {}   @({},{})", sel.w, sel.h, sel.x, sel.y)
            .encode_utf16()
            .collect();
        let mut sz = windows::Win32::Foundation::SIZE::default();
        let _ = GetTextExtentPoint32W(hdc, &text, &mut sz);
        let tx = sel.x.min(screen_w - sz.cx - 8).max(4);
        let ty = if sel.y > sz.cy + 8 {
            sel.y - sz.cy - 8
        } else {
            sel.y + sel.h + 8
        };
        let _ = TextOutW(hdc, tx, ty, &text);
    }
}

/// UTF-16 结尾 NUL 的 PCWSTR（临时值，仅限同一表达式内使用）。
fn wide(s: &str) -> windows::core::PCWSTR {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    buf.push(0);
    windows::core::PCWSTR::from_raw(buf.as_ptr())
}
