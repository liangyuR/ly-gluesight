import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useSiteStatus } from "../src/features/cycle/api";
import ValidationPage from "../src/features/workspace/ValidationPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { historyApi } from "../src/features/history/api";
import { projectApi } from "../src/lib/project";
import { burstPartDetail, burstPartSummary, burstRecipe, burstWorkspaceView, siteStatus, workspaceState } from "./fixtures";
vi.mock("../src/features/cycle/api", () => ({ useSiteStatus: vi.fn() }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { publish: vi.fn(), retryPublish: vi.fn(), abandonPublish: vi.fn(), collectChecks: vi.fn(), publishCollect: vi.fn(), publishChecks: vi.fn() } }));
vi.mock("../src/features/history/api", () => ({ historyApi: { query: vi.fn(), detail: vi.fn() } }));
vi.mock("../src/lib/project", () => ({ projectApi: { current: vi.fn(), list: vi.fn() } }));
let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter><ValidationPage /></MemoryRouter>);
beforeEach(() => {
  vi.mocked(useSiteStatus).mockReturnValue(siteStatus());
  const view = burstWorkspaceView(burstRecipe({ id: "A" }));
  view.workspace.revision = 7;
  view.plan!.segments.forEach(segment => { segment.expectedFrames = 4; });
  view.workspace.validation = { revision: 7, passed: true, checkedAt: 1, environmentTag: {}, checks: [], samples: [] };
  view.workspace.sampleBank = [{ id: "good", name: "良品组", geometryTag: null, expected: "OK", createdAt: 1 }];
  view.workspace.samples = [{ historyId: null, sampleId: "good", expected: "OK" }];
  ws = workspaceState(view); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(historyApi.query).mockResolvedValue({ items: [], total: 0, counts: { ok: 0, ng: 0, err: 0, excursion: 0 } });
  vi.mocked(projectApi.current).mockResolvedValue({ schema: 1, id: "line-a", name: "A 线", description: "", createdAt: 0, createdBy: "", copiedFrom: null });
  vi.mocked(projectApi.list).mockResolvedValue([]);
  vi.mocked(workspaceApi.publish).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.publishChecks).mockResolvedValue([{ name: "相机配置", passed: true, detail: "三台计划相机已配置" }]);
  vi.mocked(workspaceApi.retryPublish).mockResolvedValue(ws.data!);
});

