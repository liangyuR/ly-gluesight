import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { ArrowLeft, ChevronLeft, ChevronRight, Scale, ScanEye } from "lucide-react";
import { Alert, Button, Card, Chip, IconButton, Select, Stack, StatusBadge, Text } from "qing-steel";
import FrameGroupStrip from "../../cycle/components/FrameGroupStrip";
import RegionResultPanel from "../../cycle/components/RegionResultPanel";
import { CameraSegmentOverview } from "../../plan/CameraSegmentOverview";
// 图表库按需加载：只有打开带逐站数据的历史详情时才进来
const WidthProfile = lazy(() => import("../../plan/WidthProfile").then(m => ({ default: m.WidthProfile })));
import type { CalibrationFrameView, FrameGroupView, FrameRecord, PartDetailBurst } from "../../plan/types";
import { expectedText, flagBadges, regionKindLabel } from "../../plan/vis";
import { burstImageApi } from "../../workspace/burstImages";
import { workspaceApi } from "../../workspace/api";
import { FrameViewer, KV, Panel } from "../../workspace/components";
import { useWorkspace } from "../../workspace/context";
import type { CompareOptions, Comparison, FrameImage } from "../../workspace/types";
import { CycleTimeline } from "../../logs";
import { errorText } from "../../../lib/errors";
import { displayReason, formatTime, partVerdict, sampleLabelText, sourceLabel, triggerModeLabel, verdictLabel } from "../meta";
import { burstSideOf, comparisonBurst, regionDiffs } from "../replay";
import type { PartDetail } from "../types";
import { BurstPartTrace } from "./PartTrace";
import { RegionReplayDiff } from "./ReplayDiff";

const comparisonLabel = { rules: "规则重判", raw: "候选原图复测", original: "原发布包重现" };
const recordingReason = { off: "录制关闭，本件未保存原图", notRetained: "按录制策略本件未保存原图", pending: "原图仍在落盘", incomplete: "原图不完整", failed: "原图录制失败", complete: "原图已按保留策略清理或无法核验" };
const frameStateName = { measured: "已测", missing: "缺帧", error: "测量失败", unmeasured: "未测", outOfCorridor: "出走廊", notInspected: "本段不检测" };

/** 格子按该帧的单帧结论着色（后端按所在区域判，与在线同一口径），整段性问题只在段条上；测过但没有结论的为未判定，
 * 缺帧、测量失败等状态照常显示为灰色 / 紫色。 */
export function historyGroups(burst: PartDetailBurst): FrameGroupView[] {
  const bySeq = new Map<number, Map<number, FrameRecord>>();
  for (const frame of burst.frames) bySeq.set(frame.seq, (bySeq.get(frame.seq) ?? new Map()).set(frame.camera, frame));
  const verdicts = new Map(burst.frameVerdicts.map(v => [`${v.camera}:${v.seq}`, v.verdict]));
  const incomplete = new Set(burst.judgement.regions.flatMap(r => r.incompleteGroups));
  return [...bySeq.keys()].sort((a, b) => a - b).map(seq => {
    const frames = bySeq.get(seq)!;
    const cells = burst.cameras.map((_, c) => {
      const frame = frames.get(c);
      if (!frame) return { status: "missing" as const, verdict: null, frameCounter: null };
      const verdict = frame.status === "measured" ? verdicts.get(`${c}:${seq}`) ?? null : null;
      return { status: frame.status, verdict, unjudged: frame.status === "measured" && verdict == null, frameCounter: frame.frameCounter };
    });
    return { seq, incomplete: incomplete.has(seq) || cells.some(cell => cell.status !== "measured" && cell.status !== "notInspected"), cells };
  });
}

