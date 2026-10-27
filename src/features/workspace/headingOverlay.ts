/** 逐帧定向叠加（D-SB-47）：出胶点、检测窗口环带、本段方向范围、搜索扇区与回退方向，均为这张图的像素坐标与度数（0° 朝右、90° 朝下）。 */
export interface HeadingGuide {
  nozzle: [number, number];
  /** 标定的出胶点；否则为走廊起点推算 */
  calibrated: boolean;
  /** 检测窗口 [内半径, 外半径]（px）；未设置时为 null */
  zone: [number, number] | null;
  range?: [number, number] | null;
  sector?: [number, number] | null;
  fallback?: number | null;
  /** 试测时这一帧测得的胶条方向 */
  measured?: number | null;
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

/** 方向与扇区的绘制半径：有检测窗口时用窗口，否则用图像短边的 30%。 */
export function guideRadii(guide: HeadingGuide, size: [number, number]): [number, number] {
  return guide.zone ?? [0, Math.min(size[0], size[1]) * .3];
}

export function rayEnd(c: [number, number], r: number, deg: number) {
  return at(c, r, deg);
}
