import type { CameraImaging, DetectParams, FixtureSpec, Judgement, Measured, PoseRecord, Recipe, RecipeDoc, ShotLimits, Verdict } from "../cycle/types";
import type { BurstJudgement, CalibrationRound, CameraTeaching, PathSegment, WorkspacePlanView } from "../plan/types";

/** 段模型（分段连拍飞拍）的候选工作区类型，定义在 plan/types.ts；这里转出供工作台页面使用。
 * 段配方的进度用 StepIdV2 / ProgressV2（在 WorkspaceView.plan.progress），旧配方的 StepId / Progress 不变。 */
export type {
  CalibOutcome, CalibrationRound, CalibrationRoundView, CalibrationView, CameraTeachDraft, CameraTeaching, CameraTeachingView, CaptureFailureV2, CaptureRoundV2, MissingFrame,
  ProgressStepV2, ProgressTargetV2, ProgressV2, SegmentCalibView, StepIdV2, WorkspacePlanView,
} from "../plan/types";

/** 一个拍照点的示教：在冻结原图上点出的中线（图像像素，从胶嘴一侧往外）、像素当量与可选的检测参数。
 * workspace_save_params 把它写进候选配方的这个拍照点；detect 不设时不发送（用配方的检测参数）。
 * fixture 必须发送（没有特征定位时为 null）：后端漏收会报错，不会悄悄清掉已保存的特征定位。 */
export interface ShotTeach {
  path: [number, number][];
  mmPerPx: number | null;
  detect?: DetectParams;
  limits?: ShotLimits;
  fixture: FixtureSpec | null;
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
  /** 特征定位视图的示教图检查报告；没有时（G4 之前的试测）需要重新试测 */
  fixture?: FixtureTeachReport | null;
}
/** 特征定位检查项编号（后端 vision::fixture_quality::CheckId）；说明文字由后端给出 */
export type FixtureCheckId = "inside" | "corridor" | "texture" | "bright" | "lever" | "selfLocate"
  | "locateAll" | "sigma" | "brightnessLock" | "observable" | "searchRange" | "nominal" | "brightnessReject" | "rotation" | "endParams" | "fewSamples";
