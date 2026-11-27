type Point = [number, number];

export function prepareSearchBand(path: Point[], half: number | undefined) {
  if (half == null || !Number.isFinite(half) || half <= 0) return null;
  const vertices: Point[] = [], arc: number[] = [];
  for (const p of path) {
    if (!Number.isFinite(p[0]) || !Number.isFinite(p[1])) return null;
    const previous = vertices[vertices.length - 1];
    const distance = previous ? Math.hypot(p[0] - previous[0], p[1] - previous[1]) : 0;
    if (previous && distance < 1e-9) continue;
    arc.push(previous ? arc[arc.length - 1] + distance : 0);
    vertices.push(p);
  }
  if (vertices.length < 2) return null;
  const total = arc[arc.length - 1];
  if (!Number.isFinite(total) || total <= 0) return null;
  return { vertices, arc, total, half };
}

export function searchBand({ vertices, arc, total, half }: NonNullable<ReturnType<typeof prepareSearchBand>>) {
  let segment = 0;
  const pointAt = (s: number): Point => {
    while (segment + 2 < vertices.length && arc[segment + 1] < s) segment++;
    const span = arc[segment + 1] - arc[segment];
    const f = span > 1e-12 ? Math.max(0, Math.min(1, (s - arc[segment]) / span)) : 0;
    const a = vertices[segment], b = vertices[segment + 1];
    return [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
  };
  const regularLimit = total - 1e-9;
  const spacing = Math.max(half * 2, total / 8);
  const left: string[] = [], right: string[] = [];
  const bars: [Point, Point][] = [];
  let distance = regularLimit > 0 ? 0 : total;
  let current = pointAt(distance);
  let previous = current;
  let first = true;
  let lastBar = 0;
  while (true) {
    const finished = distance === total;
    const nextDistance = distance + 2 < regularLimit ? distance + 2 : total;
    const next = finished ? current : pointAt(nextDistance);
    const dx = next[0] - previous[0], dy = next[1] - previous[1];
    const length = Math.hypot(dx, dy);
    const nx = length > 1e-12 ? -dy / length * half : 0;
    const ny = length > 1e-12 ? dx / length * half : half;
    const lx = current[0] + nx, ly = current[1] + ny;
    const rx = current[0] - nx, ry = current[1] - ny;
    left.push(`${lx},${ly}`);
    right.push(`${rx},${ry}`);
    if (first || finished || distance - lastBar >= spacing) {
      const bar: [Point, Point] = [[lx, ly], [rx, ry]];
      bars.push(bar);
      if (first && finished) bars.push(bar);
      lastBar = distance;
    }
    if (finished) break;
    first = false;
    previous = current;
    current = next;
    distance = nextDistance;
  }
  return { left: left.join(" "), right: right.join(" "), bars };
}
