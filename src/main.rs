// SPDX-License-Identifier: GPL-3.0-only
//! 轻量截图工具入口。
//!
//! 当前处于 M0 骨架阶段，仅声明模块结构，尚未实现功能。
//! 里程碑规划见 `screenshot-tool-PRD.md` 第 7 节。

#![allow(dead_code)] // 骨架阶段：模块占位，待各里程碑实现后移除

pub mod app;
pub mod capture;
pub mod editor;
pub mod longshot;
pub mod ocr; // 二期
pub mod output;
pub mod overlay;
pub mod pin;
pub mod render;
pub mod settings;
pub mod theme;

fn main() {
    // 骨架阶段暂无逻辑；M0 将在此初始化托盘、热键与消息循环。
}
