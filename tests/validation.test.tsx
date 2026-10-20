import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useSiteStatus } from "../src/features/cycle/api";
import ValidationPage from "../src/features/workspace/ValidationPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { historyApi } from "../src/features/history/api";
import { siteStatus, sampleSet, workspaceState } from "./fixtures";

vi.mock("../src/features/cycle/api", () => ({ useSiteStatus: vi.fn() }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { publish: vi.fn(), retryPublish: vi.fn(), abandonPublish: vi.fn() } }));
vi.mock("../src/features/history/api", () => ({ historyApi: { query: vi.fn() } }));
let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter><ValidationPage /></MemoryRouter>);
beforeEach(() => {
  vi.mocked(useSiteStatus).mockReturnValue(siteStatus());
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(historyApi.query).mockResolvedValue({ items: [], total: 0, counts: { ok: 0, ng: 0, err: 0, excursion: 0 } });
  vi.mocked(workspaceApi.publish).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.retryPublish).mockResolvedValue(ws.data!);
});

describe("验证与发布页面", () => {
  it("离线验证发布 → 明确仅供离线回放且候选调试无需发布", async () => {
    vi.mocked(useSiteStatus).mockReturnValue(siteStatus({ mode: "offline", source: "sampleSet", sampleSet: sampleSet() }));
    show();
    await userEvent.click(screen.getByRole("button", { name: "发布离线回放配方" }));
    expect(screen.getByRole("dialog", { name: "发布离线回放配方" })).toHaveTextContent("该版本仅用于离线回放，不能用于真机生产");
    expect(screen.getByRole("link", { name: "前往在线检测调试候选", hidden: true })).toHaveAttribute("href", "/inspect?candidate=A");
    expect(workspaceApi.publish).not.toHaveBeenCalled();
  });

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

  it("拍照计划变化 → 发布确认里提供 PLC 对接参数入口", async () => {
    ws.data!.planChanged = true;
    show();
    await userEvent.click(screen.getByRole("button", { name: "发布生产配方" }));
    expect(screen.getByRole("link", { name: "查看 PLC 对接参数" })).toHaveAttribute("href", "/plc?panel=s7-plan");
  });

  it.each([
    ["未选用任何样本", () => { ws.data!.workspace.samples = []; }, "先选用至少一组代表性样本"],
    ["已选样本但未确认适用", () => {}, "勾选上方确认后才能运行"],
    ["候选版本待生效且确认勾选被冻结", () => {
      ws.data!.workspace.pending = { doc: ws.doc!, bundleId: "bundle-v2", revision: 7, baseRevision: "old", frames: [], overview: ws.data!.workspace.overview, validation: ws.data!.workspace.validation!, validationSite: "real" };
      ws.data!.workspace.publish = { state: "failed", message: "发布包缺失" };
    }, "候选版本正在等待生效，生效或放弃后再验证"],
  ])("%s → 运行验证不可用并在按钮旁说明原因", (_, arrange, reason) => {
    arrange();
    show();
    expect(screen.getByRole("button", { name: "运行整套图像验证" })).toBeDisabled();
    expect(screen.getByRole("note")).toHaveTextContent(reason);
  });

  it("验证未通过时不能发布", () => {
    ws.data!.workspace.validation!.passed = false;
    show();
    expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
    expect(workspaceApi.publish).not.toHaveBeenCalled();
  });

  it("确认期间候选修订变更必须重新验证和确认",async()=>{
    const page=show();await userEvent.click(screen.getByRole("button",{name:"发布生产配方"}));
    ws.data!.workspace.revision++;page.rerender(<MemoryRouter><ValidationPage/></MemoryRouter>);
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByRole("button",{name:"发布生产配方"})).toBeDisabled();expect(screen.getByText("未通过或待重验")).toBeVisible();
    expect(workspaceApi.publish).not.toHaveBeenCalled();
  });
});
