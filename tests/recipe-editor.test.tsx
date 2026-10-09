import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RecipeEditor, { defaultPosition, defaultWidth, newShot, nextShotId } from "../src/features/recipe/components/RecipeEditor";
import { recipeApi } from "../src/features/cycle/api";
import type { Recipe, RecipeDoc, ShotSpec } from "../src/features/cycle/types";
import { deferred, shotList, summary, twoLines, workspaceView } from "./fixtures";

vi.mock("../src/features/cycle/api", () => ({ recipeApi: { preview: vi.fn(), save: vi.fn() } }));
beforeEach(() => {
  vi.mocked(recipeApi.preview).mockResolvedValue(workspaceView().layout);
  vi.mocked(recipeApi.save).mockResolvedValue(summary());
});
function show(saveCandidate?: (doc: RecipeDoc) => Promise<boolean>, initial = workspaceView().workspace.doc, cameras = [{ id: "CAM-1", name: "相机 1" }]) {
  const onDraftChange = vi.fn(), onSaved = vi.fn();
  const page = render(<RecipeEditor initial={initial} originalId="A" cameras={cameras} onSaved={onSaved} onDraftChange={onDraftChange} saveCandidate={saveCandidate} />);
  return { ...page, onDraftChange, onSaved };
}
const readySave = async (name = "保存") => { await waitFor(() => expect(screen.getByRole("button", { name })).toBeEnabled()); return screen.getByRole("button", { name }); };
const last = (onDraftChange: ReturnType<typeof vi.fn>) => onDraftChange.mock.lastCall![0] as RecipeDoc;
const number = (name: string, value: string) => fireEvent.change(screen.getByRole("spinbutton", { name }), { target: { value } });

