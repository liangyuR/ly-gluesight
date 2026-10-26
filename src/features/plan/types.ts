/**
 * 段模型（分段连拍飞拍）的前后端契约。
 *
 * 前半部分镜像 src-tauri/src/{plan,frame_group,segmentation,burst_judge,calibration}.rs 的可序列化类型
 * （serde camelCase，Option 一律序列化为 null）；后半部分是各 IPC 新增的 DTO（cycle.rs、store.rs、recipe_capture/burst.rs、
 * workspace/burst.rs、workspace/burst_history.rs），字段与 Rust serde 输出逐一对应。字段口径与命令清单见 docs/decisions/segment-burst.md。
 *
 * 与旧拍照点模型并存：配方 segments 为空即旧模型，cycle/workspace/history 的旧类型不动；
 * 段配方的数据都挂在旧 DTO 新增的可空字段上（PartView.burst、ResultView.burst、PartDetail.burst、WorkspaceView.plan…）。
 */
import type { DetectParams, RecipeSummary, Verdict } from "../cycle/types";
import type { PartSummary } from "../history/types";
import type { CaptureCamera, CaptureRound, FrozenImage, ProgressStep } from "../workspace/types";

// ───────────── plan.rs ─────────────

export type SegmentKind = "straight" | "corner";

/** 胶宽（mm），每帧独立判。 */
export interface WidthCheck {
  enabled: boolean;
  nominalMm: number;
  tolUpper: number;
  tolLower: number;
  /** 一帧内允许的累计超差长度（mm）：不超过时为 OK_WITH_EXCURSION */
  maxExcursionMm: number;
  /** 均匀性：区域内各帧 widthMean 的极差上限，只在直线区域判；null 不判 */
  maxRangeMm: number | null;
}

/** 断胶，每帧独立判。 */
export interface BreakCheck {
  enabled: boolean;
  maxGapMm: number;
  /** 帧内有胶站最低占比（0–1） */
  minPresent: number;
}

/** 溢胶：每帧最大胶宽超过上限判 NG_WIDTH，原因“溢胶”。 */
export interface OverflowCheck {
  enabled: boolean;
  maxWidthMm: number;
}

/** 段级胶偏（只在直线段）。 */
export interface OffsetCheck {
  enabled: boolean;
  maxMeanShiftMm: number;
  maxLocalDevMm: number;
}

export interface SegmentChecks {
  width: WidthCheck;
  /** Rust 字段名 gap，JSON 名 break */
  break: BreakCheck;
  overflow: OverflowCheck;
  /** 弯道段必须为 null */
  offset: OffsetCheck | null;
}

/** 本轮只实现 absoluteX；toEdge 的段结果标“胶偏未判：测量量未实现”。 */
export type OffsetMeasure = "toEdge" | "absoluteX";

/** 胶偏参考（只在直线段）：xRef / spread 由标准件多轮采集算出，不能手填。 */
export interface OffsetRef {
  measure: OffsetMeasure;
  /** 用哪台相机的帧测；必须属于计划相机 */
  camera: string;
  /** null = 未标定（toEdge 恒为 null） */
  xRef: number | null;
  spread: number | null;
}

/** 多轮采集的帧组数统计。 */
export interface SegmentCalib {
  rounds: number;
  min: number;
  max: number;
  mean: number;
}

/** 检测区间 [aMm, bMm]，区间外为“未检区域”。 */
export interface InspectRange {
  aMm: number;
  bMm: number;
}

/** 沿胶路的一段（与旧模型 cycle/types 的 Segment 同名不同义，这里叫 PathSegment）。 */
export interface PathSegment {
  /** 1..N，沿胶路顺序连续 */
  index: number;
  kind: SegmentKind;
  lengthMm: number | null;
  /** N_exp：只能由多轮采集写入；null = 待标定 */
  expectedFrames: number | null;
  /** 帧数容差，默认 1 */
  tolerance: number;
  calib: SegmentCalib | null;
  checks: SegmentChecks;
  offsetRef: OffsetRef | null;
  /** 已停用的毫米检测区间（判定从未排除区间外的帧），只为读入旧配方保留；改用 skipHead / skipTail */
  inspectRange: InspectRange | null;
  /** 检测相机（D-SB-41）：计划相机的子集；空 = 全部计划相机 */
  cameras: string[];
  /** 段首、段尾跳过的帧组数：这些帧不判定，结果里列为未检区域 */
  skipHead: number;
  skipTail: number;
}

