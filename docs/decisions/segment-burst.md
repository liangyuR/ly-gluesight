# 分段连拍飞拍决策记录（segment-burst）

段配方（分段连拍飞拍）是唯一的生产模型，本文是它的现行基准：规格与代码冲突时采用的做法（第四节 D-SB-n）、前后端 IPC 契约（第五节）、口径（第六节）、内存与队列（第七节）和实现要点（第九节）。PLC 握手与现场接线以 [生产通讯协议 7](../integration/plc-communication-protocol.md) 为准，整圈示教握手见 [示教采集接口 V1](../integration/teaching-capture-sequence.md)。

依据：分段连拍飞拍方案第 18 版（2026-10-22，下称“规格”）与实现决策补充（2026-10-22，用户已确认，下称“决策补充”）；两者冲突时以决策补充为准。契约类型在 `src-tauri/src/{plan,frame_group,segmentation,burst_judge,calibration}.rs` 与 `src/features/plan/types.ts`（字段与 Rust serde 输出逐一对应）。

## 一、采集模型

- 三台独立单视角相机（D-SB-11），Line0 高电平期间按软件设定的固定帧率连续采集，默认 10 fps；PLC 只门控采集使能，不产生逐帧脉冲。Software 触发只用于单帧标定与调试。
- 帧按各相机 SDK 帧号（`frame_counter`）配组（D-SB-10、第六节）；帧数告警只比较相机之间，不代表曝光同步。
- PLC 报运动结束（plcState=20）后，PC 等满收尾窗口（`drain_ms`，默认 1000 ms）、已接受的帧都测完，再判定并回写结果；下一件须等当前件释放。
- 分段算法只有 `Segmenter` 接口与 `Unsegmented` 实现：运行时整条胶路按“未分段”处理，只跑每帧独立检测、不判胶偏；算法要等现场按量产曝光与速度拍的含弯道三相机图像序列再开发验证。

## 二、多段配方的发布与运行

- **发布门槛**：整件 ΣN_exp（`expected_total`）由多轮采集得出即可发布；各段 `expected_frames` 允许为 null。
- **`expected_total` 的来源**：标定轮次统计整件帧组数的均值（四舍五入），存为配方的 `part_calib: Option<PartCalib>`；各段均已标定时等于各段 N_exp 之和（`plan::expected_total(segments, part_calib)`）。
- **逐段标定**：多段计划只有每个标定轮次都在整圈采集复核里确认了手动分段时，才按边界逐段写 N_exp；否则各段保持待标定（D-SB-21）。
- **运行时（Unsegmented）**：整条胶路是一个未分段区域，只跑每帧独立检测，不判胶偏；只核对整件帧组数 ∈ `expected_total ± tolerance_total`，越界只告警。
- 协议 7 的 PLC 只报型号、序号与 SN，ΣN_exp 只用于 PC 侧的帧数核对，重新标定不用改 PLC。

## 三、已接受的默认做法

- **默认做法 1**：**溢胶**按“每帧最大胶宽超过溢胶上限（`overflow.max_width_mm`）”判，结论 NG，结果码沿用 NG_WIDTH（14），原因写“溢胶”。**气泡、拉丝不检**，结果中标注“未检项”（`BurstJudgement.unchecked`）。
- **默认做法 2**：**胶偏只实现 `AbsoluteX`**；`ToEdge` 保留在模型里，选了它的段结果标“胶偏未判：测量量未实现”（`OffsetUnjudged::MeasureNotImplemented`），x_ref 也不计算。
- **默认做法 3**：**提前结束**（PLC 提前报运动结束）：PC 用已收到的帧照常判 OK/NG；帧组数 < ΣN_exp − Σtol 时加 `short_part`（疑似提前终止）标记，历史中单独显示（`PartSummary.flags`）。
- **默认做法 4**：**每台相机示教一条按直线段几何的走廊**；弯道帧走出走廊记“检测不完整”（`FrameState::OutOfCorridor`），不判 NG。判据没有经过真实弯道图像验证。
- **默认做法 6**：**重新标定使 N_exp / expected_total 变化时分配新的 `plan_version`**（二者都进入计划身份，见 D-SB-19）；计划版本是 PC 侧发布包与记录的身份，不下发 PLC。
- **默认做法 7**：三台相机**取最差**；帧序号取 SDK **`frame_counter`**；三台相机整件**全无帧判 ERR 91**；采集失败的异常码为 98 / 99（D-SB-33）；**容差默认 1**。

## 四、冲突与决策

每条写问题、采用的做法与理由；问题里的文件行号指当时的代码。

### D-SB-1 计划模型：段列表

- 问题：原模型 `RecipeDoc.shots: Vec<ShotSpec>`（id/pose/camera/view）按触发分组编计划；规格 §1 改为段列表。
- 采用：`plan::PathSegment`（Rust、TS 同名，避开 `recipe::Segment`）。`RecipeDoc` schema 8 有 `segments`、`inspect: Vec<CameraInspect>`（有序，计划相机顺序取它）、`segment_defaults: KindDefaults`、`calibration_rounds`（默认 10）、`part_calib: Option<PartCalib>`。`segments` 非空即段配方，要求 `shots` 为空。`segments` 为空的配方（旧拍照点配方，从未部署）不能开工、不能发布（“旧拍照点配方已停用，请新建段配方”），遗留类型不再维护。
- 回退：不计划。

### D-SB-2 帧数只告警，接件不核对数量

- 问题：规格 §2 帧数在 N_exp ± tol 内正常，超出只告警。
- 采用：接件只核对型号与计划能否编出（[core-flow.md](core-flow.md) D-CF-4），不核对拍照数量。运行时按区域比较帧组数与 N_exp ± tol（未分段时比较 `expected_total ± tolerance_total`），越界记“帧数异常”告警，不改结论。记录库按实际帧落库。

### D-SB-3 区域帧数口径：帧数 = 帧组数

- 问题：规格 §2 每段取各轮帧数均值作 N_exp，运行时每段帧数与 N_exp ± tol 比较，但没说三台相机各自的帧数怎么合成一个“帧数”。
- 采用：标定和运行同一口径——帧数 = 帧组数，即区域内 seq 的个数（`last_seq − first_seq + 1`）；整件或整轮的帧组数 = 各相机最大 seq 的最大值。单台相机的帧数只用于丢帧告警（与另外每一台相机都相差 > 1），不参与容差比较。详见第六节。
- 理由：某台相机缺帧时帧组数不变，结论与告警分开；标定与运行用同一个量，N_exp 才可比。

### D-SB-7 下一件基线：每台相机各取自己见到的最高 frame_counter

- 问题：规格 §3 帧序号取 SDK 帧号；PLC 不报触发计数。
- 采用：每台相机下一件的帧计数下限 = max(该相机原基线, 该相机本件见到的最高 frame_counter)（`Grouper::max_counter` → `Ledgers::set_frame_floor`，记在 `Ledger.frame_floor`，布防时由 `shot_router::arm_frames` 与本会话最高帧计数取大）。故障复位（`Ledgers::clear_floors`）清掉下限，之后只按实际见到的计数布防。
- 理由：相机偶发丢末帧（已计数、帧没送到）只会让下一件出现两个不完整帧组，且下一件后自动对齐。

### D-SB-8 示教采集数量与内存

- 问题：原示教轮次 `MAX_TRIGGERS = 64` 同时是写盘队列容量，PLC 计划数限 1–64，收图超过计划即失败，写 png 且重复写；规格 §5 取消上限，示教轮次不核对计数。
- 采用：示教轮次（`CaptureRoundV2`）不设上限、不核对计数。写盘队列条数上限 `MAX_FRAMES_PER_CAMERA = 8192`，图像内存由 FramePool 每相机字节预算封顶；预算满或队满时该帧记缺帧并告警，整轮不判失败。图像写无压缩 PGM，一帧一个文件；`round.json` 只在状态变化时重写，帧元数据追加写 `frames.jsonl`。

### D-SB-9 示教采集完成判据

