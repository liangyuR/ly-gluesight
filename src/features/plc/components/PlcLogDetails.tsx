import { useEffect, useState } from "react";
import { Download } from "lucide-react";
import { Alert, Button, EmptyState, Pagination, Select, Stack, StatusBadge, Table, Text, type TableColumn } from "qing-steel";
import { errorText } from "../../../lib/errors";
import { plcApi } from "../api";
import { categoryLabels, isText, levelLabels } from "../meta";
import { formatTs } from "../time";
import type { HistorySample, LogEntry, LogGroup, LogPage, PlcPoint } from "../types";
import Modal from "./Modal";
import StepChart from "./StepChart";

const PAGE_SIZE = 100;
const levelTone = { info: "info", warn: "warn", error: "ng" } as const;

function DetailExport({ group, pointId }: { group: LogGroup; pointId: string | null }) {
  const [path, setPath] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  async function exportCsv() {
    setBusy(true);
    setPath("");
    setError("");
    try {
      setPath(await plcApi.exportLogs({ start: group.firstTs, end: group.lastTs, pointId },
        { categories: categoryLabels, levels: levelLabels }, group));
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  }
  return <>
    <Button leadingIcon={<Download size={16} />} loading={busy} disabled={busy} onClick={exportCsv}>导出明细 CSV</Button>
    {path && <Button variant="text" title={path} onClick={() => plcApi.reveal(path).catch(cause => setError(errorText(cause)))}>打开导出文件位置</Button>}
    {error && <Alert tone="ng">{error}</Alert>}
  </>;
}

export default function PlcLogDetails({ group, onClose }: { group: LogGroup; onClose: () => void }) {
  const [page, setPage] = useState(1);
  const [point, setPoint] = useState<{ id: string; name: string } | null>(null);
  const [points, setPoints] = useState<PlcPoint[]>([]);
  const [configError, setConfigError] = useState("");
  const [logs, setLogs] = useState<LogPage | null>(null);
  const [samples, setSamples] = useState<HistorySample[] | null>(null);
  const [logError, setLogError] = useState("");
  const [trendError, setTrendError] = useState("");
  const [retry, setRetry] = useState(0);
  const pointId = point?.id ?? null;
  const pointType = points.find(p => p.id === pointId)?.dataType;
  const supportsTrend = !pointType || (!isText(pointType) && pointType !== "bytes");
  const title = group.partSeq === null ? "未关联工件 · 通讯明细" : `工件 #${group.partSeq} · 通讯明细`;

  useEffect(() => {
    let active = true;
    plcApi.getConfig().then(config => { if (active) setPoints(config.points); })
      .catch(cause => { if (active) setConfigError(errorText(cause)); });
    return () => { active = false; };
  }, []);
  useEffect(() => {
    let active = true;
    setLogs(null);
    setLogError("");
    plcApi.queryGroupLogs(group, { start: group.firstTs, end: group.lastTs, pointId, limit: PAGE_SIZE, offset: (page - 1) * PAGE_SIZE })
      .then(result => {
        if (!active) return;
        const lastPage = Math.max(1, Math.ceil(result.total / PAGE_SIZE));
        if (page > lastPage) setPage(lastPage);
        else setLogs(result);
      })
      .catch(cause => { if (active) setLogError(errorText(cause)); });
    return () => { active = false; };
  }, [group, page, pointId, retry]);
  useEffect(() => {
    let active = true;
    setSamples(null);
    setTrendError("");
    if (pointId) {
      plcApi.pointHistory(pointId, group.firstTs, group.lastTs, group)
        .then(result => { if (active) setSamples(result); })
        .catch(cause => { if (active) setTrendError(errorText(cause)); });
    }
    return () => { active = false; };
  }, [group, pointId, retry]);

  function selectPoint(value: typeof point) { setPoint(value); setPage(1); }
  const columns: TableColumn<LogEntry>[] = [
    { key: "ts", header: "时间", nowrap: true, render: e => <span className="mono">{formatTs(e.ts)}</span> },
    { key: "level", header: "级别", render: e => <StatusBadge tone={levelTone[e.level]}>{levelLabels[e.level]}</StatusBadge> },
    { key: "category", header: "类别", nowrap: true, render: e => categoryLabels[e.category] ?? e.category },
    { key: "point", header: "点位", render: e => e.pointId
      ? <Button variant="text" size="sm" onClick={() => selectPoint({ id: e.pointId!, name: e.pointName ?? e.pointId! })}>{e.pointName ?? e.pointId}</Button>
      : "—" },
    { key: "message", header: "内容", render: e => e.message },
  ];
  return <Modal title={title} onClose={onClose} width={1060} footer={<Button onClick={onClose}>关闭</Button>}>
    <Stack direction="column" align="stretch" gap={12}>
      <Stack gap={12}>
        {group.sn && <Text>SN {group.sn}</Text>}
        <Text muted>{formatTs(group.firstTs)} 至 {formatTs(group.lastTs)}</Text>
      </Stack>
      <Stack gap={8}>
        <Select aria-label="日志点位" value={pointId ?? ""} onValueChange={id => {
          const selected = points.find(p => p.id === id);
          selectPoint(id ? { id, name: selected?.name ?? point?.name ?? id } : null);
        }} options={[
          { value: "", label: "全部点位" },
          ...(point && !points.some(p => p.id === point.id) ? [{ value: point.id, label: `${point.name}（历史点位）` }] : []),
          ...points.map(p => ({ value: p.id, label: p.name })),
        ]} />
        <DetailExport key={pointId ?? ""} group={group} pointId={pointId} />
        <div className="spacer" />
        {logs && logs.total > PAGE_SIZE && <Pagination label="明细分页" total={logs.total} page={page} pageSize={PAGE_SIZE}
          showPageSize={false} onPageChange={setPage} />}
      </Stack>
      {configError && <Alert tone="warn">点位配置读取失败：{configError}。可点击日志中的点位筛选。</Alert>}
      {samples?.length === 0 && <Text muted>本组没有该点位的值变化记录。</Text>}
      {samples && samples.length > 0 && point && supportsTrend && <section aria-label="点位趋势">
        <Text>{point.name} · 值变化趋势</Text>
        <StepChart samples={samples} start={group.firstTs} end={Math.max(group.lastTs, group.firstTs + 1)} />
      </section>}
      {(logError || trendError) && <Alert tone="ng" action={<Button size="sm" onClick={() => setRetry(n => n + 1)}>重试</Button>}>
        {logError && <div>明细读取失败：{logError}</div>}
        {trendError && <div>趋势读取失败：{trendError}</div>}
      </Alert>}
      <Table caption="通讯明细" rows={logs?.items ?? []} columns={columns} rowKey={e => String(e.id)} loading={!logs && !logError}
        emptyState={<EmptyState title="所选条件下暂无通讯明细" />} />
    </Stack>
  </Modal>;
}
