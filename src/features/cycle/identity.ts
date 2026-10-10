import type { Measured, PartView } from "./types";

export function matchesPart(measured: Measured, part: PartView | null | undefined): boolean {
  if (!part || !part.cycleId || measured.cycleId !== part.cycleId || measured.bundleId !== part.bundleId || measured.sn !== part.sn) return false;
  const frame = part.frames[measured.k];
  return !!frame && !!frame.shotId && frame.shotId === measured.shotId && !!frame.camera && frame.camera === measured.camera;
}

export function mergeMeasurements(part: PartView, previous: Measured[], incoming: Measured[]): Measured[] {
  const frames = new Map<number, Measured>();
  for (const measured of [...previous, ...incoming]) {
    if (matchesPart(measured, part)) frames.set(measured.k, measured);
  }
  return [...frames.values()].sort((a, b) => a.k - b.k);
}
