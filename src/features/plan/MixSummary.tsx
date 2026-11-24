import type { MixDirection, MixEvidence } from "./types";

export const mixDirection: Record<MixDirection, string> = { yellow: "偏黄", blue: "偏蓝" };

export default function MixSummary({ evidence, diagnostic = false }: { evidence: MixEvidence; diagnostic?: boolean }) {
  const { observation, directions, reason, mmPerPx } = evidence;
  const status = reason ?? (directions.length ? `${diagnostic ? "诊断超限" : "PC 混胶报警"} · ${directions.map(d => mixDirection[d]).join("、")}` : "未见超限");
  return <div className="wp-stack" aria-label="混胶色差">
    <span className={directions.length ? "c-ng" : evidence.incomplete ? "c-warn" : ""}>{evidence.datasetKind === "synthetic" && "合成联调 · "}{status}{!reason && evidence.incomplete && " · 部分不可测"}</span>
    {observation && mmPerPx != null && <details><summary>查看色差量测</summary>
      <div>模型 {evidence.modelId ?? "未知"} · 背景归一偏移 {observation.background.normalizedShift?.toFixed(2) ?? "不可测"}</div>
      {observation.regions.map((r, i) => <div key={i}>{mixDirection[r.direction]} · 可见 {(r.confirmedLengthPx*mmPerPx).toFixed(2)} mm · 跨度 {(r.spanPx*mmPerPx).toFixed(2)} mm · 面积 {(r.areaPx2*mmPerPx*mmPerPx).toFixed(2)} mm²</div>)}
      {evidence.shiftRuns.map((r, i) => <div key={i}>测站连续{mixDirection[r.direction]} · {r.startMm.toFixed(2)}–{r.endMm.toFixed(2)} mm</div>)}
      {observation.stations.length > 1 && <MixCurve stations={observation.stations}/>}
    </details>}
  </div>;
}

function MixCurve({ stations }: { stations: NonNullable<MixEvidence["observation"]>["stations"] }) {
  const start = stations[0].sPx, length = stations.at(-1)!.sPx - start;
  if (length <= 0) return null;
  let move = true;
  const path = stations.map(s => {
    if (s.medianAxial == null) { move = true; return ""; }
    const point = `${move ? "M" : "L"} ${(s.sPx - start) / length * 200} ${24 - 22 * s.medianAxial}`;
    move = false;
    return point;
  }).join(" ");
  return <figure style={{ margin: 0 }}>
    <svg viewBox="0 0 200 48" width="200" height="48" role="img" aria-label="沿胶路各测站轴向偏移中位数">
      <path d="M 0 24 H 200" stroke="currentColor" opacity={.3}/>
      <path d={path} fill="none" stroke="currentColor" strokeWidth={1.5}/>
    </svg>
    <figcaption>测站中位数 · 上方偏蓝，下方偏黄</figcaption>
  </figure>;
}
