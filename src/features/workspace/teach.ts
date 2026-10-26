import type { RecipeDoc } from "../cycle/types";
import type { CameraInspect, CameraTeachDraft } from "../plan/types";
import { pathLength } from "./teachingPath";

const canonical = (v: unknown): unknown => Array.isArray(v) ? v.map(canonical)
  : v && typeof v === "object" ? Object.fromEntries(Object.keys(v).sort().map(key => [key, canonical((v as Record<string, unknown>)[key])]))
  : typeof v === "number" ? Math.fround(v) : v;

/** 候选配方里这台相机已保存的走廊、像素当量与检测参数；配方里还没有这台相机的项时为空草稿。 */
export function corridorDraft(inspect: Pick<CameraInspect, "corridor" | "mmPerPx" | "detect"> | undefined): CameraTeachDraft {
  return {
    corridor: (inspect?.corridor ?? []).map(([x, y]) => [x, y] as [number, number]),
    mmPerPx: inspect?.mmPerPx ?? null,
    detect: inspect?.detect ? structuredClone(inspect.detect) : null,
  };
}

/** 按 f32 比较：后端以 f32 存走廊与参数，取回的值与草稿在 f32 精度内相同就算没改。 */
export function sameCorridor(a: CameraTeachDraft | undefined, b: CameraTeachDraft | undefined) {
  return JSON.stringify(canonical(a ?? null)) === JSON.stringify(canonical(b ?? null));
}

/** 走廊草稿能否保存为“已示教”：至少两点、坐标为有限数、长度不为零，像素当量为正；自定义检测参数要合法。返回错误说明，空串表示可以。 */
export function corridorError(t: CameraTeachDraft) {
  if (!(t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0 && t.mmPerPx <= 10)) return "像素当量需在 0–10 mm/px 之间";
  if (t.corridor.some(p => !p.every(Number.isFinite))) return "走廊坐标必须是有限数";
  if (t.corridor.length < 2 || pathLength(t.corridor) < 1) return "走廊至少两个点、长度不能为零";
  if (t.detect) {
    const { searchMm, widthRange: [lo, hi] } = t.detect;
    if (!(Number.isFinite(searchMm) && searchMm > 0 && Number.isFinite(lo) && Number.isFinite(hi) && lo > 0 && lo < hi)) return "搜索半宽需为正，胶宽范围需满足 0 < 下限 < 上限";
    if (hi >= 2 * searchMm) return `胶宽上限要小于搜索宽度 ${(2 * searchMm).toFixed(1)} mm`;
  }
  return "";
}

/** 走廊草稿的键：默认走廊为相机编号，段走廊（D-SB-42）为“相机#段号”。 */
export const draftKey = (camera: string, segment: number | null) => segment == null ? camera : `${camera}#${segment}`;

export function parseDraftKey(key: string): { camera: string; segment: number | null } {
  const at = key.lastIndexOf("#");
  return at < 0 ? { camera: key, segment: null } : { camera: key.slice(0, at), segment: Number(key.slice(at + 1)) };
}

/** 候选里已保存的草稿：段走廊只有走廊本身按段，像素当量与检测参数取这台相机的。 */
export function savedDraft(doc: RecipeDoc | undefined, key: string): CameraTeachDraft {
  const { camera, segment } = parseDraftKey(key);
  const inspect = doc?.inspect?.find(item => item.camera === camera);
  const draft = corridorDraft(inspect);
  if (segment == null) return draft;
  const corridor = inspect?.segments?.find(item => item.segment === segment)?.corridor ?? [];
  return { ...draft, corridor: corridor.map(([x, y]) => [x, y] as [number, number]) };
}

/** 把一条走廊草稿写进配方副本（保存草稿用：整份配方经 workspace_save_doc 提交）；配方里没有这台相机的项时原样返回。
 *  段走廊只写这一段的走廊，不动相机的像素当量与检测参数；清空的段走廊从配方里去掉（示教帧仍在，这一段先用默认走廊）。 */
export function withCorridor(doc: RecipeDoc, key: string, draft: CameraTeachDraft): RecipeDoc {
  const { camera, segment } = parseDraftKey(key);
  const inspect = doc.inspect ?? [];
  if (!inspect.some(item => item.camera === camera)) return doc;
  return { ...doc, inspect: inspect.map(item => {
    if (item.camera !== camera) return item;
    if (segment == null) return { ...item, corridor: draft.corridor, mmPerPx: draft.mmPerPx, detect: draft.detect };
    const others = (item.segments ?? []).filter(s => s.segment !== segment);
    const segments = draft.corridor.length ? [...others, { segment, corridor: draft.corridor }].sort((a, b) => a.segment - b.segment) : others;
    return { ...item, segments };
  }) };
}


/** 弯道段走廊的建议长度上限（D-SB-42）：从出胶点起画的短走廊，超过只提示。 */
export const CORNER_CORRIDOR_MM = 8;

/** 一轮已确认的手动分段里第 `segment` 段的帧组范围（从 1 起，含两端）；没确认或段数对不上时为 null。 */
export function confirmedSegmentRange(review: { status: string; cuts: number[]; groupCount: number } | undefined, segments: { index: number }[], segment: number): { start: number; end: number } | null {
  const at = segments.findIndex(item => item.index === segment);
  if (!review || review.status !== "confirmed" || at < 0 || review.cuts.length !== segments.length - 1) return null;
  const bounds = [0, ...review.cuts, review.groupCount];
  return { start: bounds[at] + 1, end: bounds[at + 1] };
}
