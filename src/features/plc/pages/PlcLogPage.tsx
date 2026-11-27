import { useEffect, useRef, useState } from "react";
import { Radio, RefreshCw, Search, X } from "lucide-react";
import { Alert, Button, Card, Chip, EmptyState, Heading, IconButton, Pagination, SegmentedControl, Stack, StatusBadge, Table, TextField, type TableColumn } from "qing-steel";
import { errorText } from "../../../lib/errors";
import { plcApi, subscribe } from "../api";
import PlcLogDetails from "../components/PlcLogDetails";
import { resultCodeLabel } from "../handshake";
import { formatTs, fromLocalInput, toLocalInput } from "../time";
import type { LogGroup, LogGroupPage, PartLogEvent } from "../types";

const PAGE_SIZE = 50;
const ranges = [
  { key: "15m", label: "15 分钟", ms: 15 * 60_000 },
  { key: "1h", label: "1 小时", ms: 3_600_000 },
  { key: "24h", label: "24 小时", ms: 86_400_000 },
  { key: "7d", label: "7 天", ms: 7 * 86_400_000 },
  { key: "custom", label: "自定义", ms: 0 },
] as const;

const eventLabels: Record<PartLogEvent, string> = {
  received: "接到工件",
  ended: "工件结束",
  resultWritten: "结果写入完成",
  resultFailed: "结果写入失败",
  interrupted: "工件中断",
  skipped: "跳过工件",
};

function groupLabel(group: LogGroup) {
  return group.partSeq === null ? "未关联工件" : `#${group.partSeq}`;
}

