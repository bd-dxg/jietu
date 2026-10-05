// SPDX-License-Identifier: GPL-3.0-only
//! 应用壳：托盘（SYS-1）、全局热键（SYS-2）、单实例（SYS-3）、
//! 开机自启（SYS-4）、消息分发。基于 Win32 自管消息循环（事件驱动，无轮询）。

use std::mem::size_of;

use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_HOTKEY_ALREADY_REGISTERED, ERROR_SUCCESS, GetLastError, HINSTANCE, HWND, LPARAM,
    LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
};
use windows::Win32::UI::Shell::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::settings::Config;
use crate::theme;

const WINDOW_CLASS: PCWSTR = w!("jietu.hidden");
const MUTEX_NAME: PCWSTR = w!("Local\\jietu.single.instance");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const STARTUP_APPROVED_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run");
const RUN_VALUE: PCWSTR = w!("jietu");

const WM_TRAY: u32 = WM_APP + 1;
const WM_ACTIVATE: u32 = WM_APP + 2; // 单实例唤起已有实例

const ID_TRAY: u32 = 1;
const ID_HOTKEY_SHOT: i32 = 101;
const ID_HOTKEY_PIN: i32 = 102;

const IDM_SHOT: usize = 40001;
const IDM_LONGSHOT: usize = 40002;
const IDM_SETTINGS: usize = 40003;
const IDM_EXIT: usize = 40004;

