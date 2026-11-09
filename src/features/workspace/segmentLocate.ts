import type { Location } from "react-router-dom";
import type { ReturnNavigationState } from "./returnNavigation";

/** 从历史详情跳到分段与示教页时带的定位：历史件选中的帧组、所在段与历史件的帧组数、各笔帧组数。 */
export interface SegmentLocate {
  sn: string;
  historyId: number;
  seq: number;
  groupCount: number;
  passes?: number[];
  /** 选中帧组所在区域的段号；未分段区域取覆盖的第一段，没有区域为 null */
  segment: number | null;
}
export interface SegmentLocateState extends ReturnNavigationState { locate?: SegmentLocate }

export function segmentLocateOf(location: Pick<Location, "state">): SegmentLocate | null {
  return (location.state as SegmentLocateState | null)?.locate ?? null;
}

const sum = (values: number[]) => values.reduce((a, b) => a + b, 0);

/**
 * 历史件的帧组在示教件上的对应帧组（从 1 起）：两件笔数相同时按离所在笔起点的帧数对应，超出示教件这一笔的笔尾帧对到这一笔
 * 末帧（与生产按笔换算段边界同一口径），否则按整件进度对应（与比例分段同一口径）。只是定位，不代表这两帧拍的是同一处。
 */
export function teachingSeqFor(locate: Pick<SegmentLocate, "seq" | "groupCount" | "passes">, teachingCount: number, teachingPasses?: number[]): { seq: number; byPass: boolean } {
  const clamp = (seq: number) => Math.min(teachingCount, Math.max(1, seq));
  const offset = locate.seq - 1;
  const passes = locate.passes;
  if (passes?.length && teachingPasses?.length === passes.length && offset < sum(passes) && teachingPasses.every(n => n > 0)) {
    let start = 0;
    for (let p = 0; p < passes.length; p++) {
      if (offset < start + passes[p]) {
        const teachingStart = sum(teachingPasses.slice(0, p));
        return { seq: clamp(teachingStart + Math.min(offset - start, teachingPasses[p] - 1) + 1), byPass: true };
      }
      start += passes[p];
    }
  }
  return { seq: clamp(Math.floor(offset * teachingCount / Math.max(1, locate.groupCount)) + 1), byPass: false };
}
