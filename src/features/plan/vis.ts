/**
 * 段模型结果的显示映射：在线检测、历史、工作台共用。只把契约类型（plan/types.ts）映射成文字、色调和格子状态，
 * 不做判定——结论、标记、帧数状态都以后端给出的为准。不依赖旧拍照点模型的 cycle/vis.ts 与 cycle/pose.ts。
 */
import type { Verdict } from "../cycle/types";
import type {
  CountStatus, Flags, FrameGroupView, OffsetUnjudged, RegionKind, RegionResult, SegmentKind, UncheckedItem, Uninspected, UnsegReason,
} from "./types";

export type PlanTone = "info" | "ok" | "warn" | "ng" | "neutral";

/** 一个徽标：key 稳定（可作 React key 与测试定位），label 为界面文字。 */
export interface PlanBadge {
  key: string;
  label: string;
  tone: PlanTone;
}

/** 段配方的计划相机数（规格 §1：三台独立相机共用一个触发）。 */
export const PLAN_CAMERAS = 3;

export const segmentKindLabel: Record<SegmentKind, string> = { straight: "直线", corner: "弯道" };

/** 预期帧数与容差：“20 ± 1”；N_exp 为 null 时“待标定”。整件用 expectedTotal / toleranceTotal。 */
export function expectedText(expected: number | null | undefined, tolerance: number): string {
  return expected == null ? "待标定" : `${expected} ± ${tolerance}`;
}

export const countStatusText: Record<CountStatus, string> = { normal: "帧数正常", low: "帧数偏少", high: "帧数偏多" };

/** 帧数状态；null 表示无从比较（N_exp 待标定），不核对。 */
export function countStatusLabel(status: CountStatus | null | undefined): string {
  return status ? countStatusText[status] : "未核对帧数";
}

export const unsegReasonText: Record<UnsegReason, string> = { algorithmPending: "分段算法未就绪", lowConfidence: "分段不确定", transition: "边界过渡区" };

/** 区域名：“段 1 · 直线”“段 2 · 弯道”“段 1–3 · 未分段”“段 1–2 · 边界过渡”。 */
export function regionKindLabel(kind: RegionKind): string {
  switch (kind.type) {
    case "straight": return `段 ${kind.segment} · 直线`;
    case "corner": return `段 ${kind.segment} · 弯道`;
    case "skipped": return `段 ${kind.segment} · 跳过`;
    case "unsegmented": return `${kind.firstSegment === kind.lastSegment ? `段 ${kind.firstSegment}` : `段 ${kind.firstSegment}–${kind.lastSegment}`} · ${kind.reason === "transition" ? "边界过渡" : "未分段"}`;
  }
}

/** 区域覆盖的段号范围（含两端）。 */
export function regionSegments(kind: RegionKind): [number, number] {
  return kind.type === "unsegmented" ? [kind.firstSegment, kind.lastSegment] : [kind.segment, kind.segment];
}

export const offsetUnjudgedText: Record<OffsetUnjudged, string> = {
  unsegmented: "胶偏未判：未分段",
  lowConfidence: "胶偏未判：分段不确定",
  measureNotImplemented: "胶偏未判：测量量未实现",
  noReference: "胶偏未判：未标定参考值",
  noFrames: "胶偏未判：参考相机没有可用帧",
};

export const uncheckedText: Record<UncheckedItem, string> = { bubble: "气泡", stringing: "拉丝" };

/** 未检区域：“段 1 · 帧组 1–2（跳过）”；旧记录按毫米：“段 1 · 0–5 mm”，段长未知时“段 3 · 75 mm 之后”。 */
export function uninspectedText(u: Uninspected): string {
  if (u.fromSeq != null) return `段 ${u.segment} · 帧组 ${u.fromSeq === u.toSeq ? u.fromSeq : `${u.fromSeq}–${u.toSeq}`}（跳过）`;
  return `段 ${u.segment} · ${u.toMm == null ? `${u.fromMm} mm 之后` : `${u.fromMm}–${u.toMm} mm`}`;
}

