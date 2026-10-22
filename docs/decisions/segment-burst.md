# 分段连拍飞拍决策记录（segment-burst）

依据：分段连拍飞拍方案第 18 版（2026-10-22，下称“规格”）、实施计划修订版第 2 版（下称“计划”）与实现决策补充（2026-10-22，用户已确认，下称“决策补充”）。三者冲突时以决策补充为准，其次规格，再次计划。基线为分支 `feat/segment-burst-flyshot` 的 `feb3a7c`。

本文件记录三类内容：

1. 规格与现有代码的冲突及采用的做法（第四节，D-SB-n，沿用 [plc-table.md](plc-table.md) 的条目格式：问题、采用、理由、回退）；
2. 前后端 IPC 契约、关键口径、内存与队列（第五至七节），后续各工作包按此实现，不再另行约定；
3. 旧拍照点模型与段模型并存的规则（第八节）。

契约类型的代码骨架：`src-tauri/src/{plan,frame_group,segmentation,burst_judge,calibration}.rs`（函数体待 A1–A5 填写）与 `src/features/plan/types.ts`（字段与 Rust serde 输出逐一对应）。

## 一、本轮范围与总原则

- 本轮只做并行组 G0（WP00）、G1（A1/A2/A3/A5/P1/B1/C2/FE1/DOC1）、G2（A4/C1/FE2/FE3/FE4/FE5）。G3 及之后的包不做，也不提前实现它们的内容。
- 计划中的 F1（后端清理）、FE6（前端清理）取消。**旧拍照点模型的代码一律保留并停用，不删除**：
  - 不删除任何现有公开函数、类型、命令、前端组件；要换掉的在新路径旁新增，旧路径标 `#[deprecated]` 或注释“旧拍照点模型”，不改其行为。
  - 存量测试只删除被规格推翻、且在当前工作包范围内的用例（计划 `tests_to_delete_or_rewrite` 所列），改写成能防住新规则的测试；不属于当前包的不动。
- 分段算法本轮只有 `Segmenter` 接口、`Unsegmented` 实现和合成序列回归入口；算法验证前整条胶路按“未分段”处理。

## 二、多段配方的发布与运行

用户对此无偏好，按以下执行（取代计划中“本轮只有单段计划能标定和发布，多段计划保持待标定”的做法）：

- **发布门槛**：整件 ΣN_exp（`expected_total`）由多轮采集得出即可发布；各段 `expected_frames` 允许为 null（待分段算法）。
- **`expected_total` 的来源**：标定轮次统计整件帧组数的均值（四舍五入），存为配方的 `part_calib: Option<PartCalib>`；各段均已标定时等于各段 N_exp 之和（`plan::expected_total(segments, part_calib)`）。
- **运行时（Unsegmented）**：整条胶路是一个未分段区域，只跑每帧独立检测，不判胶偏；只核对整件帧组数 ∈ `expected_total ± tolerance_total`，各段 N_exp 为 null 时不逐段核对。
- **`deviceShots` = `shotCount` = `expected_total`**。

## 三、已接受的默认做法

1. **溢胶**按“每帧最大胶宽超过溢胶上限（`overflow.max_width_mm`）”判，结论 NG，PLC 结果码沿用 NG_WIDTH（14），原因写“溢胶”。**气泡、拉丝本轮不检**，结果中标注“未检项”（`BurstJudgement.unchecked`）。
2. **胶偏只实现 `AbsoluteX`**；`ToEdge` 保留在模型里，选了它的段结果标“胶偏未判：测量量未实现”（`OffsetUnjudged::MeasureNotImplemented`），x_ref 也不计算。
3. **PLC 提前 partEnd**：PC 用已收到的帧照常判 OK/NG；帧组数 < ΣN_exp − Σtol 时加 `short_part`（疑似提前终止）标记，历史中单独显示（`PartSummary.flags`）。
4. **每台相机示教一条按直线段几何的走廊**（G3 之后实现）；弯道帧走出走廊记“检测不完整”（`FrameState::OutOfCorridor`），不判 NG。本轮只在类型和本文件里体现。
5. **现场 PLC 草稿** `scripts/s7-handshake/site/gluesight_site.scl` 不改，只在其 README 增加“协议 6 需要的改动点”一节（DOC1）。
6. **重新标定使 N_exp / expected_total 变化时分配新的 `plan_version`**（二者都进入计划身份，见 D-SB-19），并在界面提示 PLC 同步 shotCount（`WorkspacePlanView.shotCountChanged`、`CalibrationView.planVersionChange`）。
7. 三台相机**取最差**；帧序号取 SDK **`frame_counter`**；三台相机整件**全无帧判 ERR 91**；采集失败报 **90 + 98/99**；**容差默认 1**。

## 四、冲突与决策

### D-SB-1 计划模型：段列表与拍照点列表并存

- 问题：现状 `RecipeDoc.shots: Vec<ShotSpec>`（id/pose/camera/view），按触发分组编计划，`PathPoints` / `recipe::Segment` 按拍照点×视图生成测站（recipe.rs:414-450、819-876，plc_plan.rs:101-118）；规格 §1 改为段列表。
- 采用：新增 `plan::PathSegment`（Rust、TS 同名，避开 `recipe::Segment`）。`RecipeDoc` schema 8 增加 `segments`、`inspect: Vec<CameraInspect>`（有序，计划相机顺序取它）、`segment_defaults: KindDefaults`、`calibration_rounds`（默认 10）、`part_calib: Option<PartCalib>`（C1）。**`segments` 为空 = 旧拍照点配方**，走旧校验、旧节拍、旧发布；**非空 = 段模型**，要求 `shots` 为空，两者都非空时报错。两套永久并存。`ShotSpec`、`PathPoints`、`recipe::Segment`、`trigger_groups` 保留并停用（段配方不经过）。
- 理由：决策补充取消 F1，旧模型不删；以 `segments` 是否为空区分，旧配方文件除 schema 号外无需改动。
- 回退：段配方校验一律报错即可让段模型不可用，旧配方不受影响。

### D-SB-2 计数精确：生产接件按 ΣN_exp 核对，运行时只告警

- 问题：现状 `shot_count = device_shots = shots.len()`，admission 精确比对否则 94，`store.insert` 要求覆盖每个计划拍照点（plc_plan.rs:48-49、admission.rs:14-16、store.rs:693-701）；规格 §5 shotCount/deviceShots = ΣN_exp，§2 帧数在 N_exp ± tol 内正常，超出只告警。
- 采用：段配方生产接件时 shotCount 与 deviceShots 都必须精确等于 `expected_total`，否则 94。运行时只按区域比较帧组数与 N_exp ± tol（本轮只有整件一个未分段区域，比较 `expected_total ± tolerance_total`），越界记“帧数异常”告警。记录库按实际帧落库。旧配方的精确核对不改。
- 回退：admission 改为只核对身份字段。

### D-SB-3 区域帧数口径：帧数 = 帧组数

- 问题：规格 §2 每段取各轮帧数均值作 N_exp，运行时每段帧数与 N_exp ± tol 比较，但没说三台相机各自的帧数怎么合成一个“帧数”。
- 采用：标定和运行同一口径——帧数 = 帧组数，即区域内 seq 的个数（`last_seq − first_seq + 1`）；整件或整轮的帧组数 = 各相机最大 seq 的最大值。单台相机的帧数只用于丢帧告警（与 deviceTriggers 相差 > 1，或与另外每一台相机都相差 > 1），不参与容差比较。详见第六节。
- 理由：某台相机缺帧时帧组数不变，结论与告警分开；标定与运行用同一个量，N_exp 才可比。

### D-SB-4 partEnd 不再核对计数

- 问题：`handshake::verify_part_end` 要求 deviceTriggers == deviceShots（handshake.rs:551-562）；规格 §5 去掉计数相等校验。
- 采用：B1 删除相等判断，只保留 partEnd/partStart 有效、请求身份不变两项；`SessionEvent::End` 携带 partEnd 时的 deviceTriggers，只用于丢帧告警和收尾等待。这是协议 6 的行为，对新旧配方相同。
- 回退：协议版本退回 5 时恢复（不计划）。

### D-SB-5 请求层不再约束 shotCount

- 问题：`Request::validate` 要求 shotCount ≠ 0 且 deviceShots == shotCount（handshake.rs:479-481）；规格 §5 示教采集轮次不核对，PLC 可填估算值。
- 采用：`Request::validate`（含 recovery、validate_pending）删除这两条；生产核对移到 admission（D-SB-2），示教路径不核对。

### D-SB-6 协议版本 6

