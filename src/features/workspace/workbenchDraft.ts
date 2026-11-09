/**
 * 分段草稿的编辑规则（纯函数，分段与示教页用，D-SB-44）：在示教件的帧组时间轴上定段结构与边界，段不跨笔。
 * cuts 与后端 CaptureReview 同一口径：每个值是上一段的最后一个原始帧组，严格递增，0 < cut < 帧组数，个数 = 段数 − 1。
 */
import type { KindDefaults, PassSkip, PathSegment, SegmentKind } from "../plan/types";
import { clearCalibration, newSegment } from "./segmentEdit";
import { frameChannels } from "../../lib/frame";

export interface WorkbenchDraft {
  groupCount: number;
  cuts: number[];
  kinds: SegmentKind[];
  /** 各段检测相机（D-SB-41），按计划相机顺序列出选中的几台，不为空；段不检测时保留，取消后恢复 */
  cameras: string[][];
  /** 各段是否不检测（D-SB-53） */
  skips: boolean[];
}

/** 一段实际参与判定的相机：段没指定（空）时为全部计划相机，不检测的段没有（后端 plan::segment_cameras）。 */
export const segmentCameras = (segment: Pick<PathSegment, "cameras" | "skip">, planCameras: string[]) =>
  segment.skip ? [] : planCameras.filter(camera => !segment.cameras?.length || segment.cameras.includes(camera));

/** 段选中的检测相机（不管是否不检测）：工作台显示与恢复用。 */
const chosenCameras = (segment: Pick<PathSegment, "cameras">, planCameras: string[]) =>
  planCameras.filter(camera => !segment.cameras?.length || segment.cameras.includes(camera));

/** 各段的帧组范围 [first, last]（含两端）。 */
export function spans(draft: WorkbenchDraft): [number, number][] {
  const bounds = [0, ...draft.cuts, draft.groupCount];
  return bounds.slice(0, -1).map((start, i) => [start + 1, bounds[i + 1]]);
}

/** 示教件各笔帧组数：缺省、为空或与帧组数对不上时整件当一笔。 */
export function piecePasses(passes: number[] | undefined, groupCount: number): number[] {
  return passes?.length && passes.every(n => Number.isInteger(n) && n > 0) && passes.reduce((sum, n) => sum + n, 0) === groupCount ? [...passes] : groupCount > 0 ? [groupCount] : [];
}

/** 各笔最后一个帧组（最后一笔除外）：段不跨笔，这些都必须是段边界。 */
export function passEnds(passes: number[]): number[] {
  return passes.slice(0, -1).map((_, i) => passes.slice(0, i + 1).reduce((sum, n) => sum + n, 0));
}

/** 补上缺的笔末边界：跨笔的段在笔末拆开，各半段沿用原段的类型、检测相机与是否检测。 */
export function withPassEnds(draft: WorkbenchDraft, ends: number[]): WorkbenchDraft {
  return ends.reduce((next, end) => next.cuts.includes(end) ? next : toggleCut(next, end), draft);
}

/** 整笔帧数不超过每笔首尾跳过之和：这一笔整笔不检（与 passCells 的 short 同一口径）。 */
const shortPass = (n: number, skip: PassSkip | undefined) => n <= (Number.isFinite(skip?.head) ? skip!.head : 0) + (Number.isFinite(skip?.tail) ? skip!.tail : 0);

/**
 * 起始草稿，始终含全部笔末边界：本轮已有且与段数相符、未失效的边界原样带入，跨笔的段在笔末拆开；
 * 本轮还没存过边界时每笔一段，类型与检测相机按位置沿用现有段（多出的笔沿用最后一段），短笔默认不检测；
 * 边界失效或对不上时按段数等分再补笔末。
 */
