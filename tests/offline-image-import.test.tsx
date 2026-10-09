import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ImageImportButton from "../src/features/workspace/ImageImportButton";
import StationCapture from "../src/features/workspace/StationCapture";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { workspaceApi } from "../src/features/workspace/api";
import { useWorkspace } from "../src/features/workspace/context";
import { useCycle } from "../src/features/cycle/api";
import { deferred, snapshot, tricamWorkspaceView, workspaceState, workspaceView } from "./fixtures";

vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { stationCapture: vi.fn(), stationImport: vi.fn(), stationImage: vi.fn(), importImage: vi.fn(), image: vi.fn(), capture: vi.fn() } }));
vi.mock("../src/features/workspace/context", () => ({ useWorkspace: vi.fn() }));
vi.mock("../src/features/cycle/api", () => ({ useCycle: vi.fn() }));
const metadata = { ...workspaceView().workspace.frames[0].image!, source: "import", exposureUs: null, gainDb: null };
const gray = { url: "data:image/png;base64,AA==", width: 100, height: 60 };
let ws: ReturnType<typeof workspaceState>;
export function file(name = "offline.pgm", bytes = [80, 53, 10, 0, 255]) {
  const value = new File([Uint8Array.from(bytes)], name, { type: "image/x-portable-graymap" });
  Object.defineProperty(value, "arrayBuffer", { value: vi.fn(async () => Uint8Array.from(bytes).buffer) }); return value;
}
function upload(value = file()) { fireEvent.change(screen.getByLabelText("导入离线原图"), { target: { files: [value] } }); }
beforeEach(() => {
  ws = workspaceState(); vi.mocked(useWorkspace).mockImplementation(() => ws);
  vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(), logs: [], measured: [] });
  vi.mocked(workspaceApi.stationCapture).mockResolvedValue(metadata); vi.mocked(workspaceApi.stationImport).mockResolvedValue(metadata);
  vi.mocked(workspaceApi.stationImage).mockResolvedValue(gray); vi.mocked(workspaceApi.image).mockResolvedValue(gray);
  vi.mocked(workspaceApi.importImage).mockResolvedValue(ws.data!);
});
describe("离线原图入口", () => {
  it("导入二进制原图，按钮打开文件选择并允许再次选择同一文件", async () => {
    const onImport = vi.fn(async () => {}), onError = vi.fn();
    render(<ImageImportButton scope="test" onImport={onImport} onError={onError}/>);
    const input = screen.getByLabelText("导入离线原图"); const picker = vi.spyOn(input, "click");
    await userEvent.click(screen.getByRole("button", { name: "导入离线原图" })); expect(picker).toHaveBeenCalledTimes(1);
    const value = file(); upload(value); await waitFor(() => expect(onImport).toHaveBeenCalledWith([80, 53, 10, 0, 255]));
    await waitFor(() => expect(screen.getByRole("button", { name: "导入离线原图" })).toBeEnabled()); upload(value);
    await waitFor(() => expect(onImport).toHaveBeenCalledTimes(2)); expect(onError).not.toHaveBeenCalled();
  });
  it.each(["bad.txt", "empty", "oversized"])("拒绝 %s，保留可重试入口", async kind => {
    const onImport = vi.fn(async () => {}), onError = vi.fn(); render(<ImageImportButton scope="test" onImport={onImport} onError={onError}/>);
    const value = file(kind === "bad.txt" ? kind : "offline.png", kind === "empty" ? [] : [1]);
    if (kind === "oversized") Object.defineProperty(value, "size", { value: 15_000_001 }); upload(value);
    await waitFor(() => expect(onError).toHaveBeenCalled()); expect(onImport).not.toHaveBeenCalled(); expect(screen.getByRole("button", { name: "导入离线原图" })).toBeEnabled();
  });
  it("读取期间锁定，切换工位或配方后丢弃旧文件结果", async () => {
    const pending = deferred<ArrayBuffer>(); const value = file(); vi.mocked(value.arrayBuffer).mockReturnValueOnce(pending.promise);
    const onImport = vi.fn(async () => {}), onError = vi.fn(); const page = render(<ImageImportButton scope="old" onImport={onImport} onError={onError}/>);
    upload(value); expect(screen.getByRole("button", { name: "正在导入…" })).toBeDisabled();
    page.rerender(<ImageImportButton scope="new" onImport={onImport} onError={onError}/>);
    await act(async () => pending.resolve(Uint8Array.from([1]).buffer)); expect(onImport).not.toHaveBeenCalled(); expect(onError).not.toHaveBeenCalled();
  });
  it("导入命令失败报告原因，读取期间重复选择只执行一次", async () => {
    const request = deferred<void>(), onError = vi.fn(); const onImport = vi.fn(() => request.promise);
    render(<ImageImportButton scope="test" onImport={onImport} onError={onError}/>);
    upload(); upload(); await waitFor(() => expect(onImport).toHaveBeenCalledTimes(1));
    await act(async () => request.reject(new Error("原图损坏"))); expect(onError).toHaveBeenCalledWith("Error: 原图损坏");
  });
  it("报告读取及导入的整个等待过程，父级禁用按钮不会取消正在读取的文件", async () => {
    const bytes=deferred<ArrayBuffer>(),command=deferred<void>(),value=file();vi.mocked(value.arrayBuffer).mockReturnValue(bytes.promise);
    const onImport=vi.fn(()=>command.promise),onError=vi.fn(),onReadingChange=vi.fn();
    const page=render(<ImageImportButton scope="same" onImport={onImport} onError={onError} onReadingChange={onReadingChange}/>);
    upload(value);expect(onReadingChange).toHaveBeenLastCalledWith(true);
    page.rerender(<ImageImportButton scope="same" disabled onImport={onImport} onError={onError} onReadingChange={onReadingChange}/>);
    await act(async()=>bytes.resolve(Uint8Array.from([0,255]).buffer));expect(onImport).toHaveBeenCalledWith([0,255]);
    expect(onReadingChange).toHaveBeenCalledTimes(1);await act(async()=>command.resolve());
    expect(onReadingChange.mock.calls).toEqual([[true],[false]]);expect(onError).not.toHaveBeenCalled();
  });
  it("切换作用域解除旧读取，新读取期间旧文件失败不能解锁或报错", async () => {
    const old=deferred<ArrayBuffer>(),next=deferred<ArrayBuffer>(),first=file(),second=file("new.pgm");
    vi.mocked(first.arrayBuffer).mockReturnValue(old.promise);vi.mocked(second.arrayBuffer).mockReturnValue(next.promise);
    const onImport=vi.fn(async()=>{}),onError=vi.fn(),onReadingChange=vi.fn();
    const page=render(<ImageImportButton scope="old" onImport={onImport} onError={onError} onReadingChange={onReadingChange}/>);upload(first);
    page.rerender(<ImageImportButton scope="new" onImport={onImport} onError={onError} onReadingChange={onReadingChange}/>);
    expect(onReadingChange.mock.calls).toEqual([[true],[false]]);upload(second);
    await act(async()=>old.reject(new Error("旧读取失败")));expect(onError).not.toHaveBeenCalled();
    expect(screen.getByRole("button",{name:"正在导入…"})).toBeDisabled();expect(onReadingChange).toHaveBeenLastCalledWith(true);
    await act(async()=>next.resolve(Uint8Array.from([2]).buffer));expect(onImport).toHaveBeenCalledExactlyOnceWith([2]);
    expect(onReadingChange.mock.calls).toEqual([[true],[false],[true],[false]]);
  });
  it("卸载时解除读状态并丢弃晚到的文件", async()=>{
    const bytes=deferred<ArrayBuffer>(),value=file();vi.mocked(value.arrayBuffer).mockReturnValue(bytes.promise);
    const onImport=vi.fn(async()=>{}),onReadingChange=vi.fn(),onError=vi.fn();
    const page=render(<ImageImportButton scope="test" onImport={onImport} onError={onError} onReadingChange={onReadingChange}/>);
    upload(value);page.unmount();expect(onReadingChange.mock.calls).toEqual([[true],[false]]);
    await act(async()=>bytes.resolve(Uint8Array.from([1]).buffer));expect(onImport).not.toHaveBeenCalled();
  });
  it("文件读取失败后解除等待并允许重新选择",async()=>{
    const bad=file();vi.mocked(bad.arrayBuffer).mockRejectedValue(new Error("读取被拒绝"));
    const onImport=vi.fn(async()=>{}),onError=vi.fn(),onReadingChange=vi.fn();
    render(<ImageImportButton scope="test" onImport={onImport} onError={onError} onReadingChange={onReadingChange}/>);upload(bad);
    await waitFor(()=>expect(onError).toHaveBeenCalledWith("Error: 读取被拒绝"));expect(onReadingChange).toHaveBeenLastCalledWith(false);
    upload();await waitFor(()=>expect(onImport).toHaveBeenCalledTimes(1));
  });
});
describe("标定与示教的原图绑定", () => {
  it("标定导入绑定所选工位，再按新样本 ID 读取真实预览", async () => {
    const onSample = vi.fn(); render(<StationCapture cam={2} sample={null} onSample={onSample}/>); upload();
    await waitFor(() => expect(onSample).toHaveBeenLastCalledWith({ metadata, image: gray }));
    expect(workspaceApi.stationImport).toHaveBeenCalledWith(2, [80, 53, 10, 0, 255]); expect(workspaceApi.stationImage).toHaveBeenCalledWith(2, metadata.id);
  });
  it("取样失败显示原因，旧工位预览晚到不能替换新工位样本", async () => {
    const request = deferred<typeof gray>(); vi.mocked(workspaceApi.stationImage).mockReturnValueOnce(request.promise);
    const onSample = vi.fn(); const page = render(<StationCapture cam={0} sample={null} onSample={onSample}/>);
    await userEvent.click(screen.getByRole("button", { name: "取新样本" })); page.unmount();
    await act(async () => request.resolve(gray)); expect(onSample).toHaveBeenCalledTimes(1); expect(onSample).toHaveBeenCalledWith(null);
  });
  it("标定读文件期间禁用取样，导入命令等待期间也不能重复取样",async()=>{
    const bytes=deferred<ArrayBuffer>(),command=deferred<typeof metadata>(),value=file();vi.mocked(value.arrayBuffer).mockReturnValue(bytes.promise);
    vi.mocked(workspaceApi.stationImport).mockReturnValueOnce(command.promise);const onSample=vi.fn();
    render(<StationCapture cam={0} sample={null} onSample={onSample}/>);upload(value);
    expect(screen.getByRole("button",{name:"取新样本"})).toBeDisabled();await userEvent.click(screen.getByRole("button",{name:"取新样本"}));
    expect(workspaceApi.stationCapture).not.toHaveBeenCalled();await act(async()=>bytes.resolve(Uint8Array.from([1]).buffer));
    expect(screen.getByRole("button",{name:"取样中…"})).toBeDisabled();await act(async()=>command.resolve(metadata));
    expect(onSample).toHaveBeenLastCalledWith({metadata,image:gray});expect(screen.getByRole("button",{name:"取新样本"})).toBeEnabled();
  });
  it.each(["stationCapture","stationImage"] as const)("标定 %s 失败显示原因并允许重试",async method=>{
    vi.mocked(workspaceApi[method]).mockRejectedValueOnce(new Error("取图失败"));const onSample=vi.fn();
    render(<StationCapture cam={1} sample={null} onSample={onSample}/>);await userEvent.click(screen.getByRole("button",{name:"取新样本"}));
    expect((await screen.findAllByText("Error: 取图失败"))[0]).toBeVisible();expect(screen.getByRole("button",{name:"取新样本"})).toBeEnabled();
    await userEvent.click(screen.getByRole("button",{name:"取新样本"}));await waitFor(()=>expect(onSample).toHaveBeenLastCalledWith({metadata,image:gray}));
    expect(screen.queryAllByText("Error: 取图失败")).toHaveLength(0);
  });
  it("同一组件切换工位后允许新取样，旧请求完成不会解锁新取样",async()=>{
    const old=deferred<typeof metadata>(),next=deferred<typeof metadata>();
    vi.mocked(workspaceApi.stationCapture).mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise);
    const onSample=vi.fn(),page=render(<StationCapture cam={0} sample={null} onSample={onSample}/>);
    fireEvent.click(screen.getByRole("button",{name:"取新样本"}));page.rerender(<StationCapture cam={2} sample={null} onSample={onSample}/>);
    expect(screen.getByRole("button",{name:"取新样本"})).toBeEnabled();fireEvent.click(screen.getByRole("button",{name:"取新样本"}));
    await act(async()=>old.resolve(metadata));expect(workspaceApi.stationImage).not.toHaveBeenCalled();
    expect(screen.getByRole("button",{name:"取样中…"})).toBeDisabled();fireEvent.click(screen.getByRole("button",{name:"取样中…"}));
    expect(workspaceApi.stationCapture).toHaveBeenCalledTimes(2);await act(async()=>next.resolve({...metadata,id:"new-station"}));
    expect(workspaceApi.stationImage).toHaveBeenCalledExactlyOnceWith(2,"new-station");
    expect(onSample).toHaveBeenLastCalledWith({metadata:{...metadata,id:"new-station"},image:gray});
  });
  it("示教导入绑定候选修订与所选帧，文件错误不执行后端", async () => {
    render(<MemoryRouter initialEntries={["/recipe/teach?frame=1"]}><TeachingPage/></MemoryRouter>); upload();
    await waitFor(() => expect(workspaceApi.importImage).toHaveBeenCalledWith("A", 7, 1, [80, 53, 10, 0, 255]));
    upload(file("invalid.txt")); await waitFor(() => expect(ws.setError).toHaveBeenCalledWith(expect.stringContaining("请选择")));
    expect(workspaceApi.importImage).toHaveBeenCalledTimes(1);
  });
  it("示教读取本帧文件期间锁定帧、中线参数和取样，导入后恢复",async()=>{
    ws=workspaceState(tricamWorkspaceView());
    const bytes=deferred<ArrayBuffer>(),value=file();vi.mocked(value.arrayBuffer).mockReturnValue(bytes.promise);
    render(<MemoryRouter><TeachingPage/></MemoryRouter>);upload(value);
    const next=screen.getByRole("button",{name:"选择帧 k2"});expect(next).toBeDisabled();
    expect(screen.getByRole("button",{name:"取新样本"})).toBeDisabled();expect(screen.getByRole("spinbutton",{name:"像素当量"})).toBeDisabled();expect(screen.getByRole("button",{name:"清空中线"})).toBeDisabled();
    expect(screen.getByRole("button",{name:"选择视角 2"})).toBeDisabled();expect(screen.getByRole("button",{name:"选择视角 3"})).toBeDisabled();
    await userEvent.click(next);expect(screen.getByRole("button",{name:"选择帧 k1"})).toHaveAttribute("aria-pressed","true");
    await act(async()=>bytes.resolve(Uint8Array.from([7]).buffer));expect(workspaceApi.importImage).toHaveBeenCalledExactlyOnceWith("A",7,0,[7]);
    await waitFor(()=>expect(next).toBeEnabled());
    expect(screen.getByRole("button",{name:"选择视角 2"})).toBeEnabled();
  });
  it("示教候选切换解除旧读取，新候选的文件读取不会被旧完成打断",async()=>{
    const old=deferred<ArrayBuffer>(),next=deferred<ArrayBuffer>(),first=file(),second=file("new.png");
    vi.mocked(first.arrayBuffer).mockReturnValue(old.promise);vi.mocked(second.arrayBuffer).mockReturnValue(next.promise);
    const page=render(<MemoryRouter><TeachingPage/></MemoryRouter>);upload(first);
    ws=workspaceState(workspaceView("B"));page.rerender(<MemoryRouter><TeachingPage/></MemoryRouter>);upload(second);
    await act(async()=>old.resolve(Uint8Array.from([1]).buffer));expect(workspaceApi.importImage).not.toHaveBeenCalled();
    expect(screen.getByRole("button",{name:"选择帧 k2"})).toBeDisabled();await act(async()=>next.resolve(Uint8Array.from([2]).buffer));
    expect(workspaceApi.importImage).toHaveBeenCalledExactlyOnceWith("B",7,0,[2]);
  });
  it.each(["busy", "dirty", "production"])("%s 状态禁用离线导入", condition => {
    if (condition === "busy") ws.busy = true; if (condition === "dirty") ws.dirty = true;
    if (condition === "production") vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot("ACQUIRE"), logs: [], measured: [] });
    render(<MemoryRouter><TeachingPage/></MemoryRouter>); expect(screen.getByRole("button", { name: "导入离线原图" })).toBeDisabled();
  });
});
