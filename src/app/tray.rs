// SPDX-License-Identifier: GPL-3.0-only
//! 托盘图标与右键菜单（SYS-1）。
//! 右键菜单为自绘弹出窗口（见 `popmenu`）：无边框 + 阴影、单击选中 / 双击执行，
//! 颜色跟随主题调色板（系统原生菜单固定跟随 Windows 主题，深色设置下仍白底且带边框）。

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{App, ID_TRAY, IDM_EXIT, IDM_SETTINGS, IDM_SHOT, WM_TRAY, icon_resource, popmenu, wide_array};
use std::mem::size_of;

impl App {
    /// 添加托盘图标（SYS-1）。
    pub fn init_tray(&self) {
        let nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ID_TRAY,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: unsafe { LoadIconW(Some(self.hinstance), icon_resource()).unwrap_or_default() },
            szTip: wide_array("jietu 截图工具"),
            ..Default::default()
        };
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        }
    }

    /// 移除托盘图标。
    pub fn remove_tray(&self) {
        let nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ID_TRAY,
            ..Default::default()
        };
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
    }

    /// 托盘回调（SYS-1）：左键打开设置，右键弹出菜单。
    pub fn handle_tray(&mut self, lparam: LPARAM) {
        let msg = (lparam.0 & 0xFFFF) as u16;
        if u32::from(msg) == WM_LBUTTONUP {
            self.open_settings();
        } else if u32::from(msg) == WM_RBUTTONUP {
            self.show_tray_menu();
        }
    }

    /// 右键托源自绘菜单：设置 / 退出。
    fn show_tray_menu(&mut self) {
        let items = vec![
            popmenu::Item {
                id: IDM_SETTINGS,
                text: "设置...",
                separator_before: false,
            },
            popmenu::Item {
                id: IDM_EXIT,
                text: "退出",
                separator_before: false,
            },
        ];
        let palette = crate::theme::palette(crate::theme::resolve(self.config.theme));
        if let Some(id) = popmenu::popup(self.hwnd, items, palette) {
            self.handle_command(id as u16);
        }
    }

    /// 菜单项分发。
    pub fn handle_command(&mut self, id: u16) {
        match id as usize {
            IDM_SHOT => self.start_capture(),
            IDM_SETTINGS => self.open_settings(),
            IDM_EXIT => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            _ => {}
        }
    }
}
