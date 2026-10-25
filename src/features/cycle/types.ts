
import type { BurstJudgement, CameraInspect, KindDefaults, PartCalib, PartViewBurst, PathSegment, RecipeDocSegmentFields } from "../plan/types";

/** 段模型（分段连拍飞拍）的契约类型，定义在 plan/types.ts；这里转出供 cycle 相关页面使用。 */
export type {
  BurstJudgement, CameraInspect, CountStatus, Flags, FrameCellStatus, FrameCellView, FrameGroupView, FrameMetrics, FrameState,
  KindDefault, KindDefaults, LossAlarm, PartCalib, PartViewBurst, PathSegment, PlanExport, Region, RegionKind, RegionResult,
  RecipeDocSegmentFields, RecipeSummaryV2, SegmentChecks, SegmentKind, Uninspected,
} from "../plan/types";

export type Phase = "IDLE" | "VALIDATE" | "ACQUIRE" | "DRAIN" | "JUDGE" | "REPORT" | "RELEASE" | "FAULT";
export type FrameStatus = "waiting" | "measuring" | "done" | "error" | "missing";
export type Verdict = "OK" | "OK_WITH_EXCURSION" | "NG_POSITION" | "NG_WIDTH" | "NG_ABSOLUTE" | "NG_GAP" | "ERR_INSPECT";
export type TriggerMode = "fly" | "stop";
export type ProductSource = "plc" | "manual";
/** 离线工况，与后端 offline::scenario::Scenario（camelCase）同名。
 * cameraLost / pulseJitter / cornerSlowdown / notInspected / earlyEnd 是分段连拍（段配方）的脉冲串工况，
 * 只改虚拟 PLC 的脉冲与握手、不改图像，样本集也能选；旧拍照点配方选了后端拒绝。 */
export type Scenario = "normal" | "excursion" | "gap" | "lostFrame" | "locateFail" | "locateFailGap" | "countMismatch" | "motionTimeout" | "ackMismatch" | "random"
  | "cameraLost" | "pulseJitter" | "cornerSlowdown" | "notInspected" | "earlyEnd";
export type OfflineFault = "disconnect" | "heartbeatStop" | "heartbeatResume" | "rejectWrite" | "rejectRead" | "delay" | "clear";
export type RecordMode = "off" | "failed" | "all";

export interface JudgeParams {
  nominal: number;
  tolUpper: number;
  tolLower: number;
  absMin: number;
  absMax: number;
  maxExcursionLen: number;
}

/** 一段：一个已示教、要检的拍照点。测量点 first..first+count 属于它，段内弧长 (j - first) × spacing。 */
export interface Segment {
  /** 如 "P2 · J1" */
  name: string;
  /** 拍照点下标 */
  shot: number;
  view: number;
  first: number;
  count: number;
  /** 胶条中线相对示教中线的横向偏移（mm）；null 时不判位置 */
  position: JudgeParams | null;
  /** 胶宽（mm）；null 时不判胶宽 */
  width: JudgeParams | null;
  /** 允许的连续缺胶长度（mm） */
  maxGapLen: number;
  /** 有胶站的最低占比（0–1）：累计缺胶超过 maxGapLen 且占比低于它时判断胶 */
  minPresent: number;
}

export type Polarity = "dark" | "light";

/** 沿示教中线找胶的参数（mm）。 */
export interface DetectParams {
  /** 沿法向的搜索半宽 */
  searchMm: number;
  polarity: Polarity;
  /** 胶宽的搜索范围 [下限, 上限] */
  widthRange: [number, number];
}

/** 一个拍照点的判定限值。 */
export interface ShotLimits {
  position: JudgeParams | null;
  width: JudgeParams | null;
  maxGapLen: number;
  /** 有胶站的最低占比（0–1），防零散断胶 */
  minPresent: number;
  /** 量具确认件编号：位置名义值不为 0 时必填 */
  gaugeRef?: string;
}

