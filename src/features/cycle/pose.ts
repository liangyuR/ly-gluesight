import type { FixtureSpec, FrameView, PartView, PoseRecord, Recipe, ShotSpec } from "./types";

/** 图像像素坐标（与 lyFlow 一致：像素中心为整数，y 向下）。 */
export type Pt = [number, number];
type Rect = [number, number, number, number];

/** 示教框中心：(x + (w − 1)/2, y + (h − 1)/2)，与 glue.locate 的 teachCenter、recipe.rs 同一口径。 */
export const teachCenter = ([x, y, w, h]: Rect): Pt => [x + (w - 1) / 2, y + (h - 1) / 2];

/**
 * 位姿的变换 T(p) = R(angle)·(p − tc) + tc + offset，不看是否被采用（示教页画被拒的候选落点用）。
 * 与 vision/fixture.rs 的 `Pose::apply` 逐字对齐：y 向下，angle（度）> 0 为屏幕顺时针，R·v = [c·x − s·y, s·x + c·y]；
 * offset = center − teachCenter（`PoseRecord.offset`），tc 由快照里的示教框还原。offset 或 angle 不是有限数时为 null。
 */
export function rawPoseTransform(rect: Rect, pose: PoseRecord | null | undefined): ((p: Pt) => Pt) | null {
  if (!pose?.offset || pose.angle == null) return null;
  const [dx, dy] = pose.offset, angle = pose.angle;
  if (![dx, dy, angle].every(Number.isFinite)) return null;
  const [cx, cy] = teachCenter(rect);
  const r = angle * Math.PI / 180, c = Math.cos(r), s = Math.sin(r);
  return ([x, y]) => { const u = x - cx, v = y - cy; return [c * u - s * v + cx + dx, s * u + c * v + cy + dy]; };
}

/** 叠图用的位姿变换：只有被采用的位姿才搬基准。未采用的位姿即使 offset 是有限数（例如贴边错锁 38 px）也返回 null。 */
export const poseTransform = (rect: Rect, pose: PoseRecord | null | undefined) => pose?.accepted ? rawPoseTransform(rect, pose) : null;

/** 拍照点在图 view 上的中线与特征定位，与 recipe.rs 的 `ShotSpec::for_view` 同口径：有 views[] 时取该视图的，shot 级 fixture 可以为空。 */
export function viewSpec(shot: ShotSpec, view: number): { path: Pt[]; fixture: FixtureSpec | null } | null {
  if (shot.views?.length) {
    const v = shot.views.find(item => item.view === view);
    return v ? { path: v.path, fixture: v.fixture ?? null } : null;
  }
  return view === shot.view ? { path: shot.path, fixture: shot.fixture ?? null } : null;
}

/** 拍照点 k 在图 view 上的测点下标（与 recipe.rs 的 `view_points` 一致）。 */
export const viewStations = (layout: Pick<Recipe, "segments">, k: number, view: number): number[] =>
  layout.segments.filter(g => g.shot === k && g.view === view).flatMap(g => Array.from({ length: g.count }, (_, i) => g.first + i));

/** 帧记录里图 view 的位姿（G3 起 `viewResults[].pose`）；没用定位、测量出错或还没测时为 null。 */
export const framePose = (frame: Pick<FrameView, "viewResults"> | null | undefined, view: number): PoseRecord | null =>
  frame?.viewResults?.find(v => v.view === view)?.pose ?? null;

/**
 * 件的帧只能和测出它的配方快照配对（与 vis.ts 的 computeVis 同口径）：件的配方或修订与布局不同时返回 null，
 * 否则换修订后用旧件的位姿配新框会画错。生产页的叠图与缩略图都经过它。
 */
export const partFrames = (layout: Pick<Recipe, "id" | "revisionId"> | null | undefined, part: Pick<PartView, "recipeId" | "recipeRevision" | "frames"> | null | undefined) =>
  layout && part && part.recipeId === layout.id && part.recipeRevision === layout.revisionId ? part.frames : null;

/**
 * 示教框加 pad 像素外圈的四角（多边形，按 T 搬移；不给 T 为示教位置）。
 * 角点取像素中心坐标下的 [x − 0.5, x + w − 0.5]，与中线（像素中心坐标）同一约定，示教页与生产、历史页的框一致。
 */
export function movedBox([x, y, w, h]: Rect, pad: number, T?: ((p: Pt) => Pt) | null): Pt[] {
  const x0 = x - pad - .5, y0 = y - pad - .5, x1 = x + w + pad - .5, y1 = y + h + pad - .5;
  const corners: Pt[] = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
  return T ? corners.map(T) : corners;
}

/** 未采用原因的显示文字，与 Rust 侧 `vision::fixture_quality::reason_text` 同一张表。 */
export function poseReasonText(reason: string | null | undefined) {
  switch (reason) {
    case null: case undefined: case "": return "定位失败";
    case "low_score": return "匹配分数低于最低分数";
    case "ambiguous": return "有相近的次峰（次峰比超过上限，模板不唯一）";
    case "at_search_edge": return "峰贴到搜索范围边上（偏移超出搜索半径）";
    case "at_angle_edge": return "角度贴到扫描边界";
    case "flat_template": return "模板纹理太弱";
    case "out_of_image": return "模板或搜索区超出图像";
    case "angle_unobservable": return "开了转角但看不出角度";
    default: return `定位失败（${reason}）`;
  }
}

const num = (v: number | null | undefined, digits: number) => v == null || !Number.isFinite(v) ? "—" : v.toFixed(digits);

/** 一行定位摘要：采用 · 偏移 (dx, dy) px · θ° · 两端排除 a / b mm，或“未采用：原因”。 */
export function poseSummary(pose: PoseRecord | null | undefined) {
  if (!pose) return "—";
  if (!pose.accepted) return "未采用：" + poseReasonText(pose.reason);
  const [dx, dy] = pose.offset ?? [NaN, NaN];
  const excluded = pose.excludedMm ? ` · 两端排除 ${num(pose.excludedMm[0], 1)} / ${num(pose.excludedMm[1], 1)} mm` : "";
  return `采用 · 偏移 (${num(dx, 1)}, ${num(dy, 1)}) px · ${num(pose.angle, 2)}°${excluded}`;
}