/** 整件标定：多轮整件帧组数统计；本轮多段配方凭它得到 expectedTotal 并发布。 */
export interface PartCalib {
  expectedTotal: number;
  calib: SegmentCalib;
}

export interface KindDefault {
  tolerance: number;
  checks: SegmentChecks;
}

/** 配方默认参数：直线段与弯道段各一套，容差默认 1。 */
export interface KindDefaults {
  straight: KindDefault;
  corner: KindDefault;
}

export interface ExportSegment {
  index: number;
  kind: SegmentKind;
  lengthMm: number | null;
  expectedFrames: number | null;
  tolerance: number;
  inspectRange: InspectRange | null;
  skipHead?: number;
  skipTail?: number;
}

/** 计划导出 JSON（recipe_plan_export）；协议 7 下 PLC 不再填计划与帧数。 */
export interface PlanExport {
  protocolVersion: number;
  recipeId: string;
  productCode: number;
  planVersion: number;
  cameras: string[];
  segments: ExportSegment[];
  expectedTotal: number | null;
  toleranceTotal: number;
}

// ───────────── frame_group.rs ─────────────

export interface FrameGroup {
  seq: number;
  /** 按计划相机顺序 */
  present: boolean[];
  incomplete: boolean;
}

export type CountStatus = "normal" | "low" | "high";

export type LossReason =
  | { type: "triggers"; deviceTriggers: number }
  | { type: "cameras"; other: number; otherCount: number };

/** 某台相机疑似丢帧：只告警。camera 是计划相机下标。 */
export interface LossAlarm {
  camera: number;
  count: number;
  reason: LossReason;
}

// ───────────── segmentation.rs ─────────────

export interface Boundary {
  afterSegment: number;
  frameSeq: number;
  confidence: number;
}

export type SegOutcome = { kind: "unsegmented" } | { kind: "boundaries"; boundaries: Boundary[] };

export type UnsegReason = "algorithmPending" | "lowConfidence" | "transition";

export type RegionKind =
  | { type: "straight"; segment: number }
  | { type: "corner"; segment: number }
  | { type: "unsegmented"; firstSegment: number; lastSegment: number; reason: UnsegReason }
  /** 段首尾跳过的帧：不判定 */
  | { type: "skipped"; segment: number };

/** 一段连续帧组；帧数 = lastSeq − firstSeq + 1。 */
export interface Region {
  kind: RegionKind;
  firstSeq: number;
  lastSeq: number;
}

// ───────────── burst_judge.rs ─────────────

/** 除 measured 外都只记“检测不完整”，不影响 OK/NG。 */
export type FrameState = "measured" | "missing" | "error" | "unmeasured" | "outOfCorridor";

/** 一台相机一帧的独立指标；数值只在 state = measured 时有意义。 */
export interface FrameMetrics {
  /** 计划相机下标 */
  camera: number;
  seq: number;
  state: FrameState;
  error: string | null;
  widthMin: number | null;
  widthMax: number | null;
  widthMean: number | null;
  /** 沿走廊逐站胶宽，null = 该站无胶或测不了 */
  widths: (number | null)[];
  spacingMm: number;
  gapMaxMm: number;
  presentRatio: number;
  /** 帧内胶中心相对走廊的法向偏移均值（absoluteX） */
  xMm: number | null;
}

export interface WidthStats {
  min: number;
  max: number;
  mean: number;
}

export interface OffsetStat {
  meanShift: number;
  maxDev: number;
}

/** 胶偏未判的原因；弯道区域本来就不判，不标。 */
export type OffsetUnjudged = "unsegmented" | "lowConfidence" | "measureNotImplemented" | "noReference" | "noFrames";

export interface RegionResult {
  region: Region;
  verdict: Verdict;
  reason: string;
  frameCount: number;
  /** 单段区域为该段 N_exp，覆盖全部段的未分段区域为 expectedTotal；无从比较为 null */
  expected: number | null;
  tolerance: number;
  countStatus: CountStatus | null;
  cameraFrames: number[];
  width: WidthStats | null;
  offset: OffsetStat | null;
  offsetUnjudged: OffsetUnjudged | null;
  incompleteGroups: number[];
}

