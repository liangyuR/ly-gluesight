# 测试记录与复现

更新时间：2026-10-23（同步到段模型，即分段连拍飞拍；2026-10-22 起测试范围按选择收缩，测试删除不改变产品业务规则或运行时校验）。前端与后端的分组见[前端轻量测试](frontend-lightweight-tests.md)、[后端轻量测试](backend-lightweight-tests.md)。

## 频闪、Chunk 能力与参数反馈（2026-10-24）

- `cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib camera:: -- --nocapture`：19 项通过，完成新增 SDK 能力查询和配置分支的编译，未替代真机验证。
- 相机页面测试文件 18 项及 `pnpm typecheck` 通过。新增一个参数化回归（0 / 1 条参数问题）：保护 JHEM131GC-XS 验收提醒不计入失败数量，并防止恢复“保持写入前的值”的错误承诺。
- 真机待验收：LineSource 仅提供 ExposureActive 时频闪配置写入、回读成功；设备没有 Chunk 节点时只显示一条不支持提示；验收提醒独立显示。参数写入不执行整组回滚。

## Software 单帧与 TriggerDelay 兼容（2026-10-24）

- `cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib camera:: -- --nocapture`：19 项通过，包含现有 Software 帧归属与触发完成保护；本次 SDK 适配代码完成编译，不代表设备写入已实测。
- 本机 MVS DLL 导出检查：`MV_XML_GetNodeInterfaceType`、`MV_CC_GetIntValueEx` 均存在；绑定布局与安装 SDK 头文件核对。
- 真机待验收：仅有 FrameBurstStart 的设备切换到 Software 后，回读 BurstFrameCount=1，每次手动命令只得到一帧；具有 FrameStart 的设备继续选择 FrameStart。
- 真机待验收：Integer TriggerDelay 写入整数微秒并回读一致，小数应明确失败；Float 节点继续接受浮点值。类型查询、写入或回读失败时相机不得就绪。
- 后续按用户要求，仅对 JHEM131GC-XS 放开 Line0 的 FrameBurstStart + LevelHigh 配置，强制回读 BurstFrameCount=1、触发缓存关闭及已有帧率和触发参数。其他型号不增加 FrameBurstStart 门控回退。高低电平时序由用户在软件中实测，尚未完成硬件验收；未实现生产自动分段。

## 相机故障维护与状态提示（2026-10-24）

- `camera::access_tests::unresolved_plc_without_active_capture_allows_maintenance_but_keeps_configuration_locked`：一个参数化回归覆盖六种工况，防止历史未结 PLC 事务被误当实际采集阻止释放/重连，同时保护真实在途、示教采集和普通配置操作。`cargo test --manifest-path src-tauri/Cargo.toml --lib camera::access_tests -- --nocapture` 通过。
- `tests/camera-page.test.tsx` 新增初始化失败回归：参数面板保留原始门控错误，不把未就绪误报物理未连接，Fault 加历史 PLC 锁仍可发起释放。该文件 16 项及 `pnpm typecheck` 通过。
- 差异检查通过。未操作现场 PLC 或状态文件，未通过程序执行真机释放或复位，也未放宽门控节点检查；用户反馈复位后可释放与重连，是原未结事务阻塞的现场佐证，不代表高电平连续采集已验收。

## 高电平门控与 Software 调试验证（2026-10-23）

| 检查 | 本轮结果 |
| --- | --- |
| Rust 定向测试 | 相机、cycle、握手、PLC 会话、采集、帧组、配方、标定一致性及选定离线边界共 183 项通过；离线环境 `site::tests` 另 2 项通过 |
| 前端与类型 | 相机、标定、采集、示教、配方编辑、进度与几何相关 64 项通过；`pnpm typecheck` 通过 |
| S7 与协议文档 | Python S7 27 项通过；协议 HTML 生成物检查通过；协议仍为 6，DB 布局不变 |
| 桌面端到端 | 已构建调试应用，隔离实例运行 `pnpm test:ui:production -SkipBuild`，使用 lyFlow 1.1.5，54.8 秒通过；10 轮普通标定、2 轮标准件、独立验证、发布和生产 OK；12 帧组、36 张原图、SQLite 和历史读回通过 |
| 默认数据 | 首次启动空配方及重启回归通过；本机两个默认配方文件不存在，用户要求保留的工作区草稿仍在 |

桌面测试等待采集完成后的 PLC 空闲状态，并对设备事务忙响应使用有界重试；产品侧采集期间禁止配置变更的约束保持不变。报告保存在已忽略的 `output/playwright/flow-regression/result.json`。本轮为定向验证，未执行全量测试或安装包验收。

SDK 枚举成功但本机发现 0 台设备；Jinghang JHEM131GC-XS 的门控节点、Software 一命令一帧、低电平停止新曝光、触发切换、重连与多相机行为仍待实机验证。SCL 尚未在 TIA Portal 编译或下载至 PLC。

## PR #169 CI 修复与 main 同步（2026-10-23）

- 原 Windows CI 在前端测试失败：删除页面后遗留的旧拍照点示教 17 项专属用例仍被执行。删除 `tests/fixture-teach.test.tsx` 的 9 项及 `tests/teaching.test.tsx` 的 8 项旧单帧用例，保留后者 10 项段配方与通用采集信息用例；不恢复已停用产品入口。
- 同步 main 的 PR #168 Playwright 桌面测试框架，保留实例锁、清理和失败证据，改为仅运行段配方；CI 恢复 lyFlow 后执行 burst，不再运行 legacy 或因缺运行库全部跳过。
- 修正存储往返用例的丢帧夹具：同一相机缺两帧，符合当前相机间帧差告警阈值，继续覆盖告警、缺帧与预留字段的落库读回。
- 合并后前端全量 50 个文件、332 项通过；类型检查通过。Playwright 实例复用未修改的产品构建，1 项段配方完整流程通过、0 跳过，62 秒；工作流、桌面基础设施及长稳判断脚本共 32 项通过。

本机曾出现沙箱临时文件缺失、临时 Git 元数据写入受限，以及自定义 TEMP 导致受管样本拒绝访问；对应检查在正常环境、系统默认 TEMP 下重跑，不修改产品代码来绕过限制。远端 CI 结果以本 PR 最新提交为准。

## 当前保留范围

| 测试层 | 保留验证 |
| --- | --- |
| 前端 | 创建配方、草稿编辑、采集、示教验证发布、生产操作锁、周期/图源隔离与历史显示；段配方的段列表、多轮标定、相机走廊示教、发布确认与按区域和帧组的历史展示；旧拍照点页面及专属用例不再维护 |
| Rust 默认 | 配方版本、采集与示教流程、相机/帧关联、生产周期、PLC、调度、SQLite、原图录制、审计恢复及历史；段模型的 `plan`、`frame_group`、`segmentation`、`burst_judge`、`calibration` 五个纯模块，以及段配方的示教采集、生产节拍、候选工作区与发布、按帧落库和离线脉冲串仿真；不再维护旧拍照点运行路径与专属测试 |
| S7 专项 | `plc_session` 的 S7 线协议用例（真实生产驱动 + 进程内 S7Server，已进默认 `cargo test`，不再 ignore、不依赖 Python），以及 Python 测试 PLC 自己的线报文自测 |
| 桌面端到端 | 仅合成图段配方生产落盘：flow 实例内建站、标定、发布、生产、SQLite、原图与历史读回，需要 lyFlow。现场样本集桌面入口和旧拍照点流程已删除 |
| 保留工具 | 打包契约、Windows 发布门禁、安装包自检、样本集转换脚本、S7 Python 服务与 Robot/Modbus 隔离协议 |
| 手动专项 | 长稳测试、算法评测，需样本集与 lyFlow 运行库，不进 CI；吞吐探针 `throughput_probe`（3 × 25 Hz 门槛的实测依据，见“段模型专项”） |

各层用例数量随代码变化，以实际筛选命令和 CI（`.github/workflows/windows-ci.yml`）结果为准。需要 lyFlow 或样本集的专项不属于无外部输入的默认验证；段配方走廊真引擎用例见下文。旧拍照点专属测试不作为当前验收要求，历史运行结果不能替代本轮验证。

此前测试收缩已删除真实 DLL 的 8 项专项、性能与队列压力专项、性能工具自身测试、Robot 五工况桌面联调、现场验收清单、冻结发布资源的 9 项跨平台独立测试（Windows 默认 8 项）和质量判定规则的 11 项独立测试。旧 flyshot 脚本、去哈希专项、重复的 P0 性能/联调准备入口、覆盖率报告及重复前端快捷入口同步清理。保留的主流程可能经过发布和判定，但不据此声明已覆盖被删除组的全部规则。

## 段模型（分段连拍飞拍）专项

段模型的测试按 AGENTS.md 准入编写：每个用例对应一条业务规则、对外契约或易错边界，名称写成“条件 → 预期结果”，不测 getter、DTO 与框架行为。旧拍照点配方从未部署，已停止使用；不再维护其运行入口、迁移、兼容分支及专属测试。保留通用标定与 Software 单帧调试。PLC 协议保持版本 6 和现有 DB 布局，`deviceTriggers` 为预留字段、PLC 写 0；Line0 高电平门控，帧率由软件设置、默认 10 fps。规则取舍见 [分段连拍决策记录](../decisions/segment-burst.md)，本轮新增、删除与改写的用例及其保护的规则见 [分段连拍测试清单](segment-burst-test-inventory.md)。

