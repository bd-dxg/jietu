// SPDX-License-Identifier: GPL-3.0-only
//! 开机自启（SYS-4）：任务计划 ONLOGON + 最高权限（见 ADR 0003）。
//! 程序始终以管理员运行，HKCU Run 键无法启动需提权的程序，故改用 schtasks。

use windows::Win32::System::LibraryLoader::GetModuleFileNameW;

/// 任务计划名称。
const TASK_NAME: &str = "jietu";

/// 开机自启是否启用（SYS-4）：任务计划中存在 "jietu" 任务。
pub fn autostart_enabled() -> bool {
    run_schtasks(&["/Query", "/TN", TASK_NAME])
}

/// 设置开机自启（SYS-4）：创建/删除登录时以最高权限运行的任务计划。
pub fn set_autostart(enabled: bool) {
    if enabled {
        let path = exe_path_quoted();
        run_schtasks(&[
            "/Create", "/F", "/TN", TASK_NAME, "/TR", &path, "/SC", "ONLOGON", "/RL", "HIGHEST",
        ]);
    } else {
        run_schtasks(&["/Delete", "/F", "/TN", TASK_NAME]);
    }
}

/// 调用 schtasks.exe 并返回是否成功（隐藏控制台窗口）。
fn run_schtasks(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("schtasks")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 当前可执行文件路径（带引号）。
fn exe_path_quoted() -> String {
    unsafe {
        let mut buf = [0u16; 1024];
        let len = GetModuleFileNameW(None, &mut buf) as usize;
        let path = String::from_utf16_lossy(&buf[..len]);
        format!("\"{path}\"")
    }
}
