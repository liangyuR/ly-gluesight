import type { CameraImaging, DetectParams, Judgement, Measured, Recipe, RecipeDoc, ShotLimits, Verdict } from "../cycle/types";

/** 一个拍照点的示教：在冻结原图上点出的中线（图像像素，从胶嘴一侧往外）、像素当量与可选的检测参数。
 * workspace_save_params 把它写进候选配方的这个拍照点；detect 不设时不发送（用配方的检测参数）。 */
export interface ShotTeach {
  path: [number, number][];
  mmPerPx: number | null;
  detect?: DetectParams;
  limits?: ShotLimits;
}
export interface FrozenImage {
  id: string; source: string; capturedAt: number; size: [number, number];
  camera: string; view: number; cameraTag: unknown; calibTag: unknown; geometryTag: unknown;
  exposureUs: number | null; gainDb: number | null; historyId: number | null;
}
export interface Trial {
  imageId: string; paramsTag: unknown; geometryTag: unknown; engineTag: unknown; passed: boolean;
  score: number; coverage: number; elapsedMs: number; reason: string;
  measurement: unknown; verdict?: Verdict;
}
export interface CalibrationCheck {
  basis: { kind: "manual"; referenceMm: number; measuredMm: number; errorMm: number } | { kind: "sampleSet"; setId: string };
  passed: boolean; mmPerPx: number | null; verificationId?: string;
}
/** 示教帧：冻结原图、试测与“已保存”；中线、像素当量与检测参数在候选配方的拍照点里。 */
export interface Teaching {
  viewStates?: { view: number; trial: Trial | null; saved: boolean; calibrationCheck?: CalibrationCheck | null }[];
  k: number; image: FrozenImage | null; views: FrozenImage[]; trial: Trial | null; saved: boolean; backup: Teaching | null;
}
export interface Overview { background: string | null; positions: [number, number][]; saved: boolean }
export interface Sample { historyId: number | null; sampleId?: string | null; expected: Verdict }
export interface BankSample { id:string; name:string; geometryTag: unknown; expected:Verdict; createdAt:number }
export interface Validation {
  revision: number; passed: boolean; checkedAt: number; environmentTag: unknown;
  checks: { name: string; passed: boolean; detail: string }[];
  samples: { historyId: number | null; sampleId:string | null; name:string; sn: string; expected: Verdict; actual: Verdict | null; passed: boolean; reason: string }[];
}
/** 发布时所在站点：离线验证发布的版本只能在离线运行。 */
export type ValidationSite = "real" | "offline";
export interface Release {
  doc: RecipeDoc; bundleId: string; baseRevision: string | null; revision: number; frames: Teaching[]; overview: Overview; validation: Validation;
  validationSite: ValidationSite;
}
export interface Workspace {
  doc: RecipeDoc; baseRevision: string | null; revision: number; frames: Teaching[];
  overview: Overview; samples: Sample[]; validation: Validation | null; pending: Release | null;
  sampleBank: BankSample[];
  lastPosition?: { k: number; view: number } | null; captureId?: string | null;
  unboundImaging?: Record<string, CameraImaging>;
  publish: PublishState; updatedAt: number;
}
/** 发布与生效状态（后端显式枚举）。 */
export type PublishState =
  | { state: "draft" } | { state: "waiting" }
  | { state: "failed"; message: string } | { state: "uncertain"; message: string }
  | { state: "active"; version: number; revision: number; warning: string | null; validationSite: ValidationSite };
export type StepId = "capture" | "defaults" | "teach" | "calibration" | "overview" | "validation" | "publish";
export type StepStatus = "notStarted" | "inProgress" | "done" | "blocked" | "optional";
export type ProgressTarget = { page: "capture" } | { page: "teach"; k: number } | { page: "validation" } | { page: "camera" };
export interface ProgressStep {
  id: StepId; label: string; optional: boolean; status: StepStatus; detail: string;
  blockers: { message: string; target: ProgressTarget }[];
}
export type ShotState = "recapture" | "unconfigured" | "draft" | "pendingTrial" | "trialFailed" | "trialPassed" | "taught" | "skipped";
export type CalibrationState =
  | { state: "unverified" } | { state: "stale" }
  | { state: "verified"; errorMm: number } | { state: "failed"; errorMm: number }
  | { state: "sampleSet"; setId: string; mmPerPx: number };
export interface ViewProgress { view: number; enabled: boolean; state: ShotState; trialStale: boolean; calibration: CalibrationState; hint: string }
export interface ShotProgress { k: number; shotId: string; state: ShotState; completed: number; total: number; views: ViewProgress[] }
/** 后端计算的候选进度：步骤、逐拍照点状态与下一步。 */
export interface Progress {
  steps: ProgressStep[]; shots: ShotProgress[];
  next: { step: StepId; label: string; target: ProgressTarget } | null;
}
/** coverage：要检的拍照点里已示教中线的比例（%）。 */
export interface WorkspaceView {
  workspace: Workspace; layout: Recipe; productionVersion: number | null;
  /** 生产配方的 PLC 计划版本 */
  productionPlanVersion: number | null;
  /** 候选的拍照计划（产品代码、拍照点顺序 / Pose / 相机）与生产不同：生效后 PLC 要同步新的计划版本 */
  planChanged: boolean;
  coverage: number;
  progress: Progress;
}
export interface RawFrame { k: number; camera: string; view: number; file: string; ts: number; available: boolean; error: string | null; cam?: number | null; frameCounter?: number | null; triggerCounter?: number | null }
export interface RecordImages { imageSource: "offlineSample" | "recording"; historyId: number; frames: RawFrame[]; complete: boolean; message: string }
export interface Comparison {
  id: string; historyId: number; source: "rules" | "raw" | "original"; cycleId: string; bundleId: string | null; candidateId: string; candidateRevision: number;
  candidateRecipe: Recipe;
  originalVerdict: Verdict; judgement: Judgement; measurements: Measured[]; createdAt: number;
}
/** 预览像素（8 位灰度，可能是降采样后的尺寸）。 */
export interface GrayPixels { data: Uint8Array; width: number; height: number }
/** width / height 为原图尺寸；带 pixels 时直接画到 canvas，url 只作为这张图的标识。 */
export interface GrayImage { url: string; width: number; height: number; pixels?: GrayPixels }
export interface ImagingSaved { view: WorkspaceView; applyErrors: Record<string, string> }

export interface CaptureCamera { cameraId: string; deviceSession: number; receivedCount: number; imaging?: CameraImaging }
/** 一台相机在一次触发里交付的帧 */
export interface CaptureCameraFrame { cameraId: string; frameCounter: number; triggerCounter: number | null; originalPath: string; views: { view: number; path: string; width: number; height: number }[] }
export interface CaptureRound {
  roundId: string; recipeId: string; cameras: CaptureCamera[]; plannedCount: number | null;
  state: "waitingStart" | "receiving" | "draining" | "complete" | "failed";
  plcPlannedCount?: number | null; plcActualCount?: number | null;
  createdAt: number; endedAt: number | null; error: string | null; simulated: boolean;
  /** 每项是一次 PLC 触发，cameras 按本轮相机顺序 */
  frames: { shotId: string; ordinal: number; cameras: CaptureCameraFrame[] }[];
}
export interface CaptureSummary { roundId: string; recipeId: string; cameraIds: string[]; plannedCount: number | null; receivedCount: number; state: CaptureRound["state"]; createdAt: number; error: string | null }
