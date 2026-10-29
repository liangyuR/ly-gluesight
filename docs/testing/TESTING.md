# 测试说明与复现

本文说明各层测试的运行方法、环境要求、隔离实例约定与证据边界。用例及保护规则以测试源码为准，准入遵循 [AGENTS.md](../../AGENTS.md)：每个用例保护一条业务规则、对外契约、已修复缺陷或易错边界。修改时按影响范围筛选文件或模块，不以测试数量作为完成指标。

## 当前保留范围

默认 Rust 库回归保留 85 条、前端 Vitest 保留 84 条用例（各约 80 条），以测试运行器展开后的实际用例数计数。保留集优先覆盖结果交付、数据归属、发布准入、草稿防丢和资料安全；同一业务规则的等价输入使用参数化，跨层重复验证优先保留能观察业务结果的一层。增删用例按保护规则评估，数量是维护规模参考。

打包门禁、样本转换、桌面生产验收和现场长稳脚本独立维护，不计入默认两套回归。默认回归不覆盖所有日志展示与导出分页、兼容输入、起胶建议、视觉搜索重试与边缘统计、离线模拟产线完整路径；修改这些路径时需补充针对性的验证。

| 测试层 | 保留验证 |
| --- | --- |
| 前端 | 默认 Vitest 回归：段配方创建与草稿防丢、采集准入和独立样本、多轮标定与走廊示教、验证发布门槛、周期/帧归属隔离、历史区域重判、PLC 操作锁、维护模式、项目管理、单视角标定更新与配置安全 |
| Rust 默认 | `cargo test --lib`：当前段配方、帧组与采集、逐段标定、比例分段与区域判定、PLC 握手、调度、发布与恢复、项目导出导入与复制；包含少量真实 S7 TCP 回环、SQLite 和原图录制集成测试，不能把全部库测试称为纯单元测试 |
| S7 专项 | `plc_session` 的交付、复位、心跳及维护隔离用例（真实生产驱动 + 进程内 S7Server）已进默认 `cargo test`；离线模拟产线完整运行需专项验收 |
| 桌面端到端 | 仅合成图段配方生产落盘：flow 实例内建站、标定、发布、生产、SQLite、原图与历史读回，需要 lyFlow |
| 保留工具 | 打包契约、Windows 发布门禁、安装包自检、样本集转换脚本 |
| 手动专项 | 长稳测试（需样本集与 lyFlow 运行库，不进 CI）；吞吐探针 `throughput_probe`（见“3 × 25 Hz 吞吐门槛”） |

默认回归不加载 lyFlow DLL，不替代真实设备或现场节拍验收。

显式设置 `GLUESIGHT_DATA_DIR` 的隔离实例将日志写入 `<隔离资料目录>/gs_logs/`，不使用当前账号的 `~/gs_logs`。桌面与长稳脚本通过 `storage_status` 和 `config/log-storage.json` 核对资料归属、环境及命名空间，再访问运行日志和视觉原图；不能从原图目录推断项目数据库位置。隔离路径检查可单独运行 `node --test tests/native/storage-paths.test.mjs`。

## 运行

工具版本以 `package.json`、`rust-toolchain.toml` 和锁文件为准。首次安装执行 `pnpm install --frozen-lockfile`。应用 Vite 预构建仅扫描根目录 `index.html`，避免 `output/` 中的验收夹具或组件库示例引入其他 React 副本。

