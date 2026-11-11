# 测试说明与复现

本文说明各层测试的运行方法、环境要求、隔离实例约定与证据边界。用例及保护规则以测试源码为准，准入遵循 [AGENTS.md](../../AGENTS.md)：每个用例保护一条业务规则、对外契约、已修复缺陷或易错边界。修改时按影响范围筛选文件或模块，不以测试数量作为完成指标。

## GitHub Actions

`Windows CI` 先按 Git diff 路径选择校验：只有 `README.md` 和 `docs/` 的改动跳过测试；混合改动、其他路径以及手动运行均执行完整校验。重命名按删除和新增两端判定，push 比较整个提交范围。路径判断失败会阻止合并。工作流保留轻量的必需检查 `verify`，不使用会让分支保护一直等待的 workflow 级 `paths-ignore`。

所有 CI 任务只使用 Windows runner。前端 Vitest 用原有发现范围分成两个 shard，与 Rust 库回归并行；格式、空白和 CI 路径回归只在第一个 shard 执行。`verify` 等待两个 shard 和 Rust 全部完成，任何失败或意外跳过都不能通过。`node --test scripts/Plan-Ci.test.mjs` 保护文档过滤不能漏掉代码与混合改动。PR 连续推送只保留最新运行；main 和手动运行使用独立并发组，既不取消执行中的提交，也不替换排队中的提交。

Node 使用 `setup-node` 的 pnpm 缓存；Rust 仅缓存依赖，不缓存工作区产物，CI Rust 缓存只在 main 保存供 PR 复用。沿用已有缓存路径，避免缓存版本改变后依赖全部重编；不通过 `cargo install` 安装工具。前端任务不恢复 Rust 缓存。`Cargo.toml` 的 `profile.test.package.gluesight` 将测试根 crate 的 `opt-level` 设为 0，本地与 CI 均通过 `cargo test --locked --lib` 使用该配置，减少测试编译耗时；图像解码依赖继续保留优化，开发运行和正式打包配置不变。测试仍完整发现并执行，不按耗时分片重复编译。

`Windows package` 保留运行库、实际 DLL 视觉与应用验证、NSIS 构建及报告；依赖安装与编译各执行一次，发布草稿仍等待安装包验证成功。

纯文档 PR 仍会执行轻量的 `changes` 与 `verify`，测试任务显示为跳过；`verify` 成功后即可满足分支保护，不需要修改必需检查配置。

复查耗时时，按 job 的 `created_at → started_at` 统计排队；准备阶段包含依赖恢复与编译，不能把整个 `cargo test` 步骤都算作测试。Rust 实际测试时长以日志中的 `test result` 为准，前端以测试命令耗时计；清理单独记录。并行 job 的时间不能直接相加作为整条 workflow 耗时，比较时还应确认测试发现范围与条数一致。

```powershell
gh run list --workflow windows-ci.yml --limit 50 --json databaseId,headSha,event,conclusion,createdAt,updatedAt
$runId = gh run list --workflow windows-ci.yml --limit 1 --json databaseId --jq '.[0].databaseId'
gh api "repos/liangyuR/ly-gluesight/actions/runs/$runId/jobs?per_page=100"
gh run view $runId --log
```

## 当前保留范围

默认 Rust 库回归与前端 Vitest 优先覆盖结果交付、数据归属、发布准入、草稿防丢和资料安全；同一业务规则的等价输入使用参数化，重复验证优先保留能观察业务结果的一层。增删用例按保护规则评估，不通过跳过测试或修改发现范围控制计数。

打包门禁、样本转换和独立生产模拟器分别维护。默认回归的覆盖边界如下，修改这些路径时需补充针对性的验证：

- 前端保留地图选帧、草稿与发布联动、交叉歧义和非法几何拒绝；不完整覆盖弧长定位、闭环跨起点采集、视图偏好持久化、提示文案、配置控件的逐项赋值及试测前自动保存顺序。
- Rust 优先验证生产状态与资料安全；不完整覆盖调试会话与拓扑编辑、MCP 画布坐标、图像字节布局与颜色转换、软触发缓存与手动释放后的相机重连、导航资料持久化与传输、旧项目记录迁移、标定图像写入失败恢复、短示教件裁段、采集版发布前置检查、以及比例分段的精确布局、中心线遮挡裁剪、视觉候选评分、精细找边窗口和插值算法。
- 日志展示与导出分页、兼容输入、起胶建议、离线图像对齐和外部模拟器完整生产路径仍需专项验证。默认回归中的合成数据断言不替代现场测量精度与节拍验收。