- 问题：`PROTOCOL_VERSION = 5`（handshake.rs:9），SCL、Python 测试 PLC、前端文案都写 5；规格 §5 protocolVersion/pcProtocolVersion = 6，DB4/DB2 布局不变。
- 采用：PC 只接受 6，点表模板不变。B1 改代码，DOC1 改文档、SCL 和 s7_test_plc.py，FE5 改前端文案。旧配方在协议 6 下 shotCount 仍为其触发次数。
- 回退：`handshake::PROTOCOL_VERSION` 改回 5，同步参考程序、测试 PLC 与文档。

### D-SB-7 下一件基线：每台相机各取自己见到的最高 frame_counter

- 问题：End 时标 `issued_verified`，`close_routing` 按计划 device_shots 推下一件基线（cycle.rs:1325、1612-1620、1767-1773）；规格 §5 deviceTriggers 只作参考，§3 帧序号取 SDK 帧号。
- 采用：两套规则按配方类型分开，永久并存（第八节）：
  - **段配方**（`segments` 非空，C3）：每台相机下一件的帧计数下限 = max(该相机原基线, 该相机本件见到的最高 frame_counter)（`Grouper::max_counter` → `Ledgers::set_frame_floor`，记在 `Ledger.frame_floor`，布防时由 `shot_router::arm_frames` 与本会话最高帧计数取大）。deviceTriggers 与计划数都不参与，deviceTriggers 只用于丢帧告警和收尾等待（等每台相机 max_seq ≥ end_triggers − 1，或到 drain 截止）。
  - **旧拍照点配方**（`segments` 为空，规则不变）：`Part::close_routing` → `ShotRouter::close(ledgers, issued)`，每台相机的下限 = max(原基线 + PLC 报的本件已发触发数（确认结果时报的，否则 partEnd 时报的；都没有时按计划 device_shots）, 该相机本件见到的最高触发计数)，记在 `Ledger.floor`；没等到 partEnd 就判 ERR 的件（运动超时等）延到 PLC 确认结果（`SessionEvent::Released`）时用那时报的触发数收尾。
  - 故障复位（`Ledgers::clear_floors`）两种下限都清掉，之后只按实际见到的计数布防。
- 理由：段配方——某台相机漏收一个脉冲时帧号不增，用“基线 + deviceTriggers”会让它下一件的首帧被判为游离，而且每件都丢；PLC 在使能边沿可能多数或少数一个脉冲，用它推基线会系统性地让下一件首帧被判游离；相机偶发丢末帧（已计数、帧没送到）只会让下一件出现两个不完整帧组，且下一件后自动对齐。旧拍照点配方——决策补充要求旧路径不改其行为；原规则防的是上一件末帧在收尾和下一件布防之后才到、被绑成下一件第 1 帧使整件错位，代价是相机漏收脉冲后每件判 ERR（91，原因写明“触发计数错位”），直到故障复位（见 `shot_router.rs` 模块说明）。FIX-B1 曾把旧配方也改成 max(原基线, 最高触发计数)，未经用户确认，C3 已恢复旧规则与对应用例。
- 回退：段配方若要防末帧迟到，可改为 max(原基线 + deviceTriggers, 最高帧计数)，代价是漏收脉冲的相机下一件首帧判游离；不计划。

### D-SB-8 示教采集数量与内存

- 问题：`MAX_TRIGGERS = 64` 同时是写盘队列容量；`plc_started` 只接受 1..=64；`plc_ended` 要求与计划相等；收图超过计划即失败；写 png 且 original 和 view-1 重复写（recipe_capture.rs:72、382、502、526、575）；规格 §5 取消 1～64 上限，示教轮次不核对计数。
- 采用：新轮次（D1，`CaptureRoundV2`）不设上限、不核对计数。写盘队列条数上限 `MAX_FRAMES_PER_CAMERA = 8192`，图像内存由 C2 的 FramePool 每相机字节预算封顶；预算满或队满时该帧记缺帧并告警，整轮不判失败。图像写无压缩 PGM，一帧一个文件；`round.json` 只在状态变化时重写，帧元数据追加写 `frames.jsonl`。旧轮次读取与旧接口保留。

### D-SB-9 示教采集完成判据

- 问题：现状要求 plc_ended、每台相机 queued == n 且写盘完成才 Complete，否则判 ShortCameras/PlcEndMissing（recipe_capture.rs:634-664）；规格 §5 不再用计数做结束校验，§3 缺帧只告警。
- 采用：新轮次三条都满足即 Complete——收到 PLC partEnd、drain 窗口到期、已入队的帧全部写完。各相机帧数、不完整组和丢帧告警写进轮次记录（`CaptureRoundV2`）。超时只保留 StartTimeout 和 WriteTimeout。

### D-SB-10 帧序号来源：frame_counter

- 问题：采集按到达顺序编 ordinal（recipe_capture.rs:568）；生产 `shot_router` 按 `f.trigger` 路由，Policy 只信 ChunkTrigger（shot_router.rs:218-221、417、427）；`camera.rs:518` 的 frame_counter 已是“有 Chunk 帧计数用它，否则用 nFrameNum”。
- 采用：`camera.rs` 的 frame_counter 取值已符合规格，不改。段模型的消费方以 `Frame.frame_counter − 该相机基线` 作 seq（D1 示教、C3 生产）；段配方的 Policy 信任 ChunkFrame、SdkFrame、ChunkTrigger 三种来源的 frame_counter，离线另外信任 Synthetic。trigger_counter 只作诊断。旧配方的按触发路由保留。

### D-SB-11 设备模型：三台独立单视角相机共用一个触发

- 问题：`view_count ∈ {1,3}`，三视角靠 `composite_layout` 拆拼接图；前端标定页在 viewCount === 3 时选视角（FlyshotCalibrationPage.tsx:56、CalibPanel.tsx:61）；协议文档写唯一三目设备。规格 §0 一次触发 = 三台独立相机各出 1 帧，§1 Plan.cameras = 3 台设备 id。
- 采用：段配方的计划相机必须是 3 个互不相同、单视角（`view_count = 1`、无 `composite_layout`）的设备；不满足时段配方校验或布防报错。**相机配置本身仍允许三目拼接，供旧配方使用**；`CompositeLayout` 及拆分代码保留并停用（段配方不经过）。前端标定页对段配方不再选视角。
- 理由：计划原定 C2 在 cameras.json 加载时拒绝三目拼接，会改变旧配方的行为，与决策补充“旧路径不改其行为”冲突；改为在段配方一侧约束。
- 回退：无需回退；若决定彻底停用三目拼接，再在相机配置加载处拒绝。

### D-SB-12 所有帧都测，不再每组只检一张

- 问题：`select_group_image` 同 Pose 组只检所选的一张，其余标 skip（workflow.rs:158-207）；规格 §0 所有帧都参与每帧独立检测，不裁帧。
- 采用：段配方每台相机的每一帧都测。组选图的后端、发布前检查、前端保留并停用（只服务旧配方）。

### D-SB-13 缺帧、多帧、未完成的结论

- 问题：现状缺帧判 ERR 91，多帧/超计划判 ERR 96，未完成判 ERR 99（cycle.rs:759-773，shot_router.rs:312、470-475）；规格 §0/§3 某台缺帧时该组照常判，组和整件标“检测不完整”，不影响 OK/NG，帧数超出容差只告警。
- 采用（段配方）：缺帧（含回调通道满、内存预算满、测量队列满、测量失败、走出走廊）→ 组标 incomplete，整件 `flags.incomplete`；帧数越界 → `frame_count_anomaly`；都不改 OK/NG。只有三台相机整件一帧都没有时才判 ERR 91；会话变化仍判 ERR 98。96 对段配方不再使用，常量和文档表保留。旧配方不变。

### D-SB-14 三台相机结论取最差

- 问题：`aggregate_views` 同一拍照点任一视图合格即合格（judge.rs:196-202）；规格 §0 所有帧都参与独立检测。
- 采用：段配方任一相机任一帧 NG，所在区域即 NG，整件取最严重结论。`aggregate_views` 保留给旧配方。

### D-SB-15 胶偏判定：段级 AbsoluteX，ToEdge 暂不实现

- 问题：现状逐站与示教中线比对 d，以及特征定位 d′ = B − T(N)；规格 §4 直线段去首尾各一帧，用 x 均值 − x_ref 判整段偏移、最大偏差判局部跑偏，中线逐点比对停用；§0/§1 有稳定边缘用 ToEdge，否则 AbsoluteX。
- 采用：每帧输出 `x_mm`（帧内胶中心相对走廊的法向偏移均值，即 AbsoluteX）。胶偏只在 Straight 区域按段判。ToEdge 在模型中保留，结果标“胶偏未判：测量量未实现”（默认做法 2）。中线逐点比对与特征定位对段配方不使用，代码保留，旧配方行为不变；`packaging/runtime.json` 的 `glue.locate` 和 `locateSmoke` 不改。
- 回退：若要实现 ToEdge，需另定参考边缘的示教方式，工作量在 C4、A4、D2。

