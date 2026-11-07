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