export interface FixtureCheckItem { id: FixtureCheckId; level: "block" | "warn"; passed: boolean; detail: string }
export interface FixtureLever { from: [number, number]; to: [number, number]; px: number; mm: number | null }
/** 画框时的只读检查（workspace_fixture_check） */
export interface FixtureCheck {
  passed: boolean; checks: FixtureCheckItem[]; padded: [number, number, number, number] | null;
  search: [number, number, number, number]; lever: FixtureLever | null; textureStd: number | null; brightFraction: number | null;
}
/** 试测时的示教图检查报告：几何、纹理加示教图上的定位验收 */
export interface FixtureTeachReport {
  passed: boolean; checks: FixtureCheckItem[]; pose: PoseRecord | null; lever: FixtureLever | null; textureStd: number | null; brightFraction: number | null;
}
export interface FixtureEndSuggestion { p99Mm: number; maxEndShiftMm: number; endMarginMm: number; applicable: boolean; note: string }
export interface FixtureRotationEvidence {
  enabled: boolean; leverMm: number; leverThetaP95Mm: number | null; thresholdMm: number; exceeds: boolean;
  frames: number; observableFrames: number; allObservable: boolean; autoConditions: boolean;
}
/** 一个特征定位视图在独立正常样本上的验证统计 */
export interface FixtureViewReport {
  k: number; view: number; shotId: string; passed: boolean; samples: number; checks: FixtureCheckItem[];
  sigmaMm: number | null; meanMm: number | null; nominalMm: number; toleranceMm: number; searchUse: number | null;
  suggestion: FixtureEndSuggestion | null; rotation: FixtureRotationEvidence;
  rows: { name: string; pose: PoseRecord | null; brighter: PoseRecord | null; darker: PoseRecord | null; error: string | null }[];
}
/** 定位试测（workspace_fixture_trial）：只作诊断，不落盘 */
export interface FixtureTrialRow {
  label: string; source: "teach" | "sample"; sampleId: string | null; pose: PoseRecord | null; reason: string | null;
  stations: { id: number; status: "ok" | "no_ref" | "out_of_range" | "no_bead"; d: number | null }[]; error: string | null;
}
export interface FixtureTrial { revision: number; k: number; view: number; rows: FixtureTrialRow[] }
/** 开转角的独立证据（定稿 §10 决定二）：只记进应用日志 */
export interface RotationConfirmation { basis: "twoTemplates" | "lowerDeviation" | "physical"; note: string }
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
export interface BankSample { id:string; name:string; geometryTag: unknown; expected:Verdict; createdAt:number; captureId?: string | null }
export interface Validation {
  revision: number; passed: boolean; checkedAt: number; environmentTag: unknown;
  checks: { name: string; passed: boolean; detail: string }[];
  samples: { historyId: number | null; sampleId:string | null; name:string; sn: string; expected: Verdict; actual: Verdict | null; passed: boolean; reason: string }[];
  /** 每个特征定位视图的统计；G4 之前的验证没有 */
  fixture?: FixtureViewReport[];
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
  /** 段配方：每台计划相机的走廊示教帧（旧拍照点配方为空，示教在 frames）；界面读 plan.cameraTeaching */
  cameraTeaching: CameraTeaching[];
  /** 段配方：多轮标定加入的轮次（旧拍照点配方为空）；界面读 plan.calibration */
  calibration: { rounds: CalibrationRound[] };
  captureReview?: CaptureReview;
}
export interface CaptureRoundReview {
  groupCount: number;
  cuts: number[];
  status: "draft" | "confirmed" | "stale";
  excludedGroups: { seq: number; reason: string }[];
  segmentSignature: unknown;
  captureSignature: unknown;
  templateVersion: number | null;
}
export interface CaptureReview {
  rounds: Record<string, CaptureRoundReview>;
  template: {
    roundId: string; groupCount: number; cuts: number[];
    segmentSignature: unknown; captureSignature: unknown; version: number;
  } | null;
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
/** PLC 开始请求要填的参数；shotCount 即触发次数，deviceShots 同值。 */
export interface PlcParams { productCode: number; planVersion: number; shotCount: number; shots: { shotId: string; poseId: string }[] }
/** coverage：要检的拍照点里已示教中线的比例（%）；没有要检的拍照点时为 null。 */
export interface WorkspaceView {
  workspace: Workspace; layout: Recipe; productionVersion: number | null;
  /** 生产配方的 PLC 计划版本 */
  productionPlanVersion: number | null;
  /** 候选的拍照计划（产品代码、拍照点顺序 / Pose / 相机）与生产不同：生效后 PLC 要同步新的计划版本 */
  planChanged: boolean;
  /** 生产配方的 PLC 对接参数；未发布时为 null */
  productionPlc: PlcParams | null;
  /** 候选发布后的 PLC 对接参数；编不出时原因在 candidatePlcError */
  candidatePlc: PlcParams | null;
  candidatePlcError: string | null;
  coverage: number | null;
  progress: Progress;
  /** 段配方的计划部分：后端只对 doc.segments 非空的候选输出，旧配方为 null；页面按它分流新旧页面（决策记录第八节）。
   * 段配方时 productionPlc / candidatePlc / candidatePlcError / coverage 为 null，progress 为空，进度在 plan.progress */
  plan: WorkspacePlanView | null;
}
export interface RawFrame { k: number; camera: string; view: number; file: string; ts: number; available: boolean; error: string | null; cam?: number | null; frameCounter?: number | null; triggerCounter?: number | null }
export interface RecordImages { imageSource: "offlineSample" | "recording"; historyId: number; frames: RawFrame[]; complete: boolean; message: string }
export interface Comparison {
  id: string; historyId: number; source: "rules" | "raw" | "original"; cycleId: string; bundleId: string | null; candidateId: string; candidateRevision: number;
  candidateRecipe: Recipe;
  originalVerdict: Verdict; judgement: Judgement; measurements: Measured[]; createdAt: number;
  /** 段配方的复测结果（后端 workspace::burst_history::ComparisonBurst）：重判所用的候选段、相机、预期总数与区域判定。
   * 旧拍照点记录（含之前保存的对照）为 null；有值时 judgement 的 verdict / plcCode / faultCode / reason 与 burst.judgement 相同，segments、gaps 为空 */
  burst: { segments: PathSegment[]; cameras: string[]; expectedTotal: number | null; judgement: BurstJudgement } | null;
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

export interface TeachingRegisters {
  version: number;
  state: number;
  roundId: number;
  reason: number;
  reserved: number;
}

export interface TeachingViolation {
  field: keyof TeachingRegisters;
  detail: string;
}

export interface CapturePlcStatus {
  available: boolean;
  busy: boolean;
  roundId: number | null;
  phase: "unavailable" | "resetRequired" | "idle" | "requested" | "accepted" | "draining" | "reporting" | "cancelling" | "releasing" | "fault";
  message: string;
  monitor?: { pc: TeachingRegisters | null; plc: TeachingRegisters | null; fresh: boolean; violation?: TeachingViolation | null };
}
