import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, ChevronLeft, ChevronRight } from "lucide-react";
import { Alert, Card, IconButton, Stack, StatusBadge, Text } from "qing-steel";
import { displayReason, formatTime, HistoryRecordActions, historyApi, partVerdict, sampleLabelText, sourceLabel, triggerModeLabel, type HistoryDetailState, type PartDetail } from "../features/history";
import BurstHistoryDetail from "../features/history/components/BurstDetail";
import { subscribe } from "../features/plc";
import { Panel } from "../features/workspace/components";
import { CycleTimeline } from "../features/logs";
import { errorText } from "../lib/errors";
import { useProjectDirectory, type ProjectDirectory } from "../features/project/hooks";
import { recordProject } from "../features/project/rules";

type Neighbors = { older: number | null; newer: number | null };

export default function HistoryDetailPage() {
  const { id } = useParams(), navigate = useNavigate(), location = useLocation();
  const state = location.state as HistoryDetailState | null;
  const listQuery = state?.query ?? null;
  const back = { onBack: () => navigate(state?.returnTo?.path ?? "/history"), backLabel: state?.returnTo?.label ?? "返回历史列表" };
  const [neighbors, setNeighbors] = useState<Neighbors>({ older: null, newer: null });
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
    let alive = true; setNeighbors({ older: null, newer: null });
    void historyApi.neighbors(Number(id), listQuery).then(v => alive && setNeighbors(v)).catch(() => {});
    return () => { alive = false; };
  }, [id, JSON.stringify(listQuery)]);
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

  const openPart = (target: number | null) => { if (target != null) navigate("/history/" + target, { state: { ...state, scrollTop: undefined } }); };
  if (!detail || detail.summary.id !== Number(id)) return error ? <div className="stack"><IconButton onClick={back.onBack} aria-label={back.backLabel}><ArrowLeft size={18} /></IconButton><Alert tone="ng">{error}</Alert></div> : <Text muted>加载中…</Text>;
  const foreignProject = foreignProjectNote(detail.summary.projectId, projects);
  const actions = <HistoryRecordActions key={detail.summary.id} id={detail.summary.id} tags={detail.summary.tags ?? []}
      onBusyChange={setMutationBusy}
      onSaved={tags => { refreshPending.current = false; setDetail(current => current?.summary.id === detail.summary.id ? { ...current, summary: { ...current.summary, tags } } : current); reload(); }}
      onDeleted={result => { refreshPending.current = false; navigate(state?.returnTo?.path ?? "/history", { replace: true, state: { historyNotice: { message: `已删除 ${result.deleted} 件历史记录。`, warnings: result.warnings } } }); }} />;
  if (detail.burst) return <BurstHistoryDetail key={detail.summary.id} detail={detail} burst={detail.burst} neighbors={neighbors} onOpen={openPart} foreignProject={foreignProject} actions={actions} {...back} />;
  return <PlainHistoryDetail detail={detail} neighbors={neighbors} onOpen={openPart} foreignProject={foreignProject} actions={actions} {...back} />;
}

/** 记录不属于当前项目时的提示；当前项目还没读到时不提示。 */
function foreignProjectNote(projectId: string, projects: ProjectDirectory): string {
  if (!projects.current || projects.current.id === projectId) return "";
  const owner = recordProject(projectId, projects.entries);
  return owner.deleted ? `这条记录属于已删除的项目 ${projectId}。` : owner.label === projectId ? `这条记录属于项目 ${projectId}。` : `这条记录属于项目“${owner.label}”（${projectId}）。`;
}

/** 没有帧记录的记录（如工件身份分配失败）：只有整件结论与追溯信息。 */
function PlainHistoryDetail({ detail, neighbors, onOpen, onBack, backLabel, foreignProject, actions }: { detail: PartDetail; neighbors: Neighbors; onOpen: (target: number | null) => void; onBack: () => void; backLabel: string; foreignProject: string; actions: ReactNode }) {
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
          {s.source !== "real" && <StatusBadge tone="off">{sourceLabel[s.source]}</StatusBadge>}
          <Text muted>{formatTime(s.ts)} · {s.recipeId ?? "无配方"} · {"v" + (s.recipeVersion ?? "—") + " · #" + (s.recipeRevision?.slice(0, 6) ?? "—")} · {triggerModeLabel(s.triggerMode)}</Text>
        </Stack>
        <StatusBadge tone={partVerdict(s).tone}>{partVerdict(s).label} · PLC {s.plcCode}{s.faultCode ? " / " + s.faultCode : ""}</StatusBadge>
      </Stack>
      {actions}
    </Card>
    {foreignProject && <Alert title="其他项目的记录">{foreignProject}</Alert>}
    <Alert>本件没有帧记录，只能查看整件结论与事件。</Alert>
    <Panel title="原始整件判定">
      <p className="reason-box">{displayReason(detail.judgement.reason)}</p>
      <div className="kv2">
        <span>记录号</span><b>#{s.id}</b>
        {s.sample && <><span>离线样本</span><b>{s.sample.partId} · SN <span className="sn">{s.sample.sn}</span> · 期望 {sampleLabelText[s.sample.label]}</b></>}
        <span>配方修订</span><b>{s.recipeRevision ?? "—"}</b>
        <span>软件版本</span><b>{detail.softwareVersion}</b>
        <span>复检自</span><b>{s.retestOf ? <Link to={"/history/" + s.retestOf}>#{s.retestOf}</Link> : "—"}</b>
        <span>后续复检</span><b>{detail.retests.length ? detail.retests.map(r => <Link key={r} to={"/history/" + r}>#{r} </Link>) : "—"}</b>
      </div>
    </Panel>
    <CycleTimeline historyId={s.id} cycleId={s.cycleId} ts={s.ts} />
  </div>;
}
