import { useEffect, useMemo, useRef, useState } from "react";
import type { JudgeParams, Measured, PointVis, Recipe } from "../types";
import { CAM_COLORS, segmentLength } from "../vis";

const PAD = { l: 38, r: 8, t: 8, b: 30 };

interface Props {
  layout: Recipe;
  measured: Measured[];
  vis: PointVis[];
  /** d：横向偏移（相对示教中线）；w：胶宽 */
  quantity?: "d" | "w";
  selected?: number | null;
  onSelect?: (index: number | null) => void;
}
const pointLabels: Record<PointVis, string> = { none: "待测", ok: "合格", exc: "超公差", ng: "NG", gap: "断胶", inv: "未测成", miss: "缺帧" };

function niceTicks(lo: number, hi: number): number[] {
  const span = hi - lo;
  const raw = span / 4;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? raw;
  const out: number[] = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + 1e-9; v += step) out.push(Number(v.toFixed(6)));
  return out;
}

/** 展开曲线：横轴按拍照点分段（段与段之间留断口），段内是沿示教中线的弧长。 */
export default function UnrolledCurve({ layout, measured, vis, quantity = "d", selected, onSelect }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 800, h: 150 });
  const [localSelection, setLocalSelection] = useState<{ scope: string; index: number } | null>(null);
  const selectionScope = `${layout.id}:${layout.hash}:${measured[0]?.cycleId ?? "empty"}:${measured[0]?.bundleHash ?? ""}:${quantity}`;

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setSize({ w: e.contentRect.width, h: e.contentRect.height }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const n = layout.points.k.length;
  const sp = layout.spacing;
  // 各段在横轴上的起点：段长之和，段与段之间留一个断口
  const axis = useMemo(() => {
    const lens = layout.segments.map((g) => segmentLength(g, sp));
    const content = lens.reduce((a, b) => a + b, 0);
    const gap = layout.segments.length > 1 ? Math.max(content * 0.04, sp) : 0;
    let o = 0;
    const offsets = lens.map((len) => {
      const at = o;
      o += len + gap;
      return at;
    });
    return { lens, offsets, gap, total: Math.max(0, o - gap) };
  }, [layout, sp]);
  /** 点 j 在横轴上的位置与段内弧长。 */
  const at = (j: number) => {
    const gi = layout.points.seg[j];
    const g = layout.segments[gi];
    if (!g) return null;
    const s = (j - g.first) * sp;
    return { gi, g, s, v: axis.offsets[gi] + s };
  };

  const chosen = selected === undefined ? (localSelection?.scope === selectionScope ? localSelection.index : null) : selected;
  const point = chosen !== null && Number.isInteger(chosen) && chosen >= 0 && chosen < n ? chosen : null;
  const select = (index: number | null) => {
    if (index !== null && (!Number.isInteger(index) || index < 0 || index >= n)) return;
    setLocalSelection(index === null ? null : { scope: selectionScope, index });
    onSelect?.(index);
  };
  const curveName = quantity === "w" ? "胶宽" : "位置";
  const limitOf = (g: Recipe["segments"][number]): JudgeParams | null => (quantity === "w" ? g.width : g.position);

  const values = useMemo(() => {
    const values = new Float32Array(n).fill(NaN);
    measured.forEach((m) =>
      m.idx.forEach((j, i) => {
        if (m.st[i] !== 0) return;
        const v = quantity === "w" ? m.w?.[i] : m.d[i];
        if (v == null || !Number.isFinite(v) || j < 0 || j >= n) return;
        values[j] = v;
      }),
    );
    return values;
  }, [measured, n, quantity]);

  const [lo, hi] = useMemo(() => {
    let lo = Infinity,
      hi = -Infinity;
    layout.segments.forEach((g) => {
      const p = limitOf(g);
      if (!p) return;
      lo = Math.min(lo, p.absMin, p.nominal - p.tolLower);
      hi = Math.max(hi, p.absMax, p.nominal + p.tolUpper);
    });
    values.forEach((v) => {
      if (!Number.isNaN(v)) {
        lo = Math.min(lo, v);
        hi = Math.max(hi, v);
      }
    });
    if (!Number.isFinite(lo)) return [0, 1];
    const pad = (hi - lo) * 0.08 || 0.1;
    return [lo - pad, hi + pad];
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layout, values, quantity]);

  const { w, h } = size;
  const plotB = h - PAD.b;
  const X = (v: number) => PAD.l + (axis.total > 0 ? v / axis.total : 0) * (w - PAD.l - PAD.r);
  const Y = (v: number) => plotB - ((Math.max(lo, Math.min(hi, v)) - lo) / (hi - lo)) * (plotB - PAD.t);

  // 每段各自成线，测不到的点处断开，段与段之间不连
  const lines = useMemo(() => {
    const out: string[] = [];
    layout.segments.forEach((g, gi) => {
      let cur: string[] = [];
      for (let j = g.first; j < g.first + g.count && j < n; j++) {
        if (Number.isNaN(values[j])) {
          if (cur.length > 1) out.push(cur.join(" "));
          cur = [];
        } else cur.push(`${X(axis.offsets[gi] + (j - g.first) * sp).toFixed(1)},${Y(values[j]).toFixed(1)}`);
      }
      if (cur.length > 1) out.push(cur.join(" "));
    });
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [values, w, h, n, axis, lo, hi, layout.segments, sp]);

  /** 横轴位置 → 最近一段里最近的点。 */
  const pick = (x: number): number | null => {
    const v = ((x - PAD.l) / (w - PAD.l - PAD.r)) * axis.total;
    let best: number | null = null, dist = Infinity;
    for (let gi = 0; gi < layout.segments.length; gi++) {
      const local = Math.min(axis.lens[gi], Math.max(0, v - axis.offsets[gi]));
      const d = Math.abs(v - axis.offsets[gi] - local);
      if (d < dist) {
        dist = d;
        best = layout.segments[gi].first + Math.round(local / sp);
      }
    }
    return best;
  };
  const sel = point === null ? null : at(point);

  return (
    <div style={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0, minWidth: 0 }}>
    <div ref={ref} className="curve-box">
      <svg width={w} height={h} aria-label={`${curveName}测量曲线`} onClick={event => {
        const rect = event.currentTarget.getBoundingClientRect();
        if (!rect.width || !n || !sp || !layout.segments.length) return;
        const x = ((event.clientX - rect.left) / rect.width) * w;
        if (x < PAD.l || x > w - PAD.r) return;
        const j = pick(x);
        if (j !== null) select(Math.min(n - 1, j));
      }}>
        {niceTicks(lo, hi).map((v) => (
          <g key={v}>
            <line x1={PAD.l} x2={w - PAD.r} y1={Y(v)} y2={Y(v)} stroke="var(--border)" />
            <text x={PAD.l - 5} y={Y(v) + 3} textAnchor="end" fontSize={10} fill="var(--text-muted)">
              {v}
            </text>
          </g>
        ))}
        {layout.segments.map((g, gi) => {
          const p = limitOf(g);
          const x0 = X(axis.offsets[gi]), x1 = X(axis.offsets[gi] + axis.lens[gi]);
          return (
            <g key={`${g.name}:${gi}`} aria-label={`段 ${g.name}`}>
              {gi > 0 && <rect x={X(axis.offsets[gi] - axis.gap)} width={Math.max(0, x0 - X(axis.offsets[gi] - axis.gap))} y={PAD.t} height={plotB - PAD.t} fill="var(--bg-hover)" />}
              {p && (
                <>
                  <rect x={x0} width={x1 - x0} y={Y(p.nominal + p.tolUpper)} height={Y(p.nominal - p.tolLower) - Y(p.nominal + p.tolUpper)} fill="var(--ok-soft)" />
                  <line x1={x0} x2={x1} y1={Y(p.absMax)} y2={Y(p.absMax)} stroke="var(--ng)" strokeDasharray="4 3" />
                  <line x1={x0} x2={x1} y1={Y(p.absMin)} y2={Y(p.absMin)} stroke="var(--ng)" strokeDasharray="4 3" />
                </>
              )}
              <line x1={x0} x2={x0} y1={PAD.t} y2={plotB + 8} stroke="var(--border-strong)" />
              <rect x={x0} width={Math.max(1, x1 - x0)} y={plotB + 3} height={4} fill={CAM_COLORS[g.shot % CAM_COLORS.length]} opacity={0.7} />
              {x1 - x0 > 36 && (
                <text x={(x0 + x1) / 2} y={h - 6} textAnchor="middle" fontSize={10.5} fill="var(--text-muted)">
                  {g.name}
                </text>
              )}
            </g>
          );
        })}
        {lines.map((pts, i) => (
          <polyline key={i} points={pts} fill="none" stroke="var(--text)" strokeWidth={1} />
        ))}
        {vis.map((v, j) => {
          const a = j < n ? at(j) : null;
          if (!a) return null;
          return (v === "exc" || v === "ng") && !Number.isNaN(values[j]) ? (
            <circle key={j} cx={X(a.v)} cy={Y(values[j])} r={2.2} fill={v === "ng" ? "var(--ng)" : "var(--warn)"} />
          ) : v === "gap" ? (
            <line key={j} x1={X(a.v)} x2={X(a.v)} y1={PAD.t} y2={plotB} stroke="var(--ng)" strokeWidth={2} />
          ) : null;
        })}
        {point !== null && sel && <g aria-label={`曲线选中点 ${point + 1}`}>
          <line x1={X(sel.v)} x2={X(sel.v)} y1={PAD.t} y2={plotB} stroke="var(--accent)" strokeDasharray="3 3" />
          {!Number.isNaN(values[point]) && <circle cx={X(sel.v)} cy={Y(values[point])} r={4} fill="var(--accent)" />}
        </g>}
      </svg>
    </div>
    {n > 0 && <div className="curve-selection" style={{ display: "flex", alignItems: "center", flexWrap: "wrap", flexShrink: 0, gap: 8, paddingTop: 6, fontSize: 12 }}>
      <input type="range" style={{ width: 160 }} min={0} max={n - 1} step={1} value={point ?? 0} aria-label={`${curveName}曲线选点`}
        aria-valuetext={`点 ${(point ?? 0) + 1}`} onFocus={() => { if (point === null) select(0); }} onChange={event => select(Number(event.target.value))} />
      <span role="status">{point === null ? "点击曲线或用方向键选择测量点" : `点 ${point + 1}${sel ? ` · ${sel.g.name} · s=${sel.s.toFixed(2)} mm` : ""} · ${quantity === "w" ? "胶宽" : "d"}=${Number.isNaN(values[point]) ? "—" : values[point].toFixed(2)} mm · ${pointLabels[vis[point] ?? "none"]}`}</span>
      {point !== null && <button type="button" className="btn small" aria-label={`清除${curveName}曲线选点`} onClick={() => select(null)}>清除选点</button>}
    </div>}
    </div>
  );
}
