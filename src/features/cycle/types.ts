
import type { PartMode } from "../history/types";
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
/** 离线工况，与后端 offline::scenario::Scenario（camelCase）同名。 */
export type Scenario = "normal" | "excursion" | "gap" | "lostFrame" | "motionTimeout" | "ackMismatch" | "random"
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
  /** 各相机的曝光与增益 */
  cameras: Record<string, CameraImaging>;
  /** 段列表（即 RecipeDoc.segments，编译后改名） */
  pathSegments: PathSegment[];
  /** 段配方的检测相机，顺序即计划相机顺序 */
  inspect: CameraInspect[];
  segmentDefaults: KindDefaults;
  calibrationRounds: number;
  boundaryGuard: number;
  boundaryRefine?: boolean;
  partCalib: PartCalib | null;
}

/** 配方文件内容，配方页编辑它。
 * 段模型字段（segments、inspect、segmentDefaults、calibrationRounds、boundaryGuard、boundaryRefine、partCalib）后端总是输出；前端提交时可缺省（后端按默认值补齐）。 */
export interface RecipeDoc extends Partial<RecipeDocSegmentFields> {
  id: string;
  name: string;
  version: number;
  /** PLC 拍照计划版本：配方库分配，只在计划身份（产品代码、各段序号 / 类型 / N_exp、相机顺序与 ΣN_exp）变化时换新号；0 或缺省为未分配 */
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
  detect: DetectParams;
  limits: ShotLimits;
  /** 各相机的曝光与增益：检测相机各一组 */
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
  triggerMode: TriggerMode;
  cameras: string[];
  length: number;
  /** PLC 计划版本 */
  planVersion: number;
  /** PLC 触发次数 ΣN_exp；未标定为 null */
  triggerCount: number | null;
  segmentCount: number;
  /** ΣN_exp；待标定为 null */
  expectedTotal: number | null;
  toleranceTotal: number;
  /** expectedTotal 非空 */
  calibrated: boolean;
  /** 只有配方库列表（recipe_list）给出 */
  stage?: RecipeStage;
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
  /** 段配方的判定；没有配方（拒收）时为 null。有值时 verdict / plcCode / faultCode / reason 与它相同 */
  burst: BurstJudgement | null;
  candidateDebug?: CandidateDebug | null;
  cycleId: string | null;
  sn: string;
  recipeId: string | null;
  ts: number;
  drainMs: number | null;
  mode?: PartMode;
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
  maintenance?: Maintenance | null;
  /** 维护采集进度，按配方；只含采集版配方录下的件 */
  collection?: CollectionProgress[];
  /** 维护试判统计，按配方；只看每个配方最新正式版本的试判件 */
  trial?: TrialProgress[];
}

/** 维护模式（在线投运）：开启后每件都布防采图，向 PLC 回结果码 0（按未检放行），不输出 NG。 */
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
  /** 只由 cycle_set_maintenance 改写；保存设置时后端沿用当前值 */
  maintenance?: Maintenance | null;
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
  /** 清单里的相机；views 段配方不使用。后端摘要不带每台相机的帧数 */
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