/** 整件标记：都只告警，不改 OK/NG。 */
export interface Flags {
  incomplete: boolean;
  frameCountAnomaly: boolean;
  offsetUnjudged: boolean;
  unsegmented: boolean;
  /** 疑似提前终止：整件帧组数 < expectedTotal − toleranceTotal */
  shortPart: boolean;
}

/** 未检区域：跳过的帧组 fromSeq–toSeq；2026-10-26 之前的记录按毫米检测区间写 fromMm / toMm。 */
export interface Uninspected {
  segment: number;
  fromSeq?: number | null;
  toSeq?: number | null;
  fromMm?: number | null;
  /** 段长未知时为 null */
  toMm?: number | null;
}

/** 本轮没有算法的检测项（“未检项”）。 */
export type UncheckedItem = "bubble" | "stringing";

export interface BurstJudgement {
  verdict: Verdict;
  plcCode: number;
  faultCode: number;
  reason: string;
  groupCount: number;
  countStatus: CountStatus | null;
  regions: RegionResult[];
  flags: Flags;
  alarms: LossAlarm[];
  uninspected: Uninspected[];
  unchecked: UncheckedItem[];
}

// ───────────── calibration.rs ─────────────

export interface CalibOutcome {
  nExp: number;
  calib: SegmentCalib;
  /** 建议容差；操作员确认（workspace_confirm_tolerance）后才写进配方 */
  suggestedTolerance: number;
}

// ───────────── 配方新增字段（C1，RecipeDoc schema 8）─────────────

/** 一台相机的检测设置：按直线段几何示教的一条走廊（图像像素）与当量。 */
export interface CameraInspect {
  camera: string;
  corridor: [number, number][];
  mmPerPx: number | null;
  calib: string | null;
  detect: DetectParams | null;
  /** 按段示教的走廊（D-SB-42）：落在该段的帧用它，其余用默认走廊 corridor */
  segments?: SegmentCorridor[];
}

export interface SegmentCorridor {
  /** PathSegment.index */
  segment: number;
  corridor: [number, number][];
}

/** RecipeDoc 新增字段；segments 为空即旧拍照点配方（其余新字段被忽略）。 */
export interface RecipeDocSegmentFields {
  segments: PathSegment[];
  /** 有序，计划相机顺序取它 */
  inspect: CameraInspect[];
  segmentDefaults: KindDefaults;
  /** 标定轮数，默认 10，范围 10..=100 */
  calibrationRounds: number;
  /** 比例分段边界两侧各留的过渡帧数，默认 2，范围 0..=5（D-SB-40） */
  boundaryGuard: number;
  partCalib: PartCalib | null;
}

// ───────────── IPC DTO ─────────────

/** 在线帧格状态：到达前 waiting，测量中 measuring，之后为帧的最终状态。 */
export type FrameCellStatus = "waiting" | "measuring" | FrameState;

export interface FrameCellView {
  status: FrameCellStatus;
  /** 这一帧单独判的结论；未测完为 null */
  verdict: Verdict | null;
  frameCounter: number | null;
}

/** 一个帧组：cells 按计划相机顺序。 */
export interface FrameGroupView {
  seq: number;
  incomplete: boolean;
  cells: FrameCellView[];
}

/** 在线检测：段配方在途件（cycle::PartView.burst，C3）。旧配方为 null。 */
export interface PartViewBurst {
  cameras: string[];
  expectedTotal: number | null;
  toleranceTotal: number;
  /** PLC partEnd 时的 deviceTriggers，只作参考；partEnd 前为 null */
  deviceTriggers: number | null;
  /** 已见帧组数 = 各相机最大 seq 的最大值 */
  groupCount: number;
  /** 各相机已接受的帧数 */
  cameraFrames: number[];
  measuredFrames: number;
  pendingMeasure: number;
  groups: FrameGroupView[];
}

/** 历史：一台相机一帧的记录（C5 part_frames）。1..=groupCount 内每个 (camera, seq) 都有一条，缺帧也有。 */
export interface FrameRecord {
  /** 计划相机下标 */
  camera: number;
  seq: number;
  status: FrameState;
  frameCounter: number | null;
  triggerCounter: number | null;
  /** BurstJudgement.regions 的下标；不在任何区域为 null */
  region: number | null;
  /** status = missing 时为 null */
  metrics: FrameMetrics | null;
  /** 录制根内相对路径；未录制或录制队满丢弃为 null */
  rawFile: string | null;
}

