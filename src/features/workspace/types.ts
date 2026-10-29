import type { Judgement, Recipe, RecipeDoc, RecipeStage, Verdict } from "../cycle/types";
import type { CameraImaging } from "../camera/types";
import type { BurstJudgement, CalibrationRound, CameraTeaching, PathSegment, WorkspacePlanView } from "../plan/types";
import type { PartMode } from "../history/types";
import type { NavigationMapData } from "./navigation/types";

export interface NavigationArchiveStatus {
  state: "none" | "ready" | "unconfirmed" | "frozen" | "error";
  bundleId: string | null;
  detail: string;
}
export interface NavigationView {
  map: NavigationMapData | null;
  error: string | null;
  imageAvailable: boolean;
  structureRevision: number;
  published: NavigationArchiveStatus | null;
}

/** 段模型（分段连拍飞拍）的候选工作区类型，定义在 plan/types.ts；这里转出供工作台页面使用。 */
export type {
  CalibOutcome, CalibrationRound, CalibrationRoundView, CalibrationView, CameraTeachDraft, CameraTeaching, CameraTeachingView, CaptureFailureV2, CaptureRoundV2, CaptureState, MissingFrame,
  PieceView, ProgressStepV2, ProgressTargetV2, ProgressV2, RoundOrigin, SegmentCalibView, StepIdV2, WorkspacePlanView,
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
  navigation?: NavigationArchiveStatus | null;
}
export interface Workspace {
  doc: RecipeDoc; baseRevision: string | null; revision: number;
  samples: Sample[]; validation: Validation | null; pending: Release | null;
  sampleBank: BankSample[];
  captureId?: string | null;
  publish: PublishState; updatedAt: number;
  /** 每台计划相机的走廊示教帧；界面读 plan.cameraTeaching */
  cameraTeaching: CameraTeaching[];
  /** 多轮标定加入的轮次；界面读 plan.calibration */
  calibration: { rounds: CalibrationRound[] };
  captureReview?: CaptureReview;
  /** 候选的建法：2 = 示教件流程（RB-7）；其他为旧流程候选，已停用，只能删除（RB-5） */
  flow: number;
  /** 示教件（RB-1）：导入的轮次与来源历史件 */
  teachingPiece: TeachingPiece | null;
  navigationId?: string | null;
  navigationStructureRevision?: number;
}
export interface TeachingPiece { roundId: string; recordId: number; cycleId: string; sn: string; ts: number }
/** 新流程候选（RB-5）：flow 不是 2 的候选只能在配方库删除。 */
export const isCurrentFlow = (workspace: Pick<Workspace, "flow">) => workspace.flow === 2;
export interface CaptureRoundReview {
  groupCount: number;
  cuts: number[];
  status: "draft" | "confirmed" | "stale";
  excludedGroups: { seq: number; reason: string }[];
  segmentSignature: unknown;
  captureSignature: unknown;
  templateVersion: number | null;
  /** 最近一次确认的来源（D-SB-51）：人工，或对齐可信时自动确认；缺省为人工 */
  confirmedBy?: "manual" | "aligned";
}
/** 边界需重点复核的原因（D-SB-51）：被挪动、偏离比例换算超过 boundaryGuard、对齐不清晰；null 为可信边界 */
export type AlignReviewReason = "moved" | "deviates" | "unclear";
/** 分段模板边界对齐到一轮的建议（workspace_capture_review_suggest）：cut 为上一段最后一个帧组（从 1 起）。 */
export interface AlignSuggestion {
  round: string;
  templateRound: string;
  templateCuts: number[];
  cuts: { cut: number; proportional: number; sharpness: number; reason: AlignReviewReason | null; review: boolean }[];
}
/** 选标定件、同步标定件的结果：每件是否已加入标定，没加入的带原因（待复核或不可用） */
export interface CalibrationSyncResult {
  view: WorkspaceView;
  pieces: { roundId: string; added: boolean; issue: string | null }[];
}
export interface CaptureReview {
  rounds: Record<string, CaptureRoundReview>;
}
/** 发布与生效状态（后端显式枚举）。 */
export type PublishState =
  | { state: "draft" } | { state: "waiting" }
  | { state: "failed"; message: string } | { state: "uncertain"; message: string }
  | { state: "active"; version: number; revision: number; warning: string | null; validationSite: ValidationSite };
export type StepStatus = "notStarted" | "inProgress" | "done" | "blocked";
export interface WorkspaceView {
  navigation?: NavigationView;
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

export interface CaptureCamera { cameraId: string; deviceSession: number; receivedCount: number; imaging?: CameraImaging }

/** 历史件选择器的一行：reasons 为空、usage 为 null 即可选用 */
export interface HistoryCandidate {
  recordId: number;
  cycleId: string | null;
  ts: number;
  sn: string;
  mode: PartMode;
  groupCount: number;
  /** 按录制的计划相机顺序 */
  cameras: { cameraId: string; frames: number }[];
  /** 原图完整：无缺帧、无 rawDropped、每帧已落盘 */
  complete: boolean;
  reasons: string[];
  /** 不阻断导入的提示 */
  warnings: string[];
  durationMs: number | null;
  measuredFps: number | null;
  setFps: number | null;
  importedRound: string | null;
  /** 本候选已把它选为示教件或标定件：不能重复选 */
  usage: "teaching" | "calibration" | null;
  /** 手动配置数据（capture_fixture 写的仿历史件）的标签；真机件为 null */
  fixture: string | null;
}

export interface HistoryPick { recordId: number; standard: boolean }
