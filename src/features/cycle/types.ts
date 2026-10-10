
export type Phase = "IDLE" | "VALIDATE" | "ACQUIRE" | "DRAIN" | "JUDGE" | "REPORT" | "RELEASE" | "FAULT";
export type FrameStatus = "waiting" | "measuring" | "done" | "locateFailed" | "error" | "missing";
export type Verdict = "OK" | "OK_WITH_EXCURSION" | "NG_POSITION" | "NG_WIDTH" | "NG_ABSOLUTE" | "NG_GAP" | "ERR_INSPECT";
export type TriggerMode = "fly" | "stop";
export type ProductSource = "plc" | "manual";
export type Scenario = "normal" | "excursion" | "gap" | "lostFrame" | "locateFail" | "countMismatch" | "random";
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
}

/** 一个拍照点：机器人走到 Pose 时 PLC 触发这台相机拍一帧，在这帧里沿示教中线量胶。可选字段不设时不发送（不发 null）。 */
export interface ShotSpec {
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
}

export interface Recipe {
  id: string;
  name: string;
  version: number;
  revisionId: string;
  teachingId?: string | null;
  productCode: number;
  triggerMode: TriggerMode;
  /** 配方文件格式版本，当前为 4 */
  schemaVersion: number;
  /** 站距（mm） */
  spacing: number;
  filterWindow: number;
  detect: DetectParams;
  limits: ShotLimits;
  shots: ShotSpec[];
  segments: Segment[];
  /** 各站：所在拍照点图像里的像素位置、所属段与拍照点 */
  points: { x: number[]; y: number[]; seg: number[]; k: number[] };
}

/** 配方文件内容，配方页编辑它。 */
export interface RecipeDoc {
  id: string;
  name: string;
  version: number;
  teachingId?: string | null;
  productCode: number;
  triggerMode: TriggerMode;
  /** 配方文件格式版本，当前为 4；不符的文件后端拒绝 */
  schemaVersion: number;
  /** 站距（mm） */
  spacing: number;
  filterWindow: number;
  /** 拍照点没单独设时用的检测参数与限值 */
  detect: DetectParams;
  limits: ShotLimits;
  shots: ShotSpec[];
}

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
}

export interface FrameView {
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
  sn: number;
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
}

export interface SegmentResult {
  verdict: Verdict;
  min: number | null;
  max: number | null;
  excursionLen: number;
  wMin: number | null;
  wMax: number | null;
  wExcursionLen: number;
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
  cycleId: string | null;
  sn: number;
  recipeId: string | null;
  ts: number;
  drainMs: number | null;
}

export interface Snapshot {
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
}

export interface LogLine {
  ts: number;
  level: "info" | "warn" | "err" | "ok" | "ng";
  ev: string;
  msg: string;
}

export interface Measured {
  cycleId: string;
  shotId: string;
  camera: string;
  bundleId: string | null;
  sn: number;
  k: number;
  cam: number;
  located: boolean;
  score: number;
  ms: number;
  queueMs: number | null;
  engineMs: number | null;
  coreMs: number | null;
  error: string | null;
  idx: number[];
  /** 胶条中线相对示教中线的横向偏移（mm） */
  d: number[];
  /** 胶宽（mm）；没测为 null */
  w: (number | null)[];
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
  lyflowCore: string | null;
  vision: boolean;
  record: RecordMode;
  recordKeep: number;
  recordMaxGb: number;
}

export interface SimStatus {
  running: boolean;
  continuous: boolean;
  parts: number;
  message: string;
}

/** 测量点显示状态 */
export type PointVis = "none" | "ok" | "exc" | "ng" | "gap" | "inv" | "miss";
