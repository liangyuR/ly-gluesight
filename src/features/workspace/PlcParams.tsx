import type { ExportSegment, PlanExport, WorkspacePlanView } from "../plan/types";
import { expectedText, segmentKindLabel } from "../plan/vis";
import { Notice } from "./components";

const camerasText = (cameras: string[]) => cameras.join("、");
const segmentsText = (segments: ExportSegment[]) => segments.map(segment => `${segmentKindLabel[segment.kind]}${segment.expectedFrames == null ? "" : " " + segment.expectedFrames}`).join(" · ");
const sameSegments = (a: ExportSegment[], b: ExportSegment[]) =>
  a.length === b.length && a.every((segment, i) => segment.index === b[i].index && segment.kind === b[i].kind && segment.expectedFrames === b[i].expectedFrames);
const sameList = (a: string[], b: string[]) => a.length === b.length && a.every((item, i) => item === b[i]);

/**
 * 发布确认框：这次发布后 PLC 要同步的项，只列计划身份变化的部分（产品代码、段序号 / 类型 / N_exp、相机顺序、ΣN_exp）；
 * 限值、容差、检测区间等不参与身份，变了也不用同步。首次发布时列出候选值本身。身份没变返回空数组。
 */
export function planSyncItems(production: PlanExport | null, candidate: PlanExport): string[] {
  const change = (from: string | number | undefined, to: string | number) => from === undefined ? String(to) : `${from} → ${to}`;
  const totalText = (plan: PlanExport) => plan.expectedTotal ?? "待标定";
  const total = production === null || production.expectedTotal !== candidate.expectedTotal;
  const product = production === null || production.productCode !== candidate.productCode;
  const segments = production === null || !sameSegments(production.segments, candidate.segments);
  const cameras = production === null || !sameList(production.cameras, candidate.cameras);
  if (production && !(total || product || segments || cameras)) return [];
  return [
    `planVersion：${production ? production.planVersion + " → " : ""}发布时分配新号（生效后在配方库卡片查看）`,
    ...total ? [`shotCount / deviceShots（ΣN_exp）：${change(production ? totalText(production) : undefined, totalText(candidate))}`] : [],
    ...product ? [`productCode：${change(production?.productCode, candidate.productCode)}`] : [],
    ...segments ? [`段序列：${change(production ? segmentsText(production.segments) : undefined, segmentsText(candidate.segments))}`] : [],
    ...cameras ? [`相机顺序：${change(production ? camerasText(production.cameras) : undefined, camerasText(candidate.cameras))}`] : [],
  ];
}

/** 发布确认框里候选与生产的对比行：ΣN_exp、段序列、相机，各自标明“不变”或生产值。 */
export function planCompareItems(production: PlanExport | null, candidate: PlanExport): string[] {
  const note = (same: boolean, previous: string) => production === null ? "" : same ? "（不变）" : `（生产：${previous}）`;
  const total = (plan: PlanExport) => `${expectedText(plan.expectedTotal, plan.toleranceTotal)}`;
  return [
    `预期总帧数 ΣN_exp ± Σtol：${total(candidate)}${production ? note(production.expectedTotal === candidate.expectedTotal && production.toleranceTotal === candidate.toleranceTotal, total(production)) : ""}`,
    `段序列（${candidate.segments.length} 段）：${segmentsText(candidate.segments) || "无"}${production ? note(sameSegments(production.segments, candidate.segments), segmentsText(production.segments)) : ""}`,
    `相机（共用一个触发）：${camerasText(candidate.cameras)}${production ? note(sameList(production.cameras, candidate.cameras), camerasText(production.cameras)) : ""}`,
  ];
}

/** 段配方的发布确认内容：候选与生产的对比，以及计划身份变化时 PLC 要同步的项。 */
export function PlanPublishBody({ plan, planChanged, productionPlanVersion }: { plan: WorkspacePlanView; planChanged: boolean; productionPlanVersion: number | null }) {
  const { productionExport: production, candidateExport: candidate } = plan;
  const changed = planChanged || plan.shotCountChanged;
  const items = candidate ? planSyncItems(production, candidate) : [];
  return <>
    <p className="muted">发布内容包含当前已验证的段列表、各台相机的检测走廊、检测项与帧数容差。等待期间候选已冻结，生效或完成失败处理后再继续编辑。</p>
    {candidate && <ul aria-label="候选与生产对比">{planCompareItems(production, candidate).map(item => <li key={item}>{item}</li>)}</ul>}
    {changed ? <Notice tone="warn" title="计划已变，生效后 PLC 要同步">
      {candidate ? <ul aria-label="PLC 同步项">{items.map(item => <li key={item}>{item}</li>)}</ul> : <p>{plan.candidateExportError}</p>}
      <p>PLC 未同步前 PC 拒绝布防：planVersion 不一致返回 100，shotCount / deviceShots 与 ΣN_exp 不一致返回 94，productCode 不一致返回 95。</p>
    </Notice> : <p className="muted">计划未变，沿用计划版本 {productionPlanVersion}，PLC 不用改。</p>}
  </>;
}