/** 历史详情：段配方的部分（store::PartDetail.burst，C5）。旧记录为 null。 */
export interface PartDetailBurst {
  /** 本件配方快照的段 */
  segments: PathSegment[];
  cameras: string[];
  expectedTotal: number | null;
  toleranceTotal: number;
  deviceTriggers: number | null;
  cameraFrames: number[];
  judgement: BurstJudgement;
  /** 按 (camera, seq) 排序 */
  frames: FrameRecord[];
}

/** 历史列表：段配方记录带标记（疑似提前终止等单独显示）；旧记录 flags 为 null。 */
export interface PartSummaryV2 extends PartSummary {
  flags: Flags | null;
}

/** 示教采集轮次失败原因（新轮次）；计数不符不再是失败。 */
export type CaptureFailureV2 =
  /** 相机断连 / 出错 → PLC 90 + 98 */
  | { kind: "camera"; cameraId: string }
  /** 设备会话变化 → 90 + 98 */
  | { kind: "sessionChanged"; cameraId: string }
  /** PLC 开始 / 结束顺序异常 */
  | { kind: "plc" }
  | { kind: "startTimeout" }
  /** 写盘超时 → 90 + 99 */
  | { kind: "writeTimeout"; queued: number; written: number }
  /** 操作员中止或应用退出 → 90 + 99 */
  | { kind: "interrupted" }
  | { kind: "other" };

export interface MissingFrame {
  camera: number;
  seq: number;
}

/** 新示教采集轮次的逐帧告警（D1）：这些帧都不判整轮失败。duplicate 丢弃重复帧号、保留先到的；其余三种不写图，另记在 missingFrames。 */
export interface CaptureAlarm {
  kind: "duplicate" | "queueFull" | "budgetDropped" | "corrupt";
  /** 本轮相机下标 */
  camera: number;
  seq: number;
}

/** 新示教采集轮次（D1）：按帧号配组，不核对计数；帧元数据在 frames.jsonl，这里只给统计。 */
export interface CaptureRoundV2 {
  format: 2;
  roundId: string;
  recipeId: string;
  cameras: CaptureCamera[];
  state: CaptureRound["state"];
  /** PLC 写来的 shotCount（示教轮次可为估算值，不核对） */
  plcShotCount: number | null;
  plcPlanVersion: number | null;
  /** partEnd 时的 deviceTriggers，只作参考 */
  deviceTriggers: number | null;
  /** 整轮帧组数（标定统计用） */
  groupCount: number;
  cameraFrames: number[];
  incompleteGroups: number[];
  missingFrames: MissingFrame[];
  lossAlarms: LossAlarm[];
  /** 逐帧告警，最多 1024 条（缺帧本身见 missingFrames，不受此限） */
  alarms: CaptureAlarm[];
  createdAt: number;
  endedAt: number | null;
  error: string | null;
  failure: CaptureFailureV2 | null;
  simulated: boolean;
}

export interface CalibrationRoundView {
  roundId: string;
  /** 标准件轮次：用于 xRef */
  standard: boolean;
  groupCount: number;
  cameraFrames: number[];
  incompleteGroups: number;
  lossAlarms: number;
  createdAt: number;
}

export interface SegmentCalibView {
  index: number;
  kind: SegmentKind;
  /** null = 本段未能标定，原因见 pending */
  outcome: CalibOutcome | null;
  /** 如“分段算法未就绪” */
  pending: string | null;
  /** 配方里当前的容差 */
  tolerance: number;
  xRef: { xRef: number; spread: number } | null;
  /** 如“测量量未实现”“标准件轮次不足” */
  xRefPending: string | null;
}

/** 多轮标定（workspace_calibration_apply 返回，D2）。 */
export interface CalibrationView {
  /** 配方 calibrationRounds */
  minRounds: number;
  rounds: CalibrationRoundView[];
  /** 整件帧组数统计 → PartCalib */
  part: CalibOutcome | null;
  segments: SegmentCalibView[];
  /** 应用后的 ΣN_exp */
  expectedTotal: number | null;
  toleranceTotal: number;
  /** N_exp / expectedTotal 有变化：发布时分配新计划版本 */
  planVersionChange: boolean;
}

