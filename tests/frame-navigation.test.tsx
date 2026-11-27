import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import { FrameGrid } from "../src/features/workspace/FrameGrid";
import FrameGroupStrip from "../src/features/cycle/components/FrameGroupStrip";
import { BurstPartTrace } from "../src/features/history/components/PartTrace";
import type { FrameGroupView } from "../src/features/plan/types";
import { burstPartDetail, frameMetrics } from "./fixtures";

const cameras = ["CAM-1", "CAM-2", "CAM-3"];

it.each(["示教", "历史"])("%s的1000帧组 → 跳到末帧并跨窗口键盘选择，焦点和全局帧号保持正确且DOM有界", async kind => {
  const groups: FrameGroupView[] = Array.from({ length: 1000 }, (_, i) => ({ seq: i + 1, incomplete: false, cells: cameras.map(() => ({ status: "measured", verdict: "OK", frameCounter: i + 1 })) }));
  function Browser() {
    const [selected, setSelected] = useState({ camera: 0, seq: 1 });
    const select = (camera: number, seq: number) => setSelected({ camera, seq });
    return <><output aria-label="当前帧">{selected.camera}:{selected.seq}</output>
      {kind === "示教"
        ? <FrameGrid label="浏览" cameras={cameras.map(id => ({ id, name: id }))} groupCount={1000} {...selected} onSelect={select} />
        : <FrameGroupStrip label="浏览" cameras={cameras} groups={groups} selected={selected.seq} selectedCamera={selected.camera} onSelectCell={(seq, camera) => select(camera, seq)} />}</>;
  }
  const view = render(<Browser />), user = userEvent.setup();
  const browser = within(screen.getByRole("group", { name: "浏览" }));
  await user.click(browser.getByRole("button", { name: kind === "示教" ? "帧组 1 · CAM-1" : "帧组 1 · CAM-1 · OK" }));
  await user.keyboard("{End}{ArrowDown}{ArrowLeft>32/}");
  expect(screen.getByLabelText("当前帧")).toHaveTextContent("1:968");
  const active = browser.getByRole("button", { name: kind === "示教" ? "帧组 968 · CAM-2" : "帧组 968 · CAM-2 · OK" });
  expect(active).toHaveFocus();
  expect(active).toHaveAttribute("aria-pressed", "true");
  expect(view.container.querySelectorAll(".fg-cell-button").length).toBeLessThan(150);
  await user.keyboard("{Home}");
  expect(screen.getByLabelText("当前帧")).toHaveTextContent("1:1");
});

it("3000条追溯记录分页并切换筛选 → 保留选中帧，恢复筛选后定位该页且每页不超过配置数量", async () => {
  const detail = burstPartDetail(), burst = detail.burst!;
  burst.frames = cameras.flatMap((_, camera) => Array.from({ length: 1000 }, (_, i) => ({ camera, seq: i + 1, status: i === 50 ? "error" as const : "measured" as const, frameCounter: i + 1, triggerCounter: i + 1, region: 0, metrics: frameMetrics(camera, i + 1), rawFile: "frame.pgm" })));
  function Trace() {
    const [selected, setSelected] = useState<{ camera: number; seq: number } | null>(null);
    return <><output aria-label="当前帧">{selected ? `${selected.camera}:${selected.seq}` : "无"}</output><BurstPartTrace detail={detail} burst={burst} selected={selected} onSelect={(camera, seq) => setSelected({ camera, seq })} /></>;
  }
  render(<Trace />);
  const user = userEvent.setup(), table = within(screen.getByRole("table", { name: "逐帧追溯" }));
  expect(table.getAllByRole("row")).toHaveLength(51);
  await user.click(screen.getByRole("button", { name: "下一页帧记录" }));
  await user.click(table.getByRole("button", { name: "追溯 CAM-1 帧组 51" }));
  await user.selectOptions(screen.getByLabelText("相机筛选", { selector: "select" }), "1");
  expect(screen.getByLabelText("当前帧")).toHaveTextContent("0:51");
  await user.selectOptions(screen.getByLabelText("相机筛选", { selector: "select" }), "all");
  expect(table.getByRole("button", { name: "追溯 CAM-1 帧组 51" })).toHaveAttribute("aria-pressed", "true");
  await user.click(screen.getByRole("checkbox", { name: "只看异常帧（3）" }));
  expect(table.getAllByRole("row")).toHaveLength(4);
  expect(table.getByRole("button", { name: "追溯 CAM-1 帧组 51" })).toHaveAttribute("aria-pressed", "true");
  await user.click(screen.getByRole("checkbox", { name: "只看异常帧（3）" }));
  await user.selectOptions(screen.getByLabelText("每页帧数", { selector: "select" }), "100");
  expect(table.getAllByRole("row")).toHaveLength(101);
  expect(table.getByRole("button", { name: "追溯 CAM-1 帧组 51" })).toHaveAttribute("aria-pressed", "true");
});

