import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cameraApi, usePreview } from "../src/features/camera/api";
import type { PreviewImage } from "../src/features/camera/types";
import { deferred } from "./fixtures";

const image=(width:number):PreviewImage=>({width,height:10,fullWidth:width,fullHeight:10,data:{} as ImageData});
beforeEach(()=>{vi.useFakeTimers();vi.setSystemTime(new Date("2026-10-09T00:00:00Z"));vi.spyOn(cameraApi,"preview").mockResolvedValue(image(100));});
afterEach(()=>vi.useRealTimers());
const tick=async(ms=0)=>act(()=>vi.advanceTimersByTimeAsync(ms));
describe("相机预览的图像源与限速",()=>{
  it("收到新帧才读取预览，同一源按最小间隔合并刷新",async()=>{
    const page=renderHook(({frame})=>usePreview(0,frame,250,"source-a"),{initialProps:{frame:1}});
    await tick();expect(cameraApi.preview).toHaveBeenCalledTimes(1);expect(page.result.current?.width).toBe(100);
    page.rerender({frame:2});await tick(100);page.rerender({frame:3});await tick(149);expect(cameraApi.preview).toHaveBeenCalledTimes(1);
    await tick(1);expect(cameraApi.preview).toHaveBeenCalledTimes(2);
    await tick(500);expect(cameraApi.preview).toHaveBeenCalledTimes(2);
  });
  it("持续帧到达时仍接受同一源正在读取的图像，避免永久空白",async()=>{
    const request=deferred<PreviewImage|null>();vi.mocked(cameraApi.preview).mockReturnValue(request.promise);
    const page=renderHook(({frame})=>usePreview(0,frame,250,"source-a"),{initialProps:{frame:1}});await tick();
    page.rerender({frame:2});await tick(250);page.rerender({frame:3});await tick(250);expect(cameraApi.preview).toHaveBeenCalledTimes(1);
    await act(async()=>request.resolve(image(200)));expect(page.result.current?.width).toBe(200);
    vi.mocked(cameraApi.preview).mockResolvedValue(image(201));page.rerender({frame:4});await tick();expect(page.result.current?.width).toBe(201);
  });
  it("同工位换源不被旧请求拦住，旧响应不会覆盖新源图像",async()=>{
    const old=deferred<PreviewImage|null>(),next=deferred<PreviewImage|null>();vi.mocked(cameraApi.preview).mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise);
    const page=renderHook(({source})=>usePreview(0,1,250,source),{initialProps:{source:"mvs"}});await tick();
    page.rerender({source:"replay:D:/new:2"});expect(page.result.current).toBeNull();await tick();expect(cameraApi.preview).toHaveBeenCalledTimes(2);
    await act(async()=>next.resolve(image(300)));await act(async()=>old.resolve(image(100)));expect(page.result.current?.width).toBe(300);
  });
  it("旧源完成不能解除当前读取锁，同源下一帧仍合并等待",async()=>{
    const old=deferred<PreviewImage|null>(),next=deferred<PreviewImage|null>();vi.mocked(cameraApi.preview).mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise);
    const page=renderHook(({source,frame})=>usePreview(0,frame,250,source),{initialProps:{source:"old",frame:1}});await tick();page.rerender({source:"new",frame:1});await tick();
    await act(async()=>old.resolve(image(100)));page.rerender({source:"new",frame:2});await tick(500);expect(cameraApi.preview).toHaveBeenCalledTimes(2);
    await act(async()=>next.resolve(image(300)));expect(page.result.current?.width).toBe(300);
  });
  it("切换回来仍忽略上一轮相同来源的迟到响应",async()=>{
    const old=deferred<PreviewImage|null>();vi.mocked(cameraApi.preview).mockReturnValueOnce(old.promise).mockResolvedValueOnce(image(200)).mockResolvedValueOnce(image(300));
    const page=renderHook(({source})=>usePreview(0,1,250,source),{initialProps:{source:"a"}});await tick();page.rerender({source:"b"});await tick();page.rerender({source:"a"});await tick();
    await act(async()=>old.resolve(image(100)));expect(page.result.current?.width).toBe(300);
  });
  it("切换相机立即隐藏旧图，不等前一台的刷新间隔",async()=>{
    const page=renderHook(({cam})=>usePreview(cam,1),{initialProps:{cam:0}});await tick();expect(page.result.current?.width).toBe(100);
    vi.mocked(cameraApi.preview).mockResolvedValue(image(500));page.rerender({cam:1});expect(page.result.current).toBeNull();await tick();expect(cameraApi.preview).toHaveBeenLastCalledWith(1);expect(page.result.current?.width).toBe(500);
  });
  it("预览读取失败不生成虚假图像，下一帧可重新请求；空返回清掉旧图",async()=>{
    vi.mocked(cameraApi.preview).mockRejectedValueOnce(new Error("图像丢失"));
    const page=renderHook(({frame})=>usePreview(0,frame),{initialProps:{frame:1}});await tick();expect(page.result.current).toBeNull();
    page.rerender({frame:2});await tick(250);expect(page.result.current?.width).toBe(100);
    vi.mocked(cameraApi.preview).mockResolvedValueOnce(null);page.rerender({frame:3});await tick(250);expect(page.result.current).toBeNull();
  });
  it("卸载后迟到取图结果不继续更新或请求",async()=>{
    const request=deferred<PreviewImage|null>();vi.mocked(cameraApi.preview).mockReturnValue(request.promise);
    const page=renderHook(()=>usePreview(0,1));await tick();page.unmount();await act(async()=>request.resolve(image(900)));await tick(1000);expect(cameraApi.preview).toHaveBeenCalledTimes(1);
  });
});