### D-SB-16 胶宽均匀性

- 问题：现状只有逐站胶宽上下限；规格 §4 直线段胶宽/均匀性按段级 min/max/mean 检，弯道用弯道允差，未分段用较宽允差。
- 采用：`WidthCheck` 增加 `max_range_mm: Option<f64>`（区域内各帧 `width_mean` 的极差上限，默认 None 即不检），只在 Straight 区域判，超限 NG_WIDTH，原因“胶宽不均”。Corner 和 Unsegmented 不判极差，只判每帧胶宽。每帧胶宽按 `widths × spacing_mm` 算帧内超差长度，不超过 `max_excursion_mm` 为 OK_WITH_EXCURSION。

### D-SB-17 检测走廊：每台相机一条按直线段几何的走廊

- 问题：`taught.rs` 要求示教中线完整落在图内、站数固定（taught.rs:334-336）；规格 §4 中线逐点比对停用，保留边缘提取。
- 采用（默认做法 4）：每台相机示教一条按直线段几何的走廊（`CameraInspect.corridor`），所有帧共用，只决定 ROI 和测站；弯道帧胶条走出走廊时该帧记 `OutOfCorridor`（检测不完整），不判 NG。G3 之后实现，本轮只在类型和本文件中体现。全流程验收用合成直线/弯道三相机；真实样本集 gs-20261017 是停稳拍数据，只做冒烟并报告不完整比例。

### D-SB-18 胶偏参考值 x_ref

- 问题：现状位置名义值手填，≠ 0 时必须填 gaugeRef（recipe.rs:77-81）；规格 §0/§1 `offset_ref.x_ref` 取标准件多轮测量均值，只用于 Straight 段。
- 采用：`x_ref` / `spread` 由标记为标准件的采集轮次自动算出，不能手填；段配方不用 gaugeRef（旧配方的 gaugeRef 规则不变）。`OffsetRef` 增加 `camera` 字段（三台相机几何不同，不能共用 x_ref），默认取计划第一台相机。未标定或 ToEdge 时 `x_ref`、`spread` 为 null。

### D-SB-19 计划版本身份

- 问题：`same_plan` = 产品代码 + 拍照点 id/pose/camera 序列（recipe.rs:724-729）；规格 §1 段序列、类型、相机绑定变化分配新版本，限值、容差、x_ref 变化沿用。
- 采用：段身份 = `product_code` + 各段 `(index, kind, expected_frames)` + 有序 cameras + `expected_total`（`plan::plan_identity`）。限值、容差、x_ref、inspect_range、length_mm 不参与。`same_plan` 要求段身份和旧拍照点身份同时相等（旧配方 segments 为空，段身份恒等；段配方 shots 为空，旧身份恒等），两部分永久保留。
- 理由：ΣN_exp 就是 PLC 的 shotCount，N_exp 或 expected_total 变了 PLC 必须同步（默认做法 6）；本轮多段配方各段 N_exp 为 null，只有 `expected_total` 能反映重新标定。

### D-SB-20 就绪与发布前置

- 问题：`Recipe::ready` = 每个要检的拍照点中线至少 3 个测点（recipe.rs:651-662）；规格 §1 新建时 expected_frames = null 标“待标定”，未标定不能发布，§0 N_exp 不能手填。
- 采用：新增 `ready_burst`：3 台相机都示教了走廊和当量，且 `expected_total` 非空（第二节：各段 N_exp 可以为 null）。`update_doc` 直接修改 `expected_frames`、`calib`、`part_calib` 时拒绝；段的序列或 kind 改变时清空全部 N_exp、calib、`part_calib`、x_ref。`Recipe::ready` 保留给旧配方。

### D-SB-21 本轮无分段算法时的标定

- 问题：规格 §3 算法验证前整条胶路按未分段，§2 每段 N_exp 由多轮采集得出，§1 未标定不能发布；`Unsegmented` 只能给出整件帧组数。
- 采用（决策补充第二节，取代计划“多段计划保持待标定”）：每轮帧组数做 `calibrate_counts`，结果写 `part_calib`。单段计划同时写该段 `expected_frames` 和 `calib`；多段计划各段保持 null，界面标“待标定（分段算法未就绪）”，但凭 `expected_total` 可以发布。

### D-SB-22 检测范围与未检区域

- 问题：首末不判的范围隐含在 `FixtureSpec.end_margin_mm` 里；规格 §0/§1 每段写明 inspect_range [a_mm, b_mm]，结果页标“未检区域”。
- 采用：`PathSegment.inspect_range` 显式配置；`burst_judge` 按配方算出 `[0, a)` 与 `(b, length_mm]` 放进 `BurstJudgement.uninspected`，段长未知时 `to_mm` 为 null，界面按 mm 展示。

### D-SB-23 触发方式 Stop 保留

- 问题：`TriggerMode` Fly | Stop（recipe.rs:11-16），多处读写；规格 §0 只有飞拍。
- 采用：`trigger_mode` 字段与 Stop（停稳拍）**保留**，旧配方照常使用；段配方固定写 Fly，界面对段配方不展示触发方式，段配方的离线节拍按脉冲频率，不读 `trigger_mode`。`RecipeSummary.trigger_mode`、`parts.trigger_mode` 列保留。

### D-SB-24 新建模板：三台相机、一个直线段

- 问题：`recipe_template` 只取相机组第一台（recipe_api.rs:57-63）；规格 §1 cameras = 3 台，容差默认 ±1，直线与弯道分别设。
- 采用：新建模板取相机组前三台（不足 3 台也建，但无法发布），附一个 Straight 段，`expected_frames = null`。`segment_defaults` 的 serde 默认值即 `KindDefaults::default()`（straight、corner 的 tolerance 都是 1），`new_segment` 从这里取。`recipe::samples()` 不变，另加 `samples_burst()`。

### D-SB-25 PLC 对接参数与计划导出

- 问题：`PlcParams` 只用于界面展示拍照顺序和 Pose 表（workspace.rs:570-571）；规格 §1 导出给 PLC/机器人工程师的计划 JSON：段序列、各段 N_exp、容差、ΣN_exp。
- 采用：新增 `plan::PlanExport` 与命令 `recipe_plan_export`。**`PlcParams` 保留**（旧配方继续用，结构不变）；段配方的导出放在 `WorkspaceView.plan.productionExport / candidateExport`，旧字段 `productionPlc / candidatePlc` 对段配方为 null。
- 理由：计划原定 `pub type PlcParams = PlanExport`，会改掉现有公开类型的序列化形状，与“不删除、不改旧路径”冲突。

### D-SB-26 SCL 参考 FB 与现场程序

- 问题：`phase1_plc.scl` 运动中 Delta > FrozenDeviceShots 和结束时计数不等都判 7006；`site/gluesight_site.scl` 达到计划数就停发、按“开始拍摄”边沿逐个出脉冲；规格 §0 使能为高时连续发脉冲，§5 SCL 去掉计数相等检查。
- 采用：`phase1_plc.scl` 两处 7006 都删，INT#5 改为 6（DOC1）。`site/gluesight_site.scl` 不改，只在其 README 增加“协议 6 需要的改动点”一节（默认做法 5）。

### D-SB-27 离线仿真

- 问题：虚拟 PLC 精确发 device_shots 次触发；CountMismatch 期望握手故障；丢帧整组相机一起丢；`offline_capture` 只接受样本集相机（offline/mod.rs:574）；规格 §6 要求使能电平、脉冲串、首尾 ±1、弯道减速波动、三台相机之一缺帧。
- 采用：E1（G5，本轮不做）按剖面发脉冲串（每段帧数 + 种子化波动），丢帧按相机独立设置；CountMismatch 改为告警工况；新增未检、提前 partEnd 工况；`offline_capture` 支持合成图相机，使合成图离线环境能完成 10 轮标定和发布。旧配方的离线节拍保留。

### D-SB-28 样本集 v1

- 问题：`Manifest::recipe_shots` 要求与配方拍照点一一对应（sample_set.rs:191-212）；规格 §2/§3 帧数波动、按帧号配组。
- 采用：保留 v1 格式，段配方解释为“每台相机按 shotId 顺序的帧序列”。E1 新增 `camera_frames(camera)`；`recipe_shots` 保留并停用（注释“旧拍照点模型”），旧配方继续使用。

