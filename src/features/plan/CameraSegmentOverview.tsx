import type { CSSProperties } from "react";
import { verdictLabel } from "../history/meta";
import type { PathSegment, RegionResult } from "./types";
import { countStatusLabel, PLAN_CAMERAS, regionKindLabel, regionSegments, segmentKindLabel, verdictPlanTone } from "./vis";
import "./plan.css";

export interface CameraSegmentOverviewProps {
  /** 计划相机（有序，共用一个触发） */
  cameras: string[];
  segments: PathSegment[];
  /** 判定结果的区域（在线检测、历史）；给出时在段条下方按区域标出结论与帧数 */
  regions?: RegionResult[];
}

/** 段条的列宽：段长都已知按段长，否则 N_exp 都已知按 N_exp，否则等宽；最短不小于最长的 15%，保证弯道段看得见。 */
function segmentWeights(segments: PathSegment[]): number[] {
  const lengths = segments.map(s => s.lengthMm);
  const frames = segments.map(s => s.expectedFrames);
  const raw = lengths.every(v => v != null && v > 0) ? lengths as number[]
    : frames.every(v => v != null && v > 0) ? frames as number[]
      : segments.map(() => 1);
  const max = Math.max(1e-9, ...raw);
  return raw.map(v => Math.max(v, max * 0.15));
}

function segmentAria(s: PathSegment) {
  return `段 ${s.index} · ${segmentKindLabel[s.kind]} · ${s.expectedFrames == null ? "待标定" : `预期 ${s.expectedFrames} ± ${s.tolerance} 帧`}`;
}

/**
 * 工件总览（段模型）：上面是三台计划相机，下面是沿胶路的段条（直线 / 弯道、N_exp 或“待标定”、容差、检测区间）；
 * 给了 regions 时再画一行区域结果，未分段区域横跨它覆盖的段。只做展示，不依赖旧拍照点模型的 WorkpieceOverview。
 */
export function CameraSegmentOverview({ cameras, segments, regions }: CameraSegmentOverviewProps) {
  const columns: CSSProperties = { gridTemplateColumns: segmentWeights(segments).map(w => `minmax(0, ${w}fr)`).join(" ") };
  const column = new Map(segments.map((s, i) => [s.index, i + 1]));
  return <section className="plan-overview" aria-label="相机与段">
    <div className="plan-overview-cams">
      <ul className="plan-cams" aria-label="计划相机">
        {cameras.map((camera, i) => <li key={camera} className="plan-cam plan-mono"><i aria-hidden="true" style={{ background: `var(--cam-${(i % 8) + 1})` }} />{camera}</li>)}
      </ul>
      <span className={cameras.length === PLAN_CAMERAS ? "muted" : "plan-warn"} role={cameras.length === PLAN_CAMERAS ? undefined : "status"}>
        {cameras.length === PLAN_CAMERAS ? "三台相机共用一个触发" : `段配方需要 ${PLAN_CAMERAS} 台相机，当前 ${cameras.length} 台`}
      </span>
    </div>
    {segments.length ? <ol className="plan-segbar" style={columns} aria-label="段序列">
      {segments.map(s => <li key={s.index} className={`plan-seg ${s.kind}` + (s.expectedFrames == null ? " pending" : "")} aria-label={segmentAria(s)}>
        <strong>{s.index} · {segmentKindLabel[s.kind]}</strong>
        <span className="plan-seg-n plan-mono">{s.expectedFrames == null ? "待标定" : `${s.expectedFrames} ± ${s.tolerance} 帧`}</span>
        {(s.lengthMm != null || s.skipHead > 0 || s.skipTail > 0) && <small className="plan-mono">
          {s.lengthMm != null && `${s.lengthMm} mm`}{s.lengthMm != null && (s.skipHead > 0 || s.skipTail > 0) && " · "}{(s.skipHead > 0 || s.skipTail > 0) && `跳过首 ${s.skipHead} / 尾 ${s.skipTail} 帧`}
        </small>}
      </li>)}
    </ol> : <p className="muted">尚无段。</p>}
    {regions?.length ? <ol className="plan-regions" style={columns} aria-label="检测区域">
      {regions.map((r, i) => {
        const [first, last] = regionSegments(r.region.kind);
        const from = column.get(first), to = column.get(last);
        return <li key={i} className={"plan-region " + verdictPlanTone(r.verdict)} data-verdict={r.verdict} style={{ gridColumn: from != null && to != null ? `${from} / ${to + 1}` : "1 / -1" }}>
          <strong>{regionKindLabel(r.region.kind)}</strong>
          <span>{verdictLabel[r.verdict]}</span>
          <small className="plan-mono">{r.frameCount} 帧 · {countStatusLabel(r.countStatus)}</small>
        </li>;
      })}
    </ol> : null}
  </section>;
}