| 测试层 | 保留验证 |
| --- | --- |
| 前端 | 默认 Vitest 回归：段配方创建与草稿防丢、采集准入、走廊示教与试测、发布门槛、周期/帧归属隔离、历史区域重判、PLC 操作锁、维护模式、项目管理、单视角标定更新与配置安全 |
| Rust 默认 | `cargo test --lib`：当前段配方、帧组与采集、示教件分段写 N_exp 与测量示教件、比例分段与区域判定、PLC 握手、调度、发布与恢复、项目导出导入与复制；包含少量真实 S7 TCP 回环、SQLite 和原图录制集成测试，不能把全部库测试称为纯单元测试 |
| S7 专项 | `plc_session` 的交付、复位、心跳及维护隔离用例（真实生产驱动 + 进程内 S7Server）已进默认 `cargo test`；独立模拟器完整生产需专项验收 |
| 生产模拟 | 独立进程提供 S7 与 TCP 相机发图，应用走真实 lyFlow、正式配方和正常生产记录链路；步骤见 [独立生产模拟器](production-simulator.md) |
| 保留工具 | 打包契约、Windows 发布门禁、安装包自检、样本集转换脚本 |
| 手动专项 | 独立模拟器连续生产（需匹配正式配方与原图）；吞吐探针 `throughput_probe`（见“3 × 25 Hz 吞吐门槛”）；相机事件探测 `camera_event_probe`（现场真机：列出 Line0 高电平门控下相机支持的 GenICam 事件，并轮询 Line0 电平记下每次上升 / 下降沿（锁存相机时钟、读 Line0 跳变计数器），打印边沿、事件与帧的时间线，统计首帧、末帧相对边沿的延迟，为判定收尾取代固定收尾窗口定方案；现场 JHEM131GC-XS 没有事件，靠轮询；运行说明见文件头）。按门控提前收尾（D-SB-54）由 `gate::tests`（门控只在新鲜读数都看到关闭时算关、空闲电平按没来帧时学）与 `cycle::tests::closed_gate_ends_the_drain_after_the_guard_but_an_open_gate_waits_for_the_window` 覆盖 |

默认回归不加载 lyFlow DLL，不替代真实设备或现场节拍验收。

项目精简专项使用 `cargo run --manifest-path src-tauri/Cargo.toml --example compact_project -- <来源项目目录> <不存在的副本目录> <lyflow_core.dll>`。工具只生成独立副本，逐字节比较保留的原图，并在真实运行库上比较来源与副本发布包的逐段预热结果；已有预热失败必须保持相同，不能把结果一致解释成全部段可检测。输出字节数是逻辑文件大小；硬链接的实际占用需按文件身份另计。该副本不改写本机绝对路径，不代替项目导入流程的异机验证。

显式设置 `GLUESIGHT_DATA_DIR` 的隔离实例将日志写入 `<隔离资料目录>/gs_logs/`，不使用当前账号的 `~/gs_logs`。隔离实例驱动通过 `storage_status` 和 `config/log-storage.json` 核对资料归属、环境及命名空间，再访问运行日志和视觉原图；不能从原图目录推断项目数据库位置。隔离路径检查可单独运行 `node --test tests/native/storage-paths.test.mjs`。

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
| `python tests/native/prepare-production-simulation_test.py` | 录制件转模拟输入回归，需 Python、numpy、opencv-python-headless；保护 Bayer 颜色、帧/笔结构与非法输入拒绝 |
| `python tests/native/gsr-raw_test.py` | 读录制目录的 Python 脚本共用的原图读取（`scripts/gsr_raw.py`）回归，需 Python、numpy、zstandard；保护空闲压缩过的原图（`.gsr`）读出与原 `.pgm` 相同的字节与像素、损坏文件被拒绝 |
| `cargo test --locked --manifest-path tools/production-simulator/Cargo.toml` | 独立生产模拟器协议与图片输入回归 |
| `pnpm samples:test` | 样本集转换脚本（`scripts/sample-set/convert.py`）测试，只需 Python 3 标准库 |
| `pwsh -NoProfile -File tests/packaging/Test-RuntimeBundle.ps1` | 打包版本、ABI、源码修订及运行库依赖契约 |
| `pwsh -NoProfile -File tests/packaging/Test-LyFlowDependency.ps1` | lyFlow 依赖的 Release 选择、配置与 Cargo 锁定、运行库校验、缓存与网络回退、升级回滚（网络与 DLL 探针用本地夹具） |
| `pwsh -NoProfile -File tests/packaging/Test-VendorDependency.ps1` | 厂商运行库的包隔离、结构、尺寸、CRT 版本、固定源恢复与缓存边界 |
| `node --test tests/packaging/windows-workflow.test.mjs` | Windows 打包工作流门禁：版本与安装方式校验、发布标签须在 main 上、版本设置、SDK 干净环境检查、视觉检查先于打包、依赖缓存标识与发布清单 |
| `pwsh -NoProfile -File scripts/Test-DevStartup.ps1` | Windows 旧终端在设置/清除本地运行库后启动，子进程读取最新保存值，参数与退出码保持不变 |

