import { ST, type FrameStatus, type Judgement, type Measured, type PoseRecord, type Recipe, type Verdict } from "../cycle/types";
import { referenceBadge } from "../cycle/vis";
import { rawPoseTransform, teachCenter, type Rect } from "../cycle/pose";
import type { BurstJudgement, PathSegment, RegionResult } from "../plan/types";
import { regionKindLabel, regionSegments } from "../plan/vis";
import type { PartDetail, PartDetailBurst } from "./types";

/** 重跑与原测量的逐点偏移、胶宽差超过此值（mm）记为差异。同一发布包、同一原图应逐位一致，留 0.01 mm 只为吸收序列化舍入。 */
export const REPLAY_TOLERANCE_MM = 0.01;

/** noRef：有胶、无位置基准；out：超出检测范围。与原结果状态不同即算该站变化。 */
type PointKind = "ok" | "gap" | "inv" | "noRef" | "out";
const kindOf = (st: number | undefined): PointKind => st === ST.ok ? "ok" : st === ST.gap ? "gap" : st === ST.noRef ? "noRef" : st === ST.outOfRange ? "out" : "inv";

export interface ShotSide {
  /** 该拍照点各视角分段的结论，按配方分段顺序 */
  verdicts: { view: number; verdict: Verdict | null }[];
  ok: number;
  gap: number;
  inv: number;
  noRef: number;
  out: number;
}

export interface ShotDiff {
  k: number;
  shotId: string;
  camera: string;
  skip: boolean;
  /** 重跑有该拍照点的测量（规则重判没有） */
  remeasured: boolean;
  original: ShotSide & { status: FrameStatus | null; error: string | null };
  replay: ShotSide & { error: string | null };
  /** 状态改变的测量点（配方测点下标） */
  changed: number[];
  maxDd: number | null;
  maxDw: number | null;
  differs: boolean;
}

type Replay = { candidateRecipe: Recipe; judgement: Judgement; measurements: Measured[] };
type Original = Pick<PartDetail, "points" | "judgement" | "shots">;

/** 存储的测量点能否按候选规则重判：与后端一致，只有测不了、未测的站挡住；无基准、超出检测范围的站照常重判。 */
export const rulesRejudgeable = (points: PartDetail["points"]) =>
  !!points?.st.length && points.st.every(st => st !== ST.invalid && st !== ST.pending);

const sameList = <T>(a: T[], b: T[]) => a.length === b.length && a.every((v, i) => v === b[i]);

function sameLayout(a: Recipe, b: Recipe) {
  return a.shots.length === b.shots.length && a.shots.every((shot, k) => shot.id === b.shots[k].id && shot.camera === b.shots[k].camera)
    && a.segments.length === b.segments.length
    && a.segments.every((g, i) => { const h = b.segments[i]; return g.shot === h.shot && g.view === h.view && g.first === h.first && g.count === h.count; })
    && sameList(a.points.k, b.points.k) && sameList(a.points.seg, b.points.seg) && sameList(a.points.x, b.points.x) && sameList(a.points.y, b.points.y);
}

const count = (kinds: PointKind[]) => {
  const n = (kind: PointKind) => kinds.filter(x => x === kind).length;
  return { ok: n("ok"), gap: n("gap"), inv: n("inv"), noRef: n("noRef"), out: n("out") };
};
const segmentVerdicts = (layout: Recipe, judgement: Judgement, k: number) =>
  layout.segments.flatMap((g, i) => g.shot === k ? [{ view: g.view, verdict: judgement.segments[i]?.verdict ?? null }] : []);

