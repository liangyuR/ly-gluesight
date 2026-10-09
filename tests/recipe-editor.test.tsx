import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RecipeEditor, { newShot, nextShotId } from "../src/features/recipe/components/RecipeEditor";
import { recipeApi } from "../src/features/cycle/api";
import type { Recipe, RecipeDoc, ShotSpec } from "../src/features/cycle/types";
import { deferred, shotList, summary, workspaceView } from "./fixtures";

vi.mock("../src/features/cycle/api", () => ({ recipeApi: { preview: vi.fn(), save: vi.fn(), parsePath: vi.fn() } }));
beforeEach(() => {
  vi.mocked(recipeApi.preview).mockResolvedValue(workspaceView().layout);
  vi.mocked(recipeApi.save).mockResolvedValue(summary());
  vi.mocked(recipeApi.parsePath).mockResolvedValue({points:[[0,0],[10,0],[10,10]],bulges:[.5,0,0],closed:true,note:"取最长路径"});
});
function show(saveCandidate?: (doc: RecipeDoc) => Promise<boolean>,initial=workspaceView().workspace.doc,cameras=[{ id: "CAM-1", name: "相机 1" }]) {
  const onDraftChange = vi.fn(), onSaved = vi.fn();
  const page = render(<RecipeEditor initial={initial} originalId="A" cameras={cameras} onSaved={onSaved} onDraftChange={onDraftChange} saveCandidate={saveCandidate} />);
  return { ...page, onDraftChange, onSaved };
}
const readySave=async(name="保存")=>{await waitFor(()=>expect(screen.getByRole("button",{name})).toBeEnabled());return screen.getByRole("button",{name});};
function file(name="path.csv",text="x,y\n0,0\n10,0"){
  const f=new File([text],name,{type:"text/plain"});Object.defineProperty(f,"text",{configurable:true,value:vi.fn(async()=>text)});return f;
}
const polyline=async()=>userEvent.click(screen.getByRole("button",{name:"折线 / 导入"}));
const upload=async(f:File)=>userEvent.setup({applyAccept:false}).upload(screen.getByLabelText("导入胶路文件"),f);

