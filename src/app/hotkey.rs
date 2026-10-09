// SPDX-License-Identifier: GPL-3.0-only
//! 全局热键（SYS-2）：注册、冲突提示、无修饰键风险首次提示。

use windows::Win32::Foundation::GetLastError;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_ICONWARNING, MB_OK};

use super::{App, ID_HOTKEY_PIN, ID_HOTKEY_SHOT, message_box};

const ERROR_HOTKEY_ALREADY_REGISTERED: u32 = 1409;

impl App {
    /// 注册全局热键（SYS-2），失败时提示冲突。
    /// 先注销旧注册：改键后（M3 设置面板）id 可能先前已注册，直接 Register 会报冲突。
    pub fn register_hotkeys(&self) {
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), ID_HOTKEY_SHOT);
            let _ = UnregisterHotKey(Some(self.hwnd), ID_HOTKEY_PIN);
        }
        let hk = &self.config.hotkey;
        self.register_one(ID_HOTKEY_SHOT, hk.screenshot_key, hk.screenshot_modifiers, "截图");
        self.register_one(ID_HOTKEY_PIN, hk.pin_key, hk.pin_modifiers, "贴图");
    }

    fn register_one(&self, id: i32, key: u32, modifiers: u32, name: &str) {
        let mods = mod_flags(modifiers);
        let ok = unsafe { RegisterHotKey(Some(self.hwnd), id, mods, key) }.is_ok();
        if !ok {
            let err = unsafe { GetLastError() };
            let text = if err.0 == ERROR_HOTKEY_ALREADY_REGISTERED {
                format!("热键 {name} 注册失败：该键已被其他程序占用。\n请在设置中更换热键。")
            } else {
                format!("热键 {name} 注册失败，错误码 {}.", err.0)
            };
            message_box(Some(self.hwnd), &text, MB_OK | MB_ICONWARNING);
        }
    }

    /// SYS-2：无修饰键的全局热键会抢占其他程序的按键，仅首次启动提示一次。
    pub fn maybe_warn_no_modifier_clash(&mut self) {
        if !self.config.hotkey_warned
            && self.config.hotkey.screenshot_modifiers == 0
            && self.config.hotkey.pin_modifiers == 0
        {
            self.config.hotkey_warned = true;
            self.config.save();
            message_box(
                Some(self.hwnd),
                "截图(F1)与贴图(F3)热键未使用修饰键，会覆盖其他程序的按键。\n如影响其他软件使用，请在设置中更换热键。",
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }

    /// 全局热键分发（SYS-2）。
    pub fn handle_hotkey(&self, id: i32) {
        match id {
            ID_HOTKEY_SHOT => self.start_capture(),
            ID_HOTKEY_PIN => self.not_yet("贴图功能", "M2b 里程碑"),
            _ => {}
        }
    }
}

/// 组装热键修饰键位标志。
fn mod_flags(modifiers: u32) -> HOT_KEY_MODIFIERS {
    let mut m = HOT_KEY_MODIFIERS(0);
    if modifiers & MOD_ALT.0 != 0 {
        m |= MOD_ALT;
    }
    if modifiers & MOD_CONTROL.0 != 0 {
        m |= MOD_CONTROL;
    }
    if modifiers & MOD_SHIFT.0 != 0 {
        m |= MOD_SHIFT;
    }
    if modifiers & MOD_WIN.0 != 0 {
        m |= MOD_WIN;
    }
    m |= MOD_NOREPEAT;
    m
}
