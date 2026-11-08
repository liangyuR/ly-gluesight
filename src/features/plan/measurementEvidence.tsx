import type { FrameMetrics, MeasurementEvidence, StationEvidence } from "./types";

export const stationStateText = { exact: "已完整量宽", clipped: "超出测量范围", absent: "无胶", unknown: "测量不完整" };
const reasonText: Record<string, string> = { observation_boundary: "胶延伸到观察边界", occluded: "遮挡或颜色证据不足", ambiguous: "多条候选胶路，归属不明确", path_untrusted: "无法确定可信胶路", out_of_corridor: "胶条侧向走出走廊" };
const displayReason = (reason: string) => reasonText[reason] ?? reason;

export function measurementIssue(evidence: MeasurementEvidence | undefined) {
  return evidence?.clippedStations ? "clipped" : evidence?.unknownStations ? "unknown" : null;
}

export function presentRatioText(metrics: FrameMetrics) {
  const ratio = metrics.measurementEvidence ? metrics.measurementEvidence.presentRatio : metrics.presentRatio;
  return ratio == null ? "不可用" : `${Math.round(ratio * 100)}%`;
}

export function stationEvidenceText(station: StationEvidence) {
  return `${stationStateText[station.state]}${station.state === "clipped" && station.widthLowerBoundMm != null ? ` · ≥ ${station.widthLowerBoundMm.toFixed(2)} mm` : ""}${station.reason ? ` · ${displayReason(station.reason)}` : ""}`;
}

export function evidenceOverlay(metrics: FrameMetrics | null | undefined) {
  return (metrics?.measurementEvidence?.stations ?? []).flatMap((station, index) => {
    if (station.state !== "clipped") return [];
    return [{ station: index, loEdge: station.loEdge, hiEdge: station.hiEdge, normal: station.normal, position: station.position,
      loClipped: station.loClipped, hiClipped: station.hiClipped, label: stationEvidenceText(station) }];
  });
}

export function MeasurementEvidenceSummary({ evidence }: { evidence: MeasurementEvidence | undefined }) {
  if (!evidence) return null;
  const count = evidence.exactStations + evidence.clippedStations + evidence.unknownStations + evidence.absentStations;
  const reasons = [...new Set(evidence.stations.filter(s => s.state === "unknown" || s.state === "clipped").flatMap(s => s.reason ? [s.reason] : []))];
  return <span aria-label="量测证据" className={measurementIssue(evidence) ? "c-warn" : "muted"}>
    精确量宽 {evidence.exactStations}/{count} 站
    {evidence.clippedStations > 0 && <> · 超出测量范围 {evidence.clippedStations} 站{evidence.maxWidthLowerBoundMm != null && `（最大可靠下界 ≥ ${evidence.maxWidthLowerBoundMm.toFixed(2)} mm）`}</>}
    {evidence.unknownStations > 0 && <> · 测量不完整 {evidence.unknownStations} 站</>}
    {evidence.absentStations > 0 && <> · 确认无胶 {evidence.absentStations} 站</>}
    {!!measurementIssue(evidence) && <> · 胶宽统计仅含完整测站</>}
    {reasons.length > 0 && <> · {reasons.map(displayReason).join("；")}</>}
  </span>;
}