export function initialDraft(groupCount: number, segments: PathSegment[], planCameras: string[], saved?: { cuts: number[]; status: string } | null, passes?: number[], passSkip?: PassSkip): WorkbenchDraft {
  const kinds = segments.map(s => s.kind);
  const cameras = segments.map(s => chosenCameras(s, planCameras));
  const skips = segments.map(s => !!s.skip);
  const strokes = piecePasses(passes, groupCount);
  const ends = passEnds(strokes);
  if (saved && saved.status !== "stale" && saved.cuts.length === kinds.length - 1 && valid({ groupCount, cuts: saved.cuts, kinds, cameras, skips })) {
    return withPassEnds({ groupCount, cuts: [...saved.cuts], kinds, cameras, skips }, ends);
  }
  if (!saved && strokes.length && kinds.length) {
    const like = (i: number) => Math.min(i, kinds.length - 1);
    const draft = { groupCount, cuts: ends, kinds: strokes.map((_, i) => kinds[like(i)]), cameras: strokes.map((_, i) => [...cameras[like(i)]]), skips: strokes.map(n => shortPass(n, passSkip)) };
    return valid(draft) ? draft : { ...draft, skips: draft.skips.map(() => false) };
  }
  const count = Math.max(1, Math.min(kinds.length, groupCount));
  const cuts = Array.from({ length: count - 1 }, (_, i) => Math.round(groupCount * (i + 1) / count));
  const even = { groupCount, cuts, kinds: kinds.slice(0, count), cameras: cameras.slice(0, count), skips: skips.slice(0, count) };
  const draft = withPassEnds(valid(even) ? even : { ...even, skips: even.skips.map(() => false) }, ends);
  return valid(draft) ? draft : { ...draft, skips: draft.skips.map(() => false) };
}

/** 在帧组 after 之后插入边界（所在段一分为二，类型、检测相机与是否检测相同）；已有边界则删除它（与后一段合并，保留前一段的）。fixed 里的边界（笔末）不能删。 */
export function toggleCut(draft: WorkbenchDraft, after: number, fixed: number[] = []): WorkbenchDraft {
  if (!Number.isInteger(after) || after <= 0 || after >= draft.groupCount) return draft;
  const at = draft.cuts.indexOf(after);
  if (at >= 0) {
    if (fixed.includes(after)) return draft;
    const keep = (_: unknown, i: number) => i !== at + 1;
    return { ...draft, cuts: draft.cuts.filter((_, i) => i !== at), kinds: draft.kinds.filter(keep), cameras: draft.cameras.filter(keep), skips: draft.skips.filter(keep) };
  }
  const segment = spans(draft).findIndex(([first, last]) => after >= first && after < last);
  if (segment < 0) return draft;
  const kinds = [...draft.kinds];
  kinds.splice(segment + 1, 0, kinds[segment]);
  const cameras = [...draft.cameras];
  cameras.splice(segment + 1, 0, [...cameras[segment]]);
  const skips = [...draft.skips];
  skips.splice(segment + 1, 0, skips[segment]);
  return { ...draft, cuts: [...draft.cuts, after].sort((a, b) => a - b), kinds, cameras, skips };
}

/** 把第 at 个边界挪到 after 之后：不越过相邻边界，不产生空段；fixed 里的边界（笔末）不能挪。 */
export function moveCut(draft: WorkbenchDraft, at: number, after: number, fixed: number[] = []): WorkbenchDraft {
  const low = (draft.cuts[at - 1] ?? 0) + 1;
  const high = (draft.cuts[at + 1] ?? draft.groupCount) - 1;
  if (at < 0 || at >= draft.cuts.length || after < low || after > high || fixed.includes(draft.cuts[at])) return draft;
  return { ...draft, cuts: draft.cuts.map((cut, i) => (i === at ? after : cut)) };
}

/** 点段名循环切换：直线 → 弯道 → 不检测 → 直线（D-SB-53）。不检测时类型停在弯道、检测相机保留，取消后恢复相机。 */
export function toggleKind(draft: WorkbenchDraft, segment: number): WorkbenchDraft {
  const [kind, skip]: [SegmentKind, boolean] = draft.skips[segment] ? ["straight", false]
    : draft.kinds[segment] === "straight" ? ["corner", false] : ["corner", true];
  return { ...draft, kinds: draft.kinds.map((k, i) => (i === segment ? kind : k)), skips: draft.skips.map((s, i) => (i === segment ? skip : s)) };
}