### D-SB-29 检测记录结构

- 问题：`part_shots` PK(part_id,k)、UNIQUE(shot_id)、view 限 1–3（store.rs:558-625）；规格 §3/§4/§6 要记录每台相机每一帧、帧组、段、不完整组、未检/未分段区域。
- 采用：C5（G3）新增 `part_frames` 表和 `parts` 新列（expected_total、tolerance_total、device_triggers、camera_frames、flags），DB_VERSION 8→9（D-0：备份后重建）。`part_shots`、`part_points`、`trigger_mode` 列保留，旧配方记录照旧写入。
- C5 落地细节：`parts` 另加 `burst_judgement` 列存整份区域判定；`part_frames` 主键 (part_id, camera, seq)，入库时帧数不要求等于预期，但 (camera, seq) 唯一、相机属于计划相机、指标身份与帧一致，摘要结论与区域判定相同；原图引用按 (camera, seq) 只增不换。审计 spool FORMAT 3→4，原图引用 `files[].frame` 有值即段配方帧（不受 k < 64、视角 1–3 限制）。录制文件名 `{相机}_s{seq:06}.pgm`，part.json 增加 `missingFrames`（相机没出图，只追溯，录制仍可完整）与 `rawDropped`（录制队满没录，证据 Incomplete）。历史重判 `Overrides` 增加 `straight` / `corner`（按段类型覆盖胶宽上 / 下允差与帧内允许超差长度，`maxGapLen` 作用于每帧），用当前候选重判时计划身份或走廊、当量、标定、检测参数变化跳过并记 LayoutChanged，重判结果 `changes[].flags` 带出整件标记；CSV 末尾增加 ΣN_exp、Σtol、各相机帧数、deviceTriggers、标记五列；时间线 Entry 与运行日志字段增加 `seq`。

### D-SB-30 发布包与标定键

- 问题：发布包按 (k, view) 组织，RELEASE_SCHEMA = 3；标定键 `{device}-v{view}`；规格 §0 三台独立设备，没有视角。
- 采用：C4（G3）新增 `publish_burst`，按相机组织，写 schema 4；段配方的标定键为 `{device}`。旧 `publish`、schema 3 的加载、`{device}-v{view}` 标定键保留给旧配方，load 同时接受 3 和 4。

### D-SB-31 隐藏上限

- 问题：`PathPoints.k: Vec<u8>`，配方最多 64 个拍照点；规格 §0 10～25 Hz，帧数远超 64。
- 采用：旧模型的上限随旧模型保留。新模型帧序号用 u64，ΣN_exp 不超过 32767（PLC Int），单段 tolerance ≤ 1000。

### D-SB-32 队列容量、内存与背压

- 问题：FRAME_QUEUE = 64、MEASURE_QUEUE = 32、录制 QUEUE = 48（recorder.rs:18）；FramePool 只缓存空闲缓冲、不限在途数量（frame.rs:51-62）；测量并发 2–4（measure.rs:388）；规格 §0 不裁帧，所有帧都测。
- 采用：图像内存按字节封顶，队列只按条数防失控，详见第七节。录制队满时该帧原图缺失，part.json 列出，不影响结论。

### D-SB-33 采集失败结果码

- 问题：采集轮次 Failed 时报 90/94（cycle/capture.rs:71）；规格 §5 计数不符不再是故障。
- 采用（默认做法 7）：相机断连或会话变化报 90 + 98；写盘超时或操作员中止报 90 + 99（`CaptureFailureV2` 注明对应关系）。

### D-SB-34 本件未检与提前终止

- 问题：在途撤 partStart 判故障（plc_session.rs:644-646）；规格 §0 PLC 不提交 partStart（或提前终止），整件按未检处置。
- 采用（默认做法 3）：不提交 partStart 时 PC 没有记录。提前终止由 PLC 提前置 partEnd 表达，PC 用已收到的帧照常判，帧组数 < ΣN_exp − Σtol 时加 `flags.short_part`（疑似提前终止），历史中单独显示，不改 OK/NG。按未检处置由 PLC 根据自己的“本件未检”DO 决定。在途撤 partStart 仍判故障。
- 残余风险：可能出现机器人标了未检、PC 记录却写 OK 的情况；需要时可改为 short_part 判 ERR。

### D-SB-35 示教轮次的计划版本校验

- 问题：采集要求 plc_plan_version > 0，样本和历史必须与示教轮次的 PLC 计划版本一致（workflow.rs:411-413、workspace.rs:2092）；规格 §5 示教轮次可填估算值、不核对。
- 采用：段配方的轮次只记录 PLC 写来的 planVersion 和 shotCount（`CaptureRoundV2.plcPlanVersion / plcShotCount`）；一致性改为比较配方计划身份和相机配置标签。旧轮次规则不变。

### D-SB-36 多轮标定

- 问题：`Workspace.capture_id` 只有一个；规格 §2 同一程序连续采集 ≥ 10 轮（轮数可配），每段均值四舍五入作 N_exp。
- 采用：Workspace 新增 `calibration { rounds }`（D2，每轮带 `standard`，标准件轮次不计入轮数；标定结果写进配方，视图由配方与轮次算出，见第九节第 11 条）；`calibration_rounds` 默认 10，可配 10..=100。标定结果见 D-SB-21，容差建议由操作员确认后才写入。

### D-SB-37 溢胶、气泡、拉丝

- 问题：现状没有独立检测项；规格 §0/§4 三类区域都检溢胶/气泡/拉丝。
- 采用：默认做法 1。

### D-SB-38 进度步骤

- 问题：`StepId = capture|defaults|teach|calibration|overview|validation|publish`（workspace/types.ts:97）；规格 §6 采集→走廊→标定→验证→发布，按段展示确认。
- 采用：段配方用 `StepIdV2 = capture|segments|corridor|calibration|validation|publish`（第五节）；旧配方的 StepId 与 Progress 不变。

### D-SB-39 文档与代码早已不一致

- 问题：协议文档和 plc-s7-phase1.md 写唯一三目设备；规格 §5 三台设备共用一个触发。
- 采用：文档以规格和新代码为准重写（DOC1 协议文档，DOC2 其余文档）。

## 五、IPC 契约

完整字段以 `src/features/plan/types.ts` 为准（serde camelCase，Rust 的 `Option` 一律序列化为 null）。原则：**只增不删**，段配方的数据挂在现有 DTO 新增的可空字段上，旧配方这些字段为 null、旧字段语义不变。

### 5.1 现有 DTO 的增量

| DTO（Rust 位置） | 新增 | 段配方时旧字段的取值 |
| --- | --- | --- |
| `RecipeDoc`（recipe.rs，schema 8，C1） | `segments`、`inspect`、`segmentDefaults`、`calibrationRounds`、`partCalib`（TS `RecipeDocSegmentFields`） | `shots = []`，`triggerMode = "fly"` |
| `RecipeSummary`（cycle.rs，C1） | `segmentCount`、`expectedTotal`、`toleranceTotal`、`calibrated`（TS `RecipeSummaryV2`） | `shotCount = 0`；`triggerCount = expectedTotal`；`length = Σ lengthMm`（未知按 0） |
| `PartView`（cycle.rs，C3） | `burst: PartViewBurst \| null` | `n = expectedTotal ?? 0`；`received` = 各相机帧数之和；`triggers = deviceTriggers ?? 0`；`queue = pendingMeasure`；`filled = total = 0`；`frames = []`；`measuredFrames` 与 burst 相同 |
| `ResultView`（cycle.rs，C3） | `burst: BurstJudgement \| null` | flatten 的 Judgement：`verdict/plcCode/faultCode/reason` 与 burst 相同，`segments = gaps = []` |
| `PartSummary`（store.rs，C5） | `flags: Flags \| null`（TS `PartSummaryV2`） | `framesExpected = expectedTotal ?? 0`；`framesReceived` = 帧组数；`triggerMode = "fly"` |
| `PartDetail`（store.rs，C5） | `burst: PartDetailBurst \| null` | `frames = shots = []`，`points = null`，`judgement` 同 ResultView 规则 |
| `CaptureRound`（recipe_capture.rs） | 不变（旧轮次） | 新轮次为 `CaptureRoundV2`（`format: 2`，D1） |
| `WorkspaceView`（workspace.rs，D2） | `plan: WorkspacePlanView \| null` | `productionPlc = candidatePlc = null`，`candidatePlcError = null`，`coverage = null`，`progress = { steps: [], shots: [], next: null }`（段配方进度在 `plan.progress`）；`planChanged` 按 D-SB-19 的身份 |
| `Progress`（workspace/progress.rs） | 不变（旧配方） | 段配方用 `ProgressV2` |
| `Overrides`（history.rs，重判试算参数，C5） | `straight`、`corner`（`KindOverride`：`tolUpper`、`tolLower`、`maxExcursionLen`，绝对限不适用；TS `Overrides` 已有这两个可选字段） | 旧配方记录忽略这两个字段；段配方记录的每帧胶宽只读它们，`position` / `width` 不适用，未分段区域取两者中较宽的一档。**后端现在没有这两个字段，发出去会被静默忽略**，所以前端 `RejudgeDialog` 的直线段 / 弯道段两行输入置灰（`SEGMENT_OVERRIDES_SUPPORTED = false`）；C5 读取后改为 true 并同步测试 |

