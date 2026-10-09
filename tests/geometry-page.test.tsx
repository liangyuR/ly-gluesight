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
import type { Recipe, RecipeDoc } from "../src/features/cycle/types";
import type { WorkspaceView } from "../src/features/workspace/types";
import { deferred, shotList, summary, workspaceState, workspaceView } from "./fixtures";

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
const editor = () => within(screen.getByRole("heading", { name: "候选胶路与检测规则" }).closest("section")!);
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

describe("胶路页面与真实候选编辑器的接线", () => {
  it("真实字段修改传入工作台草稿，编辑器保存使用当前修订与完整候选", async () => {
    await open();
    fireEvent.change(editor().getByRole("textbox", { name: "名称" }), { target: { value: "新候选胶路" } });
    fireEvent.change(editor().getByRole("spinbutton", { name: "产品代码（PLC 下发）" }), { target: { value: "9" } });
    fireEvent.change(editor().getByRole("spinbutton", { name: "宽（mm）" }), { target: { value: "150" } });
    fireEvent.change(editor().getByRole("spinbutton", { name: "直线段 · 位置 · 上公差" }), { target: { value: "1.5" } });
    expect(draft()).toMatchObject({ id: "A", name: "新候选胶路", productCode: 9, path: { width: 150 }, line: { position: { tolUpper: 1.5 } } });
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
    expect(editor().getByRole("textbox", { name: "名称" })).toBeDisabled(); expect(editor().getByRole("spinbutton", { name: "宽（mm）" })).toBeDisabled();
    expect(editor().getByRole("spinbutton", { name: "拍照点 1 · 中心 X（mm）" })).toBeDisabled();
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
      if (doc.fov[0] <= 0) throw new Error("视野必须大于零"); return previewFor(doc);
    });
    fireEvent.change(editor().getByRole("spinbutton", { name: "视野宽（mm）" }), { target: { value: "0" } });
    await waitFor(() => expect(screen.getAllByText("Error: 视野必须大于零")).toHaveLength(2));
    expect(screen.getByText("候选参数无效")).toBeVisible();
    for (const button of screen.getAllByRole("button", { name: "保存候选配置" })) expect(button).toBeDisabled();
    expect(workspaceApi.saveDoc).not.toHaveBeenCalled();
    fireEvent.change(editor().getByRole("spinbutton", { name: "视野宽（mm）" }), { target: { value: "100" } });
    await readySave(); expect(screen.queryByText("候选参数无效")).toBeNull();
    await userEvent.click(await readySave()); expect(workspaceApi.saveDoc).toHaveBeenCalledWith("A", 7, expect.objectContaining({ fov: [100, 80] }));
  });

  it("切换候选后编辑器重建，旧草稿不带到新候选，保存按当前候选", async () => {
    const next = workspaceView("B"); next.workspace.doc.shots = shotList([[25, 30], [75, 30]], "CAM-2");
    next.layout = { ...next.layout, ...structuredClone(next.workspace.doc) }; views.B = next;
    await open(); expect(screen.getByRole("link", { name: "进入单帧示教" })).toHaveAttribute("href", "/recipe/teach");
    expect(screen.getByRole("heading", { name: "飞拍可行性" })).toBeVisible();
    fireEvent.change(editor().getByRole("textbox", { name: "名称" }), { target: { value: "旧候选草稿" } });
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "当前配方" }), "B");
    await waitFor(() => expect(editor().getByRole("textbox", { name: "名称" })).toHaveValue("工件 B"));
    expect(editor().getByRole("combobox", { name: "拍照点 1 · 相机" })).toHaveValue("CAM-2"); expect(screen.getByRole("heading", { name: "飞拍可行性" })).toBeVisible();
    fireEvent.change(editor().getByRole("spinbutton", { name: "视野宽（mm）" }), { target: { value: "110" } });
    expect(draft()).toMatchObject({ id: "B", name: "工件 B", shots: shotList([[25, 30], [75, 30]], "CAM-2"), fov: [110, 80] });
    await userEvent.click(await readySave());
    expect(workspaceApi.saveDoc).toHaveBeenCalledWith("B", 7, expect.objectContaining({ name: "工件 B", shots: shotList([[25, 30], [75, 30]], "CAM-2"), fov: [110, 80] }));
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
    views.A.workspace.doc.shots = shotList([[25, 30], [75, 30]], "CAM-OLD"); views.A.layout.shots = shotList([[25, 30], [75, 30]], "CAM-OLD");
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