/** 逐拍照点比较原结果与重跑结果；测点布局不同（候选改了拍照点或测点）时无法逐点对应，返回 null。 */
export function shotDiffs(layout: Recipe, original: Original, replay: Replay): ShotDiff[] | null {
  if (!sameLayout(layout, replay.candidateRecipe)) return null;
  return layout.shots.map((shot, k) => {
    const owned = layout.points.k.flatMap((owner, j) => owner === k ? [j] : []);
    const before = owned.map(j => kindOf(original.points?.st[j]));
    const record = original.shots.find(s => s.k === k);
    const m = replay.measurements.find(x => x.k === k);
    const at = new Map((m?.idx ?? []).map((j, i) => [j, i]));
    const failed = !!m && !!m.error;
    const after = owned.map(j => m && !failed && at.has(j) ? kindOf(m.st[at.get(j)!]) : "inv" as PointKind);
    const changed: number[] = [];
    let maxDd: number | null = null, maxDw: number | null = null;
    if (m && !failed) owned.forEach((j, n) => {
      if (before[n] !== after[n]) { changed.push(j); return; }
      const i = at.get(j);
      if (before[n] === "gap" || before[n] === "inv" || i === undefined || !original.points) return;
      // 位置只有有基准的站才有；胶宽在无基准、超出检测范围的站上也照样判，一并比
      const d0 = original.points.d[j], d1 = m.d[i];
      if (before[n] === "ok" && d0 != null && d1 != null) maxDd = Math.max(maxDd ?? 0, Math.abs(d0 - d1));
      const w0 = original.points.w?.[j], w1 = m.w[i];
      if (w0 != null && w1 != null && Number.isFinite(w0) && Number.isFinite(w1)) maxDw = Math.max(maxDw ?? 0, Math.abs(w0 - w1));
    });
    if (m && failed) owned.forEach((j, n) => { if (before[n] !== "inv") changed.push(j); });
    const verdicts = { before: segmentVerdicts(layout, original.judgement, k), after: segmentVerdicts(replay.candidateRecipe, replay.judgement, k) };
    const verdictChanged = !sameList(verdicts.before.map(v => v.verdict), verdicts.after.map(v => v.verdict));
    const differs = verdictChanged || (!!m && (changed.length > 0 || (maxDd ?? 0) > REPLAY_TOLERANCE_MM || (maxDw ?? 0) > REPLAY_TOLERANCE_MM));
    return {
      k, shotId: shot.id, camera: shot.camera, skip: shot.skip, remeasured: !!m,
      original: { verdicts: verdicts.before, ...count(before), status: record?.status ?? null, error: record?.error ?? null },
      replay: { verdicts: verdicts.after, ...(m ? count(after) : { ok: 0, gap: 0, inv: 0, noRef: 0, out: 0 }), error: m?.error ?? referenceBadge(m?.referenceFailed) },
      changed, maxDd, maxDw, differs,
    };
  });
}

type Edge = [[number, number], [number, number]] | null;
type ViewReading = { view: number; reading: { idx: number[]; st: number[]; w: (number | null)[]; edges: Edge[]; pose?: PoseRecord | null } | null; error: string | null };

/** 重跑结果里选中拍照点、视角的胶边线（原图像素坐标），用于在原图上叠加胶宽测量。 */
export function replayEdges(replay: Replay | null, k: number, view: number, changed: number[] = []) {
  const m = replay?.measurements.find(x => x.k === k) as (Measured & { views?: ViewReading[] }) | undefined;
  const reading = m?.views?.find(v => v.view === view)?.reading;
  if (!reading) return [];
  const step = Math.max(1, Math.ceil(reading.idx.length / 4));
  return reading.idx.flatMap((j, i) => {
    const edge = reading.edges[i];
    if (!edge) return [];
    const w = reading.w[i];
    return [{ from: edge[0], to: edge[1], color: changed.includes(j) ? "var(--warn)" : "var(--ok)", label: i % step === 0 && w != null ? w.toFixed(2) + " mm" : undefined }];
  });
}

/** 重跑结果里选中拍照点、视角的位姿（复测或重现时 lyFlow 重新定位得到的）。 */
export function replayPose(replay: Replay | null, k: number, view: number): PoseRecord | null {
  const m = replay?.measurements.find(x => x.k === k) as (Measured & { views?: ViewReading[] }) | undefined;
  return m?.views?.find(v => v.view === view)?.reading?.pose ?? null;
}

/** 复测位姿与原记录相差超过这么多才在原图上另画复测定位框（同一张原图、同一模板应逐位一致，阈值只吸收序列化舍入）。 */
export const REPLAY_POSE_PX = .5;
export const REPLAY_POSE_DEG = .05;

/**
 * 复测位姿与原记录位姿是否不同：采用与否不同，或两次的变换在图像上不同（转角差 > 0.05°，或原示教框中心被搬到的位置相差 > 0.5 px）。
 * offset 是相对各自示教框中心的（center − teachCenter），从原图复测用的工作台候选可能改过框，两次的 offset 不能直接相减，
 * 所以各按自己的框还原 T 再在同一点上比；转角是 T 的线性部分，与转心无关，可以直接比。任一侧没有位姿时不比较。
 */
export function poseDiffers(original: PoseRecord | null, originalRect: Rect | null | undefined, replay: PoseRecord | null, replayRect: Rect | null | undefined) {
  if (!original || !replay) return false;
  if (original.accepted !== replay.accepted) return true;
  if (!original.accepted) return false;
  if (Math.abs((original.angle ?? 0) - (replay.angle ?? 0)) > REPLAY_POSE_DEG) return true;
  const rect = originalRect ?? replayRect;
  if (!rect) return false;
  const [Ta, Tb] = [rawPoseTransform(rect, original), rawPoseTransform(replayRect ?? rect, replay)];
  if (!Ta || !Tb) return !Ta !== !Tb;
  const p = teachCenter(rect), [a, b] = [Ta(p), Tb(p)];
  return Math.hypot(a[0] - b[0], a[1] - b[1]) > REPLAY_POSE_PX;
}

// ───────────── 段配方（分段连拍飞拍）：按区域比对 ─────────────
// 旧拍照点模型的 shotDiffs / sameLayout 保持原样；段配方的复测与重判按区域比，计划身份变了就不比。

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
