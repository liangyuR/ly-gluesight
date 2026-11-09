import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useHorizontalWindow } from "../../lib/useHorizontalWindow";
import { Button, IconButton, Surface } from "qing-steel";
import { ChevronDown, ChevronLeft, ChevronRight, ChevronUp, Play } from "lucide-react";
import { errorText } from "../../lib/errors";
import type { CalibrationFrameView, FrameMetrics, PassSkip, PathSegment, TeachPreviewSegment } from "../plan/types";
import { passCells } from "./workbenchDraft";
import { workspaceApi } from "./api";
import { Badge, FrameViewer } from "./components";
import type { HeadingGuide } from "./headingOverlay";
import type { FrameImage } from "./types";
import { OperationStatus, useOperation } from "./operation";
import { evidenceOverlay, MeasurementEvidenceSummary, measurementIssue, presentRatioText } from "../plan/measurementEvidence";

type TrialState = "ok" | "clipped" | "unknown" | "fallback" | "empty" | "out" | "error" | "missing";
const trialOrder: TrialState[] = ["ok", "clipped", "unknown", "fallback", "empty", "out", "error", "missing"];
const trialText: Record<TrialState, string> = { ok: "测到", clipped: "超出测量范围", unknown: "测量不完整", fallback: "没找到胶条", empty: "无胶", out: "出走廊", error: "测量失败", missing: "缺帧或未测" };
const trialColor: Record<TrialState, string> = {
  ok: "var(--ok)", clipped: "var(--warn)", unknown: "var(--on-surface-disabled)", fallback: "var(--warn)", empty: "var(--ng)", out: "var(--on-surface-disabled)", error: "var(--fault)", missing: "var(--outline-variant)",
};
const trialTone: Record<TrialState, "ok" | "warn" | "ng" | "neutral"> = { ok: "ok", clipped: "warn", unknown: "warn", fallback: "warn", empty: "ng", out: "neutral", error: "ng", missing: "neutral" };

function trialState(m: FrameMetrics): TrialState {
  const issue = measurementIssue(m.measurementEvidence);
  if (issue) return issue;
  if (m.measurementEvidence && m.state === "measured") return (m.measurementEvidence.presentRatio ?? 0) > 0 ? "ok" : "empty";
  if (m.state === "measured") return m.headingFallback ? "fallback" : m.presentRatio > 0 ? "ok" : "empty";
  return m.state === "outOfCorridor" ? "out" : m.state === "error" ? "error" : "missing";
}
const mm = (value: number | null | undefined, digits = 2) => value == null ? "—" : `${value.toFixed(digits)} mm`;
const skipColor = "var(--outline-variant)";

/**
 * 示教页同段试测（只读）：用候选已保存的走廊与定向参数，测示教轮次里本段的帧。“全段试测”给出逐帧状态条；
 * 选中一帧（`seq`）时显示这一帧的原图与测量叠加，代替示教画布。候选修订号变了，全段结果标为过期，选中的帧自动重测。
 */
/**
 * 本段试测：用候选已保存的走廊与定向参数只读地测示教件里本段的帧。`locked` 是真正不能试测的原因（显示出来）；
 * `waiting` 是还有改动等着自动保存：按钮照常可点，点下去先 `onFlush` 立即保存，存好再测，期间不换掉正在看的这一帧。
 */
