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
  /** 各段检测相机（D-SB-41），按计划相机顺序列出实际参与判定的几台，不为空 */
  cameras: string[][];
}

/** 一段实际参与判定的相机：段没指定（空）时为全部计划相机（后端 plan::segment_cameras）。 */
export const segmentCameras = (segment: Pick<PathSegment, "cameras">, planCameras: string[]) =>
  planCameras.filter(camera => !segment.cameras?.length || segment.cameras.includes(camera));

/** 各段的帧组范围 [first, last]（含两端）。 */
export function spans(draft: WorkbenchDraft): [number, number][] {
  const bounds = [0, ...draft.cuts, draft.groupCount];
  return bounds.slice(0, -1).map((start, i) => [start + 1, bounds[i + 1]]);
}

/** 起始草稿：本轮已有且与段数相符、未失效的边界原样带入；否则按段数等分。段类型与检测相机取现有段列表。 */
export function initialDraft(groupCount: number, segments: PathSegment[], planCameras: string[], saved?: { cuts: number[]; status: string } | null): WorkbenchDraft {
  const kinds = segments.map(s => s.kind);
  const cameras = segments.map(s => segmentCameras(s, planCameras));
  if (saved && saved.status !== "stale" && saved.cuts.length === kinds.length - 1 && valid({ groupCount, cuts: saved.cuts, kinds, cameras })) {
    return { groupCount, cuts: [...saved.cuts], kinds, cameras };
  }
  const count = Math.max(1, Math.min(kinds.length, groupCount));
  const cuts = Array.from({ length: count - 1 }, (_, i) => Math.round(groupCount * (i + 1) / count));
  return { groupCount, cuts, kinds: kinds.slice(0, count), cameras: cameras.slice(0, count) };
}

/** 在帧组 after 之后插入边界（所在段一分为二，类型与检测相机相同）；已有边界则删除它（与后一段合并，保留前一段的类型与相机）。 */
export function toggleCut(draft: WorkbenchDraft, after: number): WorkbenchDraft {
  if (!Number.isInteger(after) || after <= 0 || after >= draft.groupCount) return draft;
  const at = draft.cuts.indexOf(after);
  if (at >= 0) {
    const keep = (_: unknown, i: number) => i !== at + 1;
    return { ...draft, cuts: draft.cuts.filter((_, i) => i !== at), kinds: draft.kinds.filter(keep), cameras: draft.cameras.filter(keep) };
  }
  const segment = spans(draft).findIndex(([first, last]) => after >= first && after < last);
  if (segment < 0) return draft;
  const kinds = [...draft.kinds];
  kinds.splice(segment + 1, 0, kinds[segment]);
  const cameras = [...draft.cameras];
  cameras.splice(segment + 1, 0, [...cameras[segment]]);
  return { ...draft, cuts: [...draft.cuts, after].sort((a, b) => a - b), kinds, cameras };
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

/** 选中 / 取消第 segment 段的一台检测相机（按计划相机顺序）；每段至少保留一台。 */
export function toggleCamera(draft: WorkbenchDraft, segment: number, camera: string, planCameras: string[]): WorkbenchDraft {
  const current = draft.cameras[segment] ?? [];
  const next = current.includes(camera) ? current.filter(id => id !== camera) : planCameras.filter(id => id === camera || current.includes(id));
  if (!next.length) return draft;
  return { ...draft, cameras: draft.cameras.map((list, i) => (i === segment ? next : list)) };
}

export function valid(draft: WorkbenchDraft): boolean {
  return draft.kinds.length === draft.cuts.length + 1 && draft.cameras.length === draft.kinds.length && draft.cameras.every(list => list.length > 0)
    && draft.cuts.every((cut, i) => Number.isInteger(cut) && cut > (draft.cuts[i - 1] ?? 0) && cut < draft.groupCount);
}

/** 段的类型序列（段数、顺序、类型）是否与现有段列表不同：不同则保存后其余轮次的边界与全部标定失效。 */
export const structureChanged = (current: PathSegment[], kinds: SegmentKind[]) =>
  current.length !== kinds.length || current.some((segment, i) => segment.kind !== kinds[i]);

/**
 * 要保存的段列表：类型序列不变时沿用现有段（参数、N_exp、标定都保留），只按草稿改检测相机；变了则按新序列重建，
 * 同位置同类型的段沿用长度、容差、检测区间、检测项与胶偏参考，其余取该类型默认值，N_exp、标定与 x_ref 一律清空。
 * 检测相机选全了存为空（= 全部计划相机）；胶偏参考相机不在本段检测相机里时改为本段第一台并清空 x_ref。都没变时原样返回。
 */
export function segmentsForSave(current: PathSegment[], draft: Pick<WorkbenchDraft, "kinds" | "cameras">, defaults: KindDefaults, planCameras: string[]): PathSegment[] {
  const restructured = structureChanged(current, draft.kinds);
  const base = restructured
    ? clearCalibration(draft.kinds.map((kind, i) => {
      const old = current[i];
      return old && old.kind === kind ? { ...old, index: i + 1 } : newSegment(i + 1, kind, defaults);
    }))
    : current;
  const stored = (i: number) => {
    const chosen = planCameras.filter(camera => draft.cameras[i]?.includes(camera));
    return chosen.length === planCameras.length || !chosen.length ? [] : chosen;
  };
  const same = (a: string[] | undefined, b: string[]) => JSON.stringify(a ?? []) === JSON.stringify(b);
  if (!restructured && base.every((segment, i) => same(segment.cameras, stored(i)))) return current;
  return base.map((segment, i) => {
    const cameras = stored(i);
    const effective = segmentCameras({ cameras }, planCameras);
    const offsetRef = segment.offsetRef && !effective.includes(segment.offsetRef.camera)
      ? { ...segment.offsetRef, camera: effective[0], xRef: null, spread: null }
      : segment.offsetRef;
    return { ...segment, cameras, offsetRef };
  });
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
