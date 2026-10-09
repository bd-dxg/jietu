// SPDX-License-Identifier: GPL-3.0-only
//! 配置（SYS-5）：`%APPDATA%\jietu\config.toml`，缺失或损坏时回退默认值。

pub mod panel;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::editor::Tool;

/// 最近一次截图会话使用的工具（下次截图默认沿用，首次为矩形，M3）。
static LAST_TOOL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// 上次使用的标注工具。
pub fn last_tool() -> Tool {
    let i = LAST_TOOL.load(std::sync::atomic::Ordering::Relaxed) as usize;
    Tool::ALL[i.min(Tool::ALL.len() - 1)]
}

/// 记录本次会话最后使用的工具。
pub fn last_tool_store(t: Tool) {
    LAST_TOOL.store(t as u8, std::sync::atomic::Ordering::Relaxed);
}

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

/// 热键配置：每键独立虚拟键码 + 修饰键位标志（0 = 无修饰键，SYS-2 需提示风险）。
/// 截图与贴图各自持有修饰键，互不影响（M3 修复历史共享缺陷）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    /// 截图热键虚拟键码（默认 F1 = 0x70）
    pub screenshot_key: u32,
    /// 截图热键修饰键位标志（MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN）
    pub screenshot_modifiers: u32,
    /// 贴图热键虚拟键码（默认 F3 = 0x72）
    pub pin_key: u32,
    /// 贴图热键修饰键位标志
    pub pin_modifiers: u32,
    /// 贴图边框开关热键（默认 Ctrl+F3；0 = 未启用）
    pub border_key: u32,
    /// 贴图边框开关热键修饰键位标志
    pub border_modifiers: u32,
    /// 遗留字段（旧版本两键共享修饰键，TOML 键名仍为 `modifiers`）：仅加载迁移用，不再写入。
    #[serde(default, skip_serializing, rename = "modifiers")]
    pub legacy_modifiers: u32,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            screenshot_key: 0x70,
            screenshot_modifiers: 0,
            pin_key: 0x72,
            pin_modifiers: 0,
            border_key: 0x72,    // Ctrl+F3
            border_modifiers: 2, // MOD_CONTROL
            legacy_modifiers: 0,
        }
    }
}

/// 工具切换键（M3）：覆盖层内按数字键切换标注工具，每工具一个虚拟键码。
type ToolKey = u32;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolKeys {
    pub rect: ToolKey,
    pub arrow: ToolKey,
    pub text: ToolKey,
    pub blur: ToolKey,
    pub highlight: ToolKey,
    pub glow: ToolKey,
}

impl Default for ToolKeys {
    fn default() -> Self {
        Self {
            rect: 0x31,      // 1
            arrow: 0x32,     // 2
            text: 0x33,      // 3
            blur: 0x34,      // 4
            highlight: 0x35, // 5
            glow: 0x36,      // 6
        }
    }
}

/// 贴图初始位置（M2b）：编辑器内贴选区（PIN-1）时显示在截图原位置或屏幕中央；
/// 剪贴板贴图（PIN-2）无「原位置」，始终居中。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PinPosition {
    /// 在截图选区原位置显示（默认）。
    #[default]
    #[serde(rename = "original")]
    Original,
    /// 在虚拟屏中央显示。
    #[serde(rename = "center")]
    Center,
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
    /// 覆盖层工具切换键（M3）。
    pub tool_keys: ToolKeys,
    /// 是否已提示过无修饰键热键风险（避免每次启动弹窗）。
    pub hotkey_warned: bool,
    /// 贴图初始位置（M2b）。
    pub pin_position: PinPosition,
    /// 贴图 1px 蓝色边框（M2b，默认开）。
    pub pin_border: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_start: false,
            language: "zh-CN".to_string(),
            theme: ThemeSetting::Follow,
            hotkey: HotkeyConfig::default(),
            tool_keys: ToolKeys::default(),
            hotkey_warned: false,
            pin_position: PinPosition::Original,
            pin_border: true,
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
        let mut cfg: Config = match std::fs::read_to_string(Self::config_path()) {
            Ok(text) => toml::from_str(&text).unwrap_or_default(),
            Err(_) => Config::default(),
        };
        // 迁移：旧版共享 modifiers（同时作用于两键）→ 拆分到各键
        let h = &mut cfg.hotkey;
        if h.legacy_modifiers != 0 && h.screenshot_modifiers == 0 && h.pin_modifiers == 0 {
            h.screenshot_modifiers = h.legacy_modifiers;
            h.pin_modifiers = h.legacy_modifiers;
        }
        cfg
    }

    /// 保存配置：自动创建目录，写入失败静默忽略。
    pub fn save(&self) {
        let text = toml::to_string_pretty(self).unwrap_or_default();
        let _ = std::fs::create_dir_all(Self::config_dir());
        let _ = std::fs::write(Self::config_path(), text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧配置（共享 modifiers）能正确迁移到各键独立修饰键。
    #[test]
    fn legacy_shared_modifiers_migrate() {
        let old = "[hotkey]\nscreenshot_key = 0x70\npin_key = 0x72\nmodifiers = 1\n";
        let cfg: Config = toml::from_str(old).unwrap();
        let (mut h, _) = (cfg.hotkey.clone(), ());
        assert_eq!(h.legacy_modifiers, 1, "旧 modifiers 应读入遗留字段");
        assert_eq!(h.screenshot_modifiers, 0);
        // 模拟 load() 的迁移逻辑
        if h.legacy_modifiers != 0 && h.screenshot_modifiers == 0 && h.pin_modifiers == 0 {
            h.screenshot_modifiers = h.legacy_modifiers;
            h.pin_modifiers = h.legacy_modifiers;
        }
        assert_eq!(h.screenshot_modifiers, 1);
        assert_eq!(h.pin_modifiers, 1);
    }

    /// 新格式各键独立修饰键，互不干扰。
    #[test]
    fn per_key_modifiers_roundtrip() {
        let cfg = Config {
            hotkey: HotkeyConfig {
                screenshot_key: 0x41,
                screenshot_modifiers: 1, // Alt
                pin_key: 0x72,
                pin_modifiers: 4, // Shift
                ..Default::default()
            },
            ..Default::default()
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.hotkey.screenshot_modifiers, 1);
        assert_eq!(back.hotkey.pin_modifiers, 4);
        assert!(!text.contains("legacy_modifiers"), "遗留字段不应写入配置：{text}");
    }
}
