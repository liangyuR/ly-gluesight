import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { CalibrationFrames } from "../src/features/workspace/CalibrationFrames";
import { SegmentTrial } from "../src/features/workspace/TeachTrial";
import { workspaceApi } from "../src/features/workspace/api";
import { burstWorkspaceView, frameMetrics } from "./fixtures";

vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { calibrationRoundOverview: vi.fn(), calibrationFrame: vi.fn(), captureFrameImage: vi.fn(), teachPreviewFrame: vi.fn(), teachPreviewSegment: vi.fn() } }));
const picture = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
const view = (camera: number, seq: number) => ({ segment: 1, corridor: [] as [number, number][], searchHalfPx: 5, edges: [], partEdges: [], metrics: frameMetrics(camera, seq, { widthMean: seq / 100 }) });

it("1000帧的测量结果 → Home/End跨窗口与方向键选择保持焦点及正确帧指标，状态格有界", async () => {
  const data = burstWorkspaceView();
  data.plan.pieces[0].groupCount = 1000;
  data.plan.calibration!.roundId = data.plan.pieces[0].roundId;
  vi.mocked(workspaceApi.calibrationRoundOverview).mockResolvedValue(data.plan.cameras.flatMap((_, camera) => Array.from({ length: 1000 }, (_, i) => frameMetrics(camera, i + 1))));
  vi.mocked(workspaceApi.captureFrameImage).mockResolvedValue(picture);
  vi.mocked(workspaceApi.calibrationFrame).mockImplementation(async (_id, _round, camera, seq) => view(data.plan.cameras.indexOf(camera), seq));
  render(<CalibrationFrames data={data} plan={data.plan} />);
  const user = userEvent.setup();
  const grid = screen.getByRole("grid", { name: "逐帧状态" }), cells = within(grid);
  await user.click(await cells.findByLabelText("CAM-1 帧组 1：OK"));
  await user.keyboard("{End}{ArrowDown}");
  expect(cells.getByLabelText("CAM-2 帧组 1000：OK")).toHaveFocus();
  expect(await screen.findByText("3.80 mm / 10.00 mm / 4.20 mm")).toBeVisible();
  await user.keyboard("{Home}{ArrowRight}");
  const last = cells.getByLabelText("CAM-2 帧组 2：OK");
  expect(last).toHaveFocus();
  expect(last).toHaveAttribute("aria-selected", "true");
  expect(await screen.findByText("3.80 mm / 0.02 mm / 4.20 mm")).toBeVisible();
  expect(grid.querySelectorAll('[role="gridcell"]').length).toBeLessThan(210);
});

it("试测段从501开始且有1000帧 → Home/End跨窗口与方向键选择保持原始帧号、焦点和指标", async () => {
  vi.mocked(workspaceApi.captureFrameImage).mockResolvedValue(picture);
  vi.mocked(workspaceApi.teachPreviewFrame).mockImplementation(async (_id, _camera, _segment, _round, seq) => view(0, seq));
  vi.mocked(workspaceApi.teachPreviewSegment).mockResolvedValue({ round: "round-1", start: 501, end: 1500, frames: Array.from({ length: 1000 }, (_, i) => frameMetrics(0, i + 501, { presentRatio: i === 999 ? 0 : 1 })) });
  function Trial() {
    const [seq, setSeq] = useState<number | null>(null);
    return <SegmentTrial id="A" camera="CAM-1" segment={2} round="round-1" teachSeq={501} range={{ start: 501, end: 1500 }} revision={3} locked="" guide={null} seq={seq} onSeq={setSeq} />;
  }
  render(<Trial />);
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "全段试测" }));
  const stripElement = screen.getByRole("group", { name: "逐帧试测状态" }), strip = within(stripElement);
  await user.click(await strip.findByLabelText("帧组 501：测到（示教帧）"));
  await user.keyboard("{End}");
  expect(strip.getByLabelText("帧组 1500：无胶")).toHaveFocus();
  expect(await screen.findByText("15.00 mm")).toBeVisible();
  await user.keyboard("{Home}{ArrowRight}");
  expect(strip.getByLabelText("帧组 502：测到")).toHaveFocus();
  expect(await screen.findByText("5.02 mm")).toBeVisible();
  expect(stripElement.querySelectorAll("button").length).toBeLessThan(110);
});