### 5.2 新 DTO

- **`PartViewBurst`**（在线在途件）：`cameras`、`expectedTotal`、`toleranceTotal`、`deviceTriggers`（partEnd 前为 null）、`groupCount`、`cameraFrames`、`measuredFrames`、`pendingMeasure`、`groups: FrameGroupView[]`。
- **`FrameGroupView`**：`seq`、`incomplete`、`cells: FrameCellView[]`（按计划相机顺序）。`FrameCellView`：`status`（`waiting | measuring | FrameState`）、`verdict`（该帧单独的结论，未测完为 null）、`frameCounter`。
- **`PartDetailBurst`**（历史详情）：`segments`（本件配方快照的段）、`cameras`、`expectedTotal`、`toleranceTotal`、`deviceTriggers`、`cameraFrames`、`judgement: BurstJudgement`、`frames: FrameRecord[]`（按 camera、seq 排序）。
- **`FrameRecord`**：`camera`（计划相机下标）、`seq`、`status: FrameState`、`frameCounter`、`triggerCounter`、`region`（`judgement.regions` 下标，不在任何区域为 null）、`metrics: FrameMetrics | null`（missing 时为 null）、`rawFile`（录制根内相对路径，未录制或队满丢弃为 null）。1..=groupCount 内每个 (camera, seq) 都有一条，缺帧也有。
- **`PartSummaryV2`** = `PartSummary` + `flags`。
- **`CaptureRoundV2`**：`format: 2`、`roundId`、`recipeId`、`cameras: CaptureCamera[]`、`state`（同 `CaptureRound.state`）、`plcShotCount`、`plcPlanVersion`、`deviceTriggers`、`groupCount`、`cameraFrames`、`incompleteGroups`、`missingFrames: {camera, seq}[]`、`lossAlarms`、`alarms: CaptureAlarm[]`（D1 增加：逐帧告警 `{kind: duplicate | queueFull | budgetDropped | corrupt, camera, seq}`，最多 1024 条）、`createdAt`、`endedAt`、`error`、`failure: CaptureFailureV2 | null`、`simulated`。`CaptureFailureV2`（`kind` 区分）：`camera{cameraId}`、`sessionChanged{cameraId}`（90+98）、`plc`、`startTimeout`、`writeTimeout{queued, written}`、`interrupted`（90+99）、`other`。
- **`CalibrationView`**：`minRounds`、`rounds: CalibrationRoundView[]`、`part: CalibOutcome | null`、`segments: SegmentCalibView[]`、`expectedTotal`、`toleranceTotal`、`planVersionChange`。`CalibrationRoundView`：`roundId`、`standard`、`groupCount`、`cameraFrames`、`incompleteGroups`（个数）、`lossAlarms`（个数）、`createdAt`。`SegmentCalibView`：`index`、`kind`、`outcome: CalibOutcome | null`、`pending`（如“分段算法未就绪”）、`tolerance`（配方当前值）、`xRef: {xRef, spread} | null`、`xRefPending`（如“测量量未实现”）。
- **`WorkspacePlanView`**：`segments`、`cameras`、`segmentDefaults`、`calibrationRounds`、`partCalib`、`expectedTotal`、`toleranceTotal`、`calibrated`（expectedTotal 非空）、`cameraTeaching: CameraTeachingView[]`（`camera`、`saved`、`frame: FrozenImage | null`、`mmPerPx`）、`calibration: CalibrationView | null`、`productionExport`、`candidateExport`、`candidateExportError`、`shotCountChanged`、`progress: ProgressV2`。
- **`RecipeSummaryV2`** = `RecipeSummary` + `segmentCount`、`expectedTotal`、`toleranceTotal`、`calibrated`。
- **`ProgressV2`**：`steps: ProgressStepV2[]`（与 `ProgressStep` 相同，`id` 为 `StepIdV2`，`blockers[].target` 为 `ProgressTargetV2`）、`next: {step, label, target} | null`。`ProgressTargetV2`：`capture`、`segments`、`corridor{camera}`、`calibration`、`validation`、`camera`。
- 镜像的领域类型：`PathSegment` 及其检测项、`PartCalib`、`KindDefaults`、`PlanExport`、`FrameGroup`、`LossAlarm`、`Region`、`SegOutcome`、`FrameMetrics`、`RegionResult`、`Flags`、`Uninspected`、`BurstJudgement`、`CalibOutcome`、`CameraInspect`。

### 5.3 新步骤与路由

| StepIdV2 | 页面 | 路由 |
| --- | --- | --- |
| `capture` | 整圈采集 | `/recipe/capture` |
| `segments` | 段列表与配方默认参数 | `/recipe/segments`（FE1 新注册） |
| `corridor` | 按相机示教走廊 | `/recipe/teach` |
| `calibration` | 多轮标定 | `/recipe/calibration`（FE1 新注册） |
| `validation`、`publish` | 验证与发布 | `/recipe/validation` |

旧配方的 `StepId` 与路由不变；FE1 只新增两个路由，不删旧路由。

### 5.4 新命令

工作区修改类命令沿用现有约定：参数带 `id` 与 `revision`（乐观并发），返回 `Result<WorkspaceView, String>`。

| 命令 | 参数 | 返回 | 包 |
| --- | --- | --- | --- |
| `recipe_plan_export` | `id` | `Result<String, String>`（PlanExport JSON 文本） | C1 |
| `workspace_update_segments` | `id, revision, segments: PathSegment[]` | WorkspaceView | D2 |
| `workspace_teach_camera` | `id, revision, camera, round, seq, draft` | WorkspaceView | D2 |
| `workspace_calibration_add_round` | `id, revision, round, standard: bool` | WorkspaceView | D2 |
| `workspace_calibration_remove_round` | `id, revision, round` | WorkspaceView | D2 |
| `workspace_calibration_apply` | `id, revision` | WorkspaceView（结果在 `plan.calibration`） | D2 |
| `workspace_confirm_tolerance` | `id, revision, segment, tolerance` | WorkspaceView | D2 |
| `workspace_extract_corridor` | `id, revision, camera: String, imageId, roi: [u32; 4], guide: BrushGuide?, pointSpacingPx: f32?` | `Result<CenterlineResult, String>`（与 `workspace_extract_centerline` 相同：`path`、`message`、`failure`） | D2 |
| `recipe_capture_frame_image` | `roundId, cameraId: String, seq`（帧组序号，从 1 起） | `Result<tauri::ipc::Response, String>`（原始字节，格式同 `recipe_capture_image`）；该相机缺这一帧时报错 | D1 |
| `workspace_live_frame_image` | `cycleId, revisionId, camera: usize, seq`（camera 为计划相机下标） | `Result<tauri::ipc::Response, String>`（原始字节，格式同 `workspace_live_image`）；未到或缺帧时报错 | C3 |
| `workspace_record_frame_image` | `historyId, camera: usize, seq`（camera 为计划相机下标） | `Result<tauri::ipc::Response, String>`（原始字节，格式同 `workspace_record_image`）；未录制、录制队满丢弃或缺帧时报错 | D3 |

新示教轮次（`CaptureRoundV2`）的读取与帧图像命令由 D1 在旧命令旁新增，旧 `recipe_capture_*` 命令保留。

- **`recipe_plan_export` 前端不调用**：计划导出随 `WorkspaceView.plan.productionExport / candidateExport` 到达，界面（`PlanExportPanel`）直接序列化它们供复制或另存。该命令只读配方库里已发布的配方，导不出还没发布的候选，而候选的导出正是发布前核对所需；命令保留给脚本与工程师工具，前端不再保留未使用的 `recipeApi.planExport`。
- **相机示教的单帧试测没有命令**：旧配方的逐点试测（`workspace_trial`）按拍照点下标，段配方没有对应命令，上表也没有登记；前端不保留没有数据来源的 `FrameTrialResult`。若以后要在相机走廊示教里做单帧试测，由后端先登记命令与 `FrameMetrics` 的返回方式，前端再新增组件。

