import type { ExportSegment, PlanExport, WorkspacePlanView } from "../plan/types";
import { expectedText, segmentKindLabel } from "../plan/vis";
import { Notice } from "./components";

const camerasText = (cameras: string[]) => cameras.join("、");
const segmentsText = (segments: ExportSegment[]) => segments.map(segment => `${segmentKindLabel[segment.kind]}${segment.expectedFrames == null ? "" : " " + segment.expectedFrames}`).join(" · ");
const sameSegments = (a: ExportSegment[], b: ExportSegment[]) =>
  a.length === b.length && a.every((segment, i) => segment.index === b[i].index && segment.kind === b[i].kind && segment.expectedFrames === b[i].expectedFrames);
const sameList = (a: string[], b: string[]) => a.length === b.length && a.every((item, i) => item === b[i]);

/** 发布确认框里候选与生产的对比行：型号、ΣN_exp、段序列、相机，各自标明“不变”或生产值。 */
export function planCompareItems(production: PlanExport | null, candidate: PlanExport): string[] {
  const note = (same: boolean, previous: string) => production === null ? "" : same ? "（不变）" : `（生产：${previous}）`;
  const total = (plan: PlanExport) => `${expectedText(plan.expectedTotal, plan.toleranceTotal)}`;
  return [
    `型号：${candidate.productCode}${production ? note(production.productCode === candidate.productCode, String(production.productCode)) : ""}`,
    `预期总帧数 ΣN_exp ± Σtol：${total(candidate)}${production ? note(production.expectedTotal === candidate.expectedTotal && production.toleranceTotal === candidate.toleranceTotal, total(production)) : ""}`,
    `段序列（${candidate.segments.length} 段）：${segmentsText(candidate.segments) || "无"}${production ? note(sameSegments(production.segments, candidate.segments), segmentsText(production.segments)) : ""}`,
    `相机（共用一个触发）：${camerasText(candidate.cameras)}${production ? note(sameList(production.cameras, candidate.cameras), camerasText(production.cameras)) : ""}`,
  ];
}

/** 段配方的发布确认内容：候选与生产的对比。协议 7 下 PLC 只报型号，计划与帧数由 PC 管理，发布后 PLC 不用改。 */
export function PlanPublishBody({ plan }: { plan: WorkspacePlanView }) {
  const { productionExport: production, candidateExport: candidate } = plan;
  return <>
    <p className="muted">发布内容包含当前已验证的段列表、各台相机的检测走廊、检测项与帧数容差。等待期间候选已冻结，生效或完成失败处理后再继续编辑。</p>
    {candidate ? <ul aria-label="候选与生产对比">{planCompareItems(production, candidate).map(item => <li key={item}>{item}</li>)}</ul>
      : plan.candidateExportError && <Notice tone="warn" title="无法生成对比">{plan.candidateExportError}</Notice>}
  </>;
}
