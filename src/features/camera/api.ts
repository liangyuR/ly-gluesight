import { invoke, isTauri } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { decodeFrame, FRAME_HEADER_BYTES, frameImageData } from "../../lib/frame";
import type { BandwidthEstimate, CameraConfig, CameraConfigSaved, CameraImaging, CameraStatus, DeviceSummary, PreviewImage } from "./types";

function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: () => T): Promise<T> {
  if (!isTauri() && ["camera_save_config","camera_add","camera_remove","camera_soft_trigger","camera_release","camera_reconnect","camera_set_imaging"].includes(cmd))
    return Promise.reject(new Error("设备操作需要 GlueSight · 胶路智检 桌面后端"));
  if (!isTauri()) return Promise.resolve(fallback());
  return invoke<T>(cmd, args);
}

export const defaultCameraConfig: CameraConfig = {
  id: "",
  name: "相机",
  source: "mvs",
  viewCount: 1,
  serial: "",
  triggerSource: "Line0",
  triggerActivation: "LevelHigh",
  frameRate: 10,
  triggerDelayUs: 0,
  debouncerUs: 5,
  strobe: true,
  chunk: true,
  counterAfterOpen: null,
  imageMode: "color",
};

/** 解析 camera_preview 的二进制（格式见 decodeFrame）；浏览器查看模式下为空数据，返回 null。 */
function decodePreview(buf: ArrayBuffer): PreviewImage | null {
  if (buf.byteLength < FRAME_HEADER_BYTES) return null;
  const { fullWidth, fullHeight, pixels } = decodeFrame(buf);
  return { width: pixels.width, height: pixels.height, fullWidth, fullHeight, data: frameImageData(pixels) };
}

export const cameraApi = {
  rigStatus: () => call<CameraStatus[]>("camera_rig_status", undefined, () => []),
  rigConfig: () => call<CameraConfig[]>("camera_rig_config", undefined, () => []).then(configs => configs.map(c => ({ ...c, imageMode: c.imageMode ?? "gray" }))),
  bandwidth: (hz: number) => call<BandwidthEstimate | null>("camera_bandwidth", { hz }, () => null),
  calibInfo: (cam: number, id: string, view: number) => call<{ mmPerPx: number | null } | null>("vision_calib_info", { cam, id, view }, () => null),
  saveConfig: (cam: number, config: CameraConfig) => call<CameraConfigSaved>("camera_save_config", { cam, config }, () => ({ saved: true, applied: false, warnings: [], applyError: "需要桌面后端" })),
  add: (config: CameraConfig) => call<number>("camera_add", { config }, () => 0),
  remove: (cam: number, id: string) => call<void>("camera_remove", { cam, id }, () => undefined),
  listDevices: () => call<DeviceSummary[]>("camera_list_devices", undefined, () => []),
  preview: (cam: number, id: string, view = 1) => call<ArrayBuffer>("camera_preview", { cam, id, view }, () => new ArrayBuffer(0)).then(decodePreview),
  softTrigger: (cam: number, id: string) => call<void>("camera_soft_trigger", { cam, id }, () => undefined),
  setImaging: (cam: number, id: string, imaging: CameraImaging) => call<void>("camera_set_imaging", { cam, id, imaging }, () => undefined),
  release: (cam: number, id: string) => call<void>("camera_release", { cam, id }, () => undefined),
  reconnect: (cam: number, id: string) => call<string>("camera_reconnect", { cam, id }, () => ""),
};

/** 相机组状态每秒刷新。 */
export function useRigStatus() {
  const [statuses, setStatuses] = useState<CameraStatus[]>([]);
  useEffect(() => {
    let alive = true, pending = false;
    const refresh = () => {
      if (pending) return;
      pending = true;
      cameraApi
        .rigStatus()
        .then(next => { if (alive) setStatuses(next); })
        .catch(() => { if (alive) setStatuses([]); })
        .finally(() => (pending = false));
    };
    refresh();
    const timer = setInterval(refresh, 1000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);
  return { statuses };
}

/**
 * 某台相机的最近一帧缩略图：有新帧（frameKey 变了）才取，两次之间至少隔 intervalMs。
 * 同源读取期间到达的新帧合并为一次补读，已发出的读取仍可显示。
 */
export function usePreview(cam: number, id: string | null, frameKey: unknown, intervalMs = 250, scopeKey = "", view = 1) {
  const key=JSON.stringify([cam,id,view,scopeKey]);
  const current=useRef({key,generation:0,mounted:true});
  if(current.current.key!==key){current.current.key=key;current.current.generation++;}
  const generation=current.current.generation;
  const [state,setState]=useState<{generation:number;img:PreviewImage|null}|null>(null);
  const [followUp,setFollowUp]=useState(0);
  const lastAt=useRef({generation:-1,at:0});
  const pending=useRef<{generation:number;queued:boolean}|null>(null);
  useEffect(() => {
    current.current.mounted = true;
    return () => {
      current.current.mounted = false;
      pending.current = null;
    };
  }, []);
  // 换相机或图像源后立即隐藏旧图；持续到达的帧只触发限速读取，不使同一源的请求作废。
  useEffect(() => {
    if (!id) return;
    if (pending.current?.generation === generation) {
      pending.current.queued = true;
      return;
    }
    const t = setTimeout(
      () => {
        const request={generation,queued:false};
        pending.current = request;
        lastAt.current = {generation,at:Date.now()};
        const active = () => current.current.mounted && current.current.generation === generation && pending.current === request;
        cameraApi
          .preview(cam, id, view)
          .then(p => { if (active()) setState({generation,img:p}); })
          .catch(() => undefined)
          .finally(() => {
            const shouldReadAgain = active() && request.queued;
            if (pending.current === request) pending.current = null;
            if (shouldReadAgain) setFollowUp(value => value + 1);
          });
      },
      lastAt.current.generation===generation?Math.max(0,intervalMs-(Date.now()-lastAt.current.at)):0,
    );
    return () => clearTimeout(t);
  }, [cam, id, frameKey, intervalMs, generation, view, followUp]);
  return state?.generation===generation?state.img:null;
}

/** 缩略图画到 canvas 上：返回图像（含原图尺寸）与要挂到 canvas 上的 ref。 */
export function usePreviewCanvas(cam: number, id: string | null, frameKey: unknown, intervalMs = 250, scopeKey = "", view = 1) {
  const img = usePreview(cam, id, frameKey, intervalMs, scopeKey, view);
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