export default function PlcLogPage() {
  const [range, setRange] = useState<(typeof ranges)[number]["key"]>("1h");
  const [customStart, setCustomStart] = useState(() => toLocalInput(Date.now() - 3_600_000));
  const [customEnd, setCustomEnd] = useState(() => toLocalInput(Date.now()));
  const [input, setInput] = useState("");
  const [keyword, setKeyword] = useState("");
  const [abnormalOnly, setAbnormalOnly] = useState(false);
  const [live, setLive] = useState(true);
  const [page, setPage] = useState(1);
  const [selected, setSelected] = useState<LogGroup | null>(null);
  const [data, setData] = useState<LogGroupPage | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const refresh = useRef<() => void>(() => {});
  const following = live && range !== "custom" && page === 1;

  useEffect(() => {
    let active = true;
    let busy = false;
    let dirty = false;
    setData(null);
    async function load() {
      if (busy) { dirty = true; return; }
      busy = true;
      setLoading(true);
      setError("");
      try {
        const start = range === "custom" ? fromLocalInput(customStart) : Date.now() - ranges.find(r => r.key === range)!.ms;
        const end = range === "custom" ? fromLocalInput(customEnd) : null;
        if (start === null || (range === "custom" && (end === null || start > end))) {
          throw new Error("请选择有效时间段，开始时间不得晚于结束时间");
        }
        const result = await plcApi.queryLogGroups({ start, end, keyword, abnormalOnly, limit: PAGE_SIZE, offset: (page - 1) * PAGE_SIZE });
        if (active) {
          const lastPage = Math.max(1, Math.ceil(result.total / PAGE_SIZE));
          if (page > lastPage) setPage(lastPage);
          else setData(result);
        }
      } catch (cause) {
        if (active) setError(errorText(cause));
      } finally {
        busy = false;
        if (active) {
          setLoading(false);
          if (dirty) { dirty = false; void load(); }
        }
      }
    }
    refresh.current = load;
    void load();
    return () => { active = false; refresh.current = () => {}; };
  }, [range, customStart, customEnd, keyword, abnormalOnly, page]);

  useEffect(() => {
    if (!following) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const off = subscribe("plc://log", () => {
      if (timer !== undefined) return;
      timer = setTimeout(() => { timer = undefined; refresh.current(); }, 800);
    });
    return () => { off(); clearTimeout(timer); };
  }, [following]);

  const columns: TableColumn<LogGroup>[] = [
    { key: "partSeq", header: "件序号", rowHeader: true, render: g =>
      <Button variant="text" size="sm" onClick={e => { e.stopPropagation(); setSelected(g); }}>{groupLabel(g)}</Button> },
    { key: "sn", header: "SN", render: g => <span className="mono">{g.sn ?? "—"}</span> },
    { key: "type", header: "型号", render: g => g.productCode ?? "—" },
    { key: "time", header: "最近通讯", nowrap: true, render: g => <time className="mono">{formatTs(g.lastTs)}</time> },
    { key: "event", header: "通讯进度", render: g => g.event ? <Stack gap={6}>
      <span>{eventLabels[g.event]}</span>
      {g.event === "resultWritten" && g.resultCode != null && <span className="muted">{resultCodeLabel(g.resultCode, g.faultCode)}</span>}
    </Stack> : <span className="muted">—</span> },
    { key: "logs", header: "记录", render: g => <Stack gap={6}>
      <span>{g.entryCount} 条</span>
      {g.errorCount > 0 && <StatusBadge tone="ng">{g.errorCount} 错误</StatusBadge>}
      {g.warningCount > 0 && <StatusBadge tone="warn">{g.warningCount} 警告</StatusBadge>}
    </Stack> },
  ];

  return <div className="stack">
    <Card>
      <Stack gap={8}>
        <SegmentedControl label="时间范围" value={range} options={ranges.map(r => ({ value: r.key, label: r.label }))}
          onValueChange={v => { setRange(v as typeof range); setPage(1); }} />
        {range === "custom" && <>
          <TextField aria-label="开始时间" type="datetime-local" step={1} value={customStart} onChange={e => { setCustomStart(e.target.value); setPage(1); }} />
          <span className="muted">至</span>
          <TextField aria-label="结束时间" type="datetime-local" step={1} value={customEnd} onChange={e => { setCustomEnd(e.target.value); setPage(1); }} />
        </>}
        <form className="plc-search" onSubmit={e => { e.preventDefault(); setKeyword(input.trim()); setPage(1); }}>
          <TextField aria-label="搜索件序号或 SN" leading={<Search size={16} />} placeholder="件序号或 SN，回车搜索" value={input}
            trailing={input || keyword ? <IconButton type="button" variant="text" size="sm" aria-label="清空搜索"
              onClick={() => { setInput(""); setKeyword(""); setPage(1); }}><X size={14} /></IconButton> : undefined}
            onChange={e => setInput(e.target.value)} />
        </form>
        <Chip selected={abnormalOnly} onClick={() => { setAbnormalOnly(v => !v); setPage(1); }}>只看通讯异常</Chip>
        <div className="spacer" />
        <Button variant="tonal" aria-pressed={following} disabled={range === "custom"} leadingIcon={<Radio size={16} />}
          onClick={() => { setLive(!following); setPage(1); }}>{following ? "实时跟随中" : "实时跟随"}</Button>
        <IconButton aria-label="刷新通讯日志" disabled={loading} onClick={() => refresh.current()}><RefreshCw size={16} /></IconButton>
      </Stack>
    </Card>
    <Card title={<Heading level={3}>工件通讯记录</Heading>} actions={data && data.total > PAGE_SIZE &&
      <Pagination label="工件记录分页" total={data.total} page={page} pageSize={PAGE_SIZE} disabled={loading} showPageSize={false} onPageChange={setPage} />}>
      {error && <Alert tone="ng">{error}</Alert>}
      <Table caption="工件通讯记录" rows={data?.items ?? []} columns={columns} rowKey={g => String(g.firstId)}
        getRowProps={g => ({ onClick: () => setSelected(g), className: "plc-log-group-row" })}
        loading={loading} keepRowsWhileLoading emptyState={<EmptyState title="所选条件下暂无通讯记录" />} />
    </Card>
    {selected && <PlcLogDetails group={selected} onClose={() => setSelected(null)} />}
  </div>;
}
