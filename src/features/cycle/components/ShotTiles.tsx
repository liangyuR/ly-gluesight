import { useMemo } from "react";
import type { PointVis, Recipe } from "../types";
import { runs, segmentLength, shotGaps, shotLabel, shotSegment, shotState, shotStateLabel, visColor, type ShotState } from "../vis";

export const shotTone: Record<ShotState, string> = {
  skip: "c-mut",
  untaught: "c-mut",
  none: "c-mut",
  ok: "c-ok",
  exc: "c-warn",
  ng: "c-ng",
  gap: "c-ng",
  inv: "c-err",
  miss: "c-err",
};

const fmt = (v: number) => v.toFixed(1);

/** 一个拍照点在自己图像里的示教中线与各站状态（图像像素坐标，不做对齐变换）。 */
export function ShotLine({ layout, k, vis, compact = false, selectedPoint = null, className }: {
  layout: Recipe; k: number; vis: PointVis[]; compact?: boolean; selectedPoint?: number | null; className?: string;
}) {
  const shot = layout.shots[k];
  const own = shotSegment(layout, k)?.segment ?? null;
  const { x, y } = layout.points;
  const viewBox = useMemo(() => {
    const xs = (shot?.path ?? []).map((p) => p[0]), ys = (shot?.path ?? []).map((p) => p[1]);
    if (own) for (let j = own.first; j < own.first + own.count; j++) { xs.push(x[j]); ys.push(y[j]); }
    const finite = (v: number[]) => v.filter(Number.isFinite);
    const fx = finite(xs), fy = finite(ys);
    if (!fx.length || !fy.length) return "0 0 100 100";
    const [x0, x1, y0, y1] = [Math.min(...fx), Math.max(...fx), Math.min(...fy), Math.max(...fy)];
    const pad = Math.max(x1 - x0, y1 - y0) * 0.08 + 12;
    return `${x0 - pad} ${y0 - pad} ${x1 - x0 + 2 * pad} ${y1 - y0 + 2 * pad}`;
  }, [shot, own, x, y]);
  if (!shot) return null;
  const pts = (from: number, to: number) => {
    const out: string[] = [];
    for (let j = from; j <= to; j++) out.push(`${fmt(x[j])},${fmt(y[j])}`);
    return out.join(" ");
  };
  const segRuns = own ? runs(vis.slice(own.first, own.first + own.count)).map((r) => ({ ...r, from: r.from + own.first, to: r.to + own.first })) : [];
  const gaps = own && !compact ? shotGaps(layout, vis, k) : [];
  const gapRuns = segRuns.filter((r) => r.state === "gap");
  const sw = compact ? 1.6 : 2.6;
  const selected = own && selectedPoint !== null && selectedPoint >= own.first && selectedPoint < own.first + own.count ? selectedPoint : null;
  return (
    <svg className={className} viewBox={viewBox} preserveAspectRatio="xMidYMid meet" aria-label={`拍照点 ${shot.id} 示教中线`}>
      {shot.path.length >= 2 && (
        <polyline points={shot.path.map(([px, py]) => `${fmt(px)},${fmt(py)}`).join(" ")} fill="none" stroke="var(--border-strong)" strokeWidth={compact ? 4 : 8} strokeLinejoin="round" strokeLinecap="round" vectorEffect="non-scaling-stroke" />
      )}
      {shot.path.length > 0 && !compact && <circle cx={shot.path[0][0]} cy={shot.path[0][1]} r={4} fill="var(--bg)" stroke="var(--text-muted)" vectorEffect="non-scaling-stroke" aria-label="胶嘴一侧" />}
      {segRuns.map((r, i) =>
        r.state === "gap" ? null : (
          <polyline
            key={r.from}
            points={pts(i > 0 && segRuns[i - 1].state !== "gap" ? r.from - 1 : r.from, r.to)}
            fill="none"
            stroke={visColor[r.state]}
            strokeWidth={sw}
            strokeLinecap="round"
            strokeDasharray={r.state === "none" ? "3 3" : r.state === "inv" || r.state === "miss" ? "4 3" : undefined}
            vectorEffect="non-scaling-stroke"
          />
        ),
      )}
      {gapRuns.map((r, i) => {
        const j = Math.round((r.from + r.to) / 2);
        return (
          <g key={`gap${r.from}`} aria-label={`断胶 s=${fmt(gaps[i]?.s0 ?? 0)}–${fmt(gaps[i]?.s1 ?? 0)} mm`}>
            <circle cx={x[j]} cy={y[j]} r={compact ? 5 : 9} fill="none" stroke="var(--ng)" strokeWidth={1.8} vectorEffect="non-scaling-stroke" />
          </g>
        );
      })}
      {selected !== null && (
        <g aria-label={`选中测量点 ${selected + 1}`}>
          <circle cx={x[selected]} cy={y[selected]} r={5} fill="none" stroke="var(--text)" strokeWidth={2} vectorEffect="non-scaling-stroke" />
        </g>
      )}
    </svg>
  );
}

interface Props {
  layout: Recipe;
  vis: PointVis[];
  /** 选中的拍照点 */
  selected?: number | null;
  /** 正在采集的拍照点 */
  current?: number;
  onSelect?: (k: number) => void;
  selectedPoint?: number | null;
  className?: string;
  /** 还没有测量时已示教拍照点的标签（配方预览里显示“已示教”而不是“待测”） */
  idleLabel?: string;
}

/** 逐拍照点视图：每个拍照点一格，按计划顺序；显示编号 · 相机 · 胶条、不检 / 未示教、按测量点状态着色的示教中线与段内缺胶位置。 */
export default function ShotTiles({ layout, vis, selected = null, current = -1, onSelect, selectedPoint = null, className = "", idleLabel }: Props) {
  return (
    <div className={`shot-tiles ${className}`} aria-label="逐拍照点视图">
      {layout.shots.map((shot, k) => {
        const state = shotState(layout, vis, k);
        const own = shotSegment(layout, k)?.segment;
        const gaps = shotGaps(layout, vis, k);
        const body = (
          <>
            <div className="shot-tile-head">
              <b className="mono" title={`k${k + 1} · Pose ${shot.poseId}`}>{shotLabel(shot,k)} · {shot.bead}</b>
              <span className={shotTone[state]}>{state === "none" && idleLabel ? idleLabel : shotStateLabel[state]}</span>
            </div>
            {state === "skip" ? (
              <div className="shot-tile-empty">不检 · 只要求这一帧到达</div>
            ) : state === "untaught" ? (
              <div className="shot-tile-empty">未示教中线，不能开工</div>
            ) : (
              <ShotLine layout={layout} k={k} vis={vis} selectedPoint={selectedPoint} className="shot-tile-img" />
            )}
            <div className="shot-tile-foot mono">
              {gaps.length ? (
                gaps.map((g) => (
                  <span key={g.s0} className="c-ng">断胶 s={fmt(g.s0)}–{fmt(g.s1)} mm</span>
                ))
              ) : own ? (
                <span>{fmt(segmentLength(own, layout.spacing))} mm · {own.count} 站</span>
              ) : (
                <span>—</span>
              )}
            </div>
          </>
        );
        const cls = `shot-tile st-${state}${selected === k ? " selected" : ""}${current === k ? " current" : ""}`;
        return onSelect ? (
          <button key={k} type="button" className={cls} aria-label={`拍照点 ${shot.id}`} aria-pressed={selected === k} onClick={() => onSelect(k)}>
            {body}
          </button>
        ) : (
          <div key={k} className={cls} role="group" aria-label={`拍照点 ${shot.id}`}>
            {body}
          </div>
        );
      })}
    </div>
  );
}
