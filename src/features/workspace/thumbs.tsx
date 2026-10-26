import { useEffect, useRef, useState } from "react";
import { errorText } from "../../lib/errors";
import { workspaceApi } from "./api";
import type { ThumbStrip } from "./workbenchDraft";

/** 一张灰度缩略图：直接把像素画进 canvas（不生成图片地址）。 */
export function Thumb({ pixels, width, height }: { pixels: Uint8Array | null; width: number; height: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const context = canvas.current?.getContext("2d");
    if (!context || !pixels || !width || !height) return;
    const image = context.createImageData(width, height);
    for (let i = 0; i < pixels.length; i++) {
      image.data[i * 4] = image.data[i * 4 + 1] = image.data[i * 4 + 2] = pixels[i];
      image.data[i * 4 + 3] = 255;
    }
    context.putImageData(image, 0, 0);
  }, [pixels, width, height]);
  return pixels ? <canvas ref={canvas} width={width} height={height} /> : <span className="swb-missing">缺帧</span>;
}

/** 已取过的缩略图条带：同一轮同一相机只取一次（轮次原图不会变）。 */
const cache = new Map<string, Promise<ThumbStrip>>();
function strip(roundId: string, camera: string) {
  const key = `${roundId}|${camera}`;
  let request = cache.get(key);
  if (!request) {
    request = workspaceApi.captureRoundThumbs(roundId, camera);
    cache.set(key, request);
    request.catch(() => cache.delete(key));
  }
  return request;
}

/** 一轮各台相机的缩略图条带；roundId 为空时不取。 */
export function useThumbs(roundId: string, cameras: string[]) {
  const key = `${roundId}|${cameras.join("|")}`;
  const [state, setState] = useState<{ key: string; strips: Record<string, ThumbStrip> | null; error: string }>({ key: "", strips: null, error: "" });
  useEffect(() => {
    if (!roundId) return;
    let alive = true;
    Promise.all(cameras.map(camera => strip(roundId, camera).then(value => [camera, value] as const)))
      .then(entries => alive && setState({ key, strips: Object.fromEntries(entries), error: "" }))
      .catch(e => alive && setState({ key, strips: null, error: errorText(e) }));
    return () => { alive = false; };
  }, [key]);
  return state.key === key ? state : { key, strips: null, error: "" };
}

/** 帧组数 = 各相机条带帧数的最大值。 */
export const stripGroups = (strips: Record<string, ThumbStrip> | null) =>
  strips ? Math.max(0, ...Object.values(strips).map(item => item.frames.length)) : 0;

/** 方向键等快捷键的统一过滤：带修饰键、已处理，或焦点在输入控件与对话框里时不处理。 */
export const ignoreKey = (e: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; defaultPrevented: boolean; target: EventTarget | null }) =>
  e.altKey || e.ctrlKey || e.metaKey || e.defaultPrevented
  || !!(e.target as HTMLElement | null)?.closest?.("input,select,textarea,[contenteditable='true'],[role='dialog'],[role='combobox'],[role='listbox']");