it("追溯只看异常帧 → 逐帧结论和诊断均可找到，同相机同帧组只计一次", async () => {
  const detail = burstPartDetail(), burst = detail.burst!;
  const region = burst.judgement.regions[0];
  burst.judgement.regions = [
    { ...region, region: { ...region.region, lastSeq: 5 }, verdict: "NG_WIDTH" },
    { ...region, region: { firstSeq: 6, lastSeq: 7, kind: { type: "skipped", segment: 1 } }, frameCount: 0, incompleteGroups: [] },
  ];
  burst.frames = burst.frames.filter(frame => (frame.camera === 0 && frame.seq <= 7) ||
    (frame.camera === 1 && (frame.seq === 1 || frame.seq === 5))).map(frame => {
    if (frame.seq === 4 || frame.seq === 7) return { ...frame, region: frame.seq === 7 ? 1 : 0, metrics: frameMetrics(frame.camera, frame.seq, {
      measurementEvidence: { version: 1, stations: [], exactStations: 2, clippedStations: 1, unknownStations: 0, absentStations: 0, presentRatio: 1 },
      mixEvidence: { modelId: null, mmPerPx: null, incomplete: false, reason: null, directions: ["yellow"], shiftRuns: [], observation: null },
    }) };
    return frame.seq === 6 ? { ...frame, region: 1 } : frame;
  });
  burst.frameVerdicts = [
    { camera: 0, seq: 1, verdict: "NG_WIDTH" },
    { camera: 0, seq: 2, verdict: "OK_WITH_EXCURSION" },
    { camera: 0, seq: 3, verdict: "ERR_INSPECT" },
    { camera: 0, seq: 4, verdict: "OK" },
    { camera: 1, seq: 1, verdict: "OK" },
  ];

  render(<BurstPartTrace detail={detail} burst={burst} selected={null} onSelect={() => {}} />);
  const table = within(screen.getByRole("table", { name: "逐帧追溯" }));
  const row = (name: string) => table.getByRole("button", { name: `追溯 ${name}` }).closest("tr");
  expect(row("CAM-1 帧组 6")).toHaveTextContent("生产跳过");
  const filter = screen.getByRole("checkbox", { name: /只看异常帧/ });
  await userEvent.click(filter);

  expect(filter).toHaveAccessibleName("只看异常帧（6）");
  expect(table.getAllByRole("row")).toHaveLength(7);
  expect(row("CAM-1 帧组 1")).toHaveTextContent("NG · 胶宽超差");
  expect(row("CAM-1 帧组 2")).toHaveTextContent("OK · 局部超差");
  expect(row("CAM-1 帧组 3")).toHaveTextContent("ERR · 未测成");
  expect(row("CAM-1 帧组 4")).toHaveTextContent("超出测量范围 1 站");
  expect(row("CAM-1 帧组 4")).toHaveTextContent("PC 混胶报警 · 偏黄");
  expect(row("CAM-2 帧组 5")).toHaveTextContent("缺帧");
  expect(row("CAM-1 帧组 7")).toHaveTextContent("生产跳过");
  expect(row("CAM-1 帧组 7")).toHaveTextContent("诊断超限 · 偏黄");
  expect(table.queryByRole("button", { name: "追溯 CAM-2 帧组 1" })).not.toBeInTheDocument();
  expect(table.queryByRole("button", { name: "追溯 CAM-1 帧组 5" })).not.toBeInTheDocument();
  expect(table.queryByRole("button", { name: "追溯 CAM-1 帧组 6" })).not.toBeInTheDocument();
});
