
import type { PartMode } from "../history/types";
import type { BurstJudgement, CameraInspect, KindDefaults, PartCalib, PartViewBurst, PassSkip, PathSegment, RecipeDocSegmentFields } from "../plan/types";

/** 段模型（分段连拍飞拍）的契约类型，定义在 plan/types.ts；这里转出供 cycle 相关页面使用。 */
export type {
  BurstJudgement, CameraInspect, Flags, FrameCellStatus, FrameCellView, FrameGroupView, FrameMetrics, FrameState,
  KindDefault, KindDefaults, LossAlarm, PartCalib, PartViewBurst, PathSegment, PlanExport, Region, RegionKind, RegionResult,
  RecipeDocSegmentFields, RecipeSummaryV2, SegmentChecks, SegmentKind, Uninspected,
} from "../plan/types";

export type Phase = "IDLE" | "VALIDATE" | "ACQUIRE" | "DRAIN" | "JUDGE" | "REPORT" | "RELEASE" | "FAULT";
export type FrameStatus = "waiting" | "measuring" | "done" | "error" | "missing";
export type Verdict = "OK" | "OK_WITH_EXCURSION" | "NG_POSITION" | "NG_WIDTH" | "NG_ABSOLUTE" | "NG_GAP" | "ERR_INSPECT";
export type TriggerMode = "fly" | "stop";
export type ProductSource = "plc" | "manual";
export type RecordMode = "off" | "failed" | "all";

