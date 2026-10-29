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
  /** 为空不按模式筛选 */
  modes?: PartMode[];
  offset?: number;
  limit?: number;
}

/** 从别的页面跳到历史页时带的初始筛选（如在线检测页“复核试判”） */
export interface HistoryFilter {
  recipeId?: string;
  modes?: PartMode[];
  groups?: string[];
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
  /** 段配方记录的整件标记（疑似提前终止等单独显示）；没有帧记录的为 null。
   * framesExpected = expectedTotal（待标定为 0），framesReceived = 帧组数 */
  flags: Flags | null;
  /** 维护采集件 verdict 为 ERR、未判定；试判件为真实判定。两者 plcCode 都是 0 */
  mode?: PartMode;
}

export type PartMode = "production" | "maintenanceCollect" | "maintenanceShadow";

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
  /** 后端保留的旧字段，恒为 [] */
  frames: FrameView[];
  triggers: number;
  softwareVersion: string;
  /** 后端保留的旧字段，恒为 null */
  points: null;
  retests: number[];
  /** 后端保留的旧字段，恒为 [] */
  shots: unknown[];
  recording: RecordingEvidence;
  /** 段配方记录；没有帧记录的记录（如工件身份分配失败）为 null */
  burst: PartDetailBurst | null;
}

export interface KindOverride {
  tolUpper?: number;
  tolLower?: number;
  maxExcursionLen?: number;
}

/** 重判的试算参数：只用于这次重判，不改配方。 */
export interface Overrides {
  /** 允许断胶长度（作用于每帧） */
  maxGapLen?: number;
  /** 直线段的胶宽允差（上 / 下公差、帧内允许超差长度） */
  straight: KindOverride;
  /** 弯道段的胶宽允差，字段同 straight；未分段区域取直线、弯道中较宽的一档 */
  corner: KindOverride;
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
  /** flags：重判后的整件标记（检测不完整等与阈值无关，照常带出） */
  changes: { id: number; sn: string; ts: number; from: Verdict; to: Verdict; reason: string; flags: Flags | null }[];
  skipReasons: { id: number; sn: string; ts: number; reason: string }[];
}