| 命令 | 用途 |
| --- | --- |
| `pnpm test` | 默认前端回归 |
| `pnpm test:watch` | 开发时持续运行相关前端测试 |
| `pnpm typecheck` | 应用及测试 TypeScript 类型检查 |
| `pnpm build` | 生产前端构建 |
| `cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib` | 默认后端回归，依赖需已缓存 |
| `pnpm test:desktop:build -Profile flow` | 只构建隔离桌面实例，不启动；profile 可选 ui（默认）、p0、flow、e2e、soak |
| `pnpm test:ui:production` | Playwright 自动构建、启动与关闭隔离实例，验证合成图三相机段配方建站与生产落盘，需要 lyFlow |
| `pnpm samples:test` | 样本集转换脚本（`scripts/sample-set/convert.py`）测试，只需 Python 3 标准库 |
| `pwsh -NoProfile -File tests/packaging/Test-RuntimeBundle.ps1` | 打包版本、ABI、源码修订及运行库依赖契约 |
| `pwsh -NoProfile -File tests/packaging/Test-LyFlowDependency.ps1` | lyFlow 依赖的 Release 选择、配置与 Cargo 锁定、运行库校验、缓存与网络回退、升级回滚（网络与 DLL 探针用本地夹具） |
| `pwsh -NoProfile -File tests/packaging/Test-VendorDependency.ps1` | 厂商运行库的包隔离、结构、尺寸、CRT 版本、固定源恢复与缓存边界 |
| `node --test tests/packaging/windows-workflow.test.mjs` | Windows 打包工作流门禁：版本与安装方式校验、发布标签须在 main 上、版本设置、SDK 干净环境检查、视觉检查先于打包、依赖缓存标识与发布清单 |
| `pnpm test:soak:throughput`（即 `Test-Soak.ps1 -Stages throughput`） | 3 × 25 Hz 吞吐门槛，判定口径见下表后说明 |
| `pnpm test:soak`（即 `pwsh -NoProfile -File scripts/Test-Soak.ps1`） | 长稳测试（隔离 soak 实例）。没有必填参数，但样本集与 lyFlow 运行库必须经 `-SampleSet` / `GLUESIGHT_SAMPLE_SET`、`-LyFlowRuntime` / `GLUESIGHT_LYFLOW_RUNTIME` 提供（不自动恢复运行库，缺失时退出码 3），例如 `pnpm test:soak -SampleSet <样本集目录> -LyFlowRuntime <运行库目录> -Parts 1000`；阶段、参数与退出码见[长稳运行说明](soak-report.md) |
| `pwsh -NoProfile -File scripts/Test-DevStartup.ps1` | Windows 旧终端在设置/清除本地运行库后启动，子进程读取最新保存值，参数与退出码保持不变 |

单文件前端测试示例：`pnpm exec vitest run tests/teaching.test.tsx`；Rust 模块示例：`cargo test --manifest-path src-tauri/Cargo.toml --locked --lib segmentation::`。默认测试不生成覆盖率报告。

`Test-DevStartup.ps1` 临时修改用户级 `GLUESIGHT_LYFLOW_RUNTIME`，在 `finally` 中恢复原值；验证期间不要并行修改该配置或启动 GlueSight。测试通过临时 CLI 进程观察启动环境，不启动实际 GUI，不修改配方。

**吞吐门槛判定**（`pnpm test:soak:throughput`）：段配方、离线·样本集 lyFlow 测量（`-ThroughputSource synthetic` 压合成图帧管线），连续跑 `-ThroughputSeconds`（默认 60 s），核对 Unmeasured = 0、图像内存预算丢帧 = 0、测量队列峰值低于容量的 50%，并要求产线跑满时长、持续负载已验证。持续负载按相机使能为高的时间窗计：PC 看不到使能信号，窗口取每件第一帧到达到最后一帧到达（运行日志“帧到达”，首个帧组不计入，栅栏计数），Σ窗口内帧数 ÷ Σ使能时长 ≥ 3 × Hz 的 90%，且至少连续 `-ThroughputMinParts`（默认 10）件逐件达到；件间握手间隙单独报告为 `achieved.handshakeGapSeconds`，不参与判定，含握手的平均帧率 `achieved.framesPerSecond` 只作参考。达不到时退出码 1，实测值写进 `output/soak/throughput-*.json`。持续负载未验证时默认判不达标；只想看分项时加 `-AllowBurstOnly` 放行，报告里 `sustainedVerified` 仍为 false、`burstOnlyAccepted` 为 true，不是持续门槛的结论（分项不达标时 `burstOnlyAccepted` 为 false）。样本集每件只有 12 个脉冲，件间握手期间测量队列排空，队列峰值只反映每件的积压，不间断 3 × Hz 下的积压看 `projection` 的外推。判定函数的规则由手动命令 `node --test tests/native/throughput-gate.test.mjs tests/native/soak-faults-assess.test.mjs` 保护，不在普通 PR CI 中执行。

Windows 受限环境遇到 Node realpath EPERM、回环连接被拒等权限失败时，在获准的主机环境运行相同命令，不修改应用逻辑掩盖环境失败。Cargo 首次构建需要 MSVC、Visual Studio C++ 工具及私有 Git 依赖读取权限；离线执行要求依赖已缓存。

## Windows CI 与手动验收

