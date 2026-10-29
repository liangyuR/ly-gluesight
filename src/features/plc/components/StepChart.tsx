import { useEffect, useMemo, useRef, useState } from "react";
import { EmptyState } from "qing-steel";
import type { HistorySample } from "../types";
import { formatClock, formatTs } from "../time";

interface StepChartProps {
  samples: HistorySample[];
  start: number;
  end: number;
}

const H = 200;
const PAD = { l: 56, r: 16, t: 14, b: 28 };

function toNum(v: string | null): number | null {
  if (v === null) return null;
  if (v === "true") return 1;
  if (v === "false") return 0;
  const n = Number(v);
  return Number.isNaN(n) ? null : n;
}

function fmtNum(n: number) {
  return Math.abs(n) >= 1000 || Number.isInteger(n) ? String(Math.round(n * 100) / 100) : n.toFixed(2);
}

export default function StepChart({ samples, start, end }: StepChartProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(800);
  const [hoverX, setHoverX] = useState<number | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const chart = useMemo(() => {
    const pts = samples.map((s) => ({ ts: s.ts, raw: s.value, v: toNum(s.value) }));
    const nums = pts.flatMap((p) => (p.v === null ? [] : [p.v]));
    if (nums.length === 0 || end <= start) return null;
    const isBool = samples.every((s) => s.value === null || s.value === "true" || s.value === "false");
    let min = isBool ? 0 : Math.min(...nums);
    let max = isBool ? 1 : Math.max(...nums);
    if (min === max) {
      min -= 1;
      max += 1;
    }
    const span = max - min;
    if (!isBool) {
      min -= span * 0.08;
      max += span * 0.08;
    } else {
      min -= 0.15;
      max += 0.15;
    }
    const innerW = Math.max(10, width - PAD.l - PAD.r);
    const innerH = H - PAD.t - PAD.b;
    const x = (ts: number) => PAD.l + ((Math.min(Math.max(ts, start), end) - start) / (end - start)) * innerW;
    const y = (v: number) => PAD.t + (1 - (v - min) / (max - min)) * innerH;
    const tail = Math.min(end, Date.now());

    let d = "";
    let prevY: number | null = null;
    pts.forEach((p, i) => {
      if (p.v === null) {
        prevY = null;
        return;
      }
      const x0 = x(p.ts);
      const x1 = x(i + 1 < pts.length ? pts[i + 1].ts : tail);
      const yy = y(p.v);
      d += prevY === null ? `M${x0},${yy}` : `V${yy}`;
      d += `H${x1}`;
      prevY = yy;
    });

    const yTicks = isBool ? [0, 1] : Array.from({ length: 5 }, (_, i) => min + ((max - min) * (i + 0.5)) / 5);
    const longRange = end - start > 86_400_000;
    const xTicks = Array.from({ length: 6 }, (_, i) => start + ((end - start) * i) / 5);
    return { pts, d, x, y, yTicks, xTicks, isBool, innerW, longRange };
  }, [samples, start, end, width]);

  if (!chart) {
    return (
      <div ref={ref}><EmptyState title="所选时间段内无该点位的值变化记录" /></div>
    );
  }

  const hoverTs = hoverX === null ? null : start + ((hoverX - PAD.l) / chart.innerW) * (end - start);
  const hoverSample =
    hoverTs === null ? null : [...chart.pts].reverse().find((p) => p.ts <= hoverTs) ?? null;

  return (
    <div ref={ref} className="chart">
      <svg
        width={width}
        height={H}
        onMouseMove={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          const px = e.clientX - rect.left;
          setHoverX(px >= PAD.l && px <= PAD.l + chart.innerW ? px : null);
        }}
        onMouseLeave={() => setHoverX(null)}
      >
        {chart.yTicks.map((t) => (
          <g key={t}>
            <line className="grid" x1={PAD.l} x2={PAD.l + chart.innerW} y1={chart.y(t)} y2={chart.y(t)} />
            <text className="axis" x={PAD.l - 8} y={chart.y(t)} textAnchor="end" dominantBaseline="middle">
              {chart.isBool ? (t ? "ON" : "OFF") : fmtNum(t)}
            </text>
          </g>
        ))}
        {chart.xTicks.map((t, i) => (
          <text
            key={t}
            className="axis"
            x={chart.x(t)}
            y={H - 8}
            textAnchor={i === 0 ? "start" : i === chart.xTicks.length - 1 ? "end" : "middle"}
          >
            {chart.longRange ? formatTs(t, false).slice(5, 16) : formatClock(t)}
          </text>
        ))}
        <path className="line" d={chart.d} />
        {hoverX !== null && (
          <line className="cursor" x1={hoverX} x2={hoverX} y1={PAD.t} y2={H - PAD.b} />
        )}
      </svg>
      {hoverX !== null && hoverTs !== null && (
        <div className="chart-tip" style={{ left: Math.min(hoverX + 12, width - 200) }}>
          <div className="mono">{formatTs(hoverTs)}</div>
          <div>
            值 <b className="mono">{hoverSample?.raw ?? "--"}</b>
          </div>
        </div>
      )}
    </div>
  );
}