单文件前端测试示例：`pnpm exec vitest run tests/teaching.test.tsx`；Rust 模块示例：`cargo test --manifest-path src-tauri/Cargo.toml --locked --lib segmentation::`。默认测试不生成覆盖率报告。

[历史件管理](../architecture/history-management.md)定向验证：`pnpm exec vitest run tests/history-list.test.tsx tests/history-detail.test.tsx tests/history-management.test.tsx`，覆盖人工 Tag 保存与筛选、删除取消与确认、失败重试、保留原图警告及迟到请求；后端使用 `cargo test --manifest-path src-tauri/Cargo.toml --locked --lib -- store:: audit:: history_package:: workspace::comparisons::`，覆盖标签持久化与记录包往返、批量校验、编号不复用、删除后旧追溯不回补及对照清理。

长件浏览与复算的读取、进度和取消契约见[长件浏览与复算](../architecture/frame-browsing.md)。定向验证：

- `pnpm exec vitest run tests/frame-navigation.test.tsx tests/frame-inspection-navigation.test.tsx tests/history-detail.test.tsx tests/operation.test.tsx tests/preview-cache.test.ts tests/segment-teach.test.tsx`：保护千帧跨窗口导航与分页选择、全局帧号、迟到对照不覆盖新结果、进度任务隔离及保存边界、离页取消、快速切帧淘汰旧请求。
- `cargo test --locked --lib --manifest-path src-tauri/Cargo.toml -- operation:: workspace:: store:: recipe_capture:: history::`：覆盖概览与单帧读取契约、公开示教读取接口的身份及缺图拒绝、任务生命周期取消与提交边界、终态事件与查询计数一致及受影响业务回归。真实历史批量读取、对照摘要缓存、实际测量循环取消与测量产物保存失败回滚尚无公开命令集成覆盖。

分段按需缩略图定向验证：`pnpm exec vitest run tests/thumbPages.test.ts tests/segment-teach.test.tsx tests/workbench-draft.test.ts tests/workspace-navigation.test.tsx`。保护未加载图像时可编辑段落、笔末边界固定与跨笔段拆分、千帧件全局边界保存、会丢走廊时先确认再保存、按需页调度和缓存上限。后端 `thumbnail_range_reads_only_requested_frames_and_rejects_unbounded_requests` 验证只读请求范围、尾页截断与非法范围拒绝。

浏览器布局验证使用合成的 1000 帧组、三台相机检查末帧跳转、跨窗口方向键、焦点及 DOM 规模；真实图像解码、整件复测耗时和生产并行负载仍需现场性能验收。

胶条算法定向回归：`cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib -- workspace::centerline:: burst_judge:: vision::taught:: vision::judge::`。其中合成胶条图通过提取入口验证汇合路径歧义；公开逐帧判定入口验证胶宽边界舍入与真实超差；找边判定验证左右找边平票不产生可信边距。它们保护具体回归，不代表现场检测准确率或吞吐验收。

`Test-DevStartup.ps1` 临时修改用户级 `GLUESIGHT_LYFLOW_RUNTIME`，在 `finally` 中恢复原值；验证期间不要并行修改该配置或启动 GlueSight。测试通过临时 CLI 进程观察启动环境，不启动实际 GUI，不修改配方。


Windows 受限环境遇到 Node realpath EPERM、回环连接被拒等权限失败时，在获准的主机环境运行相同命令，不修改应用逻辑掩盖环境失败。Cargo 首次构建需要 MSVC、Visual Studio C++ 工具及私有 Git 依赖读取权限；离线执行要求依赖已缓存。

## Windows CI 与手动验收

