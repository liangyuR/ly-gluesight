# 测试记录与复现

更新时间：2026-10-12。当前测试范围按本轮明确选择收缩，测试删除不改变产品业务规则或运行时校验。前端与后端的分组见[前端轻量测试](frontend-lightweight-tests.md)、[后端轻量测试](backend-lightweight-tests.md)。

## 当前保留范围

| 测试层 | 保留验证 |
| --- | --- |
| 前端 | 创建配方、草稿编辑、采集、示教验证发布、生产操作锁、周期/图源隔离与历史显示，39 项 |
| Rust 默认 | 配方版本、采集与示教流程、相机/帧关联、生产周期、PLC、调度、SQLite、原图录制、审计恢复及历史，192 项 |
| S7 专项 | 35 项真实生产驱动与本机 Python peer 的线协议测试 |
| 桌面主流程 | 隔离 flow 实例内启动一件到 SQLite、原图和历史读回 |
| 保留工具 | 打包契约、Windows 发布门禁、安装包自检、S7 Python 服务与 Robot/Modbus 隔离协议 |

本轮默认回归已验证前端 40 项与 Rust 192 项，S7 专项 35 项通过。另有 1 个由父测试调用的耐久性子进程夹具，默认忽略，不单独执行。

本轮删除真实 DLL 的 8 项专项、性能与队列压力专项、性能工具自身测试、Robot 五工况桌面联调、现场验收清单、冻结发布资源的 9 项跨平台独立测试（Windows 默认 8 项）和质量判定规则的 11 项独立测试。旧 flyshot 脚本、去哈希专项、重复的 P0 性能/联调准备入口、覆盖率报告及重复前端快捷入口同步清理。保留的主流程可能经过发布和判定，但不据此声明已覆盖被删除组的全部规则。

## 运行

使用 Node 24.15.0、pnpm 12.11.2、Rust 1.99.0。首次安装执行 `pnpm install --frozen-lockfile`。

| 命令 | 用途 |
| --- | --- |
| `pnpm test` | 全部前端测试，包含创建配方与完整示教发布流程 |
| `pnpm test:watch` | 开发时持续运行相关前端测试 |
| `pnpm test:ui:production` | 自动构建、启动与关闭隔离桌面实例，验证一件生产落盘 |
| `pnpm test:e2e:offline` | 全新隔离站点上用离线·样本集与真实 lyFlow 走完建站、示教、验证、发布、投产与复测 |
| `pnpm test:e2e:log-replay` | 全新隔离站点上投产 3 件样本件，核对本件时间线、运行日志、原包重跑逐拍照点一致、原图缺失拒绝与诊断包 |
| `pnpm typecheck` | 应用及测试 TypeScript 类型检查 |
| `pnpm build` | 生产前端构建 |
| `cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib` | 默认后端回归，依赖需已缓存 |
| `pwsh -NoProfile -File tests/packaging/Test-RuntimeBundle.ps1` | 打包版本、ABI、源码修订及运行库依赖契约 |
| `pwsh -NoProfile -File scripts/Test-DevStartup.ps1` | Windows 旧终端在设置/清除本地运行库后启动，子进程读取最新保存值，参数与退出码保持不变 |

`Test-DevStartup.ps1` 临时修改用户级 `GLUESIGHT_LYFLOW_RUNTIME`，在 `finally` 中恢复原值；验证期间不要并行修改该配置或启动 GlueSight。测试通过临时 CLI 进程观察启动环境，不启动实际 GUI，不修改配方。

单文件前端测试示例：`pnpm exec vitest run tests/teaching.test.tsx`。默认测试不生成覆盖率报告。三个桌面端到端入口的参数、前置条件与已知限制见下文“端到端入口”。

Windows 受限环境遇到 Node realpath EPERM 时，按仓库 AGENTS.md 的 Windows 指引在同一进程加载兼容脚本。需要获准的主机权限时运行相同命令，不修改应用逻辑掩盖环境失败。Cargo 首次构建需要 MSVC、Visual Studio C++ 工具及私有 Git 依赖读取权限；离线执行要求依赖已缓存。

## S7 一期握手

协议与点表见 [S7 一期握手说明](../integration/plc-s7-phase1.md)。[S7 测试 PLC](../../scripts/s7-handshake/README.md) 仅监听本机回环地址，使用真实 TPKT/COTP/S7 报文。S7 协议版本 4 按交互表 V1.0（S7-200 SMART、DB1、DInt 序号、DBB160/DBB100 的 50 字符 SN）调整，旧协议 2、3 点表和 PLC 程序被拒绝；下列专项与历史记录中的通过数据按旧协议取得，协议 4 的运行结果见交互表适配 PR。