**分段算法未开发。** `segmentation` 只有 `Segmenter` 接口、`Unsegmented` 实现（整条胶路一个未分段区域）和合成序列回归入口（`segmentation::synthetic` 的 `generate` / `evaluate`），走向、光流、DTW 等真正的分段算法要等现场弯道图像；所以现有测试与端到端都只验证“未分段、只跑每帧独立检测、不判胶偏”，不验证分段效果（决策记录第一节、D-SB-21）。

**走廊测量的真引擎用例。** `src-tauri/src/vision/taught/engine_tests.rs` 的 `engine_burst_publish_load_measure_frame_round_trip` 默认忽略，需要 `LYFLOW_CORE_DLL`（已发布运行库即可，不要求 lyFlow#32）：段配方发布包（schema 4）经 `release::load` → `Prepared::load` → 预热 → `measure_frame` 的生产路径与真实 `glue.taught_path` / `glue.bead_width` 对得上，胶宽当量与 x 的法向换算正确。

```powershell
$env:LYFLOW_CORE_DLL = 'src-tauri\target\dependency-cache\lyflow\1.1.5\lyflow_core.dll'
cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored vision::taught::engine_tests::engine_burst_publish_load_measure_frame_round_trip
```

**离线工况（段配方）。** 段配方在离线下按脉冲串运行：布防后以固定频率（默认 20 Hz）发脉冲，每个脉冲同时触发三台相机，`deviceTriggers` 保持 0（预留字段），脉冲发完置 partEnd；名义帧数取各段 N_exp（待标定的段按段长 × 频率 ÷ 50 mm/s 估算，已有整件标定时缩放到 ΣN_exp）。下表工况在离线控制台与在线检测页的工况下拉里，样本集与合成图来源都能选，只改变脉冲与握手，不改图像内容；旧拍照点配方不再支持。

| 工况（界面文字） | 做什么 | 预期 |
| --- | --- | --- |
| 单台相机缺帧（该组检测不完整） | 第 2 台相机丢第 min(3, 总数) 帧，相机照常计数 | 该帧组标“检测不完整”，丢帧告警，OK/NG 不变，不判 ERR |
| 脉冲数波动（首尾 ±1 帧） | 首尾各多发或少发 1 个脉冲 | 帧组数偏离名义最多 2 个；在 ΣN_exp ± Σtol 内不告警，越界只告警（帧数异常，低于下限还标疑似提前终止），OK/NG 不变 |
| 弯道减速（弯道帧数波动） | 每个弯道段帧数 ±2 | 同上 |
| 本件未检（机器人未得到触发许可） | 机器人照常涂胶，PLC 不提交 partStart | PC 没有该件记录，离线运行记“本件未检” |
| 提前 partEnd（疑似提前终止） | 发约一半、且少于 ΣN_exp − Σtol 个脉冲后置 partEnd | 用已收到的帧照常判，加 `short_part` 与“帧数异常”，不改 OK/NG |
| 帧数诊断 | 比较 SDK 帧序号、相机间帧数及传输丢帧 | 不以预留字段 `deviceTriggers` 判断丢帧或排空完成 |
| 传输丢帧 / 缺帧 | 三台相机的同一帧都丢 | 该帧组标检测不完整，不判 ERR |

**合成图采集与标定。** 合成图来源的 `offline_capture` 为段配方的新轮次送一圈：按候选配方（须能编译）的名义剖面发脉冲串，首尾 ±1、弯道 ±2，种子取该候选的轮序号，所以每一轮帧数不同，多轮标定能看到 min / max 差异；合成图离线环境因此能完成 10 轮标定、走廊示教、验证与发布（加入标定时每帧测量用 lyFlow 走廊测量，需要加载 lyFlow）。合成图相机沿用真实相机组的编号与视角数，单视角相机不足 3 台时补足到 3 台。合成图来源的离线采集只服务段配方的新轮次；样本集来源同样仅服务段配方轮次，不限 1–64、各相机帧数可以不等（帧数少的相机在后面的帧组缺帧），虚拟 PLC 按帧数最多的那台发脉冲（`offline::offline_capture`）。样本集按“每台相机的帧序列”解释（见[离线样本集格式 v1](../integration/sample-set-format.md)），现有的 2026-10-17 单圈样本集是停稳拍数据，没有弯道、帧数没有波动，只作冒烟，不作分段或标定的验收依据。

**3 × 25 Hz 吞吐门槛。** 三台相机各 25 Hz 同时出帧是 75 帧/s，1280 × 1024 单字节灰度每帧 1.31 MB，约 98 MB/s。P1 探针量测量耗时与 PGM 写盘速率；它是 `src-tauri/examples/throughput_probe.rs`，不是测试，不在 `cargo test` 里，写约 4 GB 临时文件、结束后自动删除，约 1 分钟，需要已发布的 lyFlow 运行库：

```powershell
$env:LYFLOW_CORE_DLL = 'src-tauri\target\dependency-cache\lyflow\1.1.5\lyflow_core.dll'
cargo run --manifest-path src-tauri/Cargo.toml --example throughput_probe
```

门槛：现场机用探针得到串行 p95，代入 `ceil(75 fps × p95 / 0.7)` 得到所需测量线程数，不得超过 `min(4, 逻辑核 − 2)`（`MEASURE_WORKERS`）；压测中不得出现图像内存预算满或测量队列满（出现时该帧记缺帧或未测，该组标检测不完整，不会静默丢帧）。不满足时先降低相机设定帧率，各段 N_exp 随之变化，须重新标定并分配新的计划版本。开发笔记本上的实测只是参考，不能外推到现场工控机；测量与写盘是分开测的，二者与界面、SQLite 同时运行的 3 × 25 Hz 压测由 E2 的 `pnpm test:soak:throughput`（`scripts/soak/throughput.mjs`）做，**该门槛目前没有达标，也没有被直接验证，已定（2026-10-23）：门槛口径不改，等现场工控机用真实相机来源跑 `pnpm test:soak:throughput` 后再决定降低相机帧率还是加测量并发，合并前须完成该复测**；实测与能证明的范围见“段配方端到端验证（2026-10-23，E2）”。完整数据与降级方案见决策记录 7.1。

