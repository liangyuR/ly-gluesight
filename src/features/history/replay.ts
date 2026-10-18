import { shotSegment } from "../cycle/vis";
import type { FrameStatus, Judgement, Measured, Recipe, Verdict } from "../cycle/types";
import type { PartDetail } from "./types";

/** 重跑与原测量的逐点偏移、胶宽差超过此值（mm）记为差异。同一发布包、同一原图应逐位一致，留 0.01 mm 只为吸收序列化舍入。 */
export const REPLAY_TOLERANCE_MM = 0.01;

type PointKind = "ok" | "gap" | "inv";
const kindOf = (st: number | undefined): PointKind => st === 0 ? "ok" : st === 1 ? "gap" : "inv";

export interface ShotSide {
  verdict: Verdict | null;
  ok: number;
  gap: number;
  inv: number;
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

export function sameLayout(a: Recipe, b: Recipe) {
  return a.shots.length === b.shots.length && a.points.k.length === b.points.k.length
    && a.shots.every((shot, k) => shot.id === b.shots[k].id && shot.camera === b.shots[k].camera)
    && a.points.k.every((k, j) => k === b.points.k[j]);
}

const count = (kinds: PointKind[]) => ({ ok: kinds.filter(x => x === "ok").length, gap: kinds.filter(x => x === "gap").length, inv: kinds.filter(x => x === "inv").length });
const segmentVerdict = (layout: Recipe, judgement: Judgement, k: number) => {
  const own = shotSegment(layout, k);
  return own ? judgement.segments[own.index]?.verdict ?? null : null;
};

/** 逐拍照点比较原结果与重跑结果；测点布局不同（候选改了拍照点或测点）时无法逐点对应，返回 null。 */
export function shotDiffs(layout: Recipe, original: Original, replay: Replay): ShotDiff[] | null {
  if (!sameLayout(layout, replay.candidateRecipe)) return null;
  return layout.shots.map((shot, k) => {
    const owned = layout.points.k.flatMap((owner, j) => owner === k ? [j] : []);
    const before = owned.map(j => kindOf(original.points?.st[j]));
    const record = original.shots.find(s => s.k === k);
    const m = replay.measurements.find(x => x.k === k);
    const at = new Map((m?.idx ?? []).map((j, i) => [j, i]));
    const failed = !!m && (!!m.error || !m.located);
    const after = owned.map(j => m && !failed && at.has(j) ? kindOf(m.st[at.get(j)!]) : "inv" as PointKind);
    const changed: number[] = [];
    let maxDd: number | null = null, maxDw: number | null = null;
    if (m && !failed) owned.forEach((j, n) => {
      if (before[n] !== after[n]) { changed.push(j); return; }
      const i = at.get(j);
      if (before[n] !== "ok" || i === undefined || !original.points) return;
      maxDd = Math.max(maxDd ?? 0, Math.abs(original.points.d[j] - m.d[i]));
      const w0 = original.points.w?.[j], w1 = m.w[i];
      if (w0 != null && w1 != null && Number.isFinite(w0) && Number.isFinite(w1)) maxDw = Math.max(maxDw ?? 0, Math.abs(w0 - w1));
    });
    if (m && failed) owned.forEach((j, n) => { if (before[n] !== "inv") changed.push(j); });
    const verdict = { before: segmentVerdict(layout, original.judgement, k), after: segmentVerdict(replay.candidateRecipe, replay.judgement, k) };
    const differs = verdict.before !== verdict.after || (!!m && (changed.length > 0 || (maxDd ?? 0) > REPLAY_TOLERANCE_MM || (maxDw ?? 0) > REPLAY_TOLERANCE_MM));
    return {
      k, shotId: shot.id, camera: shot.camera, skip: shot.skip, remeasured: !!m,
      original: { verdict: verdict.before, ...count(before), status: record?.status ?? null, error: record?.error ?? null },
      replay: { verdict: verdict.after, ...(m ? count(after) : { ok: 0, gap: 0, inv: 0 }), error: m?.error ?? (m && !m.located ? "定位失败" : null) },
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