- 问题：规格 §5 不再用计数做结束校验，§3 缺帧只告警。
- 采用：三条都满足即 Complete——收到 PLC 采集结束、收尾窗口到期、已入队的帧全部写完。各相机帧数、不完整组和丢帧告警写进轮次记录（`CaptureRoundV2`）。接收中不设超时，只有等待开始超时与写盘超时。

### D-SB-10 帧序号来源：frame_counter

- 问题：规格 §3 帧序号取 SDK 帧号；`camera.rs` 的 frame_counter 是“有 Chunk 帧计数用它，否则用 nFrameNum”。
- 采用：`camera.rs` 的 frame_counter 取值符合规格，不改。示教轮次与生产都以 `Frame.frame_counter − 该相机基线` 作 seq；生产的 Policy 信任 ChunkFrame、SdkFrame、ChunkTrigger 三种来源的 frame_counter，离线另外信任 Synthetic。trigger_counter 只作诊断。

### D-SB-11 设备模型：三台独立单视角相机

- 问题：原相机配置 `view_count ∈ {1,3}`，三视角靠 `composite_layout` 拆拼接图；规格 §0 一次采集 = 三台独立相机各出 1 帧，§1 Plan.cameras = 3 台设备 id。
- 采用：段配方的计划相机必须是 3 个互不相同、单视角（`view_count = 1`、无 `composite_layout`）的设备；不满足时段配方校验或布防报错。相机配置本身仍允许三目拼接，标定页对单视角相机不选视角。
- 回退：若决定彻底停用三目拼接，在相机配置加载处拒绝。

### D-SB-12 所有帧都测

- 问题：原模型同 Pose 组只检所选的一张；规格 §0 所有帧都参与每帧独立检测，不裁帧。
- 采用：每台相机的每一帧都测，没有组选图。

### D-SB-13 缺帧、多帧、未完成的结论

- 问题：原模型缺帧判 ERR 91，多帧 / 超计划判 ERR 96，未完成判 ERR 99；规格 §0/§3 某台缺帧时该组照常判，组和整件标“检测不完整”，不影响 OK/NG，帧数超出容差只告警。
- 采用：缺帧（含回调通道满、内存预算满、测量队列满、测量失败、走出走廊）→ 组标 incomplete，整件 `flags.incomplete`；帧数越界 → `frame_count_anomaly`；都不改 OK/NG。只有三台相机整件一帧都没有时才判 ERR 91；会话变化判 ERR 98。96 不再使用，常量保留。

### D-SB-14 三台相机结论取最差

- 问题：规格 §0 所有帧都参与独立检测。
- 采用：任一相机任一帧 NG，所在区域即 NG，整件取最严重结论。

### D-SB-15 胶偏判定：段级 AbsoluteX，ToEdge 暂不实现

- 问题：原模型逐站与示教中线比对，以及特征定位；规格 §4 直线段去首尾各一帧，用 x 均值 − x_ref 判整段偏移、最大偏差判局部跑偏，中线逐点比对停用；§0/§1 有稳定边缘用 ToEdge，否则 AbsoluteX。
- 采用：每帧输出 `x_mm`（帧内胶中心相对走廊的法向偏移均值，即 AbsoluteX）。胶偏只在 Straight 区域按段判（`burst_judge`）。ToEdge 在模型中保留，结果标“胶偏未判：测量量未实现”（默认做法 2）。不做中线逐点比对与特征定位。
- 回退：若要实现 ToEdge，需另定参考边缘的示教方式。

### D-SB-16 胶宽均匀性

- 问题：规格 §4 直线段胶宽 / 均匀性按段级 min/max/mean 检，弯道用弯道允差，未分段用较宽允差。
- 采用：`WidthCheck` 有 `max_range_mm: Option<f64>`（区域内各帧 `width_mean` 的极差上限，默认 None 即不检），只在 Straight 区域判，超限 NG_WIDTH，原因“胶宽不均”。Corner 和 Unsegmented 不判极差，只判每帧胶宽。每帧胶宽按 `widths × spacing_mm` 算帧内超差长度，不超过 `max_excursion_mm` 为 OK_WITH_EXCURSION。默认胶宽 4 ± 1.5 mm、帧内允许超差 5 mm（`plan::KindDefaults`，依据见 [r2-algorithm.md](r2-algorithm.md) RA-5）。

### D-SB-17 检测走廊：每台相机一条按直线段几何的走廊

- 问题：规格 §4 中线逐点比对停用，保留边缘提取。
- 采用（默认做法 4）：每台相机示教一条按直线段几何的走廊（`CameraInspect.corridor`），所有帧共用，只决定 ROI 和测站；弯道帧胶条走出走廊时该帧记 `OutOfCorridor`（检测不完整），不判 NG。全流程验收用合成直线 / 弯道三相机；真实样本集是停稳拍数据，只做冒烟。

### D-SB-18 胶偏参考值 x_ref

- 问题：规格 §0/§1 `offset_ref.x_ref` 取标准件多轮测量均值，只用于 Straight 段。
- 采用：`x_ref` / `spread` 由标记为标准件的采集轮次自动算出，不能手填。`OffsetRef` 有 `camera` 字段（三台相机几何不同，不能共用 x_ref），默认取计划第一台相机。未标定或 ToEdge 时 `x_ref`、`spread` 为 null。

### D-SB-19 计划版本身份

- 问题：规格 §1 段序列、类型、相机绑定变化分配新版本，限值、容差、x_ref 变化沿用。
- 采用：段身份 = `product_code` + 各段 `(index, kind, expected_frames)` + 有序 cameras + `expected_total`（`plan::plan_identity`）。限值、容差、x_ref、inspect_range、length_mm 不参与。`same_plan` 按它判断是否沿用 `plan_version`。
- 理由：多段配方各段 N_exp 可能为 null，只有 `expected_total` 能反映重新标定（默认做法 6）。

### D-SB-20 就绪与发布前置

- 问题：规格 §1 新建时 expected_frames = null 标“待标定”，未标定不能发布，§0 N_exp 不能手填。
- 采用：`Recipe::ready_burst`：3 台相机都示教了走廊和当量，且 `expected_total` 非空（第二节：各段 N_exp 可以为 null）。`update_doc` 直接修改 `expected_frames`、`calib`、`part_calib` 时拒绝；段的序列或 kind 改变时清空全部 N_exp、calib、`part_calib`、x_ref。

### D-SB-21 没有分段算法时的标定

- 问题：规格 §3 算法验证前整条胶路按未分段，§2 每段 N_exp 由多轮采集得出，§1 未标定不能发布；`Unsegmented` 只能给出整件帧组数。
- 采用（决策补充第二节）：每轮帧组数做 `calibrate_counts`，结果写 `part_calib`。单段计划同时写该段 `expected_frames` 和 `calib`；多段计划在每个标定轮次都确认了手动分段时按边界逐段标定（各段 N_exp 之和即 ΣN_exp），否则各段保持 null，界面标“待标定”，但凭 `expected_total` 可以发布。

### D-SB-22 检测范围与未检区域

- 问题：规格 §0/§1 每段写明 inspect_range [a_mm, b_mm]，结果页标“未检区域”。
- 采用：`PathSegment.inspect_range` 显式配置；`burst_judge` 按配方算出 `[0, a)` 与 `(b, length_mm]` 放进 `BurstJudgement.uninspected`，段长未知时 `to_mm` 为 null，界面按 mm 展示。

### D-SB-23 触发方式固定为 Fly

- 问题：配方有 `TriggerMode` Fly | Stop；规格 §0 只有飞拍。
- 采用：段配方固定写 Fly，界面不展示触发方式，离线节拍按帧率，不读 `trigger_mode`。`RecipeSummary.trigger_mode`、`parts.trigger_mode` 列保留。

### D-SB-24 新建模板：三台相机、一个直线段

