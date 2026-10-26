import type { CameraImaging, Judgement, Recipe, RecipeDoc, RecipeStage, Verdict } from "../cycle/types";
import type { BurstJudgement, CalibrationRound, CameraTeaching, CaptureState, PathSegment, WorkspacePlanView } from "../plan/types";

/** 段模型（分段连拍飞拍）的候选工作区类型，定义在 plan/types.ts；这里转出供工作台页面使用。 */
export type {
  CalibOutcome, CalibrationRound, CalibrationRoundView, CalibrationView, CameraTeachDraft, CameraTeaching, CameraTeachingView, CaptureFailureV2, CaptureRoundV2, CaptureState, MissingFrame,
  ProgressStepV2, ProgressTargetV2, ProgressV2, SegmentCalibView, StepIdV2, WorkspacePlanView,
} from "../plan/types";

export interface FrozenImage {
  id: string; source: string; capturedAt: number; size: [number, number];
  camera: string; view: number; cameraTag: unknown; calibTag: unknown; geometryTag: unknown;
  exposureUs: number | null; gainDb: number | null; historyId: number | null;
}
export interface Sample { historyId: number | null; sampleId?: string | null; expected: Verdict }
export interface BankSample { id:string; name:string; geometryTag: unknown; expected:Verdict; createdAt:number; captureId?: string | null }
export interface Check { name: string; passed: boolean; detail: string }
export interface Validation {
  revision: number; passed: boolean; checkedAt: number; environmentTag: unknown;
  checks: Check[];
  samples: { historyId: number | null; sampleId:string | null; name:string; sn: string; expected: Verdict; actual: Verdict | null; passed: boolean; reason: string }[];
}
/** 发布时所在站点：离线验证发布的版本只能在离线运行。 */
export type ValidationSite = "real" | "offline";
export interface Release {
  doc: RecipeDoc; bundleId: string; baseRevision: string | null; revision: number; validation: Validation;
  validationSite: ValidationSite;
}
export interface Workspace {
  doc: RecipeDoc; baseRevision: string | null; revision: number;
  samples: Sample[]; validation: Validation | null; pending: Release | null;
  sampleBank: BankSample[];
  captureId?: string | null;
  unboundImaging?: Record<string, CameraImaging>;
  publish: PublishState; updatedAt: number;
  /** 每台计划相机的走廊示教帧；界面读 plan.cameraTeaching */
  cameraTeaching: CameraTeaching[];
  /** 多轮标定加入的轮次；界面读 plan.calibration */
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
export type StepStatus = "notStarted" | "inProgress" | "done" | "blocked";
export interface WorkspaceView {
  workspace: Workspace; layout: Recipe; productionVersion: number | null;
  /** 生产配方是采集版还是正式版；未发布为 null */
  productionStage?: RecipeStage | null;
  /** 生产配方的计划版本 */
  productionPlanVersion: number | null;
  /** 候选的计划身份与生产不同（或还没发布过）：发布时分配新的计划版本 */
  planChanged: boolean;
  plan: WorkspacePlanView;
}
export interface Comparison {
  id: string; historyId: number; source: "rules" | "raw" | "original"; cycleId: string; bundleId: string | null; candidateId: string; candidateRevision: number;
  candidateRecipe: Recipe;
  originalVerdict: Verdict; judgement: Judgement; measurements: unknown[]; createdAt: number;
  /** 复测结果（后端 workspace::burst_history::ComparisonBurst）：重判所用的候选段、相机、预期总数与区域判定。
   * 之前保存的旧对照为 null；有值时 judgement 的 verdict / plcCode / faultCode / reason 与 burst.judgement 相同 */
  burst: { segments: PathSegment[]; cameras: string[]; expectedTotal: number | null; judgement: BurstJudgement } | null;
}
/** 预览像素（8 位灰度，可能是降采样后的尺寸）。 */
export interface GrayPixels { data: Uint8Array; width: number; height: number }
/** width / height 为原图尺寸；带 pixels 时直接画到 canvas，url 只作为这张图的标识。 */
export interface GrayImage { url: string; width: number; height: number; pixels?: GrayPixels }
export interface ImagingSaved { view: WorkspaceView; applyErrors: Record<string, string> }

export interface CaptureCamera { cameraId: string; deviceSession: number; receivedCount: number; imaging?: CameraImaging }
export interface CaptureSummary { roundId: string; recipeId: string; cameraIds: string[]; plannedCount: number | null; receivedCount: number; state: CaptureState; createdAt: number; error: string | null }

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
