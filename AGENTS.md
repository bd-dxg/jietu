# jietu 开发指南

## 项目概览

轻量原生截图与标注工具（Windows 11，Rust + windows-rs，纯 CPU 渲染），面向无独显迷你主机。目标：常驻 ≈0 开销、触发即用、框选/箭头/文本/高斯模糊/高亮标注、长截图、贴图。

完整需求见 `screenshot-tool-PRD.md`，域术语见 `CONTEXT.md`，架构决策见 `docs/adr/`。

## 项目状态

- 当前处于 **M2b/M3 已完成** 阶段：M0/M1/M2/M3 均已交付；M2b 贴图完成：分层置顶贴图窗口（`WS_EX_LAYERED` + `UpdateLayeredWindow` 提交 premultiplied 像素）、左键拖动、滚轮缩放（缩小区域平均/放大双线性，滚动中最近邻预览 + 静止精修）、Ctrl+滚轮透明度、双击/Esc 关闭、右键菜单（复制/保存/关闭/关闭全部）、数量上限 20；F3 双入口：截图编辑内贴选区（含标注，跨线程 GlobalAlloc 载荷交给主线程建窗，截图期间临时撤销全局贴图热键防双重触发）、非截图时贴剪贴板图（PNG 注册格式 → CF_DIB → CF_BITMAP 三级回退，含 24/32bpp 与 alpha 探测）；贴图可配置显示位置（截图原位置 / 屏幕中央，原位置为默认；剪贴板贴图恒居中）与 1px 蓝色边框开关（默认开，边框烘焙进窗口像素；贴图窗口内快捷键切边框，非全局，M2b）。**下一步：M4 长截图**。- 已知问题记录：覆盖层线程曾因 `WM_DESTROY` 同步派发导致 `GetMessageW` 永久阻塞，每次截图泄漏一份全屏位图集合（每张 +57MB），已在 `wndproc.rs` 的 `WM_DESTROY` 分支加 `PostQuitMessage(0)` 修复；修复后截图退出常驻内存稳定在 ~24MB（debug，代码页 + 堆缓存）。
- 开发由 AI 驱动，本文件是进入此仓库的 agent 的上下文。

## 技术栈

- 语言：Rust 1.98（edition 2024）
- 系统 API：`windows` crate（windows-rs 0.62.2）
- 渲染：`tiny-skia` 0.12（纯 CPU 矢量绘制）+ DirectWrite 文本栅格化
- 配置/编码：`serde` + `toml`、`png`

## 常用命令

- 编译检查：`cargo check`
- 运行：`cargo run`（GUI 桌面程序，需 Windows 桌面会话；当前骨架阶段无可见界面）
- 构建发布版：`cargo build --release`（已配 `lto="fat"`、`opt-level="z"`、`panic="abort"`、`strip=true`）
- 测试：`cargo test`（当前 30+ 个：对象模型/撤销栈/矢量栅格化/光标/布局）
- 格式化检查：`cargo fmt --check`（提交前必须通过）

## 开发流程（AI 驱动约定）

1. **禁止在主分支（main）直接开发**：任何功能、修复、优化都必须新建分支（命名建议 `feat/*`、`fix/*`、`docs/*`），完成后以 Pull Request 提交、经审查后合并回 main。main 只接受 PR 合并，不允许本地直接 push。
2. **先验证，后提交**：完成实现后先自行编译/测试，然后把改动交给用户手动验证（运行、体验、确认交互）。**未得到用户明确确认前，禁止执行 `git commit`、`git push`、创建 PR 或合并**。只有用户说“验证通过 / 可以提交”后才走提交与 PR 流程。
3. 按里程碑推进（PRD §7：M0 → M5，二期另行），每次只实现当前里程碑需求，不做投机性代码。
4. 实现功能前先同步文档：新术语写入 `CONTEXT.md`；不可逆/高杠杆决策记入 `docs/adr/NNNN-*.md`；需求变更落在 PRD 对应编号。
5. 每个源文件保留头部 `SPDX-License-Identifier: GPL-3.0-only`。
6. 引入新依赖前确认许可证与 GPL-3.0-only 兼容，并说明用途。
7. `windows` crate 按需开启 features，不一次性全量开启。

## 代码约定

- 目录结构固定为 PRD §6.2 模块划分：`app/`、`capture/`、`overlay/`、`pin/`、`editor/`、`render/`、`longshot/`、`output/`、`settings/`、`theme/`、`ocr/`（二期）。
- **单文件源码 ≤300 行**（硬性；`#[cfg(test)]` 测试块不计入）。超出时按职责拆出子模块（如 `overlay/` 下的 `wndproc/geometry/surface/cursor/selection/interaction/annotate/toolbar/`），子模块内 `pub(super)` 共享父模块实现。
- 模块 doc 注释中标注对应需求编号（SYS-1、CAP-2、EDT-1…），便于追溯。
- 标注采用矢量对象模型（底图不变，对象列表叠加），渲染用脏矩形局部重绘。
- 所有坐标统一为物理像素（Per-Monitor V2 manifest）。
- 一期界面仅简体中文，代码层预留 i18n。

## 性能红线（必须守住）

- 待机 CPU 0%（无轮询、无定时器，长截图期间除外）
- 待机内存（Private Working Set）≤ 15 MB
- 热键到覆盖层显示 ≤ 150 ms（1080p）/ ≤ 250 ms（4K）
- 交互 ≥ 60 FPS，不依赖 GPU 合成

## 注意事项

- **程序始终以管理员权限运行**（manifest `requireAdministrator`，见 ADR 0003）：不要改回普通权限，否则无法覆盖/操作管理员窗口；开机自启必须用任务计划（`schtasks`）而非 HKCU Run 键。
- 无 GPU 可用：模糊、缩放、重采样等重操作需盒式近似 + 局部缓存 + 低质量预览，禁止全帧逐像素重算。
- 全局热键 F1/F3 会抢占其他程序按键，设置面板必须给出提示；注册失败要检测并提示冲突。
- 长截图（M4）是最大风险点：优先自动滚动 + 手动兜底，拼接要处理固定区域（页眉/页脚），失败提供撤销最后一段入口。
- 贴图窗口用 `WS_EX_TOPMOST | WS_EX_TOOLWINDOW` 分层窗口，窗口静止时禁止任何重绘。
- 二期 OCR/翻译涉及第三方服务：默认关闭、首次显式授权、密钥用 DPAPI 加密、不留存请求内容。
- 发布前用 `cargo-deny` / `cargo-about` 检查依赖许可证，生成第三方声明。