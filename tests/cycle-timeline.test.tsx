import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { expect, it, vi } from "vitest";
import CycleTimeline from "../src/features/logs/CycleTimeline";
import { logsApi } from "../src/features/logs/api";
import type { Timeline, TimelineEntry } from "../src/features/logs/types";

vi.mock("../src/features/logs/api", async importOriginal => ({ ...(await importOriginal<typeof import("../src/features/logs/api")>()), logsApi: { timeline: vi.fn() } }));

const entry = (ts: number, ev: string, camera: string | null, seq: number | null): TimelineEntry =>
  ({ ts, source: "app", level: "info", ev, msg: "", stage: null, k: null, shotId: null, camera, seq, data: null });

it("段配方选中一帧 → 勾选只看选中帧后只留该相机该帧组的事件，点事件的相机 / 帧组按钮回调选帧", async () => {
  const timeline: Timeline = { cycleId: "cycle-1", requestSeq: 1, start: 0, end: 100, stages: [], truncated: false, notes: [],
    entries: [entry(10, "frame.arrived", "CAM-1", 3), entry(20, "frame.arrived", "CAM-2", 3), entry(30, "frame.measured", "CAM-1", 4), entry(40, "part.end", null, null)] };
  vi.mocked(logsApi.timeline).mockResolvedValue(timeline);
  const onSelect = vi.fn();
  render(<MemoryRouter><CycleTimeline historyId={1} cycleId="cycle-1" ts={0} frame={{ camera: "CAM-1", seq: 3, onSelect }} /></MemoryRouter>);
  const table = await screen.findByRole("table", { name: "本件事件" });
  expect(within(table).getAllByRole("row")).toHaveLength(1 + 4);
  await userEvent.click(screen.getByRole("checkbox", { name: "只看选中帧 CAM-1 #3" }));
  const rows = within(table).getAllByRole("row").slice(1);
  expect(rows).toHaveLength(1);
  expect(rows[0]).toHaveTextContent("frame.arrived");
  await userEvent.click(within(rows[0]).getByRole("button", { name: "查看 CAM-1 帧组 3" }));
  expect(onSelect).toHaveBeenCalledWith("CAM-1", 3);
});