- **段配方的走廊提取**：段配方没有拍照点（`shots = []`），不复用 `workspace_extract_centerline`——它按 `w.frames[k]` / `w.doc.shots[k]` 解析，段配方会报“拍照点不存在”。D2 新增 `workspace_extract_corridor`：按 `camera`（相机 id）找到 `plan.cameraTeaching` 里这台相机冻结的示教帧，`imageId` 必须等于该帧 id（不等报“图像已变化”），检测极性取这台相机 `inspect.detect`，没有时取配方默认。旧配方仍用 `workspace_extract_centerline`，行为不变。
- **读图命令的参数口径**：`camera` 在在线与历史命令里是计划相机下标（与 `FrameGroupView.cells`、`FrameRecord.camera` 一致）；示教轮次命令用 `cameraId`（与 `CaptureRound.cameras[].cameraId` 一致）。
- **归属包落地前**：前端经 `src/lib/pendingBackend.ts` 的 `pendingCall` 调用上表 D1 / D2 / C3 / D3 的新命令。Tauri 对未注册的命令拒绝为 `Command <命令名> not found`，前端据此显示“该功能待后端 <包> 实现”的占位（读图区为空态说明，走廊提取为画布提示，工作区操作为操作未完成提示），不当作故障。

## 六、口径

- **seq**：`frame_counter − 该相机基线`，从 1 起；frame_counter ≤ 基线为 Stale（上一件迟到帧，不计入本件）；同一相机重复的 seq 为 Duplicate，保留先到的那帧。
- **帧数 = 帧组数**：区域帧数 = `last_seq − first_seq + 1`；整件或整轮帧组数 = 各相机最大 seq 的最大值。标定与运行同一口径。单台相机的帧数只用于丢帧告警：与 deviceTriggers 相差 > 1，或与另外每一台相机都相差 > 1（`frame_group::loss_alarms`；[10, 10, 8] 只有第 3 台告警，前两台彼此一致，不因第 3 台少帧被连带告警），每台相机至多一条，两条依据都成立时报 deviceTriggers。
- **每相机基线**（段配方）：布防时取各相机本会话见到的最高 frame_counter 与上一件推算的下限中较大者；下一件基线 = max(原基线, 该相机本件见到的最高 frame_counter)（`Grouper::max_counter`），deviceTriggers 不参与。旧拍照点配方的下一件基线规则不变，见 D-SB-7。
- **帧数越界只告警**：区域帧组数 ∉ [N_exp − tol, N_exp + tol] 记 `frame_count_anomaly`，该区域照常检测，不改 OK/NG。本轮只有覆盖全部段的未分段区域，与 `expected_total ± tolerance_total` 比较；各段 N_exp 为 null 时不逐段核对。
- **short_part**：整件帧组数 < `expected_total − tolerance_total`（`expected_total` 为 null 时不判）。
- **expected_total**：各段 N_exp 都非空时为其和，否则取 `part_calib.expected_total`，都没有为 null。**tolerance_total** = Σ 各段 tolerance。
- **N_exp**：各轮帧组数均值四舍五入（.5 进位），记录 min/max/mean；建议容差 = max(默认容差, N − min, max − N)，操作员确认后才写入。

## 七、内存与队列

| 项 | 初值 | 说明 |
| --- | --- | --- |
| `IMAGE_BUDGET_PER_CAMERA` | 256 MiB（P1 核对后保持） | FramePool 每台相机在途图像字节上限（C2）；1280×1024 单字节灰度每帧 1,310,720 B，即 204 帧、25 Hz 下 8.2 s 积压（原写“约 190 帧、7.6 s”是估算，P1 校正）；只作用于 MVS 回调来源。预算满时回调丢弃该帧并记缺帧（不中断回调）。 |
| `FRAME_QUEUE` | 1024 条 | 原 64；内存已由预算封顶 |
| `MEASURE_QUEUE` | 1024 条 | 原 32；满时该帧记 Unmeasured |
| 录制队列 | 1024 条 | 段配方按帧计（C5 `BURST_QUEUE`）；满时该帧原图不录，part.json 记 rawDropped，证据状态 Incomplete。旧拍照点配方仍为 48 组（旧路径行为不变） |
| 示教写盘 `MAX_FRAMES_PER_CAMERA` | 8192 条 | 原 `MAX_TRIGGERS = 64`；满时该帧记 QueueFull 告警，不判失败 |
| `MEASURE_WORKERS` | 4（P1 实测；运行时取 min(4, 逻辑核 − 2)，至少 1） | 现为 `(逻辑核 / 2).clamp(2, 4)`（measure.rs:388）；C4 改用本值。现场机必须重跑 7.1 的探针后再确认 |

队列条数只防失控，不再决定内存。P1 已实测 lyFlow 每帧耗时与 PGM 写盘速率，结果与回填见 7.1；E2 的 3 × 25 Hz 压测仍是合并门槛（7.1 只证明测量和写盘各自撑得住，没有同时跑）。

### 7.1 P1 吞吐摸底（实测，2026-10-22）

**怎么跑**（探针是 `src-tauri/examples/throughput_probe.rs`，不是测试，不在 `cargo test` 里；写约 4 GB 临时文件、结束后自动删除，约 1 分钟；已发布的运行库即可，不要求带 lyFlow#32 的构建；dev profile 构建，与下面记录的数值同一配置）：

```powershell
$env:LYFLOW_CORE_DLL = 'src-tauri\target\dependency-cache\lyflow\1.1.5\lyflow_core.dll'
cargo run --manifest-path src-tauri/Cargo.toml --example throughput_probe
```

**量的是什么**：1280×1024 灰度图、一条横穿的暗胶条；走廊 121 mm、121 个测点（间距 1 mm），只用中线测量的算子（glue.taught_path + glue.bead_width），不带特征定位，与段模型的走廊测量是同一组算子。经生产加载路径（`release::publish` → `release::load` → `Prepared::load` → `warm`）后用 `Prepared::measure` 计时，包含 `Engine::run` 里的像素拷贝、lyFlow 运行与结果解析。每个并发度测 300 帧（串行即计划里的“连续测 300 次”）。写盘用生产录制的 `image_file::save_pgm`，每张 1,310,720 B 像素 + 17 B 头。

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
- `IMAGE_BUDGET_PER_CAMERA` 保持 256 MiB。依据：实测最长写盘停顿 4.7 s，25 Hz 下每台相机积压 118 帧约 155 MB，占预算的 58%；测量积压可以忽略（上一条）。停顿超过 8.2 s 才会触发预算满，此时按 D-SB-32 丢该帧记缺帧、不判失败。E2 压测里看到预算满再调，不预先放大。
- 其余队列条数（FRAME_QUEUE 等 1024、示教写盘 8192）不受 P1 数据影响，保持表中初值。

**75 fps 不可达时的降级方案**（由用户决定，PC 不自行降频）：

- 触发条件（任一）：现场机探针的串行 p95 代入公式得到的线程数超过 `min(4, 逻辑核 − 2)`；E2 压测出现预算满或测量队列满（记缺帧 / Unmeasured）。
- 第一步，降低 PLC 脉冲频率（改 PLC 点表里的频率，PC 无需改代码）。可持续帧率 = 0.7 × 该机满线程实测吞吐，Hz = ⌊可持续帧率 / 3⌋。以本机串行 p95 30.3 ms 按公式推算（推算值，未逐档实测）：25 Hz（75 fps）需 4 线程，20 Hz（60 fps）需 3，15 Hz（45 fps）需 2，10 Hz（30 fps）需 2。本机 6 线程吞吐 109–135 fps 的 70% 是 76–94 fps，即 25 Hz 刚好踩在 0.7 的线上，没有富余。
- 降频的连带影响：每段 N_exp 随频率变，必须重新标定，并按默认做法 6 分配新的 `plan_version`、提示 PLC 同步 shotCount；规格 §0 的 10～25 Hz 范围不变。
- 第二步才考虑缩短走廊（减少测点数）等需要改算法的做法；本轮不做。
- 在降级之前和之后，测量跟不上时都不会静默丢帧：该帧记缺帧或 Unmeasured，该组标 incomplete，整件 `flags.incomplete`，不改 OK/NG（D-SB-13）。

**局限**：

- 只在一台开发笔记本上测，现场工控机型号未知；本表不外推，现场机要重跑探针并把 p95 代入公式。
- 测量和写盘是分开测的，没有与 MVS 回调拷贝、界面、SQLite 同时跑；同时跑的余量由 E2 的 3 × 25 Hz 压测判定。
- 只测了 1280×1024、121 个测点的合成图；更大的图、更长的走廊、真实胶条图像对耗时的影响没测。

