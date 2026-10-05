// SPDX-License-Identifier: GPL-3.0-only
//! 系统主题检测（SYS-6）：
//! 启动时读取 `AppsUseLightTheme`；运行期由 wndproc 监听
//! `WM_SETTINGCHANGE("ImmersiveColorSet")` 实时切换，见 `crate::app::wndproc`。

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::{PCWSTR, w};

/// 系统当前主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

/// 读取 `AppsUseLightTheme`（1 = 浅色，0 = 深色）；读取失败时按深色处理。
fn system_light_theme() -> Option<i32> {
    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let name = w!("AppsUseLightTheme");
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    unsafe {
        let err = RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        );
        if err == ERROR_SUCCESS && size == 4 {
            Some(value as i32)
        } else {
            None
        }
    }
}

/// 当前系统主题。
pub fn system_theme() -> Theme {
    match system_light_theme() {
        Some(1) => Theme::Light,
        _ => Theme::Dark,
    }
}

/// 判断 `WM_SETTINGCHANGE` 的 lParam 是否表示主题变更（"ImmersiveColorSet"）。
pub fn is_theme_change(lparam: usize) -> bool {
    let ptr = PCWSTR::from_raw(lparam as *const u16);
    unsafe { ptr.to_string().is_ok_and(|s| s == "ImmersiveColorSet") }
}
