# GlueSight 文档入口

本页写给接手的 agent：先读这一页建立全貌，细节按链接去看。仓库里的 `docs/` 是设计、协议与决策的唯一正本；对外展示用的页面（例如发给 PLC 方的协议页）从这里生成，不单独维护。代码事实以 main 为准。

## 一、当前设计一览

**场景**：固定胶枪，机器人抓工件（RTCP）从胶嘴下走过，三台相机固定在胶嘴周围，边涂边拍。

### 配方与触发

生产只支持段配方（`RecipeDoc.segments` 非空，配方 schema 8，发布包 schema 4）。配方分阶段：采集版只能在维护模式下采图，正式版正常检测（见 [在线投运方案](architecture/online-commissioning.md)）。

Line0 使用高电平门控，相机按软件设置的帧率连续采集，默认 10 fps。Software 触发只用于单帧标定与调试，不能进入整轮采集或生产。设定、设备回读及实测帧率分别显示。

Jinghang JHEM131GC-XS 要求设备提供 `FrameBurstActive` 或 `AcquisitionActive`，并接受 `LevelHigh` 与固定帧率设置；关键写入与回读失败则不就绪。Software 使用 `FrameStart`。不以未经确认的 `FrameBurstStart` 替代门控。

### 段配方的数据流

1. **机器人**：整条胶路只拉高一次“相机使能”（弯道不拉低，起头、收尾不拍）。
2. **PLC**：按 [生产通讯协议 7](integration/plc-communication-protocol.md)，已布防且运动时向相机输出 Line0 高电平，运动结束拉低并置 plcState=20。PLC 不参与分段，也不报帧数。
3. **三台相机**：高电平期间按内部固定帧率曝光，帧序号取 SDK `frame_num`，不按到达顺序；图像内存按每相机字节预算封顶（`FramePool`，256 MiB），预算满或通道满的帧记缺帧。
4. **帧配组**：seq = frame_counter − 该相机基线，同一 seq 的三帧为一个帧组；缺帧的组标 incomplete；单台相机帧数只用于丢帧告警（与另外每一台相机相差 > 1）。帧数 = 帧组数。每台相机下一件的基线 = 本件见到的最高帧号。
5. **分段（现为未分段）**：`Segmenter` 接口只有 `Unsegmented` 实现，整条胶路是一个未分段区域：只跑每帧检测、不判胶偏，只核对整件帧组数 ∈ ΣN_exp ± Σtol。
6. **每帧测量**：每台相机的每一帧沿这台相机示教的走廊用 lyFlow（`glue.taught_path` + `glue.bead_width`）测胶宽、断胶与胶中心偏移 x，输出 `FrameMetrics`；测量队列满、超时记 Unmeasured，走出走廊记 OutOfCorridor，都只算检测不完整。
7. **区域判定**（`burst_judge`）：每帧胶宽、断胶、溢胶（NG_WIDTH）三类区域都判；胶偏只在直线段，去首尾各一帧后用 x 均值与标准件 x_ref（只实现 AbsoluteX）判整段偏移、最大偏差判局部跑偏；三台相机取最差。`flags`（检测不完整、帧数异常、`short_part` 疑似提前终止）、未检区域、未检项（气泡、拉丝）与“胶偏未判”原因都不改 OK/NG；只有三台相机整件没有一帧测成才判 ERR 91。
8. **落库与历史**：记录库版本 9，`part_frames` 每台相机每帧一条（缺帧也有），`parts.burst_judgement` 存整份区域判定，`parts.mode` 区分正常件与维护件；原图按帧录制为 `{相机}_s{seq:06}.pgm`；审计 spool 格式 4；历史按 (计划相机, seq) 读图、复测与重判。

配方的建立流程（候选工作区）：整圈采集 → 段列表 → 每台相机的走廊示教 → 多轮标定（默认 ≥ 10 轮，得 ΣN_exp、建议容差由操作员确认、标准件轮次得 x_ref）→ 独立样本验证 → 发布。采集完成后支持轮次手动分段、模板套用和边界复核，以及带原因的帧组屏蔽与恢复。见 [配方创建流程](architecture/recipe-creation-workflow.md)。

### 协议 7 要点

- 双方各一个状态字握手（plcState / pcState），PC 回显 `seqEcho` / `snEcho`；PLC 只报型号、序号与 SN，不下发计划或拍照数量，重新标定不用改 PLC。
- 任何结果都不停线；PC 不在线时 PLC 不等待，本件记未检。复位可由 PLC 现场发起（plcState=99），也可在软件发起（pcState=99，PLC 回空闲后 PC 置就绪）。
- 维护模式 `pcMode=1`：结果码写 0，PLC 按未检放行；切换从下一件生效。
- 整圈示教使用独立 DB6 / DB12，见 [示教采集接口 V1](integration/teaching-capture-sequence.md)。

### 项目

一套可上线的检测方案（相机组、PLC 配置、标定、配方、候选、记录）收拢成一个项目，存于 `projects/<id>/`；支持新建、重命名、删除、整个项目导出导入与复制，切换项目要重启。见 [项目设计](architecture/project.md) 与 [存储布局](architecture/storage-layout.md)。

### 关键模块与文件

Rust 文件都在 `src-tauri/src/` 下；前端契约 `src/features/plan/types.ts` 与 Rust serde 输出逐字段对应。