/** 特征定位（glue.locate）：示教框（不含外圈）、模板来源与门限。模板从来源冻结示教原图按框加 4 px 外圈裁出。 */
export interface FixtureSpec {
  /** 示教框 [x, y, w, h]（示教图像素） */
  rect: [number, number, number, number];
  /** 模板来源：裁模板用的冻结示教原图编号。由后端写入，前端原样回传，后端不采用前端的值 */
  source: string;
  searchRadiusPx: number;
  /** 角度扫描范围 ±°；0 = 只找平移 */
  angleRangeDeg: number;
  minScore: number;
  /** 次峰 / 主峰上限；1 = 不判 */
  maxPeakRatio: number;
  /** 两端切向搬移上限（mm）；不设时用临时默认公式。只由验证建议写入 */
  maxEndShiftMm?: number;
  /** 示教余量 m（mm）：胶路首末各长 m 的一段永远不判。只由验证建议写入，不设为 0 */
  endMarginMm?: number;
}

/** 特征定位的位姿（帧记录追溯与叠图用）：T 由 offset、angle 与配方快照里 fixture.rect 的示教中心还原。 */
export interface PoseRecord {
  accepted: boolean;
  /** 不采用的原因：glue.locate 的 reason，或 "angle_unobservable" */
  reason: string | null;
  score: number | null;
  peakRatio: number | null;
  angle: number | null;
  /** 模板中心相对示教位置的平移（px） */
  offset: [number, number] | null;
  /** 两端切向搬移（mm） */
  endShiftMm: [number, number] | null;
  /** 两端各排除的长度（mm） */
  excludedMm: [number, number] | null;
  endLimitMm: number;
}

/** 一个拍照点：机器人走到 Pose 时 PLC 触发这台相机拍一帧，在这帧里沿示教中线量胶。可选字段不设时不发送（不发 null）。 */
export interface ShotViewSpec {
  view: number; enabled: boolean; path: [number, number][]; mmPerPx?: number; detect?: DetectParams; limits?: ShotLimits; calib?: string; fixture?: FixtureSpec;
}

export interface ShotSpec {
  views?: ShotViewSpec[];
  /** 配方内唯一，如 P1 */
  id: string;
  /** 现场机器人 / PLC 程序里的 Pose 标识；不要求唯一 */
  poseId: string;
  /** 相机编号 */
  camera: string;
  view: number;
  /** 标定引用；不给时用这台相机的工位标定 */
  calib?: string;
  /** 胶条名：同一条胶上的拍照点同名 */
  bead: string;
  /** 不检：要求这一帧到达，但不量不判 */
  skip: boolean;
  /** 示教的胶路中线（图像像素，从胶嘴一侧往外）；[] 表示尚未示教 */
  path: [number, number][];
  /** 示教时的像素当量（mm/px） */
  mmPerPx?: number;
  /** 单独设的检测参数；不给时用配方的 */
  detect?: DetectParams;
  /** 单独设的限值；不给时用配方的 */
  limits?: ShotLimits;
  /** 特征定位；不给时胶偏按静态示教线量 */
  fixture?: FixtureSpec;
}

/** 编译后的配方（后端 recipe::Recipe）：WorkspaceView.layout、配方快照、Comparison.candidateRecipe。 */
export interface Recipe {
  id: string;
  name: string;
  version: number;
  /** PLC 计划版本（见 RecipeDoc.planVersion） */
  planVersion: number;
  revisionId: string;
  teachingId?: string | null;
  productCode: number;
  /** 缺省为正式版 */
  stage?: RecipeStage;
  triggerMode: TriggerMode;
  /** 配方文件格式版本（后端 recipe::RECIPE_SCHEMA） */
  schemaVersion: number;
  /** 站距（mm） */
  spacing: number;
  filterWindow: number;
  detect: DetectParams;
  limits: ShotLimits;
  shots: ShotSpec[];
  /** 各相机的曝光与增益 */
  cameras: Record<string, CameraImaging>;
  /** 旧拍照点模型的测量段（段配方为空） */
  segments: Segment[];
  /** 各站：所在拍照点图像里的像素位置、所属段与拍照点（旧拍照点模型，段配方为空） */
  points: { x: number[]; y: number[]; seg: number[]; k: number[] };
  /** 段配方的段列表（即 RecipeDoc.segments，编译后改名，避免与旧模型的 segments 重名）；为空即旧拍照点配方 */
  pathSegments: PathSegment[];
  /** 段配方的检测相机，顺序即计划相机顺序 */
  inspect: CameraInspect[];
  segmentDefaults: KindDefaults;
  calibrationRounds: number;
  partCalib: PartCalib | null;
}

