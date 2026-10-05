// SPDX-License-Identifier: GPL-3.0-only
//! 入口：初始化应用壳并进入消息循环。

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
    std::process::exit(app::run());
}
