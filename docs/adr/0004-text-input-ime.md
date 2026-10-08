# 4. 文本标注输入：直接处理 IME 消息而非隐藏 Edit 控件

- **Status**: Accepted
- **Context**: EDT-4 文本标注需要支持中文 IME 输入。覆盖层是无边框置顶窗口（`WS_POPUP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`），不激活、不参与焦点链，且标注是在覆盖层像素图上自绘渲染（无系统控件）。两条可选路线：
  1. 创建隐藏的 `EDIT` 子控件承载输入与 IME：子控件依赖焦点链与激活状态，与 `WS_EX_NOACTIVATE` 冲突风险高；控件位置/字体同步、回车与 Esc 拦截都要桥接，调试面大。
  2. 窗口过程直接处理 IME 消息：`WM_CHAR`（普通字符）+ `WM_IME_STARTCOMPOSITION` / `WM_IME_COMPOSITION` / `WM_IME_ENDCOMPOSITION`（组合串与结果串，`ImmGetCompositionStringW` 取数据）。输入状态、光标、组合串预览全部在覆盖层像素上自绘，与现有渲染体系一致。
- **Decision**: 采用路线 2。窗口过程集中处理键盘与 IME 消息；新建独立 `overlay::textinput` 子模块承载「输入中」状态机（文本缓冲、光标、IME 组合串），绘制走既有脏矩形重绘路径；文本最终提交为编辑器对象（`Kind::Text`）进入撤销栈。
- **Consequences**:
  - 更容易：无子控件与激活/焦点调试；组合串预览和光标完全可控可自绘；烘培导出复用同一对象模型。
  - 更难：补充平面字符（emoji 等）需自行处理 UTF-16 代理对；组合串取回依赖对 `IME_COMPOSITION_STRING` 标志位的正确判断；`WS_EX_NOACTIVATE` 下 IME 若异常（先创建后激活的窗口收不到键盘）需回退到临时移除该扩展样式或改用路线 1。