Windows PR CI 执行 Format 检测、前端单元测试（`pnpm test`）、Rust 默认库回归（`cargo test --locked --lib`）、独立生产模拟器测试和生产录制输入转换测试。Format 用 rustfmt 解析变更的 Rust 文件并生成格式结果，仅检查与本次新增、修改行相交的格式差异；新文件全检，纯删除文件中的测试时仍解析保留代码，无 Rust 变更则跳过。所有变更文件另用 `git diff --check` 检查空白。PR 比较 base SHA 与当前提交，main 推送比较 before 与当前提交，手动运行比较当前提交与上一提交。存量 Rust 尚未全仓格式化，因此使用增量格式检查，避免无关源码重排；项目尚未配置前端格式化工具。

本地使用 `node scripts/Check-RustFormat.mjs --base main` 检查相对 main 的已跟踪文件变更；加 `--head <commit>` 可检查指定提交快照。脚本只读，需先为项目工具链安装 rustfmt（`rustup component add rustfmt`）。

类型检查、旧样本集转换、打包与桌面生产验收均不在普通 PR CI 中执行，CI 不恢复 lyFlow 运行库、不构建桌面应用。推送 main 和手动运行 Windows CI 使用同样的轻量验证；Windows 发包工作流仅由手动运行或 `v*` tag 触发，保留发布门禁。


## S7 生产握手

