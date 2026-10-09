import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Download, Radio, RefreshCw, Search, X } from "lucide-react";
import StepChart from "../components/StepChart";
import { plcApi, subscribe } from "../api";
import { categoryLabels, levelLabels } from "../meta";
import type { HistorySample, LogCategory, LogLevel, LogPage, LogQuery, PlcPoint } from "../types";
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
  const filterKey = JSON.stringify([rangeKey, customStart, customEnd, levels, categories, pointId, keyword]);
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
      return { query: { start, end, levels, categories, pointId: pointId || null, keyword: keyword || null }, start, end };
    }
    const start = now - ranges.find((r) => r.key === rangeKey)!.ms;
    return { query: { start, end: null, levels, categories, pointId: pointId || null, keyword: keyword || null }, start, end: now };
  }, [rangeKey, customStart, customEnd, levels, categories, pointId, keyword]);

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
    const rows: string[] = ["时间,级别,类别,点位,内容,旧值,新值"];
    let offset = 0;
    let total: number | null = null;
    while (true) {
      const batch = await plcApi.queryLogs({ ...query, limit: 1000, offset });
      if (!current()) return;
      total ??= batch.total;
      if (!Number.isSafeInteger(total) || total < 0) throw new Error("日志数量无效，请刷新后重试");
      for (const e of batch.items) {
        rows.push(
          [formatTs(e.ts), levelLabels[e.level], categoryLabels[e.category], e.pointName, e.message, e.oldValue, e.newValue]
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

  const pageCount = Math.max(1, Math.ceil(currentData.total / PAGE_SIZE));
  const selectedPoint = points.find((p) => p.id === pointId);

  return (
    <div className="stack">
      <div className="panel filters">
        <div className="filter-row">
          <div className="segmented">
            {ranges.map((r) => (
              <button
                key={r.key}
                aria-pressed={rangeKey === r.key}
                className={rangeKey === r.key ? "active" : ""}
                onClick={() => {
                  setRangeKey(r.key);
                  resetPage();
                }}
              >
                {r.label}
              </button>
            ))}
          </div>
          {rangeKey === "custom" && (
            <div className="row">
              <input className="input mono" aria-label="开始时间" type="datetime-local" step={1} value={customStart} onChange={(e) => { setCustomStart(e.target.value); resetPage(); }} />
              <span className="muted">至</span>
              <input className="input mono" aria-label="结束时间" type="datetime-local" step={1} value={customEnd} onChange={(e) => { setCustomEnd(e.target.value); resetPage(); }} />
            </div>
          )}
          <div className="spacer" />
          <button
            className={`btn ${following ? "live" : ""}`}
            disabled={rangeKey === "custom"}
            title={rangeKey === "custom" ? "自定义时间段不支持实时跟随" : ""}
            onClick={() => {
              setLive((v) => !v);
              resetPage();
            }}
          >
            <Radio size={16} />
            {following ? "实时跟随中" : "实时跟随"}
          </button>
          <button className="btn" onClick={run}>
            <RefreshCw size={16} className={loading ? "spin" : ""} />
            刷新
          </button>
          <button className="btn" disabled={exporting} onClick={() => void exportCsv()}>
            <Download size={16} />
            {exporting ? "导出中…" : "导出 CSV"}
          </button>
          <button className="btn" onClick={() => {
            const now = Date.now();
            setRangeKey("1h"); setCustomStart(toLocalInput(now - 3_600_000)); setCustomEnd(toLocalInput(now));
            setLevels([]); setCategories([]); setPointId(""); setKeywordInput(""); setKeyword(""); setPage(0); setLive(true);
          }}>清空筛选</button>
        </div>

        <div className="filter-row">
          <span className="filter-label">级别</span>
          {(Object.keys(levelLabels) as LogLevel[]).map((l) => (
            <button key={l} aria-pressed={levels.includes(l)} className={`chip level-${l} ${levels.includes(l) ? "on" : ""}`} onClick={() => { setLevels(toggle(levels, l)); resetPage(); }}>
              {levelLabels[l]}
            </button>
          ))}
          <span className="filter-label">类别</span>
          {(Object.keys(categoryLabels) as LogCategory[]).map((c) => (
            <button key={c} aria-pressed={categories.includes(c)} className={`chip ${categories.includes(c) ? "on" : ""}`} onClick={() => { setCategories(toggle(categories, c)); resetPage(); }}>
              {categoryLabels[c]}
            </button>
          ))}
        </div>

        <div className="filter-row">
          <span className="filter-label">点位</span>
          <select className="input" aria-label="日志点位" value={pointId} onChange={(e) => { setPointId(e.target.value); resetPage(); }}>
            <option value="">全部点位</option>
            {pointId && !points.some(p => p.id === pointId) && <option value={pointId}>{pointId}（已移除）</option>}
            {points.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
          <div className="search">
            <Search size={16} />
            <input
              className="input"
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
            {keywordInput && (
              <button className="icon-btn" aria-label="清空搜索" onClick={() => { setKeywordInput(""); setKeyword(""); resetPage(); }}>
                <X size={14} />
              </button>
            )}
          </div>
        </div>
      </div>

      {configError && <div className="notice error" role="alert">点位配置：{configError} <button className="btn" onClick={loadPoints}>重新加载点位</button></div>}
      {exportError && <div className="notice error" role="alert">{exportError}</div>}

      {history && dataScope === generation && dataPage === page && (
        <div className="panel">
          <div className="panel-toolbar">
            <h3 className="panel-title">点位趋势 · {selectedPoint?.name ?? pointId}</h3>
            <span className="muted">{history.samples.length} 个采样点 · 基于值变化记录</span>
          </div>
          <StepChart samples={history.samples} start={history.start} end={history.end} />
        </div>
      )}

      <div className="panel">
        <div className="panel-toolbar">
          <div className="row">
            <h3 className="panel-title">日志</h3>
            <span className="muted">共 {currentData.total} 条</span>
            {error && <span className="ng">{error}</span>}
          </div>
          <div className="row">
            <button className="icon-btn" aria-label="上一页" disabled={loading || page === 0} onClick={() => setPage(page - 1)}>
              <ChevronLeft size={18} />
            </button>
            <span className="mono muted">
              {page + 1} / {pageCount}
            </span>
            <button className="icon-btn" aria-label="下一页" disabled={loading || page + 1 >= pageCount} onClick={() => setPage(page + 1)}>
              <ChevronRight size={18} />
            </button>
          </div>
        </div>
        <div className="table-wrap">
          <table className="table log-table">
            <thead>
              <tr>
                <th>时间</th>
                <th>级别</th>
                <th>类别</th>
                <th>点位</th>
                <th>内容</th>
              </tr>
            </thead>
            <tbody>
              {currentData.items.length === 0 && (
                <tr>
                  <td colSpan={5} className="muted center">
                    {loading ? "加载中…" : "所选条件下暂无日志"}
                  </td>
                </tr>
              )}
              {currentData.items.map((e) => (
                <tr key={e.id}>
                  <td className="mono nowrap">{formatTs(e.ts)}</td>
                  <td>
                    <span className={`badge level-${e.level}`}>{levelLabels[e.level]}</span>
                  </td>
                  <td className="nowrap">{categoryLabels[e.category] ?? e.category}</td>
                  <td className="nowrap">
                    {e.pointId ? (
                      <button className="link" onClick={() => { setPointId(e.pointId!); resetPage(); }}>
                        {e.pointName ?? e.pointId}
                      </button>
                    ) : (
                      <span className="muted">—</span>
                    )}
                  </td>
                  <td className="msg">{e.message}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
