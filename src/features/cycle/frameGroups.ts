import type { FrameGroupView, RegionResult } from "../plan/types";

/** 在线和历史的帧组按原始序号升序。 */
export function firstGroupAtOrAfter(groups: readonly FrameGroupView[], seq: number) {
  let low = 0, high = groups.length;
  while (low < high) {
    const mid = low + Math.floor((high - low) / 2);
    if (groups[mid].seq < seq) low = mid + 1;
    else high = mid;
  }
  return low;
}

export function countSkippedGroups(groups: readonly FrameGroupView[], regions?: readonly RegionResult[] | null) {
  const skipped = regions?.filter(result => result.region.kind.type === "skipped") ?? [];
  skipped.sort((left, right) => left.region.firstSeq - right.region.firstSeq);
  let count = 0, coveredThrough = -Infinity;
  for (const { region } of skipped) {
    const start = Math.max(region.firstSeq, coveredThrough + 1);
    if (start > region.lastSeq) continue;
    count += firstGroupAtOrAfter(groups, region.lastSeq + 1) - firstGroupAtOrAfter(groups, start);
    coveredThrough = region.lastSeq;
  }
  return count;
}