```powershell
python -m unittest discover -s scripts/s7-handshake/tests -v
cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib plc_session -- --include-ignored --test-threads=1 --nocapture
```

S7 专项需要 Python 3 和本机 TCP 权限，可用 `S7_TEST_PYTHON` 指定 Python。Windows 沙箱拒绝 localhost 连接（WinError 10013）时，在获准主机环境运行，不能跳过 TCP 后记为通过。日志位于 `src-tauri/target/s7-session-tests/verification.log`；各案例目录保存状态、报文、事务及 DB 快照，本机产物不提交。

## 特征定位真引擎测试

`src-tauri/src/vision/taught/engine_tests.rs` 在合成工件（定位框里两个暗孔和一个亮 L 角，暗胶条沿示教线）上跑真实 lyFlow 的 glue.locate 与 bead_width，经 `measure_views` 和 `judge` 走生产路径，共 3 项：整件平移（+12, +7 px 与切向 −25 px）后 d′ 不变、两端超出检测范围、判 OK；只移胶 8 px 时 d 沿法向变 0.8 mm、判 NG_POSITION；定位框被抹平时有胶站全部无基准、胶宽保留、判 ERR 92。默认忽略，需要带 lyFlow#32 glue.locate 的 `lyflow_core.dll`（发布前用本地构建）：

```powershell
$env:LYFLOW_CORE_DLL = '<lyFlow 构建>\build\core\bin\lyflow_core.dll'
cargo test --manifest-path src-tauri/Cargo.toml --lib vision::taught::engine_tests -- --ignored --test-threads=1
```

核心库的 glue.locate 不支持带外圈模板（1.0.0）时测试直接失败，不算通过。

## 隔离桌面与证据边界

`pnpm test:ui:production` 使用 `tests/native/production-flow-ui.mjs` 与 `tests/native/tauri-flow-test.json`，应用标识为 `com.xyzrobotics.tujiaovision.flow-tests`，报告输出到 `output/playwright/flow-regression`。测试通过站点模式进入“离线 · 合成图”，经生产 S7 握手运行一件；使用真实后端、SQLite、PGM 和元数据，并在历史页读回；结束时退出离线并关闭本次进程。

其余保留的桌面测试入口及前置条件见 [原生测试目录](../../tests/native/)。恢复专项应使用独立实例和新输出目录，避免使用生产配置。Robot/Modbus 模拟服务、模拟节拍与回放目录相机已删除，由站点模式的离线产线取代。

**P0 原生脚本（站点模式下的处理）**

旧 P0 界面链路以 `p0-setup-ui.js` 在离线期间新建三视角模拟相机、`p0-create-ui.js` 在“拍照点规划”页手工编辑拍照点为起点，后续脚本都依赖它产生的 `P0-TRICAM-UI` 配方。站点模式下离线相机只由离线来源生成、离线期间相机页只读，拍照点改由整圈采集生成，“取新样本”“拍照点规划”和视觉设置也已删除，这条链路无法成立。处理如下：

