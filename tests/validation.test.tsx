import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ValidationPage from "../src/features/workspace/ValidationPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { historyApi } from "../src/features/history/api";
import { workspaceState } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { validate: vi.fn(), publish: vi.fn(), retryPublish: vi.fn(), importSample: vi.fn() } }));
vi.mock("../src/features/history/api", () => ({ historyApi: { query: vi.fn() } }));
let ws: ReturnType<typeof workspaceState>;
const acknowledge = () => { const box=screen.getByRole("checkbox",{name:"已确认所选样本的拍摄条件、轨迹和触发顺序适用于当前候选"}); if(!(box as HTMLInputElement).checked) fireEvent.click(box); };
const show = () => { const page=render(<MemoryRouter><ValidationPage /></MemoryRouter>); acknowledge(); return page; };
beforeEach(() => {
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(historyApi.query).mockResolvedValue({ items: [], total: 0, counts: { ok: 0, ng: 0, err: 0, excursion: 0 } });
  vi.mocked(workspaceApi.validate).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.publish).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.retryPublish).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.importSample).mockResolvedValue(ws.data!);
});

describe("验证与发布页面", () => {
  it("发布失败后冻结样本配置并重试同一待生效版本", async () => {
    ws.data!.workspace.pending = { doc: ws.doc!, bundleId: "bundle-v2", revision: 7, baseRevision: "old", frames: [], overview: ws.data!.workspace.overview, validation: ws.data!.workspace.validation! };
    ws.data!.workspace.publishError = "生效状态保存失败";
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

  it.each(["dirty", "frame-dirty", "busy", "failed", "stale", "pending"])("%s 禁止发布", condition => {
    if (condition === "dirty") ws.dirty = true;
    if (condition === "frame-dirty") ws.frameDirty = true;
    if (condition === "busy") ws.busy = true;
    if (condition === "failed") ws.data!.workspace.validation!.passed = false;
    if (condition === "stale") ws.data!.workspace.validation!.revision = 6;
    if (condition === "pending") ws.data!.workspace.pending = { doc: ws.doc!, bundleId: "bundle-v1", revision: 7, baseRevision: "old", frames: [], overview: ws.data!.workspace.overview, validation: ws.data!.workspace.validation! };
    show(); expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
    expect(workspaceApi.publish).not.toHaveBeenCalled();
  });

  it("发布需要确认，取消不发布，确认携带当前修订", async () => {
    show(); await userEvent.click(screen.getByRole("button", { name: "发布生产配方" }));
    expect(screen.getByRole("dialog", { name: "发布生产配方" })).toBeVisible();
    expect(workspaceApi.publish).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(workspaceApi.publish).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "发布生产配方" }));
    await userEvent.click(screen.getByRole("button", { name: "确认发布 v2" }));
    expect(workspaceApi.publish).toHaveBeenCalledWith("A", 7);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("改变样本期望使验证失效，重验携带修改后的期望", async () => {
    show(); await userEvent.selectOptions(screen.getByRole("combobox", { name: "良品组期望结论" }), "NG_WIDTH");
    expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
    expect(screen.getByText("样本选用或期望已改变")).toBeVisible();
    acknowledge(); await userEvent.click(screen.getByRole("button", { name: "运行整套图像验证" }));
    expect(workspaceApi.validate).toHaveBeenCalledWith("A", 7, expect.arrayContaining([{ historyId: null, sampleId: "good", expected: "NG_WIDTH" }]), true);
  });

  it("确认期间候选修订变更必须重新验证和确认",async()=>{
    const page=show();await userEvent.click(screen.getByRole("button",{name:"发布生产配方"}));
    ws.data!.workspace.revision++;page.rerender(<MemoryRouter><ValidationPage/></MemoryRouter>);
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByRole("button",{name:"发布生产配方"})).toBeDisabled();expect(screen.getByText("未通过或待重验")).toBeVisible();
    expect(workspaceApi.publish).not.toHaveBeenCalled();
  });
});