/** 设第 segment 段的类型或不检测（右栏段参数）：设为不检测时类型不变，取消不检测时按所选类型。 */
export function setKind(draft: WorkbenchDraft, segment: number, value: SegmentKind | "skip"): WorkbenchDraft {
  return {
    ...draft,
    kinds: draft.kinds.map((k, i) => (i === segment && value !== "skip" ? value : k)),
    skips: draft.skips.map((s, i) => (i === segment ? value === "skip" : s)),
  };
}

/** 把几段一起设为不检测或恢复检测（批量按笔操作），类型与检测相机不变。 */
export function setSkips(draft: WorkbenchDraft, segments: number[], skip: boolean): WorkbenchDraft {
  return { ...draft, skips: draft.skips.map((s, i) => (segments.includes(i) ? skip : s)) };
}

/** 选中 / 取消第 segment 段的一台检测相机（按计划相机顺序）；每段至少保留一台；不检测的段不能改。 */
export function toggleCamera(draft: WorkbenchDraft, segment: number, camera: string, planCameras: string[]): WorkbenchDraft {
  if (draft.skips[segment]) return draft;
  const current = draft.cameras[segment] ?? [];
  const next = current.includes(camera) ? current.filter(id => id !== camera) : planCameras.filter(id => id === camera || current.includes(id));
  if (!next.length) return draft;
  return { ...draft, cameras: draft.cameras.map((list, i) => (i === segment ? next : list)) };
}

/** 边界、类型、相机对得上，且至少一段检测（单段配方不能不检测，后端 validate_segments 同一约束）。 */
export function valid(draft: WorkbenchDraft): boolean {
  return draft.kinds.length === draft.cuts.length + 1 && draft.cameras.length === draft.kinds.length && draft.cameras.every(list => list.length > 0)
    && draft.skips.length === draft.kinds.length && draft.skips.some(skip => !skip)
    && draft.cuts.every((cut, i) => Number.isInteger(cut) && cut > (draft.cuts[i - 1] ?? 0) && cut < draft.groupCount);
}

/** 段的类型序列（段数、顺序、类型）是否与现有段列表不同：不同则保存后其余轮次的边界与全部标定失效。 */
export const structureChanged = (current: PathSegment[], kinds: SegmentKind[]) =>
  current.length !== kinds.length || current.some((segment, i) => segment.kind !== kinds[i]);

/**
 * 要保存的段列表：类型序列不变时沿用现有段（参数、N_exp、标定都保留），只按草稿改检测相机；变了则按新序列给出类型与检测相机，
 * 参数先取同类型已有段的检测项（没有时取该类型默认值），N_exp、标定与 x_ref 清空。保存时后端再按示教件上的帧范围
 * 把对应旧段的参数、走廊与示教帧迁过来（segment_remap，D-SB-52），这里不按位置对应。
 * 检测相机选全了存为空（= 全部计划相机）；胶偏参考相机不在本段检测相机里时改为本段第一台并清空 x_ref（不检测的段不改）。
 * 不检测按草稿写 `skip`（D-SB-53），检测相机照存，取消后恢复。都没变时原样返回。
 */
export function segmentsForSave(current: PathSegment[], draft: Pick<WorkbenchDraft, "kinds" | "cameras"> & { skips?: boolean[] }, defaults: KindDefaults, planCameras: string[]): PathSegment[] {
  const restructured = structureChanged(current, draft.kinds);
  const base = restructured
    ? clearCalibration(draft.kinds.map((kind, i) => {
      // 新段沿用同类型已有段的检测项（操作员通常已在段列表里设好），没有同类段时取配方的按类型默认值
      const like = current.find(segment => segment.kind === kind);
      const fresh = newSegment(i + 1, kind, defaults);
      return like ? { ...fresh, checks: structuredClone(like.checks) } : fresh;
    }))
    : current;
  const stored = (i: number) => {
    const chosen = planCameras.filter(camera => draft.cameras[i]?.includes(camera));
    return chosen.length === planCameras.length || !chosen.length ? [] : chosen;
  };
  const same = (a: string[] | undefined, b: string[]) => JSON.stringify(a ?? []) === JSON.stringify(b);
  const skipOf = (i: number) => !!draft.skips?.[i];
  if (!restructured && base.every((segment, i) => same(segment.cameras, stored(i)) && !!segment.skip === skipOf(i))) return current;
  return base.map((segment, i) => {
    const cameras = stored(i), skip = skipOf(i);
    const effective = segmentCameras({ cameras }, planCameras);
    const offsetRef = !skip && segment.offsetRef && !effective.includes(segment.offsetRef.camera)
      ? { ...segment.offsetRef, camera: effective[0], xRef: null, spread: null }
      : segment.offsetRef;
    const { skip: _, ...rest } = segment;
    return skip ? { ...rest, cameras, offsetRef, skip: true } : { ...rest, cameras, offsetRef };
  });
}