## 八、过渡规则（永久并存）

- **判别**：`RecipeDoc.segments` 为空 = 旧拍照点配方，走旧校验、旧节拍、旧发布（schema 3）；非空 = 段模型，走 `plan` / `frame_group` / `segmentation` / `burst_judge` / `calibration` 与新的发布路径（schema 4）。两套永久并存，旧路径一律保留并停用。
- **保留并停用**（段配方不经过；旧配方行为不变；代码标 `#[deprecated]` 或注释“旧拍照点模型”）：
  - `RecipeDoc.trigger_mode` 与 `TriggerMode::Stop`（停稳拍）；`RecipeSummary.trigger_mode`、`parts.trigger_mode` 列；
  - `ShotSpec`、`ShotViewSpec`、`PathPoints`、`recipe::Segment`、`plc_plan::trigger_groups`、`PlcParams`、shot_router 的拍照点路由；
  - `CaptureRound` 的 `recipe_shots()`、`planned_count` 与旧采集接口；`CompositeLayout` 与三目拼接拆分；
  - `Manifest::recipe_shots`、`Frame::image(view)` / `require_image`（段模型用新增的 `single_image()`）；
  - 旧 `release::publish` 与发布包 schema 3 的加载、`{device}-v{view}` 标定键；
  - 特征定位（fixture）模块、示教中线逐点比对、组选图；
  - 记录库 `part_shots`、`part_points`；
  - 前端 `cycle/pose.ts`、`cycle/vis.ts`、`WorkpieceOverview`、`teachingGroups`、`Fixture*` 等旧组件；
  - 前端旧配方的单帧示教与整圈采集页：`/recipe/teach`、`/recipe/capture` 在 `WorkspaceView.plan` 为空时挂载 `LegacyTeachingPage`、`LegacyCapturePage`（G0 之前的实现原样保留：组选图、采用本轮图像、逐点试测、特征定位、标定核对），以及 `TrialResult`、`TeachingSettings` 的旧签名和按拍照点下标的示教草稿（`frameDrafts` / `setFrameDraft`）。段配方另用 `CorridorSettings` 与按相机 id 的 `corridorDrafts` / `setCorridorDraft`。
  - **plan 为空时的分流**：页面分流以 `RecipeDoc.segments` 是否为空为准。后端 `WorkspaceView.plan` 自 D2 起对段配方输出（原定 D3）；段配方（`segments` 非空）而 `plan` 为 null 时（D2 之前的后端），`/recipe/teach`、`/recipe/capture` 显示“待后端 D3 实现”占位（`PlanPending`），不落到 `LegacyTeachingPage`、`LegacyCapturePage`——那里的采用本轮图像、逐点示教提交后会被后端 `validate_burst` 拒绝（段配方不能同时有拍照点）。进度、总览、步骤条等只读页面在 D3 之前对段配方仍按 `plan` 为空显示旧配方的空内容，不提供会被拒绝的操作。
  - **路由标题**：`/recipe/teach` 两种配方共用，标题写“单帧示教（旧配方）/ 相机走廊示教（段配方）”；总览页空态提示给段配方的写法是“相机走廊示教”（步骤条上的名称仍是“走廊示教”），不再提“单帧示教”。
  - **PLC 标签**：`shotCount`、`deviceShots`、`partEnd` 的预设标签在协议 6 的含义之后补写旧配方含义（触发次数、设备触发次数、触发计划发完）。
- **判定结果类型**：`judge::Judgement` 保留给旧配方；段配方用 `burst_judge::BurstJudgement`，两者并列，不做类型别名。界面与记录按 `burst` 字段是否为 null 区分。

## 九、相对计划的调整（供后续工作包对照）

以下是 WP00 落地骨架时相对计划 `new_types_or_interfaces` 的调整，后续包以代码骨架和本文件为准：

1. 新增 `plan::PartCalib { expected_total, calib }`；`expected_total(segments, part)`、`export(…, part)` 多一个 `Option<&PartCalib>` 参数；`plan_identity(…, expected_total)` 多一个参数（第二节、默认做法 6）。
2. `OffsetRef.x_ref` / `spread` 为 `Option<f64>`：ToEdge 段与未标定的段没有 x_ref（默认做法 2）。
3. `FrameState` 为单元枚举，增加 `OutOfCorridor`（默认做法 4）；失败原因放在 `FrameMetrics.error`。`FrameMetrics` 增加 `widths: Vec<Option<f32>>` 与 `spacing_mm`，用于帧内胶宽超差长度（`max_excursion_mm`）。
4. `burst_judge::judge` 的参数为 `(segments, expected_total, regions, groups, metrics, cameras: &[String], loss)`：`cameras` 用于把 `OffsetRef.camera` 换成相机下标，`expected_total` 来自 `PartCalib` 时无法从段推出。
5. `RegionResult` 增加 `reason`、`expected`、`tolerance`；`offset_unjudged` 为枚举 `OffsetUnjudged`。`BurstJudgement` 增加 `group_count`、`count_status`、`unchecked`（默认做法 1）。
6. `LossReason` 定义为 `Triggers { device_triggers } | Cameras { other, other_count }`；`synthetic::SyntheticSequence` 增加 `segments`（evaluate 组 SegInput 用），新增 `BoundaryError`。
7. 不做 `pub type PlcParams = PlanExport`（D-SB-25）；不删除任何旧路径（第八节）；D2 的标定应用改按 D-SB-21；相机配置不拒绝三目拼接（D-SB-11）。
8. 工作区新命令统一带 `revision`、返回 `WorkspaceView`，`workspace_calibration_apply` 的结果放在 `plan.calibration`（5.4）。
9. P1 的探针在计划步骤（串行 300 次、写 3000 张）之外加了几项，用来核对公式的线性外推和盘速下限：测量加 2/3/4/逻辑核 − 2 线程各 300 帧，写盘加 3 路并行与每张 `sync_data` 两种变体；计时用 `Prepared::measure`（单图路径），没有走 `measure_views`。结果见 7.1。
10. D1 示教采集轮次（相对计划步骤的调整，旧路径一律保留）：
    - **分流**：`recipe_capture_start` 按候选的 `doc.segments` 是否为空开轮次——段配方开 `CaptureRoundV2`，旧拍照点配方仍开旧 `CaptureRound`，旧轮次的规则不变（PLC 计划数 1–64、`deviceShots` 等于 `shotCount`、结束时计数相等、失败报 90/94）。计划中的“删除 MAX_TRIGGERS 和 1..=64”改为只对新轮次不设上限。`recipe_capture_get / stop / start` 返回旧或新轮次（前端按 `format` 区分），`recipe_capture_list` 两种都列（新轮次 `receivedCount` 为帧组数、`plannedCount` 为 PLC 估算值）。
    - **S7 示教事务**：新轮次布防用 `PlcPlan::teaching`（无拍照点、无段、计数为 0），`PlcSession::validate_plan` 只允许它用于已绑定采集轮次的事务，且不比较 shotCount / deviceShots；PLC 写来的值只记进 `plcShotCount`，`planVersion` 仍须为正数。旧轮次照旧按 shotCount 编拍照点计划并逐项核对。
    - **配组基线**：`shot_router::Ledger` 增加 `last_frame`（本会话最高 SDK 帧计数，`FrameMeta.frame`），新轮次每台相机的基线取它（没见过帧取 `counterAfterOpen`）；Chunk 触发计数与帧计数起点不同，不能拿触发计数的 `last` 当帧计数基线。
    - **seq 上限**：新轮次 seq > 32767（PLC Int，ΣN_exp 上限）按计数异常判整轮失败，防止异常帧号生成海量帧组（A2 遗留 F0-5 在 D1 消费侧的边界；`Grouper::offer` 本身未改）。
    - **整轮失败的情形**：PLC 开始前收到帧、会话变化（98）、手动帧、实拍轮次收到模拟计数、计数来源变化、帧计数不大于基线或比本轮已见的小、写图失败、PLC 开始/结束顺序异常。重复帧号丢弃并记 `duplicate`；丢包或无图、写盘队列满、图像内存预算满（`camera.rs` 预算满时把未拷图的帧交给本轮记缺帧）记缺帧并告警。接收中不设超时（D-SB-9），操作员中止记 `interrupted`（90/99）。
    - **新命令** `recipe_capture_frame_image(roundId, cameraId, seq)` 已注册，前端改为直接调用；图像路径由相机与 seq 推出（`{相机}/{seq:06}.pgm`），记录里写的路径只用于核对。