| 脚本 | 处理 | 理由 |
| --- | --- | --- |
| `p0-setup-ui.js`、`p0-create-ui.js`、`p0-camera-refresh-ui.js` | 删除 | 依赖离线期间增改模拟相机、手工编辑拍照点和已删除的视觉设置；建站、采集、建候选由离线端到端覆盖 |
| `p0-teach-ui.js`、`p0-invalidation-ui.js`、`p0-camera-invalidation-ui.js` | 删除 | 依赖“取新样本”和“拍照点规划”改 Pose / 相机；示教由离线端到端与前端示教测试覆盖，视角切换和改参后失效由 `workspace/workflow_tests.rs` 覆盖，相机与 Pose 现由采集轮次决定 |
| `p0-validate-publish-ui.js`、`p0-publish-reviewed-ui.js` | 删除 | 依赖本机不提交的 PGM 证据目录；后者断言“期望 OK、实际 OK_WITH_EXCURSION 判为不符合”，与合格类都算符合的决策冲突；验证发布由离线端到端覆盖 |
| `p0-online-ui.js`、`p0-history-produce-ui.js`、`p0-frame-visibility-ui.js` | 删除 | 依赖已删除的 `settings.vision` 字段和三视角配方；合成图生产落盘由 `test:ui:production` 覆盖，样本集投产、录制与原图读回由离线端到端覆盖；布局可见性属于当时的界面修复验收证据 |
| `p0-history-ui.js` | 改写进离线端到端 | 历史导出 CSV、按原发布包重现、按候选规则重判、从原图复测整件、已保存对照切换与“原始记录和候选不被改动”，现从离线端到端的投产终态在历史详情界面执行 |
| `p0-history-independent-ui.js` | 删除 | 依赖“拍照点规划”里切换其他候选、未保存的 Pose；原发布包重现只用冻结发布包的规则由离线端到端的重现断言（发布包、配方修订一致）覆盖 |
| `p0-restart-connect-ui.js`、`p0-startup-ui.js` | 改写为 `offline-restart.mjs` | 重启后已发布配方按原发布包预热、可再次投产且结论不变，现从离线端到端终态重启同一隔离实例验证 |
| `p0-plc-bootstrap-ui.js` | 保留 | 独立于链路：界面进入离线·合成图，核对 S7 回环地址与 28 个 DB1 点（协议 4）；2026-10-12 在 p0 实例上实跑通过 |
| `p0-single-instance-acceptance.mjs` | 保留 | 前置为 p0 实例已有原图且节拍空闲（先跑 `p0-plc-bootstrap-ui.js` 再离线运行一件 MTR-HSG-A；运行库目录可经 `GLUESIGHT_NATIVE_PATH` 前置到 PATH）；2026-10-12 实跑通过，重复启动以独占锁拒绝（退出码 101），数据库与原图不变 |
| `p0-ack-process-recovery.mjs`、`p0-audit-process-recovery.mjs`（及审计专用配置 `tauri-audit-test.json`） | 删除 | 需要外部 S7 Python 夹具（真机模式）配合模拟相机；真机模式加载时会把非海康相机改回海康，模拟相机只在离线时由站点模式生成并强制使用进程内 S7，站点模式下无法运行。另依赖已删除链路建立的 `P0-TRICAM-UI` / 701 配方，审计脚本还断言已不存在的“模拟 / 回放相机”拒绝原因。ACK 日志落盘后进程终止的重启恢复、审计 spool 在进程终止后的回放由 `plc_session`（`s7_wire_restart_after_persisted_ack_recovers_exact_cycle_before_and_after_reset` 等）、`store`（`durable_ack_recovery_matches_cycle_after_reopen_and_preserves_original_evidence`）、`audit`（`durable_spool_recovers_after_real_process_termination_before_any_sqlite_insert` 等）的 Rust 测试覆盖 |

jsdom 测试证明页面动作、关键接口参数及失败恢复，不证明真实布局、像素解码或硬件通信。桌面模拟流程证明业务接线与落盘，不证明真实 PLC、SDK、引擎测量准确率、现场节拍或持续运行。当前 UI 范围见 [UI 覆盖边界](UI_OPERATIONS.md)。

曝光与增益随配方保存（每个配方每台相机一组），相机配置不再含这两项。写入海康相机的时机有三处：采集页点“应用”、整圈采集开始前、生产布防的 `camera-imaging` 阶段（PLC 换型同样，写不进去拒绝布防）。合成图与样本集相机下发为空操作，自动测试只覆盖配方校验、候选保存与界面调用；实际写入 ExposureTime / Gain 须在台架用海康相机确认。

## 端到端入口

三个入口都会构建（`-SkipBuild` 跳过）、启动、驱动并关闭各自的隔离桌面实例，结束时打印总耗时；失败以非零退出码结束，输入缺失为退出码 3（不算通过）。决策见 [r2-test-infra 决策记录](../decisions/r2-test-infra.md)。

| 命令 | 驱动脚本 | 实例配置（应用标识） | 默认端口 | 输出 |
| --- | --- | --- | --- | --- |
| `pnpm test:e2e:offline` | `tests/native/offline-e2e.mjs` + `offline-restart.mjs` | e2e（`…e2e-tests`），每次清空资料 | 9341 | `output/playwright/e2e-regression` |
| `pnpm test:e2e:log-replay` | `tests/native/log-replay-offline.mjs` | e2e（`…e2e-tests`），每次清空资料 | 9346 | `output/playwright/log-replay`（实例日志在 e2e-regression） |
| `pnpm test:ui:production` | `tests/native/production-flow-ui.mjs` | flow（`…flow-tests`），保留资料 | 9339 | `output/playwright/flow-regression` |

参数跟在命令后，例如 `pnpm test:e2e:offline -DebugPort 9351 -SkipBuild`。

