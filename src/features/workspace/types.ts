import type { DetectParams, Judgement, Measured, Recipe, RecipeDoc, Verdict } from "../cycle/types";

/** 一个拍照点的示教：在冻结原图上点出的中线（图像像素，从胶嘴一侧往外）、像素当量与可选的检测参数。
 * workspace_save_params 把它写进候选配方的这个拍照点；detect 不设时不发送（用配方的检测参数）。 */
export interface ShotTeach {
  path: [number, number][];
  mmPerPx: number;
  detect?: DetectParams;
}
export interface FrozenImage {
  id: string; source: string; capturedAt: number; size: [number, number];
  camera: string; view: number; cameraTag: unknown; calibTag: unknown; geometryTag: unknown;
  exposureUs: number | null; gainDb: number | null; historyId: number | null;
}
export interface Trial {
  imageId: string; paramsTag: unknown; geometryTag: unknown; engineTag: unknown; passed: boolean;
  score: number; coverage: number; elapsedMs: number; reason: string;
  measurement: unknown;
}
/** 示教帧：冻结原图、试测与“已保存”；中线、像素当量与检测参数在候选配方的拍照点里。 */
export interface Teaching {
  k: number; image: FrozenImage | null; views: FrozenImage[]; trial: Trial | null; saved: boolean; backup: Teaching | null;
}
export interface Overview { background: string | null; positions: [number, number][]; saved: boolean }
export interface Sample { historyId: number | null; sampleId?: string | null; expected: Verdict }
export interface BankSample { id:string; name:string; geometryTag: unknown; expected:Verdict; createdAt:number }
export interface Validation {
  revision: number; passed: boolean; checkedAt: number; environmentTag: unknown;
  checks: { name: string; passed: boolean; detail: string }[];
  samples: { historyId: number | null; sampleId:string | null; name:string; sn: number; expected: Verdict; actual: Verdict | null; passed: boolean; reason: string }[];
}
export interface Release {
  doc: RecipeDoc; bundleId: string; baseRevision: string | null; revision: number; frames: Teaching[]; overview: Overview; validation: Validation;
}
export interface Workspace {
  doc: RecipeDoc; baseRevision: string | null; revision: number; frames: Teaching[];
  overview: Overview; samples: Sample[]; validation: Validation | null; pending: Release | null;
  sampleBank: BankSample[];
  publishError: string | null; updatedAt: number;
}
/** coverage：要检的拍照点里已示教中线的比例（%）。 */
export interface WorkspaceView {
  workspace: Workspace; layout: Recipe; productionVersion: number | null;
  /** 生产配方的 PLC 计划版本 */
  productionPlanVersion: number | null;
  /** 候选的拍照计划（产品代码、拍照点顺序 / Pose / 相机）与生产不同：生效后 PLC 要同步新的计划版本 */
  planChanged: boolean;
  coverage: number;
}
export interface RawFrame { k: number; camera: string; view: number; file: string; ts: number; available: boolean; error: string | null; cam?: number | null; frameCounter?: number | null; triggerCounter?: number | null }
export interface RecordImages { historyId: number; frames: RawFrame[]; complete: boolean; message: string }
export interface Comparison {
  id: string; historyId: number; source: "rules" | "raw" | "original"; cycleId: string; bundleId: string | null; candidateId: string; candidateRevision: number;
  candidateRecipe: Recipe;
  originalVerdict: Verdict; judgement: Judgement; measurements: Measured[]; createdAt: number;
}
export interface GrayImage { url: string; width: number; height: number }