- 问题：规格 §1 cameras = 3 台，容差默认 ±1，直线与弯道分别设。
- 采用：新建模板取相机组前三台（不足 3 台也建，但无法发布），附一个 Straight 段，`expected_frames = null`。`segment_defaults` 的 serde 默认值即 `KindDefaults::default()`（straight、corner 的 tolerance 都是 1），`new_segment` 从这里取。

### D-SB-25 计划导出

- 问题：规格 §1 要有给 PLC / 机器人工程师的计划 JSON：段序列、各段 N_exp、容差、ΣN_exp。
- 采用：`plan::PlanExport` 与命令 `recipe_plan_export`；工作区视图带 `WorkspaceView.plan.productionExport / candidateExport`，旧字段 `productionPlc / candidatePlc` 为 null。配方总览不设“PLC 对接参数”面板：协议 7 的 PLC 只需型号，已发布配方的型号与 ΣN_exp 看配方库卡片摘要。

### D-SB-27 离线仿真

- 问题：规格 §6 要求使能电平、按帧率出帧、首尾 ±1、弯道减速波动、三台相机之一缺帧。
- 采用：离线产线按剖面定频出帧（每段帧数 + 种子化波动），丢帧按相机独立设置；有本件未检、提前结束等工况；`offline_capture` 支持合成图相机，合成图离线环境能完成多轮标定和发布。细节见第九节第 14 条。

### D-SB-28 样本集 v1

- 问题：规格 §2/§3 帧数波动、按帧号配组。
- 采用：样本集保持 v1 格式，解释为“每台相机按 shotId 顺序的帧序列”（`Manifest::camera_frames(camera)`，见 [样本集格式](../integration/sample-set-format.md)）。

### D-SB-29 检测记录结构

- 问题：规格 §3/§4/§6 要记录每台相机每一帧、帧组、段、不完整组、未检 / 未分段区域。
- 采用：`part_frames` 表与 `parts` 的段配方列（ΣN_exp、Σtol、各相机帧数、整件标记、整份区域判定 `burst_judgement`），`inspection.db` 版本 9（D-0：版本不符时备份后重建）。
- `part_frames` 主键 (part_id, camera, seq)，入库时帧数不要求等于预期，但 (camera, seq) 唯一、相机属于计划相机、指标身份与帧一致，摘要结论与区域判定相同；原图引用按 (camera, seq) 只增不换。
- 审计 spool FORMAT 4，原图引用 `files[].frame` 有值即段配方帧。录制文件名 `{相机}_s{seq:06}.pgm`，part.json 有 `missingFrames`（相机没出图，只追溯，录制仍可完整）与 `rawDropped`（录制队满没录，证据 Incomplete）。
- 历史重判 `Overrides` 有 `straight` / `corner`（按段类型覆盖胶宽上 / 下允差与帧内允许超差长度，`maxGapLen` 作用于每帧）；用当前候选重判时计划身份或走廊、当量、标定、检测参数变化跳过并记 LayoutChanged，重判结果 `changes[].flags` 带出整件标记。CSV 末尾有段配方的帧数与标记列；时间线 Entry 与运行日志字段有 `seq`。

### D-SB-30 发布包与标定键

- 问题：规格 §0 三台独立设备，没有视角。
- 采用：`release::publish_burst` 按相机组织，写 schema 4；段配方的标定键为设备标定 `device/{device}`（`vision::CalibKey::Device`，文件 `calib/device/plane_calib_{device}.json`）。
- 标定页（`CalibPanel`、飞拍标定页）对任何相机都带图编号，单视角相机为 1。`vision_calibrate` 对单视角相机的图 1 同时写 `{device}-v1` 与设备标定，两份内容相同；三目拼接相机的图 N 只写 `{device}-v{N}`；不带图编号只写设备标定。`vision_calib_info` 带图编号读 `{device}-v{view}`，不带读设备标定。没有回退读取：只存过 `{device}-v1` 的单视角相机，要在标定页重新标定一次才会生成设备标定。
- 设备标定放单独子目录 `calib/device/`，与工位标定的扁平文件名 `plane_calib_{键}.json` 永不同名（相机编号允许 `-`，`cam1-v1` 与 `cam1` 的图 1 会撞名）。发布包 `calibration.json` 的 `reference` 对设备标定记 `device/{device}`，显式标定引用仍记引用本身。

### D-SB-31 隐藏上限

- 问题：原模型 `PathPoints.k: Vec<u8>`，配方最多 64 个拍照点；规格 §0 10～25 Hz，帧数远超 64。
- 采用：帧序号用 u64；ΣN_exp 不超过 32767（`plan::MAX_EXPECTED_TOTAL`，也是帧组 seq 的默认上限），单段 tolerance ≤ 1000。

### D-SB-32 队列容量、内存与背压

- 问题：原 FRAME_QUEUE = 64、测量队列 32、录制队列 48，FramePool 不限在途数量；规格 §0 不裁帧，所有帧都测。
- 采用：图像内存按字节封顶，队列只按条数防失控，详见第七节。录制队满时该帧原图缺失，part.json 列出，不影响结论。

### D-SB-33 采集失败异常码

- 问题：规格 §5 计数不符不再是故障。
- 采用（默认做法 7）：示教轮次失败时，相机断连或会话变化记 98（设备丢失）；PLC 开始 / 结束顺序异常、等待开始超时、写盘超时、操作员中止和其他原因记 99（`CaptureFailureV2::fault_code`）。

### D-SB-34 本件未检与提前结束

- 问题：规格 §0 PLC 不提交请求（或提前终止）时整件按未检处置。
- 采用（默认做法 3）：PLC 不提交请求时 PC 没有记录，由 PLC 记未检。提前结束由 PLC 提前报运动结束表达，PC 用已收到的帧照常判，帧组数 < ΣN_exp − Σtol 时加 `flags.short_part`（疑似提前终止），历史中单独显示，不改 OK/NG。在途 PLC 进入故障或复位时，本件按中断记 ERR 98（[core-flow.md](core-flow.md) D-CF-6）。
- 残余风险：可能出现 PLC 记了未检、PC 记录却写 OK 的情况；需要时可改为 short_part 判 ERR。

### D-SB-35 示教轮次的一致性校验

- 问题：原示教轮次要求 PLC 计划版本 > 0，样本和历史必须与示教轮次的计划版本一致；规格 §5 示教轮次不核对。
- 采用：示教轮次不核对 PLC 计划版本与数量（示教接口 V1 不传这两项，`plcPlanVersion / plcShotCount` 为 null）；样本、标定轮次与示教的一致性比较配方计划身份和相机配置标签。

### D-SB-36 多轮标定

- 问题：规格 §2 同一程序连续采集 ≥ 10 轮（轮数可配），每段均值四舍五入作 N_exp。
- 采用：Workspace 有 `calibration { rounds }`（每轮带 `standard`，标准件轮次不计入轮数；标定结果写进配方，视图由配方与轮次算出，见第九节第 12 条）；`calibration_rounds` 默认 10，可配 10..=100。标定结果见 D-SB-21，容差建议由操作员确认后才写入。

### D-SB-37 溢胶、气泡、拉丝

- 问题：规格 §0/§4 三类区域都检溢胶 / 气泡 / 拉丝。
- 采用：默认做法 1。

### D-SB-38 进度步骤

- 问题：规格 §6 采集 → 走廊 → 标定 → 验证 → 发布，按段展示确认。
- 采用：`StepIdV2 = capture|segments|corridor|calibration|validation|publish`（5.3）。

## 五、IPC 契约

完整字段以 `src/features/plan/types.ts` 为准（serde camelCase，Rust 的 `Option` 一律序列化为 null）。段配方的数据挂在现有 DTO 的可空字段上，下表列出段配方时旧字段的取值。

### 5.1 现有 DTO 的增量