/** 一帧录制原图：没有原图（缺帧、未录制、录制队满丢弃）时不读，按钮与说明由页面给。 */
function useFrameImage(historyId: number, camera: number, seq: number, enabled: boolean) {
  const key = JSON.stringify([historyId, camera, seq, enabled]);
  const [state, setState] = useState<{ key: string; image: FrameImage | null; error: string; loading: boolean }>({ key, image: null, error: "", loading: false });
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

/** 一帧的叠图（走廊、搜索带与胶边，按需从原图重算、不落盘）：看原始记录时按记录的发布包重测，看对照时按对照所用的配方重测。 */
function useFrameOverlay(historyId: number, comparison: Comparison | null, camera: number, seq: number, enabled: boolean) {
  const key = JSON.stringify([historyId, comparison?.id ?? null, camera, seq, enabled]);
  const [state, setState] = useState<{ key: string; view: CalibrationFrameView | null; error: string }>({ key, view: null, error: "" });
  useEffect(() => {
    let alive = true;
    setState({ key, view: null, error: "" });
    if (!enabled) return;
    (comparison ? workspaceApi.recordFrameView(historyId, comparison.id, camera, seq) : workspaceApi.historyFrameView(historyId, camera, seq))
      .then(view => alive && setState({ key, view, error: "" }))
      .catch(e => alive && setState({ key, view: null, error: errorText(e) }));
    return () => { alive = false; };
  }, [key]);
  return state.key === key ? state : { key, view: null, error: "" };
}

/** 一台相机一帧的原图格：叠出测量所用的走廊、搜索带与胶边（原始记录按原发布包，对照按对照的配方）；点格子选中这台相机。 */
function FrameTile({ detail, comparison, camera, cameraName, seq, frame, selected, onSelect, station, onStation }: {
  /** 显示对照结果时为该对照，显示原始记录时为 null */
  detail: PartDetail; comparison: Comparison | null; camera: number; cameraName: string; seq: number; frame: FrameRecord | null; selected: boolean; onSelect: () => void;
  /** 选中相机这一帧与胶宽折线联动的测站（0 起） */
  station: number | null; onStation: (station: number | null) => void;
}) {
  const s = detail.summary;
  const image = useFrameImage(s.id, camera, seq, !!frame?.rawFile);
  const traced = useFrameOverlay(s.id, comparison, camera, seq, !!frame?.rawFile && frame.status !== "notInspected" && frame.status !== "missing");
  const lit = selected ? station : null;
  // 联动的测站画在最后、加粗换色，免得被相邻测站盖住
  const lines = (traced.view?.edges ?? []).flatMap((edge, i) => edge ? [{ from: edge[0], to: edge[1], station: i,
    color: i === lit ? "var(--ov-warn)" : "var(--ov-line)", width: i === lit ? 4 : 2 }] : []).sort((a, b) => Number(a.station === lit) - Number(b.station === lit));
  const overlay = traced.view && traced.view.corridor.length >= 2 ? { path: traced.view.corridor, searchHalfPx: traced.view.searchHalfPx ?? undefined,
    segments: lines, partEdges: traced.view.partEdges, onSegmentHover: selected ? onStation : undefined } : undefined;
  const unavailable = !frame ? "没有这一帧的记录"
    : frame.status === "missing" ? "这台相机本帧组缺帧，没有原图（检测不完整，不影响 OK/NG）"
      : frame.rawFile ? "" : `本帧原图未保存或已按保留策略清理；完整指标仍可在下方查看。${detail.recording.state === "complete" ? "" : recordingReason[detail.recording.state]}`;
  return <div className={"hist-frame-tile" + (selected ? " selected" : "")} onClickCapture={onSelect}>
    <FrameViewer image={image.image} loading={image.loading} error={image.error || unavailable} label={`原始 SN ${s.sn} · ${cameraName} · 帧组 ${seq}`} overlay={overlay}
      caption={<>{cameraName} · 帧组 {seq}{frame ? ` · ${frameStateName[frame.status]}` : ""}</>} />
    {traced.error && <p className="muted hint">叠图：{traced.error}</p>}
  </div>;
}

/**
 * 历史详情：按区域展示——整件的段布置与区域结论、帧组条、按 (相机, seq) 选的原图、每相机帧表与时间线。
 * 只处理 PartDetail.burst 非空的记录。
 */
export default function BurstHistoryDetail({ detail, burst, neighbors, onOpen, onBack, backLabel = "返回历史列表" }: {
  detail: PartDetail;
  burst: PartDetailBurst;
  neighbors: { older: number | null; newer: number | null };
  onOpen: (target: number | null) => void;
  onBack: () => void;
  /** 返回按钮的说明：从历史列表进来为“返回历史列表”，从试测进来为“返回试测” */
  backLabel?: string;
}) {
  const ws = useWorkspace();
  const [search] = useSearchParams();
  const wanted = search.get("comparison");
  const s = detail.summary;
  const verdict = partVerdict(s);
  const [comparison, setComparison] = useState<Comparison | null>(null);
  const [showOriginal, setShowOriginal] = useState(false);
  const compared = useMemo(() => comparison?.burst ? comparisonBurst(burst, comparison.burst) : null, [burst, comparison]);
  const shown = compared && !showOriginal ? compared : burst, j = shown.judgement;
  const groups = useMemo(() => historyGroups(shown), [shown]);
  const lastSeq = groups.at(-1)?.seq ?? 1;
  const [selection, setSelection] = useState({ camera: 0, seq: groups[0]?.seq ?? 1 });
  const [station, setStation] = useState<number | null>(null);
  useEffect(() => { setStation(null); }, [selection.camera, selection.seq, comparison?.id, showOriginal]);
  const [saved, setSaved] = useState<Comparison[]>([]);
  const [error, setError] = useState("");
  const [working, setWorking] = useState(false);
  const pending = useRef(false), serial = useRef(0), alive = useRef(true);
  const [options, setOptions] = useState<CompareOptions | null>(null);
  const candidate = options?.candidate ?? null;
  const scope = `${s.id}:${candidate?.id ?? ""}:${candidate?.revision ?? ""}`;
  const current = useRef({ scope, historyId: s.id });
  current.current = { scope, historyId: s.id };
  useEffect(() => { alive.current = true; return () => { alive.current = false; serial.current++; }; }, []);
  useEffect(() => { setSelection({ camera: 0, seq: groups[0]?.seq ?? 1 }); setComparison(null); setShowOriginal(false); setError(""); }, [s.id]);

  const frame = shown.frames.find(f => f.camera === selection.camera && f.seq === selection.seq) ?? null;
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
    if (s.recipeId) void workspaceApi.comparisons(s.recipeId, s.id).then(v => {
      if (!live || mine !== serial.current) return;
      setSaved(v);
      const target = wanted && v.find(c => c.id === wanted);
      if (target) setComparison(previous => previous ?? target);
    }).catch(e => live && mine === serial.current && setError(errorText(e)));
    return () => { live = false; };
  }, [s.id, s.recipeId, ws.data?.workspace.revision, wanted]);

  // 复测能不能做以记录为准（后端按记录的配方找本机候选），与工作台当前选的配方无关；候选改了（修订变化）重新查
  const editedRevision = ws.data?.workspace.doc.id === s.recipeId ? ws.data.workspace.revision : null;
  useEffect(() => {
    let live = true;
    setOptions(null);
    void workspaceApi.historyCompareOptions(s.id).then(v => live && setOptions(v)).catch(e => live && setError(errorText(e)));
    return () => { live = false; };
  }, [s.id, editedRevision]);
  // 工作台正在编辑这个配方的候选且有没保存的改动时先保存：对照按候选已保存的修订做
  const editing = ws.data?.workspace.doc.id === s.recipeId;
  const unsaved = editing && (ws.dirty || ws.frameDirty) ? "先保存候选配置和走廊示教，再执行对照。" : editing && ws.busy ? "候选正在保存，稍后再对照。" : "";
  const candidateBlock = !candidate ? "" : unsaved;
  const rulesReason = options ? candidateBlock || (options.rules.available ? "" : options.rules.reason ?? "") : "";
  const rawReason = options ? candidateBlock || (options.raw.available ? "" : options.raw.reason ?? "") : "";
  const originalReason = options && !options.original.available ? options.original.reason ?? "" : "";

  const compare = async (run: () => Promise<Comparison>, guard: () => boolean) => {
    if (pending.current) return;
    pending.current = true; serial.current++; setWorking(true); setError("");
    try { const result = await run(); if (guard()) { setComparison(result); setShowOriginal(false); setSaved(v => [result, ...v.filter(c => c.id !== result.id)]); } }
    catch (e) { if (guard()) setError(errorText(e)); }
    finally { pending.current = false; if (alive.current) setWorking(false); }
  };
  const runCompare = (fromRaw: boolean) => {
    if (working || !options || !candidate || (fromRaw ? rawReason : rulesReason)) return;
    const { id, revision } = candidate;
    void compare(() => workspaceApi.compare(id, revision, s.id, fromRaw), () => alive.current && current.current.scope === scope);
  };
  const runOriginal = () => {
    if (working || !options || originalReason) return;
    void compare(() => workspaceApi.compareOriginal(s.id), () => alive.current && current.current.historyId === s.id);
  };

  const flags = flagBadges(burst.judgement.flags);
  const diffs = comparison?.burst ? regionDiffs(burstSideOf(burst), comparison.burst) : null;
  const region = frame?.region != null ? j.regions[frame.region] : null;
  const metrics = frame?.metrics;
  const mm = (v: number | null | undefined) => v == null ? "—" : v.toFixed(2);
  const viewing = shown === compared && comparison ? comparisonLabel[comparison.source] : null;
  const cameraName = shown.cameras[selection.camera] ?? `相机 ${selection.camera + 1}`;
  return <div className="stack hist-detail">
    <Card className="detail-head" role="region" aria-label="历史记录摘要">
      <Stack gap={12} justify="space-between">
        <Stack gap={12}>
          <IconButton onClick={onBack} aria-label={backLabel} title={backLabel}><ArrowLeft size={18} /></IconButton>
          <span className="detail-step">
            <IconButton aria-label="前一件" title="前一件（[ 键）" disabled={neighbors.older == null} onClick={() => onOpen(neighbors.older)}><ChevronLeft size={18} /></IconButton>
            <IconButton aria-label="后一件" title="后一件（] 键）" disabled={neighbors.newer == null} onClick={() => onOpen(neighbors.newer)}><ChevronRight size={18} /></IconButton>
          </span>
          <b className="sn" title={s.sn || undefined}>SN {s.sn || "—"}</b>
          {s.source !== "real" && <StatusBadge tone="off">{sourceLabel[s.source]}</StatusBadge>}
          <Text muted>{formatTime(s.ts)} · {s.recipeId ?? "无配方"} · {s.source === "offlineCandidate" ? "候选修订 " + (s.recipeRevision ?? "未记录") : "v" + (s.recipeVersion ?? "—") + " · #" + (s.recipeRevision?.slice(0, 6) ?? "—")} · {triggerModeLabel(s.triggerMode)} · 帧组 {burst.judgement.groupCount} / 预期 {expectedText(burst.expectedTotal, burst.toleranceTotal)}</Text>
          {flags.map(flag => <StatusBadge key={flag.key} tone={flag.tone} data-flag={flag.key}>{flag.label}</StatusBadge>)}
        </Stack>
        <StatusBadge tone={verdict.tone}>{verdict.label} · PLC {s.plcCode}{s.faultCode ? " / " + s.faultCode : ""}</StatusBadge>
      </Stack>
    </Card>
    {s.source !== "real" && <Alert title={s.source === "offlineCandidate" ? "候选离线调试记录" : "离线回放记录"}>结果来自离线环境，不作为真机生产验收；切换当前站点模式不会改变此记录的来源。</Alert>}
    {error && <Alert tone="warn" title="操作未完成">{error}</Alert>}
    <Panel title="复测与候选对照" help="对照结果另存，原始判定保留">
      <div className="wp-actions">
        <Chip>{s.source === "offlineCandidate" ? "原始候选 " + (s.recipeRevision ?? "未记录") : "原始发布 v" + (s.recipeVersion ?? "—")}</Chip>
        <StatusBadge>候选 {!options ? "…" : candidate ? "v" + candidate.version + " · 修订 " + candidate.revision : "本机没有此配方的候选"}</StatusBadge>
        <span className="spacer" />
        {s.source !== "offlineCandidate" && <Button leadingIcon={<ScanEye size={15} />} disabled={working || !options || !!originalReason} title={originalReason || undefined} onClick={runOriginal}>按原发布包重现</Button>}
        <Button leadingIcon={<Scale size={15} />} disabled={working || !options || !!rulesReason} title={rulesReason || undefined} onClick={() => runCompare(false)}>按候选规则重判</Button>
        <Button variant="filled" leadingIcon={<ScanEye size={15} />} disabled={working || !options || !!rawReason} title={rawReason || undefined} onClick={() => runCompare(true)}>从原图复测整件</Button>
      </div>
      {options && (originalReason || rulesReason || rawReason) && <ul className="compare-reasons" aria-label="不能复测的原因">
        {s.source !== "offlineCandidate" && originalReason && <li><b>按原发布包重现</b>：{originalReason}</li>}
        {rulesReason && rulesReason === rawReason ? <li><b>按候选规则重判、从原图复测</b>：{rulesReason}</li> : <>
          {rulesReason && <li><b>按候选规则重判</b>：{rulesReason}</li>}
          {rawReason && <li><b>从原图复测</b>：{rawReason}</li>}
        </>}
      </ul>}
      {options && !rawReason && options.raw.note && <Text as="p" muted className="compare-note">从原图复测：{options.raw.note}</Text>}
      {editing && ws.error && <Alert tone="warn">{ws.error}</Alert>}
      {!!saved.length && <Select label="已保存的对照结果" aria-label="已保存对照结果" disabled={working} value={comparison?.id ?? ""} onValueChange={id => { setComparison(saved.find(c => c.id === id) ?? null); setShowOriginal(false); }}
        options={[{ value: "", label: "选择记录" }, ...saved.map(c => ({ value: c.id, label: formatTime(c.createdAt) + " · " + comparisonLabel[c.source] + " · 修订 " + c.candidateRevision + " · " + verdictLabel[c.judgement.verdict] }))]} />}
    </Panel>
    {comparison && <Panel title={comparisonLabel[comparison.source] + "结果"} detail={(comparison.source === "original" ? "原发布 v" + comparison.candidateRecipe.version + " · 包 " + comparison.bundleId : "候选 v" + comparison.candidateRecipe.version + " · 修订 " + comparison.candidateRevision) + " · " + formatTime(comparison.createdAt)}>
      <div className="wp-actions">
        <StatusBadge tone={comparison.originalVerdict.startsWith("OK") ? "ok" : "ng"}>原始 {verdictLabel[comparison.originalVerdict]}</StatusBadge><span>→</span>
        <StatusBadge tone={comparison.judgement.verdict.startsWith("OK") ? "ok" : comparison.judgement.verdict === "ERR_INSPECT" ? "warn" : "ng"}>{comparison.source === "original" ? "重现" : "候选"} {verdictLabel[comparison.judgement.verdict]}</StatusBadge>
        <b>{displayReason(comparison.judgement.reason)}</b>
      </div>
      {!!comparison.burst?.unmeasured?.length && <Alert tone="warn" title="有段没测，结论只覆盖其余段">
        <ul className="compare-reasons" aria-label="没测的段">{comparison.burst.unmeasured.map(u => <li key={`${u.segment}:${u.camera}`}>{u.reason}</li>)}</ul>
      </Alert>}
      {!comparison.burst ? <Text as="p" muted>这次对照没有区域明细，只能看整件结论。</Text>
        : !burst.judgement.regions.length ? <Text as="p" muted>原记录没有区域判定（如维护采集件只录图不判定），不做逐区域对比；{comparisonLabel[comparison.source]}的逐段结果见下方帧组与原图和右侧整件判定。</Text>
          : <RegionReplayDiff diffs={diffs} source={comparison.source} />}
    </Panel>}
    <div className="detail-grid burst">
      <Panel title="帧组与原图" detail={viewing ? `显示${viewing}结果` : compared ? "显示原始记录" : undefined} help="点击小方块选择相机与帧组查看原图，左右方向键切换帧组。格子颜色只表示本帧；上方色带是所在段的结论，胶宽不均、整段胶偏、帧数异常等整段性问题看色带。灰色为检测不完整，浅灰为本段不检测或未判定。选了带逐帧指标的对照时，可切换显示对照结果或原始记录。" actions={<>
        {compared && <Select aria-label="显示结果" value={showOriginal ? "original" : "comparison"} onValueChange={value => setShowOriginal(value === "original")}
          options={[{ value: "comparison", label: "对照结果" }, { value: "original", label: "原始记录" }]} />}
        <Select aria-label="历史相机选择" value={String(selection.camera)} onValueChange={value => setSelection(prev => ({ ...prev, camera: Number(value) }))}
          options={shown.cameras.map((name, c) => ({ value: String(c), label: name }))} />
      </>}>
        <CameraSegmentOverview cameras={shown.cameras} segments={shown.segments} regions={j.regions} />
        <FrameGroupStrip groups={groups} cameras={shown.cameras} regions={j.regions} selected={selection.seq} selectedCamera={selection.camera} onSelect={seq => setSelection(prev => ({ ...prev, seq }))} onSelectCell={(seq, camera) => setSelection({ seq, camera })} />
        <div className="wp-stack">
          <div className="hist-frame-tiles" aria-label={`帧组 ${selection.seq} 各相机原图`}>
            {shown.cameras.map((name, c) => <FrameTile key={c} detail={detail} comparison={viewing ? comparison : null} camera={c} cameraName={name} seq={selection.seq}
              frame={shown.frames.find(f => f.camera === c && f.seq === selection.seq) ?? null} selected={c === selection.camera} onSelect={() => setSelection(prev => ({ ...prev, camera: c }))}
              station={station} onStation={setStation} />)}
          </div>
          <div className="wp-runtime-strip">
            <KV label="相机">{cameraName}</KV>
            <KV label="帧状态">{frame ? frameStateName[frame.status] : "—"}</KV>
            <KV label="帧号 / 触发计数">{frame?.frameCounter ?? "—"} / {frame?.triggerCounter ?? "—"}</KV>
            <KV label="所在区域">{region ? regionKindLabel(region.region.kind) : "—"}</KV>
            <KV label="所用走廊">{metrics?.corridor != null ? `段 ${metrics.corridor}` : "—"}</KV>
            <KV label="胶宽 min / max / mean">{metrics && frame?.status === "measured" ? `${mm(metrics.widthMin)} / ${mm(metrics.widthMax)} / ${mm(metrics.widthMean)} mm` : "—"}</KV>
            <KV label="胶中心 x">{metrics && frame?.status === "measured" ? `${mm(metrics.xMm)} mm` : "—"}</KV>
            <KV label="最长断胶">{metrics && frame?.status === "measured" ? `${mm(metrics.gapMaxMm)} mm` : "—"}</KV>
            {metrics?.headingFallback && <KV label="定向">没找到胶条{metrics.fallbackReason ? `：${metrics.fallbackReason}` : ""}</KV>}
            {metrics?.error && <KV label="说明">{metrics.error}</KV>}
          </div>
          {metrics && frame?.status === "measured" && metrics.widths.length > 0
            ? <div className="hist-width-profile">
              <Text as="p" muted>{cameraName} · 帧组 {selection.seq} 逐站胶宽{station != null ? ` · 第 ${station + 1} 站` : ""}（悬停折线或原图上的测量线联动）</Text>
              <Suspense fallback={<Text as="p" muted>正在加载图表…</Text>}>
                <WidthProfile metrics={metrics} checks={shown.segments.find(sg => sg.index === metrics.corridor)?.checks} station={station} onStation={setStation} />
              </Suspense>
            </div>
            : <Text as="p" muted>这一帧没有逐站胶宽数据（未测、缺帧或本段不检测）。</Text>}
        </div>
      </Panel>
      <Panel title={viewing ? `${viewing}整件判定` : "原始整件判定"} className="side">
        <p className="reason-box">{displayReason(j.reason)}</p>
        <RegionResultPanel judgement={j} segments={shown.segments} cameras={shown.cameras} expectedTotal={shown.expectedTotal} toleranceTotal={shown.toleranceTotal}
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
