import { useEffect, useRef, useState } from "react";
import { fillRgba } from "../../lib/frame";
import { workspaceApi } from "./api";
import { ThumbPages, THUMB_PAGE_SIZE, type ThumbFrame } from "./thumbPages";

/** 一张缩略图：直接把像素画进 canvas（不生成图片地址）。 */
export function Thumb({ pixels, width, height, channels, status = pixels ? "ready" : "missing" }: Omit<ThumbFrame, "status"> & { status?: ThumbFrame["status"] }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const context = canvas.current?.getContext("2d");
    if (!context || !pixels || !width || !height) return;
    const image = context.createImageData(width, height);
    fillRgba({ data: pixels, width, height, channels }, image.data);
    context.putImageData(image, 0, 0);
  }, [pixels, width, height, channels]);
  return pixels ? <canvas ref={canvas} width={width} height={height} /> : <span className="swb-missing">{status === "loading" ? "加载中…" : status === "error" ? "读取失败" : "缺帧"}</span>;
}

const pages = new ThumbPages((round, camera, startSeq, count) => workspaceApi.captureRoundThumbs(round, camera, startSeq, count));

export function useThumbs(roundId: string, cameras: string[], groupCount: number, start: number, end: number) {
  const [, refresh] = useState(0);
  const first = Math.floor(start / THUMB_PAGE_SIZE) * THUMB_PAGE_SIZE;
  const last = Math.min(groupCount, Math.ceil(end / THUMB_PAGE_SIZE) * THUMB_PAGE_SIZE);
  const key = JSON.stringify([roundId, cameras, groupCount, first, last]);
  useEffect(() => pages.watch(roundId, cameras, groupCount, first, last, () => refresh(value => value + 1)), [key]);
  return {
    getFrame: (camera: string, index: number) => pages.frame(roundId, camera, index),
    error: pages.error(roundId, cameras, first, last),
    retry: () => { pages.retry(roundId); refresh(value => value + 1); },
  };
}

/** 方向键等快捷键的统一过滤：带修饰键、已处理，或焦点在输入控件与对话框里时不处理。 */
export const ignoreKey = (e: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; defaultPrevented: boolean; target: EventTarget | null }) =>
  e.altKey || e.ctrlKey || e.metaKey || e.defaultPrevented
  || !!(e.target as HTMLElement | null)?.closest?.("input,select,textarea,[contenteditable='true'],[role='dialog'],[role='combobox'],[role='listbox']");
