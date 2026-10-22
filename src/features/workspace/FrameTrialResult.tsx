import { ChevronDown } from "lucide-react";
import { Table, type TableColumn } from "qing-steel";
import type { FrameMetrics, FrameState } from "../plan/types";

/** 除 measured 外的帧状态：都只算“检测不完整”，不判 NG（规格 §3）。 */
const UNMEASURED_TEXT: Record<Exclude<FrameState, "measured">, string> = {
  missing: "缺帧", error: "测量失败", unmeasured: "未测（测量队列满或超时）", outOfCorridor: "胶条走出走廊",
};

const mm = (value: number | null | undefined, digits = 3) => value == null ? "未测" : value.toFixed(digits);

interface StationRow { key: string; id: number; width: number | null }
const columns: TableColumn<StationRow>[] = [
  { key: "id", header: "测量站", align: "right", render: row => row.id },
  { key: "status", header: "状态", render: row => row.width == null ? <span className="c-ng">缺胶</span> : <span className="c-ok">有胶</span> },
  { key: "width", header: "胶宽 mm", align: "right", render: row => mm(row.width) },
];

/**
 * 段配方一帧的测量结果：每站胶宽（缺胶站为空，不当成 0）、最长断胶、有胶比例与帧内胶中心相对走廊的法向偏移 x。
 * 段配方不做相对示教中线的逐点偏移，胶偏只在直线段按整段的 x 均值与最大偏差判。
 * 尚无页面挂载；旧拍照点配方的逐点试测表是 TrialResult。
 */
export function FrameTrialResult({ metrics }: { metrics: FrameMetrics }) {
  if (metrics.state !== "measured")
    return <p className="muted" role="status">本帧{UNMEASURED_TEXT[metrics.state]}{metrics.error ? `：${metrics.error}` : ""}；只算“检测不完整”，不判 NG。</p>;
  const rows: StationRow[] = metrics.widths.map((width, i) => ({ key: String(i), id: i + 1, width }));
  const present = rows.filter(row => row.width != null).length;
  // 明细默认收起，摘要行已给出胶宽、断胶与 x；展开后逐站核对
  return <details className="wp-trial-details">
    <summary><ChevronDown size={15} aria-hidden="true" /><span>逐站胶宽 · 有胶 {present} / {rows.length} 站 · 最长断胶 {mm(metrics.gapMaxMm, 1)} mm · x {mm(metrics.xMm)} mm</span></summary>
    <div className="wp-trial-body">
      <p className="muted hint">胶宽 {mm(metrics.widthMin)}–{mm(metrics.widthMax)} mm，均值 {mm(metrics.widthMean)} mm；有胶比例 {(metrics.presentRatio * 100).toFixed(0)}%。缺胶站不参与胶宽统计。</p>
      <Table className="wp-trial-table" caption="逐站胶宽" columns={columns} rows={rows} rowKey={row => row.key} />
    </div>
  </details>;
}
