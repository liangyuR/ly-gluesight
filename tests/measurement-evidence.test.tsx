import { render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { WidthProfile } from "../src/features/plan/WidthProfile";
import { BurstPartTrace } from "../src/features/history/components/PartTrace";
import { BurstRuntimeFrame } from "../src/features/workspace/RuntimeFrame";
import { burstImageApi } from "../src/features/workspace/burstImages";
import type { MeasurementEvidence } from "../src/features/plan/types";
import { burstPartDetail, frameMetrics } from "./fixtures";
import { burstPartView } from "./cycle-visual-fixtures";

const chart = vi.hoisted(() => ({ option: null as any }));
vi.mock("../src/lib/chart", () => ({ themeColor: (_name: string, fallback: string) => fallback, Chart: ({ option }: any) => { chart.option = option; return <div />; } }));
vi.mock("../src/features/workspace/burstImages", () => ({ burstImageApi: { live: vi.fn(), liveView: vi.fn() } }));

it("精确、截断、未知与无胶混合 → 下界不画精确点，只有无胶标断胶；旧记录仍按原语义显示", () => {
  const evidence: MeasurementEvidence = { version: 1, exactStations: 1, clippedStations: 1, unknownStations: 1, absentStations: 1, presentRatio: 2 / 3,
    stations: [{ state: "exact" }, { state: "clipped", widthLowerBoundMm: 11 }, { state: "unknown", reason: "路径不确定" }, { state: "absent" }] };
  const metrics = frameMetrics(0, 1, { widths: [12, null, null, null], measurementEvidence: evidence });
  const { rerender } = render(<WidthProfile metrics={metrics} station={null} onStation={() => {}} />);
  expect(chart.option.series[0].data.map((point: any) => point.value[1])).toEqual([12, null, null, null]);
  expect(chart.option.series[0].markArea.data.map((area: any) => area[0].name)).toEqual(["超出测量范围", "测量不完整", "断胶"]);
  expect(chart.option.tooltip.formatter([{ dataIndex: 1 }])).toContain("≥ 11.00 mm");
  expect(chart.option.tooltip.formatter([{ dataIndex: 2 }])).toContain("路径不确定");
  rerender(<WidthProfile metrics={{ ...metrics, measurementEvidence: undefined }} station={null} onStation={() => {}} />);
  expect(chart.option.series[0].markArea.data.map((area: any) => area[0].name)).toEqual(["断胶"]);
});

it("历史总览裁剪逐站数组 → 仍保留截断、未知和精确量宽覆盖摘要", () => {
  const detail = burstPartDetail(), burst = detail.burst!;
  burst.frames = [{ ...burst.frames[0], metrics: frameMetrics(0, 1, { widths: [], measurementEvidence: { version: 1, stations: [], exactStations: 5, clippedStations: 2, unknownStations: 1, absentStations: 0, maxWidthLowerBoundMm: 11, presentRatio: 1 } }) }];
  render(<BurstPartTrace detail={detail} burst={burst} selected={null} onSelect={() => {}} />);
  const summary = screen.getByLabelText("量测证据");
  expect(summary).toHaveTextContent("精确量宽 5/8 站");
  expect(summary).toHaveTextContent("超出测量范围 2 站（最大可靠下界 ≥ 11.00 mm）");
  expect(summary).toHaveTextContent("测量不完整 1 站");
  expect(summary).toHaveTextContent("胶宽统计仅含完整测站");
});

it("在线下界超宽且量测截断 → 保留NG结论，展示下界与可见边，不伪造完整测量线", async () => {
  const part = burstPartView(), group = part.burst.groups[0];
  part.burst.cameras = ["CAM-1"];
  group.cells[0].verdict = "NG_WIDTH";
  vi.mocked(burstImageApi.live).mockResolvedValue({ url: "data:,", width: 100, height: 60 });
  vi.mocked(burstImageApi.liveView).mockResolvedValue({ segment: 1, corridor: [[10, 10], [40, 10]], searchHalfPx: 5, edges: [null], partEdges: [], metrics: frameMetrics(0, 1, {
    widthMin: null, widthMax: null, widthMean: null, widths: [null], measurementEvidence: { version: 1, exactStations: 0, clippedStations: 1, absentStations: 0, unknownStations: 0, presentRatio: 1, maxWidthLowerBoundMm: 11,
      stations: [{ state: "clipped", widthLowerBoundMm: 11, loEdge: [10, 10], hiClipped: true, normal: [0, 1], position: [10, 12] }] },
  }) });
  render(<BurstRuntimeFrame part={part} seq={1} />);
  expect(await screen.findByLabelText("量测证据")).toHaveTextContent("最大可靠下界 ≥ 11.00 mm");
  expect(screen.getByText("NG · 胶宽超差")).toBeVisible();
  const overlay = screen.getByLabelText("截断测站");
  expect(overlay.querySelectorAll("circle")).toHaveLength(1);
  expect(overlay.querySelectorAll("path")).toHaveLength(1);
  expect(screen.getByLabelText("测量横线").querySelectorAll("line")).toHaveLength(0);
});