describe("配方的拍照点表", () => {
  it("保存候选锁定编号，编辑不会直接保存生产配方", async () => {
    const saveCandidate = vi.fn().mockResolvedValue(true), { onDraftChange, onSaved } = show(saveCandidate);
    expect(screen.getByRole("textbox", { name: "配方编号" })).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox", { name: "名称" }), { target: { value: "新名称" } });
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ name: "新名称" }));
    expect(saveCandidate).not.toHaveBeenCalled(); expect(recipeApi.save).not.toHaveBeenCalled();
    await userEvent.click(await readySave("保存候选配置"));
    expect(saveCandidate).toHaveBeenCalledWith(expect.objectContaining({ id: "A", name: "新名称" }));
    expect(await screen.findByText("候选配置已保存，生产版本保持不变")).toBeVisible();
    expect(recipeApi.save).not.toHaveBeenCalled(); expect(onSaved).not.toHaveBeenCalled();
  });

  it("逐拍照点编辑编号、Pose、胶条与不检，中线保持不变", async () => {
    const { onDraftChange } = show();
    fireEvent.change(screen.getByRole("textbox", { name: "拍照点 1 · 编号" }), { target: { value: " A1 " } });
    fireEvent.change(screen.getByRole("textbox", { name: "拍照点 1 · Pose" }), { target: { value: "POSE 7" } });
    fireEvent.change(screen.getByRole("textbox", { name: "拍照点 2 · 胶条" }), { target: { value: "J2" } });
    await userEvent.click(screen.getByRole("checkbox", { name: "拍照点 2 · 不检" }));
    const shots = last(onDraftChange).shots;
    expect(shots[0]).toEqual({ id: "A1", poseId: "POSE 7", camera: "CAM-1", bead: "J1", skip: false, path: twoLines[0], mmPerPx: .1 });
    expect(shots[1]).toMatchObject({ bead: "J2", skip: true, path: twoLines[1] });
    const row = screen.getByRole("textbox", { name: "拍照点 2 · 编号" }).closest("tr")!;
    expect(within(row).getByText("不检")).toBeVisible();
    await userEvent.click(screen.getByRole("checkbox", { name: "拍照点 2 · 不检" }));
    expect(last(onDraftChange).shots[1].skip).toBe(false);
  });

  it("示教状态只读：已示教的点数与中线长度、未示教、不检", () => {
    const doc = workspaceView().workspace.doc;
    doc.shots.push({ id: "P3", poseId: "P3", camera: "CAM-1", bead: "J1", skip: false, path: [] }, { ...doc.shots[0], id: "P4", poseId: "P4", skip: true });
    show(undefined, doc);
    const status = (k: number) => screen.getByRole("textbox", { name: `拍照点 ${k} · 编号` }).closest("tr")!.querySelector(".rcp-teach")!;
    expect(status(1)).toHaveTextContent("已示教 2 点 · 1.0 mm"); expect(status(1)).toHaveClass("c-ok");
    expect(status(3)).toHaveTextContent("未示教"); expect(status(3)).toHaveClass("c-warn");
    expect(status(4)).toHaveTextContent("不检");
    // 中线不能在这里编辑
    expect(screen.queryByRole("spinbutton", { name: /拍照点 1 · 中心/ })).toBeNull();
    expect(screen.queryByRole("textbox", { name: "胶路点坐标" })).toBeNull();
    expect(screen.queryByRole("button", { name: "圆角矩形" })).toBeNull();
  });

  it("添加拍照点取第一个没用过的编号，沿用上一行相机与胶条，中线留空", async () => {
    const { onDraftChange } = show(undefined, workspaceView().workspace.doc, [{ id: "CAM-1", name: "相机 1" }, { id: "CAM-2", name: "相机 2" }]);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "拍照点 2 · 相机" }), "CAM-2");
    fireEvent.change(screen.getByRole("textbox", { name: "拍照点 2 · 胶条" }), { target: { value: "J3" } });
    await userEvent.click(screen.getByRole("button", { name: "添加拍照点" }));
    let shots = last(onDraftChange).shots;
    expect(shots[2]).toEqual({ id: "P3", poseId: "P3", camera: "CAM-2", bead: "J3", skip: false, path: [] });
    expect(screen.getByRole("textbox", { name: "拍照点 3 · 编号" })).toHaveValue("P3");
    await userEvent.click(screen.getByRole("button", { name: "删除拍照点 1" }));
    expect(last(onDraftChange).shots.map((s: ShotSpec) => s.id)).toEqual(["P2", "P3"]);
    await userEvent.click(screen.getByRole("button", { name: "添加拍照点" }));
    shots = last(onDraftChange).shots;
    expect(shots.map((s: ShotSpec) => s.id)).toEqual(["P2", "P3", "P1"]);
    expect(shots[2]).toEqual({ id: "P1", poseId: "P1", camera: "CAM-2", bead: "J3", skip: false, path: [] });
  });

  it("上移、下移交换相邻拍照点（连同中线），首行不能上移、末行不能下移", async () => {
    const { onDraftChange } = show();
    expect(screen.getByRole("button", { name: "上移拍照点 1" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "下移拍照点 2" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "下移拍照点 1" }));
    expect(last(onDraftChange).shots.map((s: ShotSpec) => [s.id, s.path])).toEqual([["P2", twoLines[1]], ["P1", twoLines[0]]]);
    expect(screen.getByRole("textbox", { name: "拍照点 1 · 编号" })).toHaveValue("P2");
    await userEvent.click(screen.getByRole("button", { name: "上移拍照点 2" }));
    expect(last(onDraftChange).shots).toEqual(workspaceView().workspace.doc.shots);
  });

  it("每个拍照点单独选相机，标定引用留空时提示用相机编号且不发送", async () => {
    const cameras = [{ id: "cam1", name: "相机 1" }, { id: "cam2", name: "相机 2" }, { id: "cam3", name: "相机 3" }];
    const initial = { ...workspaceView().workspace.doc, shots: shotList(twoLines, "cam1") };
    const { onDraftChange } = show(undefined, initial, cameras);
    await userEvent.click(screen.getByRole("button", { name: "添加拍照点" }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "拍照点 2 · 相机" }), "cam2");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "拍照点 3 · 相机" }), "cam3");
    expect(last(onDraftChange).shots.map((s: ShotSpec) => s.camera)).toEqual(["cam1", "cam2", "cam3"]);
    const calib = screen.getByRole("textbox", { name: "拍照点 3 · 标定引用" });
    expect(calib).toHaveAttribute("placeholder", "cam3");
    fireEvent.change(calib, { target: { value: " STATION-A " } }); expect(last(onDraftChange).shots[2].calib).toBe("STATION-A");
    fireEvent.change(calib, { target: { value: "  " } }); expect(last(onDraftChange).shots[2]).not.toHaveProperty("calib");
  });

  it("单独设置默认收起；勾选后从配方默认值起编辑，取消勾选即整项删除，保存时不发 null", async () => {
    const { onDraftChange } = show();
    const toggle = screen.getByRole("button", { name: "拍照点 1 · 单独设置" });
    expect(toggle).toHaveAttribute("aria-expanded", "false"); expect(toggle).toHaveTextContent("用默认");
    expect(screen.queryByRole("group", { name: "拍照点 1 · 单独设置" })).toBeNull();
    await userEvent.click(toggle);
    const panel = within(screen.getByRole("group", { name: "拍照点 1 · 单独设置" }));
    await userEvent.click(panel.getByRole("checkbox", { name: "单独设检测参数" }));
    expect(last(onDraftChange).shots[0].detect).toEqual(workspaceView().workspace.doc.detect);
    fireEvent.change(panel.getByRole("spinbutton", { name: "P1 · 搜索半宽（mm）" }), { target: { value: "8" } });
    await userEvent.selectOptions(panel.getByRole("combobox", { name: "P1 · 极性" }), "light");
    fireEvent.change(panel.getByRole("spinbutton", { name: "P1 · 胶宽上限（mm）" }), { target: { value: "9" } });
    expect(last(onDraftChange).shots[0].detect).toEqual({ searchMm: 8, polarity: "light", widthRange: [1, 9] });
    await userEvent.click(panel.getByRole("checkbox", { name: "单独设判定限值" }));
    expect(last(onDraftChange).shots[0].limits).toEqual(workspaceView().workspace.doc.limits);
    await userEvent.click(panel.getByRole("button", { name: "停用P1 · 位置判定" }));
    await userEvent.click(panel.getByRole("button", { name: "启用P1 · 胶宽判定" }));
    fireEvent.change(panel.getByRole("spinbutton", { name: "P1 · 允许断胶长度（mm）" }), { target: { value: "3" } });
    expect(last(onDraftChange).shots[0].limits).toEqual({ position: null, width: defaultWidth, maxGapLen: 3 });
    expect(toggle).toHaveTextContent("已单独设");
    // 配方默认值不受影响
    expect(last(onDraftChange).detect).toEqual(workspaceView().workspace.doc.detect);
    await userEvent.click(panel.getByRole("checkbox", { name: "单独设检测参数" }));
    await userEvent.click(panel.getByRole("checkbox", { name: "单独设判定限值" }));
    expect(last(onDraftChange).shots[0]).not.toHaveProperty("detect"); expect(last(onDraftChange).shots[0]).not.toHaveProperty("limits");
    await userEvent.click(toggle); expect(screen.queryByRole("group", { name: "拍照点 1 · 单独设置" })).toBeNull();
    await userEvent.click(await readySave());
    const saved = vi.mocked(recipeApi.save).mock.lastCall![0];
    expect(saved.shots.map(s => Object.keys(s))).toEqual([["id", "poseId", "camera", "bead", "skip", "path", "mmPerPx"], ["id", "poseId", "camera", "bead", "skip", "path", "mmPerPx"]]);
    expect(JSON.stringify(saved.shots)).not.toMatch(/null/);
  });

  it("载入时单独设的检测参数、限值与标定原样显示并保存", async () => {
    const doc = workspaceView().workspace.doc;
    doc.shots[1] = { ...doc.shots[1], calib: "CAM-1-B", detect: { searchMm: 6, polarity: "light", widthRange: [2, 5] }, limits: { position: null, width: { ...defaultWidth }, maxGapLen: 2 } };
    show(undefined, doc);
    expect(screen.getByRole("textbox", { name: "拍照点 2 · 标定引用" })).toHaveValue("CAM-1-B");
    const toggle = screen.getByRole("button", { name: "拍照点 2 · 单独设置" }); expect(toggle).toHaveTextContent("已单独设");
    await userEvent.click(toggle);
    expect(screen.getByRole("spinbutton", { name: "P2 · 搜索半宽（mm）" })).toHaveValue(6);
    expect(screen.getByRole("combobox", { name: "P2 · 极性" })).toHaveValue("light");
    expect(screen.getByRole("spinbutton", { name: "P2 · 胶宽 · 名义" })).toHaveValue(4);
    expect(screen.getByRole("button", { name: "启用P2 · 位置判定" })).toBeVisible();
    await userEvent.click(await readySave());
    expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({ shots: doc.shots }), "A");
  });

  it("没有拍照点时新增第一个用相机组第一台、胶条 J1；删空后由预览报错", async () => {
    const empty = { ...workspaceView().workspace.doc, shots: [] };
    const { onDraftChange } = show(undefined, empty, [{ id: "CAM-9", name: "相机 9" }]);
    await userEvent.click(screen.getByRole("button", { name: "添加拍照点" }));
    expect(last(onDraftChange).shots).toEqual([{ id: "P1", poseId: "P1", camera: "CAM-9", bead: "J1", skip: false, path: [] }]);
    await userEvent.click(screen.getByRole("button", { name: "删除拍照点 1" }));
    expect(last(onDraftChange).shots).toEqual([]);
  });

  it("新拍照点的编号与默认值", () => {
    expect(nextShotId([])).toBe("P1");
    expect(nextShotId(shotList([[], [], []]).filter(s => s.id !== "P2"))).toBe("P2");
    expect(newShot([], [])).toEqual({ id: "P1", poseId: "P1", camera: "", bead: "J1", skip: false, path: [] });
    const first = shotList(twoLines.slice(0, 1), "cam2"); first[0].bead = "J5"; first[0].skip = true;
    const next = newShot(first, [{ id: "cam1" }]);
    expect(next).toEqual({ id: "P2", poseId: "P2", camera: "cam2", bead: "J5", skip: false, path: [] });
    next.path.push([1, 1]); expect(first[0].path).toEqual(twoLines[0]);
  });

  it("拍照点达到 64 个后不能再添加", () => {
    show(undefined, { ...workspaceView().workspace.doc, shots: shotList(Array.from({ length: 64 }, () => [])) });
    expect(screen.getByRole("button", { name: "添加拍照点" })).toBeDisabled();
  });
});

