import type { FrameCellStatus, FrameGroupView, OffsetUnjudged, PartViewBurst, PathSegment, Region, UncheckedItem, Uninspected } from "../plan/types";
import type { ResultView, Verdict } from "./types";

export type IncompleteFrameStatus = Extract<FrameCellStatus, "missing" | "error" | "unmeasured" | "outOfCorridor">;

export interface CameraCoverageEvidence {
  camera: number;
  name: string;
  status: IncompleteFrameStatus;
  sequences: number[];
}

export interface CoverageEvidence {
  region: Region;
  sequences: number[];
  cameras: CameraCoverageEvidence[];
  locateSeq: number | null;
}

export interface OffsetEvidence {
  region: Region;
  reason: OffsetUnjudged;
  camera: string | null;
}

export interface DefectEvidence {
  region: Region;
  verdict: Verdict;
  reason: string;
  frame: { seq: number; camera: number; name: string } | null;
  locateSeq: number | null;
}

export interface PartEvidence {
  coverage: {
    status: "complete" | "incomplete" | "offsetUnjudged" | "notJudged";
    affected: CoverageEvidence[];
    offsetUnjudged: OffsetEvidence[];
    uninspected: Uninspected[];
    unchecked: UncheckedItem[];
  };
  defects: DefectEvidence[];
}

function isIncomplete(status: FrameCellStatus | undefined): status is IncompleteFrameStatus {
  return status === "missing" || status === "error" || status === "unmeasured" || status === "outOfCorridor";
}

function isNg(verdict: Verdict) {
  return verdict === "NG_POSITION" || verdict === "NG_WIDTH" || verdict === "NG_ABSOLUTE" || verdict === "NG_GAP";
}

function applicableCameras(region: Region, segment: PathSegment | undefined, cameras: readonly string[]) {
  if (region.kind.type === "unsegmented") {
    // 过渡区逐帧选相机，区域范围无法确定某一帧应该由哪台相机检测。
    return region.kind.reason === "transition" ? [] : cameras.map((_, i) => i);
  }
  if (!segment || segment.skip) return [];
  return cameras.flatMap((name, i) => !segment.cameras.length || segment.cameras.includes(name) ? [i] : []);
}

// 在线快照按帧号升序生成，正常情况下从 1 连续编号。
function firstGroupAtOrAfter(groups: readonly FrameGroupView[], seq: number) {
  let low = 0, high = groups.length;
  while (low < high) {
    const mid = low + Math.floor((high - low) / 2);
    if (groups[mid].seq < seq) low = mid + 1;
    else high = mid;
  }
  return low;
}

function incompleteGroupEvidence(sequences: readonly number[], groups: readonly FrameGroupView[], relevant: readonly number[], names: readonly string[]) {
  const findings = new Map<string, CameraCoverageEvidence>();
  const dense = groups.at(-1)?.seq === groups.length;
  let firstSeq: number | null = null;
  for (const seq of sequences) {
    const group = groups[dense ? seq - 1 : firstGroupAtOrAfter(groups, seq)];
    if (group?.seq !== seq) continue;
    firstSeq ??= seq;
    for (const camera of relevant) {
      const status = group.cells[camera]?.status;
      if (!isIncomplete(status)) continue;
      const key = `${camera}:${status}`;
      const finding = findings.get(key);
      if (finding) finding.sequences.push(seq);
      else findings.set(key, { camera, name: names[camera], status, sequences: [seq] });
    }
  }
  return { cameras: [...findings.values()], firstSeq };
}

function defectEvidence(region: Region, verdict: Verdict, reason: string, groups: readonly FrameGroupView[], cameras: readonly string[]): DefectEvidence {
  let contextSeq: number | null = null;
  let measuredSeq: number | null = null;
  for (let index = firstGroupAtOrAfter(groups, region.firstSeq); index < groups.length; index++) {
    const group = groups[index];
    if (group.seq > region.lastSeq) break;
    contextSeq ??= group.seq;
    const camera = group.cells.findIndex((cell, i) => i < cameras.length && cell.status === "measured" && cell.verdict === verdict);
    if (camera >= 0) return { region, verdict, reason, frame: { seq: group.seq, camera, name: cameras[camera] }, locateSeq: group.seq };
    if (measuredSeq === null && group.cells.some(cell => cell.status === "measured")) measuredSeq = group.seq;
  }
  return { region, verdict, reason, frame: null, locateSeq: measuredSeq ?? contextSeq };
}

export function buildPartEvidence(result: ResultView, burst: PartViewBurst | null, segments: readonly PathSegment[]): PartEvidence {
  const judgement = result.mode === "maintenanceCollect" ? null : result.burst;
  const flags = judgement?.flags;
  const coverage: PartEvidence["coverage"] = {
    status: flags?.incomplete ? "incomplete" : flags?.offsetUnjudged ? "offsetUnjudged" : judgement && result.verdict !== "ERR_INSPECT" ? "complete" : "notJudged",
    affected: [],
    offsetUnjudged: [],
    uninspected: judgement?.uninspected ?? [],
    unchecked: judgement?.unchecked ?? [],
  };
  const defects: DefectEvidence[] = [];
  const groups = burst?.groups ?? [];
  const names = burst?.cameras ?? [];
  const productNg = isNg(result.verdict);
  for (const entry of judgement?.regions ?? []) {
    const { region, incompleteGroups: sequences, offsetUnjudged } = entry;
    const { kind } = region;
    if (kind.type === "skipped") continue;
    const segment = kind.type !== "unsegmented" && (sequences.length || offsetUnjudged)
      ? segments.find(candidate => candidate.index === kind.segment)
      : undefined;
    if (sequences.length) {
      const { cameras, firstSeq } = incompleteGroupEvidence(sequences, groups, applicableCameras(region, segment, names), names);
      coverage.affected.push({ region, sequences, cameras, locateSeq: cameras[0]?.sequences[0] ?? firstSeq });
    }
    if (offsetUnjudged) coverage.offsetUnjudged.push({ region, reason: offsetUnjudged, camera: segment?.offsetRef?.camera ?? null });
    if (productNg && isNg(entry.verdict)) defects.push(defectEvidence(region, entry.verdict, entry.reason, groups, names));
  }
  return { coverage, defects };
}
