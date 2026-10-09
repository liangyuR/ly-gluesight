import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import FramePreview from "../src/features/camera/components/FramePreview";
import { cameraApi, defaultCameraConfig, usePreviewCanvas } from "../src/features/camera/api";
import type { CameraConfig, CameraStatus, Frame, PreviewImage } from "../src/features/camera/types";
import { deferred } from "./fixtures";

const preview=vi.hoisted(()=>({img:null as PreviewImage|null,canvas:{current:null}}));
vi.mock("../src/features/camera/api",async importOriginal=>{
  const actual=await importOriginal<typeof import("../src/features/camera/api")>();
  return {...actual,cameraApi:{...actual.cameraApi,softTrigger:vi.fn()},usePreviewCanvas:vi.fn(()=>preview)};
});
let config:CameraConfig,status:CameraStatus;
beforeEach(()=>{
  config={...defaultCameraConfig,id:"CAM-1",source:"replay",replayDir:"D:/images"};
  status={cam:0,id:"CAM-1",name:"相机 1",source:"replay",acquisition:"triggered",ready:true,message:"",device:null,sdkVersion:null,frames:1,fps:0,maxFps:null,lostPackets:0,droppedFrames:0,warnings:[]};
  preview.img=null;vi.mocked(cameraApi.softTrigger).mockResolvedValue(undefined);
});
const frame:Frame={cam:0,frameCounter:8,triggerCounter:7,lostPackets:3,ts:1};
const show=()=>render(<FramePreview cam={0} config={config} status={status} lastFrame={frame}/>);
describe("最新相机帧与触发",()=>{
  it("三目预览按所选视角读图，切换设备或恢复单视角时回到视角 1",async()=>{
    config={...config,viewCount:3};const page=show();
    expect(screen.getByRole("combobox",{name:"预览视角"})).toHaveValue("1");
    await userEvent.selectOptions(screen.getByRole("combobox",{name:"预览视角"}),"3");
    expect(usePreviewCanvas).toHaveBeenLastCalledWith(0,8,250,expect.any(String),3);
    page.rerender(<FramePreview cam={1} config={{...config,id:"CAM-2"}} status={{...status,cam:1}} lastFrame={frame}/>);
    expect(screen.getByRole("combobox",{name:"预览视角"})).toHaveValue("1");
    expect(usePreviewCanvas).toHaveBeenLastCalledWith(1,8,250,expect.any(String),1);
    page.rerender(<FramePreview cam={1} config={{...config,id:"CAM-2",viewCount:1}} status={{...status,cam:1}} lastFrame={frame}/>);
    expect(screen.queryByRole("combobox",{name:"预览视角"})).toBeNull();
    expect(usePreviewCanvas).toHaveBeenLastCalledWith(1,8,250,expect.any(String),1);
  });
  it("回放下一张绑定工位，请求中禁止重复取图，完成解锁",async()=>{
    const request=deferred<void>();vi.mocked(cameraApi.softTrigger).mockReturnValue(request.promise);
    show();await userEvent.click(screen.getByRole("button",{name:"下一张"}));
    expect(cameraApi.softTrigger).toHaveBeenCalledWith(0);expect(screen.getByRole("button",{name:"取图中…"})).toBeDisabled();
    fireEvent.click(screen.getByRole("button",{name:"取图中…"}));expect(cameraApi.softTrigger).toHaveBeenCalledTimes(1);
    await act(async()=>request.resolve());expect(screen.getByRole("button",{name:"下一张"})).toBeEnabled();
  });
  it("取图失败反馈原因，重试时清除旧错误",async()=>{
    vi.mocked(cameraApi.softTrigger).mockRejectedValueOnce(new Error("目录没有图片"));show();
    await userEvent.click(screen.getByRole("button",{name:"下一张"}));expect(await screen.findByText("Error: 目录没有图片")).toBeVisible();
    await userEvent.click(screen.getByRole("button",{name:"下一张"}));expect(screen.queryByText("Error: 目录没有图片")).not.toBeInTheDocument();expect(cameraApi.softTrigger).toHaveBeenCalledTimes(2);
  });
  it.each([
    {source:"sim",acquisition:"triggered",triggerSource:"Software",ready:true},
    {source:"mvs",acquisition:"triggered",triggerSource:"Line0",ready:true},
    {source:"mvs",acquisition:"freeRun",triggerSource:"Software",ready:true},
    {source:"replay",acquisition:"triggered",triggerSource:"Software",ready:false},
  ] as const)("不支持或未连接的 $source / $acquisition 禁用触发",async values=>{
    config={...config,...values};status={...status,ready:values.ready};show();
    const button=screen.getByRole("button",{name:values.source==="replay"?"下一张":"软触发一次"});
    expect(button).toBeDisabled();await userEvent.click(button);expect(cameraApi.softTrigger).not.toHaveBeenCalled();
  });
  it("Software 触发的海康工位允许软触发，显示帧与原图尺寸",async()=>{
    config={...config,source:"mvs",triggerSource:"Software"};
    preview.img={width:10,height:8,fullWidth:1280,fullHeight:1024,data:{} as ImageData};show();
    expect(screen.getByText(/帧 8 · 触发 7 · 丢包 3 · 1280×1024/)).toBeVisible();expect(screen.getByRole("img",{name:"相机最新图像"})).toBeVisible();
    await userEvent.click(screen.getByRole("button",{name:"软触发一次"}));expect(cameraApi.softTrigger).toHaveBeenCalledWith(0);
  });
  it("切换工位后旧触发失败不能污染新工位或解除其取图锁",async()=>{
    const old=deferred<void>(),current=deferred<void>();vi.mocked(cameraApi.softTrigger).mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
    const page=show();await userEvent.click(screen.getByRole("button",{name:"下一张"}));
    page.rerender(<FramePreview cam={1} config={{...config,id:"CAM-2"}} status={{...status,cam:1}} lastFrame={undefined}/>);
    await userEvent.click(screen.getByRole("button",{name:"下一张"}));await act(async()=>old.reject(new Error("旧工位断开")));
    expect(screen.queryByText("Error: 旧工位断开")).not.toBeInTheDocument();expect(screen.getByRole("button",{name:"取图中…"})).toBeDisabled();
    await act(async()=>current.resolve());expect(screen.getByRole("button",{name:"下一张"})).toBeEnabled();
  });
  it("未连接且没有状态时显示等待提示，隐藏空画布",()=>{
    render(<FramePreview cam={0} config={null} status={null} lastFrame={undefined}/>);
    expect(screen.getByText("等待相机连接")).toBeVisible();expect(document.querySelector("canvas")).not.toBeVisible();
  });
});