export interface CameraTeachingView {
  camera: string;
  /** 走廊已保存 */
  saved: boolean;
  frame: FrozenImage | null;
  mmPerPx: number | null;
  /** 已选示教帧的段走廊（D-SB-42） */
  segments?: SegmentTeachingView[];
}

export interface SegmentTeachingView {
  segment: number;
  frame: FrozenImage;
  round: string;
  seq: number;
  /** 段走廊已保存且落在示教帧内；否则这一段测量用默认走廊 */
  saved: boolean;
}

/** 候选里一台计划相机的示教帧（Workspace.cameraTeaching，D2）：从完整的新采集轮次冻结的一帧，走廊在这一帧上示教。 */
export interface CameraTeaching {
  camera: string;
  frame: FrozenImage;
  /** 来源采集轮次与帧组序号 */
  round: string;
  seq: number;
}

/** 加入多轮标定的一轮（Workspace.calibration.rounds，D2）；界面读 plan.calibration（CalibrationView），这里是候选里存的原始记录。 */
export interface CalibrationRound {
  roundId: string;
  /** 标准件轮次：只用于 xRef，不计入 N_exp 的轮数 */
  standard: boolean;
  createdAt: number;
  /** 加入时的计划相机顺序；下面几项按它排列 */
  cameras: string[];
  /** 各相机采集时的设备配置（后端原样保存，前端不解读） */
  cameraConfig: unknown[];
  groupCount: number;
  cameraFrames: number[];
  incompleteGroups: number;
  lossAlarms: number;
  /** 测每帧特征时各相机的走廊身份（前端不解读） */
  corridors: unknown[];
  /** 各相机去掉首尾各一帧后测成的帧的胶中心 x 均值（mm）；没有可用帧为 null */
  xMeans: (number | null)[];
}

/** workspace_teach_camera 的 draft：在选中帧上定出的走廊（图像像素，按直线段几何）、当量与检测参数，
 * 后端写进 RecipeDoc.inspect 中这台相机的一项（与 CameraInspect 的可编辑部分一一对应）。 */
export interface CameraTeachDraft {
  corridor: [number, number][];
  mmPerPx: number | null;
  /** null = 用配方的检测参数 */
  detect: DetectParams | null;
}

/** 候选工作区：段配方的计划部分（WorkspaceView.plan，D2）。旧配方为 null。 */
export interface WorkspacePlanView {
  segments: PathSegment[];
  cameras: string[];
  segmentDefaults: KindDefaults;
  calibrationRounds: number;
  partCalib: PartCalib | null;
  expectedTotal: number | null;
  toleranceTotal: number;
  /** expectedTotal 非空：满足发布的帧数门槛 */
  calibrated: boolean;
  cameraTeaching: CameraTeachingView[];
  calibration: CalibrationView | null;
  productionExport: PlanExport | null;
  candidateExport: PlanExport | null;
  candidateExportError: string | null;
  /** 候选 expectedTotal 与生产不同 */
  shotCountChanged: boolean;
  progress: ProgressV2;
}

/** 配方库：RecipeSummary 新增字段（C1）。旧配方 segmentCount = 0。 */
export interface RecipeSummaryV2 extends RecipeSummary {
  segmentCount: number;
  expectedTotal: number | null;
  toleranceTotal: number;
  calibrated: boolean;
}

/** 段配方的进度步骤；路由：capture→/recipe/capture，segments→/recipe/segments，corridor→/recipe/teach，
 * calibration→/recipe/calibration，validation / publish→/recipe/validation。 */
export type StepIdV2 = "capture" | "segments" | "corridor" | "calibration" | "validation" | "publish";

export type ProgressTargetV2 =
  | { page: "capture" }
  | { page: "segments" }
  | { page: "corridor"; camera: string }
  | { page: "calibration" }
  | { page: "validation" }
  | { page: "camera" };

export interface ProgressStepV2 extends Omit<ProgressStep, "id" | "blockers"> {
  id: StepIdV2;
  blockers: { message: string; target: ProgressTargetV2 }[];
}

export interface ProgressV2 {
  steps: ProgressStepV2[];
  next: { step: StepIdV2; label: string; target: ProgressTargetV2 } | null;
}
