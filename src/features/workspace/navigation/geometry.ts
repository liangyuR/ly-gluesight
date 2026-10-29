import type { FrameRange, MapPoint, NavigationMapData } from "./types";

type Geometry = Pick<NavigationMapData, "points" | "closed" | "image">;
export interface PathEdge { from: MapPoint; to: MapPoint; start: number; end: number }
export interface Projection { fraction: number; point: MapPoint; distance: number }
export type MapHit = { kind: "hit"; segment: number; seq: number; fraction: number } | { kind: "ambiguous" } | { kind: "miss" };
const EPS = 1e-8;

export function pathEdges(map: Geometry): PathEdge[] {
  const vertices = map.points.map(([x, y]): MapPoint => [x * map.image.width, y * map.image.height]);
  if (map.closed && vertices.length > 2) vertices.push(vertices[0]);
  const edges: PathEdge[] = [];
  let length = 0;
  for (let i = 1; i < vertices.length; i++) {
    const from = vertices[i - 1], to = vertices[i];
    const size = Math.hypot(to[0] - from[0], to[1] - from[1]);
    if (size <= EPS) continue;
    edges.push({ from, to, start: length, end: length + size });
    length += size;
  }
  return length > 0 ? edges.map(edge => ({ ...edge, start: edge.start / length, end: edge.end / length })) : [];
}

export function pointOnPath(map: Geometry, fraction: number): MapPoint | null {
  const edges = pathEdges(map);
  const at = map.closed ? ((fraction % 1) + 1) % 1 : Math.max(0, Math.min(1, fraction));
  const edge = edges.find(item => at <= item.end + EPS) ?? edges.at(-1);
  if (!edge) return null;
  const t = Math.max(0, Math.min(1, (at - edge.start) / (edge.end - edge.start)));
  return [edge.from[0] + t * (edge.to[0] - edge.from[0]), edge.from[1] + t * (edge.to[1] - edge.from[1])];
}

export function pathSlice(map: Geometry, start: number, end: number): MapPoint[] {
  if (!(end > start)) return [];
  const edges = pathEdges(map);
  const points: MapPoint[] = [];
  for (let cycle = Math.floor(start); cycle <= Math.floor(end); cycle++) {
    for (const edge of edges) {
      const low = Math.max(start, cycle + edge.start), high = Math.min(end, cycle + edge.end);
      if (high - low <= EPS || (!map.closed && cycle !== 0)) continue;
      const from = pointOnPath(map, low), to = pointOnPath(map, high);
      if (from && to) points.push(from, to);
    }
  }
  return points;
}

export function projectPath(map: Geometry, point: MapPoint, start = 0, end = 1): Projection[] {
  const edges = pathEdges(map);
  const projections: Projection[] = [];
  for (let cycle = Math.floor(start); cycle <= Math.floor(end); cycle++) {
    for (const edge of edges) {
      const low = Math.max(start, cycle + edge.start), high = Math.min(end, cycle + edge.end);
      if (high - low <= EPS || (!map.closed && cycle !== 0)) continue;
      const dx = edge.to[0] - edge.from[0], dy = edge.to[1] - edge.from[1];
      const raw = ((point[0] - edge.from[0]) * dx + (point[1] - edge.from[1]) * dy) / (dx * dx + dy * dy);
      const edgeSpan = edge.end - edge.start;
      const t = Math.max((low - cycle - edge.start) / edgeSpan, Math.min((high - cycle - edge.start) / edgeSpan, raw));
      const projected: MapPoint = [edge.from[0] + t * dx, edge.from[1] + t * dy];
      projections.push({ fraction: cycle + edge.start + t * edgeSpan, point: projected, distance: Math.hypot(point[0] - projected[0], point[1] - projected[1]) });
    }
  }
  return projections.sort((a, b) => a.distance - b.distance).filter((item, i, all) => !all.slice(0, i).some(other => Math.abs(item.fraction - other.fraction) < EPS));
}

export function spatialRanges(map: NavigationMapData): FrameRange[] {
  const bounds = [map.captureStart, ...map.cuts, map.captureEnd];
  return bounds.slice(0, -1).map((value, i) => [value, bounds[i + 1]]);
}