11. C3 段配方生产节拍（相对计划步骤的调整，旧路径一律保留）：
    - **分流**：布防时按 `Recipe::is_burst` 决定，段配方的件带 `Part.burst`（`cycle/burst.rs`），旧拍照点配方仍走 `ShotRouter` 与 `Part::judgement`，行为不变。计划中“删除 shot_router 按拍照点路由”“测试辅助 judge_delivered / judge_last_frame_during_end / judge_callback_channel 改返回 BurstJudgement”改为旧辅助原样保留（服务旧配方的存量用例），段配方另加 `judge_burst_delivered`；`tests_to_delete_or_rewrite` 里 cycle/tests、delivery、plc_session/tests、offline/tests、shot_router、measure 队满的用例保护的都是旧配方现行行为，未删改。`locate_health` 只由旧配方喂数据，段配方不经过。
    - **布防**：`shot_router::arm_frames` 按 SDK 帧计数定基线（D-SB-7），本会话没见过帧时取开流后计数起点；`Policy::burst_production` 认 ChunkTrigger / ChunkFrame / SdkFrame，`Policy::burst_offline` 另认 Synthetic，旧 `Policy::production` / `offline` 不改。会话或计数来源不可信拒绝布防报 98；段配方测量池全部超时未返回也拒绝布防。
    - **帧**：Grouper 的 seq 上限 = ΣN_exp + Σtol + 64，超出判 `Offer::OutOfRange`（不计入本件、不抬基线），丢弃并告警（A2 遗留 F0-5）；`Grouper::new` 的默认上限为 32767，与 D1 的 MAX_SEQ 相同，D1 行为不变。重复帧号丢弃，每台相机每件告警一次；上一件迟到帧记游离帧；布防后的手动帧丢弃并告警（不判 ERR，告警写明该相机此后帧组可能错开一格）；检测中会话变化或计数来源变为不可信判 ERR 98。回调通道满、图像内存预算满的帧不交给节拍，即缺帧。
    - **测量**：每帧经 `FramePool` 送测，结果经 `cycle::Input::FrameMeasured` 回节拍；QueueFull / Timeout 记 Unmeasured，Failed 记 Error；超过 T_proc 或到 drain 截止仍没结果记 Unmeasured；测量输出的帧身份与送测帧不符或含非有限数改记 Error（否则 C5 落库会拒绝整件记录）。都只记检测不完整，不判 99。
    - **收尾与判定**：partEnd 后已接受的帧都测完，且每台相机 max_seq ≥ deviceTriggers − 1（PLC 没报时不等）或到 drain 截止即判。故障（98、97）优先；否则 `Unsegmented` → `resolve_regions` → `loss_alarms(各相机帧数, deviceTriggers)` → `burst_judge::judge`。丢帧、帧数异常、疑似提前终止写运行日志（warn），结论不变。
    - **结果与记录**：`ResultView.burst` 带区域判定，`PartView.burst` 按 5.2 输出（帧格的 verdict 只看这一帧的独立指标，允差取覆盖全部段的较宽一档）。`BurstRecord.frames` 在 1..=帧组数内每台相机每帧一条、缺帧也有，`rawFile` 入库时为空（录制结果事后补）；`frames_expected` = ΣN_exp，`frames_received` = 帧组数，`triggers` = deviceTriggers。拒绝布防、在途中断、进程中断补记录的段配方件也带 `BurstRecord`（各相机帧数为 0 或已收到的帧，判定按结论摘要给出，结论字段与摘要相同）。诊断包按帧记录的 `rawFile` 打包段配方原图。
    - **在线读图**：`workspace_live_frame_image` 已注册（段配方预览缓存与旧配方各一份，各 128 MB）；前端仍经 `pendingCall` 调用，命令存在时照常返回图像。
    - **胶偏未判**：直线区域去掉首尾各一帧后参考相机没有可用 x 时标 `OffsetUnjudged::NoFrames`（“胶偏未判：参考相机没有可用帧”，TS 同步 `noFrames`；A4 遗留 F0-3），不再静默不判。
    - **带宽估算**：`camera::aggregate_bandwidth` 计入 GigE 协议开销（按不开巨帧约 1.05 倍；C2 遗留 F0-4），三台 130 万像素 25 Hz 由 786 Mbps 改为约 826 Mbps，超过单口 80% 预算。
12. D2 候选工作区（相对计划步骤的调整，旧路径一律保留）：
    - **旧路径不动**：`adopt_shots`、组选图（`workspace_select_group_image`、`check_group_selection`）、旧 `capture_for` 的 1..=64 与计数核对、对 `recipe_shots()` / `planned_count` 的调用都保留，旧配方行为不变；计划中的“删除 / 移除组选图命令”改为在新路径旁新增。新代码在 `workspace/burst.rs`，测试在 `workspace/burst_tests.rs`（计划写的是 workflow.rs / workflow_tests.rs）。
    - **`WorkspaceView.plan` 由 D2 输出**（5.1 原写 D3）：段配方时 productionPlc / candidatePlc / candidatePlcError / coverage 为 null、progress 没有步骤。`plan.progress`（ProgressV2）按整圈采集 → 段列表 → 走廊示教 → 多轮标定 → 验证 → 发布给出：“整圈采集”以本配方有完整采集轮次为完成；验证与发布沿用候选现有的验证记录与发布状态（段配方的验证项与发布切换仍归 D3）。
    - **Workspace 新字段**：`cameraTeaching: [{camera, frame: FrozenImage, round, seq}]`（示教帧冻结为候选 images/ 下的 PGM）、`calibration: {rounds: [CalibrationRound]}`。计划里的 `standard_rounds` 并进 `rounds`（每轮带 `standard`）；不另存 `result`：标定结果写进配方（N_exp、calib、partCalib、x_ref），`plan.calibration`（CalibrationView）每次由配方与轮次算出。建议容差 = max(该类段默认容差, N − min, max − N)，整件取各段默认容差之和。
    - **走廊示教**：`workspace_teach_camera` 只冻结所选帧、把草稿（走廊、当量、检测参数）写进 inspect，不在这里提取；提取用 `workspace_extract_corridor`（5.4）。当量缺省时沿用已有值，再缺省取这台相机的工位标定 `{device}`（模拟相机取模拟当量）。示教帧须来自本候选完整的新轮次、这台相机有这一帧、采集后设备配置没变。`cameraTeaching[].saved` = 走廊至少两点、不短于 1 px 且完整落在示教帧内。
    - **加入标定**：轮次须属于本候选、已完整完成、相机集合等于计划相机，各相机采集时的设备配置与当前设备、与已加入的轮次都相同；三台相机都已选示教帧且能建走廊测量图。整轮每帧用 `vision::measure_frame` 测（缺帧记 missing、测量失败记 error），每帧特征与分段结果写在候选目录 `calibration/<轮次>/`：`features.jsonl` 一行 `{camera: 相机编号, seq, metrics: FrameMetrics}`，1..=帧组数内每台相机每个 seq 一行，按计划相机、seq 排列；`boundaries.json` 为 SegOutcome（本轮恒为 `{"kind":"unsegmented"}`）。不写进采集轮次目录（特征依赖候选的走廊）。
    - **应用标定**：标定轮次（不含标准件轮次）不少于 `calibrationRounds`，否则报错且配方不变；写 partCalib，单段计划同时写该段 N_exp 与 calib；多段计划各段保持 null（视图 pending “分段算法未就绪”）。容差不写，由 `workspace_confirm_tolerance` 写入。
    - **x_ref**：只给单段计划的直线段；每轮标准件取参考相机去掉首尾各一帧（seq 1 与帧组数）后测成帧的 x 均值，再用 `calibration::x_ref` 取均值与极差，只用走廊与当前相同的标准件轮次。直线段启用了胶偏但没配 offsetRef 时按 AbsoluteX、计划第一台相机补上（D-SB-18 默认）；ToEdge 不写（“测量量未实现”）；多段计划不写（“分段算法未就绪”）。每次应用按当前标准件轮次重算，没有可用轮次时 x_ref 为 null。
    - **编辑约束**（`workspace_save_doc` 与 `workspace_update_segments` 共用 `update_doc`）：段的类型序列不变时 N_exp、calib、partCalib 不能改，x_ref 只能原样带过或清空，胶偏参考的测量量或相机变了 x_ref 清空；类型序列（段数、顺序、类型）变了清空全部 N_exp、calib、partCalib 与 x_ref，已加入的标定轮次保留（整件帧组数与分段无关）。
    - **前端**：D2 的命令已注册，前端仍经 `pendingCall` 调用（命令存在时与 `desktopCall` 相同）；改为直接调用、删去“待后端 D2”的占位分支与用例留给前端收口。