/** 配方文件内容，配方页编辑它。
 * 段模型字段（segments、inspect、segmentDefaults、calibrationRounds、partCalib）后端 schema 8 起总是输出；前端提交时可缺省（后端按默认值补齐），
 * 所以这里可缺省。segments 为空或缺省 = 旧拍照点配方，非空 = 段配方（此时 shots 为空、triggerMode 为 fly）。 */
export interface RecipeDoc extends Partial<RecipeDocSegmentFields> {
  id: string;
  name: string;
  version: number;
  /** PLC 拍照计划版本：配方库分配，只在计划身份变化时换新号；0 或缺省为未分配。
   * 旧拍照点配方的身份是产品代码与拍照点顺序 / Pose / 相机；段配方是产品代码、各段序号 / 类型 / N_exp、相机顺序与 ΣN_exp */
  planVersion?: number;
  teachingId?: string | null;
  productCode: number;
  /** 缺省为正式版；发布时由后端写入 */
  stage?: RecipeStage;
  triggerMode: TriggerMode;
  /** 配方文件格式版本（后端 recipe::RECIPE_SCHEMA）；不符的文件后端拒绝 */
  schemaVersion: number;
  /** 站距（mm） */
  spacing: number;
  filterWindow: number;
  /** 拍照点没单独设时用的检测参数与限值 */
  detect: DetectParams;
  limits: ShotLimits;
  shots: ShotSpec[];
  /** 各相机的曝光与增益：拍照点用到的相机各一组 */
  cameras: Record<string, CameraImaging>;
}

export interface CameraImaging {
  exposureUs: number;
  gainDb: number;
}

export const DEFAULT_IMAGING: CameraImaging = { exposureUs: 60, gainDb: 6 };

/** 配方阶段：collect = 采集版（只供维护模式采图，不判定），formal = 正式版；后端缺省即正式版。 */
export type RecipeStage = "formal" | "collect";

export interface RecipeSummary {
  id: string;
  name: string;
  version: number;
  revisionId: string;
  productCode: number;
  shotCount: number;
  triggerMode: TriggerMode;
  cameras: string[];
  length: number;
  /** PLC 计划版本 */
  planVersion: number;
  /** PLC 触发次数；拍照点编不成触发分组时为 null。段配方为 expectedTotal */
  triggerCount: number | null;
  /** 以下为段模型字段；旧配方 segmentCount = 0、expectedTotal = null、toleranceTotal = 0、calibrated = false */
  segmentCount: number;
  /** ΣN_exp；待标定为 null */
  expectedTotal: number | null;
  toleranceTotal: number;
  /** expectedTotal 非空 */
  calibrated: boolean;
  /** 只有配方库列表（recipe_list）给出 */
  stage?: RecipeStage;
}

/** 某个视图没有位置基准（帧徽标）：该视图的有胶站已是“有胶、无基准”（st 4），判定只看站状态。 */
export interface ReferenceFailure {
  view: number;
  /** 简短原因，如“定位失败”“两端搬移超限” */
  reason: string;
}

export interface FrameView {
  viewResults?: {view:number;verdict:Verdict;error:string|null;score:number|null;ms:number|null;pose?:PoseRecord|null}[];
  status: FrameStatus;
  cam: number;
  camera: string;
  shotId: string;
  view: number;
  session: number | null;
  ordinal: number | null;
  error: string | null;
  arrivedMs: number | null;
  frameCounter: number | null;
  triggerCounter: number | null;
  counterJump: boolean;
  score: number | null;
  points: number;
  gapPoints: number;
  ms: number | null;
  /** 没有位置基准的视图；空表示基准正常。与本件结论无关，NG / ERR 都照样显示徽标 */
  referenceFailed: ReferenceFailure[];
}