协议与点表见 [PLC 通讯协议](../integration/plc-communication-protocol.md)。旧版本协议的 PLC 写入被拒绝。

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib -- plc_session
```

`plc_session` 的默认用例由进程内 S7Server（`tools/production-simulator/src/s7_server.rs`，只监听 127.0.0.1 的随机端口，含仅测试编译的定点故障注入）驱动真实的生产会话，已是默认测试。用例需要本机 TCP 回环权限：Windows 沙箱拒绝 localhost 连接（WinError 10013）时，在获准主机环境运行，不能跳过 TCP 后记为通过。各案例的状态、事务与 DB 快照写在测试临时目录（`s7-session-tests`），通过后自动清理，本机产物不提交。

## 段模型（分段连拍飞拍）专项

段模型的测试按 AGENTS.md 准入编写：每个用例对应一条业务规则、对外契约或易错边界，名称写成“条件 → 预期结果”。Line0 高电平门控，帧率由软件设置、默认 10 fps；记录中的 `deviceTriggers` 为预留字段，不参与采集计数、告警或排空判断。规则取舍见 [分段连拍决策记录](../decisions/segment-burst.md)。

**分段：比例分段（D-SB-40）。** `segmentation::Proportional` 按各段 N_exp 比例与本件帧组数推算边界，单段或有段没有 N_exp 时返回未分段结果；`resolve_regions` 在可信边界两侧切出 `boundaryGuard` 帧过渡区。图像运动细化（D-SB-43）未实现。单元与生产节拍测试只验证比例公式、过渡区与判定规则，**不证明现场边界准确**；边界准确性用现场已确认手动分段做留一离线评估，它不是测试、不进 `cargo test`：

```powershell
cargo run --manifest-path src-tauri/Cargo.toml --example segment_eval -- <项目目录> <候选编号>
```

逐帧定向与找边（D-SB-47、D-SB-48）的离线评估同样不进 `cargo test`，需真引擎；逐次结果写 `<输出目录>/runs.csv`，汇总打印到标准输出：

```powershell
$env:LYFLOW_CORE_DLL = "<运行库目录>\lyflow_core.dll"
cargo run --manifest-path src-tauri/Cargo.toml --release --example heading_eval -- <项目目录> <候选编号> <输出目录>
```

分段标定（D-SB-44）的默认回归保留 `workspace::capture_review` 的计划修改后复核失效、屏蔽不压缩序号及恢复后重新测量；段只在笔内（RB-12）由 `segment_corridor_tests::saving_a_segmentation_whose_segment_spans_a_pass_end_is_rejected` 覆盖；分段保存按帧范围逐段对应（D-SB-52）由 `segment_corridor_tests::saving_the_segmentation_keeps_corridors_and_params_of_segments_that_did_not_change` 覆盖（加边界、改类型、挪边界时哪些走廊与段参数保留，以及预演列出的清除项）。段不检测（D-SB-53）由 `segmentation::skipped_segment_and_its_guard_frames_are_not_measured_by_any_camera`（整段连同两侧过渡帧划为跳过、任何相机都不量）、`plan::tests::invalid_segment_plans_are_rejected`（全部不检测、单段不检测报错）和上面的分段保存用例（设为不检测清掉它的走廊、不列为待示教）覆盖。按相机时间戳编号（D-SB-55）由 `frame_clock::tests`（跨门控连续编号、丢帧跳号、时钟回退、没有边沿时按帧间隔判及漏记边沿后的多相机对齐）、`cycle::tests::camera_reopen_during_part_keeps_numbering_by_timestamp` 与 `cycle::tests::drain_ends_at_gate_close_once_every_camera_has_its_last_frame` 覆盖。前端 `tests/workbench-draft.test.ts` 和 `tests/segment-teach.test.tsx` 验证边界编辑、点段名的三态循环、按预演列出影响并确认后一次提交。图像对齐（D-SB-51）只用于离线评估，不在默认回归范围；无区分度画面、比例容差、伸缩、丢帧和重复帧下的可信度与现场精度需专项验证。

读该候选 `workspace.json` 的段列表、`boundaryGuard` 与各轮已确认的手动分段（需要有多轮已确认分段的项目数据，如旧流程候选或手工整理的评估项目），逐轮用其余轮次的帧组数均值（`calibrate_counts`）得出各段 N_exp，再比较比例边界与手动边界，并给出“按原帧号”的对照；比例评估不读图像，模板图像对齐评估读取采集原图，均不需要 lyFlow。至少要 2 段、2 个已确认轮次。评估结论需附对应数据版本与运行输出，本仓库不保存现场数据与临时验收证据。

起涂与收尾跳过建议（D-SB-50）属于专项验证范围：改动时应核对连续成形识别、成形前无法判断时不给建议、没测量示教件时提示先测量，以及界面采用建议只更新对应一头。现有轮次都是空跑，建议值只用合成数据验证过。

**合成图采集与示教。** 合成图由 `src-tauri/examples/capture_fixture.rs` 生成仿历史录制件（[示教件决策](../decisions/recipe-build-flow.md)“手动配置数据”）：按录制时生效的生产配方（采集版：型号、段、计划相机）用 `simimage::render_burst` 生成 N 件，每件在真机录制根下写一个与真机同格式的录制目录（原图 `{相机}_s{seq:06}.pgm`、`part.json` 带 `partMode: maintenanceCollect`、`devices`、`plc` 与来源标记 `fixture: {label}`，以及维护采集保留标记），并在本机记录库写一行归 `--project` 项目的维护采集记录（SN 前缀 `FIXTURE-`）。生成的件出现在历史件选择器里，标“手动配置”，与真机件走同一选件、导入路径。记录引用采集版的修订号：先建候选并发布采集版，再生成数据（候选的版本号留给之后的正式版，用它写快照会让正式版投产时同修订号的真实记录因快照不同被拒）。本机记录库须已由软件建好且版本与软件一致（现为 10）；该项目已有同修订号的不同快照时写入前即拒绝。命令为 `cargo run --manifest-path src-tauri/Cargo.toml --offline --locked --example capture_fixture -- --doc <采集版文档.json> --cameras <camera_rig_config.json> --db <资料根目录>/history/inspection.db --project <项目编号> --records <该项目的真机录制根> --parts N [--seed S] [--label L] [--sample-set <目录> --part <件>] [--onset H --onset-jitter J --tail T --tail-jitter J]`，输出每件的 `recordId`、`cycleId`、SN 等 JSON：默认按名义剖面出合成图（首尾 ±1、弯道 ±2 帧，种子 seed+i）；给出样本集时每件都取该件的样本集原图，供示教流程手动复现。选示教件、走廊示教、测量示教件和发布仍走正常工作台，并使用真实 lyFlow。样本集按“每台相机的帧序列”解释（见[离线样本集格式 v1](../integration/sample-set-format.md)），现有的 2026-10-17 单圈样本集是停稳拍数据，没有弯道、帧数没有波动，只作冒烟，不作分段或标定的验收依据。

读录制目录的 Python 脚本（`scripts/Prepare-ProductionSimulation.py`、`scripts/history-import/import_part.py`、`scripts/color-eval/glue_color_eval.py`）经 `scripts/gsr_raw.py` 读原图：录制目录里有空闲压缩过的原图（`.gsr`，[image-storage](../decisions/image-storage.md) IS-2）时需要 `zstandard` 与 numpy（`pip install zstandard`），按 `part.json` 里的 `.pgm` 文件名读出原始字节；没有 `.gsr` 时不需要 `zstandard`。

**导入现场录制件。** 现场导出的一件录制（真机录制根下的 `*_cycle_<cycleId>` 目录，或把它整个打成的 zip）可用 `scripts/history-import/import_part.py` 导入开发项目的历史记录，在示教件页的历史件选择器里选用，或在试测页的历史件试测里直接复测：

```powershell
python scripts/history-import/import_part.py <录制目录或 zip> --project <项目编号> [--recipe <配方编号>] [--version N] [--label L] [--dry-run]
```

脚本把原图按“每台相机 seq 1..groupCount”转成单件样本集（缺的帧按缺帧处理，不错位），连同配方文档与项目当前相机配置写到 `output/history-import/<录制目录名>/`，再调用 `capture_fixture` 的样本集模式写一件仿历史录制件。导入的是原图，不是现场记录本身：SN、cycleId、时间戳与帧号重新生成（SN 前缀 `FIXTURE-`，选择器标“手动配置”），设备配置记为项目当前值（与现场不同时脚本逐项打印），型号取配方。记录归属 `--recipe`（缺省取现场 `part.json` 的配方编号），配方文档优先用项目已发布的同编号配方；没有已发布配方时用同编号候选的文档，版本、阶段取现场配方（如 `NEW-1-v2` 采集版）。之后改了候选再导入、或候选发布到同一版本时，同修订号快照不同会被拒，用 `--version` 换一个不会发布到的版本。项目须用软件打开过（本机记录库 `<资料根目录>/history/inspection.db` 与该项目的录制根已建好），记录归 `--project` 项目；软件运行中也可以导入，选择器每次重新查库。

**胶条颜色通道评估。** 相机在灰度模式下，取图把 8 位 Bayer 按 (R + 2G + B) / 4 转灰度，录图也是灰度（彩色模式见 [color](../decisions/color.md) CL-1）。有色胶（例如粉色）涂在两种材质拼焊的板上时，胶在两侧的明暗方向可能相反，D16 的单一极性测不全。`scripts/color-eval/glue_color_eval.py` 读彩色样图或相机原始 Bayer（`.raw`），比较现行灰度、G、R−G、Lab a*、饱和度五种单通道。指标都在人工标注的探针上量：胶对比度 d'（|胶 − 背景| / 背景噪声）、极性是否一致、材质间顶帽响应比（低于 D16 的 0.3 时，一帧跨两种材质，弱的一侧会被判无胶），以及焊缝、高光等干扰物响应与胶响应之比。只需要 Python、numpy、OpenCV、matplotlib，不读项目数据，不需要 lyFlow。

```powershell
python scripts/color-eval/glue_color_eval.py <标注.json> --init <图像目录>   # 生成标注模板
python scripts/color-eval/glue_color_eval.py <标注.json> [--out <输出目录>]
```

样图要在现场光源下用彩色相机采集，并包含两种材质、焊缝与高光。最直接的做法是相机用彩色模式，再用维护模式采集（[r2-records](../decisions/r2-records.md) R-8）：录制的原图本身就是原始 Bayer（PGM 头带 `# bayer` 注释，`--init` 与读图都按注释取相位）。已有录制里另存的 `*_bayer.pgm` 也能用：`--init <录制目录>` 只收这些文件，并从 `part.json` 读取相位；同目录的灰度 `.pgm` 没有颜色。也可以用 MVS 客户端存彩色图或原始 Bayer（`.raw`）。每张图标若干探针，坐标为原图像素：