**升级前检查。** 升级到协议 6 / 段模型之前，先确认 `audit-spool/` 已清空（审计 spool 格式 3 → 4，残留的旧格式文件会让就绪检查失败、拒绝接件），记录库 8 → 9 会备份后重建；其余升级项见[存储布局](../architecture/storage-layout.md#升级到段模型协议-6前)。

**文档产物核对。** `python scripts/docs/build-plc-protocol.py --check` 核对协议 HTML 与 Markdown 同步。

## 运行

使用 Node 24.15.0、pnpm 12.15.0、Rust 1.99.1。首次安装执行 `pnpm install --frozen-lockfile`。

| 命令 | 用途 |
| --- | --- |
| `pnpm test` | 全部前端测试，包含创建配方与完整示教发布流程 |
| `pnpm test:desktop:build -Profile flow` | 只构建隔离桌面实例，不启动；profile 可选 ui（默认）、p0、flow、e2e、soak |
| `pnpm test:watch` | 开发时持续运行相关前端测试 |
| `pnpm test:ui:production` | Playwright 自动构建、启动与关闭隔离实例，验证合成图三相机段配方建站与生产落盘，需要 lyFlow |
| `pnpm typecheck` | 应用及测试 TypeScript 类型检查 |
| `pnpm build` | 生产前端构建 |
| `cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib` | 默认后端回归，依赖需已缓存 |
| `pnpm samples:test` | 样本集转换脚本（`scripts/sample-set/convert.py`）测试，只需 Python 3 标准库 |
| `pwsh -NoProfile -File tests/packaging/Test-RuntimeBundle.ps1` | 打包版本、ABI、源码修订及运行库依赖契约 |
| `pwsh -NoProfile -File tests/packaging/Test-LyFlowDependency.ps1` | lyFlow 依赖的 Release 选择、配置与 Cargo 锁定、运行库校验、缓存与网络回退、升级回滚（网络与 DLL 探针用本地夹具） |
| `pwsh -NoProfile -File tests/packaging/Test-VendorDependency.ps1` | 厂商运行库的包隔离、结构、尺寸、CRT 版本、固定源恢复与缓存边界 |
| `node --test tests/packaging/windows-workflow.test.mjs` | Windows 打包工作流门禁：版本与安装方式校验、发布标签须在 main 上、版本设置、SDK 干净环境检查、视觉检查先于打包、依赖缓存标识与发布清单 |
| `pnpm test:soak:throughput`（即 `Test-Soak.ps1 -Stages throughput`） | 3 × 25 Hz 吞吐门槛（段配方、离线·样本集 lyFlow 测量，`-ThroughputSource synthetic` 压合成图帧管线）：连续跑 `-ThroughputSeconds`（默认 60 s），核对 Unmeasured = 0、图像内存预算丢帧 = 0、测量队列峰值低于容量的 50%，并要求产线跑满时长、持续负载已验证（分项只在满负载下才有意义）。**持续负载按相机使能为高的时间窗计**：PC 看不到使能信号，窗口取每件第一帧到达到最后一帧到达（运行日志“帧到达”，首个帧组不计入，栅栏计数），Σ窗口内帧数 ÷ Σ使能时长 ≥ 3 × Hz 的 90%，且至少连续 `-ThroughputMinParts`（默认 10）件逐件达到；件间握手间隙（使能为低，离线虚拟 PLC 每件约 0.9 s，是 PLC 握手开销而不是 PC 的测量能力）单独报告为 `achieved.handshakeGapSeconds`，不参与判定，含握手的平均帧率 `achieved.framesPerSecond` 只作参考。达不到时退出码 1，实测值写进 `output/soak/throughput-*.json`。持续负载未验证时默认判不达标；只想看分项时加 `-AllowBurstOnly` 放行，报告里 `sustainedVerified` 仍为 false、`burstOnlyAccepted` 为 true，不是持续门槛的结论（分项不达标时 `burstOnlyAccepted` 为 false）。样本集每件只有 12 个脉冲，件间握手期间测量队列排空，队列峰值只反映每件的积压，不间断 3 × Hz 下的积压看 `projection` 的外推。判定函数的规则由 `node --test tests/native/throughput-gate.test.mjs` 保护，该文件与 `soak-faults-assess.test.mjs` 由 CI（`windows-ci.yml`）的“Soak script decision tests”步骤运行 |
| `pnpm test:soak`（即 `pwsh -NoProfile -File scripts/Test-Soak.ps1`） | 长稳测试（隔离 soak 实例）。没有必填参数，但样本集与 lyFlow 运行库必须经 `-SampleSet` / `GLUESIGHT_SAMPLE_SET`、`-LyFlowRuntime` / `GLUESIGHT_LYFLOW_RUNTIME` 提供（不自动恢复运行库，缺失时退出码 3），例如 `pnpm test:soak -SampleSet <样本集目录> -LyFlowRuntime <运行库目录> -Parts 1000`；阶段、参数与退出码见[长稳报告](soak-report.md) |
| `pwsh -NoProfile -File scripts/Test-AlgorithmEval.ps1` | 算法评测（忽略的 cargo 评测，需样本集与 `lyflow_core.dll`），参数与读数见[算法评测](algorithm-evaluation.md) |
| `pwsh -NoProfile -File scripts/Test-DevStartup.ps1` | Windows 旧终端在设置/清除本地运行库后启动，子进程读取最新保存值，参数与退出码保持不变 |

`Test-DevStartup.ps1` 临时修改用户级 `GLUESIGHT_LYFLOW_RUNTIME`，在 `finally` 中恢复原值；验证期间不要并行修改该配置或启动 GlueSight。测试通过临时 CLI 进程观察启动环境，不启动实际 GUI，不修改配方。

单文件前端测试示例：`pnpm exec vitest run tests/teaching.test.tsx`。默认测试不生成覆盖率报告。生产落盘入口的参数、前置条件与已知限制见下文“端到端入口”。

Windows 受限环境遇到 Node realpath EPERM 时，按仓库 AGENTS.md 的 Windows 指引在同一进程加载兼容脚本。需要获准的主机权限时运行相同命令，不修改应用逻辑掩盖环境失败。Cargo 首次构建需要 MSVC、Visual Studio C++ 工具及私有 Git 依赖读取权限；离线执行要求依赖已缓存。

## S7 一期握手

协议与点表见 [S7 一期握手说明](../integration/plc-s7-phase1.md)。[S7 测试 PLC](../../scripts/s7-handshake/README.md) 仅监听本机回环地址，使用真实 TPKT/COTP/S7 报文。S7 协议版本 6（分段连拍）沿用协议 5 的布局（交互表 V1.1、S7-1200、DB4/DB2 两个标准 DB、Int/DInt、`String[50]` SN），改变的只有计数字段的语义；协议 5 及更早的点表和 PLC 程序被拒绝。下列专项的通过数据：Python 测试 PLC 自测 2026-10-22 在主机环境 27 项通过；`plc_session` 用例 2026-10-22 迁到进程内 S7Server，协议 6 下的运行结果以 CI 为准，本文不记数字。

```powershell
python -m unittest discover -s scripts/s7-handshake/tests -v
cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib -- plc_session offline::s7_server offline::start_request_tests
```

`plc_session` 的用例由进程内 S7Server（`src-tauri/src/offline/s7_server.rs`，只监听 127.0.0.1 的随机端口，含仅测试编译的定点故障注入）驱动真实的生产会话，已是默认测试，不再需要 Python 夹具、`--include-ignored` 或 `S7_TEST_PYTHON`；Python 测试 PLC 只剩它自己的线报文自测（`scripts/s7-handshake/tests`，需要 Python 3）。两者都需要本机 TCP 回环权限：Windows 沙箱拒绝 localhost 连接（WinError 10013）时，在获准主机环境运行，不能跳过 TCP 后记为通过。各案例的状态、事务与 DB 快照写在测试临时目录（`s7-session-tests`），通过后自动清理，本机产物不提交。

## 特征定位真引擎历史说明

本节保留旧拍照点专属真引擎测试的历史方法与证据边界，不属于当前验收流程，也不要求维护相关兼容路径。当前段配方走廊全链路用例见“段模型专项”，通用标定能力保留。

`src-tauri/src/vision/taught/engine_tests.rs` 在合成工件（定位框里两个暗孔和一个亮 L 角，暗胶条沿示教线）上跑真实 lyFlow 的 glue.locate 与 bead_width，E1–E5 经发布包的生产路径：临时目录里 `release::publish`（模板由包内示教原图裁出）→ `release::load` → `Prepared::load`（含预热的定位验收）→ `measure_views`，再交给 `judge`：整件平移（+12, +7 px 与切向 −25 px）后 d′ 不变、两端超出检测范围、判 OK（E1）；只移胶 8 px 时 d 沿法向变 0.8 mm、判 NG_POSITION（E2）；定位框被抹平时有胶站全部无基准、胶宽保留、判 ERR 92（E3）；加载后把包内模板换成同尺寸、特征被抹平的一块，下一帧位姿不再被采用、判 ERR 92，证明生产读的是发布包自己的模板（E4）；定位框逐像素挪向胶路，发布要在带外圈的框碰到漂到最远可测处（−16 px）的胶之前以走廊检查拒绝，最近一个能发布的框上胶漂过去时位姿不动、d 变 −1.6 mm、判 NG_POSITION（E5）。E8 走工作台示教试测的同一条路（G4）：模板从示教图裁出放进临时目录、在示教图上测量，按示教图检查出报告——示教检查通过、自定位位姿恒等（本地构建上偏移 0 px、次峰比 0.279），亮度 ±40 的帧经验证统计的锁定判据不算错锁；定位框（含外圈附近）抹成噪声后纹理检查不过。合成图的亮区 +40 也不饱和，E8 只证明接线与恒等容差，±40 能否拦住真实过曝错锁要看现场样本。`src-tauri/src/packaging.rs` 另有 E6：打包自检的定位冒烟在支持带外圈模板的运行库上报 ok。默认忽略，需要带 lyFlow#32 glue.locate 的 `lyflow_core.dll`（发布前用本地构建）：

```powershell
$env:LYFLOW_CORE_DLL = '<lyFlow 构建>\build\core\bin\lyflow_core.dll'
cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored vision::taught::engine_tests packaging::tests::locate_smoke_passes
```

核心库的 glue.locate 不支持带外圈模板（1.0.0）时测试直接失败，不算通过。发布包加载时模板被改（同尺寸改像素）的拒绝与引擎无关，由默认运行的 `release::tests::template_tampered_with_same_size_or_dropped_with_its_reference_is_rejected_on_load` 覆盖。

E7 反过来验证已发布运行库（`Restore-LyFlowRuntime.ps1` 恢复的 1.1.4，glue.locate 1.0.0）：打包自检的定位冒烟报 `unsupported` 并写明原因与影响，不当作失败：

```powershell
$env:LYFLOW_RELEASED_CORE_DLL = 'src-tauri\target\dependency-cache\lyflow\1.1.4\lyflow_core.dll'
cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored packaging::tests::locate_smoke_reports_unsupported
```

## 隔离桌面与证据边界

`pnpm test:ui:production` 由 Playwright Test 驱动，使用 `tests/native/production-flow-ui.mjs` 与 flow 实例配置（`Start-UiTestInstance.ps1 -Profile flow`），应用标识为 `com.xyzrobotics.tujiaovision.flow-tests`，报告输出到 `output/playwright/flow-regression`。全新实例不安装默认或旧配方夹具。

测试在“离线 · 合成图”下用真实 lyFlow 完成段配方走廊示教、多轮标定、独立样本验证与发布，经生产 S7 握手投产，核对 SQLite、逐帧原图和历史读回。使用 `-LyFlowRuntime <运行库目录>` 或 `GLUESIGHT_LYFLOW_RUNTIME` 提供运行库；缺失时明确记录跳过，不算通过。合成图来源补足三台单视角相机。`deviceTriggers` 仅保留存储字段，不作为采集计数依据。

结束时退出离线并关闭本次进程。

[`tests/native/`](../../tests/native/) 没有单独说明文件：当前端到端入口的驱动脚本见“端到端入口”，它们与 `scripts/soak/` 共用的 IPC 客户端、实例连接、样本集副本、整圈采集、示教与投产函数在 `tests/native/lib.mjs`；实例配置不再放在该目录，由 `Start-UiTestInstance.ps1` 按 Profile 生成（长稳测试用 soak）。其余为下表保留的 P0 脚本及“本轮验证”中的原生脚本抽查。恢复专项应使用独立实例和新输出目录，避免使用生产配置。Robot/Modbus 模拟服务、模拟节拍与回放目录相机已删除，由站点模式的离线产线取代。

**P0 原生脚本历史处理记录（不作为当前操作入口）**

旧 P0 界面链路以 `p0-setup-ui.js` 在离线期间新建三视角模拟相机、`p0-create-ui.js` 在“拍照点规划”页手工编辑拍照点为起点，后续脚本都依赖它产生的 `P0-TRICAM-UI` 配方。站点模式下离线相机只由离线来源生成、离线期间相机页只读，拍照点改由整圈采集生成，“取新样本”“拍照点规划”和视觉设置也已删除，这条链路无法成立。处理如下：

以下 P0 清理表记录当时替代关系；其中离线建站、回放与重启脚本本次已删除，对应覆盖不再保留。

| 脚本 | 处理 | 理由 |
| --- | --- | --- |
| `p0-setup-ui.js`、`p0-create-ui.js`、`p0-camera-refresh-ui.js` | 删除 | 依赖离线期间增改模拟相机、手工编辑拍照点和已删除的视觉设置；建站、采集、建候选由离线端到端覆盖 |
| `p0-teach-ui.js`、`p0-invalidation-ui.js`、`p0-camera-invalidation-ui.js` | 删除 | 依赖“取新样本”和“拍照点规划”改 Pose / 相机；示教由离线端到端与前端示教测试覆盖，视角切换和改参后失效由 `workspace/workflow_tests.rs` 覆盖，相机与 Pose 现由采集轮次决定 |
| `p0-validate-publish-ui.js`、`p0-publish-reviewed-ui.js` | 删除 | 依赖本机不提交的 PGM 证据目录；后者断言“期望 OK、实际 OK_WITH_EXCURSION 判为不符合”，与合格类都算符合的决策冲突；验证发布由离线端到端覆盖 |
| `p0-online-ui.js`、`p0-history-produce-ui.js`、`p0-frame-visibility-ui.js` | 删除 | 依赖已删除的 `settings.vision` 字段和三视角配方；合成图生产落盘由 `test:ui:production` 覆盖，样本集投产、录制与原图读回由离线端到端覆盖；布局可见性属于当时的界面修复验收证据 |
| `p0-history-ui.js` | 改写进离线端到端 | 历史导出 CSV、按原发布包重现、按候选规则重判、从原图复测整件、已保存对照切换与“原始记录和候选不被改动”，现从离线端到端的投产终态在历史详情界面执行 |
| `p0-history-independent-ui.js` | 删除 | 依赖“拍照点规划”里切换其他候选、未保存的 Pose；原发布包重现只用冻结发布包的规则由离线端到端的重现断言（发布包、配方修订一致）覆盖 |
| `p0-restart-connect-ui.js`、`p0-startup-ui.js` | 改写为 `offline-restart.mjs` | 重启后已发布配方按原发布包预热、可再次投产且结论不变，现从离线端到端终态重启同一隔离实例验证 |
| `p0-plc-bootstrap-ui.js` | 保留 | 独立于链路：界面进入离线·合成图，核对 S7 回环地址与 28 个 DB4/DB2 点（协议 5 与协议 6 布局相同）；2026-10-12 按协议 4（DB1）在 p0 实例上实跑通过，协议 5、6 下尚未实跑 |
| `p0-single-instance-acceptance.mjs` | 保留 | 前置为 p0 实例已有原图且节拍空闲（当时使用已移除的 `-LegacyRecipeFixture`；先跑 `p0-plc-bootstrap-ui.js` 再离线运行一件 MTR-HSG-A；运行库目录可经 `GLUESIGHT_NATIVE_PATH` 前置到 PATH）；2026-10-12 实跑通过，重复启动以独占锁拒绝（退出码 101），数据库与原图不变 |
| `p0-ack-process-recovery.mjs`、`p0-audit-process-recovery.mjs`（及审计专用配置 `tauri-audit-test.json`） | 删除 | 需要外部 S7 Python 夹具（真机模式）配合模拟相机；真机模式加载时会把非海康相机改回海康，模拟相机只在离线时由站点模式生成并强制使用进程内 S7，站点模式下无法运行。另依赖已删除链路建立的 `P0-TRICAM-UI` / 701 配方，审计脚本还断言已不存在的“模拟 / 回放相机”拒绝原因。ACK 日志落盘后的重启恢复、审计 spool 重启后的回放由 `plc_session::recovery`（`persisted_ack_is_recovered_after_close_and_reopen_idempotently` 等）、`store`（`durable_ack_recovery_matches_cycle_after_reopen_and_preserves_original_evidence`）、`audit`（`durable_spool_survives_coordinator_ttl_capacity_and_replays_out_of_order_evidence_after_restart` 等）的 Rust 测试覆盖。当初引用的 `s7_wire_restart_after_persisted_ack_recovers_exact_cycle_before_and_after_reset` 已在 S7 用例迁到进程内 S7Server 时删除（`836a51b`），真实进程终止的子进程用例 `durable_spool_recovers_after_real_process_termination_before_any_sqlite_insert` 已于 2026-10-22 删除（`c6c2ba5`，审计 spool 已 fsync）；事务中强制结束进程后的恢复由长稳的 `restart` 阶段演练 |

jsdom 测试证明页面动作、关键接口参数及失败恢复，不证明真实布局、像素解码或硬件通信。桌面模拟流程证明业务接线与落盘，不证明真实 PLC、SDK、引擎测量准确率、现场节拍或持续运行。当前 UI 范围见 [UI 覆盖边界](UI_OPERATIONS.md)。

曝光与增益随配方保存（每个配方每台相机一组），相机配置不再含这两项。写入海康相机的时机有三处：采集页点“应用”、整圈采集开始前、生产布防的 `camera-imaging` 阶段（PLC 换型同样，写不进去拒绝布防）。合成图与样本集相机下发为空操作，自动测试只覆盖配方校验、候选保存与界面调用；实际写入 ExposureTime / Gain 须在台架用海康相机确认。

## 端到端入口

2026-10-23 按用户决策删除依赖现场样本集的离线建站及重启、日志回放、特征定位桌面测试、PowerShell 外壳和 pnpm 入口。下文历史运行记录保留当时事实，不再是可执行指引；这些场景不在当前自动回归范围。长稳、硬件联调和底层业务范围不变。

本次删除文件与依据：用户明确要求删除依赖现场样本集的桌面测试，连同专属入口一并移除：

- PowerShell：`scripts/Test-OfflineE2E.ps1`、`scripts/Test-LogReplayOffline.ps1`、`scripts/Test-FixtureE2E.ps1`。
- Node：`tests/native/offline-e2e.mjs`、`tests/native/offline-restart.mjs`、`tests/native/log-replay-offline.mjs`、`tests/native/fixture-teach-e2e.mjs`。

对应 `test:e2e:offline`、`test:e2e:log-replay`、`test:e2e:fixture` 命令已删除；`tests/native/lib.mjs`、`scripts/E2E-Common.ps1` 及 `scripts/soak/` 仍被生产落盘或长稳使用，予以保留。

生产落盘入口检查参数后按 flow profile 构建一次（`-SkipBuild` 跳过），通过独立 Playwright Test 配置运行。夹具负责启动、读取清单、连接主窗口、采集证据和关闭隔离实例；成功返回 0，构建、启动或业务失败返回非零。显式提供的 lyFlow 目录无效时在构建及启动前失败。决策见 [r2-test-infra 决策记录](../decisions/r2-test-infra.md)。

公共构建入口为 `pnpm test:desktop:build -Profile <profile>`（`scripts/Build-UiTestInstance.ps1`）；原 `Start-UiTestInstance.ps1` 单独使用时仍默认构建。生产落盘命令由 PowerShell 检查输入、准备构建并调用 Playwright Test，夹具复用现有实例启动和身份校验。`Test-Soak.ps1` 继续通过 `scripts/E2E-Common.ps1` 使用共享能力，长稳的入口和退出码不变。Node 业务函数仍在 `tests/native/`，与长稳共用的 IPC、采集、示教和投产函数在 `tests/native/lib.mjs`。生产落盘旧 Node CLI 暂保留，供迁移对照；现场样本集桌面 CLI 已删除。

Playwright Test 固定 `workers=1`、`fullyParallel=false`、`retries=0`，每个用例最多 15 分钟；业务已有的短暂拒绝重试保持不变。段配方用例初始化和恢复配置；已移除没有消费者的模块级重试统计。

业务 JSON、截图和实例日志保留原目录，框架输出独立放在 `output/playwright/<profile>-regression/runner/`：`html/` 是可浏览报告，`junit.xml` 是 CI 结果，`test-results/` 保存失败截图与 trace。CDP context 的 trace 由夹具显式管理；取证失败不会覆盖业务失败。构建、启动、断言错误分别保留原始原因和日志位置。

| 命令 | 复用的业务函数模块 | 实例配置（应用标识） | 默认端口 | 输出 |
| --- | --- | --- | --- | --- |
| `pnpm test:ui:production` | `tests/native/production-flow-ui.mjs` | flow（`…flow-tests`），保留资料 | 9339 | `output/playwright/flow-regression` |

参数跟在命令后，例如 `pnpm test:ui:production -Suite burst -DebugPort 9351 -SkipBuild`。

| 参数 / 环境变量 | 作用 |
| --- | --- |
| `-DebugPort` | WebView2 远程调试端口，`127.0.0.1` 与 `[::1]` 上都必须空闲，否则启动前报错 |
| `-SkipBuild` | 复用上次构建复制到输出目录的 `gluesight-ui-tests.exe`（需同一配置构建过，不保证与最新源码一致） |
| `-Grep` | 生产落盘入口，传给 Playwright Test 的用例名筛选 |
| `-Suite burst` | 仅段配方生产落盘，默认 burst |
| `-ResetData` | 仅生产落盘新增可选开关，默认 false，保留资料；开启时首启重置资料 |
| `-DataRoot` / `GLUESIGHT_TEST_DATA_ROOT` | 隔离资料目录的父目录，资料目录为 `<DataRoot>\<应用标识>`；默认在输出目录下 |
| `-LyFlowRuntime` / `GLUESIGHT_LYFLOW_RUNTIME` | 生产落盘段配方的运行库；未指定时恢复锁定版本，恢复或加载失败即失败，不以跳过代替验证 |
| `CARGO_TARGET_DIR` | 共享或自定义 Cargo 构建目录时设置，脚本从这里取构建产物 |

**前置条件**：Windows 与 WebView2 Runtime；Node 24.15+，已 `pnpm install --frozen-lockfile`；Rust 工具链与私有 Git 依赖读取权限（构建时）。生产落盘使用合成图，无需现场样本集；段配方需要 lyFlow。

**隔离实例的启动**（`scripts/Start-UiTestInstance.ps1`，生产落盘夹具调用，也可单独启动后手动驱动）：

- Tauri 覆盖配置按 `-Profile`（`ui`、`p0`、`flow`、`e2e`、`soak`）在脚本内生成：产品名与窗口标题为 `GlueSight · 胶路智检 <Profile> Tests`，应用标识 `com.xyzrobotics.tujiaovision.<Profile>-tests`，窗口隐藏，不打安装包。由公共构建入口写到输出目录的 `tauri-test-config.json` 再交给 `tauri build --config`（包含 TypeScript 与 Vite 生产前端构建）；`-SkipBuild` 不重建配置或产物，优先核对 `build.json` 的 profile 标识；兼容旧构建时回读 `instance.json`。
- 子进程只继承 NUL 输入与 `stdout.log`、`stderr.log` 两个文件句柄，不继承调用方的管道；后台或被捕获输出时，脚本在实例就绪后即可返回，实例继续运行。
- 就绪标准：在 `127.0.0.1` 与 `[::1]` 两个回环地址上轮询 `/json/list`，出现且只出现主窗口页面、资料准备窗口已关闭，最长 120 s。主窗口所在地址写入 `instance.json` 的 `cdp`，各驱动脚本默认从清单读取（`--cdp` 可覆盖）。程序提前退出或超时会附带 `stderr.log` 末尾 20 行。
- `instance.json` 字段：`pid`、`executable`、`identifier`、`dataRoot`、`debugPort`、`cdp`、`stdout`、`stderr`、`startedAt`，以及可选 `runId`。外壳在运行器异常退出后仅清理清单中 `runId` 属于本次运行的实例，避免清理前次或其他任务的实例；关闭实例时脚本按 PID、可执行文件路径与启动时间三项核对身份，不一致不结束进程。

**已知限制**：

- 资料目录长度：程序启动时校验主窗口的 WebView2 资料目录 `<资料目录>\webview\<SID>` 不超过 229 个字符；资料目录尚未初始化（没有 `storage-layout.json`、需要先建资料准备窗口）时，还校验准备窗口的 `<资料目录>\migration\bootstrap\webview\<SID>` 不超过 237 个字符。超限直接报错退出（隐藏窗口的测试实例写 `stderr.log` 并以退出码 1 结束）。SID 一般 46 个字符时，全新资料目录应不超过 162 个字符，已初始化的不超过 174 个字符。主检出默认约 107 个字符；`.claude/worktrees/<名称>/` 工作树约 143 个字符，仍可用，但更深的路径需要用 `-DataRoot`（例如 `C:\gs-tmp\<名称>`）。
- 全新资料目录启动时先有资料准备窗口，它与主窗口是两个 WebView2 浏览器进程：先启动的占 `127.0.0.1:<端口>`，主窗口回落到 `[::1]:<端口>`；已初始化目录重启时主窗口在 `127.0.0.1`。不要自己拼 `localhost:<端口>` 连接，使用清单里的 `cdp`。
- 生产落盘入口从构建前到最终清理持有同 profile 互斥锁，冲突直接失败，防止覆盖资料和产物；不同工作线并行时仍须使用不同端口与 `-DataRoot`。
- 机器满载时，调试构建、虚拟 PLC 心跳与布防可能超时。框架不自动重跑整条测试，也不修改产品节拍门槛；历史负载实验仍保留在下文。
- 端到端都是隐藏窗口，截图来自 WebView2 页面；结论只覆盖业务接线、落盘与界面驱动，边界见各节。

### Windows CI 与手动验收

Windows PR CI 恢复锁定的 lyFlow 运行库并运行 `pnpm test:ui:production -Suite burst -ResetData`：使用全新隔离资料与合成图，不需要现场样本集。桌面构建已包含 `pnpm build` 对应的 TypeScript / Vite 生产构建，因此 CI 不重复运行独立前端生产构建步骤；类型检查、Vitest、Rust 和脚本契约检查继续保留。

CI 无论结果都上传 HTML、JUnit、构建日志及启动 stdout/stderr；失败时额外上传 `runner/test-results/` 下的截图和 trace，不上传可执行文件、资料目录或整个业务输出目录。合成图段配方通过不代表相机、PLC 和现场节拍验收。现场样本集桌面 E2E 已删除，不再列为手动入口；长稳与硬件专项保持原范围。下文历史通过记录属于当时脚本和版本，不能作为 Playwright 迁移后的通过证据。

### 本次 Playwright 迁移验证（2026-10-23）

以下为 Windows 本地实际运行结果，未将历史脚本证据计作迁移后的通过依据：

| 检查 | 结果与保护范围 |
| --- | --- |
| 无 lyFlow，默认 `-Suite all` | 1 passed、1 skipped，33.9 s；旧配方生产落盘通过，段配方明确跳过 |
| 真实 lyFlow 1.1.5，`-Suite all` | 最终重跑 2 passed，89.3 s；旧配方与合成图段配方分别执行 |
| 无效 DLL，`-Suite burst` | 按预期失败，退出码 1；生成 `failure.png` 与 trace，未误报跳过或通过 |
| `pnpm test:ui:production -Suite legacy -ResetData`（含构建） | 1 passed、1 skipped，40.1 s；与 CI 冒烟使用相同命令，全新资料且不需要样本集或 lyFlow |
| 基础设施与既有长稳断言 | `node --test tests/native/desktop-infrastructure.test.mjs tests/native/throughput-gate.test.mjs tests/native/soak-faults-assess.test.mjs`：21 passed（3 条基础设施保护、18 条既有断言），涵盖显式坏库预检、同 profile 互斥及释放、进程身份不符时不终止其他进程 |
| `pnpm typecheck` | 通过 |
| 相关前端用例、锁文件 | `pnpm test tests/fixture-validation.test.tsx`：1 passed；`pnpm install --frozen-lockfile` 成功 |

这些结果不代表完整长稳、真实相机 / PLC、打包或算法准确率验证。

## 段配方端到端验证（2026-10-23，E2）

> 历史记录：本节涉及的现场样本集桌面测试及入口已于 2026-10-23 删除；原命令、参数与结果仅记录当时流程，不构成当前运行指引或迁移后通过证据。

环境：Windows 11、i5-1135G7（4 核 8 线程）、15.7 GB，调试构建（gluesight opt-level 2），lyFlow 1.1.5（`src-tauri/target/dependency-cache/lyflow/1.1.5`），样本集 `2026-10-17-normal-sampleset`。各项顺序独立运行，不并行。

| 入口 | 结果 |
| --- | --- |
| `pnpm test:ui:production`（无 lyFlow） | 通过，35.7 s：旧拍照点配方一段通过，段配方一段跳过（`burst.skipped` 写明需要 lyFlow） |
| `pnpm test:ui:production -LyFlowRuntime …` | 通过，99.8 s：旧配方一段 + 段配方一段（`FLOW-BURST`，12 个帧组 × 3 台相机 = 36 行 part_frames 与 36 张按帧命名的原图逐一核对） |
| `pnpm test:e2e:offline -SampleSet … -LyFlowRuntime …`（burst） | 通过，220 s（含重启续测 7.8 s）：合成图 10 轮标定帧组数 13、10、12、13、14、10、13、12、13、11 → ΣN_exp 12、建议容差 ±2；样本集冒烟验证通过并发布，帧状态 36 帧全部测成、27 帧沿走廊找不到胶（75%）、0 个不完整组，原包重现与原结论一致 |
| `pnpm test:e2e:log-replay …`（burst） | 通过，133 s：3 件的时间线与运行日志一致、原包重跑按区域逐一一致、删一张原图后原包重跑把该帧组标检测不完整（结论不变）、界面回放与诊断包 |
| `pnpm test:e2e:offline -Model legacy …`（旧拍照点配方） | 通过，139 s（含重启续测 9.2 s）：原流程不变，新建起点改用 `legacyTemplate` |
| `pnpm test:e2e:log-replay -Model legacy …` | 通过，117 s：3 件逐拍照点比对一致 |
| `pnpm test:e2e:fixture …`（旧拍照点配方，特征定位专项） | 通过，165 s：首次运行卡在第二个框，页面停在画框状态、按钮写“在原图上拖出定位框”，脚本改为已在画框状态时不再点按钮后通过；产品行为没有改 |
| `pnpm test:soak -Parts 30`（burst，默认阶段 soak、faults、engine、audit、restart） | 通过，约 8 分钟：见[长稳报告](soak-report.md)“2026-10-23 更新”。没有跑 1000 件与 `disk` 阶段 |
| `node --test tests/native/throughput-gate.test.mjs` | 通过（2026-10-23 第二次审查修复后）：15 个用例。基线一条（件间握手 0.9 s、使能窗口内满 3 × 25 Hz、分项都满足 → 达标且持续负载已验证）；每条规则一个：Unmeasured、预算丢帧、队列峰值一半的边界、测量失败、帧通道满、没有出件、产线中途停、使能窗口内帧率只有需求 88%、只有 9 件满负载、满负载被一件掉速隔断（最长连续 7 件）；另有队列峰值边界下侧、显式放行（记为按突发放行）、显式放行但分项不达标（不记为按突发放行）、中途停即使放行仍不达标。删行检验（本次实跑）：帧率改按含握手间隙的总时长算、去掉首个帧组不计、去掉连续件数条件、连续计数不归零、`burstOnlyAccepted` 去掉“分项满足”条件，各自让对应用例失败。上一轮记录的删行检验（队列峰值 `>=` 改 `>`、去掉中途停、去掉 Unmeasured 判定）对应的用例没有改。审查修复经过：第一次补上中途停与持续帧率两条（原来空转或只有突发的运行也能判达标）；第二次把持续帧率从含件间握手的平均值改成按使能窗口计——平均值衡量的是件间握手开销，离线虚拟 PLC 每件约 0.9 s 的握手让任何来源都达不到 |
| `node --test tests/native/soak-faults-assess.test.mjs` | 通过：3 个用例（长稳 `pulseJitter` 件帧组数低于 ΣN_exp − Σtol 并标疑似提前终止不算失败、帧组数在容差内却标疑似提前终止算失败、波动件结论变成 NG 算失败）；去掉下限条件（回到旧判定）、去掉标记核对、去掉结论核对，各自让对应用例失败（审查修复：旧判定把“波动件不应标疑似提前终止”当断言，样本集标定的容差只有 1，脉冲种子约 1/9 的概率随机失败；离线端到端的 `assertBurstPart` 同样改为波动件只核对标记与帧组数一致，未重跑端到端） |

3 × 25 Hz 吞吐（`pnpm test:soak:throughput -SampleSet … -LyFlowRuntime …`，25 Hz、60 s）：

| 来源 | 结果 |
| --- | --- |
| 样本集（真实 lyFlow 测量） | **持续负载没有按使能窗口口径判定过**（这一行的数字来自审查修复前的判定，没有重跑；那时报告里还没有 `enabledFps`、`sustainedParts`、`handshakeGapSeconds`）：含件间握手的平均 25.9 帧/s（旧口径，现只作参考），每件到达速率（含首个帧组）中位数 78 帧/s；分项在突发下没出问题：43 件 1548 帧全部测成（Unmeasured 0、测量失败 0、缺帧 0），图像内存预算丢帧 0（离线恒为 0，见下）、帧通道满丢帧 0，测量队列峰值 29 / 1024（2.8%，采样下界）；引擎耗时 p50 62 ms / p99 254 ms，排队 p50 129 ms / p99 705 ms；主进程私有内存 100 → 195 MB。**每件是一次突发**：样本集每件只有 12 个脉冲（25 Hz 下使能约 0.48 s），42.2 件/min 即每件约 1.42 s，件间握手约 0.9 s，测量队列在间隙里排空，所以队列峰值只反映每件 12 个帧组的积压。按每件第一帧到达到最后一帧测完得出的突发排空速率中位数 49 帧/s，低于需求 75 帧/s，外推持续 3 × 25 Hz 约 40 s 装满 1024 的测量队列（约 20 s 就超过 50%） |
| 合成图（模拟测量） | 不达标，但不是产品管线的结论：26 件 885 帧，Unmeasured 12、缺帧 46（逐像素渲染赶不上脉冲，帧晚于 partEnd 才到），13 件标检测不完整、4 件帧数异常并标疑似提前终止；帧到达速率约 34 帧/s（渲染上限），测量队列峰值 7，CPU 100%。合成图的生产测量恒为模拟测量，lyFlow 没有参与，这一项量的是合成图渲染能力 |

结论：
- **门槛状态：没有达标，也没有被直接验证，已定（2026-10-23）：门槛口径不改，等现场工控机用真实相机来源跑 `pnpm test:soak:throughput` 后再决定降低相机帧率还是加测量并发，合并前须完成该复测。**按计划原文“合成图 3 × 25 Hz 在线连续 60 s（且 lyFlow 测量）”做不到——合成图出的帧不经 lyFlow；离线下能压 lyFlow 的只有样本集，而它只能做突发。样本集上突发下的三条分项没出问题，但持续满负载**没有达标也没有直接验证**（上表数字来自第一次审查修复前的判定函数，当时样本集记为通过；第一次修复改按含件间握手的平均帧率判，任何离线来源都恒不达标；第二次修复改按使能窗口计，数字没有按新口径重跑）。按新口径，样本集只要使能窗口内到帧跟得上 3 × 25 Hz、连续 10 件以上，持续负载就会判为已验证，但件间握手期间测量队列排空，“队列峰值低于 50%”只说明每件 12 个帧组的积压不大，不等于不间断 75 帧/s 下成立；外推显示本机（i5-1135G7、调试构建）持续 75 帧/s 不可达：测量排空约 49 帧/s（P1 探针单测量在 4 线程下 101–112 帧/s，应用里与读图、录制写盘、SQLite、界面同时跑时低一半）。按决策记录 7.1 的降级公式，可持续帧率 = 0.7 × 49 ≈ 34 帧/s，即约 11 Hz；是否降 PLC 脉冲频率（约 11 Hz）、加测量并发（本机 4 个物理核，4 → 6 线程增益有限）或只在现场机真机相机来源上核对，由用户决定，现场工控机必须重跑本压测。与 P1 回填值对照（决策记录 7.1：串行 p95 26.4–30.3 ms → `MEASURE_WORKERS = 4`，4 线程测量吞吐 101–112 fps，写盘 154–209 张/s、最长停顿 4.7 s）：应用里测到的突发排空速率 49 帧/s 只有 P1 的一半，原因没有查。
- 图像内存预算（`IMAGE_BUDGET_PER_CAMERA`）只作用于海康回调来源，合成图与样本集来源不经它，所以离线下“预算丢帧 = 0”恒成立、证明不了预算够用；这一项只能在真机相机来源上核对（相机状态 `budgetDroppedFrames`）。
- 测量队列峰值是运行中每 50 ms 读一次在途件 `pendingMeasure` 的最大值，是采样下界。
- 实测值：`output/soak/throughput-sampleSet-25hz.json`、`output/soak/throughput-synthetic-25hz.json`。

## 离线端到端

> 历史记录：本节涉及的现场样本集桌面测试及入口已于 2026-10-23 删除；原命令、参数与结果仅记录当时流程，不构成当前运行指引或迁移后通过证据。

**段配方（`-Model burst`，默认，E2）。** 先合成图主流程，再样本集冒烟，两段共用 `tests/native/lib.mjs` 的 `buildBurstRecipe`（走廊示教 `teachCorridors`、多轮标定 `calibrate`、独立样本验证、发布）：

1. 合成图三相机（进入离线·合成图，脉冲 5 Hz）：新建段配方（`recipe_template`：三台相机、一个直线段）→ 一轮整圈采集选示教帧 → 每台相机一条走廊（`workspace_teach_camera` 冻结示教帧、`workspace_extract_corridor` 刷选提取、再保存当量与检测参数）→ 10 轮多轮标定（合成图每轮首尾 ±1、弯道 ±2 帧的波动，所以 min / max 不同；另加 2 轮标准件给胶偏参考 x_ref）→ 应用标定并确认建议容差（`max(默认, N − min, max − N)`）→ 新采一轮作独立验证样本 → 发布（`validationSite = offline`）。断言：N_exp 等于各轮帧组数均值四舍五入、标定统计只含 10 轮、建议容差 ≥ 1 且已写入配方、x_ref 已得出、导出给 PLC 的 shotCount = ΣN_exp 且协议版本为 6。
2. 投产 4 件：正常件、单台相机缺帧（`cameraLost`：该组标检测不完整、OK/NG 不变）、首尾 ±1 脉冲波动（`pulseJitter`：总数最多偏离名义 2 个，是否越出 ΣN_exp ± Σtol 取决于标定轮次观察到的范围，所以只核对帧数异常与疑似提前终止标记和帧组数一致、OK/NG 不变）、断胶（`gap` → NG_GAP，PLC 码 13）。每件断言帧组数、各相机帧数、缺帧、整件标记（未分段、胶偏未判、检测不完整；波动件之外无帧数异常与疑似提前终止）与未检项（气泡、拉丝），再核对历史列表行、CSV 导出末尾五列（ΣN_exp、Σtol、各相机帧数、deviceTriggers、标记）与历史详情。
3. 退出离线并核对 PLC 与相机组配置恢复，关闭实例后保留数据目录重启，`offline-restart.mjs` 核对：合成图配方从磁盘恢复且真机仍拒绝离线验证发布的版本、再跑一件、修订与 ΣN_exp 不变。
4. 样本集冒烟（2026-10-17 停稳拍数据，`lyFlow` 测量）：同一流程走一遍。每台相机一条走廊对不上其他点位，帧里多半找不到胶，检测项开着只会判 NG_GAP、验证过不了发布，所以建站时把各段检测项全关（`checksOff`），只断言流程不崩、记录自洽、原包重现与原结论一致（`workspace_compare_original`，区域级），并把帧状态比例（测成、找到胶、缺帧）写进 `result.json` 的 `smoke`，不作门槛；重启续测里发布包按原身份预热、同一样本件的结论、发布包与帧状态分布不变。

历史 E2 实测中，合成图逐像素渲染的吞吐不足，曾使用 5 Hz，并允许尾部两个帧组缺帧；当时存在依赖 `deviceTriggers` 提前排空的判断。当前实现收到 partEnd 后等待完整排空窗口，不再使用该计数提前结束；上述历史容差不证明当前尾帧排空行为通过，须以本轮验证结果为准。

**旧拍照点配方历史测试记录（对应入口已移除）。** 下面各段描述的就是它。

`pnpm test:e2e:offline` 由 `scripts/Test-OfflineE2E.ps1` 驱动 `tests/native/offline-e2e.mjs`，实例配置为 e2e（`Start-UiTestInstance.ps1` 生成），应用标识 `com.xyzrobotics.tujiaovision.e2e-tests`（隐藏窗口），报告与截图输出到 `output/playwright/e2e-regression`（`result.json`、`restart.json`、`offline-e2e-history.png`）。第一段通过后，脚本关闭实例、保留数据目录重启同一实例，再由 `tests/native/offline-restart.mjs` 做重启续测。已构建时加 `-SkipBuild` 直接跑；端口冲突传 `-DebugPort`。共享 Cargo 缓存时设置 `CARGO_TARGET_DIR`，脚本从该目录取构建产物。

| 输入 | 环境变量 | 脚本参数（覆盖环境变量） | 默认值 |
| --- | --- | --- | --- |
| 样本集目录 | `GLUESIGHT_SAMPLE_SET` | `Test-OfflineE2E.ps1 -SampleSet`，`offline-e2e.mjs --sample-set` | 无，必须提供 |
| lyFlow 运行库目录 | `GLUESIGHT_LYFLOW_RUNTIME` | `Test-OfflineE2E.ps1 -LyFlowRuntime`，`offline-e2e.mjs --lyflow-runtime` | 自动恢复 `packaging/runtime.json` 系列最新 Release |

样本集目录必须通过环境变量或参数提供，脚本不内置任何本机路径；lyFlow 运行库未提供时由 `Restore-LyFlowRuntime.ps1` 恢复，并通过 `GLUESIGHT_LYFLOW_RUNTIME` 传给 debug 构建。任一输入缺失或不存在时不启动实例，打印缺失项并以退出码 3 结束，不算通过。样本集目录应放在本地非同步目录（不能是 junction、符号链接或 OneDrive 等 reparse point）。样本集原件只读：每次运行把它复制到输出目录的 `sample-set/`（复制前解析真实路径，副本目录与原件相同或互相包含时直接报错，不删除任何内容），按样本集 id 选取 `tests/native/lib.mjs` 中 `FIXTURES` 里的夹具，没有夹具的样本集直接报错。当前夹具为 `gs-20261017-normal`（2026-10-17 现场单圈，12 点位 × 3 相机）：副本 id 加 `-e2e` 后缀，全局 `mmPerPx` 写入模拟标定当量 0.112（样本集未标定，沿用 [P0 计划](../architecture/p0-plan.md) 记录的同分辨率现场相机假设当量，不代表真实标定）；样本集只有正常件，副本里由 p01 复制出 p02，把 S011 组所选图（样本 S032）的一段约 11 mm 胶路逐行抹成左右紧邻像素的线性渐变，作为人工断胶件（`label=ng`；早先换成旁边板面像素的做法在 lyFlow 1.1.4 下会被测成胶而漏判，见 [r2-lyflow-upgrade.md](../decisions/r2-lyflow-upgrade.md) L-2）。离线与重启续测都用这份副本；重启续测按 `site_status().last.sampleSetDir` 进入，即首次进入时导入的受管副本。每次运行前只删除资料根目录（默认为输出目录，见 `-DataRoot`）下以隔离标识命名的数据目录（`GLUESIGHT_DATA_DIR` 指向它；名称、父目录和非链接逐项校验），保证从全新站点开始，覆盖首次建站无已发布配方的场景。实例启动、CDP 地址与资料目录长度的约定见上文“端到端入口”。

**覆盖范围**：真机模式启动 → lyFlow 引擎就绪 → 进入离线·样本集（离线控制台显示“当前：离线 · 样本集”，未发布前提示尚无可运行的生产配方；离线相机组按样本集 3 台相机生成）→ 新建候选、离线产线以全部 3 台相机送一圈 p01（12 次触发 × 3 相机 = 36 个拍照点）并采用 → 每个拍照点组（Pose）按夹具选一台相机的图作为检测图，核对候选采用了样本集声明当量，用刷选笔划（S006 组用框选）提取中线、保存参数（4 个组用 3 mm 窄搜索）、试测为合格类、保存示教 → 进度模型的标定步骤完成，所有检测图的标定依据为样本集声明当量，全程不调用人工标定验证 → 独立采集 p01（期望合格）与 p02（期望 NG_GAP）作验证样本 → 验证通过，p01 的 OK 或 OK_WITH_EXCURSION 都按合格类判为符合 → 发布生效（发布状态 active，`validationSite=offline`），离线控制台默认选中新配方 → 离线产线投产 p01、p02 → p01 为合格类，p02 为 NG_GAP 且原因指向 `S011-cam2` → 历史记录带样本件引用 → 离线控制台“最近样本件”期望/实际与历史列表样本徽标、结论的界面核对 → 历史页导出 CSV（行数与记录数一致）→ 在 p02 的历史详情界面依次按原发布包重现（发布包与配方修订为本件冻结值）、按候选规则重判、从原图复测整件，三者结论均与原结论一致，已保存对照可切换查看，原始记录与候选不变 → 界面退出离线，PLC 配置与相机组恢复 → 重启同一实例：仍为真机模式，历史与生产配方修订、示教身份不变，该配方在真机就绪检查中不可运行（原因为离线环境验证发布、需在真机重新验证并发布，引导到工作台），进入离线后原发布包预热就绪且该配方变为可运行，再投产 p02 仍按原发布包得到同一结论。业务步骤主要经 IPC 驱动；PLC 事务占用、发布资源预热、离线 S7 服务重连（界面在重连期间禁用“运行一件”）等暂时拒绝按界面重试的方式最多重试 60 s，次数记在 `result.json` 的 `gateRetries`。

**结果字段**：`branches.calibration` 固定为 `sampleSetDeclared`；`branches.normalExpectation` 记录合格样本的实际结论是 `ok` 还是 `okClass`（OK_WITH_EXCURSION），两者都判为符合；`replayPart` 为做历史复测与重启续测的样本件（有断胶件时为它）；`replay.*.via` 均为 `ui`，三种复测都经历史详情界面触发（“使用该配方候选”需在确认框中切换）。

**边界**：每组所选相机、笔划、像素当量、窄搜索参数与人工断胶件都是让流程跑通的演示夹具，集中定义在 `tests/native/lib.mjs` 的 `FIXTURES`；通过只说明建站到投产的业务接线、多相机拍照点组示教、样本件追溯与原图复测一致，不代表中线提取或检测算法质量、现场标定精度、真实缺陷检出、现场 PLC/SDK 或节拍。mx11-demo 在每组必须选一张检测图的规则下需示教全部 8 个点，其中 S003、S006、S007 尚无可靠笔划夹具，暂不提供其夹具。

## 特征定位端到端历史记录

以下为旧拍照点专属特征定位端到端历史记录。对应入口、驱动脚本与 `legacyTemplate` 不再作为当前测试路径维护，不可用这些历史结果声明本轮通过。

`pnpm test:e2e:fixture` 由 `scripts/Test-FixtureE2E.ps1` 驱动 `tests/native/fixture-teach-e2e.mjs`，与离线端到端共用 e2e 配置、实例与样本集副本做法（夹具见 `tests/native/lib.mjs` 的 `FIXTURES`），不能同时运行。lyFlow 运行库必须支持带外圈模板的 `glue.locate`（本地构建用 `-LyFlowRuntime` 指过去，脚本只在本进程设环境变量）。

流程：12 组中线示教、p01 / p02 两圈样本进样本库走 IPC；拍照点组 S001（cam2）的定位特征步骤走界面：在示教图上拖框（屏幕取整误差用数值框改到夹具值）；压在中线上的框标“框未采用”、原因指向走廊、不进候选；高光框（≥ 215 的像素约 97%）进草稿但亮像素阻断，试测无效、不能保存；好框 `[248, 248, 96, 96]` 试测有效（示教图自定位偏移 ≤ 0.5 px），定位试测表里示教图与两帧样本都被采用；勾选喷嘴确认后保存。验证（IPC）要求“特征定位”检查与亮度 ±40 锁定通过、给出可采用的两端参数建议；在验证页面板上“采用建议…”并确认后，该视图要求重新试测、旧验证不能发布；示教页重新试测保存、再验证、发布，投产 p01 为合格类、p02 为断胶，p01 的记录带被采用的位姿；投产后在检测页点开该拍照点，选中帧按该帧记录的位姿画定位框（“定位框（按位姿）”，位姿没被采用时为“定位未采用”）并显示定位摘要，历史页打开 p01 的这一帧同样按记录位姿叠图（G5 E2，定稿 §14）。接着演练连续定位失败告警（G5 E3）：副本里另有 p03（由 p01 复制，S002_cam2 上好框连同外圈与整个搜索范围 `[180, 180, 232, 232]` 被逐行抹平，胶路不动；样本集来源不接受合成的“定位失败”工况，所以用真 glue.locate 的真实失败），连续投产 3 件，每件 ERR 92、只有这一幅图没有位置基准、位姿不被采用，前两件后没有告警，第 3 件后快照与检测页出现“连续定位失败告警”（横幅写明配方与修订，当前配方不标“当前未在生产”）、应用日志有本件的“连续定位失败”，选中帧画“定位未采用”框、摘要为“未采用”，该拍照点缩略图的中线落在预览格子内；再投产一件 p01，定位成功、告警消失、日志有“定位恢复”。截图 `fixture-1`–`fixture-9` 与 `result.json` 在输出目录。

**边界**：三个框是按 S002_cam2 手工选的演示夹具，不代表现场选框；p03 的定位失败是人为抹平特征造成的，只证明告警与叠图在真引擎上的接线，不代表现场失效形态；离线副本的验证样本与示教是同一件的同一批图，σ、两端 |Δ| 与定位试测偏移都接近 0，统计好看不代表现场。

## 站点模式（真机 / 离线）

软件启动总是真机模式：测量用 lyFlow，相机只接海康 MVS，PLC 只支持 S7。“总览”首页显示当前模式“真机”“离线 · 样本集 <名称>”或“离线 · 合成图”，点“查看站点模式”或侧栏“工艺 → 离线模式”进入离线控制台（`/offline`），控制台标题为“当前：真机”或“当前：离线 · <来源>”。

进入离线时软件临时派生并应用：PLC 切到进程内 S7 服务（127.0.0.1 随机端口、DB4/DB2 一期点表、PDU 协商 240）；相机组按样本集 `cameras[]` 生成样本集相机，或按真实相机组同编号、同视角数生成模拟相机（单视角相机不足 3 台时补足到 3 台，供段配方使用）；测量用 lyFlow（样本集）或模拟测量（合成图）；帧录制临时为“全部”。真实 PLC、相机组和系统设置不改写，退出离线后全部恢复；恢复真机 PLC 配置失败时 PLC 保持断开并停在安全配置，离线控制台与日志标为“故障·需人工修复”。进入离线失败回滚时按进入前状态重连真机 PLC。离线期间相机页、PLC 页只读；有在途事务或整圈采集进行中时不能切换模式。离线模式不持久化，重启回到真机，但会记住上次来源和样本集目录。

**离线 · 合成图跑一件**

1. 断开外部 PLC，打开离线控制台，来源选“合成图”，点“进入离线”。
2. 选离线配方，工况选“正常件”，点“运行一件”；在线检测页也有离线配方、运行一件和停止的紧凑控件。
3. 在线检测与历史记录（切到“离线件”）应出现同一 SN 的整件结果和原图。传输丢帧按 SDK 帧序号与相机帧数诊断；`deviceTriggers` 不参与告警或排空判断。“ACK 序号不匹配”进入握手故障。

**离线 · 样本集对照期望/实际**

格式契约见 [离线样本集格式 v1](../integration/sample-set-format.md)（只接受 schemaVersion 1）。样本集按 partId / shotId / cameraId / view 取图，缺图按缺帧处理；段配方把它解释为“每台相机按清单顺序的帧序列”（`Manifest::camera_frames`，第 seq 个脉冲取各相机的第 seq 帧）。标签只用于展示期望，不参与判定。示例：`<样本集根目录>\mx11-demo`（单相机 cam1、单视角，p02 断胶 S004–S007）。

1. 确认 lyFlow 引擎就绪（运行库位于程序目录 `runtime/lyflow/`，开发时由 `pnpm deps:lyflow:dev` 复制），否则拒绝进入并提示原因。
2. 离线控制台来源选“样本集”，点“选择样本集目录”，核对件数、拍照点数、缺图和标签分布，点“进入离线”。
3. 无已发布配方时：新建配方即段配方，整圈采集页点“开始接收”→“离线样本集送一圈”，再走相机走廊示教、多轮标定（每轮各送一圈）、独立样本验证与发布（流程同“离线端到端”的段配方段；样本集是停稳拍数据，检测项开着会判 NG_GAP，见该节说明）。旧拍照点配方不再支持。
4. 选离线配方和样本件（不选则按顺序循环），点“运行一件”或“连续运行”，用“本件后停止”/“立即停止”结束。样本集模式只提供握手类工况：正常件、传输丢帧 / 缺帧、运动超时（不发 partEnd）、ACK 序号不匹配，以及段配方的五个脉冲串工况（见上文“段模型专项”）。
5. 核对“最近样本件”表的期望 / 实际：mx11-demo 的 p02 应为 NG（S004–S007 断胶），其余件的实际结论应与样本集标签一致。

**生产记录隔离**

配方共享，生产记录按来源隔离：检测记录带来源（真机 / 离线·合成图 / 离线·样本集，库版本 8 起；当前库版本 9（段配方按帧落库，D-SB-29），旧库按 D-0 备份后重建；离线件 SN 形如 `OFFLINE-0001234567`，同一离线运行内唯一）。历史页默认只看真机件，“离线件”切换查看离线件，列表与详情标注离线来源；统计、批量重判与 CSV 导出（含“来源”列）只算当前范围，在线检测的今日统计跟随当前站点模式。离线原图写入数据目录下独立的 `records-offline`，按系统设置中的离线原图保留件数和容量上限（默认 300 件 / 10 GB）单独清理，保存后在下一次录制收尾时按新上限清理，任一超限均删除最旧原图，真机 `records` 只按系统设置清理，离线连续运行不会挤掉真机原图（`recorder` 的 `offline_retention_prunes_only_offline_recordings_and_never_real_raw_images`）。样本件认领只在离线期间生效，退出离线清空本次运行记录，同 partSn 的真机件不带样本引用。

**退出与历史复测**

1. 点“退出离线”，确认离线控制台标题回到“当前：真机”，PLC 配置、相机组和帧录制设置均为进入前的值。
2. 历史记录切到“离线件”，打开离线跑过的件，详情显示“离线样本 partId · SN · 期望 OK/NG”；因离线期间录制为全部，可直接原图复测或与候选配方对照。

## 端到端验证（2026-10-19，r2-test-infra）

> 历史记录：本节涉及的现场样本集桌面测试及入口已于 2026-10-23 删除；原命令、参数与结果仅记录当时流程，不构成当前运行指引或迁移后通过证据。

本节是旧拍照点配方在协议 4 下的运行记录（协议 5 于 2026-10-22 才引入），段模型下没有重跑，不作段配方的通过依据。

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

## 历史验证（2026-10-12，离线 S7 产线合并 P0 清理后）

本节是 2026-10-12 的历史运行记录（旧拍照点模型），其中“36 ignored”“S7 本机线协议 `--ignored`”等已不是现状（`plc_session` 用例已进默认测试，见“S7 一期握手”）。

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