| 参数 / 环境变量 | 作用 |
| --- | --- |
| `-DebugPort` | WebView2 远程调试端口，`127.0.0.1` 与 `[::1]` 上都必须空闲，否则启动前报错 |
| `-SkipBuild` | 复用上次构建复制到输出目录的 `gluesight-ui-tests.exe`（需同一配置构建过） |
| `-DataRoot` / `GLUESIGHT_TEST_DATA_ROOT` | 隔离资料目录的父目录，资料目录为 `<DataRoot>\<应用标识>`；默认在输出目录下 |
| `-SampleSet` / `GLUESIGHT_SAMPLE_SET` | 样本集目录（`test:e2e:offline` 与 `test:e2e:log-replay` 必需） |
| `-LyFlowRuntime` / `GLUESIGHT_LYFLOW_RUNTIME` | lyFlow 运行库目录；未提供时按 `packaging/runtime.json` 恢复 |
| `-Parts` | 仅 `test:e2e:log-replay`：投产件数，至少 3 |
| `-ArmBudgetMs` | 仅 `test:e2e:offline`：把布防预算 `timeouts.armMs` 临时改为该值（毫秒），写进 `result.json` 的 `inputs.armBudgetMs`；不传时按默认 200 ms 验证 |
| `CARGO_TARGET_DIR` | 共享或自定义 Cargo 构建目录时设置，脚本从这里取构建产物 |

**前置条件**：Windows 与 WebView2 Runtime；Node 24.15+，已 `pnpm install --frozen-lockfile`；Rust 工具链与私有 Git 依赖读取权限（构建时）；离线样本集与 lyFlow 运行库（两个 e2e 入口）。实测的 2026-10-17 单圈样本集原件只读，脚本先复制到输出目录再写模拟当量。

**隔离实例的启动**（`scripts/Start-UiTestInstance.ps1`，三个入口共用，也可单独启动后手动驱动）：

- 子进程只继承 NUL 输入与 `stdout.log`、`stderr.log` 两个文件句柄，不继承调用方的管道；后台或被捕获输出时，脚本在实例就绪后即可返回，实例继续运行。
- 就绪标准：在 `127.0.0.1` 与 `[::1]` 两个回环地址上轮询 `/json/list`，出现且只出现主窗口页面、资料准备窗口已关闭，最长 120 s。主窗口所在地址写入 `instance.json` 的 `cdp`，各驱动脚本默认从清单读取（`--cdp` 可覆盖）。程序提前退出或超时会附带 `stderr.log` 末尾 20 行。
- `instance.json` 字段：`pid`、`executable`、`identifier`、`dataRoot`、`debugPort`、`cdp`、`stdout`、`stderr`、`startedAt`。关闭实例时脚本按 PID、可执行文件路径与启动时间三项核对身份，不一致不结束进程。

**已知限制**：

- 资料目录长度：程序启动时校验主窗口的 WebView2 资料目录 `<资料目录>\webview\<SID>` 不超过 229 个字符；资料目录尚未初始化（没有 `storage-layout.json`、需要先建资料准备窗口）时，还校验准备窗口的 `<资料目录>\migration\bootstrap\webview\<SID>` 不超过 237 个字符。超限直接报错退出（隐藏窗口的测试实例写 `stderr.log` 并以退出码 1 结束）。SID 一般 46 个字符时，全新资料目录应不超过 162 个字符，已初始化的不超过 174 个字符。主检出默认约 107 个字符；`.claude/worktrees/<名称>/` 工作树约 143 个字符，仍可用，但更深的路径需要用 `-DataRoot`（例如 `C:\gs-tmp\<名称>`）。
- 全新资料目录启动时先有资料准备窗口，它与主窗口是两个 WebView2 浏览器进程：先启动的占 `127.0.0.1:<端口>`，主窗口回落到 `[::1]:<端口>`；已初始化目录重启时主窗口在 `127.0.0.1`。不要自己拼 `localhost:<端口>` 连接，使用清单里的 `cdp`。
- 两个 e2e 入口共用 e2e 配置、可执行文件与资料目录，不能同时运行，后跑的会清空前一个的资料。不同检出 / 工作线并行时各用不同端口与 `-DataRoot`。
- 机器满载时（实测其他进程占满 CPU），调试构建的布防会超过默认 200 ms 预算，工件判 ERR 98、没有原图，`test:e2e:offline` 失败并在报错里给出本件结论。离线端到端默认按 200 ms 验证；机器与其他任务共用、负载高时可加 `-ArmBudgetMs 1000` 再跑（满载时实测 `audit-health` 阶段可达 1 s，需要 3000），结果里会记下放宽值（`test:e2e:log-replay` 固定放宽到 1 s）。满载时 S7 轮询会停顿，虚拟 PLC 心跳超过 3 s 未观察到翻转即进入故障；若紧接着的整圈采集由离线产线发出的 faultReset 早于新的心跳翻转，复位被拒（“尚未观察到 PLC 心跳翻转，不能复位”）且不会再发，采集一直等到超时。这类失败只能等负载下降后重跑，产品侧问题已转交 r2-plc。
- 端到端都是隐藏窗口，截图来自 WebView2 页面；结论只覆盖业务接线、落盘与界面驱动，边界见各节。

## 离线端到端

