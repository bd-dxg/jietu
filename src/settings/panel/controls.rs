// SPDX-License-Identifier: GPL-3.0-only
//! 设置面板控件模型与布局（M3）：左侧分类导航 + 右侧内容区。

use crate::overlay::geometry::SelRect;

/// 窗口内容总宽。
pub const W: i32 = 520;
/// 左侧导航栏宽。
pub const NAV_W: i32 = 124;
/// 左右边距。
pub const PAD: i32 = 24;
/// 导航项高度。
pub const NAV_H: i32 = 44;
/// 右侧内容区顶部节标题高度。
pub const CONTENT_HEADER_H: i32 = 40;
/// 内容行高度。
pub const ROW_H: i32 = 46;

/// 分类页。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    General,
    Hotkeys,
    Appearance,
    About,
}

impl Section {
    pub const ALL: [Section; 4] = [Section::General, Section::Hotkeys, Section::Appearance, Section::About];

    pub fn title(self) -> &'static str {
        match self {
            Section::General => "通用",
            Section::Hotkeys => "快捷键",
            Section::Appearance => "外观",
            Section::About => "关于",
        }
    }
}

/// 控件 ID。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowId {
    /// 开机自启开关。
    Autostart,
    /// 语言展示（一期仅 zh-CN）。
    Lang,
    /// 截图热键。
    HotkeyShot,
    /// 贴图热键。
    HotkeyPin,
    /// 工具切换键（M3）：矩形 / 箭头 / 文本 / 模糊 / 荧光笔 / 高亮。
    ToolRect,
    ToolArrow,
    ToolText,
    ToolBlur,
    ToolHighlight,
    ToolGlow,
    /// 无修饰键风险提示（只读）。
    Widenote,
    /// 主题：跟随系统。
    ThemeFollow,
    /// 主题：浅色。
    ThemeLight,
    /// 主题：深色。
    ThemeDark,
    /// 关于（版本与许可）。
    About,
}

/// 行控件类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowKind {
    /// 开关（整行可点）。
    Toggle,
    /// 热键捕获按钮（点右侧区域进入捕获）。
    KeyCap,
    /// 单选（整行可点）。
    Radio,
    /// 只读：左侧标签 + 右侧说明文本。
    Readonly,
    /// 关于块：左侧产品名、右侧版本与许可。
    About,
}

/// 布局项：导航项或内容行。
#[derive(Debug)]
pub enum Item {
    Nav(Section),
    Row(Row),
}

/// 单行控件几何与标识。
#[derive(Debug)]
pub struct Row {
    pub id: RowId,
    pub kind: RowKind,
    pub label: &'static str,
    /// 整行矩形（命中 + 悬停高亮）。
    pub rect: SelRect,
    /// 右侧控件区（开关/按键框/说明文本）。
    pub control: SelRect,
}

/// 面板内容布局：导航项 + 当前分类的行 + 总高度。
#[derive(Debug)]
pub struct Layout {
    pub items: Vec<Item>,
    pub height: i32,
}

/// 内容行几何（`s` 为 DPI 缩放系数，x 从导航栏右缘开始）。
fn mk_row(id: RowId, kind: RowKind, label: &'static str, y: i32, s: f32) -> Row {
    let h = (ROW_H as f32 * s) as i32;
    let cw = (150.0 * s) as i32;
    Row {
        id,
        kind,
        label,
        rect: SelRect {
            x: NAV_W,
            y,
            w: W - NAV_W - PAD,
            h,
        },
        control: SelRect {
            x: W - PAD - cw,
            y,
            w: cw,
            h,
        },
    }
}

/// 指定分类的内容行。
fn section_rows(section: Section, y0: i32, s: f32, items: &mut Vec<Item>) {
    let mut y = y0;
    let push = |id: RowId, kind: RowKind, label: &'static str, y: &mut i32, items: &mut Vec<Item>| {
        items.push(Item::Row(mk_row(id, kind, label, *y, s)));
        *y += (ROW_H as f32 * s) as i32;
    };
    match section {
        Section::General => {
            push(RowId::Autostart, RowKind::Toggle, "开机自启", &mut y, items);
            push(RowId::Lang, RowKind::Readonly, "语言", &mut y, items);
        }
        Section::Hotkeys => {
            push(RowId::HotkeyShot, RowKind::KeyCap, "截图", &mut y, items);
            push(RowId::HotkeyPin, RowKind::KeyCap, "贴图", &mut y, items);
            push(RowId::ToolRect, RowKind::KeyCap, "工具·矩形", &mut y, items);
            push(RowId::ToolArrow, RowKind::KeyCap, "工具·箭头", &mut y, items);
            push(RowId::ToolText, RowKind::KeyCap, "工具·文本", &mut y, items);
            push(RowId::ToolBlur, RowKind::KeyCap, "工具·模糊", &mut y, items);
            push(RowId::ToolHighlight, RowKind::KeyCap, "工具·荧光笔", &mut y, items);
            push(RowId::ToolGlow, RowKind::KeyCap, "工具·聚光灯", &mut y, items);
            push(RowId::Widenote, RowKind::Readonly, "提示", &mut y, items);
        }
        Section::Appearance => {
            push(RowId::ThemeFollow, RowKind::Radio, "跟随系统", &mut y, items);
            push(RowId::ThemeLight, RowKind::Radio, "浅色", &mut y, items);
            push(RowId::ThemeDark, RowKind::Radio, "深色", &mut y, items);
        }
        Section::About => {
            push(RowId::About, RowKind::About, "jietu 截图工具", &mut y, items);
        }
    }
}

