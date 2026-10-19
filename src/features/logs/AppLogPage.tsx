import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { FolderOpen, PackageOpen, Radio, RefreshCw, Search, X } from "lucide-react";
import { Alert, Button, Card, Chip, EmptyState, IconButton, Pagination, SegmentedControl, Stack, StatusBadge, Table, Text, TextField, type TableColumn } from "qing-steel";
import { subscribe } from "../plc";
import { formatTs, fromLocalInput, toLocalInput } from "../plc/time";
import { openLogDir } from "../../lib/api";
import { errorText } from "../../lib/errors";
import { formatBytes, levelText, logsApi } from "./api";
import DateTimeField from "./DateTimeField";
import DiagnosticsDialog from "./DiagnosticsDialog";
import type { AppLogLevel, AppLogPage as Page, AppLogQuery, AppLogRecord, LogUsage } from "./types";

const PAGE_SIZE = 100;
const ranges = [
  { key: "15m", label: "15 分钟", ms: 15 * 60_000 },
  { key: "1h", label: "1 小时", ms: 3_600_000 },
  { key: "24h", label: "24 小时", ms: 86_400_000 },
  { key: "7d", label: "7 天", ms: 7 * 86_400_000 },
  { key: "custom", label: "自定义", ms: 0 },
] as const;
type RangeKey = (typeof ranges)[number]["key"];
const rangeOptions = ranges.map(r => ({ value: r.key, label: r.label }));
const levels: AppLogLevel[] = ["error", "warn", "info", "debug"];
const DEFAULT_LEVELS: AppLogLevel[] = ["error", "warn", "info"];
const levelTone = { error: "ng", warn: "warn", info: "info", debug: "off" } as const;

type Row = [string, AppLogRecord];

function rowKeys(items: AppLogRecord[]): Row[] {
  const seen = new Map<string, number>();
  return items.map(r => {
    const base = `${r.ts}|${r.level}|${r.ev}|${r.cycleId ?? ""}|${r.k ?? ""}|${r.msg.slice(0, 160)}`;
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    return [`${base}#${n}`, r];
  });
}

function Cycle({ record, onFilter }: { record: AppLogRecord; onFilter: (cycleId: string) => void }) {
  const [target, setTarget] = useState<number | null | undefined>(undefined);
  if (!record.cycleId) return record.sn ? <span className="log-cycle"><span className="sn">SN{" "}{record.sn}</span></span> : <span className="muted">—</span>;
  const cycleId = record.cycleId;
  const lookup = () => void logsApi.findCycle(cycleId).then(setTarget).catch(() => setTarget(null));
  return <span className="log-cycle">
    {record.sn && <span className="sn muted">SN{" "}{record.sn}</span>}
    <span>
      <Button variant="text" size="sm" className="mono" title={`只看工件 ${cycleId}`} onClick={() => onFilter(cycleId)}>{cycleId.slice(0, 8)}</Button>
      {typeof target === "number" ? <Link to={`/history/${target}`}>#{target}</Link>
        : <Button variant="text" size="sm" onClick={lookup}>{target === null ? "无记录，重查" : "记录"}</Button>}
    </span>
  </span>;
}