export function SegmentTrial({ id, camera, segment, round, teachSeq, range, skipRules, revision, locked, waiting = false, onFlush, guide, seq, onSeq, lead, side }: {
  id: string; camera: string; segment: number; round: string; teachSeq: number; range: { start: number; end: number } | null; revision: number;
  skipRules: { passes: number[] | undefined; groupCount: number; passSkip: PassSkip | undefined; segment: PathSegment | undefined; guardRanges?: { start: number; end: number; segment: number }[] };
  locked: string; waiting?: boolean; onFlush?: () => void; guide: HeadingGuide | null; seq: number | null; onSeq: (seq: number | null) => void;
  /** 与试测并在同一行的示教帧标题（行首）与操作（行尾） */
  lead?: ReactNode; side?: ReactNode;
}) {
  const [batch, setBatch] = useState<{ result: TeachPreviewSegment | null; revision: number; error: string; running: boolean }>({ result: null, revision, error: "", running: false });
  const [queued, setQueued] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const operation = useOperation();
  const run = async () => {
    if (locked || batch.running) return;
    if (waiting) { setQueued(true); onFlush?.(); return; }
    const at = revision;
    setExpanded(false);
    setBatch(previous => ({ ...previous, running: true, error: "" }));
    try {
      const result = await operation.run(taskId => workspaceApi.teachPreviewSegment(id, camera, segment, round, taskId));
      setBatch({ result, revision: at, error: "", running: false });
    } catch (e) { setBatch(previous => ({ ...previous, running: false, error: errorText(e) })); }
  };
  useEffect(() => {
    if (!queued || waiting) return;
    setQueued(false);
    if (!locked) void run();
  }, [queued, waiting, locked]);
  const span = batch.result ? { start: batch.result.start, end: batch.result.end } : range;
  const stale = !!batch.result && batch.revision !== revision;
  const skipReasons = useMemo(() => {
    const result = new Map<number, string>();
    if (!range) return result;
    const passes = passCells(skipRules.passes, skipRules.groupCount, skipRules.passSkip);
    const params = skipRules.segment;
    for (let n = range.start; n <= range.end; n++) {
      const reasons: string[] = [];
      const pass = passes?.[n - 1];
      if (pass?.skip) reasons.push(`第 ${pass.pass} 笔${{ head: "开头跳过", tail: "结尾跳过", short: "太短，整笔不检" }[pass.skip]}`);
      if (params?.skip) reasons.push("本段不检测");
      else if (params) {
        if (n < range.start + params.skipHead) reasons.push("段首跳过");
        if (n > range.end - params.skipTail) reasons.push("段尾跳过");
      }
      for (const guard of skipRules.guardRanges ?? []) {
        if (n >= guard.start && n <= guard.end) reasons.push(`段 ${guard.segment} 不检测的边界保护`);
      }
      if (reasons.length) result.set(n, reasons.join("；"));
    }
    return result;
  }, [range?.start, range?.end, skipRules.passes, skipRules.groupCount, skipRules.passSkip, skipRules.segment, skipRules.guardRanges]);
  const cells = useMemo(() => {
    const bySeq = new Map(batch.result?.frames.map(frame => [frame.seq, frame]));
    return span ? Array.from({ length: span.end - span.start + 1 }, (_, i) => {
      const frame = bySeq.get(span.start + i);
      return { seq: span.start + i, skipReason: skipReasons.get(span.start + i), state: frame ? trialState(frame) : null, note: frame?.error ?? frame?.fallbackReason ?? null };
    }) : [];
  }, [batch.result, span?.start, span?.end, skipReasons]);
  const window = useHorizontalWindow<HTMLDivElement>(cells.length, 10, seq != null && span ? seq - span.start : null);
  const restoreFocus = useRef(false);
  useEffect(() => {
    const cell = window.ref.current?.querySelector<HTMLElement>('[aria-pressed="true"]');
    if (restoreFocus.current && cell) { cell.focus({ preventScroll: true }); restoreFocus.current = false; }
  }, [seq, window.start, window.end]);

  const frameKey = seq == null ? "" : JSON.stringify([id, camera, segment, round, seq]);
  const key = seq == null ? "" : JSON.stringify([frameKey, revision]);
  const [detail, setDetail] = useState<{ key: string; frameKey: string; image: FrameImage | null; view: CalibrationFrameView | null; error: string }>({ key: "", frameKey: "", image: null, view: null, error: "" });
  useEffect(() => {
    if (seq == null || locked || waiting) return;
    let alive = true;
    const request = new AbortController();
    void Promise.allSettled([workspaceApi.captureFrameImage(round, camera, seq, request.signal), workspaceApi.teachPreviewFrame(id, camera, segment, round, seq, request.signal)])
      .then(([image, view]) => alive && setDetail({ key, frameKey,
        image: image.status === "fulfilled" ? image.value : null,
        view: view.status === "fulfilled" ? view.value : null,
        error: [image, view].flatMap(r => r.status === "rejected" ? [errorText(r.reason)] : []).join("；") }));
    return () => { alive = false; request.abort(); };
  }, [key, !!locked, waiting]);
  // 同一帧在参数保存后重测：结果回来前继续显示上一次的，画面不闪
  const shown = detail.key === key || (frameKey && detail.frameKey === frameKey) ? detail : null;
  const view = shown?.view ?? null;
  const metrics = view?.metrics;
  const state = metrics ? trialState(metrics) : null;
  const skipReason = seq == null ? undefined : skipReasons.get(seq);
  const step = (delta: number) => {
    if (seq == null || !span) return;
    const next = Math.min(span.end, Math.max(span.start, seq + delta));
    if (next !== seq) onSeq(next);
  };
  const move = (event: KeyboardEvent) => {
    const delta = { ArrowLeft: -1, ArrowRight: 1, Home: -Infinity, End: Infinity }[event.key];
    if (seq == null || delta == null) return;
    event.preventDefault();
    restoreFocus.current = !!window.ref.current?.contains(event.target as Node);
    step(delta);
  };
  const edges = (view?.edges ?? []).flatMap(edge => edge ? [{ from: edge[0], to: edge[1], color: "var(--ov-line)" }] : []);
  const counts = batch.result ? trialOrder.map(item => [item, cells.filter(cell => !cell.skipReason && cell.state === item).length] as const).filter(([, n]) => n > 0) : [];
  const skippedCount = cells.filter(cell => cell.skipReason).length;
  const finished = !!batch.result && !batch.running && !queued && !batch.error;
  const details = !finished || expanded;

  return <>
    <Surface role="region" className="wp-trial" aria-label="本段试测" onKeyDown={move}>
      <div className="wp-trial-head">
        {lead}
        <Button size="sm" variant={batch.result && !stale ? "tonal" : "filled"} data-variant={batch.result && !stale ? "tonal" : "filled"} leadingIcon={<Play size={13}/>}
          disabled={!!locked || batch.running} title={locked || "测本段全部帧，用已保存的走廊与定向参数，只读"} onClick={() => void run()}>{batch.running || queued ? "试测中…" : batch.result ? "重新试测" : "全段试测"}</Button>
        <Button size="sm" variant="tonal" disabled={!!locked || seq === teachSeq} title={locked || "只测示教帧"} onClick={() => onSeq(teachSeq)}>试测当前帧</Button>
        {span
          ? <div ref={window.ref} onScroll={window.onScroll} className={"wp-trial-strip" + (stale ? " stale" : "")} role="group" aria-label="逐帧试测状态" title={batch.result ? undefined : "点格子试测那一帧；“全段试测”给每一格标上状态"}>
            {window.start > 0 && <span aria-hidden="true" style={{ flex: `0 0 ${window.start * 10 - 2}px` }} />}
            {cells.slice(window.start, window.end).map(cell => {
              const text = `帧组 ${cell.seq}：${cell.skipReason ? "生产跳过" : cell.state ? trialText[cell.state] : "未试测"}${cell.seq === teachSeq ? "（示教帧）" : ""}`;
              const hint = [text, cell.skipReason, cell.skipReason && cell.state ? `仅供诊断：${trialText[cell.state]}` : null, cell.note].filter(Boolean).join("；");
              return <Button variant="text" size="sm" key={cell.seq} type="button" aria-pressed={cell.seq === seq} aria-label={text} title={hint} disabled={!!locked}
                className={"wp-trial-cell" + (cell.seq === seq ? " selected" : "") + (cell.seq === teachSeq ? " teach" : "")}
                style={{ background: cell.skipReason ? skipColor : cell.state ? trialColor[cell.state] : "var(--surface-high)" }} onClick={() => onSeq(cell.seq)} />;
            })}
            {window.end < cells.length && <span aria-hidden="true" style={{ flex: `0 0 ${(cells.length - window.end) * 10 - 2}px` }} />}
          </div>
          : <span className="muted wp-trial-fill" title="“试测当前帧”只测示教帧；“全段试测”测本段全部帧，得到逐帧状态条，之后可点任一帧看原图与测量叠加。">{batch.running ? "正在逐帧试测本段…" : "全段试测后显示逐帧状态"}</span>}
        {span && <span className="muted wp-trial-span" title="示教件里本段的帧组范围">帧组 {span.start}–{span.end}</span>}
        {seq != null && <Button size="sm" variant="text" onClick={() => onSeq(null)}>回到示教帧</Button>}
        {side && <span className="wp-trial-actions">{side}</span>}
      </div>
      {details && <OperationStatus operation={operation} label="全段试测" />}
      {locked && <p className="muted hint">{locked}</p>}
      {batch.error && batch.error !== operation.snapshot()?.error && <p className="c-warn" role="alert">{batch.error}</p>}
      {(batch.result || skippedCount > 0) && <div className="wp-trial-legend" aria-label="试测统计">
        {finished && <strong>全段试测已完成</strong>}
        {skippedCount > 0 && <span><i style={{ background: skipColor }} aria-hidden="true" />生产跳过 {skippedCount}</span>}
        {batch.result && <span>其余帧试测：</span>}
        {counts.map(([item, n]) => <span key={item}><i style={{ background: trialColor[item] }} aria-hidden="true" />{trialText[item]} {n}</span>)}
        <span><i className="teach" aria-hidden="true" />示教帧</span>
        {stale && <span className="c-warn">参数已修改，结果已过期，请重新试测</span>}
        {finished && <Button size="sm" variant="text" className="wp-trial-toggle" aria-expanded={expanded} trailingIcon={expanded ? <ChevronUp size={14}/> : <ChevronDown size={14}/>}
          onClick={() => setExpanded(open => !open)}>{expanded ? "收起" : "详情"}</Button>}
      </div>}
      {details && skippedCount > 0 && <p className="muted hint">生产跳过的帧不参与判定；全段试测仍测量这些帧供诊断查看，不计入其余帧的试测统计。</p>}
    </Surface>
    {seq != null && <FrameViewer controlsAt="footer" image={shown?.image ?? null} loading={!shown} error={shown?.error ?? ""} label={`${camera} · 段 ${segment} · 帧组 ${seq} · 试测`}
      overlay={{ path: view?.corridor ?? [], searchHalfPx: view?.searchHalfPx ?? undefined, segments: edges, clipped: evidenceOverlay(view?.metrics), partEdges: view?.partEdges, heading: guide && { ...guide, measured: metrics?.headingDeg ?? null } }}
      tools={<div className="wp-corridor-seq" role="group" aria-label="试测帧组">
        <IconButton size="sm" aria-label="试测上一帧组" disabled={!span || seq <= span.start} onClick={() => step(-1)}><ChevronLeft size={15}/></IconButton>
        <span>帧组 {seq}{seq === teachSeq ? "（示教帧）" : ""}</span>
        <IconButton size="sm" aria-label="试测下一帧组" disabled={!span || seq >= span.end} onClick={() => step(1)}><ChevronRight size={15}/></IconButton>
      </div>}
      summary={skipReason || (metrics && state) ? <span className="wp-trial-inline" aria-label="试测指标">
        {skipReason && <><Badge tone="neutral">生产跳过</Badge><span className="muted">{skipReason} · 不参与判定</span></>}
        {metrics && state && <>
        {skipReason ? <span className="muted">仅供诊断：{trialText[state]}</span> : <Badge tone={trialTone[state]}>{trialText[state]}</Badge>}
        <span className="muted">胶宽 <b>{mm(metrics.widthMean)}</b>（{mm(metrics.widthMin)}–{mm(metrics.widthMax)}）· 有胶 <b>{presentRatioText(metrics)}</b> · 最长断胶 {mm(metrics.gapMaxMm, 1)}
          {metrics.headingDeg != null && <> · 方向 <b>{metrics.headingDeg.toFixed(1)}°</b></>}{metrics.edgeMm != null && <> · 胶边距 {mm(metrics.edgeMm)}</>}</span>
        <MeasurementEvidenceSummary evidence={metrics.measurementEvidence} />
        {metrics.fallbackReason && <span className="c-warn">{metrics.measurementEvidence ? "定向说明" : "没找到胶条"}：{metrics.fallbackReason}</span>}
        {metrics.error && <span className="c-warn">{metrics.error}</span>}
        </>}
      </span> : undefined}
      emptyTitle="读取中" emptyHint="正在读这一帧的原图并试测。" />}
  </>;
}
