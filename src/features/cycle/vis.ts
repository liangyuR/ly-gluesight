import { ST, type FrameView, type JudgeParams, type Judgement, type Measured, type PartView, type PointVis, type Recipe, type Segment, type ShotSpec } from "./types";
import { matchesPart } from "./identity";

export const visColor: Record<PointVis, string> = {
  none: "var(--on-surface-disabled)",
  ok: "var(--ok)",
  exc: "var(--warn)",
  ng: "var(--ng)",
  gap: "var(--ng)",
  inv: "var(--fault)",
  miss: "var(--fault)",
  noRef: "var(--fault)",
  out: "var(--on-surface-disabled)",
};

/** 站线的虚线样式：没测到胶（未测、未测成、缺帧）与超出检测范围用虚线，有胶的站（含无基准）用实线。 */
export const visDash: Partial<Record<PointVis, string>> = { none: "3 3", inv: "4 3", miss: "4 3", out: "1 4" };

/** 帧上的位置基准徽标文字（如“定位失败”“图 2 两端搬移超限”）；基准正常时为 null。 */
export function referenceBadge(failures: FrameView["referenceFailed"] | undefined, withView = false): string | null {
  if (!failures?.length) return null;
  return [...new Set(failures.map((f) => (withView ? `图 ${f.view} ` : "") + f.reason))].join("、");
}

/** 整件的位置基准徽标：任一帧没有位置基准即显示，不论整件判 NG 还是 ERR。 */
export const partReferenceBadge = (part: Pick<PartView, "frames"> | null | undefined) => referenceBadge(part?.frames.flatMap((f) => f.referenceFailed ?? []));

/** 相机在各处的代表色（曲线底部的测量归属）。 */
export const CAM_COLORS = Array.from({ length: 8 }, (_, i) => `var(--cam-${i + 1})`);

/** 拍照点用到的相机编号，按第一次出现的顺序（与 Recipe.cameras() 一致）。 */
export function shotCameras(shots: Pick<ShotSpec, "camera">[]): string[] {
  return [...new Set(shots.map((s) => s.camera))];
}

/** 拍照点的简短名称：编号 · 相机。 */
export const shotLabel = (shot: Pick<ShotSpec, "id" | "camera" | "view"> | undefined, k: number) => (shot ? `${shot.id} · ${shot.camera} · 视角 ${shot.view}` : `k${k + 1}`);

/** 折线长度（px）。 */
export function pathLength(path: [number, number][]) {
  let len = 0;
  for (let i = 1; i < path.length; i++) len += Math.hypot(path[i][0] - path[i - 1][0], path[i][1] - path[i - 1][1]);
  return len;
}

/** 已示教：有中线（至少两点）和像素当量（与 recipe.rs 的 ShotSpec::taught 一致）。 */
export const shotTaught = (shot: Pick<ShotSpec, "path" | "mmPerPx">) => shot.path.length >= 2 && shot.mmPerPx != null;

/** 拍照点的示教状态：不检 / 已示教 n 点 · x mm / 未示教。 */
export function teachStatus(shot: Pick<ShotSpec, "path" | "mmPerPx" | "skip">) {
  if (shot.skip) return "不检";
  if (!shotTaught(shot)) return "未示教";
  const mm = pathLength(shot.path) * shot.mmPerPx!;
  return `已示教 ${shot.path.length} 点 · ${Number.isFinite(mm) ? mm.toFixed(1) : "—"} mm`;
}

/** 拍照点 k 的段（不检、未示教的拍照点没有）。 */
export const shotSegment = (layout: Pick<Recipe, "segments">, k: number): { index: number; segment: Segment } | null => {
  const index = layout.segments.findIndex((g) => g.shot === k);
  return index < 0 ? null : { index, segment: layout.segments[index] };
};

/** 段的弧长（mm）。 */
export const segmentLength = (g: Pick<Segment, "count">, spacing: number) => Math.max(0, g.count - 1) * spacing;

const outside = (v: number, p: JudgeParams) => v < p.nominal - p.tolLower || v > p.nominal + p.tolUpper;

