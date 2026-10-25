import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useSiteStatus } from "../src/features/cycle/api";
import ValidationPage from "../src/features/workspace/ValidationPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { historyApi } from "../src/features/history/api";
import { burstRecipe, burstWorkspaceView, planExport, siteStatus, workspaceState } from "./fixtures";
vi.mock("../src/features/cycle/api", () => ({ useSiteStatus: vi.fn() }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { publish: vi.fn(), retryPublish: vi.fn(), abandonPublish: vi.fn(), collectChecks: vi.fn(), publishCollect: vi.fn() } }));
vi.mock("../src/features/history/api", () => ({ historyApi: { query: vi.fn() } }));
let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter><ValidationPage /></MemoryRouter>);
beforeEach(() => {
  vi.mocked(useSiteStatus).mockReturnValue(siteStatus());
  const view = burstWorkspaceView(burstRecipe({ id: "A" }));
  view.workspace.revision = 7;
  view.workspace.validation = { revision: 7, passed: true, checkedAt: 1, environmentTag: {}, checks: [], samples: [] };
  view.workspace.sampleBank = [{ id: "good", name: "良品组", geometryTag: null, expected: "OK", createdAt: 1 }];
  view.workspace.samples = [{ historyId: null, sampleId: "good", expected: "OK" }];
  ws = workspaceState(view); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(historyApi.query).mockResolvedValue({ items: [], total: 0, counts: { ok: 0, ng: 0, err: 0, excursion: 0 } });
  vi.mocked(workspaceApi.publish).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.retryPublish).mockResolvedValue(ws.data!);
});

describe("验证与发布页面", () => {

  it("发布失败后冻结样本配置并重试同一待生效版本", async () => {
    ws.data!.workspace.pending = { doc: ws.doc!, bundleId: "bundle-v2", revision: 7, baseRevision: "old", frames: [], overview: ws.data!.workspace.overview, validation: ws.data!.workspace.validation!, validationSite: "real" };
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
    ws.data!.workspace.pending = { doc: ws.doc!, bundleId: "bundle-v2", revision: 7, baseRevision: "old", frames: [], overview: ws.data!.workspace.overview, validation: ws.data!.workspace.validation!, validationSite: "real" };
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

  describe("段配方", () => {
    /** 验证已通过的段配方候选；partCalib 决定整件 ΣN_exp 是否已标定（各段 N_exp 都待分段算法）。 */
    const burst = (over: Parameters<typeof burstRecipe>[0] = {}) => {
      const view = burstWorkspaceView(burstRecipe(over));
      view.workspace.validation = { revision: view.workspace.revision, passed: true, checkedAt: 1, environmentTag: {}, checks: [], samples: [] };
      return view;
    };

    it("整件预期总帧数未标定 → 发布不可用并列出待标定的段号", () => {
      ws = workspaceState(burst({ partCalib: null }));
      show();
      expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
      expect(screen.getAllByRole("note").some(note => note.textContent?.includes("段 1、段 2、段 3 待标定"))).toBe(true);
    });

    it("整件已标定、各段预期帧数待分段算法 → 发布可用，不因段待标定而禁用", () => {
      ws = workspaceState(burst());
      show();
      expect(screen.getByRole("button", { name: "发布生产配方" })).toBeEnabled();
    });

    it("整件预期总帧数 12 → 14 → 发布确认对比列出生产值，不再要求 PLC 同步计划与帧数", async () => {
      const view = burst({ planVersion: 3 });
      const plan = view.plan!;
      plan.productionExport = planExport();
      plan.candidateExport = { ...planExport(), expectedTotal: 14 };
      plan.shotCountChanged = true;
      view.planChanged = true; view.productionVersion = 1; view.productionPlanVersion = 3;
      ws = workspaceState(view);
      show();
      await userEvent.click(screen.getByRole("button", { name: "发布生产配方" }));
      const dialog = screen.getByRole("dialog");
      expect(dialog).toHaveTextContent("预期总帧数 ΣN_exp ± Σtol：14 ± 3（生产：12 ± 3）");
      expect(dialog).not.toHaveTextContent(/shotCount|deviceShots|PLC 要同步/);
    });
  });

  it("发布采集版 → 逐条列出未满足项且不能确认；全部满足后确认发布同一修订", async () => {
    vi.mocked(workspaceApi.publishCollect).mockResolvedValue(ws.data!);
    vi.mocked(workspaceApi.collectChecks).mockResolvedValueOnce([
      { name: "计划相机", passed: true, detail: "cam1、cam2、cam3 共用一个触发" },
      { name: "曝光与增益", passed: false, detail: "相机 cam2 缺少曝光与增益" },
      { name: "生产版本", passed: false, detail: "已有正式版 v3" },
    ]);
    show();
    await userEvent.click(screen.getByRole("button", { name: "发布采集版" }));
    let dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("采集版只在维护模式下运行，只采图不判定");
    expect(dialog).toHaveTextContent("2 项未满足");
    expect(dialog).toHaveTextContent("相机 cam2 缺少曝光与增益");
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

  it("验证未通过时不能发布", () => {
    ws.data!.workspace.validation!.passed = false;
    show();
    expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
  });

  it("确认期间候选修订变更必须重新验证和确认",async()=>{
    const page=show();await userEvent.click(screen.getByRole("button",{name:"发布生产配方"}));
    ws.data!.workspace.revision++;page.rerender(<MemoryRouter><ValidationPage/></MemoryRouter>);
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByRole("button",{name:"发布生产配方"})).toBeDisabled();expect(screen.getByText("未通过或待重验")).toBeVisible();
    expect(workspaceApi.publish).not.toHaveBeenCalled();
  });
});