| 职责 | 文件 |
| --- | --- |
| 配方文档、校验、计划身份 | `recipe.rs`（`RecipeDoc` schema 8、`is_burst`、`same_plan`、阶段 `stage`）、`plan.rs`（`PathSegment`、`PartCalib`、`KindDefaults`、`plan_identity`、`PlanExport`）、`recipe_api.rs` |
| 握手与接件 | `handshake.rs`（`PROTOCOL_VERSION = 7`）、`plc_session.rs`、`inspection.rs`（S7 点表模板）、`cycle/admission.rs` |
| 生产节拍 | `cycle.rs` 布防分流与维护模式、`cycle/burst.rs`（在途件、收尾与判定）、`shot_router.rs` 的 `arm_frames` |
| 帧配组、分段、判定、标定 | `frame_group.rs`、`segmentation.rs`、`burst_judge.rs`、`calibration.rs` |
| 相机与测量 | `camera.rs`、`frame.rs`、`measure.rs`（`FrameJob`）、`vision.rs` / `vision/taught.rs`（`measure_frame`）、`production.rs`（`burst_graphs`） |
| 示教采集轮次 | `recipe_capture/burst.rs`（`CaptureRoundV2`） |
| 候选工作区 | `workspace.rs`、`workspace/burst.rs`（走廊示教、多轮标定、x_ref）、`workspace/burst_release.rs`（验证与发布、采集版）、`workspace/burst_history.rs`（历史读图与复测） |
| 发布包 | `release.rs`（`publish_burst`，schema 4） |
| 记录、录制、审计 | `store.rs`（`part_frames`，库版本 9）、`recorder.rs`、`audit/` |
| 项目 | `project.rs`（`ActiveProject`、`resolve`、`project_switch`）、`project/manage.rs`、`project/transfer.rs`（导出导入、复制）、`storage_startup.rs`；前端 `src/pages/ProjectsPage.tsx`、`src/features/project/` |
| 离线仿真 | `offline/line.rs`、`offline/scenario.rs`、`offline/sample_set.rs`、`offline/s7_server.rs`、`simimage.rs` |
| 前端 | `src/features/workspace/` 的 `SegmentsPage`、`TeachingPage`（相机走廊示教）、`CalibrationPage`、`CapturePage`、`PlanProgress`；`src/features/plan/` |

## 二、不再采用的做法

以下做法已经放弃，不要重新引入：

| 不采用 | 现行做法 |
| --- | --- |
| 拍照点配方（逐点停稳拍、按 Pose 选检测图、特征定位判胶偏） | 段配方：段列表 + 每台相机一条走廊，每帧都测，胶偏按段级 AbsoluteX |
| 一台三目设备“一帧内三视角” | 三台独立单视角相机共用一个触发，各出 1 帧；三目拼接配置不能参与生产 |
| PLC 下发计划号、拍照数量或参与分段 | PLC 只报型号、序号、SN；帧数与分段由 PC 负责 |
| 分段置信度不足时回退到计划边界 | 该边界两侧两段合并为未分段区域，不判胶偏，标“胶偏未判” |
| 预期帧数取多轮最小值、多出的帧裁掉 | 多轮帧组数均值四舍五入作 N_exp，所有帧都测，越界只告警 |
| 项目热切换 | 切换项目时重启（开发机实测约 1.1 s） |

## 三、未完成事项

- **分段算法**：没有开发。要等现场按量产曝光与速度拍的含弯道三相机图像；在此之前整条胶路按未分段处理；多段配方的各段 N_exp 只能由已确认的手动分段逐段标定得出，未确认时凭整件 ΣN_exp 发布与运行。
- **3 × 25 Hz 吞吐门槛**：没有达标，也没有被直接验证。门槛口径不改，等现场工控机用真实相机来源跑 `pnpm test:soak:throughput`（并重跑 `throughput_probe` 探针）后，再决定降帧率还是加测量并发（[segment-burst](decisions/segment-burst.md) 第七节）。
- **在线投运二期、三期**：历史件导入为采集轮次、试判统计（[在线投运方案](architecture/online-commissioning.md)）。
- **已接受、待确认的取舍**：胶偏的 `ToEdge` 测量量未实现；气泡、拉丝不检；“每台相机一条按直线段几何的走廊”没有真实弯道图验证；提前结束只加 `short_part`、不判 ERR。汇总见 [决策索引](decisions/README.md)。
- **真机验证**：PLC 协议 7、三台相机共用触发、带宽（三路 25 Hz 估算约 826 Mbps）、现场节拍与检测准确率都没有验证。仓库不保存 PLC 程序，PLC 侧按协议 7 规格由现场工程实现。

## 四、目录

| 目录 | 内容 |
| --- | --- |
| [integration/](integration) | [生产通讯协议 7](integration/plc-communication-protocol.md)、[示教采集接口 V1](integration/teaching-capture-sequence.md)、[离线样本集格式](integration/sample-set-format.md) |
| [architecture/](architecture) | [配方创建流程](architecture/recipe-creation-workflow.md)、[在线投运方案](architecture/online-commissioning.md)、[项目设计](architecture/project.md)、[存储布局](architecture/storage-layout.md)、[lyFlow 依赖](architecture/lyflow-dependency.md)、[运行日志](architecture/agent-logging.md)与[事件编码表](architecture/agent-logging-codes.md) |
| [decisions/](decisions/README.md) | 现行决策：取舍、回退方式与需要用户确认的事项 |
| [testing/](testing/TESTING.md) | 测试运行与复现、保留测试清单、长稳报告 |
| [deployment/](deployment/PACKAGING.md) | Windows 打包与运行库部署 |
