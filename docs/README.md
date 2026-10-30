# GlueSight 文档入口

本页写给接手的 agent：先读这一页建立全貌，细节按链接去看。仓库里的 `docs/` 是设计、协议与决策的唯一正本；对外展示用的页面（例如发给 PLC 方的协议页）从这里生成，不单独维护。代码事实以 main 为准。

## 一、当前设计一览

**场景**：固定胶枪，机器人抓工件（RTCP）从胶嘴下走过，三台相机固定在胶嘴周围，边涂边拍。

### 配方与触发

生产只支持段配方（`RecipeDoc.segments` 非空，配方 schema 8，发布包 schema 5）。配方分阶段：采集版只能在维护模式下采图，正式版正常检测（见 [在线投运方案](architecture/online-commissioning.md)）。

Line0 使用高电平门控，相机按软件设置的帧率连续采集，默认 10 fps。Software 触发只用于单帧标定与调试，不能进入整轮采集或生产。设定、设备回读及实测帧率分别显示。

Line0 优先使用 `FrameBurstActive` 或 `AcquisitionActive`；Jinghang JHEM131GC-XS 已现场确认支持 `FrameBurstStart + LevelHigh`，仅该型号允许此回退。设备须接受固定帧率设置，关键写入与回读失败则不就绪。Software 优先使用 `FrameStart`，仅有 `FrameBurstStart` 时限定每次一帧。

### 段配方的数据流

1. **机械臂**：向 PLC 单向通知工件到达（型号、SN）、拍照信号 `CaptureActive`（需要采集的涂胶运动期间为 1，一件内可多次开关）与工件结束，不等 PLC 或 PC。到达到拍照信号之间留 ≥ 2 s，让 PC 先读到请求。
2. **PLC**：按 [生产通讯协议 8](integration/plc-communication-protocol.md)，工件到达后提交请求（plcState=10），不等 PC；工件在检且 `CaptureActive`=1 时向相机输出 Line0 高电平，工件结束后拉低并置 plcState=20。PLC 不参与分段，也不报帧数。
3. **三台相机**：高电平期间按内部固定帧率曝光，帧序号取 SDK `frame_num`，不按到达顺序；图像内存按每相机字节预算封顶（`FramePool`，256 MiB），预算满或通道满的帧记缺帧。
4. **帧配组**：seq = frame_counter − 该相机基线，同一 seq 的三帧为一个帧组；缺帧的组标 incomplete；单台相机帧数只用于丢帧告警（与另外每一台相机相差 > 1）。帧数 = 帧组数。每台相机下一件的基线 = 本件见到的最高帧号。
5. **分段（比例分段，D-SB-40）**：段结构由人定，各段 N_exp 由已确认手动分段的标定轮次逐段得出；本件收尾后 `Proportional` 按各段 N_exp 比例与本件帧组数推算边界，帧组数在 ΣN_exp ± Σtol 内时边界可信，两侧各 `boundaryGuard`（默认 2）帧为过渡区（只判每帧指标、不判胶偏）。单段配方、有段待标定或帧组数越界时整条胶路（或越界时全部段）按未分段区域判：只跑每帧检测、不判胶偏。帧数只核对整件。
6. **每帧测量**：帧到达时按预测段选择检测相机与该段走廊，收尾后补测归属变化的帧，其余相机记 `NotInspected`；参与检测的帧用 lyFlow 的测量算子量出每站暗段、GlueSight 判每站有无胶（[LF-3](decisions/lyflow-operators.md)），得出胶宽、断胶与胶中心偏移 x，输出 `FrameMetrics`；测量队列满、超时记 Unmeasured，走出走廊记 OutOfCorridor，都只算检测不完整。
7. **区域判定**（`burst_judge`）：每帧胶宽、断胶、溢胶（NG_WIDTH）三类区域都判；胶偏只在直线段，去首尾各一帧后用 x 均值与标准件 x_ref（只实现 AbsoluteX）判整段偏移、最大偏差判局部跑偏；按本段检测相机取最差，未分段区域按每帧预测段的检测相机取最差。`flags`（检测不完整、帧数异常、`short_part` 疑似提前终止）、未检区域、未检项（气泡、拉丝）与“胶偏未判”原因都不改 OK/NG；只有三台相机整件没有一帧测成才判 ERR 91。
8. **落库与历史**：记录库版本 9，`part_frames` 每台相机每帧一条（缺帧也有），`parts.burst_judgement` 存整份区域判定，`parts.mode` 区分正常件与维护件；原图按帧录制为 `{相机}_s{seq:06}.pgm`；审计 spool 格式 4；历史按 (计划相机, seq) 读图、复测与重判。

