/** 逐帧定向的几何部件，面板行、示意图与画布叠加用同一编号（D-SB-47）。 */
export type HeadingPart = "nozzle" | "skip" | "zone" | "sector" | "reading";
export const HEADING_MARK: Record<HeadingPart, string> = { nozzle: "1", skip: "2", zone: "3", sector: "4", reading: "5" };

/** 找方向的扇形扫描只看测量区间里离出胶点最近的这一段（px），与后端 `vision/heading.rs` 的 FAN_LENGTH_PX 一致。 */
export const FAN_LENGTH_PX = 150;

/**
 * 搜索扇区（度）：以出胶点为圆心的半圈，中线取拖过的方向 `turned`，没拖过时背离离出胶点最近的画面边。
 * 与后端 `heading::search_sector`（测量图用它）同一规则；示教页自己算，保存前后显示不换来源。
 */
export function searchSector(nozzle: [number, number], size: [number, number], turned: number | null | undefined): [number, number] {
  const mid = turned ?? [[nozzle[0], 0], [size[0] - 1 - nozzle[0], 180], [nozzle[1], 90], [size[1] - 1 - nozzle[1], -90]]
    .reduce((best, edge) => edge[0] < best[0] ? edge : best)[1];
  return [mid - 90, mid + 90];
}

/** 逐帧定向叠加（D-SB-47）：出胶点、测量区间环带与搜索扇区，均为这张图的像素坐标与度数（0° 朝右、90° 朝下）。 */
export interface HeadingGuide {
  nozzle: [number, number];
  /** 标定的出胶点；否则为走廊起点推算 */
  calibrated: boolean;
  /** 测量区间 [内半径, 外半径]（px）；未设置时为 null */
  zone: [number, number] | null;
  /** 搜索扇区（`searchSector`） */
  sector?: [number, number] | null;
  /** 扇区方向已拖动过（存在配方里）：把手不再脉冲提示 */
  turned?: boolean;
  /** 试测时这一帧测得的胶条方向 */
  measured?: number | null;
  /** 面板上正指着的部件：只突出它和出胶点 */
  focus?: HeadingPart | null;
}

const at = (c: [number, number], r: number, deg: number): [number, number] => {
  const t = deg * Math.PI / 180;
  return [c[0] + r * Math.cos(t), c[1] + r * Math.sin(t)];
};

/** 圆心 c、半径 [r0, r1]、角度 [lo, hi] 的扇环路径；跨度 ≥ 360° 时为整个圆环，r0 为 0 时为扇形。 */
export function wedgePath(c: [number, number], [r0, r1]: [number, number], [lo, hi]: [number, number]) {
  const f = (v: number) => Math.round(v * 100) / 100;
  const pt = (p: [number, number]) => `${f(p[0])} ${f(p[1])}`;
  if (hi - lo >= 360) {
    const ring = (r: number) => r > 0 ? `M${pt(at(c, r, 0))}A${f(r)} ${f(r)} 0 1 1 ${pt(at(c, r, 180))}A${f(r)} ${f(r)} 0 1 1 ${pt(at(c, r, 360))}Z` : "";
    return ring(r1) + ring(r0);
  }
  const large = hi - lo > 180 ? 1 : 0;
  const outer = `M${pt(at(c, r1, lo))}A${f(r1)} ${f(r1)} 0 ${large} 1 ${pt(at(c, r1, hi))}`;
  const inner = r0 > 0 ? `L${pt(at(c, r0, hi))}A${f(r0)} ${f(r0)} 0 ${large} 0 ${pt(at(c, r0, lo))}` : `L${pt(c)}`;
  return outer + inner + "Z";
}

/** 方向线的绘制半径：有测量区间时用区间，否则用图像短边的 30%。 */
export function guideRadii(guide: HeadingGuide, size: [number, number]): [number, number] {
  return guide.zone ?? [0, Math.min(size[0], size[1]) * .3];
}

/** 扇区的绘制半径：扇形扫描只看区间前 FAN_LENGTH_PX。 */
export function fanRadii(guide: HeadingGuide, size: [number, number]): [number, number] {
  const [r0, r1] = guideRadii(guide, size);
  return [r0, r0 + Math.min(FAN_LENGTH_PX, r1 - r0)];
}

export function rayEnd(c: [number, number], r: number, deg: number) {
  return at(c, r, deg);
}
