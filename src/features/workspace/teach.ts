import type { ShotSpec } from "../cycle/types";
import { pathLength } from "../cycle/vis";
import type { ShotTeach } from "./types";

/** 候选配方里拍照点已保存的示教；没有像素当量时为 NaN（输入框显示为空）。 */
export function shotTeach(shot: Pick<ShotSpec, "path" | "mmPerPx" | "detect">): ShotTeach {
  return { path: shot.path.map(([x, y]) => [x, y] as [number, number]), mmPerPx: shot.mmPerPx ?? NaN, ...(shot.detect ? { detect: structuredClone(shot.detect) } : {}) };
}

/** 按 f32 比较：后端以 f32 存中线与参数，取回的值与草稿在 f32 精度内相同就算没改。 */
export function sameTeach(a: ShotTeach | undefined, b: ShotTeach | undefined) {
  const norm = (t: ShotTeach | undefined) => JSON.stringify(t ?? null, (_k, v) => (typeof v === "number" ? Math.fround(v) : v));
  return norm(a) === norm(b);
}

/** 草稿能否保存：至少两点、点在图内为有限数、长度不为零，像素当量为正。返回错误说明，空串表示可以保存。 */
export function teachError(t: ShotTeach) {
  if (!(Number.isFinite(t.mmPerPx) && t.mmPerPx > 0 && t.mmPerPx <= 10)) return "像素当量需在 0–10 mm/px 之间";
  if (t.path.some((p) => !p.every(Number.isFinite))) return "中线坐标必须是有限数";
  if (t.path.length === 1 || (t.path.length >= 2 && pathLength(t.path) < 1)) return "中线至少两个点、长度不能为零";
  if (t.detect) {
    const { searchMm, widthRange: [lo, hi] } = t.detect;
    if (!(Number.isFinite(searchMm) && searchMm > 0 && Number.isFinite(lo) && Number.isFinite(hi) && lo > 0 && lo < hi)) return "搜索半宽需为正，胶宽范围需满足 0 < 下限 < 上限";
    if (hi >= 2 * searchMm) return `胶宽上限要小于搜索宽度 ${(2 * searchMm).toFixed(1)} mm`;
  }
  return "";
}
