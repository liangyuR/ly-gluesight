import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import GeometryPage from "../src/features/workspace/GeometryPage";
import { WorkspaceProvider, useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { cameraApi, defaultCameraConfig } from "../src/features/camera/api";
import { desktopAvailable } from "../src/lib/desktop";
import type { Recipe, RecipeDoc, ShotSpec } from "../src/features/cycle/types";
import type { WorkspaceView } from "../src/features/workspace/types";
import { deferred, shotList, summary, twoLines, workspaceState, workspaceView } from "./fixtures";

// 编辑和保存用真实 provider；仅对 provider 正常不会产生的空预览状态使用受控上下文。
let controlled: ReturnType<typeof useWorkspace> | null = null;
vi.mock("../src/features/workspace/context", async importOriginal => {
  const real = await importOriginal<typeof import("../src/features/workspace/context")>();
  return { ...real, useWorkspace: () => controlled ?? real.useWorkspace() };
});
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { get: vi.fn(), list: vi.fn(), saveDoc: vi.fn() } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn(), preview: vi.fn(), save: vi.fn() } }));
vi.mock("../src/features/camera/api", async importOriginal => {
  const real = await importOriginal<typeof import("../src/features/camera/api")>();
  return { ...real, cameraApi: { ...real.cameraApi, rigConfig: vi.fn() } };
});
vi.mock("../src/features/plc/api", () => ({ subscribe: () => () => {} }));

let views: Record<string, WorkspaceView>;
// 这里只提供已知测量点的 API 响应，不在测试中实现或模拟图像检测、胶路采样算法。
const previewFor = (doc: RecipeDoc): Recipe => ({ ...structuredClone(views[doc.id].layout), ...structuredClone(doc) });
function savedView(doc: RecipeDoc, revision: number) {
  const next = structuredClone(views[doc.id]);
  next.workspace.doc = structuredClone(doc); next.workspace.revision = revision + 1;
  next.layout = previewFor(doc);
  return next;
}
function Probe() {
  const { doc, busy, dirty } = useWorkspace();
  return <><output aria-label="当前候选草稿">{JSON.stringify(doc)}</output><output aria-label="工作台状态">{JSON.stringify({ busy, dirty })}</output>
    <output aria-label="当前路径">{useLocation().pathname}</output></>;
}
function show() {
  return render(<MemoryRouter initialEntries={["/recipe/geometry"]}><WorkspaceProvider><GeometryPage /><Probe /></WorkspaceProvider></MemoryRouter>);
}
const editor = () => within(screen.getByRole("heading", { name: "候选拍照点与检测规则" }).closest("section")!);
const draft = () => JSON.parse(screen.getByLabelText("当前候选草稿").textContent!) as RecipeDoc;
async function open() { const page = show(); await screen.findByRole("textbox", { name: "名称" }); return page; }
async function readySave() {
  await waitFor(() => expect(editor().getByRole("button", { name: "保存候选配置" })).toBeEnabled());
  return editor().getByRole("button", { name: "保存候选配置" });
}

beforeEach(() => {
  controlled = null; views = { A: workspaceView() };
  vi.mocked(desktopAvailable).mockReturnValue(true);
  vi.mocked(workspaceApi.list).mockReset().mockImplementation(async () => Object.values(views).map(v => structuredClone(v.workspace)));
  vi.mocked(workspaceApi.get).mockReset().mockImplementation(async id => structuredClone(views[id]));
  vi.mocked(workspaceApi.saveDoc).mockReset().mockImplementation(async (_id, revision, doc) => {
    const next = savedView(doc, revision); views[doc.id] = next; return structuredClone(next);
  });
  vi.mocked(recipeApi.list).mockReset().mockImplementation(async () => ({ recipes: Object.values(views).map(summary), errors: [] }));
  vi.mocked(recipeApi.preview).mockReset().mockImplementation(async doc => previewFor(doc));
  vi.mocked(recipeApi.save).mockReset();
  vi.mocked(cameraApi.rigConfig).mockReset().mockResolvedValue([
    { ...defaultCameraConfig, id: "CAM-1", name: "相机 1", exposureUs: 80 },
    { ...defaultCameraConfig, id: "CAM-2", name: "相机 2", exposureUs: 125 },
  ]);
});

