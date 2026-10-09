// SPDX-License-Identifier: GPL-3.0-only
//! 设置面板交互（M3）：悬停/点击/热键捕获/冲突检测/窗口自适应。
//! 窗口过程与绘制见 `wnd` 与 `draw`，消息循环见 `super::run`。

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Input::Ime::{HIMC, ImmAssociateContext};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, MB_ICONWARNING, MB_OK, SWP_NOZORDER, SetWindowPos,
};

use crate::app::message_box;
use crate::overlay::geometry::in_rect;
use crate::overlay::surface::create_dib_surface;
use crate::settings::ThemeSetting;
use crate::theme::{self};

use super::controls::{
    Item, MOD_ALT_BIT, MOD_CTRL_BIT, MOD_SHIFT_BIT, MOD_WIN_BIT, NAV_H, NAV_W, RowId, Section, conflict_for,
};
use super::{Panel, W};

impl Panel {
    /// 命中：导航区 → (分类, None)；内容区 → (None, 行下标)。
    pub(super) fn hover_at(&self, x: i32, y: i32) -> (Option<Section>, Option<usize>) {
        if x < NAV_W {
            let nh = (NAV_H as f32 * self.scale) as i32;
            return (Section::ALL.get((y / nh.max(1)) as usize).copied(), None);
        }
        (None, self.hit_line(x, y).map(|(i, _, _)| i))
    }

    /// 命中当前节的内容行：返回 (行下标, 控件 ID, 是否落在右侧控件区)。
    fn hit_line(&self, x: i32, y: i32) -> Option<(usize, RowId, bool)> {
        let layout = super::controls::layout(self.section, self.scale);
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

    pub(super) fn on_click(&mut self, x: i32, y: i32) {
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
                    message_box(
                        None,
                        "开机自启设置失败：无法创建或删除任务计划。\n请以管理员权限重新运行本程序后再试。",
                        MB_OK | MB_ICONWARNING,
                    );
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

    /// 热键捕获键盘输入。
    pub(super) fn on_key(&mut self, vk: u32) {
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
                if let Some(name) = conflict_for(&self.config.hotkey, &self.config.tool_keys, id, vk, mods) {
                    message_box(
                        None,
                        &format!("按键 {name} 已被其他快捷键使用，请换一个。"),
                        MB_OK | MB_ICONWARNING,
                    );
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
    pub(super) fn restore_ime(&mut self) {
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
    pub(super) fn fit_window(&mut self, hwnd: HWND) {
        let ch = super::controls::layout(self.section, self.scale).height;
        unsafe {
            if !self.dib_bmp.0.is_null() {
                let _ = windows::Win32::Graphics::Gdi::DeleteObject(self.dib_bmp.into());
            }
            if !self.mem_dc.0.is_null() {
                let _ = windows::Win32::Graphics::Gdi::DeleteDC(self.mem_dc);
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
            let mut cr = RECT::default();
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
