import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, ChevronLeft, ChevronRight } from "lucide-react";
import { Alert, Button, Card, IconButton, Stack, StatusBadge, Text } from "qing-steel";
import { displayReason, formatTime, HistoryRecordActions, historyApi, partVerdict, triggerModeLabel, type HistoryDetailState, type HistoryListState, type PartDetail } from "../features/history";
import BurstHistoryDetail from "../features/history/components/BurstDetail";
import { subscribe } from "../features/plc";
import { Panel } from "../features/workspace/components";
import { errorText } from "../lib/errors";
import { useProjectDirectory, type ProjectDirectory } from "../features/project/hooks";
import { recordProject } from "../features/project/rules";

type Neighbors = { older: number | null; newer: number | null };
type NeighborResult = { locationKey: string; neighbors: Neighbors };
const EMPTY_NEIGHBORS: Neighbors = { older: null, newer: null };

export default function HistoryDetailPage() {
  const { id } = useParams(), navigate = useNavigate(), location = useLocation();
  const state = location.state as HistoryDetailState | null;
  const listQuery = state?.query ?? null;
  const returnPath = state?.returnTo?.path ?? "/history";
  const listState: HistoryListState | undefined = state?.listView ? {
    view: state.listView, focusRecordId: state.listRecordId, selection: state.listSelection,
  } : undefined;
  const returnState = state?.returnTo?.state ?? listState;
  const partState: HistoryDetailState = { ...state, scrollTop: undefined };
  const back = { onBack: () => navigate(returnPath, { state: returnState }), backLabel: state?.returnTo?.label ?? "返回历史列表" };
  const [neighborResult, setNeighborResult] = useState<NeighborResult | null>(null);
  const activeNeighborResult = useRef<NeighborResult | null>(null);
  const neighbors = neighborResult?.locationKey === location.key ? neighborResult.neighbors : EMPTY_NEIGHBORS;
  const [detail, setDetail] = useState<PartDetail | null>(null);
  const [error, setError] = useState("");
  const overviewSerial = useRef(0);
  const mutationBusy = useRef(false);
  const refreshPending = useRef(false);
  const projects = useProjectDirectory();
  const reload = useCallback((preserveDetailOnError = false) => {
    const serial = ++overviewSerial.current;
    void historyApi.overview(Number(id)).then(next => {
      if (serial === overviewSerial.current) { setDetail(next); setError(""); }
    }).catch(cause => {
      if (serial === overviewSerial.current) { if (!preserveDetailOnError) setDetail(null); setError(errorText(cause)); }
    });
  }, [id]);
  const setMutationBusy = useCallback((busy: boolean) => {
    const wasBusy = mutationBusy.current;
    mutationBusy.current = busy;
    if (busy) overviewSerial.current++;
    else if (wasBusy && refreshPending.current) { refreshPending.current = false; reload(true); }
  }, [reload]);

  useEffect(() => {
    setDetail(null); setError(""); reload();
    return () => { overviewSerial.current++; };
  }, [reload]);
  useEffect(() => {
    let alive = true;
    let loading = false;
    let pending = false;
    const recordId = Number(id);
    const locationKey = location.key;
    const refresh = (invalidate = false) => {
      if (!alive) return;
      if (invalidate) {
        activeNeighborResult.current = null;
        setNeighborResult(null);
      }
      if (loading) { pending = true; return; }
      loading = true;
      void historyApi.neighbors(recordId, listQuery).then(neighbors => {
        if (!alive || pending) return;
        const next = { locationKey, neighbors };
        activeNeighborResult.current = next;
        setNeighborResult(next);
      }).catch(() => {}).finally(() => {
        loading = false;
        if (pending) { pending = false; refresh(); }
      });
    };
    const stopInserted = subscribe<number>("history://inserted", () => refresh());
    const stopChanged = subscribe<number[]>("history://changed", ids => {
      const current = activeNeighborResult.current;
      if (!current || current.locationKey !== locationKey || listQuery?.tag?.trim() ||
          ids.some(id => id === recordId || id === current.neighbors.older || id === current.neighbors.newer)) {
        refresh(true);
      }
    });
    refresh(true);
    return () => { alive = false; stopInserted(); stopChanged(); };
  }, [id, listQuery, location.key]);
  useEffect(() => {
    const stop = subscribe<string>("history://updated", cycleId => {
      if (!detail || cycleId !== detail.summary.cycleId) return;
      if (mutationBusy.current) { refreshPending.current = true; return; }
      reload();
    });
    return stop;
  }, [detail?.summary.id, detail?.summary.cycleId, reload]);
  useEffect(() => subscribe<number[]>("history://changed", ids => {
    if (!ids.includes(Number(id))) return;
    if (mutationBusy.current) refreshPending.current = true;
    else reload();
  }), [id, reload]);

  const openPart = (target: number | null) => {
    const current = activeNeighborResult.current;
    if (target != null && current?.locationKey === location.key &&
        (target === current.neighbors.older || target === current.neighbors.newer)) {
      navigate("/history/" + target, { state: partState });
    }
  };
  if (!detail || detail.summary.id !== Number(id)) return <div className="stack hist-detail">
    <div className="wp-actions">
      <Button variant="tonal" size="sm" leadingIcon={<ArrowLeft size={16} aria-hidden="true" />} onClick={back.onBack}>{back.backLabel}</Button>
    </div>
    {error ? <Alert tone="ng" title="历史详情读取失败"
      action={<Button size="sm" onClick={() => { setError(""); reload(); }}>重试</Button>}>{error}</Alert>
      : <Text muted role="status">正在读取历史详情…</Text>}
  </div>;
  const foreignProject = foreignProjectNote(detail.summary.projectId, projects);
  const actions = <HistoryRecordActions key={detail.summary.id} id={detail.summary.id} tags={detail.summary.tags ?? []}
      onBusyChange={setMutationBusy}
      onSaved={tags => { refreshPending.current = false; setDetail(current => current?.summary.id === detail.summary.id ? { ...current, summary: { ...current.summary, tags } } : current); reload(); }}
      onDeleted={result => {
        refreshPending.current = false;
        navigate(returnPath, {
          replace: true,
          state: { ...returnState, rejudgeReview: undefined, historyNotice: { message: `已删除 ${result.deleted} 件历史记录。`, warnings: result.warnings } },
        });
      }} />;
  if (detail.burst) return <BurstHistoryDetail key={detail.summary.id} detail={detail} burst={detail.burst} neighbors={neighbors} onOpen={openPart} partState={partState} foreignProject={foreignProject} actions={actions} {...back} />;
  return <PlainHistoryDetail detail={detail} neighbors={neighbors} onOpen={openPart} partState={partState} foreignProject={foreignProject} actions={actions} {...back} />;
}

