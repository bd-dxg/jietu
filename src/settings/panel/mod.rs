// SPDX-License-Identifier: GPL-3.0-only
//! 设置面板（M3）：自绘设置窗口（tiny-skia 绘制 + DirectWrite 文字）。
//! 布局为左侧分类导航 + 右侧内容区；主线程模态运行，交互只改工作副本 `config`，
//! 关闭时经 `App::apply_panel_config` 统一写回、保存并重注册热键；
//! 主题与开机自启即时生效（面板自身重绘 + schtasks 调用）。
//! 窗口高度随分类页自适应（内容多少决定高度），避免底部留白。

mod controls;
mod draw;

use tiny_skia::Pixmap;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, DeleteDC, DeleteObject, EndPaint, GetDC, HBITMAP, HDC, InvalidateRect, PAINTSTRUCT, ReleaseDC,
    SRCCOPY, UpdateWindow,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::Ime::{HIMC, ImmAssociateContext};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::app::{App, wide};
use crate::overlay::geometry::in_rect;
use crate::overlay::surface::{convert_rgba_to_bgra, create_dib_surface};
use crate::settings::ThemeSetting;
use crate::theme::{self, Palette};

use controls::{Item, MOD_ALT_BIT, MOD_CTRL_BIT, MOD_SHIFT_BIT, MOD_WIN_BIT, NAV_H, NAV_W, RowId, Section, W};

const WINDOW_CLASS: PCWSTR = w!("jietu.settings");

/// 面板运行时状态。
pub struct Panel {
    pub palette: &'static Palette,
    /// 工作副本（关闭时写回）。
    pub config: crate::settings::Config,
    /// DPI 缩放系数（字号/控件尺寸随屏缩放）。
    pub scale: f32,
    /// 内容区悬停行下标。
    pub hover: Option<usize>,
    /// 导航栏悬停分类。
    pub nav_hover: Option<Section>,
    /// 当前分类页。
    pub section: Section,
    /// 正在捕获的热键。
    pub capturing: Option<RowId>,
    /// 面板窗口句柄（IME 关联操作用）。
    hwnd: HWND,
    /// 捕获期间的 IME 上下文（禁输入法，防止按键被 IME 改写为 0xE5 之类的键码）。
    ime_old: HIMC,
    /// 开机自启真实状态（任务计划是否存在，SYS-4）。
    pub autostart_on: bool,
    /// 配置有改动、待立即应用（热键/主题即时生效，不等面板关闭）。
    config_dirty: bool,
    /// 上屏缓冲（DIB section）。
    dib_bmp: HBITMAP,
    mem_dc: HDC,
    dib_bits: *mut u8,
}

impl Drop for Panel {
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

/// 打开设置面板（主线程模态，阻塞到窗口关闭）。
pub fn run(app: &mut App) {
    let hinstance = app.hinstance;
    let mut panel = Panel {
        palette: theme::palette(theme::resolve(app.config.theme)),
        config: app.config.clone(),
        scale: 1.0,
        hover: None,
        nav_hover: None,
        section: Section::General,
        capturing: None,
        hwnd: HWND::default(),
        ime_old: HIMC(std::ptr::null_mut()),
        // 自启开关显示任务计划真实状态，而非配置猜测（SYS-4）
        autostart_on: crate::app::autostart::autostart_enabled(),
        config_dirty: false,
        dib_bmp: Default::default(),
        mem_dc: Default::default(),
        dib_bits: std::ptr::null_mut(),
    };

    let hwnd = match create_window(hinstance, &mut panel) {
        Ok(h) => h,
        Err(e) => {
            unsafe {
                let _ = MessageBoxW(None, wide(&e), w!("jietu"), MB_OK | MB_ICONERROR);
            }
            return;
        }
    };
    panel.hwnd = hwnd;
    let raw = Box::into_raw(Box::new(panel));
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
        let _ = InvalidateRect(Some(hwnd), None, false);
        let _ = UpdateWindow(hwnd);
    }

    // 面板消息循环：WM_DESTROY 时 wndproc 调 PostQuitMessage(0) 结束。
    // 每轮检查 config_dirty：热键/主题改动立即写回应用（不等关闭），
    // 避免面板开着时旧热键仍生效、新热键无反应的困惑。
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 != 0 {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
            let p = &mut *raw;
            if p.config_dirty {
                p.config_dirty = false;
                let cfg = p.config.clone();
                app.apply_panel_config(&cfg);
            }
        }
    }

    let panel = unsafe { Box::from_raw(raw) };
    // 兜底：恢复输入法 + 最后同步一次配置（如改完直接关窗）
    let mut panel = panel;
    panel.restore_ime();
    app.apply_panel_config(&panel.config);
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