`pnpm test:e2e:offline` 由 `scripts/Test-OfflineE2E.ps1` 驱动 `tests/native/offline-e2e.mjs`，配置为 `tests/native/tauri-e2e-test.json`，应用标识 `com.xyzrobotics.tujiaovision.e2e-tests`（隐藏窗口），报告与截图输出到 `output/playwright/e2e-regression`（`result.json`、`restart.json`、`offline-e2e-history.png`）。第一段通过后，脚本关闭实例、保留数据目录重启同一实例，再由 `tests/native/offline-restart.mjs` 做重启续测。已构建时加 `-SkipBuild` 直接跑；端口冲突传 `-DebugPort`。共享 Cargo 缓存时设置 `CARGO_TARGET_DIR`，脚本从该目录取构建产物。

| 输入 | 环境变量 | 脚本参数（覆盖环境变量） | 默认值 |
| --- | --- | --- | --- |
| 样本集目录 | `GLUESIGHT_SAMPLE_SET` | `Test-OfflineE2E.ps1 -SampleSet`，`offline-e2e.mjs --sample-set` | 无，必须提供 |
| lyFlow 运行库目录 | `GLUESIGHT_LYFLOW_RUNTIME` | `Test-OfflineE2E.ps1 -LyFlowRuntime`，`offline-e2e.mjs --lyflow-runtime` | 自动恢复 `packaging/runtime.json` 系列最新 Release |

样本集目录必须通过环境变量或参数提供，脚本不内置任何本机路径；lyFlow 运行库未提供时由 `Restore-LyFlowRuntime.ps1` 恢复，并通过 `GLUESIGHT_LYFLOW_RUNTIME` 传给 debug 构建。任一输入缺失或不存在时不启动实例，打印缺失项并以退出码 3 结束，不算通过。样本集目录应放在本地非同步目录（不能是 junction、符号链接或 OneDrive 等 reparse point）。样本集原件只读：每次运行把它复制到输出目录的 `sample-set/`（复制前解析真实路径，副本目录与原件相同或互相包含时直接报错，不删除任何内容），按样本集 id 选取脚本顶部 `FIXTURES` 里的夹具，没有夹具的样本集直接报错。当前夹具为 `gs-20261017-normal`（2026-10-17 现场单圈，12 点位 × 3 相机）：副本 id 加 `-e2e` 后缀，全局 `mmPerPx` 写入模拟标定当量 0.112（样本集未标定，沿用 [P0 计划](../architecture/p0-plan.md) 记录的同分辨率现场相机假设当量，不代表真实标定）；样本集只有正常件，副本里由 p01 复制出 p02，把 S011 组所选图（样本 S032）的一段约 11 mm 胶路逐行抹成左右紧邻像素的线性渐变，作为人工断胶件（`label=ng`；早先换成旁边板面像素的做法在 lyFlow 1.1.4 下会被测成胶而漏判，见 [r2-lyflow-upgrade.md](../decisions/r2-lyflow-upgrade.md) L-2）。离线与重启续测都用这份副本；重启续测按 `site_status().last.sampleSetDir` 进入，即首次进入时导入的受管副本。每次运行前只删除资料根目录（默认为输出目录，见 `-DataRoot`）下以隔离标识命名的数据目录（`GLUESIGHT_DATA_DIR` 指向它；名称、父目录和非链接逐项校验），保证从全新站点开始，覆盖首次建站无已发布配方的场景。实例启动、CDP 地址与资料目录长度的约定见上文“端到端入口”。

**覆盖范围**：真机模式启动 → lyFlow 引擎就绪 → 进入离线·样本集（离线控制台显示“当前：离线 · 样本集”，未发布前提示尚无可运行的生产配方；离线相机组按样本集 3 台相机生成）→ 新建候选、离线产线以全部 3 台相机送一圈 p01（12 次触发 × 3 相机 = 36 个拍照点）并采用 → 每个拍照点组（Pose）按夹具选一台相机的图作为检测图，核对候选采用了样本集声明当量，用刷选笔划（S006 组用框选）提取中线、保存参数（4 个组用 3 mm 窄搜索）、试测为合格类、保存示教 → 进度模型的标定步骤完成，所有检测图的标定依据为样本集声明当量，全程不调用人工标定验证 → 独立采集 p01（期望合格）与 p02（期望 NG_GAP）作验证样本 → 验证通过，p01 的 OK 或 OK_WITH_EXCURSION 都按合格类判为符合 → 发布生效（发布状态 active，`validationSite=offline`），离线控制台默认选中新配方 → 离线产线投产 p01、p02 → p01 为合格类，p02 为 NG_GAP 且原因指向 `S011-cam2` → 历史记录带样本件引用 → 离线控制台“最近样本件”期望/实际与历史列表样本徽标、结论的界面核对 → 历史页导出 CSV（行数与记录数一致）→ 在 p02 的历史详情界面依次按原发布包重现（发布包与配方修订为本件冻结值）、按候选规则重判、从原图复测整件，三者结论均与原结论一致，已保存对照可切换查看，原始记录与候选不变 → 界面退出离线，PLC 配置与相机组恢复 → 重启同一实例：仍为真机模式，历史与生产配方修订、示教身份不变，该配方在真机就绪检查中不可运行（原因为离线环境验证发布、需在真机重新验证并发布，引导到工作台），进入离线后原发布包预热就绪且该配方变为可运行，再投产 p02 仍按原发布包得到同一结论。业务步骤主要经 IPC 驱动；PLC 事务占用、发布资源预热、离线 S7 服务重连（界面在重连期间禁用“运行一件”）等暂时拒绝按界面重试的方式最多重试 60 s，次数记在 `result.json` 的 `gateRetries`。

