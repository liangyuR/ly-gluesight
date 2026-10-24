import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { ArrowRight, Copy, Download } from "lucide-react";
import { Button, Table, type TableColumn } from "qing-steel";
import { errorText } from "../../lib/errors";
import type { ExportSegment, PlanExport, WorkspacePlanView } from "../plan/types";
import { expectedText, segmentKindLabel } from "../plan/vis";
import { Badge, Notice, Panel } from "./components";

type Param = { tag: string; value: number | string; note: string };
const paramColumns: TableColumn<Param>[] = [
  { key: "tag", header: "PLC 标签", render: param => <span className="mono">{param.tag}</span> },
  { key: "value", header: "值", render: param => <span className="mono">{param.value}</span> },
  { key: "note", header: "说明", render: param => param.note },
];

/**
 * 计划导出 JSON 文本，给 PLC / 机器人工程师核对使能点位；结构与后端 recipe_plan_export 相同。
 * 界面直接用 WorkspaceView.plan 的 productionExport / candidateExport 序列化，不再单独调 recipe_plan_export：
 * 那条命令只读配方库里已发布的配方，导不出还没发布的候选，而候选的导出正是发布前核对所需。
 */
export const planExportJson = (plan: PlanExport) => JSON.stringify(plan, null, 2);

const rangeText = (segment: ExportSegment) => segment.inspectRange ? `${segment.inspectRange.aMm}–${segment.inspectRange.bMm} mm` : "—";
const segmentColumns: TableColumn<ExportSegment>[] = [
  { key: "index", header: "段", render: segment => segment.index },
  { key: "kind", header: "类型", render: segment => segmentKindLabel[segment.kind] },
  { key: "length", header: "长度", render: segment => <span className="mono">{segment.lengthMm == null ? "—" : `${segment.lengthMm} mm`}</span> },
  { key: "expected", header: "预期帧数 N_exp", render: segment => <span className="mono">{segment.expectedFrames == null ? "待标定" : segment.expectedFrames}</span> },
  { key: "tolerance", header: "容差", render: segment => <span className="mono">± {segment.tolerance}</span> },
  { key: "range", header: "检测区间", render: segment => <span className="mono">{rangeText(segment)}</span> },
];

function saveJson(name: string, text: string) {
  const blob = new Blob([text], { type: "application/json;charset=utf-8" });
  const link = document.createElement("a");
  link.href = URL.createObjectURL(blob);
  link.download = name;
  try { link.click(); } finally { URL.revokeObjectURL(link.href); }
}

/**
 * 配方总览里的 PLC 对接参数（段配方）：当前生产的计划导出，未发布时给候选的导出供提前核对。
 * 可复制或另存为 JSON：段序列、各段 N_exp、容差、ΣN_exp，PLC / 机器人工程师照此核对使能点位。
 */
export function PlanExportPanel({ plan, isProduction, version, shotCountChanged, offline }: {
  plan: PlanExport | null; isProduction: boolean; version: number | null; shotCountChanged: boolean; offline: boolean;
}) {
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState("");
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const total = plan?.expectedTotal ?? null;
  const params: Param[] = plan ? [
    { tag: "productCode", value: plan.productCode, note: "产品代码；不一致返回 95" },
    { tag: "planVersion", value: plan.planVersion > 0 ? plan.planVersion : "发布时分配", note: "计划版本，段序列、类型、预期帧数、相机或 ΣN_exp 变化后会换新号；不一致返回 100" },
    { tag: "shotCount", value: total ?? "待标定", note: "预期总帧数 ΣN_exp；不一致返回 94" },
    { tag: "deviceShots", value: total ?? "待标定", note: "与 shotCount 填同一值 ΣN_exp" },
  ] : [];
  const copy = async () => {
    if (!plan) return;
    setError("");
    try {
      await navigator.clipboard.writeText(planExportJson(plan));
      setCopied(true);
      clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopied(false), 1500);
    } catch (e) { setError(`复制失败：${errorText(e)}`); }
  };
  const save = () => {
    if (!plan) return;
    setError("");
    try { saveJson(`${plan.recipeId}-plan${plan.planVersion > 0 ? "-v" + plan.planVersion : ""}.json`, planExportJson(plan)); }
    catch (e) { setError(`另存失败：${errorText(e)}`); }
  };
  return <div id="plc-params"><Panel title="PLC 对接参数" detail={plan ? isProduction && version ? `当前生产 v${version}` : "尚未发布，以下为候选计划" : undefined}
    actions={plan && isProduction && shotCountChanged ? <Link to="/recipe/validation" className="wp-progress-link"><Badge tone="warn">候选发布后将变化</Badge><ArrowRight size={14} aria-hidden="true" /></Link> : undefined}>
    {offline && <Notice title="当前为离线模式" tone="warn">离线发布的版本只在离线运行有效，真机 PLC 以真机发布后的参数为准。</Notice>}
    {plan ? <>
      <p className="muted">三台相机共用一个触发。PLC 开始请求的 shotCount 与 deviceShots 都填预期总帧数 ΣN_exp；PC 接件时核对身份与 ΣN_exp，不一致则拒绝布防。</p>
      <Table<Param> caption="开始请求参数" rows={params} columns={paramColumns} rowKey={param => param.tag} />
      <Table<ExportSegment> caption="段序列与预期帧数" rows={plan.segments} columns={segmentColumns} rowKey={segment => String(segment.index)} />
      <p className="muted">合计 ΣN_exp {expectedText(total, plan.toleranceTotal)} 帧；运行时每段帧数在 N_exp ± 容差内为正常，超出只告警。
        deviceTriggers（PLC 累计脉冲数）只作参考：与相机帧数相差超过 1 时记丢帧告警，不判故障。</p>
      <div className="wp-actions">
        <Button leadingIcon={<Copy size={15} aria-hidden="true" />} onClick={() => void copy()}>{copied ? "已复制" : "复制 JSON"}</Button>
        <Button leadingIcon={<Download size={15} aria-hidden="true" />} onClick={save}>另存 JSON</Button>
      </div>
      {error && <Notice title="导出未完成" tone="warn">{error}</Notice>}
    </> : <p className="muted">暂无计划：先在“段列表”添加至少一段，这里显示 PLC 要填的参数。</p>}
  </Panel></div>;
}

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
    `planVersion：${production ? production.planVersion + " → " : ""}发布时分配新号（生效后在配方总览“PLC 对接参数”查看）`,
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
