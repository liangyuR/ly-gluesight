import type { RecipeDoc } from "../cycle/types";
import type { CameraTeachDraft, InspectWindow } from "../plan/types";
import { pathLength } from "./teachingPath";

const canonical = (v: unknown): unknown => Array.isArray(v) ? v.map(canonical)
  : v && typeof v === "object" ? Object.fromEntries(Object.keys(v).sort().map(key => [key, canonical((v as Record<string, unknown>)[key])]))
  : typeof v === "number" ? Math.fround(v) : v;

/** 按 f32 比较：后端以 f32 存走廊与参数，取回的值与草稿在 f32 精度内相同就算没改。 */
export function sameCorridor(a: CameraTeachDraft | undefined, b: CameraTeachDraft | undefined) {
  return JSON.stringify(canonical(a ?? null)) === JSON.stringify(canonical(b ?? null));
}

/** 测量区间终点（距出胶点）上限（mm），与后端校验一致。 */
export const MAX_WINDOW_END_MM = 500;

/** 测量区间是否可用（配方存起点与长度）：跳过段 ≥ 0、测量到的距离大于跳过段且不超过 500 mm。返回错误说明，空串表示可以。 */
export function windowError(window: InspectWindow) {
  const { startMm, lengthMm } = window;
  if (!(Number.isFinite(startMm) && startMm >= 0)) return "跳过胶嘴附近的距离需不小于 0 mm";
  if (!(Number.isFinite(lengthMm) && lengthMm > 0)) return "测量到的距离需大于跳过的距离";
  if (startMm + lengthMm > MAX_WINDOW_END_MM) return `测量到的距离不能超过 ${MAX_WINDOW_END_MM} mm`;
  return "";
}

/** 相机参数是否可用：像素当量为正且不超过 10 mm/px，自定义检测参数要合法，设了测量区间时区间要合法。返回错误说明，空串表示可以。 */
export function cameraParamsError(t: Pick<CameraTeachDraft, "mmPerPx" | "detect"> & { window?: InspectWindow | null }) {
  if (!(t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0 && t.mmPerPx <= 10)) return "像素当量需在 0–10 mm/px 之间";
  if (t.detect) {
    const { searchMm, widthRange: [lo, hi] } = t.detect;
    if (!(Number.isFinite(searchMm) && searchMm > 0 && Number.isFinite(lo) && Number.isFinite(hi) && lo > 0 && lo < hi)) return "搜索半宽需为正，胶宽范围需满足 0 < 下限 < 上限";
    if (hi >= 2 * searchMm) return `胶宽上限要小于搜索宽度 ${(2 * searchMm).toFixed(1)} mm`;
  }
  if (t.window) return windowError(t.window);
  return "";
}

/** 走廊草稿能否保存为“已示教”：相机参数可用，走廊至少两点、坐标为有限数、长度不为零，按配方站距至少三站。返回错误说明，空串表示可以。 */
export function corridorError(t: CameraTeachDraft, spacingMm: number) {
  const params = cameraParamsError(t);
  if (params) return params;
  if (t.corridor.some(p => !p.every(Number.isFinite))) return "走廊坐标必须是有限数";
  if (t.corridor.length < 2 || pathLength(t.corridor) < 1) return "走廊至少两个点、长度不能为零";
  return corridorStationError(t, spacingMm);
}

/** 测量最少站数，与后端 plan_corridor 一致：lyFlow 少于 3 站时不报错，但把有胶的帧测成无胶。 */
export const MIN_STATIONS = 3;
const MIN_SPACING_MM = 0.1;

/** 走廊按站距重采样的站数（含两端），与后端 station_count 一致（按像素算）。 */
export function stationCount(lengthPx: number, mmPerPx: number, spacingMm: number) {
  const step = spacingMm / mmPerPx;
  return Math.floor(Math.round(lengthPx * 1000) / 1000 / step + 1e-6 / step) + 1;
}

/** 让这条走廊够三站的最大站距（mm，取 0.1 mm 的整数倍）；走廊太短、站距到下限也不够时为 null。 */
function maxSpacing(lengthPx: number, mmPerPx: number) {
  let spacing = Math.floor(lengthPx * mmPerPx / (MIN_STATIONS - 1) * 10) / 10;
  if (spacing >= MIN_SPACING_MM && stationCount(lengthPx, mmPerPx, spacing) < MIN_STATIONS) spacing = Math.round((spacing - .1) * 10) / 10;
  return spacing >= MIN_SPACING_MM ? spacing : null;
}

const fixed = (value: number) => Number(value.toFixed(1));

