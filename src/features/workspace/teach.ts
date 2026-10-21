import type { FixtureSpec, ShotSpec } from "../cycle/types";
import type { FixtureOverlay } from "./components";
import { pathLength } from "../cycle/vis";
import type { ShotTeach } from "./types";

/** 候选配方里拍照点已保存的示教；没有像素当量时为 NaN（输入框显示为空）。 */
export function shotTeach(shot: Pick<ShotSpec, "path" | "mmPerPx" | "detect" | "limits" | "fixture">): ShotTeach {
  return { path: shot.path.map(([x, y]) => [x, y] as [number, number]), mmPerPx: shot.mmPerPx ?? null, ...(shot.detect ? { detect: structuredClone(shot.detect) } : {}), ...(shot.limits ? { limits: structuredClone(shot.limits) } : {}), fixture: shot.fixture ? structuredClone(shot.fixture) : null };
}

const canonical = (v: unknown): unknown => Array.isArray(v) ? v.map(canonical)
  : v && typeof v === "object" ? Object.fromEntries(Object.keys(v).sort().map(key => [key, canonical((v as Record<string, unknown>)[key])]))
  : typeof v === "number" ? Math.fround(v) : v;

/**
 * 按 f32 比较：后端以 f32 存中线与参数，取回的值与草稿在 f32 精度内相同就算没改。
 * 字段顺序不计（新画的定位框是前端拼的对象，后端回来的字段顺序不同）；定位框的 source 不计：它只由后端写入、前端传的不用，
 * 新框保存后后端改绑到当前示教图，草稿里的空来源不能让这一帧永远停在“有未试测的修改”。
 */
export function sameTeach(a: ShotTeach | undefined, b: ShotTeach | undefined) {
  const norm = (t: ShotTeach | undefined) => JSON.stringify(canonical(t ? { ...t, fixture: t.fixture && { ...t.fixture, source: "" } } : null));
  return norm(a) === norm(b);
}

/** 草稿能否保存：至少两点、点在图内为有限数、长度不为零，像素当量为正。返回错误说明，空串表示可以保存。 */
export function teachError(t: ShotTeach) {
  if (!(t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0 && t.mmPerPx <= 10)) return "像素当量需在 0–10 mm/px 之间";
  if (t.path.some((p) => !p.every(Number.isFinite))) return "中线坐标必须是有限数";
  if (t.path.length === 1 || (t.path.length >= 2 && pathLength(t.path) < 1)) return "中线至少两个点、长度不能为零";
  if (t.detect) {
    const { searchMm, widthRange: [lo, hi] } = t.detect;
    if (!(Number.isFinite(searchMm) && searchMm > 0 && Number.isFinite(lo) && Number.isFinite(hi) && lo > 0 && lo < hi)) return "搜索半宽需为正，胶宽范围需满足 0 < 下限 < 上限";
    if (hi >= 2 * searchMm) return `胶宽上限要小于搜索宽度 ${(2 * searchMm).toFixed(1)} mm`;
  }
  const position = t.limits?.position;
  if (position && position.nominal !== 0 && !t.limits?.gaugeRef?.trim()) return "位置名义值不为 0 时必须填写量具确认件编号";
  if (t.fixture) return fixtureError(t.fixture);
  return "";
}

/** 定位模板外圈（与 recipe.rs 的 FIXTURE_TEMPLATE_PAD_PX 一致） */
export const FIXTURE_PAD_PX = 4;
/** 新画的定位框的门限：与阶段 0 评测一致；v1 只找平移（定稿 §10 决定二） */
export const FIXTURE_DEFAULTS = { searchRadiusPx: 60, angleRangeDeg: 0, minScore: .6, maxPeakRatio: 1 } as const;

/** 定位框的前置校验（与 recipe.rs FixtureSpec::validate 同口径，右、下边与走廊由后端检查）。 */
export function fixtureError(f: FixtureSpec) {
  const [x, y, w, h] = f.rect;
  if (!f.rect.every(Number.isInteger)) return "定位框坐标与尺寸必须是整数像素";
  if (x < FIXTURE_PAD_PX || y < FIXTURE_PAD_PX) return `定位框离图像左、上边至少 ${FIXTURE_PAD_PX} px（模板要带 ${FIXTURE_PAD_PX} px 外圈）`;
  if (w < 16 || w > 512 || h < 16 || h > 512) return "定位框宽、高需在 16–512 px 之间";
  if (!(f.searchRadiusPx >= 3 && f.searchRadiusPx <= 200)) return "定位搜索半径需在 3–200 px 之间";
  if (!(f.minScore > 0 && f.minScore <= 1)) return "定位最低分数需在 0–1 之间（不含 0）";
  if (!(f.maxPeakRatio > 0 && f.maxPeakRatio <= 1)) return "定位次峰比上限需在 0–1 之间（不含 0）";
  return "";
}

/** 叠图用的定位几何：带外圈的模板框、搜索范围（示教框四周各扩 searchRadius）与杠杆臂 L（框中心 → 最远中线顶点）。 */
export function fixtureOverlay(rect: [number, number, number, number], searchRadiusPx: number, path: [number, number][], blocked = false): FixtureOverlay {
  const [x, y, w, h] = rect, p = FIXTURE_PAD_PX, r = Number.isFinite(searchRadiusPx) ? searchRadiusPx : 0;
  const center: [number, number] = [x + (w - 1) / 2, y + (h - 1) / 2];
  const far = path.reduce<[number, number] | null>((best, q) => !best || Math.hypot(q[0] - center[0], q[1] - center[1]) > Math.hypot(best[0] - center[0], best[1] - center[1]) ? q : best, null);
  return { rect, padded: [x - p, y - p, w + 2 * p, h + 2 * p], search: [x - r, y - r, w + 2 * r, h + 2 * r], lever: far ? [center, far] : null, blocked };
}

export function selectedViews(shot: ShotSpec): number[] {
  if (shot.skip) return [];
  return shot.views?.length ? shot.views.filter(v => v.enabled).map(v => v.view) : [shot.view];
}
export function canImportSampleGroup(shots: Pick<ShotSpec, "skip" | "views">[]) {
  return shots.every(shot => shot.skip || (shot.views?.length ? shot.views.filter(v => v.enabled).length : 1) <= 1);
}
