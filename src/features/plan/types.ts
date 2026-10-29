/**
 * 段模型（分段连拍飞拍）的前后端契约。
 *
 * 前半部分镜像 src-tauri/src/{plan,frame_group,segmentation,burst_judge,calibration}.rs 的可序列化类型
 * （serde camelCase，Option 一律序列化为 null）；后半部分是各 IPC 新增的 DTO（cycle.rs、store.rs、recipe_capture/burst.rs、
 * workspace/burst.rs、workspace/burst_history.rs），字段与 Rust serde 输出逐一对应。字段口径与命令清单见 docs/decisions/segment-burst.md。
 */
import type { DetectParams, RecipeSummary, Verdict } from "../cycle/types";
import type { PartSummary } from "../history/types";
import type { CaptureCamera, FrozenImage, StepStatus } from "../workspace/types";

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

/** absoluteX = 胶中心相对走廊的横向偏移；toEdge = 胶中心到零件边的距离（D-SB-48）。 */
export type OffsetMeasure = "toEdge" | "absoluteX";

/** 零件边的明暗跳变方向（沿测站法向从胶边往外看）。 */
export type EdgePolarity = "brightToDark" | "darkToBright";

/** toEdge 找零件边的参数（D-SB-48），作用于胶偏参考相机。 */
export interface EdgeParams {
  polarity: EdgePolarity;
  /** 沿法向从胶边往外搜索的长度（mm），0–50 */
  searchMm: number;
  /** 边两侧灰度差下限，1–255 */
  contrastMin: number;
  /** 图纸胶边距（mm）：给了就以它为参考值，否则用标准件得出的 x_ref */
  nominalMm?: number | null;
}

/** 胶偏参考（只在直线段）：xRef / spread 由标准件多轮采集算出，不能手填。 */
export interface OffsetRef {
  measure: OffsetMeasure;
  /** 用哪台相机的帧测；必须属于计划相机 */
  camera: string;
  /** null = 未标定 */
  xRef: number | null;
  spread: number | null;
  /** 只在 toEdge 时有；absoluteX 必须为空 */
  edge?: EdgeParams | null;
}

/** 逐帧定向（D-SB-47）：直线段方向范围为示教方向 ± toleranceDeg（0–45°）。 */
export interface HeadingSettings {
  toleranceDeg: number;
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

/** 沿胶路的一段。 */
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
  /** 逐帧定向（D-SB-47）；null / 缺省 = 关闭，照旧用示教走廊 */
  heading?: HeadingSettings | null;
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

/** 计划导出 JSON（recipe_plan_export）；PLC 不填计划与帧数。 */
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

/** 除 measured、notInspected 外都只记“检测不完整”，不影响 OK/NG；notInspected = 这一帧所在段不由这台相机检测，按设计不测、不判定（D-SB-45）。 */
export type FrameState = "measured" | "missing" | "error" | "unmeasured" | "outOfCorridor" | "notInspected";

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
  /** 测量用的是第几段的走廊（PathSegment.index）；旧记录没有 */
  corridor?: number | null;
  /** 帧内胶中心相对走廊的法向偏移均值（absoluteX） */
  xMm: number | null;
  /** 逐帧定向测得的胶条方向（度，图像坐标）；按示教走廊测的帧没有 */
  headingDeg?: number | null;
  /** 逐帧定向没找到胶条，按预测段示教方向回退测量 */
  headingFallback?: boolean;
  /** 胶中心到零件边的距离（mm，toEdge）；找不到边为 null */
  edgeMm?: number | null;
}

/** 标定轮次一帧按当前段走廊重测的结果（workspace_calibration_frame，多轮标定页逐帧查看）。 */
export interface CalibrationFrameView {
  /** 所用走廊的段；这台相机不检测这一帧所在段时为 null，不测 */
  segment: number | null;
  corridor: [number, number][];
  /** 搜索半宽（px） */
  searchHalfPx: number | null;
  /** 逐站两侧胶边（px），无胶站为 null */
  edges: ([[number, number], [number, number]] | null)[];
  metrics: FrameMetrics;
}

