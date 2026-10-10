// SPDX-License-Identifier: GPL-3.0-only
//! 长截图（LNG-1~13）：框选滚动视口 → 自动/手动滚动 → 条带拼接 → 输出。
//!
//! 模块分工：`stitch` 条带匹配纯函数、`rows` 行缓冲画布、`engine` 拼接状态机、
//! `scroll` 滚轮模拟、`session` 会话状态机、`window` 执行窗口。

mod bar;
mod border;
mod debug;
mod engine;
mod rows;
mod scroll;
mod session;
mod stitch;
mod window;

use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK};

/// 长截图主流程：创建执行窗口阻塞运行，完成后复制到剪贴板并保存 PNG。
pub fn run(
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    hinstance: HINSTANCE,
    max_height: u32,
    palette: &'static crate::theme::Palette,
) {
    let Some(pixmap) = window::run(x, y, w, h, hinstance, max_height, palette) else {
        return; // 取消或失败（窗口内已提示）
    };
    // 输出：复制 + 保存（与普通截图 Enter / Ctrl+S 行为一致，OUT-1/OUT-2）
    std::thread::spawn(move || {
        if let Err(e) = crate::output::copy_to_clipboard(&pixmap) {
            crate::app::message_box(None, &format!("复制失败：{e}"), MB_OK | MB_ICONERROR);
        }
        let dir = crate::output::default_save_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(crate::output::timestamped_filename("png"));
        if let Err(e) = crate::output::save_png(&pixmap, &path) {
            crate::app::message_box(None, &format!("保存失败：{e}"), MB_OK | MB_ICONERROR);
        }
    });
}