describe("配方的站距、默认检测参数与默认限值", () => {
  it("站距、滤波、默认检测参数与默认限值一起进入保存参数", async () => {
    const { onDraftChange } = show(undefined, workspaceView().workspace.doc, [{ id: "CAM-1", name: "相机 1" }, { id: "CAM-2", name: "相机 2" }]);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "拍照点 1 · 相机" }), "CAM-2");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "触发方式" }), "stop");
    number("站距（mm）", "0.5"); number("中值滤波窗口（点，奇数）", "5");
    number("搜索半宽（mm）", "12"); await userEvent.selectOptions(screen.getByRole("combobox", { name: "极性" }), "light");
    number("胶宽下限（mm）", "2"); number("胶宽上限（mm）", "8");
    number("位置 · 上公差", "1.5"); number("允许断胶长度（mm）", "6");
    expect(last(onDraftChange)).toMatchObject({ spacing: .5, filterWindow: 5, triggerMode: "stop",
      detect: { searchMm: 12, polarity: "light", widthRange: [2, 8] }, limits: { position: { tolUpper: 1.5 }, width: null, maxGapLen: 6 } });
    await userEvent.click(await readySave());
    expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({ shots: [{ ...workspaceView().workspace.doc.shots[0], camera: "CAM-2" }, workspaceView().workspace.doc.shots[1]], spacing: .5 }), "A");
    for (const removed of ["path", "line", "corner", "segmentOverrides", "fov", "maxGapLen"]) expect(vi.mocked(recipeApi.save).mock.lastCall![0]).not.toHaveProperty(removed);
  });

  it("位置、胶宽判定可分别停用（不判）与启用，启用时按 MX11 的默认值", async () => {
    const { onDraftChange } = show();
    await userEvent.click(screen.getByRole("button", { name: "启用胶宽判定" }));
    expect(last(onDraftChange).limits.width).toEqual(defaultWidth);
    fireEvent.change(screen.getByRole("spinbutton", { name: "胶宽 · 名义" }), { target: { value: "2.4" } });
    expect(last(onDraftChange).limits.width!.nominal).toBe(2.4);
    await userEvent.click(screen.getByRole("button", { name: "停用位置判定" }));
    expect(last(onDraftChange).limits.position).toBeNull(); expect(screen.getByText("不判位置")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "启用位置判定" }));
    expect(last(onDraftChange).limits.position).toEqual(defaultPosition);
    await userEvent.click(screen.getByRole("button", { name: "停用胶宽判定" })); expect(last(onDraftChange).limits.width).toBeNull();
    expect(last(onDraftChange).limits.maxGapLen).toBe(.5);
  });

  it("规则清空保持空输入并由当前预览报错，不能保存空值作为零", async () => {
    show(); await readySave(); vi.mocked(recipeApi.preview).mockRejectedValueOnce(new Error("限值必须是有限数"));
    const input = screen.getByRole("spinbutton", { name: "位置 · 名义" }); fireEvent.change(input, { target: { value: "" } }); expect(input).toHaveValue(null);
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled(); expect(await screen.findByText("Error: 限值必须是有限数")).toBeVisible(); expect(recipeApi.save).not.toHaveBeenCalled();
  });

  it("预览按拍照点显示示教中线与汇总", async () => {
    const layout = workspaceView().layout; layout.shots.push({ id: "P3", poseId: "P3", camera: "CAM-1", bead: "J1", skip: false, path: [] });
    vi.mocked(recipeApi.preview).mockResolvedValue(layout); show();
    expect(await screen.findByText("3 个拍照点 · 已示教 2/3 · 中线共 2.0 mm · 4 个测量点")).toBeVisible();
    expect(screen.getByRole("group", { name: "拍照点 P1" })).toBeVisible(); expect(screen.getByText("未示教中线，不能开工")).toBeVisible();
    expect(within(screen.getByRole("group", { name: "拍照点 P1" })).getByText("已示教")).toHaveClass("c-mut"); expect(screen.queryByText("待测")).toBeNull();
    expect(screen.getByText("P1 · J1", { selector: ".rcp-segs span" })).toBeVisible();
  });
});