/// 创建设置窗口：初始按 scale=1 粗建，创建后按真实 DPI 校正。
fn create_window(hinstance: HINSTANCE, panel: &mut Panel) -> Result<HWND, String> {
    unsafe {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: Default::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: WINDOW_CLASS,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            // 面板可多次打开：类已注册不算错误
            if windows::Win32::Foundation::GetLastError().0 != 1410 {
                return Err("设置窗口类注册失败".into());
            }
        }

        // 固定尺寸窗口：标题栏 + 系统菜单 + 最小化（禁调整大小，避免黑边）
        const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);
        let (cw, ch) = (W, controls::layout(Section::General, 1.0).height);
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: cw,
            bottom: ch,
        };
        let _ = AdjustWindowRectEx(&mut rect, STYLE, false, WINDOW_EX_STYLE(0));
        let (sw, sh) = (rect.right - rect.left, rect.bottom - rect.top);
        let cx = (GetSystemMetrics(SM_CXSCREEN) - sw) / 2;
        let cy = (GetSystemMetrics(SM_CYSCREEN) - sh) / 2;

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("jietu 设置"),
            STYLE,
            cx,
            cy,
            sw,
            sh,
            None,
            None,
            Some(hinstance),
            None,
        )
        .map_err(|e| format!("创建设置窗口失败：{e}"))?;

        let dpi = GetDpiForWindow(hwnd);
        panel.scale = dpi as f32 / 96.0;
        panel.fit_window(hwnd);
        Ok(hwnd)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // 窗口销毁（DestroyWindow 同步发送）：必须 PostQuitMessage 结束面板消息循环，
    // 否则面板线程/主线程永久阻塞在 GetMessageW（见覆盖层同款修复，M3）。
    if msg == WM_DESTROY {
        unsafe {
            let _ = PostQuitMessage(0);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
    }
    let Some(panel) = app_from(hwnd) else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                let _ = BeginPaint(hwnd, &mut ps);
            }
            repaint_full(hwnd, panel);
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let (nav_hover, hover) = panel.hover_at(x, y);
            if nav_hover != panel.nav_hover || hover != panel.hover {
                panel.nav_hover = nav_hover;
                panel.hover = hover;
                repaint_full(hwnd, panel);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            panel.on_click(x, y);
            panel.fit_window(hwnd);
            repaint_full(hwnd, panel);
            LRESULT(0)
        }
        WM_SYSKEYDOWN | WM_KEYDOWN => {
            // WM_SYSKEYDOWN：Alt 组合键走系统键路径，同样进入热键捕获（否则 Alt+A 捕获不到）
            panel.on_key(wparam.0 as u32);
            // 捕获态吞掉系统键，防止触发系统菜单
            if panel.capturing.is_none() {
                repaint_full(hwnd, panel);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn app_from(hwnd: HWND) -> Option<&'static mut Panel> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if ptr == 0 {
            None
        } else {
            Some(&mut *(ptr as *mut Panel))
        }
    }
}

/// 全量重绘：tiny-skia 绘制 → RGBA→BGRA → BitBlt 上屏。
fn repaint_full(hwnd: HWND, panel: &Panel) {
    let layout = controls::layout(panel.section, panel.scale);
    let Some(mut pixmap) = Pixmap::new(W as u32, layout.height as u32) else {
        return;
    };
    draw::render(&mut pixmap, panel, &layout);
    let w = pixmap.width() as i32;
    let h = pixmap.height() as i32;
    convert_rgba_to_bgra(pixmap.data(), panel.dib_bits as usize, w, 0, 0, w, h, 1);
    unsafe {
        let hdc = GetDC(Some(hwnd));
        if hdc.0.is_null() {
            return;
        }
        let _ = BitBlt(hdc, 0, 0, w, h, Some(panel.mem_dc), 0, 0, SRCCOPY);
        let _ = ReleaseDC(Some(hwnd), hdc);
    }
}

impl Panel {
    /// 命中：导航区 → (分类, None)；内容区 → (None, 行下标)。
    fn hover_at(&self, x: i32, y: i32) -> (Option<Section>, Option<usize>) {
        if x < NAV_W {
            let nh = (NAV_H as f32 * self.scale) as i32;
            return (Section::ALL.get((y / nh.max(1)) as usize).copied(), None);
        }
        (None, self.hit_line(x, y).map(|(i, _, _)| i))
    }

    /// 命中当前节的内容行：返回 (行下标, 控件 ID, 是否落在右侧控件区)。
    fn hit_line(&self, x: i32, y: i32) -> Option<(usize, RowId, bool)> {
        let layout = controls::layout(self.section, self.scale);
        let mut idx = 0usize;
        for item in &layout.items {
            if let Item::Row(r) = item {
                if in_rect(r.rect, x, y) {
                    return Some((idx, r.id, in_rect(r.control, x, y)));
                }
                idx += 1;
            }
        }
        None
    }

