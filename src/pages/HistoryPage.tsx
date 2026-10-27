import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { Download, History, RefreshCw, Scale } from "lucide-react";
import { Alert, Button, Card, Chip, EmptyState, IconButton, Pagination, SegmentedControl, Select, Stack, StatusBadge, Table, Text, TextField, type TableColumn } from "qing-steel";
import { subscribe } from "../features/plc";
import { useRecipes } from "../features/cycle";
import { errorText } from "../lib/errors";
import { flagBadges } from "../features/plan/vis";
import { displayReason, formatTime, historyApi, partVerdict, RejudgeDialog, sampleLabelText, sourceLabel, triggerModeLabel, verdictGroups, type HistoryFilter, type HistoryPage as Page, type HistoryQuery, type HistoryScope, type PartMode } from "../features/history";

type Item = Page["items"][number];

const PAGE = 50;
const scopeOptions: { value: HistoryScope; label: string }[] = [
  { value: "real", label: "真机件" },
  { value: "offline", label: "离线件" },
  { value: "debug", label: "候选调试" },
];
const ranges: [string, string, number | null][] = [
  ["today", "今天", 0],
  ["7d", "7 天", 7],
  ["30d", "30 天", 30],
  ["all", "全部", null],
];
const rangeOptions = ranges.map(([value, label]) => ({ value, label }));
const modeOptions: { value: string; label: string; modes: PartMode[] }[] = [
  { value: "all", label: "全部", modes: [] },
  { value: "production", label: "正式检测", modes: ["production"] },
  { value: "maintenanceShadow", label: "维护试判", modes: ["maintenanceShadow"] },
  { value: "maintenanceCollect", label: "维护采集", modes: ["maintenanceCollect"] },
];

function rangeStart(days: number | null) {
  if (days === null) return null;
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime() - days * 86_400_000;
}

