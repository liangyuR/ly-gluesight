import type { BurstJudgement, PathSegment, RegionResult } from "../plan/types";
import { regionKindLabel, regionSegments } from "../plan/vis";
import type { PartDetailBurst } from "./types";

/** 复测与原结果的胶宽、胶偏差超过此值（mm）记为差异。同一发布包、同一原图应逐位一致，留 0.01 mm 只为吸收序列化舍入。 */
export const REPLAY_TOLERANCE_MM = 0.01;

const sameList = <T>(a: T[], b: T[]) => a.length === b.length && a.every((v, i) => v === b[i]);

// 复测与重判按区域比，计划身份变了就不比。

/** 复测对照里的一侧：该次判定所依据的段、相机、整件预期总数与区域判定（原记录取 PartDetailBurst，复测取 Comparison.burst）。 */
export interface BurstSide { segments: PathSegment[]; cameras: string[]; expectedTotal: number | null; judgement: BurstJudgement }

export const burstSideOf = (burst: PartDetailBurst): BurstSide => ({ segments: burst.segments, cameras: burst.cameras, expectedTotal: burst.expectedTotal, judgement: burst.judgement });

/**
 * 两侧的计划身份是否相同（与后端 plan_identity 同口径）：各段的序号、类型、N_exp，有序相机，整件预期总数。
 * 限值、容差、x_ref、检测区间、段长不参与——它们变了计划号不变，区域仍一一对应，可以比较。
 */
export function samePlanIdentity(a: BurstSide, b: BurstSide) {
  return a.expectedTotal === b.expectedTotal && sameList(a.cameras, b.cameras) && a.segments.length === b.segments.length
    && a.segments.every((s, i) => { const t = b.segments[i]; return s.index === t.index && s.kind === t.kind && s.expectedFrames === t.expectedFrames; });
}

export type RegionChange = "verdict" | "frameCount" | "incomplete" | "width" | "offset" | "offsetUnjudged";
export const regionChangeText: Record<RegionChange, string> = { verdict: "结论", frameCount: "帧数", incomplete: "不完整组", width: "胶宽", offset: "胶偏", offsetUnjudged: "胶偏未判原因" };

export interface RegionDiff {
  /** 区域类型与覆盖的段号，两侧同一区域的键相同 */
  key: string;
  label: string;
  /** 一侧没有这个区域时为 null（区域划分不同） */
  original: RegionResult | null;
  replay: RegionResult | null;
  changes: RegionChange[];
  differs: boolean;
}

const regionKey = (r: RegionResult) => { const [first, last] = regionSegments(r.region.kind); return `${r.region.kind.type}:${first}-${last}`; };
const near = (a: number | null | undefined, b: number | null | undefined) => a == null || b == null ? a == b : Math.abs(a - b) <= REPLAY_TOLERANCE_MM;

function regionChanges(a: RegionResult, b: RegionResult): RegionChange[] {
  const changes: RegionChange[] = [];
  if (a.verdict !== b.verdict) changes.push("verdict");
  if (a.frameCount !== b.frameCount || a.countStatus !== b.countStatus) changes.push("frameCount");
  if (!sameList(a.incompleteGroups, b.incompleteGroups)) changes.push("incomplete");
  if (!!a.width !== !!b.width || (a.width && b.width && !(near(a.width.min, b.width.min) && near(a.width.max, b.width.max) && near(a.width.mean, b.width.mean)))) changes.push("width");
  if (!!a.offset !== !!b.offset || (a.offset && b.offset && !(near(a.offset.meanShift, b.offset.meanShift) && near(a.offset.maxDev, b.offset.maxDev)))) changes.push("offset");
  if (a.offsetUnjudged !== b.offsetUnjudged) changes.push("offsetUnjudged");
  return changes;
}

/** 逐区域比较原结果与重判 / 复测结果；计划身份不同（段序列、类型、N_exp、相机或预期总数变了）时区域无法对应，返回 null，只比较整件结论。 */
export function regionDiffs(original: BurstSide, replay: BurstSide): RegionDiff[] | null {
  if (!samePlanIdentity(original, replay)) return null;
  const after = new Map(replay.judgement.regions.map(r => [regionKey(r), r]));
  const seen = new Set<string>();
  const diffs: RegionDiff[] = original.judgement.regions.map(r => {
    const key = regionKey(r), other = after.get(key) ?? null;
    seen.add(key);
    const changes = other ? regionChanges(r, other) : [];
    return { key, label: regionKindLabel(r.region.kind), original: r, replay: other, changes, differs: !other || changes.length > 0 };
  });
  for (const r of replay.judgement.regions) if (!seen.has(regionKey(r))) diffs.push({ key: regionKey(r), label: regionKindLabel(r.region.kind), original: null, replay: r, changes: [], differs: true });
  return diffs;
}
