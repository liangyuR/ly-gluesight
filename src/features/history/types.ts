import type { FrameView, Judgement, Verdict } from "../cycle/types";

export interface PlcDelivery {
  state: "notRequired" | "pending" | "submitted" | "acknowledged" | "failed";
  updatedAt: number;
  message: string | null;
}

export interface RecordingEvidence {
  state: "pending" | "off" | "notRetained" | "complete" | "incomplete" | "failed";
  available: boolean;
  directory: string | null;
  errors: string[];
}

export interface PartShot {
  k: number;
  shotId: string;
  camera: string;
  view: number;
  session: number | null;
  ordinal: number | null;
  frameCounter: number | null;
  triggerCounter: number | null;
  status: FrameView["status"];
  error: string | null;
  score: number | null;
  ms: number | null;
  rawFiles: { view: number; file: string; hash: string | null }[];
}

export interface HistoryQuery {
  from?: number | null;
  to?: number | null;
  verdicts?: Verdict[];
  sn?: string;
  recipeId?: string | null;
  offset?: number;
  limit?: number;
}

export interface PartSummary {
  id: number;
  ts: number;
  sn: number;
  recipeId: string | null;
  recipeVersion: number | null;
  recipeHash: string | null;
  triggerMode: string | null;
  verdict: Verdict;
  plcCode: number;
  faultCode: number;
  reason: string;
  drainMs: number | null;
  framesExpected: number;
  framesReceived: number;
  retestOf: number | null;
  cycleId: string | null;
  bundleHash: string | null;
  delivery: PlcDelivery;
}

export interface VerdictCounts {
  ok: number;
  excursion: number;
  ng: number;
  err: number;
}

export interface HistoryPage {
  total: number;
  counts: VerdictCounts;
  items: PartSummary[];
}

export interface PartDetail {
  summary: PartSummary;
  judgement: Judgement;
  frames: FrameView[];
  triggers: number;
  softwareVersion: string;
  points: { d: number[]; w?: (number | null)[]; st: number[] } | null;
  retests: number[];
  shots: PartShot[];
  recording: RecordingEvidence;
}

export interface KindOverride {
  tolUpper?: number;
  tolLower?: number;
  absMin?: number;
  absMax?: number;
  maxExcursionLen?: number;
}

/** 重判的试算参数：只用于这次重判，不改配方。 */
export interface Overrides {
  /** 允许断胶长度（作用于每个拍照点） */
  maxGapLen?: number;
  filterWindow?: number;
  /** 位置限值（只作用于判位置的拍照点） */
  position: KindOverride;
  /** 胶宽限值（只作用于判胶宽的拍照点） */
  width: KindOverride;
}

export interface RejudgeRequest {
  query: HistoryQuery;
  ids: number[];
  useCurrentRecipe: boolean;
  overrides: Overrides;
}

export interface RejudgeResult {
  total: number;
  skipped: number;
  limitHit: boolean;
  matrix: { from: Verdict; to: Verdict; count: number }[];
  changes: { id: number; sn: number; ts: number; from: Verdict; to: Verdict; reason: string }[];
  skipReasons: { id: number; sn: number; ts: number; reason: string }[];
}
