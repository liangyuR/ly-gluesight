
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

export interface Segment {
  name: string;
  kind: "line" | "corner";
  s0: number;
  s1: number;
  params: JudgeParams;
  width: JudgeParams | null;
}

export type PathSpec =
  | { kind: "roundedRect"; width: number; height: number; radius: number }
  /** bulges[i] 不为 0 时第 i 条边是圆弧：tan(圆心角/4)，正值逆时针 */
  | { kind: "polyline"; points: [number, number][]; closed: boolean; radius: number; bulges?: number[] };

/** 一个拍照点：机器人走到 Pose 时 PLC 触发这台相机拍一帧。 */
export interface ShotSpec {
  /** 配方内唯一，如 P1 */
  id: string;
  /** 现场机器人 / PLC 程序里的 Pose 标识；不要求唯一 */
  poseId: string;
  /** 相机编号 */
  camera: string;
  /** 视野中心在工件坐标里的位置（mm） */
  center: [number, number];
  /** 视野宽高；不给时用配方的 fov */
  fov?: [number, number];
  /** 标定引用；不给时用这台相机的工位标定 */
  calib?: string;
}

export interface Recipe {
  id: string;
  name: string;
  version: number;
  hash: string;
  teachingHash?: string | null;
  productCode: number;
  triggerMode: TriggerMode;
  /** 配方文件格式版本，当前为 2 */
  schemaVersion: number;
  part: [number, number, number];
  path: PathSpec | null;
  closed: boolean;
  /** 拍照点没单独给视野时用的视野宽高 */
  fov: [number, number];
  shots: ShotSpec[];
  spacing: number;
  filterWindow: number;
  maxGapLen: number;
  segments: Segment[];
  points: { x: number[]; y: number[]; seg: number[]; k: number[] };
}

export interface SegmentLimits {
  position: JudgeParams;
  width: JudgeParams | null;
}

/** 配方文件内容，配方页编辑它。 */
export interface RecipeDoc {
  id: string;
  name: string;
  version: number;
  teachingHash?: string | null;
  productCode: number;
  triggerMode: TriggerMode;
  /** 配方文件格式版本，当前为 2；不符的文件后端拒绝 */
  schemaVersion: number;
  path: PathSpec;
  spacing: number;
  filterWindow: number;
  maxGapLen: number;
  line: SegmentLimits;
  corner: SegmentLimits;
  segmentOverrides: Record<string, SegmentLimits>;
  fov: [number, number];
  shots: ShotSpec[];
}

export interface RecipeSummary {
  id: string;
  name: string;
  version: number;
  hash: string;
  productCode: number;
  shotCount: number;
  triggerMode: TriggerMode;
  cameras: string[];
  length: number;
}

export interface FrameView {
  status: FrameStatus;
  cam: number;
  /** 相机编号（旧记录里没有） */
  camera?: string;
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
  sn: number;
  recipeId: string;
  /** 本件配方快照的哈希 */
  recipeHash: string;
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
  sn: number;
  recipeId: string | null;
  ts: number;
  drainMs: number | null;
}

export interface Snapshot {
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
  sn: number;
  k: number;
  cam: number;
  located: boolean;
  score: number;
  ms: number;
  error: string | null;
  idx: number[];
  d: number[];
  /** 胶宽；没测为 null */
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

/** 从 CSV / DXF 导入的胶路 */
export interface ImportedPath {
  points: [number, number][];
  bulges: number[];
  closed: boolean;
  note: string | null;
}

/** 测量点显示状态 */
export type PointVis = "none" | "ok" | "exc" | "ng" | "gap" | "inv" | "miss";
