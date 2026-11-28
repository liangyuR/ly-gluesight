export type ImagePoint = [number, number];
export interface BrushGuide { points: ImagePoint[]; radiusPx: number }

/** 折线总长（像素）。 */
export function pathLength(path: ImagePoint[]): number {
  let length = 0;
  for (let i = 1; i < path.length; i++) length += Math.hypot(path[i][0] - path[i - 1][0], path[i][1] - path[i - 1][1]);
  return length;
}

export function resampleTeachingPath(path: ImagePoint[], spacing: number): ImagePoint[] {
  if (path.length < 2 || !Number.isFinite(spacing) || spacing < 4 || spacing > 80) return path;
  const anchors = new Uint8Array(path.length);
  anchors[0] = anchors[path.length - 1] = 1;
  const pending: [number, number][] = [[0, path.length - 1]];
  while (pending.length) {
    const [first, last] = pending.pop()!;
    const a = path[first], b = path[last];
    const dx = b[0] - a[0], dy = b[1] - a[1], squared = dx * dx + dy * dy;
    let farthest = -1, maximum = .9;
    let bound = maximum * .7;
    for (let i = first + 1; i < last; i++) {
      const p = path[i];
      const x = p[0] - a[0], y = p[1] - a[1];
      const t = squared ? Math.max(0, Math.min(1, (x * dx + y * dy) / squared)) : 0;
      const rx = x - t * dx, ry = y - t * dy;
      // 0.7 × √2 < 1，接近最大值的点仍用精确距离比较。
      if (Math.abs(rx) <= bound && Math.abs(ry) <= bound) continue;
      const distance = Math.hypot(rx, ry);
      if (distance > maximum) {
        maximum = distance;
        bound = maximum * .7;
        farthest = i;
      }
    }
    if (farthest !== -1) {
      anchors[farthest] = 1;
      if (farthest - first > 1) pending.push([first, farthest]);
      if (last - farthest > 1) pending.push([farthest, last]);
    }
  }
  const output: ImagePoint[] = [[...path[0]]];
  let start = path[0];
  for (let i = 1; i < path.length; i++) {
    if (!anchors[i]) continue;
    const end = path[i];
    const dx = end[0] - start[0], dy = end[1] - start[1];
    const length = Math.hypot(dx, dy);
    const segments = Math.max(1, Math.ceil(length / (spacing - .15)));
    for (let j = 1; j < segments; j++) {
      const ratio = j / segments;
      output.push([Math.round((start[0] + dx * ratio) * 10) / 10, Math.round((start[1] + dy * ratio) * 10) / 10]);
    }
    output.push([...end]);
    start = end;
  }
  return output;
}
