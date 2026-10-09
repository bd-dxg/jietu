// SPDX-License-Identifier: GPL-3.0-only
//! 应用壳（SYS-1/2/3/4/6）：单实例、隐藏消息窗口、托盘、热键、自启、主题变更。
//! 拆分：`run` 入口与消息分发见本文件，托盘在 `tray`，热键在 `hotkey`，自启在 `autostart`。

pub mod autostart;
mod hotkey;
pub mod popmenu;
mod tray;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::settings::Config;
use crate::theme;

/// 消息窗口类名（`FindWindowW` 唤起已有实例时用到）。
pub const WINDOW_CLASS: PCWSTR = w!("jietu.hidden");
const MUTEX_NAME: PCWSTR = w!("Local\\jietu.single.instance");
const WM_TRAY: u32 = WM_APP + 1;
const WM_ACTIVATE: u32 = WM_APP + 2; // 单实例唤起已有实例

pub const ID_TRAY: u32 = 1;
pub const ID_HOTKEY_SHOT: i32 = 101;
pub const ID_HOTKEY_PIN: i32 = 102;

/// 托盘菜单项 id（供 `tray` 子模块使用）。
pub const IDM_SHOT: usize = 40001;
pub const IDM_LONGSHOT: usize = 40002;
pub const IDM_SETTINGS: usize = 40003;
pub const IDM_EXIT: usize = 40004;

/// 应用运行时状态。
pub struct App {
    pub config: Config,
    pub hwnd: HWND,
    pub hinstance: HINSTANCE,
    /// 设置面板是否已打开（防托盘菜单重入，SYS-3）。
    pub settings_open: bool,
}

/// 入口：单实例检查 → 创建消息窗口 → 托盘 / 热键 → 消息循环。
/// 返回进程退出码。
pub fn run() -> i32 {
    if !acquire_single_instance() {
        // 已有实例在运行，已向其发送唤起消息，本进程直接退出。
        return 0;
    }

    let config = Config::load();
    let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap_or_default().into();

    let hwnd = match create_message_window(hinstance) {
        Ok(hwnd) => hwnd,
        Err(msg) => {
            unsafe { MessageBoxW(None, wide(&msg), w!("jietu"), MB_OK | MB_ICONERROR) };
            return 1;
        }
    };

    let mut app = Box::new(App {
        config,
        hwnd,
        hinstance,
        settings_open: false,
    });
    app.init_tray();
    app.maybe_warn_no_modifier_clash();
    app.register_hotkeys();

    // 存入窗口用户数据，wndproc 通过它访问 App。
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);

        let mut msg = MSG::default();
        loop {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            if ret.0 == 0 {
                break; // WM_QUIT
            }
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        // 正常退出：恢复 Box 释放内存，并移除托盘图标。
        let app = Box::from_raw(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App);
        app.remove_tray();
    }
    0
}

/// 单实例：命名 Mutex 已存在说明已有实例，唤起后返回 false。
fn acquire_single_instance() -> bool {
    match unsafe { CreateMutexW(None, false, MUTEX_NAME) } {
        Ok(mutex) => {
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                // 句柄无 Drop（Copy 类型），let _ 保持句柄打开直到进程退出，
                // 确保 mutex 对象存活，单实例有效。
                let _ = mutex;
                let hwnd = unsafe { FindWindowW(WINDOW_CLASS, None) }.unwrap_or_default();
                let _ = unsafe { PostMessageW(Some(hwnd), WM_ACTIVATE, WPARAM(0), LPARAM(0)) };
                false
            } else {
                let _ = mutex; // 保持到进程退出
                true
            }
        }
        Err(_) => true, // 创建失败不阻塞运行，退化为无单实例保护
    }
}

