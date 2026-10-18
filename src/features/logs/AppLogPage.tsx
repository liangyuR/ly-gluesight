import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { ChevronLeft, ChevronRight, FolderOpen, PackageOpen, Radio, RefreshCw, Search, X } from "lucide-react";
import { subscribe } from "../plc";
import { formatTs, fromLocalInput, toLocalInput } from "../plc/time";
import { openLogDir } from "../../lib/api";
import { errorText } from "../../lib/errors";
import { formatBytes, levelText, logsApi } from "./api";
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
const levels: AppLogLevel[] = ["error", "warn", "info", "debug"];
const DEFAULT_LEVELS: AppLogLevel[] = ["error", "warn", "info"];
const badgeLevel = { error: "error", warn: "warn", info: "info", debug: "debug" } as const;

function Cycle({ record, onFilter }: { record: AppLogRecord; onFilter: (cycleId: string) => void }) {
  const [target, setTarget] = useState<number | null | undefined>(undefined);
  if (!record.cycleId) return record.sn ? <span className="mono">SN {record.sn}</span> : <span className="muted">—</span>;
  const cycleId = record.cycleId;
  return <span className="log-cycle">
    <button className="link mono" title={`只看工件 ${cycleId}`} onClick={() => onFilter(cycleId)}>{cycleId.slice(0, 8)}</button>
    {record.sn && <span className="mono muted">SN {record.sn}</span>}
    {target === undefined ? <button className="link" onClick={() => void logsApi.findCycle(cycleId).then(setTarget).catch(() => setTarget(null))}>记录</button>
      : target === null ? <span className="muted">无记录</span> : <Link to={`/history/${target}`}>#{target}</Link>}
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

  const following = live && rangeKey !== "custom" && page === 0;
  useEffect(() => {
    if (!following) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const stop = subscribe("cycle://log", () => {
      if (timer) return;
      timer = setTimeout(() => { timer = null; void runRef.current(); }, 1000);
    });
    return () => { stop(); if (timer) clearTimeout(timer); };
  }, [following]);

  const current = data?.key === key ? data.page : null;
  const pageCount = Math.max(1, Math.ceil((current?.total ?? 0) / PAGE_SIZE));
  const reset = () => setPage(0);
  const setCycle = (id: string) => { const next = new URLSearchParams(params); if (id) next.set("cycleId", id); else next.delete("cycleId"); setParams(next); reset(); };
  const toggle = (level: AppLogLevel) => { setChosen(chosen.includes(level) ? chosen.filter(l => l !== level) : [...chosen, level]); reset(); };
  const span = useMemo(() => {
    try { const q = query(); return { from: q.from ?? Date.now() - 3_600_000, to: q.to ?? Date.now() }; } catch { return { from: Date.now() - 3_600_000, to: Date.now() }; }
  }, [query, exporting]);

  return <div className="stack">
    <div className="panel filters">
      <div className="filter-row">
        <div className="segmented" role="group" aria-label="日志时间范围">
          {ranges.map(r => <button key={r.key} aria-pressed={rangeKey === r.key} className={rangeKey === r.key ? "active" : ""} onClick={() => { setRangeKey(r.key); reset(); }}>{r.label}</button>)}
        </div>
        {rangeKey === "custom" && <div className="row">
          <input className="input mono" aria-label="开始时间" type="datetime-local" step={1} value={customStart} onChange={e => { setCustomStart(e.target.value); reset(); }} />
          <span className="muted">至</span>
          <input className="input mono" aria-label="结束时间" type="datetime-local" step={1} value={customEnd} onChange={e => { setCustomEnd(e.target.value); reset(); }} />
        </div>}
        <div className="spacer" />
        <button className={`btn ${following ? "live" : ""}`} disabled={rangeKey === "custom"} onClick={() => { setLive(v => !v); reset(); }}><Radio size={16} />{following ? "实时跟随中" : "实时跟随"}</button>
        <button className="btn" onClick={() => void run()}><RefreshCw size={16} className={loading ? "spin" : ""} />刷新</button>
        <button className="btn primary" onClick={() => setExporting(true)}><PackageOpen size={16} />导出诊断包</button>
      </div>
      <div className="filter-row">
        <span className="filter-label">级别</span>
        {levels.map(l => <button key={l} aria-pressed={chosen.includes(l)} className={`chip level-${l} ${chosen.includes(l) ? "on" : ""}`} onClick={() => toggle(l)}>{levelText[l]}</button>)}
        <span className="filter-label">SN</span>
        <input className="input mono" aria-label="按 SN 筛选" placeholder="SN" value={sn} style={{ width: 150 }} onChange={e => { setSn(e.target.value); reset(); }} />
        <div className="search">
          <Search size={16} />
          <input className="input" aria-label="搜索日志" placeholder="搜索事件、内容、cycleId 或拍照点，回车确认" value={keywordInput} onChange={e => setKeywordInput(e.target.value)}
            onKeyDown={e => { if (e.key === "Enter") { setKeyword(keywordInput.trim()); reset(); } }} />
          {keywordInput && <button className="icon-btn" aria-label="清空搜索" onClick={() => { setKeywordInput(""); setKeyword(""); reset(); }}><X size={14} /></button>}
        </div>
        {cycleId && <span className="tag">工件 <span className="mono">{cycleId}</span> <button className="icon-btn" aria-label="取消工件筛选" onClick={() => setCycle("")}><X size={12} /></button></span>}
      </div>
      <div className="filter-row muted log-usage">
        {usage ? <>日志占用 {formatBytes(usage.bytes)} / 上限 {formatBytes(usage.maxTotalBytes)} · {usage.files} 个文件 · 保留 {usage.keepDays} 天 · 单文件 {formatBytes(usage.segmentBytes)} 滚动</> : "日志占用读取中…"}
        <button className="link" onClick={() => void openLogDir().catch(e => setError(errorText(e)))}><FolderOpen size={13} /> 打开日志目录</button>
      </div>
    </div>
    <div className="panel">
      <div className="panel-toolbar">
        <div className="row"><h3 className="panel-title">运行日志</h3><span className="muted">共 {current?.total ?? 0} 条</span>{error && <span className="ng" role="alert">{error}</span>}</div>
        <div className="row">
          <button className="icon-btn" aria-label="上一页" disabled={loading || page === 0} onClick={() => setPage(page - 1)}><ChevronLeft size={18} /></button>
          <span className="mono muted">{page + 1} / {pageCount}</span>
          <button className="icon-btn" aria-label="下一页" disabled={loading || page + 1 >= pageCount} onClick={() => setPage(page + 1)}><ChevronRight size={18} /></button>
        </div>
      </div>
      <div className="table-wrap">
        <table className="table log-table" aria-label="运行日志">
          <thead><tr><th>时间</th><th>级别</th><th>事件</th><th>工件</th><th>拍照点</th><th>内容</th></tr></thead>
          <tbody>
            {!current?.items.length && <tr><td colSpan={6} className="muted center">{loading ? "加载中…" : "所选条件下暂无日志"}</td></tr>}
            {current?.items.map((r, i) => <tr key={`${r.ts}-${i}`} className={r.level === "error" ? "log-row-error" : undefined}>
              <td className="mono nowrap">{formatTs(r.ts)}</td>
              <td><span className={`badge level-${badgeLevel[r.level]}`}>{levelText[r.level]}</span></td>
              <td className="nowrap">{r.ev}</td>
              <td className="nowrap"><Cycle record={r} onFilter={setCycle} /></td>
              <td className="nowrap mono">{r.k != null ? `k${r.k + 1}` : ""}{r.shotId ? ` · ${r.shotId}` : ""}{r.camera ? ` · ${r.camera}` : ""}</td>
              <td className="msg">{r.msg}{r.data != null && <details className="log-data"><summary>结构化字段</summary><pre className="mono">{JSON.stringify(r.data, null, 2)}</pre></details>}</td>
            </tr>)}
          </tbody>
        </table>
      </div>
    </div>
    {exporting && <DiagnosticsDialog from={span.from} to={span.to} onClose={() => setExporting(false)} />}
  </div>;
}