/** 记录不属于当前项目时的提示；当前项目还没读到时不提示。 */
function foreignProjectNote(projectId: string, projects: ProjectDirectory): string {
  if (!projects.current || projects.current.id === projectId) return "";
  const owner = recordProject(projectId, projects.entries);
  return owner.deleted ? `这条记录属于已删除的项目 ${projectId}。` : owner.label === projectId ? `这条记录属于项目 ${projectId}。` : `这条记录属于项目“${owner.label}”（${projectId}）。`;
}

/** 没有帧记录的记录（如工件身份分配失败）：只有整件结论与追溯信息。 */
function PlainHistoryDetail({ detail, neighbors, onOpen, partState, onBack, backLabel, foreignProject, actions }: { detail: PartDetail; neighbors: Neighbors; onOpen: (target: number | null) => void; partState: HistoryDetailState; onBack: () => void; backLabel: string; foreignProject: string; actions: ReactNode }) {
  const s = detail.summary;
  return <div className="stack hist-detail">
    <Card className="detail-head">
      <Stack gap={12} justify="space-between">
        <Stack gap={12}>
          <IconButton onClick={onBack} aria-label={backLabel} title={backLabel}><ArrowLeft size={18} /></IconButton>
          <span className="detail-step">
            <IconButton aria-label="前一件" title="前一件" disabled={neighbors.older == null} onClick={() => onOpen(neighbors.older)}><ChevronLeft size={18} /></IconButton>
            <IconButton aria-label="后一件" title="后一件" disabled={neighbors.newer == null} onClick={() => onOpen(neighbors.newer)}><ChevronRight size={18} /></IconButton>
          </span>
          <b className="sn" title={s.sn || undefined}>SN {s.sn || "—"}</b>
          <Text muted>{formatTime(s.ts)} · {s.recipeId ?? "无配方"} · {"v" + (s.recipeVersion ?? "—") + " · #" + (s.recipeRevision?.slice(0, 6) ?? "—")} · {triggerModeLabel(s.triggerMode)}</Text>
        </Stack>
        <StatusBadge tone={partVerdict(s).tone}>{partVerdict(s).label} · PLC {s.plcCode}{s.faultCode ? " / " + s.faultCode : ""}</StatusBadge>
      </Stack>
      {actions}
    </Card>
    {foreignProject && <Alert title="其他项目的记录">{foreignProject}</Alert>}
    <Alert>本件没有帧记录。</Alert>
    <Panel title="原始整件判定">
      <p className="reason-box">{displayReason(detail.judgement.reason)}</p>
      <div className="kv2">
        <span>记录号</span><b>#{s.id}</b>
        <span>配方修订</span><b>{s.recipeRevision ?? "—"}</b>
        <span>软件版本</span><b>{detail.softwareVersion}</b>
        <span>复检自</span><b>{s.retestOf ? <Link to={"/history/" + s.retestOf} state={partState}>#{s.retestOf}</Link> : "—"}</b>
        <span>后续复检</span><b>{detail.retests.length ? detail.retests.map(r => <Link key={r} to={"/history/" + r} state={partState}>#{r} </Link>) : "—"}</b>
      </div>
    </Panel>
  </div>;
}
