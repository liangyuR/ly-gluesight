import { ChevronDown } from "lucide-react";
import { Table, type TableColumn } from "qing-steel";

/** 与后端 vision/taught.rs 的 status 一致：no_ref 有胶、无位置基准（偏移不用）；out_of_range 超出检测范围（不参与判定）。 */
type TrialStatus = "ok" | "no_bead" | "no_ref" | "out_of_range";
const STATUS_TEXT: Record<TrialStatus, [string, string]> = {
  ok: ["测得", "c-ok"], no_bead: ["缺胶", "c-ng"], no_ref: ["无基准", "c-warn"], out_of_range: ["超出检测范围", "muted"],
};
const isStatus = (value: unknown): value is TrialStatus => typeof value === "string" && Object.hasOwn(STATUS_TEXT, value);
/** 画布上没有胶边线的站点颜色：有胶（含无基准）绿、缺胶红、超出检测范围灰。 */
export const trialStationColor = (status: TrialStatus) =>
  status === "no_bead" ? "var(--ng)" : status === "out_of_range" ? "var(--on-surface-variant)" : "var(--ok)";

export interface TrialPoint {
  id: number;
  status: TrialStatus;
  offset: number | null;
  width: number | null;
  px: [number, number];
  edge: [[number, number], [number, number]] | null;
  candidateWidth: number | null;
  candidateEdge: [[number, number], [number, number]] | null;
}

const finite = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value);
const nullableFinite = (value: unknown): value is number | null => value === null || finite(value);

export function trialPoints(measurement: unknown): TrialPoint[] | null {
  if (!measurement || typeof measurement !== "object") return null;
  const record = measurement as Record<string, unknown>;
  const { ids, status, innerCenter, width, px } = record;
  if (record.unit !== "mm" || !Array.isArray(ids) || !Array.isArray(status) || !Array.isArray(innerCenter) || !Array.isArray(width) || !Array.isArray(px)) return null;
  if ([status, innerCenter, width, px].some(values => values.length !== ids.length)) return null;
  const points: TrialPoint[] = [];
  for (let i = 0; i < ids.length; i++) {
    const position: unknown = px[i];
    const state: unknown = status[i];
    const offset: unknown = innerCenter[i];
    const size: unknown = width[i];
    if (!Number.isSafeInteger(ids[i]) || ids[i] < 0 || !isStatus(state) || !nullableFinite(offset) || !nullableFinite(size)) return null;
    if (!Array.isArray(position) || position.length !== 2 || !position.every(finite)) return null;
    const rawEdge = Array.isArray(record.edges) ? record.edges[i] : null;
    const edge: TrialPoint['edge'] = state !== 'no_bead' && Array.isArray(rawEdge) && rawEdge.length === 2 && rawEdge.every(p => Array.isArray(p) && p.length === 2 && p.every(finite))
      ? [[rawEdge[0][0], rawEdge[0][1]], [rawEdge[1][0], rawEdge[1][1]]] : null;
    const rawCandidate = Array.isArray(record.candidateEdges) ? record.candidateEdges[i] : null;
    const rawWidth = Array.isArray(record.candidateWidth) ? record.candidateWidth[i] : null;
    const candidateEdge: TrialPoint['edge'] = state === 'no_bead' && finite(rawWidth) && rawWidth > 0 && Array.isArray(rawCandidate) && rawCandidate.length === 2 && rawCandidate.every(p => Array.isArray(p) && p.length === 2 && p.every(finite))
      ? [[rawCandidate[0][0], rawCandidate[0][1]], [rawCandidate[1][0], rawCandidate[1][1]]] : null;
    points.push({ edge, candidateEdge, candidateWidth: candidateEdge ? rawWidth as number : null, id: ids[i], status: state, offset: state === "ok" ? offset : null, width: size, px: [position[0], position[1]] });
  }
  return points;
}

const value = (number: number | null) => number === null ? "未测" : number.toFixed(3);
type TrialRow = { key: string; point: TrialPoint };
const columns: TableColumn<TrialRow>[] = [
  { key: "id", header: "测量站", align: "right", render: ({ point }) => point.id },
  { key: "status", header: "状态", render: ({ point }) => <span className={STATUS_TEXT[point.status][1]}>{STATUS_TEXT[point.status][0]}</span> },
  { key: "offset", header: "偏移 mm", align: "right", render: ({ point }) => value(point.offset) },
  { key: "width", header: "胶宽 mm", align: "right", render: ({ point }) => point.candidateWidth != null ? <span className="c-ng">{value(point.candidateWidth)}（候选，未通过）</span> : value(point.width) },
];

export function TrialResult({ points }: { points: TrialPoint[] }) {
  const measured = points.filter(point => point.status === "ok" && point.offset !== null && point.width !== null).length;
  const legend = <span className="wp-trial-legend muted"><i className="ok" />有效胶宽<i className="ng" />候选胶宽未通过<span className="wp-legend-note">（仅供参考，不影响判定）</span></span>;
  // 明细默认收起，画布信息行已给出合格与量成比例；展开后中栏自己滚动
  return <details className="wp-trial-details">
    <summary><ChevronDown size={15} aria-hidden="true" /><span>逐点试测结果 · 测得 {measured} / {points.length} · 未测 {points.length - measured}</span>{legend}</summary>
    <div className="wp-trial-body">
      {legend}
      <p className="muted hint">绿色横线为有效胶宽，红色横线为未通过的候选胶宽，仅供参考，不改变 NG 判定；无候选胶边时保留红点。</p>
      <Table className="wp-trial-table" caption="逐点试测结果" columns={columns} rows={points.map((point, index) => ({ key: String(index), point }))} rowKey={row => row.key} />
    </div>
  </details>;
}