describe("拍照点规划页面与真实候选编辑器的接线", () => {
  it("真实字段修改传入工作台草稿，编辑器保存使用当前修订与完整候选", async () => {
    await open();
    fireEvent.change(editor().getByRole("textbox", { name: "名称" }), { target: { value: "新候选胶路" } });
    fireEvent.change(editor().getByRole("spinbutton", { name: "产品代码（PLC 下发）" }), { target: { value: "9" } });
    fireEvent.change(editor().getByRole("spinbutton", { name: "站距（mm）" }), { target: { value: "1.5" } });
    fireEvent.change(editor().getByRole("spinbutton", { name: "位置 · 上公差" }), { target: { value: "1.5" } });
    fireEvent.change(editor().getByRole("textbox", { name: "拍照点 2 · 胶条" }), { target: { value: "J2" } });
    expect(draft()).toMatchObject({ id: "A", name: "新候选胶路", productCode: 9, spacing: 1.5, limits: { position: { tolUpper: 1.5 } } });
    expect(draft().shots[1]).toMatchObject({ bead: "J2", path: twoLines[1] });
    expect(screen.getByText("候选配置尚未保存")).toBeVisible(); expect(workspaceApi.saveDoc).not.toHaveBeenCalled();
    const candidate = draft(); await userEvent.click(await readySave());
    expect(workspaceApi.saveDoc).toHaveBeenCalledWith("A", 7, candidate);
    expect(await screen.findByText("候选配置已保存，生产版本保持不变")).toBeVisible();
    expect(screen.getByText("修订 8")).toBeVisible(); expect(screen.getByText("生产 v1")).toBeVisible();
    expect(screen.queryByText("候选配置尚未保存")).toBeNull(); expect(recipeApi.save).not.toHaveBeenCalled();
    expect(editor().getByRole("textbox", { name: "名称" })).toHaveValue("新候选胶路");
  });

  it("编辑器保存等待防重复，锁定字段、拍照点和配方选择，接受结果后恢复", async () => {
    await open(); fireEvent.change(editor().getByRole("textbox", { name: "名称" }), { target: { value: "等待保存" } });
    const candidate = draft(), request = deferred<WorkspaceView>(); vi.mocked(workspaceApi.saveDoc).mockReturnValueOnce(request.promise);
    const save = await readySave(); fireEvent.click(save); fireEvent.click(save);
    expect(workspaceApi.saveDoc).toHaveBeenCalledTimes(1); expect(editor().getByRole("button", { name: "保存中…" })).toBeDisabled();
    expect(editor().getByRole("textbox", { name: "名称" })).toBeDisabled(); expect(editor().getByRole("spinbutton", { name: "站距（mm）" })).toBeDisabled();
    expect(editor().getByRole("textbox", { name: "拍照点 1 · 胶条" })).toBeDisabled(); expect(editor().getByRole("checkbox", { name: "拍照点 1 · 不检" })).toBeDisabled();
    expect(editor().getByRole("button", { name: "添加拍照点" })).toBeDisabled();
    expect(screen.getByRole("combobox", { name: "当前配方" })).toBeDisabled();
    expect(screen.getByLabelText("工作台状态")).toHaveTextContent('"busy":true');
    const response = savedView(candidate, 7); views.A = response; await act(async () => request.resolve(response));
    expect(editor().getByRole("textbox", { name: "名称" })).toBeEnabled(); expect(screen.getByRole("combobox", { name: "当前配方" })).toBeEnabled();
    expect(await screen.findByText("候选配置已保存，生产版本保持不变")).toBeVisible();
  });

  it("保存失败不宣称成功，草稿仍可修改并重试", async () => {
    await open(); fireEvent.change(editor().getByRole("textbox", { name: "名称" }), { target: { value: "重试候选" } });
    vi.mocked(workspaceApi.saveDoc).mockRejectedValueOnce(new Error("候选文件不可写"));
    await userEvent.click(await readySave());
    expect(await screen.findByText("候选配置未保存，请修正错误后重试")).toBeVisible(); expect(screen.getByText("Error: 候选文件不可写")).toBeVisible();
    expect(screen.queryByText("候选配置已保存，生产版本保持不变")).toBeNull(); expect(screen.getByText("候选配置尚未保存")).toBeVisible();
    fireEvent.change(editor().getByRole("textbox", { name: "名称" }), { target: { value: "已修正候选" } });
    await userEvent.click(await readySave()); expect(workspaceApi.saveDoc).toHaveBeenCalledTimes(2);
    expect(workspaceApi.saveDoc).toHaveBeenLastCalledWith("A", 7, expect.objectContaining({ name: "已修正候选" }));
    expect(await screen.findByText("候选配置已保存，生产版本保持不变")).toBeVisible(); expect(screen.queryByText("Error: 候选文件不可写")).toBeNull();
  });

  it("工作台栏发起保存的 busy 状态也锁定编辑器，与编辑器自己的等待状态独立", async () => {
    await open(); fireEvent.change(editor().getByRole("textbox", { name: "名称" }), { target: { value: "栏内保存" } });
    const candidate = draft(), request = deferred<WorkspaceView>(); vi.mocked(workspaceApi.saveDoc).mockReturnValueOnce(request.promise);
    const notice = within(screen.getByText("候选配置尚未保存").closest(".wp-notice")!);
    fireEvent.click(notice.getByRole("button", { name: "保存候选配置" }));
    expect(workspaceApi.saveDoc).toHaveBeenCalledWith("A", 7, candidate);
    expect(editor().queryByRole("button", { name: "保存中…" })).toBeNull();
    expect(editor().getByRole("button", { name: "保存候选配置" })).toBeDisabled();
    expect(editor().getByRole("textbox", { name: "名称" })).toBeDisabled(); expect(editor().getByRole("combobox", { name: "拍照点 1 · 相机" })).toBeDisabled();
    const response = savedView(candidate, 7); views.A = response; await act(async () => request.resolve(response));
    expect(editor().getByRole("textbox", { name: "名称" })).toBeEnabled(); expect(screen.queryByText("候选配置尚未保存")).toBeNull();
  });

  it("预览失败同时反馈工作台与真实编辑器，修正后重新预览才能保存", async () => {
    await open(); vi.mocked(recipeApi.preview).mockImplementation(async doc => {
      if (!(doc.spacing >= .1)) throw new Error("站距需在 0.1–10 mm 之间"); return previewFor(doc);
    });
    fireEvent.change(editor().getByRole("spinbutton", { name: "站距（mm）" }), { target: { value: "0" } });
    await waitFor(() => expect(screen.getAllByText("Error: 站距需在 0.1–10 mm 之间")).toHaveLength(2));
    expect(screen.getByText("候选参数无效")).toBeVisible();
    for (const button of screen.getAllByRole("button", { name: "保存候选配置" })) expect(button).toBeDisabled();
    expect(workspaceApi.saveDoc).not.toHaveBeenCalled();
    fireEvent.change(editor().getByRole("spinbutton", { name: "站距（mm）" }), { target: { value: "2" } });
    await readySave(); expect(screen.queryByText("候选参数无效")).toBeNull();
    await userEvent.click(await readySave()); expect(workspaceApi.saveDoc).toHaveBeenCalledWith("A", 7, expect.objectContaining({ spacing: 2 }));
  });

  it("切换候选后编辑器重建，旧草稿不带到新候选，保存按当前候选", async () => {
    const next = workspaceView("B"); next.workspace.doc.shots = shotList(twoLines, "CAM-2");
    next.layout = { ...next.layout, ...structuredClone(next.workspace.doc) }; views.B = next;
    await open(); expect(screen.getByRole("link", { name: "进入单帧示教" })).toHaveAttribute("href", "/recipe/teach");
    expect(screen.getByRole("heading", { name: "飞拍可行性" })).toBeVisible();
    fireEvent.change(editor().getByRole("textbox", { name: "名称" }), { target: { value: "旧候选草稿" } });
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "当前配方" }), "B");
    await waitFor(() => expect(editor().getByRole("textbox", { name: "名称" })).toHaveValue("工件 B"));
    expect(editor().getByRole("combobox", { name: "拍照点 1 · 相机" })).toHaveValue("CAM-2"); expect(screen.getByRole("heading", { name: "飞拍可行性" })).toBeVisible();
    fireEvent.change(editor().getByRole("spinbutton", { name: "站距（mm）" }), { target: { value: "2" } });
    expect(draft()).toMatchObject({ id: "B", name: "工件 B", shots: shotList(twoLines, "CAM-2"), spacing: 2 });
    await userEvent.click(await readySave());
    expect(workspaceApi.saveDoc).toHaveBeenCalledWith("B", 7, expect.objectContaining({ name: "工件 B", shots: shotList(twoLines, "CAM-2"), spacing: 2 }));
    await userEvent.click(screen.getByRole("link", { name: "进入单帧示教" })); expect(screen.getByLabelText("当前路径")).toHaveTextContent("/recipe/teach");
  });

  it("飞拍可行性接收首个拍照点相机的曝光，切换相机和进入示教均使用真实控件", async () => {
    await open();
    // 可行性表单通过 effect 同步相机曝光；名称字段出现时同步可能尚未提交。
    await waitFor(() => expect(screen.getByRole("spinbutton", { name: "曝光（µs）" })).toHaveValue(80));
    await userEvent.selectOptions(editor().getByRole("combobox", { name: "拍照点 2 · 相机" }), "CAM-2");
    expect(draft().shots.map(s => s.camera)).toEqual(["CAM-1", "CAM-2"]);
    expect(screen.getByRole("spinbutton", { name: "曝光（µs）" })).toHaveValue(80);
    await userEvent.selectOptions(editor().getByRole("combobox", { name: "拍照点 1 · 相机" }), "CAM-2");
    expect(draft().shots[0].camera).toBe("CAM-2");
    await waitFor(() => expect(screen.getByRole("spinbutton", { name: "曝光（µs）" })).toHaveValue(125));
    await userEvent.click(screen.getByRole("link", { name: "进入单帧示教" })); expect(screen.getByLabelText("当前路径")).toHaveTextContent("/recipe/teach");
  });

  it("配方引用相机已移除时保留引用，可行性使用默认曝光", async () => {
    views.A.workspace.doc.shots = shotList(twoLines, "CAM-OLD"); views.A.layout.shots = shotList(twoLines, "CAM-OLD");
    await open(); expect(editor().getByRole("combobox", { name: "拍照点 1 · 相机" })).toHaveValue("CAM-OLD");
    expect(screen.getAllByRole("option", { name: "CAM-OLD（不在相机组里）" })).toHaveLength(2);
    expect(screen.getByRole("spinbutton", { name: "曝光（µs）" })).toHaveValue(60);
  });

  it("未发布候选以新配方编辑，仍按候选编号和修订保存", async () => {
    views.A.workspace.baseHash = null; views.A.productionVersion = null;
    await open(); expect(editor().getByRole("heading", { name: "新配方 · 飞拍" })).toBeVisible();
    expect(screen.getByText("生产 未发布")).toBeVisible(); expect(editor().getByRole("textbox", { name: "配方编号" })).toBeDisabled();
    fireEvent.change(editor().getByRole("textbox", { name: "名称" }), { target: { value: "首个候选" } });
    await userEvent.click(await readySave()); expect(workspaceApi.saveDoc).toHaveBeenCalledWith("A", 7, expect.objectContaining({ name: "首个候选" }));
    expect(recipeApi.save).not.toHaveBeenCalled();
  });
});