Windows PR CI 仅执行 Format 检测、前端单元测试（`pnpm test`）和 Rust 默认库回归（`cargo test --locked --lib`）。Format 用 rustfmt 解析变更的 Rust 文件并生成格式结果，仅检查与本次新增、修改行相交的格式差异；新文件全检，纯删除文件中的测试时仍解析保留代码，无 Rust 变更则跳过。所有变更文件另用 `git diff --check` 检查空白。PR 比较 base SHA 与当前提交，main 推送比较 before 与当前提交，手动运行比较当前提交与上一提交。存量 Rust 尚未全仓格式化，因此使用增量格式检查，避免无关源码重排；项目尚未配置前端格式化工具。

本地使用 `node scripts/Check-RustFormat.mjs --base main` 检查相对 main 的已跟踪文件变更；加 `--head <commit>` 可检查指定提交快照。脚本只读，需先为项目工具链安装 rustfmt（`rustup component add rustfmt`）。

类型检查、样本转换、打包与长稳脚本检查、桌面生产冒烟均不在普通 PR CI 中执行，CI 不恢复 lyFlow 运行库、不构建桌面应用。推送 main 和手动运行 Windows CI 使用同样的轻量验证；Windows 发包工作流仅由手动运行或 `v*` tag 触发，保留发布门禁。

桌面 E2E 保留手动命令 `pnpm test:ui:production -Suite burst -ResetData`，使用全新隔离资料与合成图，需要 lyFlow；HTML、JUnit、构建与启动日志、失败截图和 trace 保存在本地输出目录。合成图段配方通过不代表相机、PLC 和现场节拍验收。

## S7 生产握手