impl App {
    pub fn start_capture(&self) {
        let hinstance = self.hinstance.0 as usize;
        // 截图开始时解析一次主题调色板（M3：跟随设置，浅色模式工具栏为浅色）
        let palette = crate::theme::palette(crate::theme::resolve(self.config.theme));
        // 工具切换键配置 + 默认工具（记住上次使用，M3）
        let tool_keys = self.config.tool_keys.clone();
        let default_tool = crate::settings::last_tool();
        std::thread::spawn(move || {
            let hinstance = HINSTANCE(hinstance as *mut _);
            match crate::capture::capture_virtual_screen() {
                Ok(screen) => {
                    crate::overlay::Overlay::run(screen, palette, tool_keys, default_tool, hinstance);
                }
                Err(e) => unsafe {
                    let _ = MessageBoxW(None, wide(&format!("抓屏失败：{e}")), w!("jietu"), MB_OK | MB_ICONERROR);
                },
            }
        });
    }

    pub fn not_yet(&self, feature: &str, milestone: &str) {
        unsafe {
            let text = format!("{feature}尚未实现（{milestone}）。");
            let _ = MessageBoxW(Some(self.hwnd), wide(&text), w!("jietu"), MB_OK | MB_ICONINFORMATION);
        }
    }

    /// 打开设置面板（M3）：主线程模态运行，防重入。
    pub fn open_settings(&mut self) {
        if self.settings_open {
            return;
        }
        self.settings_open = true;
        crate::settings::panel::run(self);
        self.settings_open = false;
    }

    /// 应用设置面板的工作副本：写回配置、保存、重注册热键。
    pub fn apply_panel_config(&mut self, cfg: &Config) {
        self.config = cfg.clone();
        self.config.save();
        self.register_hotkeys();
    }

    /// WM_SETTINGCHANGE：主题变更时刷新（SYS-6）。
    fn handle_setting_change(&self, lparam: LPARAM) {
        if theme::is_theme_change(lparam.0 as usize) {
            // M0 记录当前主题；设置面板（M3）与覆盖层（M1）消费。
            let _ = theme::system_theme();
        }
    }
}

/// 注册消息窗口类并创建窗口（隐藏）。
fn create_message_window(hinstance: HINSTANCE) -> Result<HWND, String> {
    unsafe {
        let icon = LoadIconW(Some(hinstance), icon_resource()).unwrap_or_default();
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            hIcon: icon,
            hCursor: Default::default(),
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: WINDOW_CLASS,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(format!("注册窗口类失败，错误码 {}", GetLastError().0));
        }

        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS,
            w!("jietu"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance),
            None,
        ) {
            Ok(h) => h,
            Err(e) => return Err(format!("创建窗口失败：{e}")),
        };
        Ok(hwnd)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as u16;
            if let Some(app) = app_from(hwnd) {
                app.handle_command(id);
            }
            LRESULT(0)
        }
        WM_TRAY => {
            if let Some(app) = app_from(hwnd) {
                app.handle_tray(lparam);
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            let id = wparam.0 as i32;
            if let Some(app) = app_from(hwnd) {
                app.handle_hotkey(id);
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            if let Some(app) = app_from(hwnd) {
                app.handle_setting_change(lparam);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn app_from(hwnd: HWND) -> Option<&'static mut App> {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if ptr == 0 { None } else { Some(&mut *(ptr as *mut App)) }
    }
}

/// 程序主图标资源（app.rc 中的 ID=1）。
pub fn icon_resource() -> PCWSTR {
    PCWSTR::from_raw(1 as *const u16)
}

/// UTF-16 结尾 NUL 的 PCWSTR（临时值，仅限同一表达式内使用）。
pub fn wide(s: &str) -> PCWSTR {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    buf.push(0);
    PCWSTR::from_raw(buf.as_ptr())
}

/// 将字符串写入固定长度 u16 数组（用于 NOTIFYICONDATAW.szTip 等）。
pub fn wide_array<const N: usize>(s: &str) -> [u16; N] {
    let mut arr = [0u16; N];
    let mut i = 0;
    for u in s.encode_utf16().take(N - 1) {
        arr[i] = u;
        i += 1;
    }
    arr
}
