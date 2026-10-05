# 2. 开源许可采用 GPL-3.0-only

- **Status**: Accepted
- **Context**: 项目确定开源，但协议未定前无法合入外部贡献、无法发布二进制。依赖生态混合了 `windows`（MIT/Apache-2.0）、`tiny-skia`（Apache-2.0/MIT）、`png`（MIT/Apache-2.0）、`libwebp`（BSD-3-Clause）等宽松许可证。未来可能引入 OCR/翻译服务商接入代码。
- **Decision**: 采用 **GPL-3.0-only**。仓库根目录放置 `LICENSE`（GPL-3.0 全文）；每个源文件头标注 `SPDX-License-Identifier: GPL-3.0-only`；发布二进制时同时提供对应源码获取方式；用 `cargo-deny` / `cargo-about` 生成第三方许可证声明并在「关于」页展示；外部贡献采用 DCO 或 CLA 明确贡献协议。
- **Consequences**:
  - 更容易：GPL-3.0 强制衍生作品开源，保护本项目不被闭源套壳；与现有宽松依赖兼容。
  - 更难：闭源商业集成不可行；**仅限 GPL-2.0-only** 的依赖不兼容（GPL-2.0+ 可并入，需逐个审查）；后续每个新依赖都要过许可证检查流程。