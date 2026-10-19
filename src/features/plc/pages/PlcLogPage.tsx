import { useCallback, useEffect, useRef, useState } from "react";
import { Download, Hash, Radio, RefreshCw, Search, X } from "lucide-react";
import { Alert, Button, Card, Chip, EmptyState, Heading, IconButton, Pagination, SegmentedControl, Select, Stack, StatusBadge, Table, TextField, type TableColumn } from "qing-steel";
import StepChart from "../components/StepChart";
import { plcApi, subscribe } from "../api";
import { categoryLabels, levelLabels } from "../meta";
import type { HistorySample, LogCategory, LogEntry, LogLevel, LogPage, LogQuery, PlcPoint } from "../types";
import { formatTs, fromLocalInput, toLocalInput } from "../time";

const PAGE_SIZE = 100;

const ranges = [
  { key: "15m", label: "15 分钟", ms: 15 * 60_000 },
  { key: "1h", label: "1 小时", ms: 3_600_000 },
  { key: "24h", label: "24 小时", ms: 86_400_000 },
  { key: "7d", label: "7 天", ms: 7 * 86_400_000 },
  { key: "custom", label: "自定义", ms: 0 },
] as const;

type RangeKey = (typeof ranges)[number]["key"];

function toggle<T>(list: T[], v: T) {
  return list.includes(v) ? list.filter((x) => x !== v) : [...list, v];
}

