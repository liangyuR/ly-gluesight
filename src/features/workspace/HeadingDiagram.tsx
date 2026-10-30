import { HEADING_MARK, rayEnd, wedgePath, type HeadingPart } from "./headingOverlay";

const C: [number, number] = [40, 34];
const ZONE: [number, number] = [20, 196];
const FAN: [number, number] = [20, 110];
const RANGE: [number, number] = [28, 50];
const SECTOR: [number, number] = [10, 72];
const FALLBACK = 39;
const READING = 34;

/** 逐帧定向示意（不按比例，D-SB-47）：编号与面板各行、画布叠加一致；`focus` 指着的部件突出，其余变淡。 */
export function HeadingDiagram({ focus }: { focus: HeadingPart | null }) {
  const dim = (part: HeadingPart) => focus && focus !== part ? .15 : 1;
  const ray = (r0: number, r1: number, deg: number) => {
    const [x1, y1] = rayEnd(C, r0, deg);
    const [x2, y2] = rayEnd(C, r1, deg);
    return { x1, y1, x2, y2 };
  };
  const mark = (part: HeadingPart, r: number, deg: number) => {
    const [x, y] = rayEnd(C, r, deg);
    return <g className="mark" opacity={dim(part)}><circle cx={x} cy={y} r={8} /><text x={x} y={y} textAnchor="middle" dominantBaseline="central">{HEADING_MARK[part]}</text></g>;
  };
  return <svg className="wp-heading-diagram" viewBox="0 0 280 190" role="img"
    aria-label="逐帧定向示意：以出胶点为圆心，先跳过胶嘴附近，再在测量区间里测；在搜索扇区里找胶条方向，示教方向附近优先，找不到时按示教方向测">
    <path className="bead" d="M40 34Q140 94 270 188" />
    <path className="zone" opacity={dim("zone")} d={wedgePath(C, ZONE, [0, 360])} fillRule="evenodd" />
    <path className="skip" opacity={dim("skip")} d={wedgePath(C, [0, ZONE[0]], [0, 360])} />
    <path className="sector" opacity={dim("sector")} d={wedgePath(C, FAN, SECTOR)} />
    <path className="range" opacity={dim("range")} d={wedgePath(C, FAN, RANGE)} />
    <line className="fallback" opacity={dim("fallback")} {...ray(0, ZONE[1], FALLBACK)} />
    <line className="reading" opacity={dim("reading")} {...ray(ZONE[0], ZONE[1] - 12, READING)} />
    <circle className="nozzle" cx={C[0]} cy={C[1]} r={4} />
    {mark("nozzle", 28, 200)}
    {mark("skip", 11, 120)}
    {mark("zone", 120, 95)}
    {mark("sector", 100, 14)}
    {mark("range", 85, 46)}
    {mark("fallback", 185, 44)}
    {mark("reading", 175, 28)}
  </svg>;
}