export function computeVis(layout: Recipe, part: PartView | null, measured: Measured[], result: Judgement | null): PointVis[] {
  const n = layout.points.k.length;
  const vis: PointVis[] = new Array(n).fill("none");
  if (!part || part.recipeId !== layout.id || part.recipeRevision !== layout.revisionId) return vis;
  for (const m of measured) {
    if (!matchesPart(m, part)) continue;
    m.idx.forEach((j, i) => {
      if (j < 0 || j >= n) return;
      const st = m.st[i];
      if (st === ST.gap) vis[j] = "gap";
      else if (st === ST.ok || st === ST.noRef || st === ST.outOfRange) {
        const seg = layout.segments[layout.points.seg[j]];
        const d = st === ST.ok ? m.d[i] : null, w = m.w?.[i];
        const bad = !!seg && ((seg.position != null && d != null && Number.isFinite(d) && outside(d, seg.position)) || (seg.width != null && w != null && outside(w, seg.width)));
        // 无基准的站有胶、胶宽照判，只是不判位置：胶宽超差仍显示超差；不判位置的段里与正常站无异
        vis[j] = bad ? "exc" : st === ST.outOfRange ? "out" : st === ST.noRef && seg?.position != null ? "noRef" : "ok";
      } else if (vis[j] === "none") vis[j] = "inv";
    });
  }
  part.frames.forEach((f, k) => {
    if (f.status !== "missing" && f.status !== "error") return;
    layout.points.k.forEach((owner, j) => {
      if (owner === k && vis[j] === "none") vis[j] = f.status === "missing" ? "miss" : "inv";
    });
  });
  result?.segments.forEach((s, gi) => {
    if (s.verdict !== "NG_POSITION" && s.verdict !== "NG_ABSOLUTE" && s.verdict !== "NG_WIDTH") return;
    layout.points.seg.forEach((seg, j) => {
      if (seg === gi && vis[j] === "exc") vis[j] = "ng";
    });
  });
  return vis;
}

export interface Run {
  state: PointVis;
  from: number;
  to: number;
}

/** 按显示状态把测量点切成连续段。 */
export function runs(vis: PointVis[]): Run[] {
  const out: Run[] = [];
  vis.forEach((v, j) => {
    const last = out[out.length - 1];
    if (last && last.state === v) last.to = j;
    else out.push({ state: v, from: j, to: j });
  });
  return out;
}

/** 一个拍照点的显示状态：不检、未示教，或按它这一段测量点里最严重的状态。 */
export type ShotState = PointVis | "skip" | "untaught";

// 超出检测范围排在最后：拍照点里只要有参与判定的站，就按它们显示
const severity: PointVis[] = ["gap", "ng", "miss", "inv", "noRef", "exc", "none", "ok", "out"];

export const shotStateLabel: Record<ShotState, string> = {
  skip: "不检",
  untaught: "未示教",
  none: "待测",
  ok: "合格",
  exc: "局部超差",
  ng: "NG",
  gap: "断胶",
  inv: "未测成",
  miss: "缺帧",
  noRef: "无基准",
  out: "超出检测范围",
};

export function shotState(layout: Recipe, vis: PointVis[], k: number): ShotState {
  const shot = layout.shots[k];
  if (shot?.skip) return "skip";
  const own = shotSegment(layout, k);
  if (!own) return "untaught";
  const { first, count } = own.segment;
  const states = new Set(vis.slice(first, first + count));
  return severity.find((s) => states.has(s)) ?? "none";
}

/** 拍照点内的缺胶区间（段内弧长 mm，与后端 GapRun 的 s0、s1 一致）。 */
export function shotGaps(layout: Recipe, vis: PointVis[], k: number): { s0: number; s1: number }[] {
  const own = shotSegment(layout, k);
  if (!own) return [];
  const { first, count } = own.segment;
  return runs(vis.slice(first, first + count))
    .filter((r) => r.state === "gap")
    .map((r) => ({ s0: r.from * layout.spacing, s1: (r.to + 1) * layout.spacing }));
}

export function currentFrame(part: PartView | null) {
  if (!part) return -1;
  let k = -1;
  part.frames.forEach((f, i) => {
    if (f.status !== "waiting" && f.status !== "missing") k = i;
  });
  return k;
}
