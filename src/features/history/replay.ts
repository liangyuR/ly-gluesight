import { ST, type FrameStatus, type Judgement, type Measured, type Recipe, type Verdict } from "../cycle/types";
import { referenceBadge } from "../cycle/vis";
import type { PartDetail } from "./types";

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
      if (before[n] !== "ok" || i === undefined || !original.points) return;
      const d0 = original.points.d[j], d1 = m.d[i];
      if (d0 != null && d1 != null) maxDd = Math.max(maxDd ?? 0, Math.abs(d0 - d1));
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
type ViewReading = { view: number; reading: { idx: number[]; st: number[]; w: (number | null)[]; edges: Edge[] } | null; error: string | null };

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
