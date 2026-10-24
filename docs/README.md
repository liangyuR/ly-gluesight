# GlueSight 文档入口

本页写给接手的 agent：先读这一页建立全貌，细节按链接去看。现行设计的唯一基准是 [分段连拍决策记录 segment-burst.md](decisions/segment-burst.md)（决策 D-SB-n、IPC 契约、口径、过渡规则；第九节是各工作包相对计划的调整），代码事实以当前分支为准。本页更新于 2026-10-23，对应分支 `feat/segment-burst-flyshot`（尚未合入 main）。

## 一、当前设计一览

**场景**：固定胶枪，机器人抓工件（RTCP）从胶嘴下走过，三台相机固定在胶嘴周围，边涂边拍。

### 当前配方与触发模式

生产仅支持段配方（`RecipeDoc.segments` 非空，发布包 schema 4）。旧拍照点配方从未部署，不再维护运行入口、迁移、兼容分支和专属测试；本地草稿不自动删除。

Line0 使用高电平门控，相机按软件设置的帧率连续采集，默认 10 fps。Software 保留用于单帧标定、调试，不能进入整轮采集或生产。设定、设备回读及实测帧率分别显示。

Jinghang JHEM131GC-XS 的实际节点需实机确认。当前实现要求设备提供 `FrameBurstActive` 或 `AcquisitionActive`，并接受 `LevelHigh` 与固定帧率设置；关键写入与回读失败则不就绪。Software 使用 `FrameStart`。不以未经确认的 `FrameBurstStart` 替代门控。

### 段配方的数据流

1. **机器人**：整条胶路只拉高一次“相机使能”DO（弯道不拉低，起头、收尾不拍）；等不到触发许可时照常涂胶并置“本件未检”DO。两个 DO 都不进 DB4/DB2。
2. **PLC**：已布防且运动时向相机输出高电平，结束或故障拉低；`deviceTriggers` 为预留字段，写 0。PLC 不参与分段。
3. **三台相机**：高电平期间按内部固定帧率曝光，帧序号取 SDK `frame_num`，不按到达顺序；图像内存按每相机字节预算封顶（`FramePool`，256 MiB），预算满或通道满的帧记缺帧。
4. **帧配组**：seq = frame_counter − 该相机基线，同一 seq 的三帧为一个帧组；缺帧的组标 incomplete；单台相机帧数只用于丢帧告警（与另外每一台相机相差 > 1）。帧数 = 帧组数（第六节口径）。
5. **分段（现为未分段）**：`Segmenter` 接口只有 `Unsegmented` 实现，整条胶路是一个未分段区域：只跑每帧检测、不判胶偏，只核对整件帧组数 ∈ ΣN_exp ± Σtol。
6. **每帧测量**：每台相机的每一帧沿这台相机示教的走廊用 lyFlow（`glue.taught_path` + `glue.bead_width`）测胶宽、断胶与胶中心偏移 x，输出 `FrameMetrics`；测量队列满、超时记 Unmeasured，走出走廊记 OutOfCorridor，都只算检测不完整。
7. **区域判定**（`burst_judge`）：每帧胶宽、断胶、溢胶（每帧最大胶宽超过溢胶上限，NG_WIDTH）三类区域都判；胶偏只在直线段，去首尾各一帧后用 x 均值与标准件 x_ref（只实现 AbsoluteX）判整段偏移、最大偏差判局部跑偏；三台相机取最差。结果另带 `flags`（检测不完整、帧数异常、`short_part` 疑似提前终止）、未检区域、未检项（气泡、拉丝）与“胶偏未判”原因，这些都不改 OK/NG；只有三台相机整件没有一帧测成才判 ERR 91。
8. **落库与历史**：记录库 9 版的 `part_frames` 每台相机每帧一条（缺帧也有），`parts.burst_judgement` 存整份区域判定；原图按帧录制为 `{相机}_s{seq:06}.pgm`；审计 spool 格式 4；历史按 (计划相机, seq) 读图、复测与重判。