/** 整件标记徽标（都只告警，不改 OK/NG），顺序固定：检测不完整、帧数异常、胶偏未判、含未分段、疑似提前终止。 */
export function flagBadges(flags: Flags | null | undefined): PlanBadge[] {
  if (!flags) return [];
  const all: [boolean, PlanBadge][] = [
    [flags.incomplete, { key: "incomplete", label: "检测不完整", tone: "warn" }],
    [flags.frameCountAnomaly, { key: "frameCountAnomaly", label: "帧数异常", tone: "warn" }],
    [flags.offsetUnjudged, { key: "offsetUnjudged", label: "胶偏未判", tone: "neutral" }],
    [flags.unsegmented, { key: "unsegmented", label: "含未分段", tone: "neutral" }],
    [flags.shortPart, { key: "shortPart", label: "疑似提前终止", tone: "warn" }],
  ];
  return all.filter(([on]) => on).map(([, badge]) => badge);
}

/** 一个区域的徽标：帧数越界、不完整组个数、胶偏未判原因、未分段原因。帧数正常与未核对不出徽标。 */
export function regionBadges(result: RegionResult): PlanBadge[] {
  const out: PlanBadge[] = [];
  if (result.countStatus === "low" || result.countStatus === "high")
    out.push({ key: "count", label: `${countStatusText[result.countStatus]}（${result.frameCount} / ${expectedText(result.expected, result.tolerance)}）`, tone: "warn" });
  if (result.incompleteGroups.length) out.push({ key: "incomplete", label: `不完整组 ${result.incompleteGroups.length}`, tone: "warn" });
  if (result.offsetUnjudged) out.push({ key: "offsetUnjudged", label: offsetUnjudgedText[result.offsetUnjudged], tone: "neutral" });
  if (result.region.kind.type === "unsegmented") out.push({ key: "unsegmented", label: unsegReasonText[result.region.kind.reason], tone: "neutral" });
  return out;
}

/** 结论的色调（与历史页 verdictTone 同一口径，ERR 归为 ng 色调以适配徽标）。 */
export function verdictPlanTone(verdict: Verdict): PlanTone {
  return verdict === "OK" ? "ok" : verdict === "OK_WITH_EXCURSION" ? "warn" : "ng";
}

/** 帧组格子（一台相机一帧）的显示状态：到达前 waiting，测量中 measuring，测完按该帧结论，其余为缺帧类状态（都只算检测不完整）。 */
export type GroupCellState = "waiting" | "measuring" | "ok" | "excursion" | "ng" | "error" | "missing" | "unmeasured" | "outOfCorridor";

export function groupCellState(group: FrameGroupView, cam: number): GroupCellState {
  const cell = group.cells[cam];
  if (!cell) return "missing";
  if (cell.status !== "measured") return cell.status;
  switch (cell.verdict) {
    case null: return "measuring";
    case "OK": return "ok";
    case "OK_WITH_EXCURSION": return "excursion";
    case "ERR_INSPECT": return "error";
    default: return "ng";
  }
}

export const groupCellText: Record<GroupCellState, string> = {
  waiting: "等待", measuring: "测量中", ok: "OK", excursion: "局部超差", ng: "NG", error: "测量失败",
  missing: "缺帧", unmeasured: "未测", outOfCorridor: "出走廊",
};

/** 格子颜色：缺帧、未测、出走廊为灰色（只算检测不完整，不是 NG）。 */
export const groupCellColor: Record<GroupCellState, string> = {
  waiting: "var(--surface-high)", measuring: "var(--primary)", ok: "var(--ok)", excursion: "var(--warn)", ng: "var(--ng)", error: "var(--fault)",
  missing: "var(--on-surface-disabled)", unmeasured: "var(--on-surface-disabled)", outOfCorridor: "var(--on-surface-disabled)",
};

/** 这一格算不算“检测不完整”（缺帧、测量失败、未测、出走廊）。 */
export const cellIncomplete = (state: GroupCellState) => state === "missing" || state === "error" || state === "unmeasured" || state === "outOfCorridor";
