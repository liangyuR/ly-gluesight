import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ValidationPage from "../src/features/workspace/ValidationPage";
import { useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { historyApi } from "../src/features/history/api";
import { deferred, partSummary, workspaceState, workspaceView } from "./fixtures";

vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { validate: vi.fn(), publish: vi.fn(), importSample: vi.fn() } }));
vi.mock("../src/features/history/api", () => ({ historyApi: { query: vi.fn() } }));
let ws: ReturnType<typeof workspaceState>;
const show = () => render(<MemoryRouter><ValidationPage /></MemoryRouter>);
beforeEach(() => {
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(historyApi.query).mockResolvedValue({ items: [], total: 0, counts: { ok: 0, ng: 0, err: 0, excursion: 0 } });
  vi.mocked(workspaceApi.validate).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.publish).mockResolvedValue(ws.data!);
  vi.mocked(workspaceApi.importSample).mockResolvedValue(ws.data!);
});

function imageFile(name="sample.pgm",bytes=[80,53,10,0,255]) {
  const file=new File([Uint8Array.from(bytes)],name);
  Object.defineProperty(file,"arrayBuffer",{value:vi.fn(async()=>Uint8Array.from(bytes).buffer)});
  return file;
}
async function openImport(){await userEvent.click(screen.getByRole("button",{name:"导入原图样本组"}));}
function selectFile(k:number,file=imageFile()){fireEvent.change(screen.getByLabelText(`k${k} 原图`),{target:{files:[file]}});}
function changeCandidate(page:ReturnType<typeof show>,change:"recipe"|"revision"|"return"){
  if(change==="revision")ws.data!.workspace.revision++;
  else ws=workspaceState(workspaceView("B"));
  page.rerender(<MemoryRouter><ValidationPage/></MemoryRouter>);
  if(change==="return"){
    ws=workspaceState(workspaceView("A"));
    page.rerender(<MemoryRouter><ValidationPage/></MemoryRouter>);
  }
}

