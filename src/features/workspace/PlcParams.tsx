import { Link } from "react-router-dom";
import { ArrowRight } from "lucide-react";
import { Table, type TableColumn } from "qing-steel";
import { Badge, Notice, Panel } from "./components";
import type { PlcParams } from "./types";

type Param = { tag: string; value: number; note: string };
const paramColumns: TableColumn<Param>[] = [
  { key: "tag", header: "PLC 标签", render: param => <span className="mono">{param.tag}</span> },
  { key: "value", header: "值", render: param => <span className="mono">{param.value}</span> },
  { key: "note", header: "说明", render: param => param.note },
];
type Shot = PlcParams["shots"][number] & { order: number };
const shotColumns: TableColumn<Shot>[] = [
  { key: "order", header: "顺序", render: shot => shot.order },
  { key: "shotId", header: "拍照点", render: shot => <span className="mono">{shot.shotId}</span> },
  { key: "poseId", header: "Pose", render: shot => <span className="mono">{shot.poseId}</span> },
];

/** 配方总览里的 PLC 对接参数：当前生产版本，PLC 工程师照此配置开始请求。 */
export function PlcParamsPanel({ production, version, planChanged, offline }: { production: PlcParams | null; version: number | null; planChanged: boolean; offline: boolean }) {
  const params: Param[] = production ? [
    { tag: "productCode", value: production.productCode, note: "产品代码；不一致返回 95" },
    { tag: "planVersion", value: production.planVersion, note: "计划版本，拍照顺序或 Pose 变化后会换新号；不一致返回 100" },
    { tag: "shotCount", value: production.shotCount, note: "触发次数，deviceShots 填同一值；不一致返回 94" },
  ] : [];
  return <div id="plc-params"><Panel title="PLC 对接参数" detail={production && version ? `当前生产 v${version}` : undefined}
    actions={production && planChanged ? <Link to="/recipe/validation" className="wp-progress-link"><Badge tone="warn">候选发布后将变化</Badge><ArrowRight size={14} aria-hidden="true" /></Link> : undefined}>
    {offline && <Notice title="当前为离线模式" tone="warn">离线发布的版本只在离线运行有效，真机 PLC 以真机发布后的参数为准。</Notice>}
    {production ? <>
      <p className="muted">PLC 开始请求要填的值；PC 接件时逐项核对，不一致则拒绝布防。</p>
      <Table<Param> caption="开始请求参数" rows={params} columns={paramColumns} rowKey={param => param.tag} />
      <Table<Shot> caption="拍照顺序与 Pose" rows={production.shots.map((shot, index) => ({ ...shot, order: index + 1 }))} columns={shotColumns} rowKey={shot => shot.shotId} />
      <p className="muted">机器人按上表顺序经过各拍照点，每个拍照点发一次触发；同一 Pose 可被多个拍照点引用，三目设备的三个视角不分别计数。</p>
    </> : <p className="muted">尚未发布：发布生效后，这里显示 PLC 要填的参数。</p>}
  </Panel></div>;
}

/** 发布确认框：这次发布后 PLC 要同步的项；首次发布时列出候选值本身。 */
export function plcSyncItems(production: PlcParams | null, candidate: PlcParams): string[] {
  const change = (from: number | undefined, to: number) => from === undefined ? String(to) : from === to ? `${to}（不变）` : `${from} → ${to}`;
  return [
    `planVersion：${production ? production.planVersion + " → " : ""}发布时分配新号（生效后在配方总览“PLC 对接参数”查看）`,
    `shotCount / deviceShots：${change(production?.shotCount, candidate.shotCount)}`,
    `productCode：${change(production?.productCode, candidate.productCode)}`,
  ];
}
