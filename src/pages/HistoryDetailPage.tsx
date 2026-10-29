import { useEffect, useState } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, ChevronLeft, ChevronRight } from "lucide-react";
import { Alert, Card, IconButton, Stack, StatusBadge, Text } from "qing-steel";
import { displayReason, formatTime, historyApi, partVerdict, sampleLabelText, sourceLabel, triggerModeLabel, type HistoryQuery, type PartDetail } from "../features/history";
import BurstHistoryDetail from "../features/history/components/BurstDetail";
import { subscribe } from "../features/plc";
import { Panel } from "../features/workspace/components";
import { CycleTimeline } from "../features/logs";
import { errorText } from "../lib/errors";

type Neighbors = { older: number | null; newer: number | null };

export default function HistoryDetailPage() {
  const { id } = useParams(), navigate = useNavigate(), location = useLocation();
  const listQuery = (location.state as { query?: HistoryQuery } | null)?.query ?? null;
  const [neighbors, setNeighbors] = useState<Neighbors>({ older: null, newer: null });
  const [detail, setDetail] = useState<PartDetail | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    let alive = true; setDetail(null); setError("");
    void historyApi.detail(Number(id)).then(d => alive && setDetail(d)).catch(e => alive && setError(errorText(e)));
    return () => { alive = false; };
  }, [id]);
  useEffect(() => {
    let alive = true; setNeighbors({ older: null, newer: null });
    void historyApi.neighbors(Number(id), listQuery).then(v => alive && setNeighbors(v)).catch(() => {});
    return () => { alive = false; };
  }, [id, JSON.stringify(listQuery)]);
  useEffect(() => {
    let alive = true;
    const stop = subscribe<string>("history://updated", cycleId => {
      if (!detail || cycleId !== detail.summary.cycleId) return;
      void historyApi.detail(detail.summary.id).then(next => alive && setDetail(next)).catch(e => alive && setError(errorText(e)));
    });
    return () => { alive = false; stop(); };
  }, [detail?.summary.id, detail?.summary.cycleId]);

  const openPart = (target: number | null) => { if (target != null) navigate("/history/" + target, { state: location.state }); };
  if (!detail) return error ? <Alert tone="ng">{error}</Alert> : <Text muted>加载中…</Text>;
  if (detail.burst) return <BurstHistoryDetail detail={detail} burst={detail.burst} neighbors={neighbors} onOpen={openPart} onBack={() => navigate("/history")} />;
  return <PlainHistoryDetail detail={detail} neighbors={neighbors} onOpen={openPart} onBack={() => navigate("/history")} />;
}

/** 没有帧记录的记录（如工件身份分配失败）：只有整件结论与追溯信息。 */
function PlainHistoryDetail({ detail, neighbors, onOpen, onBack }: { detail: PartDetail; neighbors: Neighbors; onOpen: (target: number | null) => void; onBack: () => void }) {
  const s = detail.summary;
  return <div className="stack hist-detail">
    <Card className="detail-head">
      <Stack gap={12} justify="space-between">
        <Stack gap={12}>
          <IconButton onClick={onBack} aria-label="返回历史列表"><ArrowLeft size={18} /></IconButton>
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
    </Card>
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
