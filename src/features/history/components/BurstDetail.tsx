import { useEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { ArrowLeft, ChevronLeft, ChevronRight, Scale, ScanEye } from "lucide-react";
import { Alert, Button, Card, Chip, IconButton, Select, Stack, StatusBadge, Text } from "qing-steel";
import FrameGroupStrip from "../../cycle/components/FrameGroupStrip";
import RegionResultPanel from "../../cycle/components/RegionResultPanel";
import { CameraSegmentOverview } from "../../plan/CameraSegmentOverview";
import type { FrameGroupView, FrameRecord, PartDetailBurst } from "../../plan/types";
import { expectedText, flagBadges, regionKindLabel } from "../../plan/vis";
import { burstImageApi } from "../../workspace/burstImages";
import { workspaceApi } from "../../workspace/api";
import { GrayViewer, KV, Panel } from "../../workspace/components";
import { useWorkspace } from "../../workspace/context";
import type { Comparison, GrayImage } from "../../workspace/types";
import { CycleTimeline } from "../../logs";
import { errorText } from "../../../lib/errors";
import { displayReason, formatTime, sampleLabelText, sourceLabel, triggerModeLabel, verdictLabel, verdictTone } from "../meta";
import { burstSideOf, regionDiffs } from "../replay";
import type { PartDetail } from "../types";
import { BurstPartTrace } from "./PartTrace";
import { RegionReplayDiff } from "./ReplayDiff";

const comparisonLabel = { rules: "规则重判", raw: "候选原图复测", original: "原发布包重现" };
const recordingReason = { off: "录制关闭，本件未保存原图", notRetained: "按录制策略本件未保存原图", pending: "原图仍在落盘", incomplete: "原图不完整", failed: "原图录制失败", complete: "原图已按保留策略清理或无法核验" };
const frameStateName = { measured: "已测", missing: "缺帧", error: "测量失败", unmeasured: "未测", outOfCorridor: "出走廊", notInspected: "本段不检测" };

/** 历史没有逐帧结论：格子按该帧所在区域的结论着色，没有区域的按整件结论；缺帧、测量失败等状态照常显示为灰色 / 紫色。 */
export function historyGroups(burst: PartDetailBurst): FrameGroupView[] {
  const bySeq = new Map<number, Map<number, FrameRecord>>();
  for (const frame of burst.frames) bySeq.set(frame.seq, (bySeq.get(frame.seq) ?? new Map()).set(frame.camera, frame));
  const incomplete = new Set(burst.judgement.regions.flatMap(r => r.incompleteGroups));
  return [...bySeq.keys()].sort((a, b) => a - b).map(seq => {
    const frames = bySeq.get(seq)!;
    const cells = burst.cameras.map((_, c) => {
      const frame = frames.get(c);
      if (!frame) return { status: "missing" as const, verdict: null, frameCounter: null };
      const verdict = frame.status === "measured" ? (frame.region != null ? burst.judgement.regions[frame.region]?.verdict : undefined) ?? burst.judgement.verdict : null;
      return { status: frame.status, verdict, frameCounter: frame.frameCounter };
    });
    return { seq, incomplete: incomplete.has(seq) || cells.some(cell => cell.status !== "measured" && cell.status !== "notInspected"), cells };
  });
}

/** 一帧录制原图：没有原图（缺帧、未录制、录制队满丢弃）时不读，按钮与说明由页面给。 */
function useFrameImage(historyId: number, camera: number, seq: number, enabled: boolean) {
  const key = JSON.stringify([historyId, camera, seq, enabled]);
  const [state, setState] = useState<{ key: string; image: GrayImage | null; error: string; loading: boolean }>({ key, image: null, error: "", loading: false });
  useEffect(() => {
    let alive = true;
    setState({ key, image: null, error: "", loading: enabled });
    if (!enabled) return;
    burstImageApi.record(historyId, camera, seq)
      .then(image => alive && setState({ key, image, error: "", loading: false }))
      .catch(e => alive && setState({ key, image: null, error: errorText(e), loading: false }));
    return () => { alive = false; };
  }, [key]);
  return state.key === key ? state : { key, image: null, error: "", loading: enabled };
}

/**
 * 历史详情（段配方）：按区域展示——整件的段布置与区域结论、帧组条、按 (相机, seq) 选的原图、每相机帧表与时间线。
 * 旧拍照点记录仍由 HistoryDetailPage 自己展示；这里只处理 PartDetail.burst 非空的记录。
 */
export default function BurstHistoryDetail({ detail, burst, neighbors, onOpen, onBack }: {
  detail: PartDetail;
  burst: PartDetailBurst;
  neighbors: { older: number | null; newer: number | null };
  onOpen: (target: number | null) => void;
  onBack: () => void;
}) {
  const ws = useWorkspace();
  const s = detail.summary, j = burst.judgement;
  const groups = useMemo(() => historyGroups(burst), [burst]);
  const lastSeq = groups.at(-1)?.seq ?? 1;
  const [selection, setSelection] = useState({ camera: 0, seq: groups[0]?.seq ?? 1 });
  const [comparison, setComparison] = useState<Comparison | null>(null);
  const [saved, setSaved] = useState<Comparison[]>([]);
  const [error, setError] = useState("");
  const [working, setWorking] = useState(false);
  const pending = useRef(false), serial = useRef(0), alive = useRef(true);
  const scope = `${s.id}:${ws.data?.workspace.doc.id ?? ""}:${ws.data?.workspace.revision ?? ""}`;
  const current = useRef({ scope, historyId: s.id });
  current.current = { scope, historyId: s.id };
  useEffect(() => { alive.current = true; return () => { alive.current = false; serial.current++; }; }, []);
  useEffect(() => { setSelection({ camera: 0, seq: groups[0]?.seq ?? 1 }); setComparison(null); setError(""); }, [s.id]);

  const frame = burst.frames.find(f => f.camera === selection.camera && f.seq === selection.seq) ?? null;
  const cameraName = burst.cameras[selection.camera] ?? `相机 ${selection.camera + 1}`;
  const image = useFrameImage(s.id, selection.camera, selection.seq, !!frame?.rawFile);
  const step = (delta: number) => setSelection(prev => ({ ...prev, seq: Math.min(lastSeq, Math.max(groups[0]?.seq ?? 1, prev.seq + delta)) }));
  const stepRef = useRef(step);
  stepRef.current = step;
  const neighborRef = useRef({ neighbors, onOpen });
  neighborRef.current = { neighbors, onOpen };
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.altKey || e.ctrlKey || e.metaKey || e.defaultPrevented) return;
      if ((e.target as HTMLElement | null)?.closest?.("input,select,textarea,[contenteditable='true'],[role='dialog'],[role='combobox'],[role='listbox']")) return;
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") { e.preventDefault(); stepRef.current(e.key === "ArrowLeft" ? -1 : 1); }
      else if (e.key === "[" || e.key === "]") { e.preventDefault(); neighborRef.current.onOpen(e.key === "[" ? neighborRef.current.neighbors.older : neighborRef.current.neighbors.newer); }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, []);

  useEffect(() => {
    let live = true;
    const mine = ++serial.current;
    setSaved([]);
    if (s.recipeId) void workspaceApi.comparisons(s.recipeId, s.id).then(v => live && mine === serial.current && setSaved(v)).catch(e => live && mine === serial.current && setError(errorText(e)));
    return () => { live = false; };
  }, [s.id, s.recipeId, ws.data?.workspace.revision]);

  // 原图完整：除缺帧外每帧都有录制原图；候选的走廊示教完成（各段检测相机都有走廊、相机有像素当量），才能从原图复测
  const rawComplete = detail.recording.available && burst.frames.every(f => f.status === "missing" || !!f.rawFile);
  const candidateReady = !!ws.data && ws.data.workspace.doc.id === s.recipeId && !ws.dirty && !ws.frameDirty;
  const teaching = ws.data?.plan?.cameraTeaching ?? [];
  const candidateTaught = teaching.length > 0 && teaching.every(c => c.ready);
  const disabled = working || ws.busy || !candidateReady;
  const blockers = s.source === "offlineCandidate" ? [] : [
    !s.bundleId && "本件没有不可变发布包（布防前被拒或非生产记录），无法按原版本重跑",
    !rawComplete && recordingReason[detail.recording.state],
  ].filter((v): v is string => !!v);

  const compare = async (run: () => Promise<Comparison>, guard: () => boolean) => {
    if (pending.current) return;
    pending.current = true; serial.current++; setWorking(true); setError("");
    try { const result = await run(); if (guard()) { setComparison(result); setSaved(v => [result, ...v.filter(c => c.id !== result.id)]); } }
    catch (e) { if (guard()) setError(errorText(e)); }
    finally { pending.current = false; if (alive.current) setWorking(false); }
  };
  const runCompare = (fromRaw: boolean) => {
    if (disabled || !ws.data || (fromRaw && (!rawComplete || !candidateTaught))) return;
    const docId = ws.data.workspace.doc.id, revision = ws.data.workspace.revision;
    void compare(() => workspaceApi.compare(docId, revision, s.id, fromRaw), () => alive.current && current.current.scope === scope);
  };
  const runOriginal = () => {
    if (working || !rawComplete || !s.bundleId) return;
    void compare(() => workspaceApi.compareOriginal(s.id), () => alive.current && current.current.historyId === s.id);
  };

  const flags = flagBadges(j.flags);
  const diffs = comparison?.burst ? regionDiffs(burstSideOf(burst), comparison.burst) : null;
  const region = frame?.region != null ? j.regions[frame.region] : null;
  const metrics = frame?.metrics;
  const mm = (v: number | null | undefined) => v == null ? "—" : v.toFixed(2);
  const imageLabel = `原始 SN ${s.sn} · ${cameraName} · 帧组 ${selection.seq}`;
  const unavailable = !frame ? "没有这一帧的记录"
    : frame.status === "missing" ? "这台相机本帧组缺帧，没有原图（检测不完整，不影响 OK/NG）"
      : frame.rawFile ? "" : `本帧原图未保存或已按保留策略清理；完整指标仍可在下方查看。${detail.recording.state === "complete" ? "" : recordingReason[detail.recording.state]}`;
  return <div className="stack hist-detail">
    <Card className="detail-head">
      <Stack gap={12} justify="space-between">
        <Stack gap={12}>
          <IconButton onClick={onBack} aria-label="返回历史列表"><ArrowLeft size={18} /></IconButton>
          <span className="detail-step">
            <IconButton aria-label="前一件" title="前一件（[ 键）" disabled={neighbors.older == null} onClick={() => onOpen(neighbors.older)}><ChevronLeft size={18} /></IconButton>
            <IconButton aria-label="后一件" title="后一件（] 键）" disabled={neighbors.newer == null} onClick={() => onOpen(neighbors.newer)}><ChevronRight size={18} /></IconButton>
          </span>
          <b className="sn" title={s.sn || undefined}>SN {s.sn || "—"}</b>
          {s.source !== "real" && <StatusBadge tone="off">{sourceLabel[s.source]}</StatusBadge>}
          <Text muted>{formatTime(s.ts)} · {s.recipeId ?? "无配方"} · {s.source === "offlineCandidate" ? "候选修订 " + (s.recipeRevision ?? "未记录") : "v" + (s.recipeVersion ?? "—") + " · #" + (s.recipeRevision?.slice(0, 6) ?? "—")} · {triggerModeLabel(s.triggerMode)} · 帧组 {j.groupCount} / 预期 {expectedText(burst.expectedTotal, burst.toleranceTotal)}</Text>
          {flags.map(flag => <StatusBadge key={flag.key} tone={flag.tone} data-flag={flag.key}>{flag.label}</StatusBadge>)}
        </Stack>
        <StatusBadge tone={verdictTone(s.verdict)}>{verdictLabel[s.verdict]} · PLC {s.plcCode}{s.faultCode ? " / " + s.faultCode : ""}</StatusBadge>
      </Stack>
    </Card>
    {s.source !== "real" && <Alert title={s.source === "offlineCandidate" ? "候选离线调试记录" : "离线回放记录"}>结果来自离线环境，不作为真机生产验收；切换当前站点模式不会改变此记录的来源。</Alert>}
    {error && <Alert tone="warn" title="操作未完成">{error}</Alert>}
    <Panel title="复测与候选对照" detail="原包重现使用本件冻结资源；候选复测使用当前走廊示教；所有结果另存，原始判定保留">
      <div className="wp-actions">
        <Chip>{s.source === "offlineCandidate" ? "原始候选 " + (s.recipeRevision ?? "未记录") : "原始发布 v" + (s.recipeVersion ?? "—")}</Chip>
        <StatusBadge>候选 {ws.data && ws.data.workspace.doc.id === s.recipeId ? "v" + ws.data.workspace.doc.version + " · 修订 " + ws.data.workspace.revision : "尚未选择此配方"}</StatusBadge>
        <span className="spacer" />
        {s.source !== "offlineCandidate" && <Button leadingIcon={<ScanEye size={15} />} disabled={working || !rawComplete || blockers.length > 0} onClick={runOriginal}>按原发布包重现</Button>}
        <Button leadingIcon={<Scale size={15} />} disabled={disabled} onClick={() => runCompare(false)}>按候选规则重判</Button>
        <Button variant="filled" leadingIcon={<ScanEye size={15} />} disabled={disabled || !rawComplete || !candidateTaught} onClick={() => runCompare(true)}>从原图复测整件</Button>
      </div>
      {ws.dirty || ws.frameDirty ? <Alert tone="warn">先保存候选配置和走廊示教，再执行对照。</Alert> : !candidateReady && <Text as="p" muted>在配方工作台选择此配方的候选后，可以调整规则并对照原始结论。</Text>}
      {ws.error && <Alert tone="warn">{ws.error}</Alert>}
      {blockers.length > 0 && <Alert tone="warn" title="不能按原发布包重跑">{blockers.join("；")}</Alert>}
      {!!saved.length && <Select label="已保存的对照结果" aria-label="已保存对照结果" disabled={working} value={comparison?.id ?? ""} onValueChange={id => setComparison(saved.find(c => c.id === id) ?? null)}
        options={[{ value: "", label: "选择记录" }, ...saved.map(c => ({ value: c.id, label: formatTime(c.createdAt) + " · " + comparisonLabel[c.source] + " · 修订 " + c.candidateRevision + " · " + verdictLabel[c.judgement.verdict] }))]} />}
    </Panel>
    {comparison && <Panel title={comparisonLabel[comparison.source] + "结果"} detail={(comparison.source === "original" ? "原发布 v" + comparison.candidateRecipe.version + " · 包 " + comparison.bundleId : "候选 v" + comparison.candidateRecipe.version + " · 修订 " + comparison.candidateRevision) + " · " + formatTime(comparison.createdAt)}>
      <div className="wp-actions">
        <StatusBadge tone={comparison.originalVerdict.startsWith("OK") ? "ok" : "ng"}>原始 {verdictLabel[comparison.originalVerdict]}</StatusBadge><span>→</span>
        <StatusBadge tone={comparison.judgement.verdict.startsWith("OK") ? "ok" : comparison.judgement.verdict === "ERR_INSPECT" ? "warn" : "ng"}>{comparison.source === "original" ? "重现" : "候选"} {verdictLabel[comparison.judgement.verdict]}</StatusBadge>
        <b>{displayReason(comparison.judgement.reason)}</b>
      </div>
      {comparison.burst ? <RegionReplayDiff diffs={diffs} source={comparison.source} /> : <Text as="p" muted>这次对照没有区域明细，只能看整件结论。</Text>}
    </Panel>}
    <div className="detail-grid burst">
      <Panel title="帧组与原图" detail="上方为整件的段布置与区域结论；选中帧组与相机后在下方查看原图" actions={<Select aria-label="历史相机选择" value={String(selection.camera)} onValueChange={value => setSelection(prev => ({ ...prev, camera: Number(value) }))}
        options={burst.cameras.map((name, c) => ({ value: String(c), label: name }))} />}>
        <CameraSegmentOverview cameras={burst.cameras} segments={burst.segments} regions={j.regions} />
        <FrameGroupStrip groups={groups} cameras={burst.cameras} selected={selection.seq} onSelect={seq => setSelection(prev => ({ ...prev, seq }))} />
        <p className="muted">格子颜色是该帧所在区域的结论（历史没有逐帧结论）；灰色为缺帧、未测或走出走廊，只算检测不完整；浅灰为该帧所在段不由这台相机检测，按设计不测。左右方向键切换帧组。</p>
        <div className="wp-stack">
          <GrayViewer image={image.image} loading={image.loading} error={image.error || unavailable} label={imageLabel} caption={<>原始 SN <span className="sn">{s.sn}</span> · {cameraName} · 帧组 {selection.seq}</>} />
          <div className="wp-runtime-strip">
            <KV label="帧状态">{frame ? frameStateName[frame.status] : "—"}</KV>
            <KV label="帧号 / 触发计数">{frame?.frameCounter ?? "—"} / {frame?.triggerCounter ?? "—"}</KV>
            <KV label="所在区域">{region ? regionKindLabel(region.region.kind) : "—"}</KV>
            <KV label="所用走廊">{metrics?.corridor != null ? `段 ${metrics.corridor}` : "—"}</KV>
            <KV label="胶宽 min / max / mean">{metrics && frame?.status === "measured" ? `${mm(metrics.widthMin)} / ${mm(metrics.widthMax)} / ${mm(metrics.widthMean)} mm` : "—"}</KV>
            <KV label="胶中心 x">{metrics && frame?.status === "measured" ? `${mm(metrics.xMm)} mm` : "—"}</KV>
          </div>
        </div>
      </Panel>
      <Panel title="原始整件判定" className="side">
        <p className="reason-box">{displayReason(j.reason)}</p>
        <RegionResultPanel judgement={j} segments={burst.segments} cameras={burst.cameras} expectedTotal={burst.expectedTotal} toleranceTotal={burst.toleranceTotal}
          onSelectGroup={seq => setSelection(prev => ({ ...prev, seq }))} />
        <div className="kv2">
          <span>记录号</span><b>#{s.id}</b>
          {s.sample && <><span>离线样本</span><b>{s.sample.partId} · SN <span className="sn">{s.sample.sn}</span> · 期望 {sampleLabelText[s.sample.label]}</b></>}
          <span>配方修订</span><b>{s.recipeRevision ?? "—"}</b>
          <span>软件版本</span><b>{detail.softwareVersion}</b>
          <span>收尾耗时</span><b>{s.drainMs ?? "—"} ms</b>
          <span>触发计数（参考）</span><b>{burst.deviceTriggers ?? "—"}</b>
          <span>复检自</span><b>{s.retestOf ? <Link to={"/history/" + s.retestOf}>#{s.retestOf}</Link> : "—"}</b>
          <span>后续复检</span><b>{detail.retests.length ? detail.retests.map(r => <Link key={r} to={"/history/" + r}>#{r} </Link>) : "—"}</b>
        </div>
      </Panel>
      <BurstPartTrace detail={detail} burst={burst} selected={selection} onSelect={(camera, seq) => setSelection({ camera, seq })} />
      <CycleTimeline historyId={s.id} cycleId={s.cycleId} ts={s.ts} frame={{ camera: cameraName, seq: selection.seq, onSelect: (camera, seq) => setSelection({ camera: Math.max(0, burst.cameras.indexOf(camera)), seq }) }} />
    </div>
  </div>;
}