export function mapIssues(map: NavigationMapData, segmentCount: number): string[] {
  const issues: string[] = [];
  if (!(Number.isFinite(map.image.width) && Number.isFinite(map.image.height) && map.image.width > 0 && map.image.height > 0)) issues.push("工件图尺寸无效");
  const unique = new Set(map.points.map(point => point.join(","))).size;
  if (unique < (map.closed ? 3 : 2) || !pathEdges(map).length) issues.push(map.closed ? "闭合胶路至少需要三个不同的点" : "请沿胶路至少标两个不同的点");
  const vertices = map.closed && map.points.length ? [...map.points, map.points[0]] : map.points;
  if (vertices.some((point, index) => index > 0 && point[0] === vertices[index - 1][0] && point[1] === vertices[index - 1][1])) issues.push("胶路不能包含相邻重复顶点或零长度闭合边");
  if (map.points.some(point => point.some(value => !Number.isFinite(value) || value < 0 || value > 1))) issues.push("胶路点必须在图片内");
  if (!Number.isFinite(map.captureStart) || !Number.isFinite(map.captureEnd) || map.captureStart < 0 || map.captureStart >= 1 || map.captureEnd <= map.captureStart || map.captureEnd - map.captureStart > 1 + EPS || (!map.closed && map.captureEnd > 1)) issues.push("采集范围必须沿方向递增，且不超过一圈");
  if (map.cuts.length !== Math.max(0, segmentCount - 1)) issues.push(`需要 ${Math.max(0, segmentCount - 1)} 个空间分界点，当前 ${map.cuts.length} 个`);
  if (spatialRanges(map).some(([a, b]) => !Number.isFinite(a) || !Number.isFinite(b) || b - a <= EPS)) issues.push("空间分界点必须沿胶路递增，且每段长度大于零");
  return issues;
}

export function frameFraction(map: NavigationMapData, ranges: FrameRange[], seq: number): { segment: number; fraction: number } | null {
  const segment = ranges.findIndex(([a, b]) => Number.isInteger(seq) && seq >= a && seq <= b);
  const range = ranges[segment], space = spatialRanges(map)[segment];
  if (!range || !space || range[1] < range[0] || !(space[1] > space[0])) return null;
  const t = (seq - range[0] + 0.5) / (range[1] - range[0] + 1);
  return { segment, fraction: space[0] + t * (space[1] - space[0]) };
}

export function fractionFrame(fraction: number, space: FrameRange, range: FrameRange): number | null {
  const n = range[1] - range[0] + 1;
  if (!Number.isInteger(n) || n <= 0 || !(space[1] > space[0])) return null;
  const t = Math.max(0, Math.min(1, (fraction - space[0]) / (space[1] - space[0])));
  return range[0] + Math.min(n - 1, Math.floor(t * n));
}

export function hitFrame(map: NavigationMapData, ranges: FrameRange[], point: MapPoint, threshold: number, preferredSegment?: number): MapHit {
  const spaces = spatialRanges(map);
  const hits = spaces.flatMap(([a, b], segment) => projectPath(map, point, a, b).filter(item => item.distance <= threshold).map(item => ({ ...item, segment })));
  const fullPathNearest = projectPath(map, point)[0];
  const capturedDistance = hits.reduce((nearest, item) => Math.min(nearest, item.distance), Infinity);
  if (fullPathNearest && fullPathNearest.distance + EPS < capturedDistance) return { kind: "miss" };
  const preferred = hits.filter(item => item.segment === preferredSegment);
  const sorted = (preferred.length ? preferred : hits).sort((a, b) => a.distance - b.distance);
  const best = sorted[0];
  if (!best) return { kind: "miss" };
  if (sorted.some(item => Math.abs(item.distance - best.distance) < EPS && (item.segment !== best.segment || Math.abs(item.fraction - best.fraction) > EPS))) return { kind: "ambiguous" };
  const seq = ranges[best.segment] && fractionFrame(best.fraction, spaces[best.segment], ranges[best.segment]);
  return seq == null ? { kind: "miss" } : { kind: "hit", segment: best.segment, seq, fraction: best.fraction };
}