    fn on_click(&mut self, x: i32, y: i32) {
        // 左侧导航：切换分类
        if x < NAV_W {
            let nh = (NAV_H as f32 * self.scale) as i32;
            if let Some(sec) = Section::ALL.get((y / nh.max(1)) as usize).copied() {
                self.section = sec;
                self.capturing = None;
                self.hover = None;
            }
            return;
        }
        let Some((_, id, in_control)) = self.hit_line(x, y) else {
            // 点空白处退出捕获态
            self.capturing = None;
            return;
        };
        match id {
            // 热键/工具键：点右侧控件区进入捕获
            RowId::HotkeyShot
            | RowId::HotkeyPin
            | RowId::ToolRect
            | RowId::ToolArrow
            | RowId::ToolText
            | RowId::ToolBlur
            | RowId::ToolHighlight
            | RowId::ToolGlow
                if in_control =>
            {
                self.end_capture();
                self.begin_capture(id);
            }
            RowId::HotkeyShot
            | RowId::HotkeyPin
            | RowId::ToolRect
            | RowId::ToolArrow
            | RowId::ToolText
            | RowId::ToolBlur
            | RowId::ToolHighlight
            | RowId::ToolGlow => self.end_capture(),
            RowId::Autostart => {
                self.end_capture();
                let next = !self.autostart_on;
                if crate::app::autostart::set_autostart(next) {
                    self.autostart_on = next;
                    // 同步配置并标记立即应用（config.auto_start 与任务计划一致）
                    self.config.auto_start = next;
                    self.config_dirty = true;
                } else {
                    // 任务计划创建/删除失败（通常是权限不足）：弹窗反馈并维持现状
                    unsafe {
                        let _ = MessageBoxW(
                            None,
                            wide("开机自启设置失败：无法创建或删除任务计划。\n请以管理员权限重新运行本程序后再试。"),
                            w!("jietu"),
                            MB_OK | MB_ICONWARNING,
                        );
                    }
                }
            }
            RowId::ThemeFollow => {
                self.end_capture();
                self.set_theme(ThemeSetting::Follow);
            }
            RowId::ThemeLight => {
                self.end_capture();
                self.set_theme(ThemeSetting::Light);
            }
            RowId::ThemeDark => {
                self.end_capture();
                self.set_theme(ThemeSetting::Dark);
            }
            _ => self.end_capture(),
        }
    }

    /// 修改主题并即时刷新面板配色。
    fn set_theme(&mut self, t: ThemeSetting) {
        if self.config.theme == t {
            return;
        }
        self.config.theme = t;
        self.config_dirty = true;
        self.palette = theme::palette(theme::resolve(t));
    }

