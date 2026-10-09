import { useMemo } from "react";
import type { PointVis, Recipe } from "../types";
import { bounds, polyline, runs, visColor } from "../vis";

interface Props {
  layout: Recipe;
  vis: PointVis[];
  current?: number;
  /** 放大到某个拍照点的视野 */
  focus?: number | null;
  className?: string;
  compact?: boolean;
  selectedPoint?: number | null;
}

export default function TrajectoryMap({ layout, vis, current = -1, focus = null, className, compact = false, selectedPoint = null }: Props) {
  const [w, h, r] = layout.part;
  const [fw, fh] = layout.fov;
  const rect = !layout.path || layout.path.kind === "roundedRect";

  const viewBox = useMemo(() => {
    if (focus !== null && Number.isInteger(focus) && focus >= 0 && focus < layout.shots.length) {
      const [cx, cy] = layout.shots[focus];
      return `${cx - fw / 2} ${cy - fh / 2} ${fw} ${fh}`;
    }
    const [bx0, by0, bx1, by1] = bounds(layout);
    const xs = [bx0 - 30, bx1 + 30].concat(layout.shots.flatMap(([x]) => [x - fw / 2, x + fw / 2]));
    const ys = [by0 - 30, by1 + 30].concat(layout.shots.flatMap(([, y]) => [y - fh / 2, y + fh / 2]));
    const x0 = Math.min(...xs) - 8,
      y0 = Math.min(...ys) - 8;
    return `${x0} ${y0} ${Math.max(...xs) + 8 - x0} ${Math.max(...ys) + 8 - y0}`;
  }, [layout, focus, fw, fh]);

  const segs = useMemo(() => {
    const rs = runs(vis);
    return rs.map((run, i) => ({
      ...run,
      points: polyline(layout, run.from, run.to, i > 0 && rs[i - 1].state !== "gap", layout.closed && i === rs.length - 1 && rs[0].state === run.state),
    }));
  }, [layout, vis]);

  const outline = useMemo(() => polyline(layout, 0, layout.points.x.length - 1, false, layout.closed), [layout]);

  const gaps = segs.filter((s) => s.state === "gap");
  const sw = compact ? 1.6 : 2.6;

  return (
    <svg className={className} viewBox={viewBox} preserveAspectRatio="xMidYMid meet" aria-label="检测轨迹">
      {rect ? (
        <>
          {!compact && <rect x={-28} y={-28} width={w + 56} height={h + 56} rx={r + 8} fill="var(--bg-hover)" stroke="var(--border-strong)" vectorEffect="non-scaling-stroke" />}
          <rect x={4} y={4} width={w - 8} height={h - 8} rx={Math.max(r - 4, 2)} fill="var(--bg)" stroke="var(--text-disabled)" vectorEffect="non-scaling-stroke" />
          {!compact &&
            [
              [-12, -12],
              [w + 12, -12],
              [w + 12, h + 12],
              [-12, h + 12],
            ].map(([cx, cy]) => <circle key={`${cx},${cy}`} cx={cx} cy={cy} r={5} fill="var(--bg)" stroke="var(--text-muted)" vectorEffect="non-scaling-stroke" />)}
        </>
      ) : (
        <polyline points={outline} fill="none" stroke="var(--border-strong)" strokeWidth={10} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
      )}
      {!compact &&
        layout.shots.map(([cx, cy], k) => {
          const on = k === current;
          return (
            <g key={k}>
              <rect
                x={cx - fw / 2}
                y={cy - fh / 2}
                width={fw}
                height={fh}
                fill={on ? "var(--accent-overlay)" : "none"}
                stroke={on ? "var(--accent-text)" : "var(--border-strong)"}
                strokeWidth={on ? 1.6 : 1}
                strokeDasharray={on ? undefined : "5 4"}
                vectorEffect="non-scaling-stroke"
              />
              <text x={cx - fw / 2 + 5} y={cy - fh / 2 + 13} fontSize={11} fill={on ? "var(--accent-text)" : "var(--text-muted)"} className="mono">
                k{k + 1}
              </text>
            </g>
          );
        })}
      {segs.map(
        (s) =>
          s.state !== "gap" && (
            <polyline
              key={s.from}
              points={s.points}
              fill="none"
              stroke={visColor[s.state]}
              strokeWidth={sw}
              strokeLinecap="round"
              strokeDasharray={s.state === "none" ? "3 3" : s.state === "inv" || s.state === "miss" ? "4 3" : undefined}
              vectorEffect="non-scaling-stroke"
            />
          ),
      )}
      {!compact &&
        gaps.map((g) => {
          const j = Math.round((g.from + g.to) / 2);
          const x = layout.points.x[j],
            y = layout.points.y[j];
          return (
            <g key={`gap${g.from}`}>
              <circle cx={x} cy={y} r={9} fill="none" stroke="var(--ng)" strokeWidth={1.8} vectorEffect="non-scaling-stroke" />
              <text x={x + 12} y={y + 22} fontSize={11} fill="var(--ng)">
                断胶 {((g.to - g.from + 1) * layout.spacing).toFixed(1)} mm
              </text>
            </g>
          );
        })}
      {selectedPoint !== null && Number.isInteger(selectedPoint) && selectedPoint >= 0 && selectedPoint < layout.points.x.length && (
        <g aria-label={`选中测量点 ${selectedPoint + 1}`}>
          <circle cx={layout.points.x[selectedPoint]} cy={layout.points.y[selectedPoint]} r={5} fill="none" stroke="var(--text)" strokeWidth={2} vectorEffect="non-scaling-stroke" />
          <text x={layout.points.x[selectedPoint] + 8} y={layout.points.y[selectedPoint] - 8} fontSize={11} fill="var(--text)">点 {selectedPoint + 1}</text>
        </g>
      )}
    </svg>
  );
}