- 胶探针（`kind: glue`）：垂直横穿胶条、中点落在胶中心，填 `material` 与 `halfWidthPx`，两侧各留至少 18 px 背景。
- 干扰物探针（`kind: distractor`）：横穿焊缝、高光等不该被当成胶的窄结构，填 `label`。

原始 Bayer 在标注文件里填 `bayer: {pattern}`，`.raw` 还要填 `width`、`height`；`pattern` 按 GenICam 命名（现场相机为 `BG`）。`widthMaxPx` 是检测胶宽上限，顶帽结构元直径取它加 1。

终端打印各通道的汇总表与不可用原因；输出目录（默认 `output/color-eval/`）里有 `report.json`、逐探针的 `probes.csv`，以及每张图的通道与响应对照图 `<图名>_channels.png`。可用门槛：d' ≥ 3、极性一致、材质间响应比 ≥ 0.3、干扰物 / 胶 ≤ 0.5。其中 0.3 取自 D16 的对比度比，其余是经验值，结论用于选择通道公式，不代替带实际胶路的试测。

### 3 × 25 Hz 吞吐门槛

三台相机各 25 Hz 同时出帧是 75 帧/s，1280 × 1024 单字节灰度每帧 1.31 MB，约 98 MB/s。P1 探针量测量耗时与 PGM 写盘速率；它是 `src-tauri/examples/throughput_probe.rs`，不是测试，不在 `cargo test` 里，写约 4 GB 临时文件、结束后自动删除，约 1 分钟，需要已发布的 lyFlow 运行库：

