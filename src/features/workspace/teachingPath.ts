export type ImagePoint = [number, number];
export interface BrushGuide { points: ImagePoint[]; radiusPx: number }

export function resampleTeachingPath(path: ImagePoint[], spacing: number): ImagePoint[] {
  if (path.length < 2 || !Number.isFinite(spacing) || spacing < 4 || spacing > 80) return path;
  const anchors = new Set([0, path.length - 1]);
  const pending: [number, number][] = [[0, path.length - 1]];
  while (pending.length) {
    const [first, last] = pending.pop()!;
    const a = path[first], b = path[last];
    const dx = b[0] - a[0], dy = b[1] - a[1], squared = dx * dx + dy * dy;
    let farthest = -1, maximum = .9;
    for (let i = first + 1; i < last; i++) {
      const p = path[i];
      const t = squared ? Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / squared)) : 0;
      const distance = Math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy);
      if (distance > maximum) { maximum = distance; farthest = i; }
    }
    if (farthest !== -1) {
      anchors.add(farthest);
      pending.push([first, farthest], [farthest, last]);
    }
  }
  const indices = [...anchors].sort((a, b) => a - b);
  const output: ImagePoint[] = [[...path[0]]];
  for (let i = 1; i < indices.length; i++) {
    const start = path[indices[i - 1]], end = path[indices[i]];
    const length = Math.hypot(end[0] - start[0], end[1] - start[1]);
    const segments = Math.max(1, Math.ceil(length / (spacing - .15)));
    for (let j = 1; j < segments; j++) {
      const ratio = j / segments;
      output.push([Math.round((start[0] + (end[0] - start[0]) * ratio) * 10) / 10, Math.round((start[1] + (end[1] - start[1]) * ratio) * 10) / 10]);
    }
    output.push([...end]);
  }
  return output;
}
