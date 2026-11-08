import { useEffect, useMemo, useRef } from "react";
import { Chart, themeColor, type ChartInstance, type ChartOption } from "../../lib/chart";
import type { FrameMetrics, SegmentChecks } from "./types";
import { stationEvidenceText, stationStateText } from "./measurementEvidence";

/** 逐站胶宽的参考线：胶宽下限 / 上限（名义 ∓ 允差）与溢胶上限；检测项没启用的不画。wideWarnOnly 时超上限与溢胶只预警（D-SB-61）。 */
export function widthLimits(checks: SegmentChecks | null | undefined) {
  const width = checks?.width.enabled ? checks.width : null;
  return {
    lower: width ? width.nominalMm - width.tolLower : null,
    upper: width ? width.nominalMm + width.tolUpper : null,
    overflow: checks?.overflow.enabled ? checks.overflow.maxWidthMm : null,
    wideWarnOnly: !!checks?.width.wideWarnOnly,
  };
}

/** 连续同类非精确测站（含两端，0 起）。 */
function evidenceRuns(states: string[]): [number, number, string][] {
  const runs: [number, number, string][] = [];
  states.forEach((state, i) => {
    if (state === "exact") return;
    const last = runs.at(-1);
    if (last && last[1] === i - 1 && last[2] === state) last[1] = i; else runs.push([i, i, state]);
  });
  return runs;
}

/**
 * 一帧的逐站胶宽折线：仅画精确宽度，分别标出无胶、截断和未知区间；叠胶宽上下限与溢胶上限，超限点标红，偏宽只预警时用预警色。
 * 与原图联动：鼠标停在图上给出最近的站（`onStation`），外面选中的站（`station`）在图上显示提示。
 */
export function WidthProfile({ metrics, checks, station, onStation }: {
  metrics: FrameMetrics;
  checks?: SegmentChecks | null;
  station: number | null;
  onStation: (station: number | null) => void;
}) {
  const { widths, spacingMm } = metrics;
  const evidence = metrics.measurementEvidence;
  const limits = widthLimits(checks);
  const option = useMemo<ChartOption>(() => {
    const [primary, ng, warn, muted, line] = [themeColor("--primary", "#4fc3f7"), themeColor("--ng", "#ef5350"), themeColor("--warn", "#ffb74d"), themeColor("--on-surface-variant", "#9aa4ae"), themeColor("--outline-variant", "#3a434c")];
    const wideColor = limits.wideWarnOnly ? warn : ng;
    const outColor = (w: number) => limits.lower != null && w < limits.lower ? ng
      : (limits.upper != null && w > limits.upper) || (limits.overflow != null && w > limits.overflow) ? wideColor : null;
    const reference = (value: number | null, name: string, color: string) => value == null ? [] : [{ yAxis: value, name, lineStyle: { color, type: "dashed" }, label: { formatter: `${name} ${value.toFixed(2)}`, color } }];
    return {
      animation: false,
      grid: { left: 48, right: 88, top: 16, bottom: 36 },
      tooltip: {
        trigger: "axis", axisPointer: { type: "line" },
        formatter: (params: { dataIndex: number }[]) => {
          const i = params[0]?.dataIndex ?? 0, w = widths[i], station = evidence?.stations[i];
          const description = station && station.state !== "exact" ? stationEvidenceText(station) : w == null ? evidence ? "测量不完整" : "无胶" : `胶宽 ${w.toFixed(2)} mm`;
          return `第 ${i + 1} 站 · ${(i * spacingMm).toFixed(1)} mm<br/>${description.replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!)}`;
        },
      },
      xAxis: { type: "value", name: "mm", min: 0, max: Math.max(spacingMm, (widths.length - 1) * spacingMm), axisLabel: { color: muted }, axisLine: { lineStyle: { color: line } }, splitLine: { show: false } },
      yAxis: { type: "value", name: "胶宽 mm", scale: true, axisLabel: { color: muted }, nameTextStyle: { color: muted }, splitLine: { lineStyle: { color: line } } },
      series: [{
        type: "line", name: "胶宽", connectNulls: false, symbolSize: 6, lineStyle: { color: primary }, itemStyle: { color: primary },
        data: widths.map((w, i) => {
          if (evidence && evidence.stations[i]?.state !== "exact") w = null;
          const color = w == null ? null : outColor(w);
          return { value: [i * spacingMm, w], itemStyle: color ? { color } : undefined };
        }),
        markLine: { silent: true, symbol: "none", data: [...reference(limits.lower, "下限", muted), ...reference(limits.upper, "上限", limits.wideWarnOnly ? warn : muted), ...reference(limits.overflow, "溢胶", wideColor)] },
        markArea: {
          silent: true, itemStyle: { color: muted, opacity: .18 }, label: { color: muted, position: "insideTop" },
          data: evidenceRuns(widths.map((w, i) => evidence ? evidence.stations[i]?.state ?? "unknown" : w == null ? "absent" : "exact")).map(([a, b, state]) => [{
            name: state === "absent" ? "断胶" : stationStateText[state as "clipped" | "unknown"],
            itemStyle: { color: state === "clipped" ? warn : muted, opacity: state === "unknown" ? .3 : .18 },
            xAxis: Math.max(0, (a - .5) * spacingMm),
          }, { xAxis: (b + .5) * spacingMm }]),
        },
      }],
    };
  }, [widths, spacingMm, evidence, limits.lower, limits.upper, limits.overflow, limits.wideWarnOnly]);
  const chart = useRef<ChartInstance | null>(null);
  const report = useRef(onStation);
  report.current = onStation;
  // 图只初始化一次，事件里按当前这一帧的站数与站距换算
  const shape = useRef({ count: widths.length, spacingMm });
  shape.current = { count: widths.length, spacingMm };
  useEffect(() => {
    const c = chart.current;
    if (!c) return;
    try { c.dispatchAction(station == null ? { type: "hideTip" } : { type: "showTip", seriesIndex: 0, dataIndex: station }); } catch { /* 图还没画好 */ }
  }, [station]);
  return <Chart label="逐站胶宽" option={option} onReady={c => {
    chart.current = c;
    const zr = c.getZr();
    const move = (e: { offsetX: number; offsetY: number }) => {
      const at: [number, number] = [e.offsetX, e.offsetY];
      if (!c.containPixel("grid", at)) { report.current(null); return; }
      const [mm] = c.convertFromPixel("grid", at) as number[];
      const { count, spacingMm: spacing } = shape.current;
      report.current(count ? Math.min(count - 1, Math.max(0, Math.round(mm / spacing))) : null);
    };
    const leave = () => report.current(null);
    zr.on("mousemove", move);
    zr.on("globalout", leave);
    return () => { zr.off("mousemove", move); zr.off("globalout", leave); chart.current = null; };
  }} />;
}