export default function AppLogPage() {
  const [params, setParams] = useSearchParams();
  const cycleId = params.get("cycleId") ?? "";
  const [rangeKey, setRangeKey] = useState<RangeKey>(cycleId ? "7d" : "1h");
  const [customStart, setCustomStart] = useState(() => toLocalInput(Date.now() - 3_600_000));
  const [customEnd, setCustomEnd] = useState(() => toLocalInput(Date.now()));
  const [chosen, setChosen] = useState<AppLogLevel[]>(cycleId ? levels : DEFAULT_LEVELS);
  const [keywordInput, setKeywordInput] = useState("");
  const [keyword, setKeyword] = useState("");
  const [sn, setSn] = useState("");
  const [page, setPage] = useState(0);
  const [live, setLive] = useState(true);
  const [data, setData] = useState<{ key: string; page: Page } | null>(null);
  const [usage, setUsage] = useState<LogUsage | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [exporting, setExporting] = useState(false);
  const serial = useRef(0);
  const key = JSON.stringify([rangeKey, customStart, customEnd, chosen, keyword, sn, cycleId, page]);

  const query = useCallback((): AppLogQuery => {
    const base = { levels: chosen, keyword: keyword || null, sn: sn || null, cycleId: cycleId || null, offset: page * PAGE_SIZE, limit: PAGE_SIZE };
    if (rangeKey === "custom") {
      const from = fromLocalInput(customStart), to = fromLocalInput(customEnd);
      if (from === null || to === null || from > to) throw new Error("请选择有效时间段，开始时间不得晚于结束时间");
      return { ...base, from, to };
    }
    return { ...base, from: Date.now() - ranges.find(r => r.key === rangeKey)!.ms, to: null };
  }, [rangeKey, customStart, customEnd, chosen, keyword, sn, cycleId, page]);

  const run = useCallback(async () => {
    const mine = ++serial.current;
    setLoading(true); setError("");
    try {
      const result = await logsApi.query(query());
      if (mine === serial.current) setData({ key, page: result });
    } catch (e) { if (mine === serial.current) setError(errorText(e)); }
    finally { if (mine === serial.current) setLoading(false); }
    logsApi.usage().then(setUsage).catch(() => {});
  }, [query, key]);
  const runRef = useRef(run);
  runRef.current = run;
  useEffect(() => { void run(); return () => { serial.current++; }; }, [run]);

  const following = live && rangeKey !== "custom" && page === 0 && !cycleId;
  useEffect(() => {
    if (!following) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let running = false, again = false, stopped = false;
    const refresh = async () => {
      timer = null;
      running = true;
      try { await runRef.current(); } finally { running = false; }
      if (again && !stopped) { again = false; schedule(); }
    };
    const schedule = () => {
      if (running) { again = true; return; }
      if (!timer) timer = setTimeout(() => void refresh(), 1000);
    };
    const stop = subscribe("cycle://log", schedule);
    return () => { stopped = true; stop(); if (timer) clearTimeout(timer); };
  }, [following]);

  const stale = data?.key !== key;
  const current = data && !(stale && error) ? data.page : null;
  const pending = stale && !!current;
  const rows = useMemo(() => rowKeys(current?.items ?? []), [current]);
  const reset = () => setPage(0);
  const setCycle = (id: string) => { const next = new URLSearchParams(params); if (id) next.set("cycleId", id); else next.delete("cycleId"); setParams(next); reset(); };
  const toggle = (level: AppLogLevel) => { setChosen(chosen.includes(level) ? chosen.filter(l => l !== level) : [...chosen, level]); reset(); };
  const span = useMemo(() => {
    try { const q = query(); return { from: q.from ?? Date.now() - 3_600_000, to: q.to ?? Date.now() }; } catch { return { from: Date.now() - 3_600_000, to: Date.now() }; }
  }, [query, exporting]);

  const columns: TableColumn<Row>[] = [
    { key: "ts", header: "时间", render: ([, r]) => <span className="mono nowrap" data-level={r.level}>{formatTs(r.ts)}</span> },
    { key: "level", header: "级别", render: ([, r]) => <StatusBadge tone={levelTone[r.level]}>{levelText[r.level]}</StatusBadge> },
    { key: "ev", header: "事件", render: ([, r]) => <span className="nowrap">{r.ev}</span> },
    { key: "cycle", header: "工件", render: ([, r]) => <Cycle record={r} onFilter={setCycle} /> },
    { key: "shot", header: "拍照点", render: ([, r]) => <span className="nowrap mono">{r.k != null ? `k${r.k + 1}` : ""}{r.shotId ? ` · ${r.shotId}` : ""}{r.camera ? ` · ${r.camera}` : ""}</span> },
    {
      key: "msg",
      header: "内容",
      render: ([, r]) => <div className="log-content"><span className="log-msg" title={r.msg}>{r.msg}</span>{r.data != null && <details className="log-data"><summary>结构化字段</summary><pre className="mono">{JSON.stringify(r.data, null, 2)}</pre></details>}</div>,
    },
  ];

  return <Stack direction="column" align="stretch" gap={16}>
    <Card>
      <Stack direction="column" align="stretch" gap={12}>
        <Stack gap={8} justify="space-between">
          <Stack gap={8}>
            <SegmentedControl label="日志时间范围" options={rangeOptions} value={rangeKey} onValueChange={value => { setRangeKey(value as RangeKey); reset(); }} />
            {rangeKey === "custom" && <Stack gap={8}>
              <DateTimeField label="开始" value={customStart} onChange={value => { setCustomStart(value); reset(); }} />
              <Text muted>至</Text>
              <DateTimeField label="结束" value={customEnd} onChange={value => { setCustomEnd(value); reset(); }} />
            </Stack>}
          </Stack>
          <Stack gap={8}>
            <Button variant={following ? "filled" : "tonal"} leadingIcon={<Radio size={16} />} disabled={rangeKey === "custom" || !!cycleId} title={cycleId ? "按工件筛选时不实时跟随" : undefined} onClick={() => { setLive(v => !v); reset(); }}>{following ? "实时跟随中" : "实时跟随"}</Button>
            <Button leadingIcon={<RefreshCw size={16} className={loading ? "spin" : ""} />} onClick={() => void run()}>刷新</Button>
            <Button variant="filled" leadingIcon={<PackageOpen size={16} />} onClick={() => setExporting(true)}>导出诊断包</Button>
          </Stack>
        </Stack>
        <Stack gap={8}>
          <Text variant="label" muted>级别</Text>
          {levels.map(l => <Chip key={l} selected={chosen.includes(l)} onClick={() => toggle(l)}>{levelText[l]}</Chip>)}
          <Text variant="label" muted>SN</Text>
          <TextField className="log-sn-input" aria-label="按 SN 筛选" placeholder="SN" value={sn} onChange={e => { setSn(e.target.value); reset(); }} />
          <TextField
            className="log-search"
            aria-label="搜索日志"
            placeholder="搜索事件、内容、cycleId 或拍照点，回车确认"
            value={keywordInput}
            leading={<Search size={16} />}
            trailing={keywordInput ? <IconButton size="sm" variant="text" aria-label="清空搜索" onClick={() => { setKeywordInput(""); setKeyword(""); reset(); }}><X size={14} /></IconButton> : undefined}
            onChange={e => setKeywordInput(e.target.value)}
            onKeyDown={e => { if (e.key === "Enter") { setKeyword(keywordInput.trim()); reset(); } }}
          />
          {cycleId && <Chip onRemove={() => setCycle("")} removeLabel="取消工件筛选">工件 <span className="mono">{cycleId}</span></Chip>}
        </Stack>
        <Stack gap={12} className="log-usage">
          <Text variant="label" muted>{usage ? <>日志占用 {formatBytes(usage.bytes)} / 上限 {formatBytes(usage.maxTotalBytes)} · {usage.files} 个文件 · 保留 {usage.keepDays} 天 · 单文件 {formatBytes(usage.segmentBytes)} 滚动</> : "日志占用读取中…"}</Text>
          <Button variant="text" size="sm" leadingIcon={<FolderOpen size={13} />} onClick={() => void openLogDir().catch(e => setError(errorText(e)))}>打开日志目录</Button>
        </Stack>
      </Stack>
    </Card>
    <Card title="运行日志" actions={<Text muted>共 {current?.total ?? 0} 条</Text>}>
      {error && <Alert tone="ng">{error}</Alert>}
      <div className={`app-log-body${pending ? " is-loading" : ""}`} aria-busy={loading}>
        <Table<Row>
          caption="运行日志"
          className="log-table"
          rows={rows}
          columns={columns}
          rowKey={([rowKey]) => rowKey}
          loading={!current && loading}
          emptyState={<EmptyState title="所选条件下暂无日志" />}
        />
      </div>
      <Pagination
        className="log-pager"
        total={current?.total ?? 0}
        page={page + 1}
        pageSize={PAGE_SIZE}
        pageSizeOptions={[PAGE_SIZE]}
        disabled={loading}
        onPageChange={next => setPage(next - 1)}
      />
    </Card>
    {exporting && <DiagnosticsDialog from={span.from} to={span.to} onClose={() => setExporting(false)} />}
  </Stack>;
}
