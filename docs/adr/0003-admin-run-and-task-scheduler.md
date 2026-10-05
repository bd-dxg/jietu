# 3. 始终以管理员权限运行，开机自启改用任务计划程序

- **Status**: Accepted
- **Context**: 用户反馈存在两类问题：其他软件占用 F1 热键；以及需要覆盖/操作以管理员权限运行的窗口。Windows 的 UIPI 限制使普通权限进程无法覆盖管理员窗口、无法通过 `SendInput` 向管理员窗口投递输入（后者直接影响 M4 长截图的自动滚动）。PRD SYS-4 原本规划用 `HKCU\...\Run` 注册表键实现开机自启，但该方式无法启动声明了提权的程序。
- **Decision**:
  1. `app.manifest` 声明 `requestedExecutionLevel level="requireAdministrator"`，程序始终以管理员权限运行。
  2. 开机自启从 `HKCU\...\Run` 键改为**任务计划程序**：`schtasks /Create /SC ONLOGON /RL HIGHEST`，删除对应任务即关闭自启。
  3. 热键继续使用 `RegisterHotKey`（不引入低级键盘钩子）。若被其他软件抢占，注册失败时提示用户改键。
  4. 构建区分：**release** 构建（`app.manifest`）声明 `requireAdministrator`；**dev** 构建（`app.dev.manifest`）声明 `asInvoker` 不提权，便于 `cargo run` / `cargo test` 调试。两者仅权限不同，功能代码一致。
- **Consequences**:
  - 更容易：覆盖层可覆盖管理员窗口；`SendInput` 对管理员窗口有效（长截图前提）；热键注册在与同级进程竞争中更有利。
  - 更难：每次启动弹出 UAC 提示（仅 release）；提权进程无法接收普通进程的拖放（未来若做「拖入图片」功能需注意）；自启依赖任务计划而非注册表；查询/写入任务计划需要管理员权限。
  - 注意：dev 构建不提权，因此 **dev 下无法覆盖/操作管理员窗口**，验证该能力需用 release 构建。
  - 注意：热键优先级仍由注册顺序决定，管理员权限不改变这一点；只有低级键盘钩子（WH_KEYBOARD_LL）会插队，本决策不引入钩子。