describe("配方几何与规则编辑", () => {
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

  it("逐拍照点编辑编号、Pose 与中心，清空坐标留给预览报错", () => {
    const { onDraftChange } = show();
    fireEvent.change(screen.getByRole("textbox", { name: "拍照点 1 · 编号" }), { target: { value: " A1 " } });
    fireEvent.change(screen.getByRole("textbox", { name: "拍照点 1 · Pose" }), { target: { value: "POSE 7" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "拍照点 2 · 中心 X（mm）" }), { target: { value: "40" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "拍照点 2 · 中心 Y（mm）" }), { target: { value: "" } });
    const shots = onDraftChange.mock.lastCall![0].shots;
    expect(shots[0]).toEqual({ id: "A1", poseId: "POSE 7", camera: "CAM-1", center: [25, 30] });
    expect(shots[1].center[0]).toBe(40); expect(shots[1].center[1]).toBeNaN();
    expect(screen.getByRole("spinbutton", { name: "拍照点 2 · 中心 Y（mm）" })).toHaveValue(null);
  });

  it("添加拍照点取第一个没用过的编号，沿用上一行相机与中心，视野与标定留空", async () => {
    const { onDraftChange } = show(undefined, workspaceView().workspace.doc, [{ id: "CAM-1", name: "相机 1" }, { id: "CAM-2", name: "相机 2" }]);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "拍照点 2 · 相机" }), "CAM-2");
    await userEvent.click(screen.getByRole("button", { name: "添加拍照点" }));
    let shots = onDraftChange.mock.lastCall![0].shots;
    expect(shots[2]).toEqual({ id: "P3", poseId: "P3", camera: "CAM-2", center: [75, 30] });
    expect(screen.getByRole("textbox", { name: "拍照点 3 · 编号" })).toHaveValue("P3");
    await userEvent.click(screen.getByRole("button", { name: "删除拍照点 1" }));
    expect(onDraftChange.mock.lastCall![0].shots.map((s: ShotSpec) => s.id)).toEqual(["P2", "P3"]);
    await userEvent.click(screen.getByRole("button", { name: "添加拍照点" }));
    shots = onDraftChange.mock.lastCall![0].shots;
    expect(shots.map((s: ShotSpec) => s.id)).toEqual(["P2", "P3", "P1"]);
    expect(shots[2]).toEqual({ id: "P1", poseId: "P1", camera: "CAM-2", center: [75, 30] });
  });

  it("上移、下移交换相邻拍照点，首行不能上移、末行不能下移", async () => {
    const { onDraftChange } = show();
    expect(screen.getByRole("button", { name: "上移拍照点 1" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "下移拍照点 2" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "下移拍照点 1" }));
    expect(onDraftChange.mock.lastCall![0].shots.map((s: ShotSpec) => s.id)).toEqual(["P2", "P1"]);
    expect(screen.getByRole("textbox", { name: "拍照点 1 · 编号" })).toHaveValue("P2");
    await userEvent.click(screen.getByRole("button", { name: "上移拍照点 2" }));
    expect(onDraftChange.mock.lastCall![0].shots).toEqual(workspaceView().workspace.doc.shots);
  });

  it("每个拍照点单独选相机", async () => {
    const cameras = [{ id: "cam1", name: "相机 1" }, { id: "cam2", name: "相机 2" }, { id: "cam3", name: "相机 3" }];
    const initial = { ...workspaceView().workspace.doc, shots: shotList([[25, 30], [75, 30]], "cam1") };
    const { onDraftChange } = show(undefined, initial, cameras);
    await userEvent.click(screen.getByRole("button", { name: "添加拍照点" }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "拍照点 2 · 相机" }), "cam2");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "拍照点 3 · 相机" }), "cam3");
    expect(onDraftChange.mock.lastCall![0].shots.map((s: ShotSpec) => s.camera)).toEqual(["cam1", "cam2", "cam3"]);
    expect(screen.getByRole("combobox", { name: "拍照点 1 · 相机" })).toHaveValue("cam1");
    expect(screen.getByRole("textbox", { name: "拍照点 3 · 标定引用" })).toHaveAttribute("placeholder", "cam3");
  });

  it("视野与标定留空不发送；只设一边时另一边取默认视野", async () => {
    const { onDraftChange } = show();
    const last = () => onDraftChange.mock.lastCall![0].shots[0];
    const width = screen.getByRole("spinbutton", { name: "拍照点 1 · 视野宽（mm）" }), height = screen.getByRole("spinbutton", { name: "拍照点 1 · 视野高（mm）" });
    expect(width).toHaveValue(null); expect(width).toHaveAttribute("placeholder", "120"); expect(height).toHaveAttribute("placeholder", "80");
    fireEvent.change(width, { target: { value: "150" } }); expect(last().fov).toEqual([150, 80]);
    fireEvent.change(width, { target: { value: "" } }); expect(last()).not.toHaveProperty("fov");
    fireEvent.change(height, { target: { value: "90" } }); expect(last().fov).toEqual([120, 90]);
    fireEvent.change(width, { target: { value: "140" } }); expect(last().fov).toEqual([140, 90]);
    fireEvent.change(width, { target: { value: "" } }); expect(last().fov).toEqual([120, 90]);
    fireEvent.change(height, { target: { value: "" } }); expect(last()).not.toHaveProperty("fov");
    const calib = screen.getByRole("textbox", { name: "拍照点 1 · 标定引用" });
    expect(calib).toHaveAttribute("placeholder", "CAM-1");
    fireEvent.change(calib, { target: { value: " STATION-A " } }); expect(last().calib).toBe("STATION-A");
    fireEvent.change(calib, { target: { value: "  " } }); expect(last()).not.toHaveProperty("calib");
    await userEvent.click(await readySave());
    const saved = vi.mocked(recipeApi.save).mock.lastCall![0];
    expect(saved.shots.map(s => Object.keys(s))).toEqual([["id", "poseId", "camera", "center"], ["id", "poseId", "camera", "center"]]);
    expect(JSON.stringify(saved.shots)).not.toMatch(/null/);
  });

  it("载入时单独设的视野与标定原样显示并保存", async () => {
    const doc = workspaceView().workspace.doc; doc.shots[1] = { ...doc.shots[1], fov: [60, 50], calib: "CAM-1-B" };
    show(undefined, doc);
    expect(screen.getByRole("spinbutton", { name: "拍照点 2 · 视野宽（mm）" })).toHaveValue(60);
    expect(screen.getByRole("spinbutton", { name: "拍照点 2 · 视野高（mm）" })).toHaveValue(50);
    expect(screen.getByRole("textbox", { name: "拍照点 2 · 标定引用" })).toHaveValue("CAM-1-B");
    await userEvent.click(await readySave());
    expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({ shots: doc.shots }), "A");
  });

  it("没有拍照点时新增第一个在胶路中心，用相机组第一台；删空后由预览报错", async () => {
    const empty = { ...workspaceView().workspace.doc, shots: [] };
    const { onDraftChange } = show(undefined, empty, [{ id: "CAM-9", name: "相机 9" }]);
    await userEvent.click(screen.getByRole("button", { name: "添加拍照点" }));
    expect(onDraftChange.mock.lastCall![0].shots).toEqual([{ id: "P1", poseId: "P1", camera: "CAM-9", center: [50, 30] }]);
    await userEvent.click(screen.getByRole("button", { name: "删除拍照点 1" }));
    expect(onDraftChange.mock.lastCall![0].shots).toEqual([]);
  });

  it("新拍照点的编号与默认位置", () => {
    const poly = { kind: "polyline" as const, points: [[10, 0], [110, 0], [110, 80]] as [number, number][], closed: false, radius: 0 };
    expect(nextShotId([])).toBe("P1");
    expect(nextShotId(shotList([[0, 0], [0, 0], [0, 0]]).filter(s => s.id !== "P2"))).toBe("P2");
    expect(newShot([], [], poly)).toEqual({ id: "P1", poseId: "P1", camera: "", center: [60, 40] });
    expect(newShot([], [], { ...poly, points: [[NaN, 0]] }).center).toEqual([0, 0]);
    const first = shotList([[5, 6]], "cam2");
    const next = newShot(first, [{ id: "cam1" }], poly);
    expect(next).toEqual({ id: "P2", poseId: "P2", camera: "cam2", center: [5, 6] });
    next.center[0] = 99; expect(first[0].center).toEqual([5, 6]);
  });

  it("拍照点达到 64 个后不能再添加", () => {
    show(undefined, { ...workspaceView().workspace.doc, shots: shotList(Array.from({ length: 64 }, (_, k) => [k, 0] as [number, number])) });
    expect(screen.getByRole("button", { name: "添加拍照点" })).toBeDisabled();
  });

  it("折线 bulge 表头保留圆弧，未命名第三列不当作圆弧", async () => {
    const { onDraftChange } = show(); await userEvent.click(screen.getByRole("button", { name: "折线 / 导入" }));
    const input = screen.getByPlaceholderText("每行一个点：x, y（mm），可选第 3 列 bulge");
    fireEvent.change(input, { target: { value: "x,y,bulge\n0,0,0.5\n10,0,0" } });
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ path: expect.objectContaining({ points: [[0, 0], [10, 0]], bulges: [.5, 0] }) }));
    fireEvent.change(input, { target: { value: "0,0,99\n10,0,99" } });
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ path: expect.objectContaining({ bulges: [] }) }));
  });

  it("启用胶宽规则只修改指定段，保留位置限值", async () => {
    const { onDraftChange } = show();
    await userEvent.click(screen.getAllByRole("button", { name: "启用胶宽判定" })[0]);
    const doc = onDraftChange.mock.lastCall![0];
    expect(doc.line.width).toMatchObject({ nominal: 2 });
    expect(doc.line.position).toEqual(workspaceView().workspace.doc.line.position);
    expect(doc.corner.width).toBeNull();
  });

  it("预览报错禁用保存，修正后重新预览才恢复", async () => {
    vi.mocked(recipeApi.preview).mockRejectedValueOnce(new Error("胶路覆盖不足")).mockResolvedValue(workspaceView().layout);
    show(); expect(await screen.findByText("Error: 胶路覆盖不足")).toBeVisible();
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
    fireEvent.change(screen.getByRole("spinbutton", { name: "视野宽（mm）" }), { target: { value: "200" } });
    await waitFor(() => expect(screen.getByRole("button", { name: "保存" })).toBeEnabled());
  });

  it("旧预览晚到不会覆盖新预览或解除当前错误", async () => {
    vi.useFakeTimers();
    try {
      const old = deferred<Recipe>();
      vi.mocked(recipeApi.preview).mockReturnValueOnce(old.promise).mockRejectedValueOnce(new Error("当前胶路无效"));
      show(); await act(() => vi.advanceTimersByTimeAsync(350));
      fireEvent.change(screen.getByRole("spinbutton", { name: "视野宽（mm）" }), { target: { value: "0" } });
      await act(() => vi.advanceTimersByTimeAsync(350));
      expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
      await act(async () => old.resolve(workspaceView().layout));
      expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
      expect(screen.getByText("Error: 当前胶路无效")).toBeVisible();
    } finally { vi.useRealTimers(); }
  });

  it("直接保存携带原编号，失败不通知已保存，重试成功通知", async () => {
    vi.mocked(recipeApi.save).mockRejectedValueOnce(new Error("写入失败")).mockResolvedValue(summary());
    const { onSaved } = show(); await userEvent.click(await readySave());
    expect(await screen.findByText("Error: 写入失败")).toBeVisible(); expect(onSaved).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "保存" }));
    expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({ id: "A" }), "A"); expect(onSaved).toHaveBeenCalledWith("A");
  });

  it("旧预览失败不阻止新配置保存", async () => {
    vi.useFakeTimers();
    try {
      const old = deferred<Recipe>();
      vi.mocked(recipeApi.preview).mockReturnValueOnce(old.promise).mockResolvedValueOnce(workspaceView().layout);
      show(); await act(() => vi.advanceTimersByTimeAsync(350));
      fireEvent.change(screen.getByRole("spinbutton", { name: "视野宽（mm）" }), { target: { value: "200" } });
      await act(() => vi.advanceTimersByTimeAsync(350));
      await act(async () => old.reject(new Error("旧配置错误")));
      expect(screen.getByRole("button", { name: "保存" })).toBeEnabled();
      expect(screen.queryByText("Error: 旧配置错误")).not.toBeInTheDocument();
    } finally { vi.useRealTimers(); }
  });

  it("内置矩形与开放、闭合折线切换传递实际几何参数",async()=>{
    const {onDraftChange}=show();
    fireEvent.change(screen.getByRole("spinbutton",{name:"宽（mm）"}),{target:{value:"240"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"高（mm）"}),{target:{value:"140"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"圆角半径（mm）"}),{target:{value:"20"}});
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({path:{kind:"roundedRect",width:240,height:140,radius:20}}));
    await polyline();const input=screen.getByRole("textbox",{name:"胶路点坐标"});
    fireEvent.change(input,{target:{value:"0,0\n100,0\n100,80"}});await userEvent.click(screen.getByRole("checkbox",{name:"闭合胶路"}));
    fireEvent.change(screen.getByRole("spinbutton",{name:"拐角倒圆半径（mm）"}),{target:{value:"5"}});
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({path:{kind:"polyline",points:[[0,0],[100,0],[100,80]],bulges:[],closed:true,radius:5}}));
    await userEvent.click(screen.getByRole("checkbox",{name:"闭合胶路"}));expect(onDraftChange.mock.lastCall![0].path.closed).toBe(false);
    await userEvent.click(screen.getByRole("button",{name:"圆角矩形"}));expect(onDraftChange.mock.lastCall![0].path.kind).toBe("roundedRect");
  });

  it.each(["path.csv","path.dxf"])("%s 导入调用文件解析，保留圆弧、闭合与说明",async name=>{
    const {onDraftChange}=show();await polyline();const imported=file(name);
    await upload(imported);
    expect(recipeApi.parsePath).toHaveBeenCalledWith("x,y\n0,0\n10,0",name);
    expect(await screen.findByText(`从 ${name} 读到 3 个点、1 段圆弧，闭合。取最长路径`)).toBeVisible();
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({path:expect.objectContaining({points:[[0,0],[10,0],[10,10]],bulges:[.5,0,0],closed:true})}));
    expect(screen.getByRole("textbox",{name:"胶路点坐标"})).toHaveValue("x, y, bulge\n0, 0, 0.5\n10, 0, 0\n10, 10, 0");
  });

  it("导入期间保留名称编辑与既有倒圆半径，不用旧配置覆盖整份草稿",async()=>{
    const request=deferred<Awaited<ReturnType<typeof recipeApi.parsePath>>>();vi.mocked(recipeApi.parsePath).mockReturnValue(request.promise);
    const {onDraftChange}=show();await polyline();fireEvent.change(screen.getByRole("spinbutton",{name:"拐角倒圆半径（mm）"}),{target:{value:"8"}});
    await upload(file());expect(screen.getByRole("button",{name:"保存"})).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox",{name:"名称"}),{target:{value:"编辑中的新名称"}});
    await act(async()=>request.resolve({points:[[0,0],[20,0]],bulges:[],closed:false,note:null}));
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({name:"编辑中的新名称",path:expect.objectContaining({radius:8,points:[[0,0],[20,0]]})}));
  });

  it("导入失败保持胶路并允许同一文件重新选择",async()=>{
    vi.mocked(recipeApi.parsePath).mockRejectedValueOnce(new Error("DXF 没有可读路径"));
    const {onDraftChange}=show();await polyline();const input=screen.getByRole("textbox",{name:"胶路点坐标"});
    fireEvent.change(input,{target:{value:"0,0\n40,0"}});const original=onDraftChange.mock.lastCall![0].path;
    const imported=file("path.dxf");await upload(imported);
    expect(await screen.findByText("Error: DXF 没有可读路径")).toBeVisible();expect(onDraftChange.mock.lastCall![0].path).toEqual(original);
    await upload(imported);expect(await screen.findByText(/从 path.dxf 读到 3 个点/)).toBeVisible();expect(recipeApi.parsePath).toHaveBeenCalledTimes(2);
  });

  it.each(["empty","large","format"])("%s 文件在读取之前给出可修正错误",async kind=>{
    show();await polyline();const imported=file(kind==="format"?"path.png":"path.csv",kind==="empty"?"":"0,0\n10,0");
    if(kind==="large")Object.defineProperty(imported,"size",{value:5*1024*1024+1});
    await upload(imported);
    expect(screen.getByText(kind==="empty"?"Error: 导入文件为空":kind==="large"?"Error: 胶路文件不能超过 5 MB":"Error: 胶路导入支持 CSV、TXT 或 DXF 文件")).toBeVisible();
    expect(imported.text).not.toHaveBeenCalled();expect(recipeApi.parsePath).not.toHaveBeenCalled();
  });

  it("旧文件导入晚到不能覆盖最新文件导入或报错",async()=>{
    const old=deferred<Awaited<ReturnType<typeof recipeApi.parsePath>>>();vi.mocked(recipeApi.parsePath).mockReturnValueOnce(old.promise).mockResolvedValueOnce({points:[[0,0],[50,0]],bulges:[],closed:false,note:null});
    const {onDraftChange}=show();await polyline();await upload(file("old.csv"));await upload(file("new.csv"));
    expect(await screen.findByText("从 new.csv 读到 2 个点")).toBeVisible();
    await act(async()=>old.reject(new Error("旧文件坏了")));
    expect(screen.queryByText("Error: 旧文件坏了")).not.toBeInTheDocument();expect(onDraftChange.mock.lastCall![0].path.points).toEqual([[0,0],[50,0]]);
  });

  it("文件解析尚未完成时手工编辑胶路优先，迟到导入不能撤回操作",async()=>{
    const old=deferred<Awaited<ReturnType<typeof recipeApi.parsePath>>>();vi.mocked(recipeApi.parsePath).mockReturnValue(old.promise);
    const {onDraftChange}=show();await polyline();await upload(file());
    fireEvent.change(screen.getByRole("textbox",{name:"胶路点坐标"}),{target:{value:"0,0\n90,0"}});
    await act(async()=>old.resolve({points:[[0,0],[10,0]],bulges:[],closed:true,note:null}));
    expect(onDraftChange.mock.lastCall![0].path.points).toEqual([[0,0],[90,0]]);expect(screen.queryByText(/从 path.csv 读到/)).not.toBeInTheDocument();
  });

  it("读取文件时卸载编辑器不继续解析或写入草稿",async()=>{
    const text=deferred<string>(),imported=file();Object.defineProperty(imported,"text",{configurable:true,value:vi.fn(()=>text.promise)});
    const {unmount,onDraftChange}=show();await polyline();await upload(imported);const calls=onDraftChange.mock.calls.length;
    unmount();await act(async()=>text.resolve("0,0\n10,0"));expect(recipeApi.parsePath).not.toHaveBeenCalled();expect(onDraftChange).toHaveBeenCalledTimes(calls);
  });

  it("修改后先验证当前预览，旧预览期间不能提交保存",async()=>{
    const next=deferred<Recipe>();const {onSaved}=show();await readySave();vi.mocked(recipeApi.preview).mockReturnValueOnce(next.promise);
    fireEvent.change(screen.getByRole("spinbutton",{name:"宽（mm）"}),{target:{value:"260"}});
    expect(screen.getByText("正在更新预览…")).toBeVisible();expect(screen.getByRole("button",{name:"保存"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"保存"}));expect(recipeApi.save).not.toHaveBeenCalled();
    await waitFor(()=>expect(recipeApi.preview).toHaveBeenLastCalledWith(expect.objectContaining({path:expect.objectContaining({width:260})})));
    await act(async()=>next.resolve(workspaceView().layout));await userEvent.click(await readySave());
    expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({path:expect.objectContaining({width:260})}),"A");expect(onSaved).toHaveBeenCalledWith("A");
  });

  it("保存期间锁定表单与重复提交，失败保留编辑值后可重试",async()=>{
    const request=deferred<ReturnType<typeof summary>>();vi.mocked(recipeApi.save).mockReturnValueOnce(request.promise);
    const {onSaved}=show();fireEvent.change(screen.getByRole("textbox",{name:"名称"}),{target:{value:"待保存名称"}});
    await userEvent.click(await readySave());expect(screen.getByRole("button",{name:"保存中…"})).toBeDisabled();expect(screen.getByRole("textbox",{name:"名称"})).toBeDisabled();
    fireEvent.click(screen.getByRole("button",{name:"保存中…"}));expect(recipeApi.save).toHaveBeenCalledTimes(1);
    await act(async()=>request.reject(new Error("写盘失败")));expect(screen.getByText("Error: 写盘失败")).toBeVisible();expect(onSaved).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox",{name:"名称"})).toHaveValue("待保存名称");await userEvent.click(await readySave());expect(onSaved).toHaveBeenCalledWith("A");
  });

  it("候选保存返回失败时不给出成功提示，重试成功后才确认",async()=>{
    const saveCandidate=vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);const {onSaved}=show(saveCandidate);
    await userEvent.click(await readySave("保存候选配置"));expect(await screen.findByText("候选配置未保存，请修正错误后重试")).toBeVisible();
    await userEvent.click(await readySave("保存候选配置"));expect(await screen.findByText("候选配置已保存，生产版本保持不变")).toBeVisible();
    expect(onSaved).not.toHaveBeenCalled();expect(recipeApi.save).not.toHaveBeenCalled();
  });
  it("保存请求晚于页面卸载完成时不通知其他编辑器已保存",async()=>{
    const request=deferred<ReturnType<typeof summary>>();vi.mocked(recipeApi.save).mockReturnValue(request.promise);
    const {onSaved,unmount}=show();await userEvent.click(await readySave());unmount();await act(async()=>request.resolve(summary()));expect(onSaved).not.toHaveBeenCalled();
  });

  it("飞拍工位、停稳触发、视野与规则修改一起进入保存参数",async()=>{
    const {onDraftChange}=show(undefined,workspaceView().workspace.doc,[{id:"CAM-1",name:"相机 1"},{id:"CAM-2",name:"相机 2"}]);
    await userEvent.selectOptions(screen.getByRole("combobox",{name:"拍照点 1 · 相机"}),"CAM-2");await userEvent.selectOptions(screen.getByRole("combobox",{name:"触发方式"}),"stop");
    fireEvent.change(screen.getByRole("spinbutton",{name:"视野高（mm）"}),{target:{value:"160"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"中值滤波窗口（点，奇数）"}),{target:{value:"5"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"直线段 · 位置 · 上公差"}),{target:{value:"1.5"}});
    await userEvent.click(await readySave());expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({shots:[{...workspaceView().workspace.doc.shots[0],camera:"CAM-2"},workspaceView().workspace.doc.shots[1]],triggerMode:"stop",fov:[120,160],filterWindow:5,line:expect.objectContaining({position:expect.objectContaining({tolUpper:1.5})})}),"A");
    expect(onDraftChange.mock.lastCall![0].corner.position.tolUpper).toBe(1);
  });
  it("配方重命名与共用测量参数保存仍带原编号，不漏掉编辑值",async()=>{
    vi.mocked(recipeApi.save).mockResolvedValue({...summary(),id:"NEW-A"});const {onSaved}=show();
    fireEvent.change(screen.getByRole("textbox",{name:"配方编号"}),{target:{value:" NEW-A "}});fireEvent.change(screen.getByRole("textbox",{name:"名称"}),{target:{value:"新产品"}});
    for(const [label,value] of [["产品代码（PLC 下发）","42"],["测量点间距（mm）","0.5"],["中值滤波窗口（点，奇数）","7"],["允许断胶长度（mm）","1.2"]])fireEvent.change(screen.getByRole("spinbutton",{name:label}),{target:{value}});
    await userEvent.click(await readySave());expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({id:"NEW-A",name:"新产品",productCode:42,spacing:.5,filterWindow:7,maxGapLen:1.2}),"A");expect(onSaved).toHaveBeenCalledWith("NEW-A");
  });

  it("已有相机不存在时保留引用并标明，可改选有效工位",async()=>{
    const doc=workspaceView().workspace.doc;doc.shots[1].camera="MISSING";
    const {onDraftChange}=show(undefined,doc);
    expect(screen.getByRole("option",{name:"MISSING（不在相机组里）"})).toBeVisible();
    expect(screen.getByRole("combobox",{name:"拍照点 1 · 相机"})).toHaveValue("CAM-1");expect(screen.getByRole("combobox",{name:"拍照点 2 · 相机"})).toHaveValue("MISSING");
    await userEvent.selectOptions(screen.getByRole("combobox",{name:"拍照点 2 · 相机"}),"CAM-1");expect(onDraftChange.mock.lastCall![0].shots[1].camera).toBe("CAM-1");
    expect(screen.queryByRole("option",{name:"MISSING（不在相机组里）"})).toBeNull();
  });

  it("新增胶宽规则按名义胶宽 2 mm 给出默认限值并随配方保存",async()=>{
    show();
    await userEvent.click(screen.getAllByRole("button",{name:"启用胶宽判定"})[1]);await userEvent.click(await readySave());
    expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({corner:expect.objectContaining({width:expect.objectContaining({nominal:2,tolUpper:.7,tolLower:.6,absMin:.8,absMax:3.8})})}),"A");
  });

  it("启用、修改、停用胶宽判定只影响选中段",async()=>{
    const {onDraftChange}=show();await userEvent.click(screen.getAllByRole("button",{name:"启用胶宽判定"})[0]);
    fireEvent.change(screen.getByRole("spinbutton",{name:"直线段 · 胶宽 · 名义"}),{target:{value:"2.4"}});
    expect(onDraftChange.mock.lastCall![0].line.width.nominal).toBe(2.4);
    await userEvent.click(screen.getByRole("button",{name:"停用胶宽判定"}));expect(onDraftChange.mock.lastCall![0].line.width).toBeNull();
    expect(onDraftChange.mock.lastCall![0].line.position).toEqual(workspaceView().workspace.doc.line.position);expect(onDraftChange.mock.lastCall![0].corner.width).toBeNull();
  });

  it("规则清空保持空输入并由当前预览报错，不能保存空值作为零",async()=>{
    show();await readySave();vi.mocked(recipeApi.preview).mockRejectedValueOnce(new Error("限值必须是有限数"));
    const input=screen.getByRole("spinbutton",{name:"直线段 · 位置 · 名义"});fireEvent.change(input,{target:{value:""}});expect(input).toHaveValue(null);
    expect(screen.getByRole("button",{name:"保存"})).toBeDisabled();expect(await screen.findByText("Error: 限值必须是有限数")).toBeVisible();expect(recipeApi.save).not.toHaveBeenCalled();
  });
});