    /// 检查新键是否与其它已配置快捷键冲突（热键：键+修饰都相同；工具键：虚拟键相同即冲突）。
    /// 返回冲突项名称；自身旧值不算冲突。
    fn conflict_for(&self, id: RowId, vk: u32, mods: u32) -> Option<&'static str> {
        let hk = &self.config.hotkey;
        let tk = &self.config.tool_keys;
        // (当前项自值，用于跳过自身)
        let (self_vk, self_mods) = match id {
            RowId::HotkeyShot => (hk.screenshot_key, hk.screenshot_modifiers),
            RowId::HotkeyPin => (hk.pin_key, hk.pin_modifiers),
            RowId::ToolRect => (tk.rect, 0),
            RowId::ToolArrow => (tk.arrow, 0),
            RowId::ToolText => (tk.text, 0),
            RowId::ToolBlur => (tk.blur, 0),
            RowId::ToolHighlight => (tk.highlight, 0),
            RowId::ToolGlow => (tk.glow, 0),
            _ => (u32::MAX, u32::MAX),
        };
        // 已配置键位：热键按「键+修饰」比较，工具键按「虚拟键」比较
        let pairs: [(&str, u32, u32, bool); 8] = [
            ("截图", hk.screenshot_key, hk.screenshot_modifiers, true),
            ("贴图", hk.pin_key, hk.pin_modifiers, true),
            ("工具·矩形", tk.rect, 0, false),
            ("工具·箭头", tk.arrow, 0, false),
            ("工具·文本", tk.text, 0, false),
            ("工具·模糊", tk.blur, 0, false),
            ("工具·荧光笔", tk.highlight, 0, false),
            ("工具·聚光灯", tk.glow, 0, false),
        ];
        for (name, kv, km, is_hotkey) in pairs {
            if kv == self_vk && km == self_mods {
                continue; // 自身旧值
            }
            let hit = if is_hotkey {
                kv == vk && km == mods
            } else {
                // 工具键/跨类比较：按下即按虚拟键触发，故键相同即冲突
                kv == vk
            };
            if hit {
                return Some(name);
            }
        }
        None
    }

    /// 热键捕获键盘输入。
    fn on_key(&mut self, vk: u32) {
        let Some(id) = self.capturing else {
            return;
        };
        match vk {
            0x1B => self.end_capture(), // Esc 取消
            // 纯修饰键：继续等待主键
            0x10 | 0x11 | 0x12 | 0x5B | 0x5C => {}
            _ => {
                let mods = held_modifiers();
                // 与其它已配置键冲突则拒绝（M3）：热键看键+修饰，工具键按虚拟键匹配
                if let Some(name) = self.conflict_for(id, vk, mods) {
                    unsafe {
                        let _ = MessageBoxW(
                            None,
                            wide(&format!("按键 {name} 已被其他快捷键使用，请换一个。")),
                            w!("jietu"),
                            MB_OK | MB_ICONWARNING,
                        );
                    }
                    self.end_capture();
                    return;
                }
                match id {
                    RowId::HotkeyShot => {
                        self.config.hotkey.screenshot_key = vk;
                        self.config.hotkey.screenshot_modifiers = mods;
                    }
                    RowId::HotkeyPin => {
                        self.config.hotkey.pin_key = vk;
                        self.config.hotkey.pin_modifiers = mods;
                    }
                    // 工具切换键：仅绑定虚拟键（覆盖层内按下即切工具，无需修饰键）
                    RowId::ToolRect => self.config.tool_keys.rect = vk,
                    RowId::ToolArrow => self.config.tool_keys.arrow = vk,
                    RowId::ToolText => self.config.tool_keys.text = vk,
                    RowId::ToolBlur => self.config.tool_keys.blur = vk,
                    RowId::ToolHighlight => self.config.tool_keys.highlight = vk,
                    RowId::ToolGlow => self.config.tool_keys.glow = vk,
                    _ => {}
                }
                self.end_capture();
                self.config_dirty = true;
            }
        }
    }

    /// 开始捕获：禁用输入法（防止按键被 IME 改写为 0xE5 等键码）。
    fn begin_capture(&mut self, id: RowId) {
        self.capturing = Some(id);
        unsafe {
            let old = ImmAssociateContext(self.hwnd, HIMC(std::ptr::null_mut()));
            if !old.0.is_null() {
                self.ime_old = old;
            }
        }
    }

    /// 结束捕获：恢复输入法。
    fn end_capture(&mut self) {
        self.capturing = None;
        unsafe {
            if !self.ime_old.0.is_null() {
                let _ = ImmAssociateContext(self.hwnd, self.ime_old);
                self.ime_old = HIMC(std::ptr::null_mut());
            }
        }
    }

    /// 面板关闭兜底：恢复输入法（防止异常路径残留禁用态）。
    fn restore_ime(&mut self) {
        unsafe {
            if !self.ime_old.0.is_null() {
                let _ = ImmAssociateContext(self.hwnd, self.ime_old);
                self.ime_old = HIMC(std::ptr::null_mut());
            }
        }
        self.capturing = None;
    }

    /// 窗口尺寸贴紧当前节内容：客户区高 = 节内容高（切换节后调用，消除底部留白），
    /// 非客户区高度用 GetWindowRect/GetClientRect 实测差值补齐，并重建上屏 DIB。
    fn fit_window(&mut self, hwnd: HWND) {
        let ch = controls::layout(self.section, self.scale).height;
        unsafe {
            if !self.dib_bmp.0.is_null() {
                let _ = DeleteObject(self.dib_bmp.into());
            }
            if !self.mem_dc.0.is_null() {
                let _ = DeleteDC(self.mem_dc);
            }
        }
        match create_dib_surface(W as u32, ch as u32) {
            Ok((dc, bmp, bits)) => {
                self.mem_dc = dc;
                self.dib_bmp = bmp;
                self.dib_bits = bits;
            }
            Err(_) => return,
        }
        unsafe {
            let mut wr = RECT::default();
            let mut cr = windows::Win32::Foundation::RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let _ = GetClientRect(hwnd, &mut cr);
            let nc_h = (wr.bottom - wr.top) - (cr.bottom - cr.top);
            let _ = SetWindowPos(hwnd, None, wr.left, wr.top, W, ch + nc_h, SWP_NOZORDER);
        }
    }
}

/// 当前按住的修饰键位（与 `HotkeyConfig::modifiers` 的 MOD_* 位一致）。
fn held_modifiers() -> u32 {
    let mut m = 0u32;
    if key_down(0x10) {
        m |= MOD_SHIFT_BIT;
    }
    if key_down(0x11) {
        m |= MOD_CTRL_BIT;
    }
    if key_down(0x12) {
        m |= MOD_ALT_BIT;
    }
    if key_down(0x5B) || key_down(0x5C) {
        m |= MOD_WIN_BIT;
    }
    m
}

fn key_down(vk: i32) -> bool {
    unsafe { GetKeyState(vk) as u16 & 0x8000 != 0 }
}
