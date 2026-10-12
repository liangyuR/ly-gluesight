# 测试记录与复现

更新时间：2026-10-12。当前测试范围按本轮明确选择收缩，测试删除不改变产品业务规则或运行时校验。前端与后端的分组见[前端轻量测试](frontend-lightweight-tests.md)、[后端轻量测试](backend-lightweight-tests.md)。

## 当前保留范围

| 测试层 | 保留验证 |
| --- | --- |
| 前端 | 创建配方、草稿编辑、采集、示教验证发布、生产操作锁、周期/图源隔离与历史显示，39 项 |
| Rust 默认 | 配方版本、采集与示教流程、相机/帧关联、生产周期、PLC、调度、SQLite、原图录制、审计恢复及历史，192 项 |
| S7 专项 | 35 项真实生产驱动与本机 Python peer 的线协议测试 |
| 桌面主流程 | 隔离 flow 实例内启动一件到 SQLite、原图和历史读回 |
| 保留工具 | 打包契约、Windows 发布门禁、安装包自检、S7 Python 服务、Robot/Modbus 隔离协议及桌面进程恢复 |

本轮默认回归已验证前端 40 项与 Rust 192 项，S7 专项 35 项通过。另有 1 个由父测试调用的耐久性子进程夹具，默认忽略，不单独执行。

本轮删除真实 DLL 的 8 项专项、性能与队列压力专项、性能工具自身测试、Robot 五工况桌面联调、现场验收清单、冻结发布资源的 9 项跨平台独立测试（Windows 默认 8 项）和质量判定规则的 11 项独立测试。旧 flyshot 脚本、去哈希专项、重复的 P0 性能/联调准备入口、覆盖率报告及重复前端快捷入口同步清理。保留的主流程可能经过发布和判定，但不据此声明已覆盖被删除组的全部规则。

## 运行

使用 Node 24.15.0、pnpm 12.11.2、Rust 1.99.0。首次安装执行 `pnpm install --frozen-lockfile`。

| 命令 | 用途 |
| --- | --- |
| `pnpm test` | 全部前端测试，包含创建配方与完整示教发布流程 |
| `pnpm test:watch` | 开发时持续运行相关前端测试 |
| `pnpm test:ui:production` | 自动构建、启动与关闭隔离桌面实例，验证一件生产落盘 |
| `pnpm typecheck` | 应用及测试 TypeScript 类型检查 |
| `pnpm build` | 生产前端构建 |
| `cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib` | 默认后端回归，依赖需已缓存 |
| `pwsh -NoProfile -File tests/packaging/Test-RuntimeBundle.ps1` | 打包版本、ABI、源码修订及运行库依赖契约 |

单文件前端测试示例：`pnpm exec vitest run tests/teaching.test.tsx`。默认测试不生成覆盖率报告。生产桌面测试已构建时可加 `-SkipBuild`，端口冲突可传 `-DebugPort 9349`。

Windows 受限环境遇到 Node realpath EPERM 时，按仓库 AGENTS.md 的 Windows 指引在同一进程加载兼容脚本。需要获准的主机权限时运行相同命令，不修改应用逻辑掩盖环境失败。Cargo 首次构建需要 MSVC、Visual Studio C++ 工具及私有 Git 依赖读取权限；离线执行要求依赖已缓存。

## S7 一期握手

协议与点表见 [S7 一期握手说明](../integration/plc-s7-phase1.md)。[S7 测试 PLC](../../scripts/s7-handshake/README.md) 仅监听本机回环地址，使用真实 TPKT/COTP/S7 报文。

```powershell
python -m unittest discover -s scripts/s7-handshake/tests -v
cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib plc_session -- --include-ignored --test-threads=1 --nocapture
```

S7 专项需要 Python 3 和本机 TCP 权限，可用 `S7_TEST_PYTHON` 指定 Python。Windows 沙箱拒绝 localhost 连接（WinError 10013）时，在获准主机环境运行，不能跳过 TCP 后记为通过。日志位于 `src-tauri/target/s7-session-tests/verification.log`；各案例目录保存状态、报文、事务及 DB 快照，本机产物不提交。

## 隔离桌面与证据边界

`pnpm test:ui:production` 使用 `tests/native/production-flow-ui.mjs` 与 `tests/native/tauri-flow-test.json`，应用标识为 `com.xyzrobotics.tujiaovision.flow-tests`，报告输出到 `output/playwright/flow-regression`。测试通过站点模式进入“离线 · 合成图”，经生产 S7 握手运行一件；使用真实后端、SQLite、PGM 和元数据，并在历史页读回；结束时退出离线并关闭本次进程。

其余保留的桌面测试入口及前置条件见 [原生测试目录](../../tests/native/)。恢复专项应使用独立实例和新输出目录，避免使用生产配置。Robot/Modbus 模拟服务、模拟节拍与回放目录相机已删除，由站点模式的离线产线取代。

