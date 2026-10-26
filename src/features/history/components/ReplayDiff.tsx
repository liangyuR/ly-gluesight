import { Alert, EmptyState, StatusBadge, Table, type TableColumn } from "qing-steel";
import { verdictLabel, verdictTone } from "../meta";
import { REPLAY_TOLERANCE_MM, regionChangeText, type RegionDiff } from "../replay";
import type { RegionResult } from "../../plan/types";
import "../replay.css";

const regionSide = (r: RegionResult | null) => r
  ? <>
    <StatusBadge tone={verdictTone(r.verdict)}>{verdictLabel[r.verdict]}</StatusBadge>
    <span className="muted"> · {r.frameCount} 帧{r.incompleteGroups.length ? ` · 不完整组 ${r.incompleteGroups.length}` : ""}{r.width ? ` · 胶宽均值 ${r.width.mean.toFixed(2)} mm` : ""}</span>
  </>
  : <span className="muted">无此区域</span>;

/** 复测比对：每个区域一行，原结果与重跑结果并列，列出变化的项；计划身份变了（diffs 为 null）时只比整件结论。 */
export function RegionReplayDiff({ diffs, source }: { diffs: RegionDiff[] | null; source: "rules" | "raw" | "original" }) {
  if (!diffs) return <Alert tone="warn">候选的段序列、类型、预期帧数或相机顺序与原记录不同（计划身份变了），区域无法一一对应，只比较整件结论。</Alert>;
  const differing = diffs.filter(d => d.differs);
  const title = source === "original" ? "原包重跑" : source === "raw" ? "候选复测" : "候选重判";
  const columns: TableColumn<RegionDiff>[] = [
    { key: "region", header: "区域", render: d => d.label },
    { key: "original", header: "原结果", render: d => regionSide(d.original) },
    { key: "replay", header: title, render: d => regionSide(d.replay) },
    { key: "changes", header: "变化", render: d => d.changes.length ? d.changes.map(c => regionChangeText[c]).join("、") : "—" },
    { key: "verdict", header: "结论", render: d => d.differs ? <StatusBadge tone="warn">不同</StatusBadge> : <StatusBadge tone="ok">一致</StatusBadge> },
  ];
  return <div className="replay-diff">
    <StatusBadge tone={differing.length ? "warn" : "ok"} role="status">
      {differing.length ? `${differing.length} / ${diffs.length} 个区域与原结果不同` : `${diffs.length} 个区域全部与原结果一致`}
      {source !== "rules" && `（胶宽、胶偏差 ≤ ${REPLAY_TOLERANCE_MM} mm 视为一致）`}
    </StatusBadge>
    <Table<RegionDiff> className="replay-table" caption="逐区域比对" rows={diffs} columns={columns} rowKey={d => d.key} emptyState={<EmptyState title="没有区域" />} />
  </div>;
}
