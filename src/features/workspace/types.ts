import type { Judgement, Measured, Recipe, RecipeDoc, Verdict } from "../cycle/types";

export interface FrameParams {
  rect: [number, number, number, number];
  dx: number; dy: number; deg: number; mmPerPx: number;
  searchMm: number; minContrast: number; minScore: number;
}
export interface FrozenImage {
  id: string; source: string; capturedAt: number; size: [number, number];
  camera: string; cameraTag: string; calibTag: string; geometryTag: string;
  exposureUs: number | null; gainDb: number | null; historyId: number | null;
}
export interface Trial {
  imageId: string; paramsTag: string; geometryTag: string; passed: boolean;
  score: number; coverage: number; elapsedMs: number; reason: string;
  measurement: { ids: number[]; status: string[]; innerCenter: (number | null)[]; width?: (number | null)[] } | null;
}
export interface Teaching {
  k: number; image: FrozenImage | null; params: FrameParams; trial: Trial | null; saved: boolean; backup: Teaching | null;
}
export interface Overview { background: string | null; positions: [number, number][]; saved: boolean }
export interface Sample { historyId: number | null; sampleId?: string | null; expected: Verdict }
export interface BankSample { id:string; name:string; geometryTag:string; expected:Verdict; createdAt:number }
export interface Validation {
  revision: number; passed: boolean; checkedAt: number; environmentTag: string;
  checks: { name: string; passed: boolean; detail: string }[];
  samples: { historyId: number | null; sampleId:string | null; name:string; sn: number; expected: Verdict; actual: Verdict | null; passed: boolean; reason: string }[];
}
export interface Release {
  doc: RecipeDoc; baseHash: string | null; revision: number; frames: Teaching[]; overview: Overview; validation: Validation;
}
export interface Workspace {
  doc: RecipeDoc; baseHash: string | null; revision: number; frames: Teaching[];
  overview: Overview; samples: Sample[]; validation: Validation | null; pending: Release | null;
  sampleBank: BankSample[];
  publishError: string | null; updatedAt: number;
}
export interface WorkspaceView { workspace: Workspace; layout: Recipe; productionVersion: number | null; coverage: number }
export interface RawFrame { k: number; camera: string; file: string; ts: number; available: boolean; cam?: number | null; frameCounter?: number | null; triggerCounter?: number | null }
export interface RecordImages { historyId: number; frames: RawFrame[]; complete: boolean; message: string }
export interface Comparison {
  id: string; historyId: number; source: "rules" | "raw"; candidateId: string; candidateRevision: number;
  candidateRecipe: Recipe;
  originalVerdict: Verdict; judgement: Judgement; measurements: Measured[]; createdAt: number;
}
export interface GrayImage { url: string; width: number; height: number }
