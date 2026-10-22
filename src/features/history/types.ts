import type { FrameView, Judgement, SampleLabel, Verdict } from "../cycle/types";
import type { Flags, PartDetailBurst } from "../plan/types";

/** 段模型（分段连拍飞拍）的历史记录类型，定义在 plan/types.ts；这里转出供历史页面使用。 */
export type { BurstJudgement, Flags, FrameRecord, PartDetailBurst, PartSummaryV2, RegionResult, Uninspected } from "../plan/types";

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
  rawFiles: { view: number; file: string }[];
}

/** 记录来源：真机件与离线件分开统计。 */
export type RecordSource = "real" | "offlineSynthetic" | "offlineSample" | "offlineCandidate";
export type HistoryScope = "real" | "offline" | "debug";

export interface HistoryQuery {
  scope?: HistoryScope;
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
  sn: string;
  recipeId: string | null;
  recipeVersion: number | null;
  recipeRevision: string | null;
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
  bundleId: string | null;
  delivery: PlcDelivery;
  source: RecordSource;
  sample: { setId: string; partId: string; sn: string; label: SampleLabel } | null;
  /** 段配方记录的整件标记（C5 起输出，疑似提前终止等单独显示）；旧记录为 null 或缺省。
   * 段配方时 framesExpected = expectedTotal（待标定为 0），framesReceived = 帧组数 */
  flags?: Flags | null;
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
  /** 逐站：d 只有状态 0（有基准的有胶站）有值；st 码表见 cycle/types 的 ST（3 未测） */
  points: { d: (number | null)[]; w?: (number | null)[]; st: number[] } | null;
  retests: number[];
  shots: PartShot[];
  recording: RecordingEvidence;
  /** 段配方记录（C5 起输出）；旧记录为 null 或缺省。有值时 frames、shots 为空，points 为 null */
  burst?: PartDetailBurst | null;
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
  /** 允许断胶长度（作用于每个拍照点；段配方记录作用于每帧） */
  maxGapLen?: number;
  filterWindow?: number;
  /** 位置限值（只作用于判位置的拍照点） */
  position: KindOverride;
  /** 胶宽限值（只作用于判胶宽的拍照点） */
  width: KindOverride;
  /** 段配方记录：直线段的胶宽允差（上 / 下公差、帧内允许超差长度）；绝对限不适用。C5 起后端读取，旧记录忽略 */
  straight?: KindOverride;
  /** 段配方记录：弯道段的胶宽允差，字段同 straight；未分段区域取直线、弯道中较宽的一档 */
  corner?: KindOverride;
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
  changes: { id: number; sn: string; ts: number; from: Verdict; to: Verdict; reason: string }[];
  skipReasons: { id: number; sn: string; ts: number; reason: string }[];
}
