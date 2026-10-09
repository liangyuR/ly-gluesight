import type { JudgeParams, Judgement, Measured, PartView, PointVis, Recipe } from "./types";

export const visColor: Record<PointVis, string> = {
  none: "var(--text-disabled)",
  ok: "var(--ok)",
  exc: "var(--warn)",
  ng: "var(--ng)",
  gap: "var(--ng)",
  inv: "var(--err)",
  miss: "var(--err)",
};

/** 相机在各处的代表色（曲线底部的测量归属）。 */
export const CAM_COLORS = Array.from({ length: 8 }, (_, i) => `var(--camera-${i + 1})`);

const outside = (v: number, p: JudgeParams) => v < p.nominal - p.tolLower || v > p.nominal + p.tolUpper;

export function computeVis(layout: Recipe, part: PartView | null, measured: Measured[], result: Judgement | null): PointVis[] {
  const n = layout.points.k.length;
  const vis: PointVis[] = new Array(n).fill("none");
  if (!part || part.recipeId !== layout.id || part.recipeHash !== layout.hash) return vis;
  for (const m of measured) {
    m.idx.forEach((j, i) => {
      const st = m.st[i];
      if (st === 1) vis[j] = "gap";
      else if (st === 2) {
        if (vis[j] === "none") vis[j] = "inv";
      } else {
        const seg = layout.segments[layout.points.seg[j]];
        const w = m.w?.[i];
        const bad = outside(m.d[i], seg.params) || (seg.width != null && w != null && outside(w, seg.width));
        vis[j] = bad ? "exc" : "ok";
      }
    });
  }
  part.frames.forEach((f, k) => {
    if (f.status !== "missing") return;
    layout.points.k.forEach((owner, j) => {
      if (owner === k && vis[j] === "none") vis[j] = "miss";
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

/** 按显示状态把胶路切成连续段。 */
export function runs(vis: PointVis[]): Run[] {
  const out: Run[] = [];
  vis.forEach((v, j) => {
    const last = out[out.length - 1];
    if (last && last.state === v) last.to = j;
    else out.push({ state: v, from: j, to: j });
  });
  return out;
}

export function polyline(layout: Recipe, from: number, to: number, joinPrev: boolean, close: boolean) {
  const { x, y } = layout.points;
  const pts: string[] = [];
  for (let j = joinPrev ? Math.max(0, from - 1) : from; j <= to; j++) pts.push(`${x[j].toFixed(1)},${y[j].toFixed(1)}`);
  if (close) pts.push(`${x[0].toFixed(1)},${y[0].toFixed(1)}`);
  return pts.join(" ");
}

export function currentFrame(part: PartView | null) {
  if (!part) return -1;
  let k = -1;
  part.frames.forEach((f, i) => {
    if (f.status !== "waiting" && f.status !== "missing") k = i;
  });
  return k;
}

/** 胶路包围盒 [x0, y0, x1, y1]。 */
export function bounds(layout: Recipe): [number, number, number, number] {
  const { x, y } = layout.points;
  return [Math.min(...x), Math.min(...y), Math.max(...x), Math.max(...y)];
}