配方的建立流程（候选工作区，[示教件与标定件决策](decisions/recipe-build-flow.md)）：示教件（从维护模式录制的历史件里选一件，导入前可预览原图；开发与测试用 `capture_fixture` 生成仿历史录制件）→ 分段（在示教件上按图像定段与边界）→ 每段每台检测相机的走廊示教（示教帧只取自示教件）→ 多轮标定（从历史件选标定件，默认 ≥ 10 件含示教件，按示教件边界自动对齐、可信即确认加入，其余人工复核；得 ΣN_exp、建议容差由操作员确认、标准件得 x_ref）→ 发布（发布前检查当场核对，独立样本验证可选，D-SB-46）。换示教件或从生产配方改版时清空边界、走廊与标定结果，保留段结构。标定件支持带原因的帧组屏蔽与恢复；屏蔽保留原始序号、原图和帧数，仅排除标定图像取样。界面不显示采集轮次（只是存储格式）。旧流程建的候选已停用，只能删除。每个标定件都确认分段后，多轮标定按边界逐段标定（各段 N_exp 与建议容差，ΣN_exp 取各段之和），生产与独立样本判定再按这些 N_exp 的比例分段；手动边界本身不直接进生产。见 [配方创建流程](architecture/recipe-creation-workflow.md) 与其中的[标定件分段复核与帧组屏蔽](architecture/recipe-creation-workflow.md#标定件分段复核与帧组屏蔽)。

分段与走廊示教支持可选的[工件胶路导航图](decisions/recipe-build-flow.md#rb-9-工件胶路导航图)：在候选中导入整件图、标注有向胶路及采集范围，按段原始帧组进度浏览位置，不做图像配准或相机偏移。正式发布时冻结独立辅助快照，随项目流转；地图缺失或损坏只关闭导航，不影响检测。现场导航精度尚未验证。

### 协议 8 要点

- 双方各一个状态字握手（plcState / pcState），PC 回显 `seqEcho` / `snEcho`；PLC 只报型号、序号与 SN，不下发计划或拍照数量，重新标定不用改 PLC。
- PC 读到 plcState=10 只做接件登记（核对配方、发布包、相机，定帧计数基线，开始录制），不回写 PLC；登记前收到的帧不计入本件，按缺帧判定。不能检测的件（找不到配方、相机未就绪等）照常运动采图，PC 等 plcState=20 后回结果码 90 与对应 faultCode；任何结果都在 plcState=20 之后发布。
- 任何结果都不停线；PC 不在线时 PLC 不等待，本件记未检。复位可由 PLC 现场发起（plcState=99），也可在软件发起（pcState=99，PLC 回空闲后 PC 置就绪）。
- 维护模式 `pcMode=1`：结果码写 0，PLC 按未检放行；切换从下一件生效。

### 项目

一套可上线的检测方案（相机组、PLC 配置、标定、配方、候选、记录）收拢成一个项目，存于 `projects/<id>/`；支持新建、重命名、删除、整个项目导出导入与复制，切换项目要重启。见 [项目设计](architecture/project.md) 与 [存储布局](architecture/storage-layout.md)。

运行日志和历史原图集中在 `~/gs_logs`，按环境、资料实例和项目隔离；示教、标定及发布资源随项目保存。根绑定及项目导出规则见[日志与视觉证据存储](architecture/log-storage-research.md)。

### 关键模块与文件

Rust 文件都在 `src-tauri/src/` 下；前端契约 `src/features/plan/types.ts` 与 Rust serde 输出逐字段对应。

| 职责 | 文件 |
| --- | --- |
| 配方文档、校验、计划身份 | `recipe.rs`（`RecipeDoc` schema 8、`is_burst`、`same_plan`、阶段 `stage`）、`plan.rs`（`PathSegment`、`PartCalib`、`KindDefaults`、`plan_identity`、`PlanExport`）、`recipe_api.rs` |
| 握手与接件 | `handshake.rs`（`PROTOCOL_VERSION = 8`）、`plc_session.rs`、`inspection.rs`（S7 点表模板）、`cycle/admission.rs` |
| 生产节拍 | `cycle.rs` 接件登记与维护模式、`cycle/burst.rs`（在途件、收尾与判定）、`shot_router.rs` 的 `arm_frames` |
| 帧配组、分段、判定、标定 | `frame_group.rs`、`segmentation.rs`（`Proportional`、`resolve_regions` 过渡区）、`burst_judge.rs`、`calibration.rs`；离线评估 `examples/segment_eval.rs`；逐帧定向与找边评估 `examples/heading_eval.rs` |
| 相机与测量 | `camera.rs`、`frame.rs`、`measure.rs`（`FrameJob`）、`vision.rs` / `vision/taught.rs`（`measure_frame`）、`production.rs`（`burst_graphs`） |
| 采集轮次 | `recipe_capture/burst.rs`（`CaptureRoundV2`） |
| 候选工作区 | `workspace.rs`、`workspace/pieces.rs`（示教件、标定件同步与弃用、旧流程候选）、`workspace/burst.rs`（走廊示教、多轮标定、x_ref）、`workspace/burst_release.rs`（验证与发布、采集版）、`workspace/burst_history.rs`（历史读图与复测） |
| 发布包 | `release.rs`（`publish_burst`，schema 5） |
| 记录、录制、审计 | `store.rs`（`part_frames`，库版本 9）、`recorder.rs`、`audit/` |
| 项目 | `project.rs`（`ActiveProject`、`resolve`、`project_switch`）、`project/manage.rs`、`project/transfer.rs`（导出导入、复制）、`storage_startup.rs`；前端 `src/pages/ProjectsPage.tsx`、`src/features/project/` |
| 离线仿真 | `offline/line.rs`、`offline/scenario.rs`、`offline/sample_set.rs`、`offline/s7_server.rs`、`simimage.rs` |
| 前端 | `src/features/workspace/` 的 `SegmentsPage`、`TeachingPage`（相机走廊示教）、`CalibrationPage`、`TeachingPiecePage`、`HistoryPicker`、`PlanProgress`；`src/features/plan/` |

## 二、未完成事项

- **分段**：比例分段、按段相机与走廊、手动画段和轮次对齐复核已落地，多段配方须各段 N_exp 都已逐段标定才能发布。待完成：
  - 分段标定工作台 P3：运动曲线与按运动自动建议边界（D-SB-44）。
  - 用现场已确认手动分段的轮次运行 `examples/segment_eval.rs`，评估比例分段与图像对齐建议相对手动边界的误差。
  - D-SB-47 逐帧定向与 D-SB-48 ToEdge 已实现，新建的段默认开启逐帧定向，ToEdge 默认不用；D-SB-47 生产开启前须用实际涂胶、画面里有胶嘴的轮次跑 `heading_eval` 复测（NEW-14 空跑数据验证不了）。D-SB-43 由 D-SB-47 的方向细化取代。
  - 每段每台相机的测成率统计与检测相机建议（D-SB-41）。
  - 现场工控机吞吐复测（D-SB-45 按预测段测量与收尾补测后，见下条）。
  - 原生端到端脚本没有按段走廊重跑；多段配方的端到端夹具要按段准备示教帧。
- **3 × 25 Hz 吞吐门槛**：没有达标，也没有被直接验证。门槛口径不改，等现场工控机用真实相机来源跑 `pnpm test:soak:throughput`（并重跑 `throughput_probe` 探针）后，再决定降帧率还是加测量并发（[segment-burst](decisions/segment-burst.md) 第七节）。
- **在线投运**：一期至三期（维护模式采集、历史件导入为采集轮次、试判统计与复核）已实施，只有软件回归覆盖，隔离实例端到端与现场都没有跑过（[在线投运方案](architecture/online-commissioning.md)）。
- **已接受、待确认的取舍**：气泡、拉丝不检；弯道的出胶点短走廊没有真实弯道图验证；提前结束只加 `short_part`、不判 ERR。汇总见 [决策索引](decisions/README.md)。
- **真机验证**：PLC 协议 8、机械臂信号接口、三台相机共用触发、带宽（三路 25 Hz 估算约 826 Mbps）、现场节拍与检测准确率都没有验证。仓库不保存 PLC 程序，PLC 侧按协议 8 规格由现场工程实现。

## 三、目录

| 目录 | 内容 |
| --- | --- |
| [integration/](integration) | [生产通讯协议 8](integration/plc-communication-protocol.md)、[离线样本集格式](integration/sample-set-format.md) |
| [architecture/](architecture) | [配方创建流程](architecture/recipe-creation-workflow.md)、[在线投运方案](architecture/online-commissioning.md)、[项目设计](architecture/project.md)、[存储布局](architecture/storage-layout.md)、[lyFlow 依赖](architecture/lyflow-dependency.md)、[运行日志](architecture/agent-logging.md)与[事件编码表](architecture/agent-logging-codes.md) |
| [decisions/](decisions/README.md) | 现行决策：取舍、回退方式与需要用户确认的事项 |
| [testing/](testing/TESTING.md) | 测试运行与复现、长稳与故障注入运行说明 |
| [deployment/](deployment/PACKAGING.md) | Windows 打包与运行库部署 |
