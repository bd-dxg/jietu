# 1. 纯 CPU 渲染管线（tiny-skia + DirectWrite）

- **Status**: Accepted
- **Context**: 目标设备为无独立显卡的迷你主机（仅 CPU 核显、共享显存很小）。Electron / WebView / GPU 加速 UI 框架常驻占用高、触发时有卡顿。产品目标要求待机内存 ≤ 15 MB、热键到覆盖层 ≤ 150 ms、交互 ≥ 60 FPS，均不依赖 GPU 合成。
- **Decision**: 全部绘制走软件渲染——覆盖层、编辑对象、贴图、设置面板统一用 `tiny-skia` 绘制到 CPU 位图；文本经 DirectWrite（`IDWriteBitmapRenderTarget`）栅格化到 CPU 位图以获得中文字形回退；上屏通过 `StretchDIBits` / `BitBlt` 刷新窗口（注意 RGBA↔BGRA 与预乘 alpha）。不采用任何 GPU 合成路径。
- **Consequences**:
  - 更容易：内存占用低且可预测、依赖轻、坐标统一为物理像素（Per-Monitor V2），行为跨机器一致。
  - 更难：4K 大区域高斯模糊等重操作变慢，需用三次盒式模糊近似、仅处理局部并缓存；放弃 GPU 的缩放/混合加速；重绘需依赖脏矩形局部刷新保证 60 FPS。
  - 后续若引入 GPU 管线，本决策需整体重审（ADR 级别变更）。