协议与点表见 [PLC 通讯协议](../integration/plc-communication-protocol.md)。旧版本协议的 PLC 写入被拒绝。

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib -- plc_session
```

`plc_session` 的默认用例由进程内 S7Server（`src-tauri/src/offline/s7_server.rs`，只监听 127.0.0.1 的随机端口，含仅测试编译的定点故障注入）驱动真实的生产会话，已是默认测试。用例需要本机 TCP 回环权限：Windows 沙箱拒绝 localhost 连接（WinError 10013）时，在获准主机环境运行，不能跳过 TCP 后记为通过。各案例的状态、事务与 DB 快照写在测试临时目录（`s7-session-tests`），通过后自动清理，本机产物不提交。

## 段模型（分段连拍飞拍）专项

段模型的测试按 AGENTS.md 准入编写：每个用例对应一条业务规则、对外契约或易错边界，名称写成“条件 → 预期结果”。Line0 高电平门控，帧率由软件设置、默认 10 fps；记录中的 `deviceTriggers` 为预留字段，不参与采集计数、告警或排空判断。规则取舍见 [分段连拍决策记录](../decisions/segment-burst.md)。

**分段：比例分段（D-SB-40）。** `segmentation::Proportional` 按各段 N_exp 比例与本件帧组数推算边界，单段或缺少逐段标定时返回未分段结果；`resolve_regions` 在可信边界两侧切出 `boundaryGuard` 帧过渡区。图像运动细化（D-SB-43）未实现。单元与生产节拍测试只验证比例公式、可信度、过渡区与判定规则，**不证明现场边界准确**；边界准确性用现场已确认手动分段做留一离线评估，它不是测试、不进 `cargo test`：

```powershell
cargo run --manifest-path src-tauri/Cargo.toml --example segment_eval -- <项目目录> <候选编号>
```

逐帧定向与找边（D-SB-47、D-SB-48）的离线评估同样不进 `cargo test`，需真引擎；逐次结果写 `<输出目录>/runs.csv`，汇总打印到标准输出：

```powershell
$env:LYFLOW_CORE_DLL = "<运行库目录>\lyflow_core.dll"
cargo run --manifest-path src-tauri/Cargo.toml --release --example heading_eval -- <项目目录> <候选编号> <输出目录>
```

分段标定工作台（D-SB-44）的默认回归保留 `workspace::capture_review` 的计划修改后复核失效、屏蔽不压缩序号及与示教帧互斥；前端 `tests/workbench-draft.test.ts` 和 `tests/segment-workbench.test.tsx` 验证边界编辑与结构改变确认后一次提交。对齐复核保留 `frame_align::only_distinct_alignment_close_to_proportional_is_trusted`，验证无区分度画面和超出比例容差的结果不能自动确认；`tests/align-review.test.tsx` 验证只确认需复核边界、微调不越界及整件保存。伸缩、丢帧、重复帧下的现场对齐精度需通过下述离线评估验证。

读该候选 `workspace.json` 的段列表、`boundaryGuard` 与示教件、标定件已确认的手动分段，逐轮用其余轮次按多轮标定规则得出各段 N_exp，再比较比例边界与手动边界，并给出“按原帧号”的对照；比例评估不读图像，模板图像对齐评估读取采集原图，均不需要 lyFlow。至少要 2 段、2 个已确认轮次。评估结论需附对应数据版本与运行输出，本仓库不保存现场数据与临时验收证据。

起涂与收尾跳过建议（D-SB-50）属于专项验证范围：改动时应核对连续成形识别、多轮最大值聚合、参与轮次不足不给建议，以及界面采用建议只更新对应一头。现有轮次都是空跑，建议值只用合成数据验证过。

**离线工况。** 段配方在离线下按脉冲串运行：布防后以固定频率（默认 20 Hz）发脉冲，每个脉冲同时触发三台相机，脉冲发完报运动结束；名义帧数取各段 N_exp（待标定的段按段长 × 频率 ÷ 50 mm/s 估算，已有整件标定时缩放到 ΣN_exp）。下表工况在离线控制台与在线检测页的工况下拉里，样本集与合成图来源都能选，只改变脉冲与握手，不改图像内容。

| 工况（界面文字） | 做什么 | 预期 |
| --- | --- | --- |
| 单台相机缺帧（该组检测不完整） | 第 2 台相机丢第 min(3, 总数) 帧，相机照常计数 | 该帧组标“检测不完整”，丢帧告警，OK/NG 不变，不判 ERR |
| 脉冲数波动（首尾 ±1 帧） | 首尾各多发或少发 1 个脉冲 | 帧组数偏离名义最多 2 个；在 ΣN_exp ± Σtol 内不告警，越界只告警（帧数异常，低于下限还标疑似提前终止），OK/NG 不变 |
| 弯道减速（弯道帧数波动） | 每个弯道段帧数 ±2 | 同上 |
| 本件未检（机器人未得到触发许可） | 机器人照常涂胶，PLC 不提交请求 | PC 没有该件记录，离线运行记“本件未检” |
| 提前运动结束（疑似提前终止） | 发约一半、且少于 ΣN_exp − Σtol 个脉冲后报运动结束 | 用已收到的帧照常判，加 `short_part` 与“帧数异常”，不改 OK/NG |
| 传输丢帧 / 缺帧 | 三台相机的同一帧都丢 | 该帧组标检测不完整，不判 ERR；丢帧按 SDK 帧序号、相机间帧数及传输丢帧诊断 |

**合成图采集与标定。** 合成图由 `src-tauri/examples/capture_fixture.rs` 生成仿历史录制件（[示教件与标定件决策](../decisions/recipe-build-flow.md)“手动配置数据”）：按录制时生效的生产配方（采集版：型号、段、计划相机）用 `simimage::render_burst` 生成 N 件，每件在真机录制根下写一个与真机同格式的录制目录（原图 `{相机}_s{seq:06}.pgm`、`part.json` 带 `partMode: maintenanceCollect`、`devices`、`plc` 与来源标记 `fixture: {label}`，以及维护采集保留标记），并在项目记录库写一行维护采集记录（SN 前缀 `FIXTURE-`，记录库版本仍为 9）。生成的件出现在历史件选择器里，标“手动配置”，与真机件走同一选件、导入路径。记录引用采集版的修订号：先建候选并发布采集版，再生成数据（候选的版本号留给之后的正式版，用它写快照会让正式版投产时同修订号的真实记录因快照不同被拒）。记录库须已存在且为版本 9；同修订号已有不同快照时写入前即拒绝。命令为 `cargo run --manifest-path src-tauri/Cargo.toml --offline --locked --example capture_fixture -- --doc <采集版文档.json> --cameras <camera_rig_config.json> --db <资料根目录>/projects/<项目编号>/inspection.db --records <真机录制根> --parts N [--seed S] [--label L] [--sample-set <目录> --part <件>] [--onset H --onset-jitter J --tail T --tail-jitter J]`，输出每件的 `recordId`、`cycleId`、SN 等 JSON：默认按名义剖面出合成图（首尾 ±1、弯道 ±2 帧，种子 seed+i）；给出样本集时每件都取该件的样本集原图，供 soak 使用。合成图离线环境因此能选示教件、完成走廊示教、多轮标定、验证与发布（加入标定时每帧测量用 lyFlow 走廊测量，需要加载 lyFlow）。合成图相机沿用真实相机组的编号与视角数，单视角相机不足 3 台时补足到 3 台。样本集按“每台相机的帧序列”解释（见[离线样本集格式 v1](../integration/sample-set-format.md)），现有的 2026-10-17 单圈样本集是停稳拍数据，没有弯道、帧数没有波动，只作冒烟，不作分段或标定的验收依据。

### 3 × 25 Hz 吞吐门槛

三台相机各 25 Hz 同时出帧是 75 帧/s，1280 × 1024 单字节灰度每帧 1.31 MB，约 98 MB/s。P1 探针量测量耗时与 PGM 写盘速率；它是 `src-tauri/examples/throughput_probe.rs`，不是测试，不在 `cargo test` 里，写约 4 GB 临时文件、结束后自动删除，约 1 分钟，需要已发布的 lyFlow 运行库：

```powershell
$env:LYFLOW_CORE_DLL = 'src-tauri\target\dependency-cache\lyflow\<版本>\lyflow_core.dll'
cargo run --manifest-path src-tauri/Cargo.toml --example throughput_probe
```

门槛：现场机用探针得到串行 p95，代入 `ceil(75 fps × p95 / 0.7)` 得到所需测量线程数，不得超过 `min(4, 逻辑核 − 2)`（`MEASURE_WORKERS`）；压测中不得出现图像内存预算满或测量队列满（出现时该帧记缺帧或未测，该组标检测不完整，不会静默丢帧）。不满足时先降低相机设定帧率，各段 N_exp 随之变化，须重新标定并分配新的计划版本。完整数据与降级方案见决策记录 7.1。

**门槛状态：没有达标，也没有被直接验证。** 门槛口径不改，等现场工控机用真实相机来源跑 `pnpm test:soak:throughput` 后，再决定降低相机帧率还是加测量并发。开发笔记本上的实测只是参考，不能外推到现场工控机。证据边界：

- 离线下能压 lyFlow 的只有样本集，而样本集每件只有 12 个脉冲、件间握手期间测量队列排空，只能做突发；“队列峰值低于 50%”不等于不间断 75 帧/s 下成立。
- 合成图来源的生产测量恒为模拟测量，lyFlow 不参与，合成图压测量的是渲染能力，不是产品测量管线的结论。
- 图像内存预算（`IMAGE_BUDGET_PER_CAMERA`）只作用于海康回调来源，合成图与样本集来源不经它，所以离线下“预算丢帧 = 0”恒成立、证明不了预算够用；这一项只能在真机相机来源上核对（相机状态 `budgetDroppedFrames`）。
- 测量队列峰值是运行中每 50 ms 读一次在途件 `pendingMeasure` 的最大值，是采样下界。

## 现场验收

**JHEM131GC-XS 高电平门控（2026-10-24 现场验证）。** 用户确认已在现场验证 `FrameBurstStart + LevelHigh`：持续高电平期间按相机设定的 10 FPS 连续采集，拉低后停止新曝光。该型号按已支持设备处理，参数页和批量应用结果不再提示待验收；仍逐项写入并回读校验触发节点、帧率、BurstFrameCount 和触发缓存关闭，失败仍阻止相机就绪。只对 JHEM131GC-XS 放开 Line0 的 FrameBurstStart + LevelHigh，其他型号不增加 FrameBurstStart 门控回退。不将此结论推广至其他型号，也不替代机器人互锁或故障停止验收。

**待真机验收**（自动测试只覆盖配置分支与界面，不代表设备写入已实测）：

- 频闪：LineSource 仅提供 ExposureActive 时频闪配置写入、回读成功；参数写入不执行整组回滚。
- Chunk：设备没有 Chunk 节点时只显示一条不支持提示。
- Software 单帧：仅有 FrameBurstStart 的设备切换到 Software 后，回读 BurstFrameCount=1，每次手动命令只得到一帧；具有 FrameStart 的设备继续选择 FrameStart。
- TriggerDelay：Integer 节点写入整数微秒并回读一致，小数应明确失败；Float 节点继续接受浮点值。类型查询、写入或回读失败时相机不得就绪。
- 曝光与增益：按相机保存在项目相机配置 `cameras.json`，写入海康相机的时机为相机页点“保存曝光与增益”（已连接时立即写入）和相机打开、重连后关闭自动曝光与自动增益之后；写入失败则打开失败。合成图与样本集相机下发为空操作，实际写入 ExposureTime / Gain 须在台架用海康相机确认。
- 触发切换、重连与多相机行为。

## 隔离桌面与证据边界

`pnpm test:ui:production` 由 Playwright Test 驱动，使用 `tests/native/production-flow-ui.mjs` 与 flow 实例配置（`Start-UiTestInstance.ps1 -Profile flow`），应用标识为 `com.xyzrobotics.tujiaovision.flow-tests`，报告输出到 `output/playwright/flow-regression`。全新实例不安装默认配方夹具。

测试在“离线 · 合成图”下新建候选、发布采集版，用 `capture_fixture` 生成仿历史录制件，选示教件、确认分段后用真实 lyFlow 完成段配方走廊示教、选标定件多轮标定、独立样本验证与发布，经生产 S7 握手投产，核对 SQLite、逐帧原图和历史读回。使用 `-LyFlowRuntime <运行库目录>` 或 `GLUESIGHT_LYFLOW_RUNTIME` 提供运行库；未指定时恢复锁定版本，恢复或加载失败即失败，不以跳过代替验证。合成图来源补足三台单视角相机。结束时退出离线并关闭本次进程。

[`tests/native/`](../../tests/native/) 没有单独说明文件：生产落盘驱动脚本为 `production-flow-ui.mjs`；它与 `scripts/soak/` 共用的 IPC 客户端、实例连接、样本集副本、合成轮次写入（`createFixtureCandidate`）、示教与投产函数在 `tests/native/lib.mjs`；`desktop-infrastructure.test.mjs`、`throughput-gate.test.mjs`、`soak-faults-assess.test.mjs` 是桌面基础设施与长稳判定函数的手动 `node --test` 用例；`guard.js`、`plc-ui.js`、`p0-plc-bootstrap-ui.js`、`p0-single-instance-acceptance.mjs` 是在隔离实例页面上执行的原生抽查脚本，协议 7 下未重新实跑，使用前须按当前点表核对断言。实例配置不放在该目录，由 `Start-UiTestInstance.ps1` 按 Profile 生成（长稳测试用 soak）。恢复专项应使用独立实例和新输出目录，避免使用生产配置。

jsdom 测试证明页面动作、关键接口参数及失败恢复，不证明真实布局、像素解码或硬件通信。桌面模拟流程证明业务接线与落盘，不证明真实 PLC、SDK、引擎测量准确率、现场节拍或持续运行。

## 端到端入口

生产落盘入口检查参数后按 flow profile 构建一次（`-SkipBuild` 跳过），通过独立 Playwright Test 配置运行。夹具负责启动、读取清单、连接主窗口、采集证据和关闭隔离实例；成功返回 0，构建、启动或业务失败返回非零。显式提供的 lyFlow 目录无效时在构建及启动前失败。决策见 [r2-test-infra 决策记录](../decisions/r2-test-infra.md)。

公共构建入口为 `pnpm test:desktop:build -Profile <profile>`（`scripts/Build-UiTestInstance.ps1`）；`Start-UiTestInstance.ps1` 单独使用时仍默认构建。生产落盘命令由 PowerShell 检查输入、准备构建并调用 Playwright Test，夹具复用现有实例启动和身份校验。`Test-Soak.ps1` 继续通过 `scripts/E2E-Common.ps1` 使用共享能力。

Playwright Test 固定 `workers=1`、`fullyParallel=false`、`retries=0`，每个用例最多 15 分钟；业务已有的短暂拒绝重试保持不变。

业务 JSON、截图和实例日志保留原目录，框架输出独立放在 `output/playwright/<profile>-regression/runner/`：`html/` 是可浏览报告，`junit.xml` 是 CI 结果，`test-results/` 保存失败截图与 trace。CDP context 的 trace 由夹具显式管理；取证失败不会覆盖业务失败。构建、启动、断言错误分别保留原始原因和日志位置。

| 命令 | 复用的业务函数模块 | 实例配置（应用标识） | 默认端口 | 输出 |
| --- | --- | --- | --- | --- |
| `pnpm test:ui:production` | `tests/native/production-flow-ui.mjs` | flow（`…flow-tests`），保留资料 | 9339 | `output/playwright/flow-regression` |

参数跟在命令后，例如 `pnpm test:ui:production -Suite burst -DebugPort 9351 -SkipBuild`。

| 参数 / 环境变量 | 作用 |
| --- | --- |
| `-DebugPort` | WebView2 远程调试端口，`127.0.0.1` 与 `[::1]` 上都必须空闲，否则启动前报错 |
| `-SkipBuild` | 复用上次构建复制到输出目录的 `gluesight-ui-tests.exe`（需同一配置构建过，不保证与最新源码一致） |
| `-Grep` | 传给 Playwright Test 的用例名筛选 |
| `-Suite burst` | 仅段配方生产落盘，默认 burst |
| `-ResetData` | 默认 false，保留资料；开启时首启重置资料 |
| `-DataRoot` / `GLUESIGHT_TEST_DATA_ROOT` | 隔离资料目录的父目录，资料目录为 `<DataRoot>\<应用标识>`；默认在输出目录下 |
| `-LyFlowRuntime` / `GLUESIGHT_LYFLOW_RUNTIME` | 段配方的运行库；未指定时恢复锁定版本，恢复或加载失败即失败 |
| `CARGO_TARGET_DIR` | 共享或自定义 Cargo 构建目录时设置，脚本从这里取构建产物 |

**前置条件**：Windows 与 WebView2 Runtime；Node 与 pnpm 按 `package.json`，已 `pnpm install --frozen-lockfile`；Rust 工具链与私有 Git 依赖读取权限（构建时）。生产落盘使用合成图，无需现场样本集；段配方需要 lyFlow。

**隔离实例的启动**（`scripts/Start-UiTestInstance.ps1`，生产落盘夹具调用，也可单独启动后手动驱动）：

- Tauri 覆盖配置按 `-Profile`（`ui`、`p0`、`flow`、`e2e`、`soak`）在脚本内生成：产品名与窗口标题为 `GlueSight · 胶路智检 <Profile> Tests`，应用标识 `com.xyzrobotics.tujiaovision.<Profile>-tests`，窗口隐藏，不打安装包。由公共构建入口写到输出目录的 `tauri-test-config.json` 再交给 `tauri build --config`（包含 TypeScript 与 Vite 生产前端构建）；`-SkipBuild` 不重建配置或产物，优先核对 `build.json` 的 profile 标识；兼容旧构建时回读 `instance.json`。
- 子进程只继承 NUL 输入与 `stdout.log`、`stderr.log` 两个文件句柄，不继承调用方的管道；后台或被捕获输出时，脚本在实例就绪后即可返回，实例继续运行。
- 就绪标准：在 `127.0.0.1` 与 `[::1]` 两个回环地址上轮询 `/json/list`，出现且只出现主窗口页面、资料准备窗口已关闭，最长 120 s。主窗口所在地址写入 `instance.json` 的 `cdp`，各驱动脚本默认从清单读取（`--cdp` 可覆盖）。程序提前退出或超时会附带 `stderr.log` 末尾 20 行。
- `instance.json` 字段：`pid`、`executable`、`identifier`、`dataRoot`、`debugPort`、`cdp`、`stdout`、`stderr`、`startedAt`，以及可选 `runId`。外壳在运行器异常退出后仅清理清单中 `runId` 属于本次运行的实例，避免清理前次或其他任务的实例；关闭实例时脚本按 PID、可执行文件路径与启动时间三项核对身份，不一致不结束进程。

**已知限制**：

- 资料目录长度：程序启动时校验主窗口的 WebView2 资料目录 `<资料目录>\webview\<SID>` 不超过 229 个字符；资料目录尚未初始化（没有 `storage-layout.json`、需要先建资料准备窗口）时，还校验准备窗口的 `<资料目录>\migration\bootstrap\webview\<SID>` 不超过 237 个字符。超限直接报错退出（隐藏窗口的测试实例写 `stderr.log` 并以退出码 1 结束）。SID 一般 46 个字符时，全新资料目录应不超过 162 个字符，已初始化的不超过 174 个字符。主检出默认约 107 个字符；`.claude/worktrees/<名称>/` 工作树约 143 个字符，仍可用，但更深的路径需要用 `-DataRoot`（例如 `C:\gs-tmp\<名称>`）。
- 全新资料目录启动时先有资料准备窗口，它与主窗口是两个 WebView2 浏览器进程：先启动的占 `127.0.0.1:<端口>`，主窗口回落到 `[::1]:<端口>`；已初始化目录重启时主窗口在 `127.0.0.1`。不要自己拼 `localhost:<端口>` 连接，使用清单里的 `cdp`。
- 生产落盘入口从构建前到最终清理持有同 profile 互斥锁，冲突直接失败，防止覆盖资料和产物；不同工作线并行时仍须使用不同端口与 `-DataRoot`。
- 机器满载时，调试构建、虚拟 PLC 心跳与布防可能超时。框架不自动重跑整条测试，也不修改产品节拍门槛。
- 端到端都是隐藏窗口，截图来自 WebView2 页面；结论只覆盖业务接线、落盘与界面驱动。

## 站点模式（真机 / 离线）

软件启动总是真机模式：测量用 lyFlow，相机只接海康 MVS，PLC 只支持 S7。“总览”首页显示当前模式“真机”“离线 · 样本集 <名称>”或“离线 · 合成图”，点“查看站点模式”或侧栏“工艺 → 离线模式”进入离线控制台（`/offline`），控制台标题为“当前：真机”或“当前：离线 · <来源>”。

进入离线时软件临时派生并应用：PLC 切到进程内 S7 服务（127.0.0.1 随机端口，点表按现行通讯协议）；相机组按样本集 `cameras[]` 生成样本集相机，或按真实相机组同编号、同视角数生成模拟相机（单视角相机不足 3 台时补足到 3 台，供段配方使用）；测量用 lyFlow（样本集）或模拟测量（合成图）；帧录制临时为“全部”。真实 PLC、相机组和系统设置不改写，退出离线后全部恢复；恢复真机 PLC 配置失败时 PLC 保持断开并停在安全配置，离线控制台与日志标为“故障·需人工修复”。进入离线失败回滚时按进入前状态重连真机 PLC。离线期间相机页、PLC 页只读；有在途事务时不能切换模式。离线模式不持久化，重启回到真机，但会记住上次来源和样本集目录。

**离线 · 合成图跑一件**

1. 断开外部 PLC，打开离线控制台，来源选“合成图”，点“进入离线”。
2. 选离线配方，工况选“正常件”，点“运行一件”；在线检测页也有离线配方、运行一件和停止的紧凑控件。
3. 在线检测与历史记录（切到“离线件”）应出现同一 SN 的整件结果和原图。传输丢帧按 SDK 帧序号与相机帧数诊断。“PLC 回显核对不符”进入握手故障。

**离线 · 样本集对照期望/实际**

格式契约见 [离线样本集格式 v1](../integration/sample-set-format.md)（只接受 schemaVersion 1）。段配方把样本集解释为“每台相机按清单顺序的帧序列”（`Manifest::camera_frames`，第 seq 个脉冲取各相机的第 seq 帧），缺图按缺帧处理。标签只用于展示期望，不参与判定。

1. 确认 lyFlow 引擎就绪（运行库位于程序目录 `runtime/lyflow/`，开发时由 `pnpm deps:lyflow:dev` 复制），否则拒绝进入并提示原因。
2. 离线控制台来源选“样本集”，点“选择样本集目录”，核对件数、帧数、缺图和标签分布，点“进入离线”。
3. 无已标定的正式版时（只剩采集版视为上次建站中断，先删除再重建）：新建候选并发布采集版，用 `capture_fixture` 生成仿历史录制件，再选示教件、确认分段、走廊示教、选标定件多轮标定、独立样本验证与发布。样本集是停稳拍数据，一条走廊对不上其他点位，检测项开着时多半判 NG_GAP；只作流程冒烟时关闭各段检测项。独立样本验证可选，不阻断发布。
4. 选离线配方和样本件（不选则按顺序循环），点“运行一件”或“连续运行”，用“本件后停止”/“立即停止”结束。样本集来源只提供握手类工况：正常件、传输丢帧 / 缺帧、运动超时、PLC 回显核对不符，以及段配方的脉冲串工况（见上文“段模型专项”）。
5. 核对“最近样本件”表的期望 / 实际。

**生产记录隔离**

配方共享，生产记录按来源隔离：检测记录带来源（真机 / 离线·合成图 / 离线·样本集）；离线件 SN 形如 `OFFLINE-0001234567`，同一离线运行内唯一。历史页默认只看真机件，“离线件”切换查看离线件，列表与详情标注离线来源；统计、批量重判与 CSV 导出（含“来源”列）只算当前范围，在线检测的今日统计跟随当前站点模式。离线原图写入数据目录下独立的 `records-offline`，按系统设置中的离线原图保留件数和容量上限（默认 300 件 / 10 GB）单独清理，保存后在下一次录制收尾时按新上限清理，任一超限均删除最旧原图，真机 `records` 只按系统设置清理，离线连续运行不会挤掉真机原图（`recorder` 的 `offline_retention_prunes_only_offline_recordings_and_never_real_raw_images`）。样本件认领只在离线期间生效，退出离线清空本次运行记录，同 partSn 的真机件不带样本引用。

**退出与历史复测**

1. 点“退出离线”，确认离线控制台标题回到“当前：真机”，PLC 配置、相机组和帧录制设置均为进入前的值。
2. 历史记录切到“离线件”，打开离线跑过的件，详情显示“离线样本 partId · SN · 期望 OK/NG”；因离线期间录制为全部，可直接原图复测或与候选配方对照。
