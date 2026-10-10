// SPDX-License-Identifier: GPL-3.0-only
//! 滚轮模拟（LNG-2/10）：让目标窗口真正获得前台焦点后发“真实”滚轮事件。
//!
//! 实测（`jietu-longshot.log`）发现两个决定性问题：
//! 1. `PostMessage(WM_MOUSEWHEEL)` 对多数控件无效（忽略合成消息）；
//! 2. `SendInput` 滚轮只有在目标**是前台窗口**时才生效 —— 执行窗口 `WS_EX_NOACTIVATE`
//!    期间 `GetForegroundWindow()` 常为 NULL，滚轮被直接丢弃。
//!
//! 修复：把**顶层**目标窗口（`GetAncestor(GA_ROOT)`）真正激活 —— 先把自己线程
//! `AttachThreadInput` 到目标线程（突破 Windows 前台锁定），再 `SetForegroundWindow`
//! + `SetFocus`。之后 `SendInput` 滚轮稳定生效。
//! 执行窗口用全局热键接收按键，不依赖焦点，目标获得前台不影响 Enter/Esc。

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, SetFocus,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, SetCursorPos,
    SetForegroundWindow, WindowFromPoint,
};

/// 每次滚动连发的滚轮事件数（一个事件约 3 行；发多个可一次滚更多，加快拼接）。
const NOTCHES: u32 = 3;

/// 取窗口类名。
fn class_name(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// 视口中心下的**顶层**目标窗口：跳过本进程的边框/状态条（悬浮在屏幕上）。
fn target_root(cx: i32, cy: i32) -> HWND {
    unsafe {
        let mut h = WindowFromPoint(POINT { x: cx, y: cy });
        let mut guard = 0;
        while h != HWND::default() {
            let root = GetAncestor(h, GA_ROOT);
            if root == HWND::default() {
                break;
            }
            if !class_name(root).starts_with("jietu.longshot") {
                return root;
            }
            guard += 1;
            if guard > 8 {
                break;
            }
            // 本进程窗口：把光标点挪一格再找（简单绕开）。
            h = WindowFromPoint(POINT {
                x: cx + guard * 3,
                y: cy,
            });
        }
        h
    }
}

/// 把 `target` 真正激活为前台（附到目标线程突破前台锁定）。
unsafe fn activate_window(target: HWND) {
    unsafe {
        if target == HWND::default() {
            return;
        }
        let cur = GetCurrentThreadId();
        let target_thread = GetWindowThreadProcessId(target, None);
        if target_thread != 0 && target_thread != cur {
            let _ = AttachThreadInput(cur, target_thread, true);
            let _ = SetForegroundWindow(target);
            let _ = SetFocus(Some(target));
            let _ = AttachThreadInput(cur, target_thread, false);
        } else {
            let _ = SetForegroundWindow(target);
            let _ = SetFocus(Some(target));
        }
    }
}

/// 首次滚动前激活目标：置前台 + 在视口中心点一下，把焦点落到具体控件。
pub fn activate(x: i32, y: i32, w: u32, h: u32) {
    let (cx, cy) = center(x, y, w, h);
    unsafe {
        let target = target_root(cx, cy);
        if target == HWND::default() {
            return;
        }
        activate_window(target);
        let _ = SetCursorPos(cx, cy);
        send_mouse(MOUSEEVENTF_LEFTDOWN, 0);
        std::thread::sleep(std::time::Duration::from_millis(10));
        send_mouse(MOUSEEVENTF_LEFTUP, 0);
        std::thread::sleep(std::time::Duration::from_millis(15));
        super::debug::log(&format!(
            "activate: target={target:?} class={} fg={:?}",
            class_name(target),
            GetForegroundWindow()
        ));
    }
}

/// 发一次滚轮（负 delta = 向下滚 = 页面内容上移）。
/// 连发 `NOTCHES` 个事件：单个滚轮事件只能滚 3 行的应用也能一次滚多——提升拼接速度。
pub fn wheel(x: i32, y: i32, w: u32, h: u32, delta: i32) {
    let (cx, cy) = center(x, y, w, h);
    unsafe {
        let target = target_root(cx, cy);
        activate_window(target);
        let _ = SetCursorPos(cx, cy);
        std::thread::sleep(std::time::Duration::from_millis(12));
        for _ in 0..NOTCHES {
            send_mouse(MOUSEEVENTF_WHEEL, delta);
            std::thread::sleep(std::time::Duration::from_millis(12));
        }
        super::debug::log(&format!(
            "wheel: fg={:?} target={target:?} delta={delta} x{NOTCHES}",
            GetForegroundWindow()
        ));
    }
}

fn center(x: i32, y: i32, w: u32, h: u32) -> (i32, i32) {
    (x + (w as i32) / 2, y + (h as i32) / 2)
}

/// 发送一个鼠标事件（真实输入）。
unsafe fn send_mouse(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS, data: i32) {
    unsafe {
        let mi = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: data as u32,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };
        let mut input = INPUT {
            r#type: INPUT_MOUSE,
            ..Default::default()
        };
        input.Anonymous.mi = mi;
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}
