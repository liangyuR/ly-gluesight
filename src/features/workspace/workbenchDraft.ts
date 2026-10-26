/**
 * 分段标定工作台的编辑规则（纯函数，SegmentWorkbench 用，D-SB-44）：在模板轮的帧组时间轴上定段结构与边界。
 * cuts 与后端 CaptureReview 同一口径：每个值是上一段的最后一个原始帧组，严格递增，0 < cut < 帧组数，个数 = 段数 − 1。
 */
import type { KindDefaults, PathSegment, SegmentKind } from "../plan/types";
import { clearCalibration, newSegment } from "./segmentEdit";

export interface WorkbenchDraft {
  groupCount: number;
  cuts: number[];
  kinds: SegmentKind[];
}

/** 各段的帧组范围 [first, last]（含两端）。 */
export function spans(draft: WorkbenchDraft): [number, number][] {
  const bounds = [0, ...draft.cuts, draft.groupCount];
  return bounds.slice(0, -1).map((start, i) => [start + 1, bounds[i + 1]]);
}

/** 起始草稿：本轮已有且与段数相符、未失效的边界原样带入；否则按段数等分。 */
export function initialDraft(groupCount: number, kinds: SegmentKind[], saved?: { cuts: number[]; status: string } | null): WorkbenchDraft {
  if (saved && saved.status !== "stale" && saved.cuts.length === kinds.length - 1 && valid({ groupCount, cuts: saved.cuts, kinds })) {
    return { groupCount, cuts: [...saved.cuts], kinds: [...kinds] };
  }
  const count = Math.max(1, Math.min(kinds.length, groupCount));
  const cuts = Array.from({ length: count - 1 }, (_, i) => Math.round(groupCount * (i + 1) / count));
  return { groupCount, cuts, kinds: kinds.slice(0, count) };
}

/** 在帧组 after 之后插入边界（所在段一分为二，类型相同）；已有边界则删除它（与后一段合并，保留前一段类型）。 */
export function toggleCut(draft: WorkbenchDraft, after: number): WorkbenchDraft {
  if (!Number.isInteger(after) || after <= 0 || after >= draft.groupCount) return draft;
  const at = draft.cuts.indexOf(after);
  if (at >= 0) {
    return { ...draft, cuts: draft.cuts.filter((_, i) => i !== at), kinds: draft.kinds.filter((_, i) => i !== at + 1) };
  }
  const segment = spans(draft).findIndex(([first, last]) => after >= first && after < last);
  if (segment < 0) return draft;
  const kinds = [...draft.kinds];
  kinds.splice(segment + 1, 0, kinds[segment]);
  return { ...draft, cuts: [...draft.cuts, after].sort((a, b) => a - b), kinds };
}

/** 把第 at 个边界挪到 after 之后：不越过相邻边界，不产生空段。 */
export function moveCut(draft: WorkbenchDraft, at: number, after: number): WorkbenchDraft {
  const low = (draft.cuts[at - 1] ?? 0) + 1;
  const high = (draft.cuts[at + 1] ?? draft.groupCount) - 1;
  if (at < 0 || at >= draft.cuts.length || after < low || after > high) return draft;
  return { ...draft, cuts: draft.cuts.map((cut, i) => (i === at ? after : cut)) };
}

export function toggleKind(draft: WorkbenchDraft, segment: number): WorkbenchDraft {
  return { ...draft, kinds: draft.kinds.map((kind, i) => (i === segment ? (kind === "straight" ? "corner" : "straight") : kind)) };
}

export function valid(draft: WorkbenchDraft): boolean {
  return draft.kinds.length === draft.cuts.length + 1
    && draft.cuts.every((cut, i) => Number.isInteger(cut) && cut > (draft.cuts[i - 1] ?? 0) && cut < draft.groupCount);
}

/** 段的类型序列（段数、顺序、类型）是否与现有段列表不同：不同则保存后其余轮次的边界与全部标定失效。 */
export const structureChanged = (current: PathSegment[], kinds: SegmentKind[]) =>
  current.length !== kinds.length || current.some((segment, i) => segment.kind !== kinds[i]);

/**
 * 要保存的段列表：类型序列不变时原样返回现有段（参数、N_exp、标定都保留）；变了则按新序列重建，
 * 同位置同类型的段沿用长度、容差、检测区间、检测项与胶偏参考，其余取该类型默认值，N_exp、标定与 x_ref 一律清空。
 */
export function segmentsForSave(current: PathSegment[], kinds: SegmentKind[], defaults: KindDefaults): PathSegment[] {
  if (!structureChanged(current, kinds)) return current;
  return clearCalibration(kinds.map((kind, i) => {
    const old = current[i];
    return old && old.kind === kind ? { ...old, index: i + 1 } : newSegment(i + 1, kind, defaults);
  }));
}

export interface ThumbStrip {
  width: number;
  height: number;
  /** 帧组 1..N 的灰度缩略图；缺帧为 null */
  frames: (Uint8Array | null)[];
}

/** 解码后端 recipe_capture_round_thumbs 的条带：头部 [帧组数, 宽, 高]（u32 小端），之后每组 1 字节状态加 宽 × 高 灰度。 */
export function decodeThumbStrip(input: ArrayBuffer | number[]): ThumbStrip {
  const buf = input instanceof ArrayBuffer ? input : Uint8Array.from(input).buffer;
  if (buf.byteLength < 12) throw new Error("缩略图数据为空");
  const view = new DataView(buf);
  const [count, width, height] = [0, 4, 8].map(offset => view.getUint32(offset, true));
  const size = width * height;
  if (buf.byteLength < 12 + count * (1 + size)) throw new Error("缩略图数据不完整");
  const frames = Array.from({ length: count }, (_, i) => {
    const at = 12 + i * (1 + size);
    return view.getUint8(at) ? new Uint8Array(buf, at + 1, size) : null;
  });
  return { width, height, frames };
}
