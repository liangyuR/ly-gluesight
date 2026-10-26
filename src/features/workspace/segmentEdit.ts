/**
 * 段列表的编辑规则（纯函数，SegmentsPage 与分段标定工作台用）：与后端 plan::new_segment / validate_segments 与 D-SB-20 对应。
 * N_exp、标定、x_ref 只能由多轮采集得出：段序列或类型变化时一律清空（见 workbenchDraft.segmentsForSave），不在这里手填。
 */
import type { KindDefaults, PathSegment, SegmentKind } from "../plan/types";

/** 与后端 plan::MAX_TOLERANCE 一致。 */
export const MAX_TOLERANCE = 1000;

/** 新建一段：容差与检测项取该类型的默认值（后端 new_segment），N_exp、标定、胶偏参考为空。 */
export function newSegment(index: number, kind: SegmentKind, defaults: KindDefaults): PathSegment {
  const base = defaults[kind];
  return {
    index, kind, lengthMm: null, expectedFrames: null, tolerance: base.tolerance, calib: null,
    checks: structuredClone(base.checks), offsetRef: null, inspectRange: null,
  };
}

/** 清空全部 N_exp、标定与 x_ref（后端在段序列或类型变化时同样清空，D-SB-20）。 */
export const clearCalibration = (segments: PathSegment[]): PathSegment[] => segments.map(segment => ({
  ...segment, expectedFrames: null, calib: null,
  offsetRef: segment.offsetRef && { ...segment.offsetRef, xRef: null, spread: null },
}));

/** 有没有会被“清空标定”丢掉的东西（有 N_exp、标定统计、x_ref，或整件已标定）。 */
export const hasCalibration = (segments: PathSegment[], partCalibrated: boolean) =>
  partCalibrated || segments.some(segment => segment.expectedFrames != null || segment.calib != null || segment.offsetRef?.xRef != null);

/** 不改段序列与类型的编辑（长度、容差、检测区间、检测项、胶偏参考）：标定保留。 */
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
    if (!Number.isInteger(segment.tolerance) || segment.tolerance < 0 || segment.tolerance > MAX_TOLERANCE)
      issues.push(`${name} 帧数容差需为 0–${MAX_TOLERANCE} 的整数`);
    if (segment.lengthMm != null && !(segment.lengthMm > 0)) issues.push(`${name} 长度需大于 0`);
    const range = segment.inspectRange;
    if (range && !(Number.isFinite(range.aMm) && Number.isFinite(range.bMm) && range.aMm >= 0 && range.aMm < range.bMm))
      issues.push(`${name} 检测区间需满足 0 ≤ a < b`);
    if (containsNaN(segment.checks)) issues.push(`${name} 检测项有未填写的数值`);
  }
  return issues;
}
