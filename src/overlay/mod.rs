// SPDX-License-Identifier: GPL-3.0-only
//! 覆盖层（CAP-2/3）：覆盖整个虚拟屏幕的无边框置顶窗口，
//! 冻结画面 + 选区暗化（预生成暗图 + 选区回贴原图），支持框选、移动与八向缩放。
//! 选区确认后进入标注模式（EDT-1/EDT-2/EDT-7）：工具栏切换工具与样式，在选区内拖拽即绘制标注。
//!
//! 文件分工：`wndproc` 消息分发、`geometry` 矩形运算、`surface` 像素缓冲与上屏、
//! `cursor` 光标、`selection` 脏区重绘、`interaction` 鼠标交互、`pick` 对象选中与控制柄、
//! `annotate` 标注与输出、`toolbar` 工具栏。

mod annotate;
mod cursor;
mod geometry;
mod interaction;
mod pick;
mod selection;
mod surface;
mod text_edit_state;
mod textinput;
mod toolbar;
mod wndproc;

use tiny_skia::Pixmap;
use windows::Win32::Foundation::{HINSTANCE, HWND};
use windows::Win32::Graphics::Gdi::{DeleteDC, DeleteObject, HBITMAP, HDC, InvalidateRect, UpdateWindow};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::capture::CapturedScreen;
use crate::editor::{self, Document, Object, Point, Tool};

use geometry::SelRect;
use interaction::{Drag, Phase};
use text_edit_state::TextEdit;

const WINDOW_CLASS: PCWSTR = w!("jietu.overlay");

/// 覆盖层运行时状态（完整实现分散在各子模块的 `impl Overlay` 中）。
pub struct Overlay {
    pub capture: CapturedScreen,
    pub original: Pixmap,
    pub dimmed: Pixmap,
    pub display: Pixmap,
    pub selection: Option<SelRect>,
    phase: Phase,
    drag: Option<Drag>,
    /// 标注文档（对象列表 + 撤销/重做，EDT-1/EDT-2/EDT-7）。
    doc: Document,
    /// 当前工具与样式（EDT-7）。
    tool: Tool,
    color_index: usize,
    /// 新建对象的默认线宽（滚轮 / 二级工具栏可连续调整）。
    line_width: f32,
    /// 新建模糊对象的默认半径（EDT-5）。
    blur_radius: f32,
    /// 新建文本对象的默认字号（EDT-4，滚轮可连续调整）。
    font_size: f32,
    filled: bool,
    round: bool,
    /// 正在拖拽、尚未提交的标注对象。
    draft: Option<Object>,
    /// 当前选中的对象索引（EDT-7）；控制柄仅在选中时显示（EDT-3）。
    selected: Option<usize>,
    /// 正在拖动的曲线控制柄：(控制点索引, 按下时的坐标)（EDT-3）。
    ctrl_drag: Option<(usize, Point)>,
    /// 文本输入中状态（EDT-4）。
    text_edit: Option<TextEdit>,
    /// 当前工具栏布局（选区存在时才有）。
    bar: Option<toolbar::Toolbar>,
    pub hwnd: HWND,
    pub cancelled: bool,
    /// 上屏用内存 DC（持有 DIB section，BGRA 像素直接写入 dib_bits）。
    mem_dc: HDC,
    /// DIB section 位图句柄。
    dib_bmp: HBITMAP,
    /// DIB section 像素指针（BGRA，top-down）。
    dib_bits: *mut u8,
}

impl Drop for Overlay {
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

impl Overlay {
    fn new(mut capture: CapturedScreen) -> Result<Self, String> {
        let (w, h) = (capture.width, capture.height);
        let Some(size) = tiny_skia::IntSize::from_wh(w, h) else {
            return Err("虚拟屏幕尺寸非法".into());
        };
        let rgba = std::mem::take(&mut capture.rgba);
        let original = Pixmap::from_vec(rgba, size).ok_or("创建原图像素图失败")?;
        let mut dimmed = original.clone();
        // 暗化（CAP-3：预生成暗图）：覆盖黑色 alpha=120，多线程分块整数乘法
        surface::dim_pixmap(&mut dimmed, 255 - 120);
        let display = dimmed.clone();

        // 创建上屏用 DIB section（BGRA、top-down），像素直接写入其内存。
        let (mem_dc, dib_bmp, dib_bits) = surface::create_dib_surface(w, h)?;

        Ok(Self {
            capture,
            original,
            dimmed,
            display,
            selection: None,
            phase: Phase::Adjusting,
            drag: None,
            doc: Document::new(),
            tool: Tool::Rect,
            color_index: 1, // 默认红色
            line_width: editor::DEFAULT_LINE_WIDTH,
            blur_radius: editor::DEFAULT_BLUR_RADIUS,
            font_size: editor::DEFAULT_FONT_SIZE,
            filled: false,
            round: false,
            draft: None,
            selected: None,
            ctrl_drag: None,
            text_edit: None,
            bar: None,
            hwnd: HWND::default(),
            cancelled: false,
            mem_dc,
            dib_bmp,
            dib_bits,
        })
    }

    /// 注册窗口类并创建覆盖窗口。
    fn create_window(hinstance: HINSTANCE) -> Result<HWND, String> {
        unsafe {
            let wc = WNDCLASSW {
                // CS_DBLCLKS：否则系统不会发 WM_LBUTTONDBLCLK（双击控制柄恢复直线，EDT-3）
                style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
                lpfnWndProc: Some(wndproc::wndproc),
                hInstance: hinstance,
                hCursor: cursor::create_cross_cursor(),
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
        let t_show = std::time::Instant::now();
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
            // 显示后强制整窗重绘：隐藏窗口的 DC 内容在显示时会丢失，
            // 用 InvalidateRect + UpdateWindow 同步触发 WM_PAINT 全窗绘制，避免闪烁。
            let _ = InvalidateRect(Some(hwnd), None, false);
            let _ = UpdateWindow(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if std::env::var_os("JIETU_TIMING").is_some() {
                eprintln!("[jietu] 覆盖层创建到显示完成 {:?}", t_show.elapsed());
            }
        }

        // 局部消息循环：WM_DESTROY 由 wndproc 调 PostQuitMessage(0) 结束（独立线程，
        // 不影响主线程消息队列）。退出后 `Box::from_raw` 释放 Overlay 及其 4 份全屏位图。
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
}

/// UTF-16 结尾 NUL 的 PCWSTR（临时值，仅限同一表达式内使用）。
pub fn wide(s: &str) -> PCWSTR {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    buf.push(0);
    PCWSTR::from_raw(buf.as_ptr())
}