function csvCell(v: string | null | undefined) {
  const s = v ?? "";
  return /[",\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

const levelTone = { info: "info", warn: "warn", error: "ng" } as const;

export default function PlcLogPage() {
  const [points, setPoints] = useState<PlcPoint[]>([]);
  const [rangeKey, setRangeKey] = useState<RangeKey>("1h");
  const [customStart, setCustomStart] = useState(() => toLocalInput(Date.now() - 3_600_000));
  const [customEnd, setCustomEnd] = useState(() => toLocalInput(Date.now()));
  const [levels, setLevels] = useState<LogLevel[]>([]);
  const [categories, setCategories] = useState<LogCategory[]>([]);
  const [pointId, setPointId] = useState("");
  const [keywordInput, setKeywordInput] = useState("");
  const [keyword, setKeyword] = useState("");
  const [contextInput, setContextInput] = useState("");
  const [context, setContext] = useState("");
  const [page, setPage] = useState(0);
  const [live, setLive] = useState(true);
  const [data, setData] = useState<LogPage>({ total: 0, items: [] });
  const [dataScope, setDataScope] = useState(-1);
  const [dataPage, setDataPage] = useState(-1);
  const [history, setHistory] = useState<{ samples: HistorySample[]; start: number; end: number } | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [configError, setConfigError] = useState("");
  const [exportError, setExportError] = useState("");
  const [exporting, setExporting] = useState(false);
  const exportPending = useRef<object | null>(null);
  const requestSerial = useRef(0);
  const configSerial = useRef(0);
  const alive = useRef(true);
  const filterKey = JSON.stringify([rangeKey, customStart, customEnd, levels, categories, pointId, context, keyword]);
  const scope = useRef({ key: filterKey, generation: 0 });
  if (scope.current.key !== filterKey) scope.current = { key: filterKey, generation: scope.current.generation + 1 };
  const generation = scope.current.generation;
  const currentData = dataScope === generation && dataPage === page ? data : { total: 0, items: [] };

  const loadPoints = useCallback(() => {
    const serial = ++configSerial.current;
    setConfigError("");
    plcApi.getConfig().then(c => { if (alive.current && serial === configSerial.current) setPoints(c.points); })
      .catch(e => { if (alive.current && serial === configSerial.current) setConfigError(String(e)); });
  }, []);

  useEffect(() => {
    alive.current = true; loadPoints();
    return () => { alive.current = false; configSerial.current++; };
  }, [loadPoints]);

  useEffect(() => {
    exportPending.current = null; setExporting(false); setExportError("");
  }, [generation]);

  const buildQuery = useCallback((): { query: LogQuery; start: number; end: number } => {
    const now = Date.now();
    if (rangeKey === "custom") {
      const start = fromLocalInput(customStart);
      const end = fromLocalInput(customEnd);
      if (start === null || end === null || start > end) throw new Error("请选择有效时间段，开始时间不得晚于结束时间");
      return { query: { start, end, levels, categories, pointId: pointId || null, context: context || null, keyword: keyword || null }, start, end };
    }
    const start = now - ranges.find((r) => r.key === rangeKey)!.ms;
    return { query: { start, end: null, levels, categories, pointId: pointId || null, context: context || null, keyword: keyword || null }, start, end: now };
  }, [rangeKey, customStart, customEnd, levels, categories, pointId, context, keyword]);

  const run = useCallback(async () => {
    const serial = ++requestSerial.current;
    setLoading(true);
    setError("");
    setHistory(null);
    try {
      const { query, start, end } = buildQuery();
      const [result, samples] = await Promise.allSettled([
        plcApi.queryLogs({ ...query, limit: PAGE_SIZE, offset: page * PAGE_SIZE }),
        pointId ? plcApi.pointHistory(pointId, start, end) : Promise.resolve(null),
      ]);
      if (serial !== requestSerial.current) return;
      if (result.status === "rejected") throw result.reason;
      setData(result.value);
      setDataScope(generation); setDataPage(page);
      if (samples.status === "rejected") setError(`点位趋势：${String(samples.reason)}`);
      else setHistory(samples.value ? { samples: samples.value, start, end } : null);
    } catch (e) {
      if (serial === requestSerial.current) setError(String(e));
    } finally {
      if (serial === requestSerial.current) setLoading(false);
    }
  }, [buildQuery, page, pointId, generation]);

  const runRef = useRef(run);
  runRef.current = run;

  useEffect(() => {
    void run();
    return () => { requestSerial.current++; };
  }, [run]);

  const following = live && rangeKey !== "custom" && page === 0;

  useEffect(() => {
    if (!following) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const unsubscribe = subscribe("plc://log", () => {
      if (timer) return;
      timer = setTimeout(() => {
        timer = null;
        runRef.current();
      }, 800);
    });
    return () => {
      unsubscribe();
      if (timer) clearTimeout(timer);
    };
  }, [following]);

  const resetPage = () => setPage(0);

  const exportCsv = async () => {
    if (exportPending.current) return;
    const request = {};
    const exportScope = generation;
    const current = () => alive.current && scope.current.generation === exportScope && exportPending.current === request;
    exportPending.current = request; setExporting(true); setExportError("");
    try {
    const { query, end } = buildQuery();
    query.end = end;
    const rows: string[] = ["时间,级别,类别,点位,内容,旧值,新值,事务"];
    let offset = 0;
    let total: number | null = null;
    while (true) {
      const batch = await plcApi.queryLogs({ ...query, limit: 1000, offset });
      if (!current()) return;
      total ??= batch.total;
      if (!Number.isSafeInteger(total) || total < 0) throw new Error("日志数量无效，请刷新后重试");
      for (const e of batch.items) {
        rows.push(
          [formatTs(e.ts), levelLabels[e.level], categoryLabels[e.category], e.pointName, e.message, e.oldValue, e.newValue, e.context]
            .map(csvCell)
            .join(","),
        );
      }
      offset += batch.items.length;
      if (offset >= total) break;
      if (batch.items.length === 0) throw new Error("日志在导出期间已变化，未下载不完整文件，请刷新后重试");
    }
    if (!current()) return;
    const blob = new Blob(["﻿" + rows.join("\n")], { type: "text/csv;charset=utf-8" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `plc-log-${toLocalInput(Date.now()).replace(/[:T]/g, "")}.csv`;
    try { a.click(); } finally { URL.revokeObjectURL(a.href); }
    } catch (e) { if (current()) setExportError(String(e)); }
    finally {
      if (exportPending.current === request) {
        exportPending.current = null;
        if (alive.current) setExporting(false);
      }
    }
  };

  const selectedPoint = points.find((p) => p.id === pointId);

  const logColumns: TableColumn<LogEntry>[] = [
    { key: "ts", header: "时间", render: (e) => <span className="mono nowrap">{formatTs(e.ts)}</span> },
    { key: "level", header: "级别", render: (e) => <StatusBadge tone={levelTone[e.level]}>{levelLabels[e.level]}</StatusBadge> },
    { key: "category", header: "类别", render: (e) => <span className="nowrap">{categoryLabels[e.category] ?? e.category}</span> },
    { key: "point", header: "点位", render: (e) => e.pointId
      ? <Button variant="text" size="sm" onClick={() => { setPointId(e.pointId!); resetPage(); }}>{e.pointName ?? e.pointId}</Button>
      : <span className="muted">—</span> },
    { key: "message", header: "内容", render: (e) => e.message },
    { key: "context", header: "事务", render: (e) => e.context
      ? <Button variant="text" size="sm" onClick={() => { setContextInput(e.context!); setContext(e.context!); resetPage(); }}><span className="mono">{e.context}</span></Button>
      : <span className="muted">—</span> },
  ];

  return (
    <div className="stack">
      <Card>
        <Stack direction="column" align="stretch" gap={12}>
        <Stack gap={8}>
          <SegmentedControl label="时间范围" value={rangeKey} options={ranges.map((r) => ({ value: r.key, label: r.label }))} onValueChange={(value) => { setRangeKey(value as RangeKey); resetPage(); }} />
          {rangeKey === "custom" && (
            <Stack gap={8}>
              <TextField aria-label="开始时间" type="datetime-local" step={1} value={customStart} onChange={(e) => { setCustomStart(e.target.value); resetPage(); }} />
              <span className="muted">至</span>
              <TextField aria-label="结束时间" type="datetime-local" step={1} value={customEnd} onChange={(e) => { setCustomEnd(e.target.value); resetPage(); }} />
            </Stack>
          )}
          <div className="spacer" />
          <Button
            variant={following ? "filled" : "tonal"}
            leadingIcon={<Radio size={16} />}
            disabled={rangeKey === "custom"}
            title={rangeKey === "custom" ? "自定义时间段不支持实时跟随" : ""}
            onClick={() => {
              setLive((v) => !v);
              resetPage();
            }}
          >
            {following ? "实时跟随中" : "实时跟随"}
          </Button>
          <Button leadingIcon={<RefreshCw size={16} className={loading ? "spin" : ""} />} onClick={run}>刷新</Button>
          <Button leadingIcon={<Download size={16} />} disabled={exporting} onClick={() => void exportCsv()}>{exporting ? "导出中…" : "导出 CSV"}</Button>
          <Button onClick={() => {
            const now = Date.now();
            setRangeKey("1h"); setCustomStart(toLocalInput(now - 3_600_000)); setCustomEnd(toLocalInput(now));
            setLevels([]); setCategories([]); setPointId(""); setContextInput(""); setContext(""); setKeywordInput(""); setKeyword(""); setPage(0); setLive(true);
          }}>清空筛选</Button>
        </Stack>

        <Stack gap={8}>
          <span className="plc-filter-label">级别</span>
          {(Object.keys(levelLabels) as LogLevel[]).map((l) => (
            <Chip key={l} selected={levels.includes(l)} onClick={() => { setLevels(toggle(levels, l)); resetPage(); }}>{levelLabels[l]}</Chip>
          ))}
          <span className="plc-filter-label">类别</span>
          {(Object.keys(categoryLabels) as LogCategory[]).map((c) => (
            <Chip key={c} selected={categories.includes(c)} onClick={() => { setCategories(toggle(categories, c)); resetPage(); }}>{categoryLabels[c]}</Chip>
          ))}
        </Stack>

        <Stack gap={8}>
          <span className="plc-filter-label">点位</span>
          <Select aria-label="日志点位" value={pointId} onValueChange={(value) => { setPointId(value); resetPage(); }}
            options={[
              { value: "", label: "全部点位" },
              ...(pointId && !points.some(p => p.id === pointId) ? [{ value: pointId, label: `${pointId}（已移除）` }] : []),
              ...points.map((p) => ({ value: p.id, label: p.name })),
            ]} />
          <div className="plc-search">
            <TextField
              aria-label="事务"
              leading={<Hash size={16} />}
              trailing={contextInput ? <IconButton variant="text" size="sm" aria-label="清空事务" onClick={() => { setContextInput(""); setContext(""); resetPage(); }}><X size={14} /></IconButton> : undefined}
              placeholder="事务精确匹配，如 requestSeq=12，回车确认"
              value={contextInput}
              onChange={(e) => setContextInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  setContext(contextInput.trim());
                  resetPage();
                }
              }}
            />
          </div>
          <div className="plc-search">
            <TextField
              aria-label="搜索内容或点位名称"
              leading={<Search size={16} />}
              trailing={keywordInput ? <IconButton variant="text" size="sm" aria-label="清空搜索" onClick={() => { setKeywordInput(""); setKeyword(""); resetPage(); }}><X size={14} /></IconButton> : undefined}
              placeholder="搜索内容或点位名称，回车确认"
              value={keywordInput}
              onChange={(e) => setKeywordInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  setKeyword(keywordInput.trim());
                  resetPage();
                }
              }}
            />
          </div>
        </Stack>
        </Stack>
      </Card>

      {configError && <Alert tone="ng" action={<Button size="sm" onClick={loadPoints}>重新加载点位</Button>}>点位配置：{configError}</Alert>}
      {exportError && <Alert tone="ng">{exportError}</Alert>}

      {history && dataScope === generation && dataPage === page && (
        <Card title={<Heading level={3}>点位趋势 · {selectedPoint?.name ?? pointId}</Heading>} actions={<span className="muted">{history.samples.length} 个采样点 · 基于值变化记录</span>}>
          <StepChart samples={history.samples} start={history.start} end={history.end} />
        </Card>
      )}

      <Card title={<Stack gap={10}>
          <Heading level={3}>日志</Heading>
          <span className="muted">共 {currentData.total} 条</span>
        </Stack>} actions={<Pagination label="日志分页" total={currentData.total} page={page + 1} pageSize={PAGE_SIZE} pageSizeOptions={[PAGE_SIZE]} disabled={loading} onPageChange={(value) => setPage(value - 1)} />}>
        <Stack direction="column" align="stretch" gap={12}>
          {error && <Alert tone="ng">{error}</Alert>}
          <Table<LogEntry> caption="PLC 日志" rows={currentData.items} columns={logColumns} rowKey={(e) => String(e.id)}
            emptyState={<EmptyState title={loading ? "加载中…" : "所选条件下暂无日志"} />} />
        </Stack>
      </Card>
    </div>
  );
}