/** 笔内的一段：pos 为草稿里的段位置（= 段号 − 1），label 为界面上的“笔-段”。 */
export interface PassSegment { pos: number; label: string; first: number; last: number; kind: SegmentKind; skip: boolean; cameras: string[] }
/** 一笔：帧组范围、是否短笔（整笔不超过首尾跳过之和）与笔内各段。 */
export interface PassNode { pass: number; first: number; last: number; short: boolean; segments: PassSegment[] }

/** 按笔分组草稿里的段（草稿含全部笔末边界，每段只落在一笔里）。 */
export function passTree(draft: WorkbenchDraft, passes: number[], skip: PassSkip | undefined): PassNode[] {
  const all = spans(draft);
  let start = 0;
  return passes.map((n, j) => {
    const first = start + 1, last = start + n;
    start = last;
    const segments = all.flatMap(([a, b], pos) => a >= first && a <= last ? [{ pos, a, b }] : [])
      .map(({ pos, a, b }, k): PassSegment => ({ pos, label: `${j + 1}-${k + 1}`, first: a, last: b, kind: draft.kinds[pos], skip: draft.skips[pos], cameras: draft.cameras[pos] ?? [] }));
    return { pass: j + 1, first, last, short: shortPass(n, skip), segments };
  });
}

/** 示教件一个帧组所在的笔（从 1 起）、是否该笔最后一帧（后面还有笔）与每笔首尾跳过的情况；short = 整笔不超过首尾之和，整笔不检。 */
export interface PassCell { pass: number; end: boolean; skip: "head" | "tail" | "short" | null }

/** 按示教件各笔帧组数展开到每个帧组（与后端 passes::skipped 同一口径）；没有笔结构或与帧组数对不上为 null。 */
export function passCells(passes: number[] | undefined, groupCount: number, skip: PassSkip | undefined): PassCell[] | null {
  if (!passes?.length || passes.reduce((sum, n) => sum + n, 0) !== groupCount) return null;
  const head = Number.isFinite(skip?.head) ? skip!.head : 0, tail = Number.isFinite(skip?.tail) ? skip!.tail : 0;
  return passes.flatMap((n, j) => Array.from({ length: n }, (_, k): PassCell => ({
    pass: j + 1,
    end: k === n - 1 && j < passes.length - 1,
    skip: n <= head + tail ? "short" : k < head ? "head" : k >= n - tail ? "tail" : null,
  })));
}

export interface ThumbStrip {
  width: number;
  height: number;
  channels: 1 | 3;
  /** 帧组 1..N 的缩略图像素（宽 × 高 × channels）；缺帧为 null */
  frames: (Uint8Array | null)[];
}

/** 解码后端 recipe_capture_round_thumbs 的条带：头部 [帧组数, 宽, 高, 通道数]（u32 小端），之后每组 1 字节状态加 宽 × 高 × 通道数 字节。 */
export function decodeThumbStrip(input: ArrayBuffer | number[]): ThumbStrip {
  const buf = input instanceof ArrayBuffer ? input : Uint8Array.from(input).buffer;
  if (buf.byteLength < 16) throw new Error("缩略图数据为空");
  const view = new DataView(buf);
  const [count, width, height, raw] = [0, 4, 8, 12].map(offset => view.getUint32(offset, true));
  const channels = frameChannels(raw);
  const size = width * height * channels;
  if (buf.byteLength < 16 + count * (1 + size)) throw new Error("缩略图数据不完整");
  const frames = Array.from({ length: count }, (_, i) => {
    const at = 16 + i * (1 + size);
    return view.getUint8(at) ? new Uint8Array(buf, at + 1, size) : null;
  });
  return { width, height, channels, frames };
}
