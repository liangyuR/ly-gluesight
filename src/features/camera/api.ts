import { invoke, isTauri } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { subscribe } from "../plc";
import type { CameraConfig, CameraStatus, DeviceSummary, DryFrame, Frame, PreviewImage, RecordEntry } from "./types";

function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: () => T): Promise<T> {
  if (!isTauri() && ["camera_save_config","camera_add","camera_remove","camera_soft_trigger","camera_dry_run_start","camera_dry_run_stop"].includes(cmd))
    return Promise.reject(new Error("设备操作需要 GlueSight · 胶路智检 桌面后端"));
  if (!isTauri()) return Promise.resolve(fallback());
  return invoke<T>(cmd, args);
}

export const defaultCameraConfig: CameraConfig = {
  id: "",
  name: "相机",
  source: "sim",
  serial: "",
  acquisition: "triggered",
  fps: 20,
  triggerSource: "Line0",
  triggerActivation: "RisingEdge",
  triggerDelayUs: 0,
  debouncerUs: 5,
  exposureUs: 60,
  gainDb: 6,
  strobe: true,
  chunk: true,
  replayDir: "",
  replayChannel: 0,
  follow: null,
};

/** 解析 camera_preview 的二进制：16 字节头（缩略图宽高、原图宽高）+ 灰度像素。 */
function decodePreview(buf: ArrayBuffer): PreviewImage | null {
  if (buf.byteLength < 16) return null;
  const v = new DataView(buf);
  const [width, height, fullWidth, fullHeight] = [0, 4, 8, 12].map((o) => v.getUint32(o, true));
  const px = new Uint8Array(buf, 16);
  const data = new ImageData(width, height);
  for (let i = 0; i < width * height; i++) {
    const g = px[i];
    data.data[i * 4] = data.data[i * 4 + 1] = data.data[i * 4 + 2] = g;
    data.data[i * 4 + 3] = 255;
  }
  return { width, height, fullWidth, fullHeight, data };
}

export const cameraApi = {
  rigStatus: () => call<CameraStatus[]>("camera_rig_status", undefined, () => []),
  rigConfig: () => call<CameraConfig[]>("camera_rig_config", undefined, () => []),
  saveConfig: (cam: number, config: CameraConfig) => call<string[]>("camera_save_config", { cam, config }, () => []),
  add: (config: CameraConfig) => call<number>("camera_add", { config }, () => 0),
  remove: (cam: number) => call<void>("camera_remove", { cam }, () => undefined),
  listDevices: () => call<DeviceSummary[]>("camera_list_devices", undefined, () => []),
  preview: (cam: number) => call<ArrayBuffer>("camera_preview", { cam }, () => new ArrayBuffer(0)).then(decodePreview),
  softTrigger: (cam: number) => call<void>("camera_soft_trigger", { cam }, () => undefined),
  dryRunStart: () => call<void>("camera_dry_run_start", undefined, () => undefined),
  dryRunGet: () => call<DryFrame[] | null>("camera_dry_run_get", undefined, () => null),
  dryRunStop: () => call<DryFrame[]>("camera_dry_run_stop", undefined, () => []),
  records: () => call<{ root: string; items: RecordEntry[] }>("records_list", undefined, () => ({ root: "", items: [] })),
};

/** 相机组状态每秒刷新；各相机最近一帧攒起来每 250 ms 交一次（缩略图本来也是 250 ms 取一次），页面不必跟着每帧重画。 */
export function useRigStatus() {
  const [statuses, setStatuses] = useState<CameraStatus[]>([]);
  const [lastFrame, setLastFrame] = useState<Record<number, Frame>>({});
  useEffect(() => {
    let pending = false;
    const refresh = () => {
      if (pending) return;
      pending = true;
      cameraApi
        .rigStatus()
        .then(setStatuses)
        .catch(() => setStatuses([]))
        .finally(() => (pending = false));
    };
    refresh();
    const timer = setInterval(refresh, 1000);
    const latest: Record<number, Frame> = {};
    let fresh = false;
    const flush = setInterval(() => {
      if (!fresh) return;
      fresh = false;
      setLastFrame({ ...latest });
    }, 250);
    const off = subscribe<Frame>("camera://frame", (f) => {
      latest[f.cam] = f;
      fresh = true;
    });
    return () => {
      clearInterval(timer);
      clearInterval(flush);
      off();
    };
  }, []);
  return { statuses, lastFrame };
}

/**
 * 某台相机的最近一帧缩略图：有新帧（frameKey 变了）才取，两次之间至少隔 intervalMs。
 * 已经发出去的请求不因为又来了新帧而作废，否则帧来得比取图快时画面永远不更新。
 */
export function usePreview(cam: number, frameKey: unknown, intervalMs = 250, scopeKey = "") {
  const key=`${cam}:${scopeKey}`;
  const current=useRef({key,generation:0,mounted:true});
  if(current.current.key!==key){current.current.key=key;current.current.generation++;}
  const generation=current.current.generation;
  const [state,setState]=useState<{generation:number;img:PreviewImage|null}|null>(null);
  const lastAt=useRef({generation:-1,at:0});
  const pending=useRef<{generation:number}|null>(null);
  useEffect(() => {
    current.current.mounted = true;
    return () => {
      current.current.mounted = false;
    };
  }, []);
  // 换相机或图像源后立即隐藏旧图；持续到达的帧只触发限速读取，不使同一源的请求作废。
  useEffect(() => {
    if (pending.current?.generation === generation) return;
    const t = setTimeout(
      () => {
        const request={generation};
        pending.current = request;
        lastAt.current = {generation,at:Date.now()};
        cameraApi
          .preview(cam)
          .then(p=>{if(current.current.mounted&&current.current.generation===generation)setState({generation,img:p});})
          .catch(() => undefined)
          .finally(() => {
            if (pending.current === request) pending.current = null;
          });
      },
      lastAt.current.generation===generation?Math.max(0,intervalMs-(Date.now()-lastAt.current.at)):0,
    );
    return () => clearTimeout(t);
  }, [cam, frameKey, intervalMs, scopeKey, generation]);
  return state?.generation===generation?state.img:null;
}

/** 缩略图画到 canvas 上：返回图像（含原图尺寸）与要挂到 canvas 上的 ref。 */
export function usePreviewCanvas(cam: number, frameKey: unknown, intervalMs = 250, scopeKey = "") {
  const img = usePreview(cam, frameKey, intervalMs, scopeKey);
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const el = canvas.current;
    if (!el || !img) return;
    el.width = img.width;
    el.height = img.height;
    el.getContext("2d")?.putImageData(img.data, 0, 0);
  }, [img]);
  return { img, canvas };
}