**结果字段**：`branches.calibration` 固定为 `sampleSetDeclared`；`branches.normalExpectation` 记录合格样本的实际结论是 `ok` 还是 `okClass`（OK_WITH_EXCURSION），两者都判为符合；`replayPart` 为做历史复测与重启续测的样本件（有断胶件时为它）；`replay.*.via` 均为 `ui`，三种复测都经历史详情界面触发（“使用该配方候选”需在确认框中切换）。

**边界**：每组所选相机、笔划、像素当量、窄搜索参数与人工断胶件都是让流程跑通的演示夹具，集中定义在脚本顶部；通过只说明建站到投产的业务接线、多相机拍照点组示教、样本件追溯与原图复测一致，不代表中线提取或检测算法质量、现场标定精度、真实缺陷检出、现场 PLC/SDK 或节拍。mx11-demo 在每组必须选一张检测图的规则下需示教全部 8 个点，其中 S003、S006、S007 尚无可靠笔划夹具，暂不提供其夹具。

## 站点模式（真机 / 离线）

软件启动总是真机模式：测量用 lyFlow，相机只接海康 MVS，PLC 只支持 S7。“总览”首页显示当前模式“真机”“离线 · 样本集 <名称>”或“离线 · 合成图”，点“查看站点模式”或侧栏“工艺 → 离线模式”进入离线控制台（`/offline`），控制台标题为“当前：真机”或“当前：离线 · <来源>”。

进入离线时软件临时派生并应用：PLC 切到进程内 S7 服务（127.0.0.1 随机端口、DB1 一期点表、PDU 240）；相机组按样本集 `cameras[]` 生成样本集相机，或按真实相机组同编号、同视角数生成模拟相机；测量用 lyFlow（样本集）或模拟测量（合成图）；帧录制临时为“全部”。真实 PLC、相机组和系统设置不改写，退出离线后全部恢复；恢复真机 PLC 配置失败时 PLC 保持断开并停在安全配置，离线控制台与日志标为“故障·需人工修复”。进入离线失败回滚时按进入前状态重连真机 PLC。离线期间相机页、PLC 页只读；有在途事务或整圈采集进行中时不能切换模式。离线模式不持久化，重启回到真机，但会记住上次来源和样本集目录。

**离线 · 合成图跑一件**

1. 断开外部 PLC，打开离线控制台，来源选“合成图”，点“进入离线”。
2. 选离线配方，工况选“正常件”，点“运行一件”；在线检测页也有离线配方、运行一件和停止的紧凑控件。
3. 在线检测与历史记录（切到“离线件”）应出现同一 SN 的整件结果和原图。可再试其他工况（如传输丢帧、拍照数不符）或故障注入，确认得到预期 ERR。

**离线 · 样本集对照期望/实际**

格式契约见 [离线样本集格式 v1](../integration/sample-set-format.md)（只接受 schemaVersion 1）。样本集按 partId / shotId / cameraId / view 取图，缺图按缺帧处理；标签只用于展示期望，不参与判定。示例：`<样本集根目录>\mx11-demo`（单相机 cam1、单视角，p02 断胶 S004–S007）。

1. 确认 lyFlow 引擎就绪（运行库位于程序目录 `runtime/lyflow/`，开发时由 `pnpm deps:lyflow:dev` 复制），否则拒绝进入并提示原因。
2. 离线控制台来源选“样本集”，点“选择样本集目录”，核对件数、拍照点数、缺图和标签分布，点“进入离线”。
3. 无已发布配方时：整圈采集页点“开始接收”→“离线产线送一圈”，选样本件后采用本轮图像建立配方，示教、验证并发布。
4. 选离线配方和样本件（不选则按顺序循环），点“运行一件”或“连续运行”，用“本件后停止”/“立即停止”结束。样本集模式只提供握手类工况：正常件、传输丢帧、设备触发数与计划不符、运动超时、ACK 序号不匹配。
5. 核对“最近样本件”表的期望 / 实际：mx11-demo 的 p02 应为 NG（S004–S007 断胶），其余件的实际结论应与样本集标签一致。

