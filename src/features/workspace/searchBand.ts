type Point = [number, number];

export function searchBand(path: Point[], half: number | undefined) {
  if (half == null || !Number.isFinite(half) || half <= 0 || path.some(p => !p.every(Number.isFinite))) return null;
  const vertices: Point[] = [], arc: number[] = [];
  for (const p of path) {
    const previous = vertices[vertices.length - 1];
    const distance = previous ? Math.hypot(p[0] - previous[0], p[1] - previous[1]) : 0;
    if (previous && distance < 1e-9) continue;
    arc.push(previous ? arc[arc.length - 1] + distance : 0);
    vertices.push(p);
  }
  if (vertices.length < 2) return null;
  const total = arc[arc.length - 1];
  if (!Number.isFinite(total) || total <= 0) return null;
  const points: Point[] = [], distances: number[] = [];
  for (let s = 0; s < total - 1e-9; s += 2) distances.push(s);
  distances.push(total);
  let segment = 0;
  for (const s of distances) {
    while (segment + 2 < vertices.length && arc[segment + 1] < s) segment++;
    const span = arc[segment + 1] - arc[segment];
    const f = span > 1e-12 ? Math.max(0, Math.min(1, (s - arc[segment]) / span)) : 0;
    const a = vertices[segment], b = vertices[segment + 1];
    points.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]);
  }
  const left: Point[] = [], right: Point[] = [];
  for (let i = 0; i < points.length; i++) {
    const previous = points[Math.max(0, i - 1)], next = points[Math.min(points.length - 1, i + 1)];
    const dx = next[0] - previous[0], dy = next[1] - previous[1];
    const length = Math.hypot(dx, dy);
    const nx = length > 1e-12 ? -dy / length * half : 0;
    const ny = length > 1e-12 ? dx / length * half : half;
    left.push([points[i][0] + nx, points[i][1] + ny]);
    right.push([points[i][0] - nx, points[i][1] - ny]);
  }
  const spacing = Math.max(half * 2, distances[distances.length - 1] / 8);
  let last = 0;
  const bars = [0];
  for (let i = 1; i < points.length - 1; i++) {
    if (distances[i] - last >= spacing) { bars.push(i); last = distances[i]; }
  }
  bars.push(points.length - 1);
  return { left, right, bars };
}
