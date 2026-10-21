import { useState } from "react";
import { Alert, Button, Checkbox, EmptyState, StatusBadge, Table, type TableColumn } from "qing-steel";
import { verdictLabel, verdictTone } from "../meta";
import { REPLAY_TOLERANCE_MM, type ShotDiff, type ShotSide } from "../replay";
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