```powershell
$env:LYFLOW_CORE_DLL = 'src-tauri\target\dependency-cache\lyflow\<版本>\lyflow_core.dll'
cargo run --manifest-path src-tauri/Cargo.toml --example throughput_probe
```

门槛：现场机用探针得到串行 p95，代入 `ceil(75 fps × p95 / 0.7)` 得到所需测量线程数，不得超过 `min(4, 逻辑核 − 2)`（`MEASURE_WORKERS`）；压测中不得出现图像内存预算满或测量队列满（出现时该帧记缺帧或未测，该组标检测不完整，不会静默丢帧）。不满足时先降低相机设定帧率，各段 N_exp 随之变化，须按新帧率的示教件重新分段并发布。完整数据与降级方案见决策记录 7.1。


**门槛状态：没有达标，也没有被直接验证。** 持续吞吐须在现场工控机、真实相机与已发布配方上验证，核对每台相机实际帧率、预算丢帧、测量队列和未测帧。独立模拟器使用正常 lyFlow 测量，可验证软件输入链路，但不证明海康 SDK 回调、网络带宽与物理门控的持续吞吐。

## 现场验收

**JHEM131GC-XS 高电平门控（2026-10-24 现场验证）。** 用户确认已在现场验证 `FrameBurstStart + LevelHigh`：持续高电平期间按相机设定的 10 FPS 连续采集，拉低后停止新曝光。该型号按已支持设备处理，参数页和批量应用结果不再提示待验收；仍逐项写入并回读校验触发节点、帧率、BurstFrameCount 和触发缓存关闭，失败仍阻止相机就绪。只对 JHEM131GC-XS 放开 Line0 的 FrameBurstStart + LevelHigh，其他型号不增加 FrameBurstStart 门控回退。不将此结论推广至其他型号，也不替代机器人互锁或故障停止验收。

**待真机验收**（自动测试只覆盖配置分支与界面，不代表设备写入已实测）：

- 频闪：LineSource 仅提供 ExposureActive 时频闪配置写入、回读成功；参数写入不执行整组回滚。
- Chunk：设备没有 Chunk 节点时只显示一条不支持提示。
- Software 单帧：仅有 FrameBurstStart 的设备切换到 Software 后，回读 BurstFrameCount=1，每次手动命令只得到一帧；具有 FrameStart 的设备继续选择 FrameStart。
- TriggerDelay：Integer 节点写入整数微秒并回读一致，小数应明确失败；Float 节点继续接受浮点值。类型查询、写入或回读失败时相机不得就绪。
- 曝光与增益：按相机保存在项目相机配置 `cameras.json`，写入海康相机的时机为相机页点“保存曝光与增益”（已连接时立即写入）和相机打开、重连后关闭自动曝光与自动增益之后；写入失败则打开失败。外部模拟器没有可写入的曝光和增益，实际写入 ExposureTime / Gain 须在台架用海康相机确认。
- 触发切换、重连与多相机行为。

## 隔离桌面与证据边界

使用 `Build-UiTestInstance.ps1`、`Start-UiTestInstance.ps1`、`Stop-UiTestInstance.ps1` 准备和管理隔离实例，配合[独立生产模拟器](production-simulator.md)检查正常生产。`tests/native/lib.mjs` 保留通用 IPC、实例连接、合成历史件与配方示教辅助函数，供手工复现使用；`desktop-infrastructure.test.mjs` 与 `storage-paths.test.mjs` 验证实例身份、互斥与路径隔离；`p0-single-instance-acceptance.mjs` 检查隔离实例重启前后数据库与原图。

jsdom 测试证明页面动作、接口参数和失败恢复，不证明真实布局、硬件通讯或现场节拍。独立模拟器验证生产协议和图像输入，算法精度须用匹配正式配方的现场原图核对，PLC 与相机硬件仍需现场验收。

## 隔离实例入口

```powershell
pnpm test:desktop:build -Profile flow
pwsh -NoProfile -File scripts/Start-UiTestInstance.ps1 -Profile flow -SkipBuild
pwsh -NoProfile -File scripts/Stop-UiTestInstance.ps1 -Profile flow
```