/// 应用运行时状态。
pub struct App {
    config: Config,
    hwnd: HWND,
    hinstance: HINSTANCE,
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

/// 注册隐藏窗口类并创建消息窗口。
fn create_message_window(hinstance: HINSTANCE) -> Result<HWND, String> {
    unsafe {
        let icon = LoadIconW(Some(hinstance), icon_resource()).unwrap_or_default();
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            hIcon: icon,
            hCursor: HCURSOR::default(),
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

impl App {
    /// 添加托盘图标（SYS-1）。
    fn init_tray(&self) {
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
    fn remove_tray(&self) {
        let nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ID_TRAY,
            ..Default::default()
        };
        unsafe {
            let _ = Shell_NotifyIconW(windows::Win32::UI::Shell::NIM_DELETE, &nid);
        }
    }

    /// 注册全局热键（SYS-2），失败时提示冲突。
    fn register_hotkeys(&self) {
        let hk = &self.config.hotkey;
        self.register_one(ID_HOTKEY_SHOT, hk.screenshot_key, "截图");
        self.register_one(ID_HOTKEY_PIN, hk.pin_key, "贴图");
    }

    fn register_one(&self, id: i32, key: u32, name: &str) {
        let mods = mod_flags(self.config.hotkey.modifiers);
        let ok = unsafe { RegisterHotKey(Some(self.hwnd), id, mods, key) }.is_ok();
        if !ok {
            let err = unsafe { GetLastError() };
            let text = if err == ERROR_HOTKEY_ALREADY_REGISTERED {
                format!("热键 {name} 注册失败：该键已被其他程序占用。\n请在设置中更换热键（设置面板后续版本提供）。")
            } else {
                format!("热键 {name} 注册失败，错误码 {}.", err.0)
            };
            unsafe { MessageBoxW(Some(self.hwnd), wide(&text), w!("jietu"), MB_OK | MB_ICONWARNING) };
        }
    }

    /// SYS-2：无修饰键的全局热键会抢占其他程序的按键，仅首次启动提示一次。
    fn maybe_warn_no_modifier_clash(&mut self) {
        if !self.config.hotkey_warned && self.config.hotkey.modifiers == 0 {
            self.config.hotkey_warned = true;
            self.config.save();
            unsafe {
                MessageBoxW(
                    Some(self.hwnd),
                    wide(
                        "截图(F1)与贴图(F3)热键未使用修饰键，会覆盖其他程序的按键。\n如影响其他软件使用，请在设置中更换热键（设置面板后续版本提供）。",
                    ),
                    w!("jietu"),
                    MB_OK | MB_ICONINFORMATION,
                )
            };
        }
    }

    /// 托盘回调（SYS-1）：左键 / 右键弹出菜单。
    fn handle_tray(&self, lparam: LPARAM) {
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
            let _ = AppendMenuW(menu, MF_STRING, IDM_SHOT, w!("截图..."));
            let _ = AppendMenuW(menu, MF_STRING, IDM_LONGSHOT, w!("长截图..."));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, IDM_SETTINGS, w!("设置..."));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT, w!("退出"));

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

    fn handle_command(&self, id: u16) {
        match id as usize {
            IDM_SHOT => self.not_yet("截图功能", "M1 里程碑"),
            IDM_LONGSHOT => self.not_yet("长截图功能", "M4 里程碑"),
            IDM_SETTINGS => self.not_yet("设置面板", "M3 里程碑"),
            IDM_EXIT => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            _ => {}
        }
    }

    /// 全局热键分发（SYS-2）。
    fn handle_hotkey(&self, id: i32) {
        match id {
            ID_HOTKEY_SHOT => self.not_yet("截图功能", "M1 里程碑"),
            ID_HOTKEY_PIN => self.not_yet("贴图功能", "M2b 里程碑"),
            _ => {}
        }
    }

    fn not_yet(&self, feature: &str, milestone: &str) {
        unsafe {
            let text = format!("{feature}尚未实现（{milestone}）。");
            let _ = MessageBoxW(Some(self.hwnd), wide(&text), w!("jietu"), MB_OK | MB_ICONINFORMATION);
        }
    }

    /// WM_SETTINGCHANGE：主题变更时刷新（SYS-6）。
    fn handle_setting_change(&self, lparam: LPARAM) {
        if theme::is_theme_change(lparam.0 as usize) {
            // M0 记录当前主题；设置面板（M3）与覆盖层（M1）消费。
            let _ = theme::system_theme();
        }
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

/// 程序主图标资源（app.rc 中的 ID=1）。
fn icon_resource() -> PCWSTR {
    PCWSTR::from_raw(1 as *const u16)
}

/// UTF-16 结尾 NUL 的 PCWSTR（临时值，仅限同一表达式内使用）。
fn wide(s: &str) -> PCWSTR {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    buf.push(0);
    PCWSTR::from_raw(buf.as_ptr())
}

/// 将字符串写入固定长度 u16 数组（用于 NOTIFYICONDATAW.szTip 等）。
fn wide_array<const N: usize>(s: &str) -> [u16; N] {
    let mut arr = [0u16; N];
    let mut i = 0;
    for u in s.encode_utf16().take(N - 1) {
        arr[i] = u;
        i += 1;
    }
    arr
}

/// 开机自启是否启用（SYS-4）：
/// Run 键值存在，且 StartupApproved 未标记禁用（尊重任务管理器的禁用状态）。
pub fn autostart_enabled() -> bool {
    unsafe {
        let mut size = 0u32;
        let err = RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        );
        if err != ERROR_SUCCESS {
            return false;
        }
        let mut data = [0u8; 12];
        let mut dsize = data.len() as u32;
        let err2 = RegGetValueW(
            HKEY_CURRENT_USER,
            STARTUP_APPROVED_KEY,
            RUN_VALUE,
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr() as *mut _),
            Some(&mut dsize),
        );
        // StartupApproved 缺失 = 启用；byte0 == 2 表示被任务管理器禁用。
        match err2 == ERROR_SUCCESS {
            true => data[0] != 2,
            false => true,
        }
    }
}

/// 设置开机自启（SYS-4）。
pub fn set_autostart(enabled: bool) {
    unsafe {
        if enabled {
            let path = exe_path_quoted();
            let mut buf: Vec<u16> = path.encode_utf16().collect();
            buf.push(0);
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(buf.as_ptr() as *const _),
                (buf.len() * 2) as u32,
            );
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}

/// 当前可执行文件路径（带引号，注册表 Run 键的标准格式）。
fn exe_path_quoted() -> String {
    unsafe {
        let mut buf = [0u16; 1024];
        let len = GetModuleFileNameW(None, &mut buf) as usize;
        let path = String::from_utf16_lossy(&buf[..len]);
        format!("\"{path}\"")
    }
}