export interface JudgeParams {
  nominal: number;
  tolUpper: number;
  tolLower: number;
  absMin: number;
  absMax: number;
  maxExcursionLen: number;
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

/** 配方级判定限值（后端 recipe::ShotLimits）。 */
export interface ShotLimits {
  position: JudgeParams | null;
  width: JudgeParams | null;
  maxGapLen: number;
  /** 有胶站的最低占比（0–1），防零散断胶 */
  minPresent: number;
  /** 量具确认件编号：位置名义值不为 0 时必填 */
  gaugeRef?: string;
}

/** 编译后的配方（后端 recipe::Recipe）：WorkspaceView.layout、配方快照、Comparison.candidateRecipe。 */
export interface Recipe {
  id: string;
  name: string;
  version: number;
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
  /** 段列表（即 RecipeDoc.segments，编译后改名） */
  pathSegments: PathSegment[];
  /** 段配方的检测相机，顺序即计划相机顺序 */
  inspect: CameraInspect[];
  segmentDefaults: KindDefaults;
  boundaryGuard: number;
  passSkip?: PassSkip;
  passFrames?: number[];
  partCalib: PartCalib | null;
}

/** 配方文件内容，配方页编辑它。
 * 段模型字段（segments、inspect、segmentDefaults、boundaryGuard、partCalib）后端总是输出；前端提交时可缺省（后端按默认值补齐）。 */
export interface RecipeDoc extends Partial<RecipeDocSegmentFields> {
  id: string;
  name: string;
  version: number;
  teachingId?: string | null;
  /** 示教件来源（RB-7）：发布时从候选写入；候选里恒为 null，旧流程发布的配方没有 */
  teachingPiece?: TeachingPieceSource | null;
  productCode: number;
  /** 缺省为正式版；发布时由后端写入 */
  stage?: RecipeStage;
  triggerMode: TriggerMode;
  /** 配方文件格式版本（后端 recipe::RECIPE_SCHEMA）；不符的文件后端拒绝 */
  schemaVersion: number;
  /** 站距（mm） */
  spacing: number;
  filterWindow: number;
  detect: DetectParams;
  limits: ShotLimits;
}

/** 配方的示教件来源：历史件的 cycleId、记录号、SN 与时间。 */
export interface TeachingPieceSource { cycleId: string; recordId: number; sn: string; ts: number }

/** 配方阶段：collect = 采集版（只供维护模式采图，不判定），formal = 正式版；后端缺省即正式版。 */
export type RecipeStage = "formal" | "collect";

export interface RecipeSummary {
  id: string;
  name: string;
  version: number;
  revisionId: string;
  productCode: number;
  triggerMode: TriggerMode;
  cameras: string[];
  length: number;
  /** PLC 触发次数 ΣN_exp；未标定为 null */
  triggerCount: number | null;
  segmentCount: number;
  /** ΣN_exp；待标定为 null */
  expectedTotal: number | null;
  /** expectedTotal 非空 */
  calibrated: boolean;
  /** 只有配方库列表（recipe_list）给出 */
  stage?: RecipeStage;
  /** 生产配方的示教件来源（RB-7）；旧流程发布的配方没有 */
  teachingPiece?: TeachingPieceSource | null;
}

export interface FrameView {
  viewResults?: { view: number; verdict: Verdict; error: string | null; score: number | null; ms: number | null }[];
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
}


export interface PartView {
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
  measuredFrames: number;
  burst: PartViewBurst;
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
  /** 段配方的判定；没有配方时为 null。有值时 verdict / plcCode / faultCode / reason 与它相同 */
  burst: BurstJudgement | null;
  cycleId: string | null;
  sn: string;
  recipeId: string | null;
  ts: number;
  drainMs: number | null;
  mode?: PartMode;
}

export interface Snapshot {
  statsScope?: "real";
  plcLocked?: boolean;
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
  maintenance?: Maintenance | null;
  /** 维护采集进度，按配方；只含采集版配方录下的件 */
  collection?: CollectionProgress[];
  /** 维护试判统计，按配方；只看每个配方最新正式版本的试判件 */
  trial?: TrialProgress[];
}

/** 维护模式（在线投运）：开启后每件都照常采图，向 PLC 回结果码 0（按未检放行），不输出 NG。 */
export interface Maintenance {
  since: number;
  reason: string;
}

/** usable：录制完整、各计划相机帧数一致且非零的件数 */
export interface CollectionProgress {
  recipeId: string;
  recorded: number;
  usable: number;
}

/** ng：NG 结论细分，多的在前 */
export interface TrialProgress {
  recipeId: string;
  recipeVersion: number;
  counts: { ok: number; excursion: number; ng: number; err: number };
  ng: { verdict: Verdict; count: number }[];
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

export interface Timeouts {
  motionMs: number;
  drainMs: number;
  procMs: number;
  ackMs: number;
  /** 看到 Line0 门控关闭后再等多久收尾（D-SB-54）；0 = 不提前，固定等满 drainMs */
  gateGuardMs: number;
}

export interface CycleSettings {
  productSource: ProductSource;
  manualRecipeId: string | null;
  timeouts: Timeouts;
  historyDays: number;
  record: RecordMode;
  /** 帧录制最多保留件数、总大小（GB）；null 为不限（默认），被示教件引用的录制不删 */
  recordKeep: number | null;
  recordMaxGb: number | null;
  /** 空闲时无损压缩历史件原图；本机设置，默认关闭 */
  idleCompress: boolean;
  /** 只由 cycle_set_maintenance 改写；保存设置时后端沿用当前值 */
  maintenance?: Maintenance | null;
}


export type ReadinessKey = "plc" | "camera" | "recipe" | "engine" | "audit";
export type ReadinessState = "ok" | "waiting" | "blocked";
/** 处理该项要去的页面 */
export type ReadinessTarget = "plc" | "camera" | "workspace" | "settings";

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
  published: boolean;
  runnable: boolean;
  reason: string | null;
  target: ReadinessTarget | null;
  /** 可运行但仍需等待的原因（如生产预热中） */
  waiting: string | null;
  /** 可运行但部分段不测的说明（发布包有段预热没测成） */
  note: string | null;
}

export interface Readiness {
  ready: boolean;
  items: ReadinessItem[];
  recipes: RecipeOption[];
}