describe("配方保存与预览", () => {
  it("预览报错禁用保存，修正后重新预览才恢复", async () => {
    vi.mocked(recipeApi.preview).mockRejectedValueOnce(new Error("站距需在 0.1–10 mm 之间")).mockResolvedValue(workspaceView().layout);
    show(); expect(await screen.findByText("Error: 站距需在 0.1–10 mm 之间")).toBeVisible();
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
    number("站距（mm）", "2");
    await waitFor(() => expect(screen.getByRole("button", { name: "保存" })).toBeEnabled());
  });

  it("旧预览晚到不会覆盖新预览或解除当前错误", async () => {
    vi.useFakeTimers();
    try {
      const old = deferred<Recipe>();
      vi.mocked(recipeApi.preview).mockReturnValueOnce(old.promise).mockRejectedValueOnce(new Error("当前配方无效"));
      show(); await act(() => vi.advanceTimersByTimeAsync(350));
      number("站距（mm）", "0");
      await act(() => vi.advanceTimersByTimeAsync(350));
      expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
      await act(async () => old.resolve(workspaceView().layout));
      expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
      expect(screen.getByText("Error: 当前配方无效")).toBeVisible();
    } finally { vi.useRealTimers(); }
  });

  it("旧预览失败不阻止新配置保存", async () => {
    vi.useFakeTimers();
    try {
      const old = deferred<Recipe>();
      vi.mocked(recipeApi.preview).mockReturnValueOnce(old.promise).mockResolvedValueOnce(workspaceView().layout);
      show(); await act(() => vi.advanceTimersByTimeAsync(350));
      number("站距（mm）", "2");
      await act(() => vi.advanceTimersByTimeAsync(350));
      await act(async () => old.reject(new Error("旧配置错误")));
      expect(screen.getByRole("button", { name: "保存" })).toBeEnabled();
      expect(screen.queryByText("Error: 旧配置错误")).not.toBeInTheDocument();
    } finally { vi.useRealTimers(); }
  });

  it("直接保存携带原编号，失败不通知已保存，重试成功通知", async () => {
    vi.mocked(recipeApi.save).mockRejectedValueOnce(new Error("写入失败")).mockResolvedValue(summary());
    const { onSaved } = show(); await userEvent.click(await readySave());
    expect(await screen.findByText("Error: 写入失败")).toBeVisible(); expect(onSaved).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "保存" }));
    expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({ id: "A" }), "A"); expect(onSaved).toHaveBeenCalledWith("A");
  });

  it("修改后先验证当前预览，旧预览期间不能提交保存", async () => {
    const next = deferred<Recipe>(); const { onSaved } = show(); await readySave(); vi.mocked(recipeApi.preview).mockReturnValueOnce(next.promise);
    number("站距（mm）", "2");
    expect(screen.getByText("正在更新预览…")).toBeVisible(); expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "保存" })); expect(recipeApi.save).not.toHaveBeenCalled();
    await waitFor(() => expect(recipeApi.preview).toHaveBeenLastCalledWith(expect.objectContaining({ spacing: 2 })));
    await act(async () => next.resolve(workspaceView().layout)); await userEvent.click(await readySave());
    expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({ spacing: 2 }), "A"); expect(onSaved).toHaveBeenCalledWith("A");
  });

  it("保存期间锁定表单与重复提交，失败保留编辑值后可重试", async () => {
    const request = deferred<ReturnType<typeof summary>>(); vi.mocked(recipeApi.save).mockReturnValueOnce(request.promise);
    const { onSaved } = show(); fireEvent.change(screen.getByRole("textbox", { name: "名称" }), { target: { value: "待保存名称" } });
    await userEvent.click(await readySave()); expect(screen.getByRole("button", { name: "保存中…" })).toBeDisabled(); expect(screen.getByRole("textbox", { name: "名称" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "保存中…" })); expect(recipeApi.save).toHaveBeenCalledTimes(1);
    await act(async () => request.reject(new Error("写盘失败"))); expect(screen.getByText("Error: 写盘失败")).toBeVisible(); expect(onSaved).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: "名称" })).toHaveValue("待保存名称"); await userEvent.click(await readySave()); expect(onSaved).toHaveBeenCalledWith("A");
  });

  it("候选保存返回失败时不给出成功提示，重试成功后才确认", async () => {
    const saveCandidate = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true); const { onSaved } = show(saveCandidate);
    await userEvent.click(await readySave("保存候选配置")); expect(await screen.findByText("候选配置未保存，请修正错误后重试")).toBeVisible();
    await userEvent.click(await readySave("保存候选配置")); expect(await screen.findByText("候选配置已保存，生产版本保持不变")).toBeVisible();
    expect(onSaved).not.toHaveBeenCalled(); expect(recipeApi.save).not.toHaveBeenCalled();
  });

  it("保存请求晚于页面卸载完成时不通知其他编辑器已保存", async () => {
    const request = deferred<ReturnType<typeof summary>>(); vi.mocked(recipeApi.save).mockReturnValue(request.promise);
    const { onSaved, unmount } = show(); await userEvent.click(await readySave()); unmount(); await act(async () => request.resolve(summary())); expect(onSaved).not.toHaveBeenCalled();
  });

  it("配方重命名与共用参数保存仍带原编号，不漏掉编辑值", async () => {
    vi.mocked(recipeApi.save).mockResolvedValue({ ...summary(), id: "NEW-A" }); const { onSaved } = show();
    fireEvent.change(screen.getByRole("textbox", { name: "配方编号" }), { target: { value: " NEW-A " } }); fireEvent.change(screen.getByRole("textbox", { name: "名称" }), { target: { value: "新产品" } });
    for (const [label, value] of [["产品代码（PLC 下发）", "42"], ["站距（mm）", "0.5"], ["中值滤波窗口（点，奇数）", "7"]]) number(label, value);
    await userEvent.click(await readySave()); expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({ id: "NEW-A", name: "新产品", productCode: 42, spacing: .5, filterWindow: 7 }), "A"); expect(onSaved).toHaveBeenCalledWith("NEW-A");
  });

  it("已有相机不存在时保留引用并标明，可改选有效工位", async () => {
    const doc = workspaceView().workspace.doc; doc.shots[1].camera = "MISSING";
    const { onDraftChange } = show(undefined, doc);
    expect(screen.getByRole("option", { name: "MISSING（不在相机组里）" })).toBeVisible();
    expect(screen.getByRole("combobox", { name: "拍照点 1 · 相机" })).toHaveValue("CAM-1"); expect(screen.getByRole("combobox", { name: "拍照点 2 · 相机" })).toHaveValue("MISSING");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "拍照点 2 · 相机" }), "CAM-1"); expect(last(onDraftChange).shots[1].camera).toBe("CAM-1");
    expect(screen.queryByRole("option", { name: "MISSING（不在相机组里）" })).toBeNull();
  });
});