参数 `-DebugPort` 指定 WebView2 调试端口，`-DataRoot` 或 `GLUESIGHT_TEST_DATA_ROOT` 指定隔离资料父目录，`-ResetData` 清理该实例资料。`-SkipBuild` 只复用已有同 profile 产物，不保证与最新源码一致。通过 `GLUESIGHT_LYFLOW_RUNTIME` 指向锁定系列运行库。

**隔离实例的启动**（`scripts/Start-UiTestInstance.ps1`，可单独启动后手动驱动）：

- Tauri 覆盖配置按 `-Profile`（`ui`、`p0`、`flow`、`e2e`、`soak`）在脚本内生成：产品名与窗口标题为 `GlueSight · 胶路智检 <Profile> Tests`，应用标识 `com.xyzrobotics.tujiaovision.<Profile>-tests`，窗口隐藏，不打安装包。由公共构建入口写到输出目录的 `tauri-test-config.json` 再交给 `tauri build --config`（包含 TypeScript 与 Vite 生产前端构建）；`-SkipBuild` 不重建配置或产物，优先核对 `build.json` 的 profile 标识；兼容旧构建时回读 `instance.json`。
- 子进程只继承 NUL 输入与 `stdout.log`、`stderr.log` 两个文件句柄，不继承调用方的管道；后台或被捕获输出时，脚本在实例就绪后即可返回，实例继续运行。
- 就绪标准：在 `127.0.0.1` 与 `[::1]` 两个回环地址上轮询 `/json/list`，出现且只出现主窗口页面、资料准备窗口已关闭，最长 120 s。主窗口所在地址写入 `instance.json` 的 `cdp`，各驱动脚本默认从清单读取（`--cdp` 可覆盖）。程序提前退出或超时会附带 `stderr.log` 末尾 20 行。
- `instance.json` 字段：`pid`、`executable`、`identifier`、`dataRoot`、`debugPort`、`cdp`、`stdout`、`stderr`、`startedAt`，以及可选 `runId`。外壳在运行器异常退出后仅清理清单中 `runId` 属于本次运行的实例，避免清理前次或其他任务的实例；关闭实例时脚本按 PID、可执行文件路径与启动时间三项核对身份，不一致不结束进程。

**已知限制**：

- 资料目录长度：程序启动时校验主窗口的 WebView2 资料目录 `<资料目录>\webview\<SID>` 不超过 229 个字符；资料目录尚未初始化（没有 `storage-layout.json`、需要先建资料准备窗口）时，还校验准备窗口的 `<资料目录>\migration\bootstrap\webview\<SID>` 不超过 237 个字符。超限直接报错退出（隐藏窗口的测试实例写 `stderr.log` 并以退出码 1 结束）。SID 一般 46 个字符时，全新资料目录应不超过 162 个字符，已初始化的不超过 174 个字符。主检出默认约 107 个字符；`.claude/worktrees/<名称>/` 工作树约 143 个字符，仍可用，但更深的路径需要用 `-DataRoot`（例如 `C:\gs-tmp\<名称>`）。
- 全新资料目录启动时先有资料准备窗口，它与主窗口是两个 WebView2 浏览器进程：先启动的占 `127.0.0.1:<端口>`，主窗口回落到 `[::1]:<端口>`；已初始化目录重启时主窗口在 `127.0.0.1`。不要自己拼 `localhost:<端口>` 连接，使用清单里的 `cdp`。
- 构建与启动操作持有同 profile 互斥锁，冲突直接失败，防止覆盖资料和产物；不同工作线并行时仍须使用不同端口与 `-DataRoot`。
- 机器满载时，调试构建、虚拟 PLC 心跳与 PC 输出读回确认可能超时。框架不自动重跑整条测试，也不修改产品节拍门槛。
- 端到端都是隐藏窗口，截图来自 WebView2 页面；结论只覆盖业务接线、落盘与界面驱动。

## 独立生产模拟器

[独立生产模拟器](production-simulator.md)提供本机 S7 PLC 和三路 TCP 相机发图，应用按正常生产链路处理。工具运行在独立进程中，不修改 GlueSight 的运行模式。配置、原图准备、单件/连续生产与验收步骤见该文档。

模拟生产记录按正常来源写入，必须使用独立资料目录和测试项目，不修改现场 PLC、相机与正式项目配置。连续生产可用于观察稳定性，但目前没有自动故障注入和持续吞吐达标判定脚本。