describe("验证与发布页面", () => {
  it("尚未验证时列出发布前检查：胶路示教按要检拍照点算，不检的拍照点不用保存示教", () => {
    ws.data!.workspace.validation = null; ws.data!.coverage = 50;
    ws.data!.workspace.doc.shots[1].skip = true; ws.data!.workspace.frames[0].saved = true;
    show();
    const rows = Array.from(document.querySelectorAll(".wp-check-row")).map(r => [r.querySelector("strong")!.textContent, r.querySelector("p")!.textContent, r.classList.contains("passed")]);
    expect(rows).toEqual([
      ["设备与采集", "验证时读取实际相机状态与采集方式", false],
      ["胶路示教", "要检的拍照点里已示教中线 50%", false],
      ["示教与标定", "已保存 1/1 帧（不检的拍照点不用示教）", true],
      ["代表性样本", "至少选择一件合格样本和一件缺陷样本", false],
    ]);
    expect(screen.queryByText("物理覆盖")).toBeNull();
  });

  it.each(["dirty", "frame-dirty", "busy", "failed", "stale", "pending"])("%s 禁止发布", condition => {
    if (condition === "dirty") ws.dirty = true;
    if (condition === "frame-dirty") ws.frameDirty = true;
    if (condition === "busy") ws.busy = true;
    if (condition === "failed") ws.data!.workspace.validation!.passed = false;
    if (condition === "stale") ws.data!.workspace.validation!.revision = 6;
    if (condition === "pending") ws.data!.workspace.pending = { doc: ws.doc!, revision: 7, baseHash: "old", frames: [], overview: ws.data!.workspace.overview, validation: ws.data!.workspace.validation! };
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

  it("发布失败保留确认弹窗", async () => {
    vi.mocked(ws.act).mockResolvedValueOnce(null); show();
    await userEvent.click(screen.getByRole("button", { name: "发布生产配方" }));
    await userEvent.click(screen.getByRole("button", { name: "确认发布 v2" }));
    expect(screen.getByRole("dialog")).toBeVisible();
  });

  it("改变样本期望使验证失效，重验携带修改后的期望", async () => {
    show(); await userEvent.selectOptions(screen.getByRole("combobox", { name: "良品组期望结论" }), "NG_WIDTH");
    expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
    expect(screen.getByText("样本选用或期望已改变")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "运行规则与图像验证" }));
    expect(workspaceApi.validate).toHaveBeenCalledWith("A", 7, expect.arrayContaining([{ historyId: null, sampleId: "good", expected: "NG_WIDTH" }]));
  });

  it("取消所有样本后不能运行验证，未选样本不能编辑期望", async () => {
    show(); await userEvent.click(screen.getByRole("checkbox", { name: "选用样本 良品组" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "选用样本 断胶组" }));
    expect(screen.getByRole("button", { name: "运行规则与图像验证" })).toBeDisabled();
    expect(screen.getByRole("combobox", { name: "良品组期望结论" })).toBeDisabled();
  });

  it("连续导入样本保留此前勾选和人工期望，修订后仍需重新验证", async () => {
    const bad = ws.data!.workspace.sampleBank[1];
    ws.data!.workspace.sampleBank = [ws.data!.workspace.sampleBank[0]];
    ws.data!.workspace.samples = [];
    const page = show();
    await userEvent.click(screen.getByRole("checkbox", { name: "选用样本 良品组" }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "良品组期望结论" }), "NG_POSITION");
    vi.mocked(workspaceApi.importSample).mockImplementationOnce(async () => {
      ws.data = structuredClone(ws.data!);
      ws.data.workspace.sampleBank.push(bad);
      ws.data.workspace.revision++;
      return ws.data;
    });
    await openImport(); selectFile(1); selectFile(2);
    await userEvent.click(screen.getByRole("button", { name: "保存样本组" }));
    page.rerender(<MemoryRouter><ValidationPage /></MemoryRouter>);
    expect(screen.getByRole("checkbox", { name: "选用样本 良品组" })).toBeChecked();
    expect(screen.getByRole("combobox", { name: "良品组期望结论" })).toHaveValue("NG_POSITION");
    expect(screen.getByRole("checkbox", { name: "选用样本 断胶组" })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
    expect(screen.getByText("未通过或待重验")).toBeVisible();

    await userEvent.click(screen.getByRole("checkbox", { name: "选用样本 断胶组" }));
    const selected = [
      { historyId: null, sampleId: "good", expected: "NG_POSITION" },
      { historyId: null, sampleId: "bad", expected: "NG_GAP" },
    ];
    vi.mocked(workspaceApi.validate).mockImplementationOnce(async (_id, revision, samples) => {
      ws.data = structuredClone(ws.data!);
      ws.data.workspace.samples = samples;
      ws.data.workspace.validation = { ...ws.data.workspace.validation!, revision, passed: true };
      return ws.data;
    });
    await userEvent.click(screen.getByRole("button", { name: "运行规则与图像验证" }));
    expect(workspaceApi.validate).toHaveBeenCalledWith("A", 8, selected);
    page.rerender(<MemoryRouter><ValidationPage /></MemoryRouter>);
    expect(screen.getByRole("button", { name: "发布生产配方" })).toBeEnabled();
  });

  it("同一候选刷新保留主动取消的样本和期望修改", async () => {
    const page = show();
    await userEvent.click(screen.getByRole("checkbox", { name: "选用样本 断胶组" }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "良品组期望结论" }), "ERR_INSPECT");
    ws.data = structuredClone(ws.data!); ws.data.workspace.revision++;
    page.rerender(<MemoryRouter><ValidationPage /></MemoryRouter>);
    expect(screen.getByRole("checkbox", { name: "选用样本 断胶组" })).not.toBeChecked();
    expect(screen.getByRole("combobox", { name: "良品组期望结论" })).toHaveValue("ERR_INSPECT");
    expect(screen.getByRole("button", { name: "发布生产配方" })).toBeDisabled();
  });

  it("刷新时剔除已删除原图样本，之后出现同名样本不会自动重选", async () => {
    const removed = ws.data!.workspace.sampleBank[0], page = show();
    ws.data = structuredClone(ws.data!);
    ws.data.workspace.sampleBank = ws.data.workspace.sampleBank.filter(sample => sample.id !== removed.id);
    page.rerender(<MemoryRouter><ValidationPage /></MemoryRouter>);
    await userEvent.click(screen.getByRole("button", { name: "运行规则与图像验证" }));
    expect(workspaceApi.validate).toHaveBeenCalledWith("A", 7, [{ historyId: null, sampleId: "bad", expected: "NG_GAP" }]);
    ws.data = structuredClone(ws.data!); ws.data.workspace.sampleBank.push(removed);
    page.rerender(<MemoryRouter><ValidationPage /></MemoryRouter>);
    expect(screen.getByRole("checkbox", { name: "选用样本 良品组" })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "选用样本 断胶组" })).toBeChecked();
  });

  it.each(["recipe", "version"] as const)("%s 切换清除旧候选的临时样本选择", async change => {
    ws.data!.workspace.samples = [];
    const page = show();
    await userEvent.click(screen.getByRole("checkbox", { name: "选用样本 良品组" }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "良品组期望结论" }), "NG_POSITION");
    const next = workspaceView(change === "recipe" ? "B" : "A");
    next.workspace.samples = [];
    if (change === "version") next.workspace.doc.version++;
    ws = workspaceState(next);
    page.rerender(<MemoryRouter><ValidationPage /></MemoryRouter>);
    expect(screen.getByRole("checkbox", { name: "选用样本 良品组" })).not.toBeChecked();
    expect(screen.getByRole("combobox", { name: "良品组期望结论" })).toHaveValue("OK");
    expect(screen.getByRole("button", { name: "运行规则与图像验证" })).toBeDisabled();
  });

  it("历史来源切换改变查询范围", async () => {
    show(); await waitFor(() => expect(historyApi.query).toHaveBeenCalledWith({ recipeId: "A", limit: 50 }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "历史样本来源" }), "all");
    await waitFor(() => expect(historyApi.query).toHaveBeenLastCalledWith({ recipeId: undefined, limit: 50 }));
  });

  it("导入需要完整帧组，超大文件报错且不会导入", async () => {
    show(); await userEvent.click(screen.getByRole("button", { name: "导入原图样本组" }));
    expect(screen.getByRole("button", { name: "保存样本组" })).toBeDisabled();
    const files = screen.getByRole("dialog").querySelectorAll('input[type="file"]');
    const oversized = new File(["x"], "large.png", { type: "image/png" });
    Object.defineProperty(oversized, "size", { value: 15_000_001 });
    fireEvent.change(files[0], { target: { files: [oversized] } });
    expect(ws.setError).toHaveBeenCalledWith("Error: 原图不得为空，单图不得超过 15 MB");
    expect(workspaceApi.importSample).not.toHaveBeenCalled();
  });

  it.each(["bad.txt","empty","oversized"])("样本组拒绝 %s，并清除此前已选的该帧",async kind=>{
    show();await openImport();selectFile(1);selectFile(2);
    expect(screen.getByRole("button",{name:"保存样本组"})).toBeEnabled();
    const invalid=imageFile(kind==="bad.txt"?kind:"sample.png",kind==="empty"?[]:[1]);
    if(kind==="oversized")Object.defineProperty(invalid,"size",{value:15_000_001});
    selectFile(1,invalid);expect(ws.setError).toHaveBeenCalled();
    expect(screen.getByRole("button",{name:"保存样本组"})).toBeDisabled();
    expect(workspaceApi.importSample).not.toHaveBeenCalled();
    selectFile(1);expect(screen.getByRole("button",{name:"保存样本组"})).toBeEnabled();
  });

  it("整组按帧编号发送真实二进制、名称与人工期望，成功后关闭",async()=>{
    show();await openImport();selectFile(1,imageFile("a.PNG",[0,128,255]));selectFile(2,imageFile("b.tiff",[255,0]));
    fireEvent.change(screen.getByRole("textbox",{name:"样本名称"}),{target:{value:"  缺陷原图  "}});
    await userEvent.selectOptions(screen.getByRole("combobox",{name:"人工确认的期望结论"}),"NG_POSITION");
    await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    expect(workspaceApi.importSample).toHaveBeenCalledWith("A",7,"缺陷原图","NG_POSITION",[{k:0,bytes:[0,128,255]},{k:1,bytes:[255,0]}]);
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("必须填名称、完整帧组；取消并重新打开不保留旧文件",async()=>{
    show();await openImport();selectFile(1);
    expect(screen.getByRole("button",{name:"保存样本组"})).toBeDisabled();selectFile(2);
    fireEvent.change(screen.getByRole("textbox",{name:"样本名称"}),{target:{value:" "}});
    expect(screen.getByRole("button",{name:"保存样本组"})).toBeDisabled();
    await userEvent.click(screen.getByRole("button",{name:"取消"}));await openImport();
    expect(screen.getByRole("button",{name:"保存样本组"})).toBeDisabled();
    expect(screen.queryByText(/sample.pgm/)).not.toBeInTheDocument();
  });

  it("相同文件重新选择替换该帧，取消文件选择保留原选择",async()=>{
    show();await openImport();const same=imageFile();selectFile(1,same);selectFile(1,same);selectFile(2);
    fireEvent.change(screen.getByLabelText("k1 原图"),{target:{files:[]}});
    expect(screen.getByRole("button",{name:"保存样本组"})).toBeEnabled();
    await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    expect(same.arrayBuffer).toHaveBeenCalledTimes(1);
  });

  it("整组超过 80 MB 在读取前阻止，且允许重新选图",async()=>{
    ws.data!.workspace.frames=Array.from({length:6},(_,k)=>({...ws.data!.workspace.frames[0],k}));
    show();await openImport();
    const huge=imageFile();Object.defineProperty(huge,"size",{value:14_000_000});
    for(let k=1;k<=6;k++)selectFile(k,huge);
    await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    expect(ws.setError).toHaveBeenCalledWith("整组原图不得超过 80 MB");expect(huge.arrayBuffer).not.toHaveBeenCalled();
    expect(screen.getByRole("button",{name:"保存样本组"})).toBeEnabled();
  });

  it("读取与后端保存期间锁定输入和关闭方式，重复点击只导入一次",async()=>{
    const reading=deferred<ArrayBuffer>(),saving=deferred<typeof ws.data>();
    const first=imageFile();vi.mocked(first.arrayBuffer).mockReturnValueOnce(reading.promise);
    vi.mocked(workspaceApi.importSample).mockReturnValueOnce(saving.promise as ReturnType<typeof workspaceApi.importSample>);
    show();await openImport();selectFile(1,first);selectFile(2);
    fireEvent.click(screen.getByRole("button",{name:"保存样本组"}));fireEvent.click(screen.getByRole("button",{name:"正在导入…"}));
    expect(screen.getByRole("textbox",{name:"样本名称"})).toBeDisabled();expect(screen.getByRole("combobox",{name:"人工确认的期望结论"})).toBeDisabled();
    expect(screen.getByLabelText("k1 原图")).toBeDisabled();expect(screen.getByRole("button",{name:"取消"})).toBeDisabled();
    await userEvent.keyboard("{Escape}");expect(screen.getByRole("dialog")).toBeVisible();
    expect(workspaceApi.importSample).not.toHaveBeenCalled();
    await act(async()=>reading.resolve(Uint8Array.from([0,255]).buffer));
    expect(workspaceApi.importSample).toHaveBeenCalledTimes(1);await userEvent.click(screen.getByRole("button",{name:"关闭"}));
    expect(screen.getByRole("dialog")).toBeVisible();
    await act(async()=>saving.resolve(ws.data));await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it.each(["recipe","revision","unmount"])("%s 改变后旧读取不能调用导入",async change=>{
    const pending=deferred<ArrayBuffer>(),first=imageFile();vi.mocked(first.arrayBuffer).mockReturnValueOnce(pending.promise);
    const page=show();await openImport();selectFile(1,first);selectFile(2);await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    if(change==="unmount")page.unmount();else{
      if(change==="recipe")ws=workspaceState(workspaceView("B"));else ws.data!.workspace.revision++;
      page.rerender(<MemoryRouter><ValidationPage/></MemoryRouter>);
      await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    }
    await act(async()=>pending.resolve(Uint8Array.from([1]).buffer));
    expect(workspaceApi.importSample).not.toHaveBeenCalled();
  });

  it.each(["recipe","revision"] as const)("旧读取仍悬挂时，%s 切换后的候选立即可导入",async change=>{
    const oldReading=deferred<ArrayBuffer>(),oldFile=imageFile();vi.mocked(oldFile.arrayBuffer).mockReturnValueOnce(oldReading.promise);
    const page=show();await openImport();selectFile(1,oldFile);selectFile(2);
    await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    changeCandidate(page,change);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("button",{name:"导入原图样本组"})).toBeEnabled();
    expect(screen.getByRole("button",{name:"运行规则与图像验证"})).toBeEnabled();
    expect(screen.getByRole("combobox",{name:"历史样本来源"})).toBeEnabled();
    expect(screen.getByRole("checkbox",{name:"选用样本 良品组"})).toBeEnabled();
    await openImport();selectFile(1,imageFile("new-1.pgm",[7,8]));selectFile(2,imageFile("new-2.pgm",[9]));
    await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    expect(workspaceApi.importSample).toHaveBeenCalledTimes(1);
    expect(workspaceApi.importSample).toHaveBeenCalledWith(change==="recipe"?"B":"A",change==="recipe"?7:8,"代表性样本","OK",[{k:0,bytes:[7,8]},{k:1,bytes:[9]}]);
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await act(async()=>oldReading.resolve(Uint8Array.from([1]).buffer));
    expect(workspaceApi.importSample).toHaveBeenCalledTimes(1);
  });

  it.each(["recipe","revision","return"] as const)("%s 后旧读取成功与结束不解除新读取等待",async change=>{
    const oldReading=deferred<ArrayBuffer>(),newReading=deferred<ArrayBuffer>(),oldFile=imageFile(),newFile=imageFile();
    vi.mocked(oldFile.arrayBuffer).mockReturnValueOnce(oldReading.promise);vi.mocked(newFile.arrayBuffer).mockReturnValueOnce(newReading.promise);
    const oldError=ws.setError,page=show();await openImport();selectFile(1,oldFile);selectFile(2);await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    changeCandidate(page,change);await openImport();selectFile(1,newFile);selectFile(2);await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    await act(async()=>oldReading.resolve(Uint8Array.from([1]).buffer));
    expect(screen.getByRole("dialog")).toBeVisible();expect(screen.getByRole("textbox",{name:"样本名称"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"正在导入…"})).toBeDisabled();expect(screen.getByRole("button",{name:"取消"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"导入原图样本组"})).toBeDisabled();expect(workspaceApi.importSample).not.toHaveBeenCalled();
    expect(oldError).not.toHaveBeenCalled();expect(ws.setError).not.toHaveBeenCalled();
    await act(async()=>newReading.resolve(Uint8Array.from([2]).buffer));
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());expect(workspaceApi.importSample).toHaveBeenCalledTimes(1);
  });

  it.each(["recipe","revision","return"] as const)("%s 后旧读取错误与结束不污染新读取",async change=>{
    const oldReading=deferred<ArrayBuffer>(),newReading=deferred<ArrayBuffer>(),oldFile=imageFile(),newFile=imageFile();
    vi.mocked(oldFile.arrayBuffer).mockReturnValueOnce(oldReading.promise);vi.mocked(newFile.arrayBuffer).mockReturnValueOnce(newReading.promise);
    const oldError=ws.setError,page=show();await openImport();selectFile(1,oldFile);selectFile(2);await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    changeCandidate(page,change);await openImport();selectFile(1,newFile);selectFile(2);await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    await act(async()=>oldReading.reject(new Error("旧图读取失败")));
    expect(screen.getByRole("textbox",{name:"样本名称"})).toBeDisabled();expect(screen.getByLabelText("k1 原图")).toBeDisabled();
    expect(screen.getByRole("button",{name:"正在导入…"})).toBeDisabled();expect(screen.getByRole("button",{name:"取消"})).toBeDisabled();
    expect(oldError).not.toHaveBeenCalled();expect(ws.setError).not.toHaveBeenCalled();expect(workspaceApi.importSample).not.toHaveBeenCalled();
    await act(async()=>newReading.resolve(Uint8Array.from([2]).buffer));
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());expect(workspaceApi.importSample).toHaveBeenCalledTimes(1);
  });

  it.each(["success","error"])("旧验证 %s 不复位新候选的操作等待",async outcome=>{
    const oldView=ws.data!,oldValidation=deferred<typeof oldView>(),newValidation=deferred<typeof oldView>(),oldError=ws.setError;
    vi.mocked(workspaceApi.validate).mockReturnValueOnce(oldValidation.promise).mockReturnValueOnce(newValidation.promise);
    const page=show();await userEvent.click(screen.getByRole("button",{name:"运行规则与图像验证"}));
    changeCandidate(page,"recipe");expect(screen.getByRole("button",{name:"运行规则与图像验证"})).toBeEnabled();
    await userEvent.click(screen.getByRole("button",{name:"运行规则与图像验证"}));
    await act(async()=>{if(outcome==="success")oldValidation.resolve(oldView);else oldValidation.reject(new Error("旧验证失败"));});
    expect(screen.getByRole("button",{name:"验证中…"})).toBeDisabled();expect(screen.getByRole("button",{name:"发布生产配方"})).toBeDisabled();
    expect(screen.getByRole("checkbox",{name:"选用样本 良品组"})).toBeDisabled();expect(oldError).not.toHaveBeenCalled();expect(ws.setError).not.toHaveBeenCalled();
    expect(workspaceApi.validate).toHaveBeenCalledTimes(2);await act(async()=>newValidation.resolve(ws.data!));
    expect(screen.getByRole("button",{name:"运行规则与图像验证"})).toBeEnabled();
  });

  it.each(["success","error"])("回到同一候选后，旧发布 %s 不关闭或解锁新确认",async outcome=>{
    const oldView=ws.data!,oldPublishing=deferred<typeof oldView>(),newPublishing=deferred<typeof oldView>(),oldError=ws.setError;
    vi.mocked(workspaceApi.publish).mockReturnValueOnce(oldPublishing.promise).mockReturnValueOnce(newPublishing.promise);
    const page=show();await userEvent.click(screen.getByRole("button",{name:"发布生产配方"}));await userEvent.click(screen.getByRole("button",{name:"确认发布 v2"}));
    changeCandidate(page,"return");await userEvent.click(screen.getByRole("button",{name:"发布生产配方"}));await userEvent.click(screen.getByRole("button",{name:"确认发布 v2"}));
    await act(async()=>{if(outcome==="success")oldPublishing.resolve(oldView);else oldPublishing.reject(new Error("旧发布失败"));});
    expect(screen.getByRole("dialog",{name:"发布生产配方"})).toBeVisible();expect(screen.getByRole("button",{name:"确认发布 v2"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"取消"})).toBeDisabled();expect(oldError).not.toHaveBeenCalled();expect(ws.setError).not.toHaveBeenCalled();
    expect(workspaceApi.publish).toHaveBeenCalledTimes(2);await act(async()=>newPublishing.resolve(ws.data!));
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("读取或后端损坏错误可在同一窗口修正后重试",async()=>{
    const first=imageFile();vi.mocked(first.arrayBuffer).mockRejectedValueOnce(new Error("读取失败"));
    show();await openImport();selectFile(1,first);selectFile(2);await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    await waitFor(()=>expect(ws.setError).toHaveBeenCalledWith("Error: 读取失败"));
    expect(screen.getByRole("dialog")).toBeVisible();expect(screen.getByRole("button",{name:"保存样本组"})).toBeEnabled();
    vi.mocked(workspaceApi.importSample).mockRejectedValueOnce(new Error("k2 图像损坏"));
    await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));await waitFor(()=>expect(ws.setError).toHaveBeenCalledWith("Error: k2 图像损坏"));
    selectFile(2,imageFile("fixed.pgm"));await userEvent.click(screen.getByRole("button",{name:"保存样本组"}));
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("旧历史查询晚到或报错不会污染新范围",async()=>{
    const old=deferred<Awaited<ReturnType<typeof historyApi.query>>>();vi.mocked(historyApi.query).mockReturnValueOnce(old.promise);
    show();await userEvent.selectOptions(screen.getByRole("combobox",{name:"历史样本来源"}),"all");
    await act(async()=>old.resolve({items:[partSummary(1)],total:1,counts:{ok:0,ng:1,err:0,excursion:0}}));
    expect(screen.queryByRole("checkbox",{name:"选用历史 SN 101"})).not.toBeInTheDocument();
  });

  it("历史样本可选用并编辑人工期望，验证等待期间保护所有选用和发布",async()=>{
    vi.mocked(historyApi.query).mockResolvedValue({items:[partSummary(1)],total:1,counts:{ok:0,ng:1,err:0,excursion:0}});
    const pending=deferred<NonNullable<typeof ws.data>>();vi.mocked(workspaceApi.validate).mockReturnValueOnce(pending.promise);
    show();await screen.findByRole("checkbox",{name:"选用历史 SN 101"});
    await userEvent.click(screen.getByRole("checkbox",{name:"选用历史 SN 101"}));await userEvent.selectOptions(screen.getByRole("combobox",{name:"SN 101期望结论"}),"OK");
    fireEvent.click(screen.getByRole("button",{name:"运行规则与图像验证"}));
    fireEvent.click(screen.getByRole("button",{name:"验证中…"}));
    expect(workspaceApi.validate).toHaveBeenCalledTimes(1);expect(workspaceApi.validate).toHaveBeenCalledWith("A",7,expect.arrayContaining([{historyId:1,sampleId:null,expected:"OK"}]));
    expect(screen.getByRole("checkbox",{name:"选用历史 SN 101"})).toBeDisabled();expect(screen.getByRole("combobox",{name:"历史样本来源"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"发布生产配方"})).toBeDisabled();await act(async()=>pending.resolve(ws.data!));
  });

  it("发布提交单次并禁止取消；切换修订关闭旧确认",async()=>{
    const pending=deferred<NonNullable<typeof ws.data>>();vi.mocked(workspaceApi.publish).mockReturnValueOnce(pending.promise);
    show();await userEvent.click(screen.getByRole("button",{name:"发布生产配方"}));
    fireEvent.click(screen.getByRole("button",{name:"确认发布 v2"}));fireEvent.click(screen.getByRole("button",{name:"确认发布 v2"}));
    expect(workspaceApi.publish).toHaveBeenCalledTimes(1);expect(screen.getByRole("button",{name:"取消"})).toBeDisabled();
    await userEvent.keyboard("{Escape}");expect(screen.getByRole("dialog")).toBeVisible();await act(async()=>pending.resolve(ws.data!));
  });

  it("确认期间候选修订变更必须重新验证和确认",async()=>{
    const page=show();await userEvent.click(screen.getByRole("button",{name:"发布生产配方"}));
    ws.data!.workspace.revision++;page.rerender(<MemoryRouter><ValidationPage/></MemoryRouter>);
    await waitFor(()=>expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByRole("button",{name:"发布生产配方"})).toBeDisabled();expect(screen.getByText("未通过或待重验")).toBeVisible();
    expect(workspaceApi.publish).not.toHaveBeenCalled();
  });
});