| DTO（Rust 位置） | 段配方字段 | 段配方时旧字段的取值 |
| --- | --- | --- |
| `RecipeDoc`（recipe.rs，schema 8） | `segments`、`inspect`、`segmentDefaults`、`calibrationRounds`、`partCalib`（TS `RecipeDocSegmentFields`） | `shots = []`，`triggerMode = "fly"` |
| `RecipeSummary`（cycle.rs） | `segmentCount`、`expectedTotal`、`toleranceTotal`、`calibrated`（TS `RecipeSummaryV2`） | `shotCount = 0`；`triggerCount = expectedTotal`；`length = Σ lengthMm`（未知按 0） |
| `PartView`（cycle.rs） | `burst: PartViewBurst \| null` | `n = expectedTotal ?? 0`；`received` = 各相机帧数之和；`triggers = 0`；`queue = pendingMeasure`；`filled = total = 0`；`frames = []`；`measuredFrames` 与 burst 相同 |
| `ResultView`（cycle.rs） | `burst: BurstJudgement \| null` | flatten 的 Judgement：`verdict/plcCode/faultCode/reason` 与 burst 相同，`segments = gaps = []` |
| `PartSummary`（store.rs） | `flags: Flags \| null`（TS `PartSummaryV2`） | `framesExpected = expectedTotal ?? 0`；`framesReceived` = 帧组数；`triggerMode = "fly"` |
| `PartDetail`（store.rs） | `burst: PartDetailBurst \| null` | `frames = shots = []`，`points = null`，`judgement` 同 ResultView 规则 |
| `CaptureRound`（recipe_capture.rs） | — | 示教轮次为 `CaptureRoundV2`（`format: 2`） |
| `WorkspaceView`（workspace.rs） | `plan: WorkspacePlanView \| null` | `productionPlc = candidatePlc = null`，`candidatePlcError = null`，`coverage = null`，`progress = { steps: [], shots: [], next: null }`（段配方进度在 `plan.progress`）；`planChanged` 按 D-SB-19 的身份 |
| `Progress`（workspace/progress.rs） | — | 段配方用 `ProgressV2` |
| `Overrides`（history.rs，重判试算参数） | `straight`、`corner`（`KindOverride`：`tolUpper`、`tolLower`、`maxExcursionLen`，绝对限不适用；TS `Overrides` 已有这两个可选字段） | 每帧胶宽只读它们（`Overrides::apply_segments`），`position` / `width` 不适用，未分段区域取两者中较宽的一档；前端 `RejudgeDialog` 的直线段 / 弯道段两行可填 |
| `Comparison`（workspace.rs，候选 / 原包对照） | `burst: {segments, cameras, expectedTotal, judgement} \| null`（TS `Comparison.burst`） | `judgement` 为结论摘要（verdict / plcCode / faultCode / reason 与 `burst.judgement` 相同，segments、gaps 为空），`measurements = []` |

### 5.2 新 DTO

- **`PartViewBurst`**（在线在途件）：`cameras`、`expectedTotal`、`toleranceTotal`、`deviceTriggers`（保留字段，协议 7 不报，为 null）、`groupCount`、`cameraFrames`、`measuredFrames`、`pendingMeasure`、`groups: FrameGroupView[]`。
- **`FrameGroupView`**：`seq`、`incomplete`、`cells: FrameCellView[]`（按计划相机顺序）。`FrameCellView`：`status`（`waiting | measuring | FrameState`）、`verdict`（该帧单独的结论，未测完为 null）、`frameCounter`。
- **`PartDetailBurst`**（历史详情）：`segments`（本件配方快照的段）、`cameras`、`expectedTotal`、`toleranceTotal`、`deviceTriggers`（同上）、`cameraFrames`、`judgement: BurstJudgement`、`frames: FrameRecord[]`（按 camera、seq 排序）。
- **`FrameRecord`**：`camera`（计划相机下标）、`seq`、`status: FrameState`、`frameCounter`、`triggerCounter`、`region`（`judgement.regions` 下标，不在任何区域为 null）、`metrics: FrameMetrics | null`（missing 时为 null）、`rawFile`（录制根内相对路径，未录制或队满丢弃为 null）。1..=groupCount 内每个 (camera, seq) 都有一条，缺帧也有。
- **`PartSummaryV2`** = `PartSummary` + `flags`。
- **`CaptureRoundV2`**：`format: 2`、`roundId`、`recipeId`、`cameras: CaptureCamera[]`、`state`（同 `CaptureRound.state`）、`plcShotCount`、`plcPlanVersion`、`deviceTriggers`（这三项为保留字段，示教接口 V1 不传，为 null）、`groupCount`、`cameraFrames`、`incompleteGroups`、`missingFrames: {camera, seq}[]`、`lossAlarms`、`alarms: CaptureAlarm[]`（逐帧告警 `{kind: duplicate | queueFull | budgetDropped | corrupt, camera, seq}`，最多 1024 条）、`createdAt`、`endedAt`、`error`、`failure: CaptureFailureV2 | null`、`simulated`。`CaptureFailureV2`（`kind` 区分）：`camera{cameraId}`、`sessionChanged{cameraId}`（90+98）、`plc`、`startTimeout`、`writeTimeout{queued, written}`、`interrupted`（90+99）、`other`。
- **`CalibrationView`**：`minRounds`、`rounds: CalibrationRoundView[]`、`part: CalibOutcome | null`、`segments: SegmentCalibView[]`、`expectedTotal`、`toleranceTotal`、`planVersionChange`。`CalibrationRoundView`：`roundId`、`standard`、`groupCount`、`cameraFrames`、`incompleteGroups`（个数）、`lossAlarms`（个数）、`createdAt`。`SegmentCalibView`：`index`、`kind`、`outcome: CalibOutcome | null`、`pending`（如“分段算法未就绪”）、`tolerance`（配方当前值）、`xRef: {xRef, spread} | null`、`xRefPending`（如“测量量未实现”）。
- **`WorkspacePlanView`**：`segments`、`cameras`、`segmentDefaults`、`calibrationRounds`、`partCalib`、`expectedTotal`、`toleranceTotal`、`calibrated`（expectedTotal 非空）、`cameraTeaching: CameraTeachingView[]`（`camera`、`saved`、`frame: FrozenImage | null`、`mmPerPx`）、`calibration: CalibrationView | null`、`productionExport`、`candidateExport`、`candidateExportError`、`shotCountChanged`（ΣN_exp 相对生产版本变化）、`progress: ProgressV2`。
- **`RecipeSummaryV2`** = `RecipeSummary` + `segmentCount`、`expectedTotal`、`toleranceTotal`、`calibrated`。
- **`ProgressV2`**：`steps: ProgressStepV2[]`（与 `ProgressStep` 相同，`id` 为 `StepIdV2`，`blockers[].target` 为 `ProgressTargetV2`）、`next: {step, label, target} | null`。`ProgressTargetV2`：`capture`、`segments`、`corridor{camera}`、`calibration`、`validation`、`camera`。
- 镜像的领域类型（`LossReason` 的 `triggers` 分支保留，现在只产生 `cameras`）：`PathSegment` 及其检测项、`PartCalib`、`KindDefaults`、`PlanExport`、`FrameGroup`、`LossAlarm`、`Region`、`SegOutcome`、`FrameMetrics`、`RegionResult`、`Flags`、`Uninspected`、`BurstJudgement`、`CalibOutcome`、`CameraInspect`。

### 5.3 新步骤与路由

| StepIdV2 | 页面 | 路由 |
| --- | --- | --- |
| `capture` | 整圈采集 | `/recipe/capture` |
| `segments` | 段列表与配方默认参数 | `/recipe/segments` |
| `corridor` | 按相机示教走廊 | `/recipe/teach` |
| `calibration` | 多轮标定 | `/recipe/calibration` |
| `validation`、`publish` | 验证与发布 | `/recipe/validation` |

### 5.4 新命令

工作区修改类命令沿用现有约定：参数带 `id` 与 `revision`（乐观并发），返回 `Result<WorkspaceView, String>`。

