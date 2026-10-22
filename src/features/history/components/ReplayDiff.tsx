import { useState } from "react";
import { Alert, Button, Checkbox, EmptyState, StatusBadge, Table, type TableColumn } from "qing-steel";
import { verdictLabel, verdictTone } from "../meta";
import { REPLAY_TOLERANCE_MM, regionChangeText, type RegionDiff, type ShotDiff, type ShotSide } from "../replay";
import type { RegionResult } from "../../plan/types";
import "../replay.css";

const verdicts = (vs: ShotSide["verdicts"], empty = "—") => vs.some(v => v.verdict)
  ? <span className="replay-verdicts">{vs.map(({ view, verdict }) => {
    const prefix = vs.length > 1 ? `图 ${view} · ` : "";
    return verdict ? <StatusBadge key={view} tone={verdictTone(verdict)}>{prefix}{verdictLabel[verdict]}</StatusBadge> : <span key={view} className="muted">{prefix}—</span>;
  })}</span>
  : <span className="muted">{empty}</span>;
const side = (s: ShotSide) => <>{verdicts(s.verdicts)}
  <span className="muted"> · 有胶 {s.ok} · 缺胶 {s.gap}{s.inv ? ` · 未测成 ${s.inv}` : ""}{s.noRef ? ` · 无基准 ${s.noRef}` : ""}{s.out ? ` · 超出检测范围 ${s.out}` : ""}</span></>;
const mm = (v: number | null) => v == null ? "—" : v.toFixed(3);

export default function ReplayDiff({ diffs, source, selected, onSelect }: {
  diffs: ShotDiff[] | null; source: "rules" | "raw" | "original"; selected: number; onSelect: (k: number) => void;
}) {
  const [onlyDiff, setOnlyDiff] = useState(true);
  if (!diffs) return <Alert tone="warn">候选的拍照点或测点布局与原记录不同，无法逐点对应，只比较整件结论。</Alert>;
  const checked = diffs.filter(d => !d.skip);
  const differing = checked.filter(d => d.differs);
  const rows = onlyDiff ? differing : checked;
  const skipped = diffs.length - checked.length;
  const title = source === "original" ? "原包重跑" : source === "raw" ? "候选复测" : "候选重判";
  const columns: TableColumn<ShotDiff>[] = [
    {
      key: "shot",
      header: "拍照点",
      render: d => <>
        <Button variant={d.k === selected ? "tonal" : "text"} size="sm" className="mono" aria-label={`比对查看拍照点 k${d.k + 1}`} onClick={() => onSelect(d.k)}>k{d.k + 1} · {d.shotId}</Button>
        <div className="muted">{d.camera}</div>
      </>,
    },
    { key: "original", header: "原结果", render: d => <>{side(d.original)}{d.original.error && <div className="c-err">{d.original.error}</div>}</> },
    { key: "replay", header: title, render: d => <>{d.remeasured ? side(d.replay) : verdicts(d.replay.verdicts, "未重测")}{d.replay.error && <div className="c-err">{d.replay.error}</div>}</> },
    { key: "changed", header: "状态变化", align: "right", render: d => d.remeasured ? d.changed.length : "—" },
    { key: "dd", header: "最大偏移差 mm", align: "right", render: d => mm(d.maxDd) },
    { key: "dw", header: "最大胶宽差 mm", align: "right", render: d => mm(d.maxDw) },
    { key: "verdict", header: "结论", render: d => d.differs ? <StatusBadge tone="warn">不同</StatusBadge> : <StatusBadge tone="ok">一致</StatusBadge> },
  ];
  return <div className="replay-diff">
    <div className="wp-actions">
      <StatusBadge tone={differing.length ? "warn" : "ok"} role="status">
        {differing.length ? `${differing.length} / ${checked.length} 个拍照点与原结果不同` : `${checked.length} 个拍照点全部与原结果一致`}
        {source !== "rules" && `（偏移、胶宽差 ≤ ${REPLAY_TOLERANCE_MM} mm 视为一致）`}
      </StatusBadge>
      {skipped > 0 && <span className="muted">另有 {skipped} 个不检拍照点未列出</span>}
      <span className="spacer" />
      <Checkbox label="只看差异" checked={onlyDiff} onCheckedChange={setOnlyDiff} />
    </div>
    <Table<ShotDiff>
      className="replay-table"
      caption="逐拍照点比对"
      rows={rows}
      columns={columns}
      rowKey={d => String(d.k)}
      emptyState={<EmptyState title={onlyDiff ? "没有差异" : "没有拍照点"} />}
    />
  </div>;
}

const regionSide = (r: RegionResult | null) => r
  ? <>
    <StatusBadge tone={verdictTone(r.verdict)}>{verdictLabel[r.verdict]}</StatusBadge>
    <span className="muted"> · {r.frameCount} 帧{r.incompleteGroups.length ? ` · 不完整组 ${r.incompleteGroups.length}` : ""}{r.width ? ` · 胶宽均值 ${r.width.mean.toFixed(2)} mm` : ""}</span>
  </>
  : <span className="muted">无此区域</span>;

/** 段配方的比对：每个区域一行，原结果与重跑结果并列，列出变化的项；计划身份变了（diffs 为 null）时只比整件结论。 */
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