**生产记录隔离**

配方共享，生产记录按来源隔离：检测记录带来源（真机 / 离线·合成图 / 离线·样本集，库版本 6，旧库按 D-0 备份后重建；离线件 SN 形如 `OFFLINE-0001234567`，同一离线运行内唯一）。历史页默认只看真机件，“离线件”切换查看离线件，列表与详情标注离线来源；统计、批量重判与 CSV 导出（含“来源”列）只算当前范围，在线检测的今日统计跟随当前站点模式。离线原图写入数据目录下独立的 `records-offline`，按系统设置中的离线原图保留件数和容量上限（默认 300 件 / 10 GB）单独清理，保存后在下一次录制收尾时按新上限清理，任一超限均删除最旧原图，真机 `records` 只按系统设置清理，离线连续运行不会挤掉真机原图（`recorder` 的 `changed_offline_retention_prunes_next_recording_without_removing_real_raw_images`）。样本件认领只在离线期间生效，退出离线清空本次运行记录，同 partSn 的真机件不带样本引用。

**退出与历史复测**

1. 点“退出离线”，确认离线控制台标题回到“当前：真机”，PLC 配置、相机组和帧录制设置均为进入前的值。
2. 历史记录切到“离线件”，打开离线跑过的件，详情显示“离线样本 partId · SN · 期望 OK/NG”；因离线期间录制为全部，可直接原图复测或与候选配方对照。

## 端到端验证（2026-10-19，r2-test-infra）

本机 Windows 11、WebView2 Runtime 155.0.4283.62、lyFlow 1.1.0（`src-tauri/target/dependency-runtime/runtime/lyflow`）、2026-10-17 单圈样本集，调试端口 9351，工作树资料目录约 143 字符。每轮依次跑三个入口，耗时为脚本打印的总耗时（含构建时另注）。

| 轮次 | 代码 | 机器负载 | `test:ui:production` | `test:e2e:offline` | `test:e2e:log-replay` |
| --- | --- | --- | --- | --- | --- |
| 1 | main `a626cb8` + 本线 | 正常 | 通过，43 s（含增量构建 27 s） | 通过，85.9 s（首段 63.9 s、重启续测 8.4 s） | 通过，76.4 s |
| 2 | 同上 | 正常 | 通过，10.4 s | 通过，85.4 s（65.5 s、8.4 s） | 通过，99.5 s |
| 3 | main `804c14d` / `d4726c1` + 本线（含长稳、磁盘下限等合并；两者只差文档） | 满载：约 30 个外部占满 CPU 的进程 | 通过，167.8 s（含全量重编约 158 s） | 失败：p02 布防 246 ms 超 200 ms 预算，判 ERR、无原图 | 失败：整圈采集时虚拟 PLC 心跳超过 3 s 未翻转 |
| 4 | 同上 | 满载：其他工作线并行编译与性能测试 | 通过，23.1 s | 失败：p01 布防 `audit-health` 213 ms 超预算 | 通过，231 s |
| 5 | 同上（`-ArmBudgetMs 1000`） | 50%–90% | — | 通过，261.4 s（170.7 s、10.8 s） | 失败：离线 S7 轮询过期 2.7 s 后重连，`offline_run` 报“尚未连接”；之后驱动脚本对此按界面方式重试 |
| 6 | 同上（`-ArmBudgetMs 1000`） | 60%–95% | — | 失败：空闲时心跳故障，紧接的 p02 整圈采集发出的 faultReset 被拒（“尚未观察到 PLC 心跳翻转”），采集等到超时 | 通过，96.2 s |
| 7 | 同上（`-ArmBudgetMs 1000`） | 70%–80% | — | 通过，107.2 s（82.6 s、9.9 s） | 通过，87.8 s |
| 8 | 同上（默认 200 ms） | 约 75% | — | 失败：首段通过（116.3 s，布防 166/179 ms）；重启续测再投产 p02 布防 332 ms 超预算 | — |
| 9 | 同上（`-ArmBudgetMs 1000`） | 约 100%：其他工作线并行跑前端测试与性能测试 | 通过，18.9 s | 失败：p02 布防 `audit-health` 1014 ms 超 1 s | 通过，249.2 s |
| 10 | 同上（`-ArmBudgetMs 3000`） | 约 100% | — | 通过，324 s（179.2 s、13.4 s） | — |
| 11 | 同上（`-ArmBudgetMs 3000`） | 约 100% | — | 通过，163.7 s（125.8 s、10.2 s） | — |

