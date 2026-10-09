import { useEffect, useMemo, useRef, useState } from "react";
import type { JudgeParams, Measured, PointVis, Recipe } from "../types";
import { CAM_COLORS } from "../vis";

const PAD = { l: 38, r: 8, t: 8, b: 30 };

interface Props {
  layout: Recipe;
  measured: Measured[];
  vis: PointVis[];
  /** d：位置（飞拍为距内边距离，随动为横向偏移）；w：胶宽 */
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

export default function UnrolledCurve({ layout, measured, vis, quantity = "d", selected, onSelect }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 800, h: 150 });
  const [localSelection, setLocalSelection] = useState<{ scope: string; index: number } | null>(null);
  const selectionScope = `${layout.id}:${layout.hash}:${measured[0]?.sn ?? "empty"}:${quantity}`;

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setSize({ w: e.contentRect.width, h: e.contentRect.height }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const n = layout.points.k.length;
  const total = Math.max(0, (layout.closed ? n : n - 1) * layout.spacing);
  const chosen = selected === undefined ? (localSelection?.scope === selectionScope ? localSelection.index : null) : selected;
  const point = chosen !== null && Number.isInteger(chosen) && chosen >= 0 && chosen < n ? chosen : null;
  const select = (index: number | null) => {
    if (index !== null && (!Number.isInteger(index) || index < 0 || index >= n)) return;
    setLocalSelection(index === null ? null : { scope: selectionScope, index });
    onSelect?.(index);
  };
  const curveName = quantity === "w" ? "胶宽" : "位置";
  const limitOf = (g: Recipe["segments"][number]): JudgeParams | null => (quantity === "w" ? g.width : g.params);

  const { values, cams } = useMemo(() => {
    const values = new Float32Array(n).fill(NaN);
    const cams = new Int8Array(n).fill(-1);
    measured.forEach((m) =>
      m.idx.forEach((j, i) => {
        if (m.st[i] !== 0) return;
        const v = quantity === "w" ? m.w?.[i] : m.d[i];
        if (v == null || !Number.isFinite(v) || j < 0 || j >= n) return;
        values[j] = v;
        cams[j] = m.cam ?? 0;
      }),
    );
    return { values, cams };
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
  const X = (s: number) => PAD.l + (total > 0 ? s / total : 0) * (w - PAD.l - PAD.r);
  const Y = (v: number) => plotB - ((Math.max(lo, Math.min(hi, v)) - lo) / (hi - lo)) * (plotB - PAD.t);

  const lines = useMemo(() => {
    const out: string[] = [];
    let cur: string[] = [];
    for (let j = 0; j < n; j++) {
      if (Number.isNaN(values[j])) {
        if (cur.length > 1) out.push(cur.join(" "));
        cur = [];
      } else cur.push(`${X(j * layout.spacing).toFixed(1)},${Y(values[j]).toFixed(1)}`);
    }
    if (cur.length > 1) out.push(cur.join(" "));
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [values, w, h, n, total, lo, hi, layout.spacing]);

  // 底部色条：飞拍为各拍照点负责区间，随动为实际测到该点的相机
  const owners = useMemo(() => {
    const out: { k: number; s0: number; s1: number }[] = [];
    const key = (j: number) => (layout.mode === "flyShot" ? layout.points.k[j] : cams[j]);
    for (let j = 0; j < n; j++) {
      const k = key(j);
      const last = out[out.length - 1];
      if (last && last.k === k) last.s1 = Math.min(total, (j + 1) * layout.spacing);
      else out.push({ k, s0: j * layout.spacing, s1: Math.min(total, (j + 1) * layout.spacing) });
    }
    return out.filter((o) => o.k >= 0 && o.s1 > o.s0);
  }, [layout, cams, n, total]);

  return (
    <div style={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0, minWidth: 0 }}>
    <div ref={ref} className="curve-box">
      <svg width={w} height={h} aria-label={`${curveName}测量曲线`} onClick={event => {
        const rect = event.currentTarget.getBoundingClientRect();
        if (!rect.width || !n || !layout.spacing) return;
        const x = ((event.clientX - rect.left) / rect.width) * w;
        if (x < PAD.l || x > w - PAD.r) return;
        select(Math.min(n - 1, Math.max(0, Math.round(((x - PAD.l) / (w - PAD.l - PAD.r)) * total / layout.spacing))));
      }}>
        {niceTicks(lo, hi).map((v) => (
          <g key={v}>
            <line x1={PAD.l} x2={w - PAD.r} y1={Y(v)} y2={Y(v)} stroke="var(--border)" />
            <text x={PAD.l - 5} y={Y(v) + 3} textAnchor="end" fontSize={10} fill="var(--text-muted)">
              {v}
            </text>
          </g>
        ))}
        {layout.segments.map((g) => {
          const p = limitOf(g);
          return (
            <g key={g.name}>
              {p && (
                <>
                  <rect x={X(g.s0)} width={X(g.s1) - X(g.s0)} y={Y(p.nominal + p.tolUpper)} height={Y(p.nominal - p.tolLower) - Y(p.nominal + p.tolUpper)} fill="var(--ok-soft)" />
                  <line x1={X(g.s0)} x2={X(g.s1)} y1={Y(p.absMax)} y2={Y(p.absMax)} stroke="var(--ng)" strokeDasharray="4 3" />
                  <line x1={X(g.s0)} x2={X(g.s1)} y1={Y(p.absMin)} y2={Y(p.absMin)} stroke="var(--ng)" strokeDasharray="4 3" />
                </>
              )}
              <line x1={X(g.s0)} x2={X(g.s0)} y1={PAD.t} y2={plotB + 8} stroke="var(--border-strong)" />
              {X(g.s1) - X(g.s0) > 36 && (
                <text x={(X(g.s0) + X(g.s1)) / 2} y={h - 6} textAnchor="middle" fontSize={10.5} fill="var(--text-muted)">
                  {g.name}
                </text>
              )}
            </g>
          );
        })}
        {owners.map((o) => (
          <rect key={o.s0} x={X(o.s0)} width={Math.max(1, X(o.s1) - X(o.s0))} y={plotB + 3} height={4} fill={CAM_COLORS[o.k % CAM_COLORS.length]} opacity={0.7} />
        ))}
        {lines.map((pts, i) => (
          <polyline key={i} points={pts} fill="none" stroke="var(--text)" strokeWidth={1} />
        ))}
        {vis.map((v, j) =>
          (v === "exc" || v === "ng") && !Number.isNaN(values[j]) ? (
            <circle key={j} cx={X(j * layout.spacing)} cy={Y(values[j])} r={2.2} fill={v === "ng" ? "var(--ng)" : "var(--warn)"} />
          ) : v === "gap" ? (
            <line key={j} x1={X(j * layout.spacing)} x2={X(j * layout.spacing)} y1={PAD.t} y2={plotB} stroke="var(--ng)" strokeWidth={2} />
          ) : null,
        )}
        {point !== null && <g aria-label={`曲线选中点 ${point + 1}`}>
          <line x1={X(point * layout.spacing)} x2={X(point * layout.spacing)} y1={PAD.t} y2={plotB} stroke="var(--accent)" strokeDasharray="3 3" />
          {!Number.isNaN(values[point]) && <circle cx={X(point * layout.spacing)} cy={Y(values[point])} r={4} fill="var(--accent)" />}
        </g>}
      </svg>
    </div>
    {n > 0 && <div className="curve-selection" style={{ display: "flex", alignItems: "center", flexWrap: "wrap", flexShrink: 0, gap: 8, paddingTop: 6, fontSize: 12 }}>
      <input type="range" style={{ width: 160 }} min={0} max={n - 1} step={1} value={point ?? 0} aria-label={`${curveName}曲线选点`}
        aria-valuetext={`点 ${(point ?? 0) + 1}`} onFocus={() => { if (point === null) select(0); }} onChange={event => select(Number(event.target.value))} />
      <span role="status">{point === null ? "点击曲线或用方向键选择测量点" : `点 ${point + 1} · s=${(point * layout.spacing).toFixed(2)} mm · ${quantity === "w" ? "胶宽" : "d"}=${Number.isNaN(values[point]) ? "—" : values[point].toFixed(2)} mm · ${pointLabels[vis[point] ?? "none"]}`}</span>
      {point !== null && <button type="button" className="btn small" aria-label={`清除${curveName}曲线选点`} onClick={() => select(null)}>清除选点</button>}
    </div>}
    </div>
  );
}