describe("胶路页面的搜索余量覆盖提示", () => {
  it.each([
    { name: "零余量包含视野边界", points: [[-10, 0], [0, 0], [10, 0]], fov: [20, 20], shots: [[0, 0]], margins: [0], percent: "100.00", tone: "ok" },
    { name: "增加搜索余量后边界点不算覆盖", points: [[-10, 0], [0, 0], [10, 0]], fov: [20, 20], shots: [[0, 0]], margins: [4], percent: "33.33", tone: "warn" },
    { name: "每个拍照点使用自己的搜索余量", points: [[0, 0], [4, 0], [10, 0], [14, 0]], fov: [12, 10], shots: [[0, 0], [10, 0]], margins: [0, 4], percent: "75.00", tone: "warn" },
    { name: "缺少帧参数使用默认四毫米余量", points: [[2, 0], [4, 0]], fov: [12, 12], shots: [[0, 0]], margins: [], percent: "50.00", tone: "warn" },
    { name: "纵向也必须完整包含搜索窗口", points: [[0, 0], [0, 7], [0, -7]], fov: [30, 20], shots: [[0, 0]], margins: [4], percent: "33.33", tone: "warn" },
    { name: "没有测量点时不显示虚假满覆盖", points: [], fov: [20, 20], shots: [[0, 0]], margins: [4], percent: "0.00", tone: "warn" },
  ])("$name", async ({ points, fov, shots, margins, percent, tone }) => {
    const view = views.A;
    view.workspace.doc.fov = fov as [number, number]; view.workspace.doc.shots = shotList(shots as [number, number][]);
    view.layout.fov = fov as [number, number]; view.layout.shots = shotList(shots as [number, number][]);
    view.layout.points = { x: points.map(p => p[0]), y: points.map(p => p[1]), k: points.map(() => 0), seg: points.map(() => 0) };
    const template = view.workspace.frames[0]; view.workspace.frames = margins.map((searchMm, k) => ({ ...structuredClone(template), k, params: { ...template.params, searchMm } }));
    await open(); const badge = screen.getByText(`搜索窗口覆盖 ${percent}%`);
    expect(badge).toHaveClass(tone); expect(view.coverage).toBe(100);
  });

  it("拍照点单独设的视野参与覆盖计算，没设的用配方视野", async () => {
    const view = views.A; view.workspace.doc.fov = view.layout.fov = [20, 20];
    view.workspace.doc.shots = view.layout.shots = [{ ...shotList([[0, 0]])[0], fov: [40, 20] }, { ...shotList([[0, 0], [100, 0]])[1] }];
    view.layout.points = { x: [-15, 0, 15, 92, 100], y: [0, 0, 0, 0, 0], k: [0, 0, 0, 1, 1], seg: [0, 0, 0, 0, 0] };
    view.workspace.frames = view.workspace.frames.map(f => ({ ...f, params: { ...f.params, searchMm: 0 } }));
    // P1 的 40 mm 宽视野覆盖 ±15，P2 用配方的 20 mm 视野只覆盖 90–110
    await open(); expect(screen.getByText("搜索窗口覆盖 100.00%")).toHaveClass("ok");
  });

  it("真实编辑器修改视野后工作台覆盖提示随新的 API 预览更新", async () => {
    const view = views.A; view.workspace.doc.shots = shotList([[0, 0]]); view.workspace.doc.fov = [30, 30];
    view.layout.shots = shotList([[0, 0]]); view.layout.fov = [30, 30]; view.layout.points = { x: [-10, 0, 10], y: [0, 0, 0], k: [0, 0, 0], seg: [0, 0, 0] };
    view.workspace.frames = [view.workspace.frames[0]];
    await open(); expect(screen.getByText("搜索窗口覆盖 100.00%")).toHaveClass("ok");
    fireEvent.change(editor().getByRole("spinbutton", { name: "视野宽（mm）" }), { target: { value: "20" } });
    expect(await screen.findByText("搜索窗口覆盖 33.33%")).toHaveClass("warn");
    expect(recipeApi.preview).toHaveBeenCalledWith(expect.objectContaining({ fov: [20, 30] }));
    expect(workspaceApi.saveDoc).not.toHaveBeenCalled();
  });

  it("预览暂缺时显示工作台已有覆盖率，真实编辑器仍保留", () => {
    const view = workspaceView(); view.coverage = 42.5; controlled = { ...workspaceState(view), preview: null };
    show(); expect(screen.getByText("搜索窗口覆盖 42.50%")).toHaveClass("warn");
    expect(screen.getByRole("textbox", { name: "名称" })).toHaveValue("工件 A");
  });
});

describe("胶路页面的空状态", () => {
  it("浏览器模式显示桌面数据说明与查看入口", () => {
    vi.mocked(desktopAvailable).mockReturnValue(false); show();
    expect(screen.getByText("当前为浏览器查看模式")).toBeVisible(); expect(screen.getByRole("link", { name: "查看交互原型" })).toHaveAttribute("href", "/workflow/guide");
    expect(screen.queryByRole("heading", { name: "候选胶路与检测规则" })).toBeNull(); expect(workspaceApi.get).not.toHaveBeenCalled();
  });

  it("有工作台但没有候选文档时显示选择入口，不装载编辑器", () => {
    controlled = { ...workspaceState(), doc: null }; show();
    expect(screen.getByText("尚未选择配方")).toBeVisible(); expect(screen.getByRole("link", { name: "打开配方库" })).toHaveAttribute("href", "/recipe");
    expect(screen.queryByRole("textbox", { name: "名称" })).toBeNull();
  });
});