| 命令 | 参数 | 返回 |
| --- | --- | --- |
| `recipe_plan_export` | `id` | `Result<String, String>`（PlanExport JSON 文本） |
| `workspace_update_segments` | `id, revision, segments: PathSegment[]` | WorkspaceView |
| `workspace_teach_camera` | `id, revision, camera, round, seq, draft` | WorkspaceView |
| `workspace_calibration_add_round` | `id, revision, round, standard: bool` | WorkspaceView |
| `workspace_calibration_remove_round` | `id, revision, round` | WorkspaceView |
| `workspace_calibration_apply` | `id, revision` | WorkspaceView（结果在 `plan.calibration`） |
| `workspace_confirm_tolerance` | `id, revision, segment, tolerance` | WorkspaceView |
| `workspace_extract_corridor` | `id, revision, camera: String, imageId, roi: [u32; 4], guide: BrushGuide?, pointSpacingPx: f32?` | `Result<CenterlineResult, String>`（与 `workspace_extract_centerline` 相同：`path`、`message`、`failure`） |
| `recipe_capture_frame_image` | `roundId, cameraId: String, seq`（帧组序号，从 1 起） | `Result<tauri::ipc::Response, String>`（原始字节，格式同 `recipe_capture_image`）；该相机缺这一帧时报错 |
| `workspace_live_frame_image` | `cycleId, revisionId, camera: usize, seq`（camera 为计划相机下标） | `Result<tauri::ipc::Response, String>`（原始字节，格式同 `workspace_live_image`）；未到或缺帧时报错 |
| `workspace_record_frame_image` | `historyId, camera: usize, seq`（camera 为计划相机下标） | `Result<tauri::ipc::Response, String>`（原始字节，格式同 `workspace_record_image`）；未录制、录制队满丢弃或缺帧时报错 |

- **`recipe_plan_export` 前端不调用**：计划导出随 `WorkspaceView.plan.productionExport / candidateExport` 到达，发布确认框（`PlanPublishBody`）用它们列出候选与生产的对比。命令保留给脚本与工程师工具。
- **相机示教的单帧试测没有命令**：若以后要在相机走廊示教里做单帧试测，由后端先登记命令与 `FrameMetrics` 的返回方式，前端再新增组件。

- **走廊提取**：段配方没有拍照点（`shots = []`），不用按拍照点解析的 `workspace_extract_centerline`，用 `workspace_extract_corridor`：按 `camera`（相机 id）找到 `plan.cameraTeaching` 里这台相机冻结的示教帧，`imageId` 必须等于该帧 id（不等报“图像已变化”），检测极性取这台相机 `inspect.detect`，没有时取配方默认。
- **读图命令的参数口径**：`camera` 在在线与历史命令里是计划相机下标（与 `FrameGroupView.cells`、`FrameRecord.camera` 一致）；示教轮次命令用 `cameraId`（与 `CaptureRound.cameras[].cameraId` 一致）。
- **前端调用**：上表命令前端直接调用（`desktopCall`），失败按普通错误显示。

## 六、口径

- **seq**：`frame_counter − 该相机基线`，从 1 起；frame_counter ≤ 基线为 Stale（上一件迟到帧，不计入本件）；同一相机重复的 seq 为 Duplicate，保留先到的那帧。
- **帧数 = 帧组数**：区域帧数 = `last_seq − first_seq + 1`；整件或整轮帧组数 = 各相机最大 seq 的最大值。标定与运行同一口径。单台相机的帧数只用于丢帧告警：与另外每一台相机都相差 > 1（`frame_group::loss_alarms`；[10, 10, 8] 只有第 3 台告警，前两台彼此一致，不因第 3 台少帧被连带告警），每台相机至多一条。
- **每相机基线**：布防时取各相机本会话见到的最高 frame_counter 与上一件推算的下限中较大者；下一件基线 = max(原基线, 该相机本件见到的最高 frame_counter)（`Grouper::max_counter`，D-SB-7）。
- **帧数越界只告警**：区域帧组数 ∉ [N_exp − tol, N_exp + tol] 记 `frame_count_anomaly`，该区域照常检测，不改 OK/NG。运行时只有覆盖全部段的未分段区域，与 `expected_total ± tolerance_total` 比较。
- **short_part**：整件帧组数 < `expected_total − tolerance_total`（`expected_total` 为 null 时不判）。
- **expected_total**：各段 N_exp 都非空时为其和，否则取 `part_calib.expected_total`，都没有为 null。**tolerance_total** = Σ 各段 tolerance。
- **N_exp**：各轮帧组数均值四舍五入（.5 进位），记录 min/max/mean；建议容差 = max(默认容差, N − min, max − N)，操作员确认后才写入。

## 七、内存与队列

| 项 | 初值 | 说明 |
| --- | --- | --- |
| `IMAGE_BUDGET_PER_CAMERA` | 256 MiB | FramePool 每台相机在途图像字节上限；1280×1024 单字节灰度每帧 1,310,720 B，即 204 帧、25 Hz 下 8.2 s 积压；只作用于 MVS 回调来源。预算满时回调丢弃该帧并记缺帧（不中断回调）。 |
| `FRAME_QUEUE` | 1024 条 | 内存已由预算封顶 |
| `MEASURE_QUEUE`（代码 `FRAME_MEASURE_QUEUE`） | 1024 条 | 满时该帧记 Unmeasured |
| 录制队列 | 1024 条 | 按帧计（`BURST_QUEUE`）；满时该帧原图不录，part.json 记 rawDropped，证据状态 Incomplete |
| 示教写盘 `MAX_FRAMES_PER_CAMERA` | 8192 条 | 满时该帧记 QueueFull 告警，不判失败 |
| `MEASURE_WORKERS` | 4（运行时取 min(4, 逻辑核 − 2)，至少 1） | 7.1 实测回填；现场机必须重跑 7.1 的探针后再确认 |

队列条数只防失控，不决定内存。7.1 实测了 lyFlow 每帧耗时与 PGM 写盘速率，但测量和写盘是分开测的；3 × 25 Hz 吞吐门槛的持续满负载**按相机使能为高的时间窗计**：每件第一帧到达到最后一帧到达为一个窗口（首个帧组不计入），Σ窗口内帧数 ÷ Σ使能时长 ≥ 3 × Hz 的 90%，且至少连续 10 件逐件达到；件间握手间隙单独报告为 `handshakeGapSeconds`，不参与判定。**该门槛目前没有达标，也没有被直接验证：门槛口径不改，等现场工控机真实相机来源复测后再决定降帧率还是加并发**；实测与待决策的选项见第九节第 17 条。

### 7.1 吞吐摸底（实测，2026-10-22）

**怎么跑**（探针是 `src-tauri/examples/throughput_probe.rs`，不是测试，不在 `cargo test` 里；写约 4 GB 临时文件、结束后自动删除，约 1 分钟；已发布的运行库即可；dev profile 构建，与下面记录的数值同一配置）：

```powershell
$env:LYFLOW_CORE_DLL = 'src-tauri\target\dependency-cache\lyflow\1.1.5\lyflow_core.dll'
cargo run --manifest-path src-tauri/Cargo.toml --example throughput_probe
```

**量的是什么**：1280×1024 灰度图、一条横穿的暗胶条；走廊 121 mm、121 个测点（间距 1 mm），只用中线测量的算子（glue.taught_path + glue.bead_width），与走廊测量是同一组算子。经生产加载路径（`release::publish` → `release::load` → `Prepared::load` → `warm`）后用 `Prepared::measure` 计时，包含 `Engine::run` 里的像素拷贝、lyFlow 运行与结果解析。每个并发度测 300 帧。写盘用生产录制的 `image_file::save_pgm`，每张 1,310,720 B 像素 + 17 B 头。

**环境**：Intel Core i5-1135G7 2.40 GHz（4 核 / 8 线程）、15.7 GB 内存、Windows 11 Pro 10.0.26200；lyFlow 1.1.5（`dependency-cache/lyflow/1.1.5`）；测试构建（gluesight 包 opt-level 2、其余依赖未优化，结果解析比发布构建慢一些，数值偏保守）；写盘目录是系统盘的 `%TEMP%`，没有排除杀毒软件的影响。共跑 3 轮，下表为各轮的范围。