配方的建立流程（候选工作区）：整圈采集 → 段列表 → 每台相机的走廊示教 → 多轮标定（默认 ≥ 10 轮，得 ΣN_exp、建议容差由操作员确认、标准件轮次得 x_ref）→ 独立样本验证 → 发布。见 [配方创建流程](architecture/recipe-creation-workflow.md) 的“段配方流程”。

采集完成后支持轮次手动分段、模板套用和边界复核，以及带原因的帧组屏蔽与恢复；屏蔽保留原始序号、原图和帧数，仅排除标定图像取样。每个标定轮次都确认手动分段后，多轮标定按边界逐段标定（各段 N_exp 与建议容差，ΣN_exp 取各段之和）；手动分段暂不接入生产分段或独立样本判定，操作及失效规则见[轮次分段复核与帧组屏蔽](architecture/recipe-creation-workflow.md#轮次分段复核与帧组屏蔽)。

### 协议 6 要点

- `protocolVersion` / `pcProtocolVersion` = 6，其他值（含 5）拒绝；DB4 / DB2 布局、28 个点位、握手顺序、心跳与复位都同协议 5。
- `shotCount` = `deviceShots` = ΣN_exp：生产请求在接件时精确核对，不符返回 90/94；示教采集使用独立 DB6 / DB12，不发送预估帧数，见 [示教采集接口 V1](integration/teaching-capture-sequence.md)。
- `partEnd` 不再比较计数；`deviceTriggers` 不参与丢帧告警、收尾等待或基线计算，partEnd 后等待完整 drain 窗口并完成已接受帧的测量。段配方每台相机的下一件基线 = 本件见到的最高帧号（D-SB-7）。
- 重新标定使 N_exp 或 ΣN_exp 变化时分配新的 `planVersion`，PLC 须同步 `shotCount` / `deviceShots`。
- 结果码：96 对段配方不再使用；示教通过独立状态及原因报告完成、失败或取消，不产生生产结果。生产中 PLC 不提交 partStart = 本件未检，PC 无记录；提前 partEnd 照常判并加 `short_part`。

细节见 [PLC 通讯协议](integration/plc-communication-protocol.md)、[一期 S7 握手契约](integration/plc-s7-phase1.md)、[交互表适配决策的协议 6 节](decisions/plc-table.md#协议-6-适配分段连拍)。

### 关键模块与文件

Rust 文件都在 `src-tauri/src/` 下；前端契约 `src/features/plan/types.ts` 与 Rust serde 输出逐字段对应。

| 职责 | 段配方 |
| --- | --- |
| 配方文档、校验、计划身份 | `recipe.rs`（`RecipeDoc` schema 8、`is_burst`、`same_plan`）、`plan.rs`（`PathSegment`、`PartCalib`、`KindDefaults`、`plan_identity`、`PlanExport`）、`recipe_api.rs`（`recipe_template`、`recipe_plan_export`） |
| 握手与接件 | `handshake.rs`（`PROTOCOL_VERSION = 6`）、`plc_session.rs`、`inspection.rs`（S7 点表模板）、`cycle/admission.rs`（94 接件核对） |
| 生产节拍 | `cycle.rs` 布防分流、`cycle/burst.rs`（在途件、收尾与判定）、`shot_router.rs` 的 `arm_frames` |
| 帧配组、分段、判定、标定 | `frame_group.rs`、`segmentation.rs`、`burst_judge.rs`、`calibration.rs` |
| 相机与测量 | `camera.rs`、`frame.rs`、`measure.rs`（`FrameJob`）、`vision.rs` / `vision/taught.rs`（`measure_frame`）、`production.rs`（`burst_graphs`） |
| 示教采集轮次 | `recipe_capture/burst.rs`（`CaptureRoundV2`） |
| 候选工作区 | `workspace.rs`（`WorkspaceView.plan` 分流）、`workspace/burst.rs`（走廊示教、多轮标定、x_ref）、`workspace/burst_release.rs`（验证与发布）、`workspace/burst_history.rs`（历史读图与复测） |
| 发布包 | `release.rs`（`publish_burst`，schema 4） |
| 记录、录制、审计 | `store.rs`（`part_frames`，库版本 9）、`recorder.rs`、`audit/` |
| 离线仿真 | `offline/line.rs`（脉冲串 `run_burst`）、`offline/scenario.rs`、`offline/sample_set.rs`（`camera_frames`）、`simimage.rs` |
| 前端 | `src/features/workspace/` 的 `SegmentsPage`、`TeachingPage`（相机走廊示教）、`CalibrationPage`、`CapturePage`、`PlanProgress`；`src/features/plan/` |

## 二、已放弃的设计

以下做法在方案定稿或实施中被放弃，不是现行方案；文档里出现时只作历史记录。

| 已放弃 | 现行做法 | 记录 |
| --- | --- | --- |
| 一台三目设备“一帧内三视角”交付 | 三台独立单视角相机共用一个触发，各出 1 帧；三目拼接配置不能参与生产 | D-SB-11，协议文档第 13 节 |
| 协议 5 的 `deviceTriggers` 必须等于 `deviceShots`（结束时计数相等） | 预留写 0，不参与检测判断 | D-SB-4，plc-table P9 |
| 示教采集 1–64 上限与计数核对 | 段配方不设上限、不核对 | D-SB-8、D-SB-9，plc-table P10 |
| PLC 参与分段（段号、每段脉冲数字段） | PLC 不参与分段，协议 6 不加、不预留分段字段，段由 PC 图像算法划分 | 协议文档 4.2 节，plc-table 协议 6 适配节 |
| 机器人逐点 TB 触发（每张图对应一个确定的轨迹点） | 机器人只出相机使能电平，相机内部固定帧率采集，配方改为段列表 | D-SB-1，协议文档 4.2 节 |
| 分段置信度不足时回退到计划边界 | 该边界两侧两段合并为未分段区域，不判胶偏，标“胶偏未判” | `segmentation.rs` 的 `resolve_regions`（`UnsegReason::LowConfidence`），见“需确认”说明 |
| 预期帧数取多轮最小值、多出的帧裁掉 | 多轮帧组数均值四舍五入作 N_exp，所有帧都测，越界只告警 | D-SB-12、D-SB-32，segment-burst 第六节 |
| 特征定位判胶偏、示教中线逐点比对作为段配方的方案 | 段级 AbsoluteX；特征定位与逐点比对只服务旧配方 | D-SB-15，[fixture-position 讨论](discussions/fixture-position/README.md) |

“分段置信度不足时回退计划边界”与“逐点 TB 触发”没有单独的决策条目，依据是分段连拍方案规格（第 18 版）与上表所列代码 / 协议文档。

## 三、未完成事项

- **分段算法**：没有开发。走向、帧间光流、与标定序列 DTW 对齐等线索要等现场按量产曝光与速度拍的含弯道三相机图像；在此之前整条胶路按未分段处理；多段配方的各段 N_exp 只能由已确认的手动分段逐段标定得出，未确认时保持待标定，凭整件 ΣN_exp 发布与运行（D-SB-21）。
- **3 × 25 Hz 吞吐门槛**：没有达标，也没有被直接验证。已定（2026-10-23）：门槛口径不改，等现场工控机用真实相机来源跑 `pnpm test:soak:throughput`（并重跑 `throughput_probe` 探针）后，再决定降 PLC 频率还是加测量并发；合并前须完成该复测（segment-burst 7.1、第九节第 17 条）。
- **Rust 回归范围**：当前 106 项默认回归与 1 项手动算法评测，见 [保留测试清单](testing/rust-test-retained-inventory.md)。原 7 项真引擎和 2 项 packaging ignored 测试已删除，原段配方全链路真引擎用例的实测事实仅保留为[历史记录](testing/TESTING.md#特征定位真引擎历史说明)。
- **已接受、待确认的取舍**：胶偏的 `ToEdge` 测量量未实现；气泡、拉丝不检；“每台相机一条按直线段几何的走廊”没有真实弯道图验证；提前 partEnd 只加 `short_part`、不判 ERR；现场 PLC 草稿 `gluesight_site.scl` 未改，只列改动点。汇总见 [决策索引](decisions/README.md)。
- **真机验证**：PLC、三台相机共用触发、带宽（三路 25 Hz 估算约 826 Mbps）、现场节拍与检测准确率都没有验证；现有样本集 gs-20261017 是停稳拍数据，段配方上只作冒烟。

## 四、各子目录的用途

| 目录 | 内容 | 性质 |
| --- | --- | --- |
| [decisions/](decisions/README.md) | 各批工作线的决策记录；[README](decisions/README.md) 标明哪些仍有效、哪些在段配方下或整体被取代 | 历史记录，正文不改写，被取代处只加标注 |
| [architecture/](architecture) | [配方创建流程](architecture/recipe-creation-workflow.md)、[当前进度](architecture/recipe-creation-progress.md)、[存储布局](architecture/storage-layout.md)、[lyFlow 依赖](architecture/lyflow-dependency.md)描述现状；[面向 Agent 的日志改造方案](architecture/agent-logging.md)为待实施设计稿；[P0 计划](architecture/p0-plan.md)、[历史规划](architecture/roadmap.md)、两张架构 HTML 与[工件边缘方案草案](architecture/workpiece-edge-position.md)是历史或草案，开头标有现状 | 现状 + 历史 |
| [integration/](integration) | [PLC 通讯协议](integration/plc-communication-protocol.md)（另有可离线阅读的 [HTML 版](integration/plc-communication-protocol.html)）、[S7 握手契约](integration/plc-s7-phase1.md)、[离线样本集格式](integration/sample-set-format.md) | 现状契约 |
| [testing/](testing) | [测试记录与复现](testing/TESTING.md)（现状）、[前端保留测试清单](testing/frontend-test-retained-inventory.md)与[Rust 保留测试清单](testing/rust-test-retained-inventory.md)（当前逐项保护规则与删除依据）、[分段连拍历史清单](testing/segment-burst-test-inventory.md)（开发期间新增与删改记录）；带日期的文件（如 [UX_FIXES_2026-10-09](testing/UX_FIXES_2026-10-09.md)、[hands-on-2026-10-19](testing/hands-on-2026-10-19.md)、[soak-report](testing/soak-report.md)）是当时的记录，开头标有现状，`evidence/` 是历史证据 | 现状 + 历史 |
| [design/](design) | 界面设计稿与早期设计状态文件：[PLC 页布局](design/plc-page-layout.md)已按协议 6 与段配方的计划导出更新；单帧示教设计稿（`teaching-page/`）、[`implementation-contract.json`](design/implementation-contract.json) 与 [`black-blue-workflow.json`](design/black-blue-workflow.json) 是旧拍照点模型时期的稿件，开头标有现状（JSON 为 `currentStatus` 字段） | 设计稿，部分为历史 |
| [discussions/](discussions/fixture-position/README.md) | 特征定位判胶偏的讨论记录（只服务旧配方） | 讨论记录 |
| [research/](research/glue-position-methods.md) | 胶条位置检测方法调研 | 调研 |
| [deployment/](deployment/PACKAGING.md) | Windows 打包与运行库部署 | 现状 |

PLC SCL 参考程序与 S7 测试 PLC 在 [`scripts/s7-handshake/`](../scripts/s7-handshake/README.md)。修改协议 Markdown 或其 Mermaid 时序图后，在仓库根目录 `pnpm install`、安装 `scripts/docs/requirements.txt`，运行 `python scripts/docs/build-plc-protocol.py` 重新生成 HTML，加 `--check` 核对生成物是否同步。架构 HTML 与协议 HTML 都是自包含文件，可离线打开。