jsdom 测试证明页面动作、关键接口参数及失败恢复，不证明真实布局、像素解码或硬件通信。桌面模拟流程证明业务接线与落盘，不证明真实 PLC、SDK、引擎测量准确率、现场节拍或持续运行。当前 UI 范围见 [UI 覆盖边界](UI_OPERATIONS.md)。

## 站点模式（真机 / 离线）

软件启动总是真机模式：测量用 lyFlow，相机只接海康 MVS，PLC 只支持 S7。顶栏模式标识显示“真机”“离线 · 样本集 <名称>”或“离线 · 合成图”，点击进入“离线控制台”（设备与运行 → 离线控制台，`/offline`）。

进入离线时软件临时派生并应用：PLC 切到进程内 S7 服务（127.0.0.1 随机端口、DB100 一期点表）；相机组按样本集 `cameras[]` 生成样本集相机，或按真实相机组同编号、同视角数生成模拟相机；测量用 lyFlow（样本集）或模拟测量（合成图）；帧录制临时为“全部”。真实 PLC、相机组和系统设置不改写，退出离线后全部恢复。离线期间相机页、PLC 页只读；有在途事务或整圈采集进行中时不能切换模式。离线模式不持久化，重启回到真机，但会记住上次来源和样本集目录。

**离线 · 合成图跑一件**

1. 断开外部 PLC，打开离线控制台，来源选“合成图”，点“进入离线”。
2. 选离线配方，工况选“正常件”，点“运行一件”；在线检测页也有离线配方、运行一件和停止的紧凑控件。
3. 在线检测与历史记录应出现同一 SN 的整件结果和原图。可再试其他工况（如传输丢帧、拍照数不符）或故障注入，确认得到预期 ERR。

**离线 · 样本集对照期望/实际**

格式契约见 [离线样本集格式 v1](../integration/sample-set-format.md)（只接受 schemaVersion 1）。样本集按 partId / shotId / cameraId / view 取图，缺图按缺帧处理；标签只用于展示期望，不参与判定。示例：`C:\Users\11601\gluesight-samples\mx11-demo`（单相机 cam1、单视角，p02 断胶 S004–S007）。

1. 确认系统设置已加载 lyFlow（引擎就绪），否则拒绝进入并提示原因。
2. 离线控制台来源选“样本集”，点“选择样本集目录”，核对件数、拍照点数、缺图和标签分布，点“进入离线”。
3. 无已发布配方时：整圈采集页点“开始接收”→“离线产线送一圈”，选样本件后采用本轮图像建立配方，示教、验证并发布。
4. 选离线配方和样本件（不选则按顺序循环），点“运行一件”或“连续运行”，用“本件后停止”/“立即停止”结束。样本集模式只提供握手类工况：正常件、传输丢帧、设备触发数与计划不符、运动超时、ACK 序号不匹配。
5. 核对“最近样本件”表的期望 / 实际：mx11-demo 的 p02 应为 NG（S004–S007 断胶），其余件的实际结论应与样本集标签一致。

**退出与历史复测**

1. 点“退出离线”，确认顶栏回到“真机”，PLC 配置、相机组和帧录制设置均为进入前的值。
2. 历史记录打开离线跑过的件，详情显示“离线样本 partId · SN · 期望 OK/NG”；因离线期间录制为全部，可直接原图复测或与候选配方对照。

## 本轮验证（2026-10-12，离线 S7 产线合并 P0 清理后）

| 检查 | 结果 |
| --- | --- |
| Rust 默认 | 195 passed、0 failed、36 ignored（含进程内 S7 服务、虚拟产线 + 生产 S7 会话集成测试） |
| S7 本机线协议 | `cargo test -- --ignored` 36 passed、0 failed |
| 前端 | 14 文件、41 passed |
| 类型 | `pnpm typecheck` 通过 |
| 生产桌面落盘 | 界面启用离线产线，正常件 OK，6 帧、460 测量点、6 张 PGM 和元数据落盘，SQLite 与历史原图读回通过；测试进程已关闭 |
| 原生脚本抽查 | `guard.js` + `plc-ui.js`（ui 实例）、`p0-plc-bootstrap-ui.js`（p0 实例）在离线 S7 下通过；其余 P0 链路脚本只做语法检查 |
| S7 Python | 23 项通过 |
| 打包工具、文档产物 | 本轮未重跑 |

本机日志与桌面报告 `output/playwright/flow-regression/result.json` 不提交。保留现有编译警告，未修改业务代码来消除警告。两份进程恢复专项只做脚本语法及依赖检查，本轮未重新执行完整场景。

## 历史验证记录

2026-10-11 配方创建软件交付曾通过 Rust 417 项、S7 35 项、前端 990 项，以及隔离桌面接口与重启恢复；见[当时交付记录](../architecture/recipe-creation-progress.md)。之后前后端分别精简，旧通过数字不能作为本轮结果。

[第一轮精简记录](unit-test-simplification.md)、[P0 历史计划](../architecture/p0-plan.md)与 `docs/testing/evidence/` 下已有 JSON 仅保存历史过程和证据。历史 DLL、压力、覆盖率及现场条目不再构成当前测试要求；已删除脚本可从 Git 历史查看，不再提供执行入口。