export interface CandidateDebug { recipeId: string; revision: number }

export interface PartView {
  candidateDebug?: CandidateDebug | null;
  cycleId: string;
  bundleId: string | null;
  sn: string;
  recipeId: string;
  /** 本件配方快照的修订 ID */
  recipeRevision: string;
  n: number;
  received: number;
  triggers: number;
  queue: number;
  filled: number;
  total: number;
  /** 各拍照点的帧 */
  frames: FrameView[];
  measuredFrames: number;
  /** 段配方在途件；旧配方为 null */
  burst: PartViewBurst | null;
}

export interface SegmentResult {
  verdict: Verdict;
  min: number | null;
  max: number | null;
  excursionLen: number;
  wMin: number | null;
  wMax: number | null;
  wExcursionLen: number;
  /** 位置基准无效：本段无基准的站连续超过 2 mm 或有胶站全无基准，位置判不了 */
  referenceInvalid: boolean;
}

/** 超过允许长度的连续缺胶：s0、s1 是所在拍照点中线上的段内弧长（mm）。 */
export interface GapRun {
  segment: number;
  s0: number;
  s1: number;
  len: number;
  frames: number[];
}

export interface Judgement {
  verdict: Verdict;
  plcCode: number;
  faultCode: number;
  reason: string;
  segments: SegmentResult[];
  gaps: GapRun[];
}

export interface ResultView extends Judgement {
  /** 段配方的判定；旧配方为 null。有值时 verdict / plcCode / faultCode / reason 与它相同，segments、gaps 为空 */
  burst: BurstJudgement | null;
  candidateDebug?: CandidateDebug | null;
  cycleId: string | null;
  sn: string;
  recipeId: string | null;
  ts: number;
  drainMs: number | null;
}

export interface Snapshot {
  statsScope?: "real" | "offline" | "debug";
  plcLocked?: boolean;
  candidateDebug?: CandidateDebug | null;
  measurementWorkers: { capacity: number; running: number; timedOut: number; availableCapacity: number };
  phase: Phase;
  since: number;
  fault: string | null;
  productSource: ProductSource;
  activeRecipeId: string | null;
  triggerMode: TriggerMode | null;
  part: PartView | null;
  result: ResultView | null;
  stats: { total: number; ok: number; ng: number; err: number };
  strayFrames: number;
  alarms: string[];
  recordingAlarm?: RecordingAlarm | null;
  /** 连续定位失败告警（当前站点模式的分槽）：同一配方修订、拍照点、图连续 3 件没有位置基准；定位成功后清除，重启清零 */
  locateAlarms?: LocateAlarm[];
}

export interface LocateAlarm {
  recipeId: string;
  revisionId: string;
  shotId: string;
  k: number;
  view: number;
  consecutive: number;
  /** 最近一件的原因（定位失败 / 两端搬移超限） */
  reason: string;
  cycleId: string;
  since: number;
}

export interface RecordingAlarm {
  consecutive: number;
  cycleId: string;
  reason: string;
  since: number;
}

export interface LogLine {
  ts: number;
  level: "info" | "warn" | "err" | "ok" | "ng";
  ev: string;
  msg: string;
  /** 这一行属于哪个工件；与工件无关的记录没有 */
  cycleId?: string;
}

export interface Measured {
  cycleId: string;
  shotId: string;
  camera: string;
  bundleId: string | null;
  sn: string;
  k: number;
  cam: number;
  /** 没有位置基准的视图（这些视图的有胶站 st 为 4） */
  referenceFailed: ReferenceFailure[];
  score: number;
  ms: number;
  queueMs: number | null;
  engineMs: number | null;
  coreMs: number | null;
  error: string | null;
  idx: number[];
  /** 胶条中线相对示教中线的横向偏移（mm）；只有 st 0 有值，其余为 null */
  d: (number | null)[];
  /** 胶宽（mm）；没测为 null */
  w: (number | null)[];
  /** 站状态，见 ST */
  st: number[];
  /** 图像测量时各点在原图里的像素位置 */
  px: [number, number][];
}