/// 构建布局：导航项 + 当前节内容；高度取两者的最大值（窗口刚好包住内容）。
pub fn layout(section: Section, scale: f32) -> Layout {
    let s = scale;
    let mut items = Vec::new();
    for sec in Section::ALL {
        items.push(Item::Nav(sec));
    }
    // 内容区：节标题 + 各节最大行数（每节各自累计，取最高者）
    let mut max_content = 0i32;
    for sec in Section::ALL {
        let mut rows = Vec::new();
        section_rows(sec, 0, s, &mut rows);
        let h = (CONTENT_HEADER_H as f32 * s) as i32 + rows.len() as i32 * (ROW_H as f32 * s) as i32;
        max_content = max_content.max(h);
    }
    // 当前节的内容行（实际绘制用）
    section_rows(section, (CONTENT_HEADER_H as f32 * s) as i32, s, &mut items);

    let nav_h = (NAV_H as f32 * s) as i32 * Section::ALL.len() as i32;
    Layout {
        items,
        height: nav_h.max(max_content),
    }
}

/// 虚拟键 → 显示名（如 F1、Ctrl+C）。
/// 修饰位与 Windows `MOD_*` 一致：ALT=0x1、CONTROL=0x2、SHIFT=0x4、WIN=0x8。
pub fn key_name(vk: u32, mods: u32) -> String {
    let mut s = String::new();
    if mods & MOD_CTRL_BIT != 0 {
        s += "Ctrl+";
    }
    if mods & MOD_ALT_BIT != 0 {
        s += "Alt+";
    }
    if mods & MOD_SHIFT_BIT != 0 {
        s += "Shift+";
    }
    if mods & MOD_WIN_BIT != 0 {
        s += "Win+";
    }
    s += &vk_name(vk);
    s
}

/// 虚拟键 → 键名。
pub fn vk_name(vk: u32) -> String {
    match vk {
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0x1B => "Esc".into(),
        0x20 => "Space".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x2E => "Del".into(),
        0x27 => "→".into(),
        0x25 => "←".into(),
        0x26 => "↑".into(),
        0x28 => "↓".into(),
        // 常规字符键：MapVirtualKeyW 取键名（组合键 Shift+ 时字符可能大写/符号）
        _ => {
            use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_CHAR, MapVirtualKeyW};
            unsafe {
                let ch = MapVirtualKeyW(vk, MAPVK_VK_TO_CHAR) & 0xFFFF;
                if ch != 0 && (ch as u8).is_ascii_graphic() {
                    (ch as u8 as char).to_uppercase().to_string()
                } else {
                    format!("键 {vk:#x}")
                }
            }
        }
    }
}

/// 修饰键位掩码（与 `HotkeyConfig::modifiers` 的 MOD_* 位一致）。
pub const MOD_ALT_BIT: u32 = 1 << 0;
pub const MOD_CTRL_BIT: u32 = 1 << 1;
pub const MOD_SHIFT_BIT: u32 = 1 << 2;
pub const MOD_WIN_BIT: u32 = 1 << 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_has_nav_and_rows() {
        let l = layout(Section::General, 1.0);
        let nav: Vec<_> = l
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Nav(s) => Some(*s),
                _ => None,
            })
            .collect();
        assert_eq!(nav, Section::ALL);
        let rows: Vec<_> = l
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Row(r) => Some(r.id),
                _ => None,
            })
            .collect();
        assert!(rows.contains(&RowId::Autostart));
        assert!(rows.contains(&RowId::Lang));
        assert!(l.height > 0);
    }

    #[test]
    fn each_section_has_its_rows() {
        for sec in Section::ALL {
            let l = layout(sec, 1.0);
            let rows: Vec<_> = l
                .items
                .iter()
                .filter_map(|i| match i {
                    Item::Row(r) => Some(r.id),
                    _ => None,
                })
                .collect();
            let expected = match sec {
                Section::General => vec![RowId::Autostart, RowId::Lang],
                Section::Hotkeys => vec![
                    RowId::HotkeyShot,
                    RowId::HotkeyPin,
                    RowId::ToolRect,
                    RowId::ToolArrow,
                    RowId::ToolText,
                    RowId::ToolBlur,
                    RowId::ToolHighlight,
                    RowId::ToolGlow,
                    RowId::Widenote,
                ],
                Section::Appearance => vec![RowId::ThemeFollow, RowId::ThemeLight, RowId::ThemeDark],
                Section::About => vec![RowId::About],
            };
            assert_eq!(rows, expected, "节 {sec:?} 行集合错误");
        }
    }

    #[test]
    fn rows_do_not_overlap_nav() {
        let l = layout(Section::Hotkeys, 1.0);
        for item in &l.items {
            if let Item::Row(r) = item {
                assert!(r.rect.x >= NAV_W, "内容行不得进入导航区：{r:?}");
            }
        }
    }

    #[test]
    fn key_names_are_sensible() {
        assert_eq!(vk_name(0x70), "F1");
        assert_eq!(key_name(0x43, MOD_CTRL_BIT), "Ctrl+C");
        assert_eq!(vk_name(0x1B), "Esc");
    }
}