/** 示教页全段试测（workspace_teach_preview_segment）：示教轮次里本段帧组 start–end 按候选已保存的走廊逐帧测得的指标。 */
export interface TeachPreviewSegment {
  round: string;
  start: number;
  end: number;
  frames: FrameMetrics[];
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

/** 一台相机的参数（D-SB-45）：像素当量、标定引用与检测参数；走廊只按“段 × 检测相机”示教，存在 segments。 */
export interface CameraInspect {
  camera: string;
  mmPerPx: number | null;
  calib: string | null;
  detect: DetectParams | null;
  /** 这台相机在它检测的各段的走廊（图像像素） */
  segments?: SegmentCorridor[];
  /** 逐帧定向的检测窗口（D-SB-47）；null / 缺省 = 未设置 */
  window?: InspectWindow | null;
  /** 标定的出胶点（图像像素，D-SB-47）；null / 缺省 = 由开启定向的各段走廊起点推出 */
  nozzle?: [number, number] | null;
}

/** 检测窗口：沿胶路距出胶点 [startMm, startMm + lengthMm]（mm）。 */
export interface InspectWindow {
  startMm: number;
  lengthMm: number;
}

export interface SegmentCorridor {
  /** PathSegment.index */
  segment: number;
  corridor: [number, number][];
}

/** RecipeDoc 的段模型字段。 */
export interface RecipeDocSegmentFields {
  segments: PathSegment[];
  /** 有序，计划相机顺序取它 */
  inspect: CameraInspect[];
  segmentDefaults: KindDefaults;
  /** 标定轮数，默认 10，范围 10..=100 */
  calibrationRounds: number;
  /** 比例分段边界两侧各留的过渡帧数，默认 2，范围 0..=5（D-SB-40） */
  boundaryGuard: number;
  /** 按逐帧定向的胶条方向序列细化比例边界（D-SB-47），默认 false；只作用于开启定向的段 */
  boundaryRefine?: boolean;
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

/** 在线检测：在途件的帧组（cycle::PartView.burst）。 */
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

/** 历史详情：段配方的部分（store::PartDetail.burst）；没有帧记录的记录为 null。 */
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

/** 历史列表：段配方记录带标记（疑似提前终止等单独显示）；没有帧记录的为 null。 */
export interface PartSummaryV2 extends PartSummary {
  flags: Flags | null;
}

export type CaptureState = "waitingStart" | "receiving" | "draining" | "complete" | "failed";

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

/** 历史件导入的轮次的来源（round.json 3，代替示教握手编号）；standard = 导入时操作员确认是标准件，加入标定按标准件轮次 */
export interface RoundOrigin { kind: "history"; cycleId: string; recordId: number; standard: boolean }

/** 段配方采集轮次（D1）：按帧号配组，不核对计数；帧元数据在 frames.jsonl，这里只给统计。format 3 加 origin。 */
export interface CaptureRoundV2 {
  format: 2 | 3;
  roundId: string;
  recipeId: string;
  cameras: CaptureCamera[];
  state: CaptureState;
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
  /** 历史件导入的轮次；示教实拍为空 */
  origin?: RoundOrigin | null;
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
  /** 本段的标定提示，如检测窗口覆盖不足、弯道定向回退比例高 */
  hints: string[];
}

export type SkipEnd = "head" | "tail";

/** 一段一头的跳过帧建议（workspace_calibration_skip_suggest，D-SB-50）。 */
export interface SkipSuggestView {
  segment: number;
  end: SkipEnd;
  /** 配方里当前的 skipHead / skipTail */
  current: number;
  /** 参与轮次的最大值；参与轮次不足时为 null，原因见 pending */
  suggested: number | null;
  min: number | null;
  max: number | null;
  /** 各标定轮次（不含标准件）的起涂（收尾）帧组数；不参与时 frames 为 null，原因见 note */
  rounds: { roundId: string; frames: number | null; note: string | null }[];
  pending: string | null;
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
  mmPerPx: number | null;
  /** 有像素当量，且它检测的每一段都有已保存的段走廊 */
  ready: boolean;
  /** 已选示教帧的段（D-SB-45：走廊只按段示教） */
  segments: SegmentTeachingView[];
  /** 定向用的出胶点：标定值，或由开启定向的各段走廊起点推出；推不出或不一致时为 null */
  nozzle: { point: [number, number]; calibrated: boolean } | null;
}

/** 示教页叠加的定向几何（度，图像坐标 0° 朝右、90° 朝下，与测量图一致）。 */
export interface HeadingOverlay {
  /** 本段方向范围 [lo, hi] */
  range: [number, number];
  /** 扇形搜索范围：本段与相邻段方向范围的并集 */
  sector: [number, number];
  /** 回退方向（本段示教方向） */
  fallback: number;
}

export interface SegmentTeachingView {
  segment: number;
  frame: FrozenImage;
  round: string;
  seq: number;
  /** 段走廊已保存且落在示教帧内 */
  saved: boolean;
  /** 本段开启逐帧定向且示教了走廊时的定向几何（按已保存的配方算） */
  heading: HeadingOverlay | null;
}

/** 候选里一段一台检测相机的示教帧（Workspace.cameraTeaching，D2 / D-SB-45）：从完整的新采集轮次冻结的一帧，这一段的走廊在这一帧上示教。 */
export interface CameraTeaching {
  camera: string;
  frame: FrozenImage;
  /** 来源采集轮次与帧组序号 */
  round: string;
  seq: number;
  /** PathSegment.index */
  segment?: number | null;
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

/** 走廊示教页里一条段走廊的草稿：走廊（图像像素）按段，像素当量与检测参数随相机（只读，用于校验与换算）。 */
export interface CameraTeachDraft {
  corridor: [number, number][];
  mmPerPx: number | null;
  /** null = 用配方的检测参数 */
  detect: DetectParams | null;
  /** 这台相机的检测窗口 */
  window: InspectWindow | null;
}

/** 候选工作区：段配方的计划部分（WorkspaceView.plan）。 */
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
  /** 候选导入的各件，示教件在前 */
  pieces: PieceView[];
}

/** 候选导入的一件（RB-1、RB-3）：示教件、标定件或标准件；界面用 SN 与时间标识，不显示轮次编号（RB-4）。 */
export interface PieceView {
  roundId: string;
  role: "teaching" | "calibration" | "standard";
  recordId: number | null;
  cycleId: string | null;
  sn: string | null;
  ts: number | null;
  groupCount: number;
  /** 边界复核：none = 还没对齐 */
  review: "none" | "draft" | "confirmed" | "stale";
  confirmedBy: "manual" | "aligned" | null;
  inCalibration: boolean;
  excludedGroups: number;
}

/** 配方库摘要的段模型字段。 */
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
  | { page: "corridor"; camera: string; segment?: number | null }
  | { page: "calibration" }
  | { page: "validation" }
  | { page: "camera" };

export interface ProgressStepV2 {
  id: StepIdV2;
  label: string;
  optional: boolean;
  status: StepStatus;
  detail: string;
  blockers: { message: string; target: ProgressTargetV2 }[];
}

export interface ProgressV2 {
  steps: ProgressStepV2[];
  next: { step: StepIdV2; label: string; target: ProgressTargetV2 } | null;
}
