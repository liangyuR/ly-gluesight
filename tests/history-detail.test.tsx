import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import HistoryDetailPage from "../src/pages/HistoryDetailPage";
import { historyApi } from "../src/features/history/api";
import { workspaceApi } from "../src/features/workspace/api";
import { useWorkspace } from "../src/features/workspace/context";
import { deferred, partDetail, workspaceState } from "./fixtures";
import type { Comparison, RecordImages } from "../src/features/workspace/types";

vi.mock("../src/features/history/api", () => ({ historyApi: { detail: vi.fn(), recipe: vi.fn() } }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  recordImages: vi.fn(), recordImage: vi.fn(), comparisons: vi.fn(), compare: vi.fn(), compareOriginal: vi.fn(), historyCapture: vi.fn(), runtimeOverview: vi.fn(),
} }));
let ws: ReturnType<typeof workspaceState>;
let detail: ReturnType<typeof partDetail>;
const raw: RecordImages = { historyId: 1, complete: true, message: "原图完整", frames: [0, 1].map(k => ({ k, camera: "CAM-1", view: 1, file: `${k}.png`, ts: 1, available: true, error: null })) };
function element(){return <MemoryRouter initialEntries={["/history/1"]}><Routes><Route path="/history/:id" element={<HistoryDetailPage />} /></Routes></MemoryRouter>;}
function show() { return render(element()); }
function comparison():Comparison{return {id:"compare-1",historyId:1,source:"rules",cycleId:"cycle-1",bundleId:"bundle-A",candidateId:"A",candidateRevision:7,candidateRecipe:ws.data!.layout,
  originalVerdict:"NG_GAP",judgement:{...detail.judgement,verdict:"OK",reason:"候选合格"},measurements:[],createdAt:1};}
beforeEach(() => {
  ws = workspaceState(); ws.data!.workspace.frames.forEach(f => f.saved = true);
  detail = partDetail(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(historyApi.detail).mockResolvedValue(detail); vi.mocked(historyApi.recipe).mockResolvedValue(ws.data!.layout);
  vi.mocked(workspaceApi.recordImages).mockResolvedValue(structuredClone(raw));
  vi.mocked(workspaceApi.recordImage).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.runtimeOverview).mockResolvedValue(null); vi.mocked(workspaceApi.comparisons).mockResolvedValue([]);
  vi.mocked(workspaceApi.historyCapture).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.compare).mockResolvedValue(comparison());
  vi.mocked(workspaceApi.compareOriginal).mockResolvedValue({...comparison(),source:"original"});
});

describe("历史工件复测", () => {
  it("录制失败明确显示原因，保留规则重判但禁止原图复测与示教", async () => {
    detail.recording = { state: "failed", available: false, directory: null, errors: ["原图写入失败：磁盘空间不足"] };
    vi.mocked(workspaceApi.recordImages).mockResolvedValue({ ...raw, complete: false, frames: [], message: "原图写入失败：磁盘空间不足" });
    show(); await screen.findByText("录制失败");
    expect(screen.getAllByRole("status").find(node => node.textContent?.includes("原图写入失败：磁盘空间不足"))).toBeVisible();
    expect(screen.queryByText(/原图完整|完整落盘|已核验落盘/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "按候选规则重判" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "从原图复测整件" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "按原发布包重现" })).toBeDisabled();
    expect(workspaceApi.compareOriginal).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "将此帧用于示教" })).toBeDisabled();
  });

  it("候选含不检拍照点 → 其余点已示教即可从原图复测整件",async()=>{
    ws.data!.workspace.doc.shots[1].skip=true;ws.data!.workspace.frames[1].saved=false;
    show();await screen.findByText(/原图完整/);
    expect(screen.getByRole("button",{name:"从原图复测整件"})).toBeEnabled();
  });

  it("候选修订改变后旧复测结果不能覆盖当前目标",async()=>{
    const pending=deferred<Comparison>();vi.mocked(workspaceApi.compare).mockReturnValueOnce(pending.promise);
    const old=comparison(),page=show();await screen.findByText(/原图完整/);await userEvent.click(screen.getByRole("button",{name:"按候选规则重判"}));
    ws.data!.workspace.revision++;
    page.rerender(element());await act(async()=>pending.resolve(old));
    expect(screen.queryByRole("heading",{name:"规则重判结果"})).not.toBeInTheDocument();expect(screen.queryByText("候选合格")).not.toBeInTheDocument();
  });
});
