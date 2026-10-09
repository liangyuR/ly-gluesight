import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RecipeEditor from "../src/features/recipe/components/RecipeEditor";
import { recipeApi } from "../src/features/cycle/api";
import type { Recipe, RecipeDoc } from "../src/features/cycle/types";
import { deferred, summary, workspaceView } from "./fixtures";

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

  it("拍照点文本支持分隔符、备注和额外列，过滤非有限坐标", () => {
    const { onDraftChange } = show();
    fireEvent.change(screen.getByRole("textbox", { name: "拍照点中心（每行 x, y，按拍照顺序）" }), {
      target: { value: "x,y,z\n10,20,100\n30;40;备注\nNaN,50\n60,Infinity\n70\n80\t90" },
    });
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ shots: [[10, 20], [30, 40], [80, 90]] }));
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
    await userEvent.selectOptions(screen.getByRole("combobox",{name:"相机"}),"CAM-2");await userEvent.selectOptions(screen.getByRole("combobox",{name:"触发方式"}),"stop");
    fireEvent.change(screen.getByRole("spinbutton",{name:"视野高（mm）"}),{target:{value:"160"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"中值滤波窗口（点，奇数）"}),{target:{value:"5"}});
    fireEvent.change(screen.getByRole("spinbutton",{name:"直线段 · 位置 · 上公差"}),{target:{value:"1.5"}});
    await userEvent.click(await readySave());expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({camera:"CAM-2",triggerMode:"stop",fov:[120,160],filterWindow:5,line:expect.objectContaining({position:expect.objectContaining({tolUpper:1.5})})}),"A");
    expect(onDraftChange.mock.lastCall![0].corner.position.tolUpper).toBe(1);
  });
  it("配方重命名与共用测量参数保存仍带原编号，不漏掉编辑值",async()=>{
    vi.mocked(recipeApi.save).mockResolvedValue({...summary(),id:"NEW-A"});const {onSaved}=show();
    fireEvent.change(screen.getByRole("textbox",{name:"配方编号"}),{target:{value:" NEW-A "}});fireEvent.change(screen.getByRole("textbox",{name:"名称"}),{target:{value:"新产品"}});
    for(const [label,value] of [["产品代码（PLC 下发）","42"],["测量点间距（mm）","0.5"],["中值滤波窗口（点，奇数）","7"],["允许断胶长度（mm）","1.2"]])fireEvent.change(screen.getByRole("spinbutton",{name:label}),{target:{value}});
    await userEvent.click(await readySave());expect(recipeApi.save).toHaveBeenCalledWith(expect.objectContaining({id:"NEW-A",name:"新产品",productCode:42,spacing:.5,filterWindow:7,maxGapLen:1.2}),"A");expect(onSaved).toHaveBeenCalledWith("NEW-A");
  });

  it("已有相机不存在时保留引用并标明，可改选有效工位",async()=>{
    const {onDraftChange}=show(undefined,{...workspaceView().workspace.doc,camera:"MISSING"});
    expect(screen.getByRole("option",{name:"MISSING（不在相机组里）"})).toBeVisible();
    await userEvent.selectOptions(screen.getByRole("combobox",{name:"相机"}),"CAM-1");expect(onDraftChange.mock.lastCall![0].camera).toBe("CAM-1");
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