export interface Timeouts {
  armMs: number;
  motionMs: number;
  drainMs: number;
  procMs: number;
  ackMs: number;
}

export interface CycleSettings {
  productSource: ProductSource;
  manualRecipeId: string | null;
  timeouts: Timeouts;
  historyDays: number;
  record: RecordMode;
  recordKeep: number;
  recordMaxGb: number;
  offlineRecordKeep: number;
  offlineRecordMaxGb: number;
}

export type SiteMode = "real" | "offline";
export type SiteSourceKind = "sampleSet" | "synthetic";
export type OfflineSource = { kind: "sampleSet"; dir: string } | { kind: "synthetic" };

export interface OfflineChoice {
  kind: SiteSourceKind;
  sampleSetDir: string | null;
}

export interface SiteStatus {
  mode: SiteMode;
  source: SiteSourceKind | null;
  last: OfflineChoice | null;
  measurement: "lyFlow" | "simulated";
  port: number | null;
  running: boolean;
  continuous: boolean;
  parts: number;
  message: string;
  heartbeatStopped: boolean;
  sampleSet: SampleSetSummary | null;
  nextPart: string | null;
  recent: SampleRun[];
}

export type SampleLabel = "ok" | "ng" | "unknown";

export interface SamplePart {
  partId: string;
  sn: string;
  label: SampleLabel;
}

export interface SampleSetSummary {
  managedDir?: string | null;
  id: string;
  name: string;
  source: string;
  dir: string;
  shotCount: number;
  /** 清单里的相机；views 是旧拍照点模型的视角，段配方不使用。后端摘要不带每台相机的帧数 */
  cameras: { id: string; views: number[] }[];
  mmPerPx: number | null;
  parts: SamplePart[];
  missing: string[];
  missingCount: number;
}

export interface SampleRun {
  ts: number;
  partSn: string;
  partId: string;
  sampleSn: string;
  label: SampleLabel;
  purpose: "production" | "capture" | "candidateDebug";
  cycleId: string | null;
  resultCode: number | null;
  faultCode: number | null;
  verdict: Verdict | null;
  message: string | null;
}

/** 测站状态（与后端 measure.rs、记录库 BLOB 同一张码表；3 只出现在记录库里，表示未测）。 */
export const ST = { ok: 0, gap: 1, invalid: 2, pending: 3, noRef: 4, outOfRange: 5 } as const;

/** 测量点显示状态：noRef 有胶但没有位置基准（不判位置）；out 超出检测范围（不参与判定）。 */
export type PointVis = "none" | "ok" | "exc" | "ng" | "gap" | "inv" | "miss" | "noRef" | "out";

export type ReadinessKey = "plc" | "camera" | "recipe" | "engine" | "audit" | "site";
export type ReadinessState = "ok" | "waiting" | "blocked";
/** 处理该项要去的页面 */
export type ReadinessTarget = "plc" | "camera" | "workspace" | "settings" | "offline";

export interface ReadinessItem {
  key: ReadinessKey;
  title: string;
  state: ReadinessState;
  detail: string;
  /** 操作前提（如软件复位需要满足的条件） */
  hint: string | null;
  target: ReadinessTarget | null;
}

/** 配方能否在当前站点（相机组、样本集、测量方式）开工 */
export interface RecipeOption {
  id: string;
  name: string;
  version: number;
  productCode: number;
  /** 旧拍照点配方的拍照点数；段配方恒为 0，段数与 ΣN_exp 见 RecipeSummary */
  shotCount: number;
  published: boolean;
  runnable: boolean;
  reason: string | null;
  target: ReadinessTarget | null;
  /** 可运行但仍需等待的原因（如生产预热中） */
  waiting: string | null;
}

export interface Readiness {
  candidateDebug?: CandidateDebug | null;
  ready: boolean;
  items: ReadinessItem[];
  recipes: RecipeOption[];
}