export default function HistoryPage() {
  const go = useNavigate();
  const recipes = useRecipes();
  const location = useLocation();
  const [initial] = useState(() => (location.state as { filter?: HistoryFilter } | null)?.filter ?? null);
  const [scope, setScope] = useState<HistoryScope>("real");
  const [range, setRange] = useState(initial ? "all" : "today");
  const [groups, setGroups] = useState<string[]>(initial?.groups ?? []);
  const [mode, setMode] = useState(modeOptions.find((m) => m.modes.join() === (initial?.modes ?? []).join())?.value ?? "all");
  const [sn, setSn] = useState("");
  const [recipeId, setRecipeId] = useState(initial?.recipeId ?? "");
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
      modes: modeOptions.find((m) => m.value === mode)!.modes,
    }),
    [scope, range, groups, sn, recipeId, mode],
  );
  const currentQuery = useRef(query);
  currentQuery.current = query;
  const stale = result?.query !== query || result.offset !== offset;
  const page = result && !(stale && error) ? result.page : null;
  const pending = stale && !!page;
  const shownScope = page ? result!.query.scope : scope;
  const navigate = (to: string) => go(to, { state: { query: result?.query ?? query } });

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

  const openRow = (e: MouseEvent<HTMLElement>) => {
    const target = e.target as HTMLElement;
    if (target.closest("button, a")) return;
    const id = target.closest("tbody tr")?.querySelector<HTMLElement>("[data-history-id]")?.dataset.historyId;
    if (id) navigate(`/history/${id}`);
  };

  const columns: TableColumn<Item>[] = [
    {
      key: "ts",
      header: "时间",
      render: (p) => (
        <Button variant="text" size="sm" className="hist-open mono" data-history-id={p.id} data-tone={partVerdict(p).tone} aria-label={`查看 SN ${p.sn} 的记录`} onClick={() => navigate(`/history/${p.id}`)}>
          {formatTime(p.ts)}
        </Button>
      ),
    },
    {
      key: "sn",
      header: "SN",
      render: (p) => (
        <div className="hist-sn">
          <span className="sn" title={p.sn || undefined}>{p.sn || "—"}</span>
          {p.retestOf && <StatusBadge tone="warn" title={`复检，上一次记录 #${p.retestOf}`}>复检</StatusBadge>}
          {p.source !== "real" && <StatusBadge tone="off">{sourceLabel[p.source]}</StatusBadge>}
          {p.sample && <span title={`样本集 ${p.sample.setId} · 样本 SN ${p.sample.sn}`}><Chip>样本 {p.sample.partId}（{sampleLabelText[p.sample.label]}）</Chip></span>}
        </div>
      ),
    },
    {
      key: "recipe",
      header: "配方",
      render: (p) => (
        <span className="nowrap">
          {p.recipeId ?? "—"}
          {p.source === "offlineCandidate"
            ? <span className="muted"> · 候选修订 {p.recipeRevision ?? "未记录"}</span>
            : p.recipeVersion != null && <span className="muted"> v{p.recipeVersion}</span>}
          {p.triggerMode && <span className="muted"> · {triggerModeLabel(p.triggerMode)}</span>}
        </span>
      ),
    },
    { key: "verdict", header: "结果", render: (p) => { const v = partVerdict(p); return <StatusBadge tone={v.tone}>{v.label}</StatusBadge>; } },
    { key: "plc", header: "PLC 码", render: (p) => <span className="mono">{p.plcCode}{p.faultCode ? ` / ${p.faultCode}` : ""}</span> },
    { key: "reason", header: "原因", render: (p) => <span className="hist-reason">{displayReason(p.reason)}</span> },
    {
      key: "frames",
      header: "帧",
      render: (p) => {
        // 帧组数 / 预期总数 ΣN_exp（待标定为 0），整件标记（检测不完整、帧数异常、疑似提前终止…）单独显示；没有帧记录的为 收到/计划
        if (!p.flags) return <span className="mono nowrap">{p.framesExpected ? `${p.framesReceived}/${p.framesExpected}` : "—"}</span>;
        const badges = flagBadges(p.flags);
        return <div className="hist-frames">
          <span className="mono nowrap" title="帧组数 / 预期总数 ΣN_exp">{p.framesReceived}/{p.framesExpected || "—"}</span>
          {badges.map((b) => <StatusBadge key={b.key} tone={b.tone} data-flag={b.key}>{b.label}</StatusBadge>)}
        </div>;
      },
    },
    { key: "drain", header: "收尾", render: (p) => <span className="mono nowrap">{p.drainMs != null ? `${p.drainMs} ms` : "—"}</span> },
  ];

  const c = page?.counts;
  return (
    <div className="hist-page">
      <Card className="hist-filters">
        <Stack direction="column" align="stretch" gap={10}>
          <Stack justify="space-between" gap={12}>
            <Stack gap={12}>
              <SegmentedControl label="记录来源" options={scopeOptions} value={scope} onValueChange={(value) => { setScope(value as HistoryScope); setOffset(0); }} />
              <SegmentedControl label="记录日期" options={rangeOptions} value={range} onValueChange={(value) => { setRange(value); setOffset(0); }} />
              <SegmentedControl label="检测模式" options={modeOptions} value={mode} onValueChange={(value) => { setMode(value); setOffset(0); }} />
              <Stack gap={8}>
                <Text variant="label" muted>结果</Text>
                {verdictGroups.map((g) => (
                  <Chip
                    key={g.key}
                    selected={groups.includes(g.key)}
                    className={`hist-chip g-${g.key}`}
                    onClick={() => { setGroups(groups.includes(g.key) ? groups.filter((x) => x !== g.key) : [...groups, g.key]); setOffset(0); }}
                  >
                    <span className="hist-dot" aria-hidden="true" />
                    {g.label}
                  </Chip>
                ))}
              </Stack>
              <TextField id="history-sn" aria-label="历史 SN" className="hist-sn-input" placeholder="SN" value={sn} onChange={(e) => { setSn(e.target.value); setOffset(0); }} />
              <Select
                id="history-recipe"
                aria-label="历史配方"
                className="hist-recipe"
                value={recipeId}
                options={[{ value: "", label: "全部配方" }, ...recipes.map((r) => ({ value: r.id, label: r.id }))]}
                onValueChange={(value) => { setRecipeId(value); setOffset(0); }}
              />
            </Stack>
            <Stack gap={8}>
              <IconButton aria-label="刷新" title="刷新" onClick={load} disabled={loading}><RefreshCw size={16} /></IconButton>
              <Button leadingIcon={<Scale size={15} />} onClick={() => page && setRejudge({ query, total: page.total })} disabled={loading || stale || !page?.total}>批量重判</Button>
              <Button leadingIcon={<Download size={15} />} onClick={() => void exportCsv()} disabled={exporting || loading || stale || !page?.total}>
                {exporting ? "导出中…" : "导出 CSV"}
              </Button>
            </Stack>
          </Stack>
          {scope === "debug" && <Alert>候选配方离线调试结果，未发布，不作为生产验收或生产统计。</Alert>}
          {scope === "offline" && <Alert>离线件来自离线验证（合成图或样本集），与真机件分开统计，不计入真机产量与良率。</Alert>}
          <div className={`hist-counts${pending ? " is-loading" : ""}`} aria-busy={pending}>
            <span>{shownScope === "debug" ? "候选调试" : shownScope === "offline" ? "离线" : "真机"}共 <b>{page?.total ?? 0}</b> 件</span>
            <span>OK <b className="c-ok">{c?.ok ?? 0}</b></span>
            <span>局部超差 <b className="c-warn">{c?.excursion ?? 0}</b></span>
            <span>NG <b className="c-ng">{c?.ng ?? 0}</b></span>
            <span>ERR <b className="c-err">{c?.err ?? 0}</b></span>
            {page && page.total > 0 && <span>良率 <b>{(((c!.ok + c!.excursion) / page.total) * 100).toFixed(1)}%</b></span>}
            <span className="hist-loading muted" role="status">{loading && stale ? "正在加载历史记录…" : ""}</span>
          </div>
          {exported && (
            <Alert tone="ok" action={<Button variant="text" size="sm" disabled={revealing} onClick={() => void reveal()}>{revealing ? "正在打开…" : "打开所在文件夹"}</Button>}>
              已导出：<span className="mono">{exported}</span>
            </Alert>
          )}
          {error && <Alert tone="ng">{error}</Alert>}
          {operationError && <Alert tone="ng">{operationError}</Alert>}
        </Stack>
      </Card>

      <Card className="hist-results">
        <div className={`hist-results-body${pending ? " is-loading" : ""}`} aria-busy={loading} onClick={openRow}>
          <Table<Item>
            className="hist-table"
            caption="历史记录"
            rows={page?.items ?? []}
            columns={columns}
            rowKey={(p) => String(p.id)}
            loading={!page && loading}
            emptyState={page ? <EmptyState icon={<History />} title="没有符合条件的记录" description="调整来源、日期范围、结果或配方筛选后再查。" /> : <></>}
          />
        </div>
        {page && page.total > PAGE && (
          <Pagination
            className="hist-pager"
            total={page.total}
            page={offset / PAGE + 1}
            pageSize={PAGE}
            pageSizeOptions={[PAGE]}
            disabled={loading}
            onPageChange={(next) => setOffset((next - 1) * PAGE)}
          />
        )}
      </Card>
      {rejudge && <RejudgeDialog query={rejudge.query} total={rejudge.total} onClose={() => setRejudge(null)} />}
    </div>
  );
}
