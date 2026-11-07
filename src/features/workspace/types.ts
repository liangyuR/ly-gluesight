import type { Judgement, Recipe, RecipeDoc, RecipeStage, Verdict } from "../cycle/types";
import type { CameraImaging } from "../camera/types";
import type { BurstJudgement, CalibrationRound, CameraTeaching, FrameMetrics, FrameVerdict, InspectionCoverage, PathSegment, WorkspacePlanView } from "../plan/types";
import type { PartMode } from "../history/types";
import type { NavigationMapData } from "./navigation/types";
import type { FramePixels } from "../../lib/frame";

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
  CalibrationRound, CalibrationView, CameraTeachDraft, CameraTeaching, CameraTeachingView, CaptureFailureV2, CaptureRoundV2, CaptureState, MissingFrame,
  PieceView, ProgressStepV2, ProgressTargetV2, ProgressV2, RoundOrigin, SegmentCalibView, StepIdV2, WorkspacePlanView,
} from "../plan/types";

export interface FrozenImage {
  id: string; source: string; capturedAt: number; size: [number, number];
  camera: string; view: number; cameraTag: unknown; calibTag: unknown; geometryTag: unknown;
  exposureUs: number | null; gainDb: number | null; historyId: number | null;
}
export interface Sample { historyId: number | null; sampleId?: string | null; expected: Verdict }
export interface BankSample { id:string; name:string; geometryTag: unknown; expected:Verdict; createdAt:number; captureId?: string | null }
/** 发布前检查的一项；advisory 为只提醒、不拦发布的项（如有段在示教帧上没测成）。 */
export interface Check { name: string; passed: boolean; detail: string; advisory?: boolean }
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
  /** 示教件的测量；界面读 plan.calibration */
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

/** 配方回传包（候选与示教件轮次记录，不带原图）的导出结果、导入预览与导入结果 */
export interface PackageExported { path: string; bytes: number; files: number }
export interface PackagePreview {
  path: string; bytes: number; recipeId: string; name: string; productCode: number; sourceProjectName: string;
  exportedAt: number; appVersion: string; teachingPiece: TeachingPiece | null;
  productionVersion: number | null; candidate: { revision: number; updatedAt: number; pending: boolean } | null; blocker: string | null;
}
export interface PackageImported { id: string; notes: string[] }
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
  burst: { segments: PathSegment[]; cameras: string[]; expectedTotal: number | null; judgement: BurstJudgement;
    /** 这次对照用的每帧指标；之前保存的对照为空 */
    frames: FrameMetrics[];
    /** 建测量图用的各计划相机示教帧尺寸 [宽, 高]，叠图按它重建；原包重现与之前保存的对照为空 */
    sizes?: [number, number][];
    /** 逐帧结论（帧格颜色），后端读出时按这次对照的区域与逐帧指标算出 */
    frameVerdicts?: FrameVerdict[];
    /** 从原图复测时没有可用走廊、没测的段；结论只覆盖其余段 */
    unmeasured?: UnmeasuredSegment[] } | null;
}
/** 一种复测能不能做（后端 burst_history::CompareOption）：不能做时 reason 写明原因。 */
export interface CompareOption { available: boolean; reason: string | null;
  /** 能做但只覆盖一部分时的说明（如有段没有可用走廊、不测） */
  note?: string | null }
/** 从原图复测时没有可用走廊、没测的一段一台相机（后端 burst_history::UnmeasuredSegment）。 */
export interface UnmeasuredSegment { segment: number; camera: string; reason: string }
/** 可选来复测的一个本机候选。 */
export interface CompareCandidate { id: string; revision: number; version: number; name: string }
/** 一条历史记录能做哪几种复测（后端 burst_history::CompareOptions）：任意本机候选都能复测任意记录（不看型号、配方编号与项目）。 */
export interface CompareOptions {
  /** 选来复测的候选：指定的，或缺省为记录所属配方的候选；本机没有时为 null */
  candidate: CompareCandidate | null;
  original: CompareOption; rules: CompareOption; raw: CompareOption;
  /** 本机全部可选的候选 */
  choices: CompareCandidate[];
}
/** 分段保存的影响（后端 capture_review::SegmentSaveImpact，保存前确认用）。 */
export interface SegmentSaveImpact {
  restructured: boolean;
  /** 会清掉的走廊：“原第 j 段（类型）· 相机” */
  dropped: string[];
  /** 保存后还没有走廊的（段, 检测相机） */
  untaught: string[];
  measurementCleared: boolean;
  coverage: InspectionCoverage | null;
}
export type { FramePixels };
/** width / height 为原图尺寸；channels 为 1（灰度）或 3（RGB）；带 pixels 时直接画到 canvas，url 只作为这张图的标识。 */
export interface FrameImage { url: string; width: number; height: number; channels?: 1 | 3; pixels?: FramePixels }

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
  /** 记录所属的配方与项目（试测“全部历史件”里可能是别的配方、别的项目） */
  recipeId?: string | null;
  projectId?: string;
}