**测量耗时**（每个线程数 300 帧）：

| 测量线程 | 轮数 | 单帧 p50（ms） | 单帧 p95（ms） | 单帧 max（ms） | 吞吐（fps） |
| --- | --- | --- | --- | --- | --- |
| 1（串行） | 3 | 15.9–18.1 | 26.4–30.3 | 30.5–34.5 | 51.5–56.8 |
| 2 | 3 | 25.3–25.9 | 34.0–37.2 | 43.7–160.0 | 74.0–78.3 |
| 3 | 1 | 28.7 | 46.6 | 85.6 | 99.0 |
| 4 | 3 | 32.6–36.7 | 56.0–63.3 | 118.2–163.7 | 101.1–112.4 |
| 6（逻辑核 − 2） | 3 | 41.1–51.6 | 68.3–93.1 | 140.5–201.7 | 109.3–134.7 |

线程数翻倍吞吐不翻倍（1 → 4 线程约 2 倍，4 → 6 线程增加有限）：这台只有 4 个物理核，逻辑核再多也不线性。

**写盘**（PGM，不压缩；需求 = 3 台 × 25 Hz × 1.31 MB ≈ 98 MB/s、75 张/s）：

| 方式 | 张数 | 速率（MB/s） | 张/s | 单张 p95（ms） | 单张 max（ms） |
| --- | --- | --- | --- | --- | --- |
| 串行 `save_pgm`（不 sync） | 3000 | 201–275 | 154–209 | 12.8–13.5 | 297–4724 |
| 3 路并行（每台相机一个目录，3 × 1000） | 3000 | 248–273 | 189–209 | 36.1–63.3 | 405–981 |
| 串行，每张 `sync_data`（不经系统缓存的盘速） | 300 | 157–231 | 120–176 | 10.8–27.6 | 45–161 |

平均速率是需求的 1.6–2.8 倍，但**单张写入偶发长停顿**（串行三轮里最长的一张分别是 3.1 s、0.3 s、4.7 s），停顿期间帧只能在队里等。原因没有查（系统缓存回写或杀毒扫描都有可能）。

**回填**：

- `MEASURE_WORKERS = 4`。公式 `ceil(75 fps × p95 / 0.7)`：串行 p95 取三轮最差的 30.3 ms 得 ceil(3.24) = 4（另两轮 26.5、26.4 ms 得 3）；上限 逻辑核 8 − 2 = 6，不起作用。核对：4 线程实测吞吐 101–112 fps，75 fps 占其 67–74%；3 线程（只测了一轮）99 fps，占 76%，高于 0.7 的目标，所以取 4 不取 3。4 线程下单帧 p95 56–63 ms，75 fps 下在途约 5 帧，可以忽略。
- 逻辑核少于 6 的机器上上限 `逻辑核 − 2` 会把线程数压到 4 以下（如 4 逻辑核只剩 2 线程，按本机 2 线程的实测约 74–78 fps，没有余量），这类机器直接走下面的降级方案。
- `IMAGE_BUDGET_PER_CAMERA` 保持 256 MiB。依据：实测最长写盘停顿 4.7 s，25 Hz 下每台相机积压 118 帧约 155 MB，占预算的 58%；测量积压可以忽略（上一条）。停顿超过 8.2 s 才会触发预算满，此时按 D-SB-32 丢该帧记缺帧、不判失败。真机压测里看到预算满再调，不预先放大（离线来源不经过 MVS 回调，预算恒不满，见第九节第 17 条）。
- 其余队列条数（FRAME_QUEUE 等 1024、示教写盘 8192）不受 7.1 数据影响，保持表中初值。

**75 fps 不可达时的降级方案**（由用户决定，PC 不自行降频）：

- 触发条件（任一）：现场机探针的串行 p95 代入公式得到的线程数超过 `min(4, 逻辑核 − 2)`；吞吐压测出现预算满或测量队列满（记缺帧 / Unmeasured）。
- 第一步，在软件相机配置中降低固定帧率（重新标定、验证和发布）。可持续帧率 = 0.7 × 该机满线程实测吞吐，Hz = ⌊可持续帧率 / 3⌋。以本机串行 p95 30.3 ms 按公式推算（推算值，未逐档实测）：25 Hz（75 fps）需 4 线程，20 Hz（60 fps）需 3，15 Hz（45 fps）需 2，10 Hz（30 fps）需 2。本机 6 线程吞吐 109–135 fps 的 70% 是 76–94 fps，即 25 Hz 刚好踩在 0.7 的线上，没有富余。
- 降频的连带影响：每段 N_exp 随帧率变，必须重新标定（按默认做法 6 换计划版本）；规格 §0 的 10～25 Hz 范围不变。
- 第二步才考虑缩短走廊（减少测点数）等需要改算法的做法。
- 在降级之前和之后，测量跟不上时都不会静默丢帧：该帧记缺帧或 Unmeasured，该组标 incomplete，整件 `flags.incomplete`，不改 OK/NG（D-SB-13）。

**局限**：

- 只在一台开发笔记本上测，现场工控机型号未知；本表不外推，现场机要重跑探针并把 p95 代入公式。
- 测量和写盘是分开测的，没有与 MVS 回调拷贝、界面、SQLite 同时跑；同时跑的余量由 3 × 25 Hz 吞吐门槛判定，目前没有通过的结论（见第九节第 17 条）。
- 只测了 1280×1024、121 个测点的合成图；更大的图、更长的走廊、真实胶条图像对耗时的影响没测。

## 八、旧拍照点配方

旧拍照点配方（`segments` 为空）从未部署，不能新建、开工或发布，不维护运行入口、迁移与专属测试；通用单帧标定和 Software 调试保留。本地草稿不自动删除。

## 九、实现要点

条目编号沿用原记录，代码注释按编号引用。

#### 10. 示教采集轮次（`CaptureRoundV2`）

- **配组基线**：`shot_router::Ledger` 的 `last_frame` 记本会话最高 SDK 帧计数（`FrameMeta.frame`），轮次每台相机的基线取它；没见过帧取 `counterAfterOpen`，仍未知时取本轮这台相机首帧计数 − 1（Line0 高电平门控下软件取不了帧，而 PLC 开始前到帧整轮判失败，首帧必属本轮）。
- **seq 上限**：seq > 32767 按计数异常判整轮失败，防止异常帧号生成海量帧组。
- **整轮失败的情形**：PLC 开始前收到帧、会话变化（98）、手动帧、实拍轮次收到模拟计数、计数来源变化、帧计数不大于基线或比本轮已见的小、写图失败、PLC 开始 / 结束顺序异常。重复帧号丢弃并记 `duplicate`；丢包或无图、写盘队列满、图像内存预算满记缺帧并告警。操作员中止记 `interrupted`（99）。
- **读图**：`recipe_capture_frame_image(roundId, cameraId, seq)`；图像路径由相机与 seq 推出（`{相机}/{seq:06}.pgm`），记录里写的路径只用于核对。

#### 11. 生产节拍

