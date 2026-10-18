import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { ChevronLeft, ChevronRight, Download, History, RefreshCw, Scale } from "lucide-react";
import { subscribe } from "../features/plc";
import { useRecipes } from "../features/cycle";
import { errorText } from "../lib/errors";
import SelectionTrack from "../components/SelectionTrack";
import { displayReason, formatTime, historyApi, RejudgeDialog, sampleLabelText, sourceLabel, triggerModeLabel, verdictClass, verdictGroups, verdictLabel, type HistoryPage as Page, type HistoryQuery, type HistoryScope } from "../features/history";

const PAGE = 50;
const scopes: [HistoryScope, string][] = [
  ["real", "真机件"],
  ["offline", "离线件"],
  ["debug", "候选调试"],
];
const ranges: [string, string, number | null][] = [
  ["today", "今天", 0],
  ["7d", "7 天", 7],
  ["30d", "30 天", 30],
  ["all", "全部", null],
];

function rowTone(verdict: Page["items"][number]["verdict"]) {
  return verdict === "OK" ? "" : verdict === "OK_WITH_EXCURSION" ? "r-exc" : verdict === "ERR_INSPECT" ? "r-err" : "r-ng";
}

function rangeStart(days: number | null) {
  if (days === null) return null;
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime() - days * 86_400_000;
}

export default function HistoryPage() {
  const navigate = useNavigate();
  const recipes = useRecipes();
  const [scope, setScope] = useState<HistoryScope>("real");
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
      scope,
      from: rangeStart(ranges.find((r) => r[0] === range)![2]),
      verdicts: verdictGroups.filter((g) => groups.includes(g.key)).flatMap((g) => g.verdicts),
      sn,
      recipeId: recipeId || null,
    }),
    [scope, range, groups, sn, recipeId],
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
      .catch((e) => { if (serial === requestSerial.current) setError(errorText(e)); })
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
      if (alive.current && currentQuery.current === scope) setOperationError(errorText(e));
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
    catch (e) { if (alive.current && currentQuery.current === scope) setOperationError(errorText(e)); }
    finally {
      revealPending.current = false;
      if (alive.current) setRevealing(false);
    }
  };

  const c = page?.counts;
  return (
    <div className="hist-page">
      <div className="panel hist-filters">
        <div className="filter-row">
          <SelectionTrack className="segmented" activeKey={scope} role="group" aria-label="记录来源">
            {scopes.map(([key, label]) => (
              <button key={key} aria-pressed={scope === key} className={scope === key ? "active" : ""} onClick={() => { setScope(key); setOffset(0); }}>
                {label}
              </button>
            ))}
          </SelectionTrack>
          <SelectionTrack className="segmented" activeKey={range} role="group" aria-label="记录日期">
            {ranges.map(([key, label]) => (
              <button key={key} aria-pressed={range === key} className={range === key ? "active" : ""} onClick={() => { setRange(key); setOffset(0); }}>
                {label}
              </button>
            ))}
          </SelectionTrack>
          <span className="filter-label">结果</span>
          {verdictGroups.map((g) => (
            <button
              key={g.key}
              aria-pressed={groups.includes(g.key)}
              className={`qs-chip action hist-chip g-${g.key}${groups.includes(g.key) ? " selected" : " outlined"}`}
              onClick={() => { setGroups(groups.includes(g.key) ? groups.filter((x) => x !== g.key) : [...groups, g.key]); setOffset(0); }}
            >
              <span className="hist-dot" aria-hidden="true" />
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
        {scope === "debug" && <div className="qs-banner tertiary">候选配方离线调试结果，未发布，不作为生产验收或生产统计。</div>}
        {scope === "offline" && <div className="qs-banner tertiary">离线件来自离线验证（合成图或样本集），与真机件分开统计，不计入真机产量与良率。</div>}
        <div className="hist-counts">
          <span>{scope === "debug" ? "候选调试" : scope === "offline" ? "离线" : "真机"}共 <b>{page?.total ?? 0}</b> 件</span>
          <span>OK <b className="c-ok">{c?.ok ?? 0}</b></span>
          <span>局部超差 <b className="c-warn">{c?.excursion ?? 0}</b></span>
          <span>NG <b className="c-ng">{c?.ng ?? 0}</b></span>
          <span>ERR <b className="c-err">{c?.err ?? 0}</b></span>
          {page && page.total > 0 && <span>良率 <b>{(((c!.ok + c!.excursion) / page.total) * 100).toFixed(1)}%</b></span>}
          <span className="hist-loading muted" role="status">{loading ? "正在加载历史记录…" : ""}</span>
        </div>
        {exported && (
          <div className="qs-banner ok">
            <span>已导出：<span className="mono">{exported}</span>{" "}
            <button className="link" disabled={revealing} onClick={() => void reveal()}>{revealing ? "正在打开…" : "打开所在文件夹"}</button></span>
          </div>
        )}
        {error && <div className="qs-banner ng" role="alert">{error}</div>}
        {operationError && <div className="qs-banner ng" role="alert">{operationError}</div>}
      </div>

      <div className="panel hist-results">
        <div className="table-wrap">
          <table className={`table hist-table${loading ? " is-loading" : ""}`} aria-busy={loading}>
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
                <tr key={p.id} className={rowTone(p.verdict)} tabIndex={0} aria-label={`查看 SN ${p.sn} 的记录`} onClick={() => navigate(`/history/${p.id}`)} onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") { e.preventDefault(); navigate(`/history/${p.id}`); }
                }}>
                  <td className="mono nowrap">{formatTime(p.ts)}</td>
                  <td className="mono nowrap">
                    {p.sn || "—"}
                    {p.retestOf && <span className="tag retest" title={`复检，上一次记录 #${p.retestOf}`}>复检</span>}
                    {p.source !== "real" && <span className="tag offline">{sourceLabel[p.source]}</span>}
                    {p.sample && <span className="tag subtle" title={`样本集 ${p.sample.setId} · 样本 SN ${p.sample.sn}`}>样本 {p.sample.partId}（{sampleLabelText[p.sample.label]}）</span>}
                  </td>
                  <td className="nowrap">
                    {p.recipeId ?? "—"}
                    {p.source === "offlineCandidate"
                      ? <span className="muted"> · 候选修订 {p.recipeRevision ?? "未记录"}</span>
                      : p.recipeVersion != null && <span className="muted"> v{p.recipeVersion}</span>}
                    {p.triggerMode && <span className="muted"> · {triggerModeLabel(p.triggerMode)}</span>}
                  </td>
                  <td className="nowrap"><span className={`vt ${verdictClass(p.verdict)}`}>{verdictLabel[p.verdict]}</span></td>
                  <td className="mono">{p.plcCode}{p.faultCode ? ` / ${p.faultCode}` : ""}</td>
                  <td className="reason">{displayReason(p.reason)}</td>
                  <td className="mono nowrap">{p.framesExpected ? `${p.framesReceived}/${p.framesExpected}` : "—"}</td>
                  <td className="mono nowrap">{p.drainMs != null ? `${p.drainMs} ms` : "—"}</td>
                </tr>
              ))}
              {page && page.items.length === 0 && (
                <tr className="hist-empty">
                  <td colSpan={8}>
                    <div className="qs-empty">
                      <History />
                      <b>没有符合条件的记录</b>
                      <small>调整来源、日期范围、结果或配方筛选后再查。</small>
                    </div>
                  </td>
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