describe("拍照点规划页面的胶路示教比例", () => {
  const untaught = (id: string): ShotSpec => ({ id, poseId: id, camera: "CAM-1", view: 1, bead: "J1", skip: false, path: [] });
  it.each([
    { name: "要检的拍照点都已示教", shots: () => shotList(twoLines), percent: "100", tone: "ok", note: /沿示教中线量胶/ },
    { name: "有未示教的拍照点：可以保存但不能开工", shots: () => [...shotList(twoLines), untaught("P3"), untaught("P4")], percent: "50", tone: "warn", note: /未示教：P3、P4，可以保存，但不能开工/ },
    { name: "不检的拍照点不算在内", shots: () => [...shotList(twoLines), { ...untaught("P3"), skip: true }], percent: "100", tone: "ok", note: /沿示教中线量胶/ },
    { name: "只有像素当量或只有中线都不算示教", shots: () => [{ ...untaught("P1"), mmPerPx: .1 }, { ...untaught("P2"), path: [[1, 1], [9, 9]] as [number, number][] }], percent: "0", tone: "warn", note: /未示教：P1、P2/ },
  ])("$name", async ({ shots, percent, tone, note }) => {
    views.A.workspace.doc.shots = shots(); views.A.layout.shots = shots();
    await open(); expect(screen.getByText(`胶路示教 ${percent}%`)).toHaveClass(tone);
    expect(screen.getByText(note)).toBeVisible();
  });

  it("在表里把未示教的拍照点设为不检，示教比例随新的 API 预览更新", async () => {
    views.A.workspace.doc.shots = [...shotList(twoLines), untaught("P3")]; views.A.layout.shots = [...shotList(twoLines), untaught("P3")];
    await open(); expect(screen.getByText("胶路示教 67%")).toHaveClass("warn");
    expect(within(editor().getByRole("textbox", { name: "拍照点 3 · 编号" }).closest("tr")!).getByText("未示教")).toBeVisible();
    await userEvent.click(editor().getByRole("checkbox", { name: "拍照点 3 · 不检" }));
    expect(await screen.findByText("胶路示教 100%")).toHaveClass("ok");
    expect(recipeApi.preview).toHaveBeenCalledWith(expect.objectContaining({ shots: expect.arrayContaining([expect.objectContaining({ id: "P3", skip: true })]) }));
    expect(workspaceApi.saveDoc).not.toHaveBeenCalled();
  });

  it("预览暂缺时显示工作台已有示教比例，真实编辑器仍保留", () => {
    const view = workspaceView(); view.coverage = 42.5; controlled = { ...workspaceState(view), preview: null };
    show(); expect(screen.getByText("胶路示教 43%")).toHaveClass("warn");
    expect(screen.getByRole("textbox", { name: "名称" })).toHaveValue("工件 A");
  });
});

describe("拍照点规划页面的空状态", () => {
  it("浏览器模式显示桌面数据说明与查看入口", () => {
    vi.mocked(desktopAvailable).mockReturnValue(false); show();
    expect(screen.getByText("当前为浏览器查看模式")).toBeVisible(); expect(screen.getByRole("link", { name: "查看交互原型" })).toHaveAttribute("href", "/workflow/guide");
    expect(screen.queryByRole("heading", { name: "候选拍照点与检测规则" })).toBeNull(); expect(workspaceApi.get).not.toHaveBeenCalled();
  });

  it("有工作台但没有候选文档时显示选择入口，不装载编辑器", () => {
    controlled = { ...workspaceState(), doc: null }; show();
    expect(screen.getByText("尚未选择配方")).toBeVisible(); expect(screen.getByRole("link", { name: "打开配方库" })).toHaveAttribute("href", "/recipe");
    expect(screen.queryByRole("textbox", { name: "名称" })).toBeNull();
  });
});
