/**
 * 段配方的规模显示：段数、整件预期帧数 ΣN_exp ± Σtol 或“待标定”。配方库、离线配方选择、工作台摘要共用。
 * 只做取值与文字，不做判定；整件口径与后端 plan::expected_total / tolerance_total 一致（D-SB 第六节）。
 */
import type { PartCalib, PathSegment } from "./types";
import { expectedText } from "./vis";

export interface PlanTotals {
  /** ΣN_exp：各段 N_exp 都非空时为其和，否则取整件标定，都没有为 null（待标定） */
  expectedTotal: number | null;
  /** Σ 各段容差 */
  toleranceTotal: number;
}

/** 从配方里的段与整件标定算出合计（候选配方的段列表在界面上随时会变，这里不依赖后端 DTO）。 */
export function planTotals(doc: { segments?: PathSegment[]; partCalib?: PartCalib | null }): PlanTotals {
  const segments = doc.segments ?? [];
  const frames = segments.map(segment => segment.expectedFrames);
  const summed = frames.length > 0 && frames.every((n): n is number => n != null) ? frames.reduce((sum, n) => sum + n, 0) : null;
  return {
    expectedTotal: summed ?? doc.partCalib?.expectedTotal ?? null,
    toleranceTotal: segments.reduce((sum, segment) => sum + segment.tolerance, 0),
  };
}

/** “3 段 · ΣN_exp 12 ± 3”；待标定时“3 段 · ΣN_exp 待标定”。 */
export function planScaleText(scale: { segmentCount: number; expectedTotal: number | null | undefined; toleranceTotal: number | undefined }): string {
  return `${scale.segmentCount} 段 · ΣN_exp ${expectedText(scale.expectedTotal, scale.toleranceTotal ?? 0)}`;
}