- **布防**：`shot_router::arm_frames` 按 SDK 帧计数定基线（D-SB-7），本会话没见过帧时取开流后计数起点，仍未知时不拒绝布防，取本件这台相机首帧计数 − 1（本件没收到帧的相机不记下一件下限）；`Policy::burst_production` 认 ChunkTrigger / ChunkFrame / SdkFrame，`Policy::burst_offline` 另认 Synthetic。会话或计数来源不可信拒绝布防报 98；测量池全部超时未返回也拒绝布防。
- **帧**：Grouper 的 seq 上限 = ΣN_exp + Σtol + 64，超出判 `Offer::OutOfRange`（不计入本件、不抬基线），丢弃并告警。重复帧号丢弃，每台相机每件告警一次；上一件迟到帧记游离帧；布防后的手动帧丢弃并告警（不判 ERR，告警写明该相机此后帧组可能错开一格）；检测中会话变化或计数来源变为不可信判 ERR 98。回调通道满、图像内存预算满的帧不交给节拍，即缺帧。
- **测量**：每帧经 `FramePool` 送测，结果经 `cycle::Input::FrameMeasured` 回节拍；QueueFull / Timeout 记 Unmeasured，Failed 记 Error；超过 T_proc 或到收尾截止仍没结果记 Unmeasured；测量输出的帧身份与送测帧不符或含非有限数改记 Error。都只记检测不完整，不判 99。
- **收尾与判定**：运动结束后等满收尾窗口、已接受的帧都测完即判。故障（98、97）优先；否则 `Unsegmented` → `resolve_regions` → `loss_alarms(各相机帧数)` → `burst_judge::judge`。丢帧、帧数异常、疑似提前终止写运行日志（warn），结论不变。
- **结果与记录**：`ResultView.burst` 带区域判定，`PartView.burst` 按 5.2 输出（帧格的 verdict 只看这一帧的独立指标，允差取覆盖全部段的较宽一档）。`BurstRecord.frames` 在 1..=帧组数内每台相机每帧一条、缺帧也有，`rawFile` 入库时为空（录制结果事后补）；`frames_expected` = ΣN_exp，`frames_received` = 帧组数。拒绝布防、在途中断、进程中断补记录的件也带 `BurstRecord`。诊断包按帧记录的 `rawFile` 打包原图。
- **在线读图**：`workspace_live_frame_image`（预览缓存 128 MB）。
- **胶偏未判**：直线区域去掉首尾各一帧后参考相机没有可用 x 时标 `OffsetUnjudged::NoFrames`（“胶偏未判：参考相机没有可用帧”）。
- **带宽估算**：`camera::aggregate_bandwidth` 计入 GigE 协议开销（不开巨帧约 1.05 倍），三台 130 万像素 25 Hz 约 826 Mbps，超过单口 80% 预算。

#### 12. 候选工作区（`workspace/burst.rs`）

- **`WorkspaceView.plan`**：productionPlc / candidatePlc / candidatePlcError / coverage 为 null、旧 progress 没有步骤。`plan.progress`（ProgressV2）按整圈采集 → 段列表 → 走廊示教 → 多轮标定 → 验证 → 发布给出：“整圈采集”以本配方有完整采集轮次为完成。
- **Workspace 字段**：`cameraTeaching: [{camera, frame: FrozenImage, round, seq}]`（示教帧冻结为候选 images/ 下的 PGM）、`calibration: {rounds: [CalibrationRound]}`（每轮带 `standard`）。不另存标定结果：结果写进配方（N_exp、calib、partCalib、x_ref），`plan.calibration`（CalibrationView）每次由配方与轮次算出。建议容差 = max(该类段默认容差, N − min, max − N)，整件取各段默认容差之和。
- **走廊示教**：`workspace_teach_camera` 只冻结所选帧、把草稿（走廊、当量、检测参数）写进 inspect，不在这里提取；提取用 `workspace_extract_corridor`（5.4）。当量缺省时沿用已有值，再缺省取这台相机的设备标定 `device/{device}`（模拟相机取模拟当量）。示教帧须来自本候选完整的轮次、这台相机有这一帧、采集后设备配置没变。`cameraTeaching[].saved` = 走廊至少两点、不短于 1 px 且完整落在示教帧内。
- **加入标定**：轮次须属于本候选、已完整完成、相机集合等于计划相机，各相机采集时的设备配置与当前设备、与已加入的轮次都相同；三台相机都已选示教帧且能建走廊测量图。整轮每帧用 `vision::measure_frame` 测（缺帧记 missing、测量失败记 error），每帧特征与分段结果写在候选目录 `calibration/<轮次>/`：`features.jsonl` 一行 `{camera: 相机编号, seq, metrics: FrameMetrics}`，1..=帧组数内每台相机每个 seq 一行；`boundaries.json` 为 SegOutcome。不写进采集轮次目录（特征依赖候选的走廊）。
- **应用标定**：标定轮次（不含标准件轮次）不少于 `calibrationRounds`，否则报错且配方不变；写 partCalib，单段计划同时写该段 N_exp 与 calib；多段计划按 D-SB-21。容差不写，由 `workspace_confirm_tolerance` 写入。
- **x_ref**：只给单段计划的直线段；每轮标准件取参考相机去掉首尾各一帧（seq 1 与帧组数）后测成帧的 x 均值，再用 `calibration::x_ref` 取均值与极差，只用走廊与当前相同的标准件轮次。直线段启用了胶偏但没配 offsetRef 时按 AbsoluteX、计划第一台相机补上；ToEdge 不写（“测量量未实现”）；多段计划不写。每次应用按当前标准件轮次重算，没有可用轮次时 x_ref 为 null。
- **编辑约束**（`workspace_save_doc` 与 `workspace_update_segments` 共用 `update_doc`）：段的类型序列不变时 N_exp、calib、partCalib 不能改，x_ref 只能原样带过或清空，胶偏参考的测量量或相机变了 x_ref 清空；类型序列（段数、顺序、类型）变了清空全部 N_exp、calib、partCalib 与 x_ref，已加入的标定轮次保留（整件帧组数与分段无关）。

#### 13. 候选验证、发布、进度与历史

- **验证项**（`workspace/burst_release.rs`）：设备与采集；走廊示教（恰好 3 台计划相机，每台都有示教帧、走廊完整落在帧内、有像素当量，且示教帧采集后这台相机的设备标签没变）；段标定（有 expected_total 即通过，逐段列出 N_exp、“待分段”或“待标定”）；标准件 x_ref（只在有启用胶偏检查的直线段时列出：单段 AbsoluteX 要有标准件得出的 x_ref；多段计划与 ToEdge 运行时不判胶偏，列出原因、不阻断）；独立正常样本；生产版本。
- **验证样本**：`workspace_capture_sample` 把采集轮次记进样本库（`BankSample.captureId` = 轮次编号，帧留在采集轮次目录，验证时按 (相机, seq) 读；`geometryTag` = 产品代码 + 有序计划相机 + 各段 (序号, 类型)，不含 N_exp，标定前采的样本标定后仍可用）。样本轮次不能是走廊示教或多轮标定用过的轮次，相机集合等于计划相机，各相机采集时的设备配置与当前相同。原图齐全的生产历史也可作样本：段结构身份与候选相同（历史没有记相机配置，不比较）。样本用当前候选的走廊逐帧重测（缺帧不读图，读图或测量失败只算检测不完整），按候选的段未分段判定；丢帧告警对采集轮次按计划相机顺序重算，对历史沿用记录。帧组数超出 ΣN_exp ± Σtol 与检测不完整只写进样本原因，不改是否符合。不接受按拍照点导入的样本图片。
- **发布**：`workspace_publish` 调用 `burst_release::freeze_bundle` → `release::publish_burst`（schema 4）：每台计划相机的示教帧、标定 JSON（`mmPerPx` = 示教当量，`reference` = 标定引用或 `device/{device}`，`source` = 工位标定文件内容）与走廊；测量图为 `production::burst_graphs`，`versions.graph` = `GRAPH_VERSION_BURST`。发布门槛 = 当前修订验证通过 + `ready_burst`。计划版本由 `plan_version_for` 按 `same_plan` 分配，重新标定使 ΣN_exp 变化即换新号。验证环境标签（`environment`）含各检测相机的工位标定（缺省为设备标定 `device/{device}`）。
- **进度**：验证步骤的阻塞跳转按检查项：走廊示教 → 第一台没示教好的相机，段标定 / 标准件 x_ref → 多轮标定，设备与采集 → 相机。
- **历史**：`workspace_record_frame_image(historyId, camera, seq)`（5.4）读历史一帧原图。生产记录读录制目录：路径在 `*_cycle_{cycleId}` 内、文件名为 `{相机}_s{seq:06}.pgm`，part.json 里这一帧的 cycleId、相机下标与编号、seq、帧号一致且已落盘，解码尺寸等于录制尺寸。候选离线调试记录读受管样本集（D-SB-28 口径）。`workspace_compare` 从原图用候选走廊逐帧复测（要求计划相机相同、原图齐全），或按已存每帧指标与记录里的区域用候选规则重判（要求 `store::same_measurement_layout`，ERR 记录不重判）；`workspace_compare_original` 按原发布包（schema 4）的走廊逐帧重现。结果写进 `Comparison.burst`（5.1）。

