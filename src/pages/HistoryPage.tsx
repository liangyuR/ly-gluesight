import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { ChevronLeft, ChevronRight, Download, RefreshCw, Scale } from "lucide-react";
import { subscribe } from "../features/plc";
import { useRecipes } from "../features/cycle";
import { displayReason, formatTime, historyApi, RejudgeDialog, triggerModeLabel, verdictClass, verdictGroups, verdictLabel, type HistoryPage as Page, type HistoryQuery } from "../features/history";

const PAGE = 50;
const ranges: [string, string, number | null][] = [
  ["today", "今天", 0],
  ["7d", "7 天", 7],
  ["30d", "30 天", 30],
  ["all", "全部", null],
];

function rangeStart(days: number | null) {
  if (days === null) return null;
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime() - days * 86_400_000;
}

export default function HistoryPage() {
  const navigate = useNavigate();
  const recipes = useRecipes();
  const [range, setRange] = useState("today");
  const [groups, setGroups] = useState<string[]>([]);
  const [sn, setSn] = useState("");
  const [recipeId, setRecipeId] = useState("");
  const [offset, setOffset] = useState(0);
  const [result, setResult] = useState<{ page: Page; query: HistoryQuery; offset: number } | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [operationError, setOperationError] = useState("");
  const [exported, setExported] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [revealing, setRevealing] = useState(false);
  const [rejudge, setRejudge] = useState<{ query: HistoryQuery; total: number } | null>(null);
  const requestSerial = useRef(0);
  const exportPending = useRef(false);
  const revealPending = useRef(false);
  const alive = useRef(true);

  const query = useMemo<HistoryQuery>(
    () => ({
      from: rangeStart(ranges.find((r) => r[0] === range)![2]),
      verdicts: verdictGroups.filter((g) => groups.includes(g.key)).flatMap((g) => g.verdicts),
      sn,
      recipeId: recipeId || null,
    }),
    [range, groups, sn, recipeId],
  );
  const currentQuery = useRef(query);
  currentQuery.current = query;
  const page = result?.query === query && result.offset === offset ? result.page : null;

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  useEffect(() => {
    setExported(null);
    setOperationError("");
    setRejudge(null);
  }, [query]);

  const load = useCallback(() => {
    const serial = ++requestSerial.current;
    setLoading(true);
    setError("");
    historyApi
      .query({ ...query, offset, limit: PAGE })
      .then((p) => {
        if (serial !== requestSerial.current) return;
        setResult({ page: p, query, offset });
        setError("");
      })
      .catch((e) => { if (serial === requestSerial.current) setError(String(e)); })
      .finally(() => { if (serial === requestSerial.current) setLoading(false); });
  }, [query, offset]);

  useEffect(() => { load(); return () => { requestSerial.current++; }; }, [load]);
  useEffect(() => subscribe<number>("history://inserted", () => offset === 0 && load()), [load, offset]);

  const exportCsv = async () => {
    if (exportPending.current || loading || !page?.total) return;
    exportPending.current = true;
    setExporting(true); setExported(null); setOperationError("");
    const scope = query;
    try {
      const path = await historyApi.exportCsv(scope);
      if (alive.current && currentQuery.current === scope) setExported(path);
    } catch (e) {
      if (alive.current && currentQuery.current === scope) setOperationError(String(e));
    } finally {
      exportPending.current = false;
      if (alive.current) setExporting(false);
    }
  };

  const reveal = async () => {
    if (!exported || revealPending.current) return;
    revealPending.current = true; setRevealing(true); setOperationError("");
    const scope = query;
    try { await historyApi.reveal(exported); }
    catch (e) { if (alive.current && currentQuery.current === scope) setOperationError(String(e)); }
    finally {
      revealPending.current = false;
      if (alive.current) setRevealing(false);
    }
  };

  const c = page?.counts;
  return (
    <div className="stack">
      <div className="panel">
        <div className="filter-row">
          <div className="segmented">
            {ranges.map(([key, label]) => (
              <button key={key} aria-pressed={range === key} className={range === key ? "active" : ""} onClick={() => { setRange(key); setOffset(0); }}>
                {label}
              </button>
            ))}
          </div>
          <span className="filter-label">结果</span>
          {verdictGroups.map((g) => (
            <button
              key={g.key}
              aria-pressed={groups.includes(g.key)}
              className={`chip${groups.includes(g.key) ? " on" : ""}`}
              onClick={() => { setGroups(groups.includes(g.key) ? groups.filter((x) => x !== g.key) : [...groups, g.key]); setOffset(0); }}
            >
              {g.label}
            </button>
          ))}
          <input id="history-sn" aria-label="历史 SN" className="input" placeholder="SN" value={sn} onChange={(e) => { setSn(e.target.value); setOffset(0); }} style={{ width: 140 }} />
          <select id="history-recipe" aria-label="历史配方" className="input" value={recipeId} onChange={(e) => { setRecipeId(e.target.value); setOffset(0); }}>
            <option value="">全部配方</option>
            {recipes.map((r) => (
              <option key={r.id} value={r.id}>{r.id}</option>
            ))}
          </select>
          <span className="spacer" />
          <button className="icon-btn" onClick={load} disabled={loading} title="刷新"><RefreshCw size={16} /></button>
          <button className="btn" onClick={() => page && setRejudge({ query, total: page.total })} disabled={loading || !page?.total}>
            <Scale size={15} />
            批量重判
          </button>
          <button className="btn" onClick={() => void exportCsv()} disabled={exporting || loading || !page?.total}>
            <Download size={15} />
            {exporting ? "导出中…" : "导出 CSV"}
          </button>
        </div>
        <div className="hist-counts">
          <span>共 <b>{page?.total ?? 0}</b> 件</span>
          <span>OK <b className="c-ok">{c?.ok ?? 0}</b></span>
          <span>局部超差 <b className="c-warn">{c?.excursion ?? 0}</b></span>
          <span>NG <b className="c-ng">{c?.ng ?? 0}</b></span>
          <span>ERR <b className="c-err">{c?.err ?? 0}</b></span>
          {page && page.total > 0 && <span>良率 <b>{(((c!.ok + c!.excursion) / page.total) * 100).toFixed(1)}%</b></span>}
        </div>
        {exported && (
          <div className="notice ok">
            已导出：<span className="mono">{exported}</span>{" "}
            <button className="link" disabled={revealing} onClick={() => void reveal()}>{revealing ? "正在打开…" : "打开所在文件夹"}</button>
          </div>
        )}
        {error && <div className="notice error">{error}</div>}
        {operationError && <div className="notice error">{operationError}</div>}
        {loading && <p role="status" className="muted">正在加载历史记录…</p>}
      </div>

      <div className="panel">
        <div className="table-wrap">
          <table className="table hist-table">
            <thead>
              <tr>
                <th>时间</th>
                <th>SN</th>
                <th>配方</th>
                <th>结果</th>
                <th>PLC 码</th>
                <th>原因</th>
                <th>帧</th>
                <th>收尾</th>
              </tr>
            </thead>
            <tbody>
              {page?.items.map((p) => (
                <tr key={p.id} tabIndex={0} aria-label={`查看 SN ${p.sn} 的记录`} onClick={() => navigate(`/history/${p.id}`)} onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") { e.preventDefault(); navigate(`/history/${p.id}`); }
                }}>
                  <td className="mono nowrap">{formatTime(p.ts)}</td>
                  <td className="mono nowrap">
                    {p.sn}
                    {p.retestOf && <span className="tag retest" title={`复检，上一次记录 #${p.retestOf}`}>复检</span>}
                  </td>
                  <td className="nowrap">
                    {p.recipeId ?? "—"}
                    {p.recipeVersion != null && <span className="muted"> v{p.recipeVersion}</span>}
                    {p.triggerMode && <span className="muted"> · {triggerModeLabel(p.triggerMode)}</span>}
                  </td>
                  <td className="nowrap"><span className={`vt ${verdictClass(p.verdict)}`}>{verdictLabel[p.verdict]}</span></td>
                  <td className="mono">{p.plcCode}{p.faultCode ? ` / ${p.faultCode}` : ""}</td>
                  <td className="reason">{displayReason(p.reason)}</td>
                  <td className="mono nowrap">{p.triggerMode === "follow" ? `收 ${p.framesReceived}` : p.framesExpected ? `${p.framesReceived}/${p.framesExpected}` : "—"}</td>
                  <td className="mono nowrap">{p.drainMs != null ? `${p.drainMs} ms` : "—"}</td>
                </tr>
              ))}
              {page && page.items.length === 0 && (
                <tr>
                  <td colSpan={8} className="muted center">没有符合条件的记录</td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
        {page && page.total > PAGE && (
          <div className="pager">
            <button className="icon-btn" aria-label="上一页" disabled={loading || offset === 0} onClick={() => setOffset(Math.max(0, offset - PAGE))}><ChevronLeft size={16} /></button>
            <span className="muted">{offset + 1}–{Math.min(offset + PAGE, page.total)} / {page.total}</span>
            <button className="icon-btn" aria-label="下一页" disabled={loading || offset + PAGE >= page.total} onClick={() => setOffset(offset + PAGE)}><ChevronRight size={16} /></button>
          </div>
        )}
      </div>
      {rejudge && <RejudgeDialog query={rejudge.query} total={rejudge.total} onClose={() => setRejudge(null)} />}
    </div>
  );
}
