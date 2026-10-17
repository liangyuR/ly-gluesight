export interface TrialPoint {
  id: number;
  status: "ok" | "no_bead";
  offset: number | null;
  width: number | null;
  px: [number, number];
  edge: [[number, number], [number, number]] | null;
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
    if (!Number.isSafeInteger(ids[i]) || ids[i] < 0 || (state !== "ok" && state !== "no_bead") || !nullableFinite(offset) || !nullableFinite(size)) return null;
    if (!Array.isArray(position) || position.length !== 2 || !position.every(finite)) return null;
    const rawEdge = Array.isArray(record.edges) ? record.edges[i] : null;
    const edge: TrialPoint['edge'] = state === 'ok' && Array.isArray(rawEdge) && rawEdge.length === 2 && rawEdge.every(p => Array.isArray(p) && p.length === 2 && p.every(finite))
      ? [[rawEdge[0][0], rawEdge[0][1]], [rawEdge[1][0], rawEdge[1][1]]] : null;
    points.push({ edge, id: ids[i], status: state, offset, width: size, px: [position[0], position[1]] });
  }
  return points;
}

export function TrialResult({ points }: { points: TrialPoint[] }) {
  const measured = points.filter(point => point.status === "ok" && point.offset !== null && point.width !== null).length;
  const value = (number: number | null) => number === null ? "未测" : number.toFixed(3);
  return <details open>
    <summary>逐点试测结果 · 测得 {measured} / {points.length} · 未测 {points.length - measured}</summary>
    <p className="muted hint">绿色横线连接测得的两侧胶边，红点表示未测到胶条；偏移与胶宽见下表。</p>
    <div className="table-wrap" style={{ maxHeight: 280, overflow: "auto" }}>
      <table className="table" aria-label="逐点试测结果"><thead><tr><th>测量站</th><th>状态</th><th>偏移 mm</th><th>胶宽 mm</th></tr></thead>
        <tbody>{points.map((point, index) => <tr key={index}><td>{point.id}</td><td className={point.status === "ok" ? "c-ok" : "c-warn"}>{point.status === "ok" ? "测得" : "缺胶"}</td><td>{value(point.offset)}</td><td>{value(point.width)}</td></tr>)}</tbody>
      </table>
    </div>
  </details>;
}
