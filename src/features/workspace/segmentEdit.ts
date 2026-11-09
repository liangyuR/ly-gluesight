/**
 * 段列表的编辑规则（纯函数，分段与示教页用）：与后端 plan::new_segment / validate_segments 与 D-SB-20 对应。
 * N_exp 取自示教件分段、x_ref 由测量示教件得出：段序列或类型变化时清空（见 workbenchDraft.segmentsForSave），不在这里手填。
 */
import type { EdgeParams, HeadingSettings, KindDefaults, PathSegment, SegmentKind } from "../plan/types";
import { windowError } from "./teach";

/** 段首尾跳过帧组数的上限，与后端 plan::MAX_SKIP_FRAMES 一致。 */
export const MAX_SKIP_FRAMES = 1000;

/** 新开启逐帧定向的段：方向容差 5°，不设测量区间覆盖（用相机的，与后端 HeadingSettings::default 一致）。 */
export const DEFAULT_HEADING: HeadingSettings = { toleranceDeg: 5 };
/** 胶偏改为 toEdge 时的默认找边参数（D-SB-48）。 */
export const DEFAULT_EDGE: EdgeParams = { polarity: "brightToDark", searchMm: 20, contrastMin: 40, nominalMm: null };

/** 新建一段：检测项取该类型的默认值，逐帧定向默认开启（后端 new_segment），N_exp、标定、胶偏参考为空。 */
export function newSegment(index: number, kind: SegmentKind, defaults: KindDefaults): PathSegment {
  const base = defaults[kind];
  return {
    index, kind, lengthMm: null, expectedFrames: null, calib: null,
    checks: structuredClone(base.checks), offsetRef: null, inspectRange: null, cameras: [], skipHead: 0, skipTail: 0, heading: structuredClone(DEFAULT_HEADING),
  };
}

/** 清空全部 N_exp、标定与 x_ref（后端在段序列或类型变化时同样清空，D-SB-20）。 */
export const clearCalibration = (segments: PathSegment[]): PathSegment[] => segments.map(segment => ({
  ...segment, expectedFrames: null, calib: null,
  offsetRef: segment.offsetRef && { ...segment.offsetRef, xRef: null, spread: null },
}));

/** 把第 at 段的检测项套用到其余同类型段（不改相机、跳过帧数、长度、逐帧定向与胶偏参考）：段序列与类型不变，标定保留。 */
export const applyToKind = (segments: PathSegment[], at: number): PathSegment[] => {
  const source = segments[at];
  if (!source) return segments;
  return segments.map((segment, i) => i !== at && segment.kind === source.kind
    ? { ...segment, checks: structuredClone(source.checks) } : segment);
};

/** 不改段序列与类型的编辑（长度、跳过帧数、检测区间、检测项、胶偏参考）：标定保留。 */
export const patchSegment = (segments: PathSegment[], at: number, patch: Partial<PathSegment>) =>
  segments.map((segment, i) => i === at ? { ...segment, ...patch } : segment);

const containsNaN = (value: unknown): boolean =>
  typeof value === "number" ? Number.isNaN(value) : Array.isArray(value) ? value.some(containsNaN)
    : value && typeof value === "object" ? Object.values(value).some(containsNaN) : false;

/** 保存前的本地检查：有空值（NaN 序列化成 null，后端读不了）或明显越界时不发送，原因显示在保存状态旁。 */
export function segmentIssues(segments: PathSegment[]): string[] {
  const issues: string[] = [];
  for (const segment of segments) {
    const name = `段 ${segment.index}`;
    if (segment.lengthMm != null && !(segment.lengthMm > 0)) issues.push(`${name} 长度需大于 0`);
    for (const [label, value] of [["段首跳过", segment.skipHead], ["段尾跳过", segment.skipTail]] as const)
      if (!Number.isInteger(value) || value < 0 || value > MAX_SKIP_FRAMES) issues.push(`${name} ${label}帧数需为 0–${MAX_SKIP_FRAMES} 的整数`);
    if (containsNaN(segment.checks)) issues.push(`${name} 检测项有未填写的数值`);
    if (segment.heading && !(segment.heading.toleranceDeg > 0 && segment.heading.toleranceDeg <= 45)) issues.push(`${name} 方向容差需在 0–45° 之间`);
    if (segment.searchMm != null && !(Number.isFinite(segment.searchMm) && segment.searchMm > 0)) issues.push(`${name} 搜索半宽需为正`);
    if (segment.widthRange && !segment.widthRange.every(Number.isFinite)) issues.push(`${name} 胶宽范围有未填写的数值`);
    const windowIssue = segment.heading?.window ? windowError(segment.heading.window) : "";
    if (windowIssue) issues.push(`${name} ${windowIssue}`);
    const edge = segment.offsetRef?.measure === "toEdge" ? segment.offsetRef.edge : null;
    if (edge) {
      if (!(edge.searchMm > 0 && edge.searchMm <= 50)) issues.push(`${name} 找边搜索长度需在 0–50 mm 之间`);
      if (!(edge.contrastMin >= 1 && edge.contrastMin <= 255)) issues.push(`${name} 找边对比度下限需在 1–255 之间`);
      if (edge.nominalMm != null && !(edge.nominalMm >= 0)) issues.push(`${name} 图纸胶边距不能为负`);
    }
  }
  return issues;
}