describe("验证与发布页面", () => {

  // 防的回归：记录库本机共用、配方编号只在项目内唯一，历史样本查询不带项目时混入其他项目（含同编号配方）的记录
  it("列历史样本 → 查询带当前项目编号", async () => {
    show();
    await waitFor(() => expect(historyApi.query).toHaveBeenCalledWith(expect.objectContaining({ project: "line-a" })));
  });

  // 防的回归：复制来的项目沿用来源项目的历史记录作验证样本，查询只列当前项目时这些已选样本看不到，也取消不了
  it("已选的历史样本不在当前项目的列表里 → 单独读出并显示为已选", async () => {
    ws.data!.workspace.samples = [{ historyId: 42, sampleId: null, expected: "OK" }];
    vi.mocked(historyApi.detail).mockResolvedValue({ ...burstPartDetail(), summary: { ...burstPartSummary(42), sn: "SN-SRC", projectId: "line-src" } });
    show();
    expect(await screen.findByRole("checkbox", { name: "选用历史 SN SN-SRC" })).toBeChecked();
    expect(historyApi.detail).toHaveBeenCalledWith(42);
  });

  it("发布失败后冻结样本配置并重试同一待生效版本", async () => {
    ws.data!.workspace.pending = { doc: ws.doc!, bundleId: "bundle-v2", revision: 7, baseRevision: "old", validation: ws.data!.workspace.validation!, validationSite: "real" };
    ws.data!.workspace.publish = { state: "failed", message: "发布包缺失" };
    show();
    expect(screen.getByRole("checkbox", { name: "选用样本 良品组" })).toBeDisabled();
    expect(screen.getByRole("combobox", { name: "良品组期望结论" })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: "已确认所选样本的拍摄条件、轨迹和触发顺序适用于当前候选" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "运行整套图像验证" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "重试发布 / 切换" }));
    expect(workspaceApi.retryPublish).toHaveBeenCalledWith("A", 7);
    expect(workspaceApi.publish).not.toHaveBeenCalled();
  });

  it("生效失败 → 确认后放弃同一修订的待生效版本；状态不确定时不能放弃", async () => {
    vi.mocked(workspaceApi.abandonPublish).mockResolvedValue(ws.data!);
    ws.data!.workspace.pending = { doc: ws.doc!, bundleId: "bundle-v2", revision: 7, baseRevision: "old", validation: ws.data!.workspace.validation!, validationSite: "real" };
    ws.data!.workspace.publish = { state: "failed", message: "发布包缺失" };
    const page = show();
    await userEvent.click(screen.getByRole("button", { name: "放弃待生效版本" }));
    expect(workspaceApi.abandonPublish).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: /确认放弃/ }));
    expect(workspaceApi.abandonPublish).toHaveBeenCalledWith("A", 7);
    page.unmount();
    ws.data!.workspace.publish = { state: "uncertain", message: "写盘失败" };
    show();
    expect(screen.queryByRole("button", { name: "放弃待生效版本" })).not.toBeInTheDocument();
  });

  it("发布采集版 → 逐条列出未满足项且不能确认；全部满足后确认发布同一修订", async () => {
    vi.mocked(workspaceApi.publishCollect).mockResolvedValue(ws.data!);
    vi.mocked(workspaceApi.collectChecks).mockResolvedValueOnce([
      { name: "计划相机", passed: true, detail: "cam1、cam2、cam3 共用一个触发" },
      { name: "相机组", passed: false, detail: "相机组里没有 cam2" },
      { name: "生产版本", passed: false, detail: "已有正式版 v3" },
    ]);
    show();
    await userEvent.click(screen.getByRole("button", { name: "发布采集版" }));
    let dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("采集版只在维护模式下运行，只采图不判定");
    expect(dialog).toHaveTextContent("2 项未满足");
    expect(dialog).toHaveTextContent("相机组里没有 cam2");
    expect(dialog).toHaveTextContent("已有正式版 v3");
    expect(within(dialog).getByRole("button", { name: /确认发布采集版/ })).toBeDisabled();
    await userEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    vi.mocked(workspaceApi.collectChecks).mockResolvedValueOnce([{ name: "计划相机", passed: true, detail: "cam1、cam2、cam3 共用一个触发" }]);
    await userEvent.click(screen.getByRole("button", { name: "发布采集版" }));
    dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: /确认发布采集版/ }));
    expect(workspaceApi.publishCollect).toHaveBeenCalledWith("A", 7);
    expect(workspaceApi.publish).not.toHaveBeenCalled();
  });

  // 防的回归（D-SB-46）：发布仍要求先通过独立样本验证；或发布前检查有未满足项（如走廊未示教）时仍能发布
  it("没有样本验证结果、发布前检查全部满足 → 可以发布；检查有未满足项 → 发布不可用并说明原因", async () => {
    ws.data!.workspace.validation = null;
    ws.data!.workspace.samples = [];
    const page = show();
    await waitFor(() => expect(screen.getByRole("button", { name: "发布生产配方" })).toBeEnabled());
    page.unmount();
    vi.mocked(workspaceApi.publishChecks).mockResolvedValue([{ name: "走廊示教", passed: false, detail: "第 2 段相机 cam2 走廊未示教" }]);
    show();
    await waitFor(() => expect(screen.getAllByRole("note").some(note => note.textContent?.includes("走廊示教：第 2 段相机 cam2 走廊未示教"))).toBe(true));
    expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
  });

  // 防的回归（D-SB-59）：示教帧上部分段没测成只是提醒，却把发布拦住；或发布确认时不再提醒这些段生产不测
  it("发布前检查只有提醒项未满足 → 仍可发布，确认框列出不测的段", async () => {
    vi.mocked(workspaceApi.publishChecks).mockResolvedValue([{ name: "示教帧试测", passed: false, advisory: true, detail: "第 2 段相机 cam1：runs：n_map（bad_param）" }]);
    show();
    await waitFor(() => expect(screen.getByRole("button", { name: "发布生产配方" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "发布生产配方" }));
    expect(await screen.findByRole("dialog")).toHaveTextContent("第 2 段相机 cam1：runs：n_map（bad_param）");
  });

  it("确认期间候选修订变更必须重新核对发布前检查和确认",async()=>{
    const page=show();await waitFor(()=>expect(screen.getByRole("button",{name:"发布生产配方"})).toBeEnabled());await userEvent.click(screen.getByRole("button",{name:"发布生产配方"}));
    ws.data!.workspace.revision++;page.rerender(<MemoryRouter><ValidationPage/></MemoryRouter>);
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(()=>expect(workspaceApi.publishChecks).toHaveBeenLastCalledWith("A",8));
    expect(workspaceApi.publish).not.toHaveBeenCalled();
  });
});
