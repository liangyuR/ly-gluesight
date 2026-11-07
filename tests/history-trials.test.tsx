import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { HistoryTrials } from "../src/features/workspace/HistoryTrials";
import { workspaceApi } from "../src/features/workspace/api";
import { useWorkspace } from "../src/features/workspace/context";
import type { HistoryCandidate } from "../src/features/workspace/types";
import { burstWorkspaceView, workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { historyCandidates: vi.fn(), compare: vi.fn() } }));

const row: HistoryCandidate = { recordId: 7, cycleId: "cycle-7", ts: 1, sn: "SN-7", mode: "production", groupCount: 12, cameras: [], complete: true,
  reasons: [], warnings: [], durationMs: null, measuredFps: null, setFps: null, importedRound: null, usage: null, fixture: null };

let ws: ReturnType<typeof workspaceState>;
beforeEach(() => {
  ws = workspaceState(burstWorkspaceView());
  vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(workspaceApi.historyCandidates).mockResolvedValue([row]);
});

describe("历史件试测", () => {
  // 防的回归（D-SB-57）：改段后有段没示教走廊时，试测按钮整体禁用，帧数一致的历史件也测不了
  it("部分段示教了走廊即可试测，并说明没示教的段不测；一段都没示教时才禁用", async () => {
    const view = render(<MemoryRouter><HistoryTrials recipeId="S" /></MemoryRouter>);
    expect(await screen.findByRole("button", { name: /^触发测试/ })).toBeEnabled();
    expect(screen.getByText(/走廊示教未完成（CAM-2、CAM-3）：没示教的段不测/)).toBeVisible();
    view.unmount();
    ws.data!.plan.cameraTeaching = ws.data!.plan.cameraTeaching.map(c => ({ ...c, ready: false, segments: c.segments.map(s => ({ ...s, saved: false })) }));
    render(<MemoryRouter><HistoryTrials recipeId="S" /></MemoryRouter>);
    expect(await screen.findByRole("button", { name: /^触发测试/ })).toBeDisabled();
    expect(screen.getByText(/还没有任何一段示教走廊/)).toBeVisible();
  });
});
