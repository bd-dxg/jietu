// SPDX-License-Identifier: GPL-3.0-only
//! 托盘图标与右键菜单（SYS-1）。

use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use super::{App, ID_TRAY, IDM_EXIT, IDM_LONGSHOT, IDM_SETTINGS, IDM_SHOT, WM_TRAY, icon_resource, wide_array};
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

    /// 托盘回调（SYS-1）：左键 / 右键弹出菜单。
    pub fn handle_tray(&self, lparam: LPARAM) {
        let msg = (lparam.0 & 0xFFFF) as u16;
        if u32::from(msg) == WM_LBUTTONUP || u32::from(msg) == WM_RBUTTONUP {
            self.show_tray_menu();
        }
    }

    /// 右键托盘菜单。
    fn show_tray_menu(&self) {
        unsafe {
            let menu = CreatePopupMenu().unwrap_or_default();
            if menu.0.is_null() {
                return;
            }
            let _ = AppendMenuW(menu, MF_STRING, IDM_SHOT as usize, w!("截图..."));
            let _ = AppendMenuW(menu, MF_STRING, IDM_LONGSHOT as usize, w!("长截图..."));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, IDM_SETTINGS as usize, w!("设置..."));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT as usize, w!("退出"));

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(self.hwnd);
            let id = TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_BOTTOMALIGN,
                pt.x,
                pt.y,
                Some(0),
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            if id.0 != 0 {
                self.handle_command(id.0 as u16);
            }
        }
    }

    /// 菜单项分发。
    pub fn handle_command(&self, id: u16) {
        match id as usize {
            IDM_SHOT => self.start_capture(),
            IDM_LONGSHOT => self.not_yet("长截图功能", "M4 里程碑"),
            IDM_SETTINGS => self.not_yet("设置面板", "M3 里程碑"),
            IDM_EXIT => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            _ => {}
        }
    }
}