负载正常的第 1、2 轮三个入口全部通过。合并长稳等改动后的代码上：生产落盘第 3、4、9 轮全部通过；回放验证第 6、7、9 轮连续通过；离线端到端在满载下默认 200 ms 预算不稳定，显式放宽后第 10、11 轮连续通过。p01 为 OK_WITH_EXCURSION、p02 为 NG_GAP（`S011-cam2` 断胶 11.0 mm），三种历史复测经界面得到同一结论，重启后原发布包预热并再投产 p02 结论不变；回放验证 3 件原包重跑逐拍照点最大偏移差约 3×10⁻⁸ mm。失败的轮次都发生在整机满载时，原因见“已知限制”。

另做的验证：`$out = pwsh -File scripts/Start-UiTestInstance.ps1 …` 捕获输出时，修复前要等实例退出才返回（实测 94.5 s 后手动结束实例才返回），修复后实例就绪即返回（满载下 24.8 s），实例继续运行；`-DataRoot C:\gs-tmp\r2ti\data -ResetData` 全新启动时主窗口在 `[::1]`，清单 `cdp` 随之记录；隐藏窗口的测试实例在数据目录锁被占用时 0.5 s 内以退出码 1 结束并在 stderr 给出独占锁原因；资料目录长度按窗口分别校验后（flow 配置、SID 46 字符）：全新资料目录准备窗口资料目录 238 字符时以退出码 1 结束，stderr 给出“资料准备窗口”、238 与上限 237；237 字符时正常启动到主窗口；把这份已初始化的资料目录移到主窗口资料目录 229 字符（准备窗口资料目录 249 字符）处重启仍正常启动，移到 230 字符处以退出码 1 结束，stderr 给出“主窗口”、230 与上限 229；`cargo test --lib` 294 项通过（满载时另有 3 次运行出现与本线无关的偶发失败，见 PR）；`pnpm exec tsc -b` 与 `vitest run`（35 文件、207 项）通过。

## 本轮验证（2026-10-12，离线 S7 产线合并 P0 清理后）

| 检查 | 结果 |
| --- | --- |
| Rust 默认 | 集成分支 201 passed、0 failed、36 ignored（含进程内 S7 服务、虚拟产线 + 生产 S7 会话集成测试） |
| S7 本机线协议 | `cargo test -- --ignored` 36 passed、0 failed |
| 前端 | 集成分支 17 文件、54 passed |
| 类型 | `pnpm typecheck` 通过 |
| 离线端到端 | 集成分支 mx11-demo 副本（声明逐点当量）+ lyFlow 1.1.0 全新站点通过：首段 29.7 s、重启续测 5.7 s，含增量构建总计 122 s；标定依据样本集声明当量、合格样本按合格类符合；p01 OK_WITH_EXCURSION（码 2），p02 NG_GAP（码 13，S004）；原包重现、规则重判、原图复测均经界面得到 NG_GAP；重启后原发布包预热并再投产 p02 结论不变 |
| P0 保留脚本 | `p0-plc-bootstrap-ui.js` 与 `p0-single-instance-acceptance.mjs` 在 p0 实例实跑通过；两份进程恢复专项已删除（见上表） |
| 生产桌面落盘 | 界面启用离线产线，正常件 OK，6 帧、460 测量点、6 张 PGM 和元数据落盘，SQLite 与历史原图读回通过；测试进程已关闭 |
| 原生脚本抽查 | `guard.js` + `plc-ui.js`（ui 实例）、`p0-plc-bootstrap-ui.js`（p0 实例）在离线 S7 下通过；旧 P0 界面链路脚本已按上表删除或改写 |
| S7 Python | 23 项通过 |
| 打包工具、文档产物 | 本轮未重跑 |

本机日志与桌面报告 `output/playwright/flow-regression/result.json` 不提交。保留现有编译警告，未修改业务代码来消除警告。

## 历史验证记录

2026-10-11 配方创建软件交付曾通过 Rust 417 项、S7 35 项、前端 990 项，以及隔离桌面接口与重启恢复；见[当时交付记录](../architecture/recipe-creation-progress.md)。之后前后端分别精简，旧通过数字不能作为本轮结果。

[第一轮精简记录](unit-test-simplification.md)、[P0 历史计划](../architecture/p0-plan.md)与 `docs/testing/evidence/` 下已有 JSON 仅保存历史过程和证据。历史 DLL、压力、覆盖率及现场条目不再构成当前测试要求；已删除脚本可从 Git 历史查看，不再提供执行入口。
