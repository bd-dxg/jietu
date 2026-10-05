// SPDX-License-Identifier: GPL-3.0-only
//! 覆盖层（CAP-2/3）：覆盖整个虚拟屏幕的无边框置顶窗口，
//! 冻结画面 + 选区暗化（预生成暗图 + 选区回贴原图），支持框选、移动与八向缩放。
//! 选区确认后进入标注模式（EDT-1/EDT-2/EDT-7）：工具栏切换工具与样式，在选区内拖拽即绘制标注。

mod toolbar;

use std::mem::size_of;
use std::sync::atomic::{AtomicIsize, Ordering};

use tiny_skia::{Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CreateBitmap, CreateCompatibleDC, CreateDIBSection,
    DEFAULT_GUI_FONT, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint, GetDC, GetStockObject, HBITMAP, HDC,
    InvalidateRect, PAINTSTRUCT, ReleaseDC, SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT, TextOutW,
    UpdateWindow,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::capture::CapturedScreen;
use crate::editor::{self, Document, Kind, Object, Point, Style, Tool};
use crate::output;
use crate::render;

const WINDOW_CLASS: PCWSTR = w!("jietu.overlay");
const HANDLE_SIZE: i32 = 7; // 手柄边长
const PICK_RADIUS: i32 = 6; // 命中判定半径
const DIRTY_PAD: i32 = 60; // 脏区域外扩（覆盖边框、手柄与信息文字）
const INFO_TEXT_H: i32 = 22; // 信息文字高度（text_rect 与工具栏布局共用）
const INFO_OFFSET: i32 = 8; // 信息文字/工具栏与选区的间隔
const ACCENT: u8 = 26; // 主题蓝
const ACCENT_G: u8 = 115;
const ACCENT_B: u8 = 232;

/// 矩形选区（相对虚拟屏幕左上角的物理像素坐标）。
#[derive(Clone, Copy, Debug, PartialEq)]
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
    pub selection: Option<SelRect>,
    phase: Phase,
    drag: Option<Drag>,
    /// 标注文档（对象列表 + 撤销/重做，EDT-1/EDT-2/EDT-7）。
    doc: Document,
    /// 当前工具与样式（EDT-7）。
    tool: Tool,
    color_index: usize,
    width_index: usize,
    filled: bool,
    round: bool,
    /// 正在拖拽、尚未提交的标注对象。
    draft: Option<Object>,
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
        dim_pixmap(&mut dimmed, 255 - 120);
        let display = dimmed.clone();

        // 创建上屏用 DIB section（BGRA、top-down），像素直接写入其内存。
        let (mem_dc, dib_bmp, dib_bits) = create_dib_surface(w, h)?;

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
            width_index: 1, // 默认 4px
            filled: false,
            round: false,
            draft: None,
            bar: None,
            hwnd: HWND::default(),
            cancelled: false,
            mem_dc,
            dib_bmp,
            dib_bits,
        })
    }

    /// 注册窗口类并创建覆盖窗口。
    fn create_window(hinstance: windows::Win32::Foundation::HINSTANCE) -> Result<HWND, String> {
        unsafe {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                hCursor: create_cross_cursor(),
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

    /// 更新选区并做脏矩形增量重绘（PRD 6.3.2）：
    /// 只处理旧/新选区的装饰与工具栏区域，避免每次鼠标移动全屏重绘。
    fn set_selection(&mut self, new: Option<SelRect>) {
        if same_rect(self.selection, new) {
            return;
        }
        let (vw, vh) = (self.display.width() as i32, self.display.height() as i32);
        let old = self.selection;
        self.selection = new;
        let old_bar = self.bar.as_ref().map(|b| b.rect);
        self.bar = new.map(|s| toolbar::layout(s, vw, vh));
        let new_bar = self.bar.as_ref().map(|b| b.rect);

        let mut dirty: Option<SelRect> = None;
        for r in [
            old.map(|o| expand_rect(o, DIRTY_PAD)),
            new.map(|n| expand_rect(n, DIRTY_PAD)),
            old.map(|o| text_rect(o, vw)),
            new.map(|n| text_rect(n, vw)),
            old_bar.map(|b| expand_rect(b, 2)),
            new_bar.map(|b| expand_rect(b, 2)),
        ]
        .into_iter()
        .flatten()
        {
            dirty = union_rect(dirty, Some(r));
        }
        if let Some(d) = dirty {
            self.repaint(d);
        }
    }

    /// 上屏指定区域：局部 RGBA → BGRA 写入 DIB section，再用 BitBlt 刷到窗口。
    /// 用 BitBlt 而非 SetDIBitsToDevice：后者对 top-down DIB 的源行语义易出错。
    fn present_region(&mut self, r: SelRect) {
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

    /// 重绘指定区域并上屏（PRD 6.3.2 脏矩形）：
    /// 1) 底图复位（选区外恢复暗图、选区内回贴原图）2) 重绘标注 3) 选区装饰 4) 工具栏。
    fn repaint(&mut self, r: SelRect) {
        let r = self.expand_for_objects(r);
        let (w, h) = (self.display.width() as i32, self.display.height() as i32);
        let x0 = r.x.clamp(0, w);
        let y0 = r.y.clamp(0, h);
        let x1 = (r.x + r.w).clamp(0, w);
        let y1 = (r.y + r.h).clamp(0, h);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let region = SelRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        };

        // 1) 底图复位（CAP-3：预生成暗图 + 选区回贴原图）
        restore_region(&mut self.display, &self.dimmed, region);
        if let Some(sel) = self.selection {
            if let Some(overlap) = intersect_rect(region, sel) {
                blit_region(&mut self.display, &self.original, overlap);
            }
        }
        // 2) 标注对象（含正在拖拽的草稿）
        render::draw_objects(
            &mut self.display,
            self.doc.objects(),
            self.draft.as_ref(),
            (x0, y0, x1, y1),
        );
        // 3) 选区边框与手柄（只在与选区装饰区域相交时重画）
        if let Some(sel) = self.selection {
            if intersect_rect(expand_rect(sel, DIRTY_PAD), region).is_some() {
                draw_rect(&mut self.display, sel);
                draw_handles(&mut self.display, sel);
            }
        }
        // 4) 工具栏
        let state = self.bar_state();
        if let Some(bar) = &self.bar {
            if intersect_rect(expand_rect(bar.rect, 2), region).is_some() {
                toolbar::draw(&mut self.display, bar, &state);
            }
        }
        self.present_region(region);
    }

    /// 把脏区扩展到与它相交的所有标注对象的完整包围盒：
    /// 保证被绘制的对象不会画到 `region` 之外，避免像素图与窗口显示不一致。
    fn expand_for_objects(&self, r: SelRect) -> SelRect {
        let mut r = r;
        loop {
            let clip = (r.x, r.y, r.x + r.w, r.y + r.h);
            let mut changed = false;
            for obj in self.doc.objects().iter().chain(self.draft.iter()) {
                let bounds = obj.bounds();
                if render::intersects(&bounds, clip) {
                    let c = render::bounds_to_clip(&bounds);
                    let b = SelRect {
                        x: c.0,
                        y: c.1,
                        w: c.2 - c.0,
                        h: c.3 - c.1,
                    };
                    if let Some(u) = union_rect(Some(r), Some(b)) {
                        if u != r {
                            r = u;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                return r;
            }
        }
    }

    /// 当前样式（EDT-7：颜色 + 线宽）。
    fn current_style(&self) -> Style {
        Style::new(
            editor::PALETTE[self.color_index],
            editor::WIDTH_PRESETS[self.width_index],
        )
    }

    fn bar_state(&self) -> toolbar::State {
        toolbar::State {
            tool: self.tool,
            color: self.color_index,
            width: self.width_index,
            filled: self.filled,
            round: self.round,
            can_undo: self.doc.can_undo(),
            can_redo: self.doc.can_redo(),
        }
    }

    /// 执行工具栏动作（EDT-7）。
    fn apply_action(&mut self, action: toolbar::Action) {
        match action {
            toolbar::Action::Tool(t) => self.tool = t,
            toolbar::Action::Color(i) => self.color_index = i,
            toolbar::Action::Width(i) => self.width_index = i,
            toolbar::Action::ToggleFill => self.filled = !self.filled,
            toolbar::Action::ToggleRound => self.round = !self.round,
            toolbar::Action::Undo => return self.on_undo(),
            toolbar::Action::Redo => return self.on_redo(),
        }
        // 仅工具栏自身外观发生变化
        let bar = self.bar.as_ref().map(|b| b.rect);
        if let Some(rect) = bar {
            self.repaint(expand_rect(rect, 2));
        }
    }

    /// 撤销（Ctrl+Z）。
    fn on_undo(&mut self) {
        if self.doc.undo() {
            self.repaint_after_history();
        }
    }

    /// 重做（Ctrl+Y / Ctrl+Shift+Z）。
    fn on_redo(&mut self) {
        if self.doc.redo() {
            self.repaint_after_history();
        }
    }

    /// 撤销/重做后重绘：被撤销的对象可能位于选区之外，而另一份历史快照里的
    /// 对象包围盒不可得，故整屏重绘（仅按键/按钮触发，不在拖拽路径上）。
    fn repaint_after_history(&mut self) {
        let (vw, vh) = (self.display.width() as i32, self.display.height() as i32);
        self.repaint(SelRect {
            x: 0,
            y: 0,
            w: vw,
            h: vh,
        });
    }

    /// 在选区内开始拖拽绘制标注（EDT-1/EDT-2）。
    fn start_draw(&mut self, x: i32, y: i32) {
        let p = Point::new(x as f32, y as f32);
        let kind = match self.tool {
            Tool::Rect => Kind::Rect {
                a: p,
                b: p,
                radius: if self.round { 12.0 } else { 0.0 },
                filled: self.filled,
            },
            Tool::Arrow => Kind::Arrow { from: p, to: p },
        };
        self.doc.begin();
        self.draft = Some(Object {
            kind,
            style: self.current_style(),
        });
    }

    /// 草稿对象当前占据的像素区域。
    fn draft_region(&self) -> Option<SelRect> {
        let c = render::bounds_to_clip(&self.draft.as_ref()?.bounds());
        Some(SelRect {
            x: c.0,
            y: c.1,
            w: c.2 - c.0,
            h: c.3 - c.1,
        })
    }

    /// 开始新的框选（返回初始 1px 选区）。
    fn begin_select(&mut self, x: i32, y: i32) -> SelRect {
        let sel = SelRect { x, y, w: 1, h: 1 };
        self.phase = Phase::Selecting;
        self.drag = Some(Drag {
            handle: Handle::Move,
            orig: sel,
            px: x,
            py: y,
        });
        sel
    }

    fn on_lbutton_down(&mut self, x: i32, y: i32) {
        // 工具栏优先（EDT-7）：点在工具栏上只切换状态，不开始绘制
        let bar = self.bar.clone();
        if let Some(bar) = &bar {
            if in_rect(bar.rect, x, y) {
                if let Some(action) = bar.hit(x, y) {
                    self.apply_action(action);
                }
                return;
            }
        }
        let new = match self.selection {
            None => Some(self.begin_select(x, y)),
            Some(sel) => {
                if let Some(handle) = hit_handle(sel, x, y) {
                    self.drag = Some(Drag {
                        handle,
                        orig: sel,
                        px: x,
                        py: y,
                    });
                    self.phase = Phase::Adjusting;
                    Some(sel)
                } else if sel.contains(x, y) {
                    if space_down() {
                        // 按住空格拖动：移动整个选区（CAP-2）
                        self.drag = Some(Drag {
                            handle: Handle::Move,
                            orig: sel,
                            px: x,
                            py: y,
                        });
                        self.phase = Phase::Adjusting;
                        Some(sel)
                    } else {
                        // 选区内按下：用当前工具绘制标注（EDT-1/EDT-2）
                        self.start_draw(x, y);
                        return;
                    }
                } else {
                    // 远处点击：开始新选区
                    Some(self.begin_select(x, y))
                }
            }
        };
        self.set_selection(new);
    }

    fn on_mouse_move(&mut self, x: i32, y: i32) {
        // 绘制中：草稿终点跟随鼠标（限制在选区内）
        if self.draft.is_some() {
            let Some(sel) = self.selection else {
                return;
            };
            let cx = x.clamp(sel.x, (sel.x + sel.w - 1).max(sel.x));
            let cy = y.clamp(sel.y, (sel.y + sel.h - 1).max(sel.y));
            let before = self.draft_region();
            if let Some(draft) = self.draft.as_mut() {
                match &mut draft.kind {
                    Kind::Rect { b, .. } => {
                        b.x = cx as f32;
                        b.y = cy as f32;
                    }
                    Kind::Arrow { to, .. } => {
                        to.x = cx as f32;
                        to.y = cy as f32;
                    }
                }
            }
            if let Some(d) = union_rect(before, self.draft_region()) {
                self.repaint(expand_rect(d, 2));
            }
            return;
        }
        let Some(drag) = &self.drag else {
            return;
        };
        let (dx, dy) = (x - drag.px, y - drag.py);
        let handle = drag.handle;
        let orig = drag.orig;
        let phase = self.phase;
        let (vw, vh) = (self.display.width() as i32, self.display.height() as i32);
        let max_x = vw.saturating_sub(1);
        let max_y = vh.saturating_sub(1);
        let x = x.clamp(0, max_x);
        let y = y.clamp(0, max_y);

        let new = if phase == Phase::Selecting {
            // 框选阶段：以按下点为锚点拉伸矩形
            SelRect::normalized(orig.x, orig.y, x, y)
        } else {
            match handle {
                Handle::Move => {
                    let lim_x = (max_x - orig.w).max(0);
                    let lim_y = (max_y - orig.h).max(0);
                    SelRect {
                        x: (orig.x + dx).clamp(0, lim_x),
                        y: (orig.y + dy).clamp(0, lim_y),
                        w: orig.w,
                        h: orig.h,
                    }
                }
                Handle::N => SelRect::normalized(orig.x, y, orig.x + orig.w, orig.y + orig.h),
                Handle::S => SelRect {
                    x: orig.x,
                    y: orig.y,
                    w: orig.w,
                    h: (y - orig.y).max(1),
                },
                Handle::W => SelRect::normalized(x, orig.y, orig.x + orig.w, orig.y + orig.h),
                Handle::E => SelRect {
                    x: orig.x,
                    y: orig.y,
                    w: (x - orig.x).max(1),
                    h: orig.h,
                },
                Handle::NE => SelRect::normalized(orig.x, y, orig.x + orig.w, orig.y + orig.h),
                Handle::NW => SelRect::normalized(x, y, orig.x + orig.w, orig.y + orig.h),
                Handle::SE => SelRect::normalized(orig.x, orig.y, x.max(orig.x + 1), y.max(orig.y + 1)),
                Handle::SW => SelRect::normalized(x, orig.y, orig.x + orig.w, orig.y + orig.h),
            }
        };
        self.set_selection(Some(new));
    }

    fn on_lbutton_up(&mut self) {
        // 结束绘制：过短视为误触，丢弃；提交后进入撤销栈
        if let Some(draft) = self.draft.take() {
            let kept = !is_degenerate(&draft);
            if kept {
                self.doc.push(draft);
            }
            self.doc.commit(kept);
            let c = render::bounds_to_clip(&draft.bounds());
            let mut region = SelRect {
                x: c.0,
                y: c.1,
                w: c.2 - c.0,
                h: c.3 - c.1,
            };
            if let Some(bar) = self.bar.as_ref() {
                // 撤销/重做按钮的可用性随之变化
                region = union_rect(Some(region), Some(expand_rect(bar.rect, 2))).unwrap_or(region);
            }
            self.repaint(expand_rect(region, 2));
            return;
        }
        self.drag = None;
        let new = match self.selection {
            Some(sel) if sel.w < 2 || sel.h < 2 => None, // 误触，放弃
            other => {
                if other.is_some() {
                    self.phase = Phase::Adjusting;
                }
                other
            }
        };
        self.set_selection(new);
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
            let Some(cropped) = crop_pixmap(&self.original, sel) else {
                return;
            };
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
            let Some(cropped) = crop_pixmap(&self.original, sel) else {
                return;
            };
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
        // 主动绘制，不靠系统擦除；只重绘无效区域
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                let _ = BeginPaint(hwnd, &mut ps);
            }
            let rc = ps.rcPaint;
            let w = overlay.display.width() as i32;
            let h = overlay.display.height() as i32;
            let x0 = rc.left.clamp(0, w);
            let y0 = rc.top.clamp(0, h);
            let x1 = rc.right.clamp(0, w);
            let y1 = rc.bottom.clamp(0, h);
            if x1 > x0 && y1 > y0 {
                overlay.present_region(SelRect {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                });
            }
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        // 按住空格时切换为自绘抓手光标，提示「空格 + 拖动可移动选区」
        WM_SETCURSOR => {
            apply_cursor(hwnd, space_down());
            LRESULT(1)
        }
        WM_KEYDOWN => {
            // 空格按下立即切抓手光标：系统只在鼠标移动时发 WM_SETCURSOR，不能等它
            if wparam.0 as u32 == 0x20 {
                apply_cursor(hwnd, true);
                return LRESULT(0);
            }
            match wparam.0 as u32 {
                0x1B => overlay.on_cancel(),              // Esc
                0x0D => overlay.on_copy(),                // Enter
                0x43 if ctrl_down() => overlay.on_copy(), // Ctrl+C
                0x53 if ctrl_down() => overlay.on_save(), // Ctrl+S
                0x5A if ctrl_down() => {
                    // Ctrl+Z 撤销 / Ctrl+Shift+Z 重做（EDT-7）
                    if shift_down() {
                        overlay.on_redo()
                    } else {
                        overlay.on_undo()
                    }
                }
                0x59 if ctrl_down() => overlay.on_redo(), // Ctrl+Y
                _ => {}
            }
            LRESULT(0)
        }
        WM_KEYUP => {
            if wparam.0 as u32 == 0x20 {
                apply_cursor(hwnd, false);
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

/// 区域从 src 拷贝到 dst（用于从暗图恢复旧选区区域）。
fn restore_region(dst: &mut Pixmap, src: &Pixmap, r: SelRect) {
    blit_region(dst, src, r);
}

/// 两个可选矩形是否相同。
fn same_rect(a: Option<SelRect>, b: Option<SelRect>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.x == y.x && x.y == y.y && x.w == y.w && x.h == y.h,
        _ => false,
    }
}

/// 矩形外扩 pad 像素。
fn expand_rect(r: SelRect, pad: i32) -> SelRect {
    SelRect {
        x: r.x - pad,
        y: r.y - pad,
        w: r.w + pad * 2,
        h: r.h + pad * 2,
    }
}

/// 两可选矩形的并集。
fn union_rect(a: Option<SelRect>, b: Option<SelRect>) -> Option<SelRect> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let x0 = a.x.min(b.x);
            let y0 = a.y.min(b.y);
            let x1 = (a.x + a.w).max(b.x + b.w);
            let y1 = (a.y + a.h).max(b.y + b.h);
            Some(SelRect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            })
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// 估算信息文本占位矩形（供脏区域使用，宽度保守）。
fn text_rect(sel: SelRect, screen_w: i32) -> SelRect {
    let len = format!("{} × {}   @({},{})", sel.w, sel.h, sel.x, sel.y)
        .chars()
        .count() as i32;
    let tw = (len * 10 + 16).min(screen_w.max(0));
    let ty = if sel.y > INFO_TEXT_H + INFO_OFFSET {
        sel.y - INFO_TEXT_H - INFO_OFFSET
    } else {
        sel.y + sel.h + INFO_OFFSET
    };
    SelRect {
        x: sel.x.min((screen_w - tw - 8).max(4)).max(4),
        y: ty,
        w: tw,
        h: INFO_TEXT_H,
    }
}

/// RGBA → BGRA 写入目标缓冲区：转换 (x0,y0,rw,rh) 区域，按行多线程分块。
/// 目标地址用 usize 传递（裸指针不满足 Send）。
fn convert_rgba_to_bgra(src: &[u8], dst_addr: usize, w: i32, x0: i32, y0: i32, rw: i32, rh: i32, workers: usize) {
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

/// 暗化像素图：逐通道乘以 k/256，多线程分块并行。
fn dim_pixmap(pixmap: &mut Pixmap, k: u32) {
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
fn create_dib_surface(w: u32, h: u32) -> Result<(HDC, HBITMAP, *mut u8), String> {
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

/// 创建白十字 + 黑描边彩色光标（32bpp alpha），保证在暗化画面上清晰可见。
fn create_cross_cursor() -> HCURSOR {
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

/// 裁剪出选区像素；尺寸非法时返回 None。
fn crop_pixmap(src: &Pixmap, r: SelRect) -> Option<Pixmap> {
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
    if let Some(inner) = Rect::from_xywh(
        sel.x as f32 + 1.5,
        sel.y as f32 + 1.5,
        sel.w as f32 - 3.0,
        sel.h as f32 - 3.0,
    ) {
        pb.push_rect(inner);
    }
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

/// GDI 文本：显示选区尺寸与坐标（物理像素），位置由 text_rect 确定。
fn draw_info_text(hdc: windows::Win32::Graphics::Gdi::HDC, sel: SelRect, screen_w: i32) {
    let r = text_rect(sel, screen_w);
    unsafe {
        let font = GetStockObject(DEFAULT_GUI_FONT);
        let _ = SelectObject(hdc, font);
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, windows::Win32::Foundation::COLORREF(0x00FFFFFF));
        let text: Vec<u16> = format!("{} × {}   @({},{})", sel.w, sel.h, sel.x, sel.y)
            .encode_utf16()
            .collect();
        let _ = TextOutW(hdc, r.x + 4, r.y + 4, &text);
    }
}

/// UTF-16 结尾 NUL 的 PCWSTR（临时值，仅限同一表达式内使用）。
fn wide(s: &str) -> windows::core::PCWSTR {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    buf.push(0);
    windows::core::PCWSTR::from_raw(buf.as_ptr())
}

/// 点是否在矩形内（半开区间）。
fn in_rect(r: SelRect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

/// 两矩形交集；不相交时返回 None。
fn intersect_rect(a: SelRect, b: SelRect) -> Option<SelRect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    if x1 <= x0 || y1 <= y0 {
        None
    } else {
        Some(SelRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

/// 误触判定：长宽均不足 2px 的对象丢弃（EDT-1/EDT-2）。
fn is_degenerate(obj: &Object) -> bool {
    match obj.kind {
        Kind::Rect { a, b, .. } => (a.x - b.x).abs() < 2.0 && (a.y - b.y).abs() < 2.0,
        Kind::Arrow { from, to } => (from.x - to.x).abs() < 2.0 && (from.y - to.y).abs() < 2.0,
    }
}

/// 切换当前光标：按住空格时用自绘抓手，否则用窗口类光标（十字）。
fn apply_cursor(hwnd: HWND, space: bool) {
    unsafe {
        let cur = if space {
            hand_cursor()
        } else {
            HCURSOR(GetClassLongPtrW(hwnd, GCLP_HCURSOR) as *mut _)
        };
        let _ = SetCursor(Some(cur));
    }
}

/// 修饰键是否按下（`GetKeyState` 高位为 1 表示按下）。
fn key_down(vk: i32) -> bool {
    unsafe { GetKeyState(vk) as u16 & 0x8000 != 0 }
}

fn ctrl_down() -> bool {
    key_down(0x11)
}

fn shift_down() -> bool {
    key_down(0x10)
}

/// 空格键是否按下（按住空格拖动可移动整个选区，CAP-2）。
fn space_down() -> bool {
    key_down(0x20)
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
