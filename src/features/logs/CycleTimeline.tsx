import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { PackageOpen } from "lucide-react";
import { Alert, Button, Card, Checkbox, Chip, EmptyState, Heading, MeasurementReadings, Stack, StatusBadge, Table, Text, type MeasurementItem, type TableColumn } from "qing-steel";
import { formatTs } from "../plc/time";
import { errorText } from "../../lib/errors";
import { levelText, logsApi } from "./api";
import DiagnosticsDialog from "./DiagnosticsDialog";
import type { Timeline } from "./types";

const markedStages = new Set(["partStart", "armed", "partEnd", "result", "released", "fault", "reset"]);
const levelTone = { error: "ng", warn: "warn", info: "info", debug: "off" } as const;

type Row = [number, Timeline["entries"][number]];

function relative(ms: number) {
  return (ms >= 0 ? "+" : "") + (Math.abs(ms) >= 10_000 ? (ms / 1000).toFixed(2) + " s" : ms + " ms");
}

export default function CycleTimeline({ historyId, cycleId, ts, selected = -1, onSelectShot, frame }: {
  historyId: number; cycleId: string | null; ts: number;
  /** 旧拍照点记录：选中的拍照点下标与跳转 */
  selected?: number; onSelectShot?: (k: number) => void;
  /** 段配方记录：按 (相机, seq) 选帧，事件带 camera 与 seq 时按它过滤、跳转；给了它就不再按拍照点 k 过滤 */
  frame?: { camera: string; seq: number; onSelect: (camera: string, seq: number) => void } | null;
}) {
  const navigate = useNavigate();
  const [timeline, setTimeline] = useState<Timeline | null>(null);
  const [error, setError] = useState("");
  const [detail, setDetail] = useState(true);
  const [plc, setPlc] = useState(true);
  const [onlyShot, setOnlyShot] = useState(false);
  const [exporting, setExporting] = useState(false);
  useEffect(() => {
    let alive = true;
    setTimeline(null); setError("");
    if (!cycleId) return;
    logsApi.timeline(historyId).then(t => alive && setTimeline(t)).catch(e => alive && setError(errorText(e)));
    return () => { alive = false; };
  }, [historyId, cycleId]);
  const start = timeline?.entries.find(e => e.stage === "partStart")?.ts ?? timeline?.start ?? ts;
  const onSelected = (e: Row[1]) => frame ? e.camera === frame.camera && e.seq === frame.seq : e.k === selected;
  const rows = useMemo(() => (timeline?.entries ?? []).map((e, i): Row => [i, e]).filter(([, e]) => (detail || e.level !== "debug") && (plc || e.source !== "plc") && (!onlyShot || (frame ? e.camera === frame.camera && e.seq === frame.seq : e.k === selected))),
    [timeline, detail, plc, onlyShot, selected, frame?.camera, frame?.seq]);
  const errors = timeline?.entries.filter(e => e.level === "error").length ?? 0;
  const readings: MeasurementItem[] = timeline ? [
    ...(timeline.requestSeq != null ? [{ id: "requestSeq", label: "PLC 请求序号", value: timeline.requestSeq, precision: 0, size: "md" as const }] : []),
    ...timeline.stages.map(s => ({ id: s.key, label: s.label, value: s.ms, unit: "ms", precision: 0, size: "md" as const })),
    ...(errors > 0 ? [{ id: "errors", label: "错误", value: errors, precision: 0, status: "ng" as const, size: "md" as const }] : []),
  ] : [];
  const columns: TableColumn<Row>[] = [
    { key: "relative", header: "相对", render: ([, e]) => <span className="mono nowrap" data-level={e.level} data-selected={(frame ? e.seq != null && onSelected(e) : e.k != null && e.k === selected) ? "" : undefined}>{relative(e.ts - start)}</span> },
    { key: "time", header: "时间", render: ([, e]) => <span className="mono nowrap">{formatTs(e.ts).slice(11)}</span> },
    { key: "source", header: "来源", render: ([, e]) => <Chip>{e.source === "plc" ? "PLC" : "运行"}</Chip> },
    { key: "level", header: "级别", render: ([, e]) => <StatusBadge tone={levelTone[e.level]}>{levelText[e.level]}</StatusBadge> },
    { key: "ev", header: "事件", render: ([, e]) => <span className={"nowrap" + (e.stage && markedStages.has(e.stage) ? " log-mark" : "")}>{e.ev}</span> },
    frame
      ? { key: "frame", header: "相机 / 帧组", render: ([, e]) => e.camera != null && e.seq != null ? <Button variant="text" size="sm" className="mono" aria-label={`查看 ${e.camera} 帧组 ${e.seq}`} onClick={() => frame.onSelect(e.camera!, e.seq!)}>{e.camera} · #{e.seq}</Button> : null }
      : { key: "shot", header: "拍照点", render: ([, e]) => e.k != null ? <Button variant="text" size="sm" className="mono" aria-label={`查看拍照点 k${e.k + 1}`} onClick={() => onSelectShot?.(e.k!)}>k{e.k + 1}{e.shotId ? " · " + e.shotId : ""}</Button> : null },
    { key: "msg", header: "内容", render: ([, e]) => <span className="log-msg" title={e.msg}>{e.msg}</span> },
  ];
  return <Card className="timeline-panel" role="region" aria-label="本件事件时间线"
    title={<div><Heading level={3}>本件事件时间线</Heading><Text as="p" muted variant="label">按 cycleId 关联的运行日志与同时段 PLC 信号变化；{frame ? "点相机 / 帧组" : "点拍照点"}可在上方查看对应原图</Text></div>}
    actions={<Stack gap={8}>
      {cycleId && <Button size="sm" onClick={() => navigate(`/logs?cycleId=${encodeURIComponent(cycleId)}`)}>在运行日志中查看</Button>}
      <Button size="sm" leadingIcon={<PackageOpen size={14} />} onClick={() => setExporting(true)}>导出本件诊断包</Button>
    </Stack>}>
    {!cycleId ? <Text as="p" muted>本记录没有 cycleId，无法关联运行日志。</Text> : error ? <Alert tone="warn">{error}</Alert> : !timeline ? <Text as="p" muted>正在读取本件日志…</Text> : <>
      <MeasurementReadings label="事件阶段耗时" items={readings} minWidth={120} />
      {timeline.notes.map((note, i) => <Alert key={i} tone="warn">{note}</Alert>)}
      <Stack gap={12} justify="space-between">
        <Stack gap={12}>
          <Checkbox label="逐帧明细（调试级）" checked={detail} onCheckedChange={setDetail} />
          <Checkbox label="PLC 信号" checked={plc} onCheckedChange={setPlc} />
          <Checkbox label={frame ? `只看选中帧 ${frame.camera} #${frame.seq}` : `只看选中拍照点 k${selected + 1}`} checked={onlyShot} onCheckedChange={setOnlyShot} />
        </Stack>
        <Text muted>{rows.length} / {timeline.entries.length} 条</Text>
      </Stack>
      <Table<Row>
        className="timeline-table log-table"
        caption="本件事件"
        rows={rows}
        columns={columns}
        rowKey={([i]) => String(i)}
        emptyState={<EmptyState title="没有符合条件的事件" />}
      />
    </>}
    {exporting && <DiagnosticsDialog title="导出本件诊断包" historyIds={[historyId]} from={(timeline?.start ?? ts) - 60_000} to={(timeline?.end ?? ts) + 60_000} onClose={() => setExporting(false)} />}
  </Card>;
}