/** 站数不足时的说明：实际站数、能用的最大站距或需要的走廊长度；站距或当量无效时不判（另有校验）。 */
export function corridorStationError(t: Pick<CameraTeachDraft, "corridor" | "mmPerPx">, spacingMm: number) {
  const scale = t.mmPerPx;
  if (!(scale != null && scale > 0 && Number.isFinite(spacingMm) && spacingMm > 0) || t.corridor.length < 2) return "";
  const lengthPx = pathLength(t.corridor);
  const n = stationCount(lengthPx, scale, spacingMm);
  if (n >= MIN_STATIONS) return "";
  const max = maxSpacing(lengthPx, scale);
  return `按站距 ${spacingMm} mm，这条走廊（${fixed(lengthPx * scale)} mm）只有 ${n} 站，至少要 ${MIN_STATIONS} 站：`
    + `${max != null ? `把站距改到不大于 ${max} mm，或` : ""}把走廊画到不短于 ${fixed(Math.ceil((MIN_STATIONS - 1) * spacingMm * 10 - 1e-9) / 10)} mm`;
}

/** 改站距前检查候选里已保存的各条段走廊，返回站数不足的走廊里要求最严的一条的说明；都够返回空串。 */
export function spacingError(doc: RecipeDoc, spacingMm: number) {
  if (!(Number.isFinite(spacingMm) && spacingMm >= MIN_SPACING_MM && spacingMm <= 10)) return "站距需在 0.1–10 mm 之间";
  const short = (doc.inspect ?? []).flatMap(inspect => {
    const scale = inspect.mmPerPx;
    if (!(scale != null && scale > 0)) return [];
    return (inspect.segments ?? []).flatMap(item => {
      if (item.corridor.length < 2) return [];
      const lengthPx = pathLength(item.corridor);
      const n = stationCount(lengthPx, scale, spacingMm);
      return n < MIN_STATIONS ? [{ camera: inspect.camera, segment: item.segment, lengthMm: lengthPx * scale, n, max: maxSpacing(lengthPx, scale) }] : [];
    });
  });
  if (!short.length) return "";
  const worst = short.reduce((a, b) => (b.max ?? 0) < (a.max ?? 0) ? b : a);
  return `站距 ${spacingMm} mm 时，${worst.camera} 第 ${worst.segment} 段走廊（${fixed(worst.lengthMm)} mm）只有 ${worst.n} 站，至少要 ${MIN_STATIONS} 站：`
    + (worst.max != null ? `站距需不大于 ${worst.max} mm` : "这条走廊太短，需重新示教")
    + (short.length > 1 ? `（共 ${short.length} 条走廊不足）` : "");
}

/** 走廊草稿的键（D-SB-45：走廊只按“段 × 检测相机”）：“相机#段号”。 */
export const draftKey = (camera: string, segment: number) => `${camera}#${segment}`;

export function parseDraftKey(key: string): { camera: string; segment: number } | null {
  const at = key.lastIndexOf("#");
  const segment = Number(key.slice(at + 1));
  return at < 0 || !Number.isInteger(segment) ? null : { camera: key.slice(0, at), segment };
}

/** 候选里已保存的段走廊草稿：走廊取这一段的，像素当量与检测参数取这台相机的。 */
export function savedDraft(doc: RecipeDoc | undefined, key: string): CameraTeachDraft {
  const target = parseDraftKey(key);
  const inspect = target ? doc?.inspect?.find(item => item.camera === target.camera) : undefined;
  const corridor = inspect?.segments?.find(item => item.segment === target?.segment)?.corridor ?? [];
  return {
    corridor: corridor.map(([x, y]) => [x, y] as [number, number]),
    mmPerPx: inspect?.mmPerPx ?? null,
    detect: inspect?.detect ? structuredClone(inspect.detect) : null,
    window: inspect?.window ? { ...inspect.window } : null,
  };
}

/** 把一条段走廊草稿写进配方副本（整份配方经 workspace_save_doc 提交）；配方里没有这台相机的项时原样返回。
 *  只写这一段的走廊，不动相机的像素当量、检测参数与其他段；清空的段走廊从配方里去掉（示教帧仍在，这一段回到待画走廊）。 */
export function withCorridor(doc: RecipeDoc, key: string, draft: CameraTeachDraft): RecipeDoc {
  const target = parseDraftKey(key);
  const inspect = doc.inspect ?? [];
  if (!target || !inspect.some(item => item.camera === target.camera)) return doc;
  return { ...doc, inspect: inspect.map(item => {
    if (item.camera !== target.camera) return item;
    const others = (item.segments ?? []).filter(s => s.segment !== target.segment);
    const segments = draft.corridor.length ? [...others, { segment: target.segment, corridor: draft.corridor }].sort((a, b) => a.segment - b.segment) : others;
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