#### 14. 离线仿真

- **入口**：`offline_run` 与候选试运行（`workspace_debug_run`）都走 `Line::run_burst`。候选试运行过 `ready_burst`，各计划相机的示教帧尺寸按计划相机顺序取自 `cameraTeaching`，示教帧采集后这台相机的设备标签须没变（与走廊示教验证项同一口径，开跑前 `recheck_debug_candidate` 再核对），交给 `Candidate::new_burst` 建走廊测量图，再由 `offline::run_candidate` 运行。
- **出帧剖面**：`line::PulseProfile { hz, segments, head_tail_jitter, corner_jitter, seed }`，波动取法与 `segmentation::synthetic::frame_counts` 相同（同一剖面与种子在离线产线和回归序列里帧数、真值边界一致，`PulseProfile::spec` 给出回归入口的 `SequenceSpec`）。名义帧数 `line::nominal_frames`：各段 N_exp；有段待标定时按 段长 × 帧率 ÷ 50 mm/s 估算（没有段长：直线 12、弯道 4），已有整件标定时把待标定段缩放到 ΣN_exp。合成图取配方名义剖面，样本集取样本帧数最多的那台相机（一段）。离线节拍覆盖时帧率按 1000 ÷ 间隔换算（[soak.md](soak.md) S-3）。
- **使能与出帧**：布防后按 1 / hz 定频出帧（时钟不被轮询周期拉长），每次同时出三台相机的帧，出完置运动结束（plcState=20）。`PartOrder` 有 `profile`、`lost`（只丢指定相机的指定帧，相机照常计数）、`early_end_after`（提前结束前的帧数）。
- **工况（与前端同名）**：`cameraLost` 第 2 台相机丢第 min(3, 总数) 帧；`pulseJitter` 首尾各 ±1；`cornerSlowdown` 每个弯道段 ±2；`notInspected` 机器人照常涂胶、PLC 不提交请求（`run_burst` 返回 `Ok(None)`，PC 无记录，离线运行记“本件未检”）；`earlyEnd` 出约一半、且少于 ΣN_exp − Σtol 帧后报运动结束；`lostFrame` 三台相机同一帧都丢；`countMismatch` 与正常相同（协议 7 不报触发计数）。只改出帧与握手的工况进样本集白名单（`Scenario::handshake`）。
- **合成图**：模拟相机触发带 `camera::BurstRender`（计划相机下标、seq、所在段类型与段内进度、工况、种子），`simimage::render_burst` 按它出单视角图：直线段胶条沿这台相机的走廊（未示教时按相机上下错开的缺省位置），弯道段整条胶绕胶嘴转动 25°·sin(π·进度)，断胶 / 超差画在第 1 台相机第 2 帧（与模拟测量同一帧）。模拟测量 `measure::simulate_frame` 按名义剖面找到这一帧所在段，弯道帧胶宽略宽、x 拱形偏出（峰值 1.5 mm）；PC 判定仍按未分段。
- **合成图离线环境**：沿用真实相机组的编号与视角数，单视角相机不足 3 台时补足到 3 台（编号取没用过的 camN）。
- **合成图示教采集**：合成图来源的 `offline_capture` 按候选配方（须能编译）的名义剖面出一圈，首尾 ±1、弯道 ±2，种子取该候选的轮序号，多轮标定看得到 min / max 差异。多轮标定加入轮次时的每帧测量仍用 lyFlow 走廊测量（需加载 lyFlow）。
- **样本集**：`Manifest::camera_frames(camera)` 按清单顺序给出一台相机的帧序列；第 seq 帧取各相机的第 seq 帧，没有这一帧的相机按缺帧（照常计数、不出帧）。
- **节拍下限**：出帧间隔下限 15 ms（66 fps）。

#### 17. 吞吐门槛与样本集冒烟

- **吞吐门槛分两种来源测**（`scripts/soak/throughput.mjs`，`pnpm test:soak:throughput`）：合成图来源恒用模拟测量（`site::measurement_of`），出的帧不经 lyFlow，只测帧管线与渲染；**样本集**（现场原图经真实 lyFlow 测量）是离线下唯一能压 lyFlow 的来源，但每件帧数少、件间有握手开销，是突发而不是持续。脚本另按每件第一帧到达到最后一帧测完得出突发排空速率，并外推持续压力下队列多久装满，写进结果的 `projection`（估算，不是门槛）。
- **判定**：持续负载按相机使能为高的时间窗计（`enableWindows`）：PC 看不到使能信号，窗口取每件第一帧到达到最后一帧到达（首个帧组不计入）；要求 Σ窗口内帧数 ÷ Σ使能时长 ≥ 3 × Hz 的 90%，且至少连续 10 件（`-ThroughputMinParts`）逐件达到；件间握手间隙单独报告为 `achieved.handshakeGapSeconds`，不参与判定。持续负载未验证时默认判不达标，只想看分项时显式加 `-AllowBurstOnly`（`--burst true`），此时 `sustainedVerified` 仍为 false；中途停了的运行任何情况下都不放行。
- **门槛状态：没有达标，也没有被直接验证；门槛口径不改，等现场工控机用真实相机来源复测后再决定降帧率还是加并发，复测前不能在任何文档里记成已满足。** 实测（2026-10-23，i5-1135G7、调试构建、lyFlow 1.1.5，25 Hz、60 s，判定改为按使能窗口计之前）：
  - **合成图**：不达标。26 件 885 帧，Unmeasured 12、缺帧 46、13 件标检测不完整，帧到达约 34 帧/s（渲染上限），lyFlow 没有参与。
  - **样本集**：43 件 1548 帧全部测成，测量队列峰值 29 / 1024（采样下界），引擎耗时 p50 62 ms / p99 254 ms；但突发排空速率中位 49 帧/s 低于需求 75 帧/s，`projection` 外推持续 3 × 25 Hz 约 40 s 装满 1024 的测量队列。分项满足只说明突发下没出问题，不是门槛的结论。
  - **图像内存预算**：“预算丢帧 = 0”在离线下恒成立（`IMAGE_BUDGET_PER_CAMERA` 只作用于海康回调来源），只能在真机相机来源上核对（相机状态 `budgetDroppedFrames`）。
  - **与 7.1 对照**：应用里测到的突发排空速率 49 帧/s 只有 7.1 的 4 线程吞吐的一半，原因没有查（应用里还有读图、录制写盘、SQLite、界面同时在跑，且是调试构建）；按 7.1 的降级公式，可持续帧率 ≈ 0.7 × 49 ≈ 34 帧/s，即约 11 Hz。
  - **复测后的取舍**：① 按 7.1 降相机帧率到约 11 Hz，各段 N_exp 随之变化，须重新标定；② 加测量并发——本机只有 4 个物理核，4 → 6 线程增益有限，需要改代码并重测。
- **合成图渲染能力限制功能验收的帧率**：每帧逐像素渲染，开发笔记本上三台相机合计只能出约 20 帧/s，所以生产落盘等功能验收固定 5 Hz；更快时帧晚于运动结束才画完，末帧记缺帧、整件标检测不完整。
- **样本集冒烟**：停稳拍数据对不上一条直线走廊，所以样本集上的建站（长稳、吞吐）统一把各段检测项全关（`checksOff`），只验证流程、原包重现一致性、稳定性与吞吐，不验证判胶质量。
- **历史原图缺一张**：录制证据状态不随事后删文件刷新（仍是 complete），原包重跑读不到那一帧时记“测量失败”、该帧组标检测不完整，OK/NG 不变，不当作原图完整。
