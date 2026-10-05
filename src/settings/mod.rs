// SPDX-License-Identifier: GPL-3.0-only
//! 配置（SYS-5）：`%APPDATA%\jietu\config.toml`，缺失或损坏时回退默认值。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 主题设置：跟随系统 / 浅色 / 深色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeSetting {
    #[default]
    #[serde(rename = "follow")]
    Follow,
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
}

/// 热键配置：虚拟键码 + 修饰键位标志（0 = 无修饰键，SYS-2 需提示风险）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    /// 截图热键虚拟键码（默认 F1 = 0x70）
    pub screenshot_key: u32,
    /// 贴图热键虚拟键码（默认 F3 = 0x72）
    pub pin_key: u32,
    /// 修饰键位标志（MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN）
    pub modifiers: u32,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            screenshot_key: 0x70,
            pin_key: 0x72,
            modifiers: 0,
        }
    }
}

/// 根配置。所有字段带默认值，旧配置文件缺字段时自动补齐。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 开机自启开关。实际生效状态以注册表 Run 键 + StartupApproved 为准。
    pub auto_start: bool,
    /// 界面语言（一期仅 zh-CN，预留 i18n）。
    pub language: String,
    pub theme: ThemeSetting,
    pub hotkey: HotkeyConfig,
    /// 是否已提示过无修饰键热键风险（避免每次启动弹窗）。
    pub hotkey_warned: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_start: false,
            language: "zh-CN".to_string(),
            theme: ThemeSetting::Follow,
            hotkey: HotkeyConfig::default(),
            hotkey_warned: false,
        }
    }
}

impl Config {
    /// 配置目录：`%APPDATA%\jietu`。
    pub fn config_dir() -> PathBuf {
        let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(appdata).join("jietu")
    }

    /// 配置文件路径。
    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// 加载配置：文件缺失或解析失败时回退默认值（SYS-5）。
    pub fn load() -> Self {
        match std::fs::read_to_string(Self::config_path()) {
            Ok(text) => toml::from_str(&text).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    /// 保存配置：自动创建目录，写入失败静默忽略。
    pub fn save(&self) {
        let text = toml::to_string_pretty(self).unwrap_or_default();
        let _ = std::fs::create_dir_all(Self::config_dir());
        let _ = std::fs::write(Self::config_path(), text);
    }
}
