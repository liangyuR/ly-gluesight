import { useEffect, useState, type KeyboardEvent } from "react";
import { Button, IconButton, Surface } from "qing-steel";
import { ChevronLeft, ChevronRight, Play } from "lucide-react";
import { errorText } from "../../lib/errors";
import type { CalibrationFrameView, FrameMetrics, TeachPreviewSegment } from "../plan/types";
import { workspaceApi } from "./api";
import { Badge, GrayViewer } from "./components";
import type { HeadingGuide } from "./headingOverlay";
import type { GrayImage } from "./types";

type TrialState = "ok" | "fallback" | "empty" | "out" | "error" | "missing";
const trialOrder: TrialState[] = ["ok", "fallback", "empty", "out", "error", "missing"];
const trialText: Record<TrialState, string> = { ok: "测到", fallback: "没找到胶条", empty: "无胶", out: "出走廊", error: "测量失败", missing: "缺帧或未测" };
const trialColor: Record<TrialState, string> = {
  ok: "var(--ok)", fallback: "var(--warn)", empty: "var(--ng)", out: "var(--on-surface-disabled)", error: "var(--fault)", missing: "var(--outline-variant)",
};
const trialTone: Record<TrialState, "ok" | "warn" | "ng" | "neutral"> = { ok: "ok", fallback: "warn", empty: "ng", out: "neutral", error: "ng", missing: "neutral" };

function trialState(m: FrameMetrics): TrialState {
  if (m.state === "measured") return m.headingFallback ? "fallback" : m.presentRatio > 0 ? "ok" : "empty";
  return m.state === "outOfCorridor" ? "out" : m.state === "error" ? "error" : "missing";
}
const mm = (value: number | null | undefined, digits = 2) => value == null ? "—" : `${value.toFixed(digits)} mm`;

/**
 * 示教页同段试测（只读）：用候选已保存的走廊与定向参数，测示教轮次里本段的帧。“全段试测”给出逐帧状态条；
 * 选中一帧（`seq`）时显示这一帧的原图与测量叠加，代替示教画布。候选修订号变了，全段结果标为过期，选中的帧自动重测。
 */
export function SegmentTrial({ id, camera, segment, round, teachSeq, range, revision, locked, guide, seq, onSeq }: {
  id: string; camera: string; segment: number; round: string; teachSeq: number; range: { start: number; end: number } | null; revision: number;
  locked: string; guide: HeadingGuide | null; seq: number | null; onSeq: (seq: number | null) => void;
}) {
  const [batch, setBatch] = useState<{ result: TeachPreviewSegment | null; revision: number; error: string; running: boolean }>({ result: null, revision, error: "", running: false });
  const run = async () => {
    if (locked || batch.running) return;
    const at = revision;
    setBatch(previous => ({ ...previous, running: true, error: "" }));
    try {
      const result = await workspaceApi.teachPreviewSegment(id, camera, segment, round);
      setBatch({ result, revision: at, error: "", running: false });
    } catch (e) { setBatch(previous => ({ ...previous, running: false, error: errorText(e) })); }
  };
  const span = batch.result ? { start: batch.result.start, end: batch.result.end } : range;
  const stale = !!batch.result && batch.revision !== revision;
  const cells = span ? Array.from({ length: span.end - span.start + 1 }, (_, i) => {
    const frame = batch.result?.frames.find(item => item.seq === span.start + i);
    return { seq: span.start + i, state: frame ? trialState(frame) : null, note: frame?.error ?? frame?.fallbackReason ?? null };
  }) : [];

  const key = seq == null ? "" : JSON.stringify([id, camera, segment, round, seq, revision]);
  const [detail, setDetail] = useState<{ key: string; image: GrayImage | null; view: CalibrationFrameView | null; error: string }>({ key: "", image: null, view: null, error: "" });
  useEffect(() => {
    if (seq == null || locked) return;
    let alive = true;
    void Promise.allSettled([workspaceApi.captureFrameImage(round, camera, seq), workspaceApi.teachPreviewFrame(id, camera, segment, round, seq)])
      .then(([image, view]) => alive && setDetail({ key,
        image: image.status === "fulfilled" ? image.value : null,
        view: view.status === "fulfilled" ? view.value : null,
        error: [image, view].flatMap(r => r.status === "rejected" ? [errorText(r.reason)] : []).join("；") }));
    return () => { alive = false; };
  }, [key, !!locked]);
  const shown = detail.key === key ? detail : null;
  const view = shown?.view ?? null;
  const metrics = view?.metrics;
  const state = metrics ? trialState(metrics) : null;
  const step = (delta: number) => {
    if (seq == null || !span) return;
    const next = Math.min(span.end, Math.max(span.start, seq + delta));
    if (next !== seq) onSeq(next);
  };
  const move = (event: KeyboardEvent) => {
    if (seq == null || (event.key !== "ArrowLeft" && event.key !== "ArrowRight")) return;
    event.preventDefault();
    step(event.key === "ArrowLeft" ? -1 : 1);
  };
  const edges = (view?.edges ?? []).flatMap(edge => edge ? [{ from: edge[0], to: edge[1], color: "var(--primary)" }] : []);
  const counts = batch.result ? trialOrder.map(item => [item, cells.filter(cell => cell.state === item).length] as const).filter(([, n]) => n > 0) : [];

  return <>
    <Surface role="region" className="wp-trial" aria-label="本段试测" onKeyDown={move}>
      <div className="wp-trial-head">
        <h3>本段试测</h3>
        <Button size="sm" variant={batch.result && !stale ? "tonal" : "filled"} data-variant={batch.result && !stale ? "tonal" : "filled"} leadingIcon={<Play size={13}/>}
          disabled={!!locked || batch.running} title={locked || undefined} onClick={() => void run()}>{batch.running ? "试测中…" : batch.result ? "重新试测" : "全段试测"}</Button>
        <Button size="sm" variant="tonal" disabled={!!locked || seq === teachSeq} title={locked || undefined} onClick={() => onSeq(teachSeq)}>试测当前帧</Button>
        <span className="muted">示教件{span ? ` · 帧组 ${span.start}–${span.end}` : ""} · 用已保存的走廊与定向参数，只读</span>
        {seq != null && <span className="wp-trial-actions"><Button size="sm" variant="text" onClick={() => onSeq(null)}>回到示教帧</Button></span>}
      </div>
      {locked && <p className="muted hint">{locked}</p>}
      {batch.error && <p className="c-warn" role="alert">{batch.error}</p>}
      {span
        ? <div className={"wp-trial-strip" + (stale ? " stale" : "")} role="group" aria-label="逐帧试测状态" style={{ gridTemplateColumns: `repeat(${cells.length}, minmax(6px, 1fr))` }}>
          {cells.map(cell => {
            const text = `帧组 ${cell.seq}：${cell.state ? trialText[cell.state] : "未试测"}${cell.seq === teachSeq ? "（示教帧）" : ""}`;
            return <Button variant="text" size="sm" key={cell.seq} type="button" aria-pressed={cell.seq === seq} aria-label={text} title={cell.note ? `${text}：${cell.note}` : text} disabled={!!locked}
              className={"wp-trial-cell" + (cell.seq === seq ? " selected" : "") + (cell.seq === teachSeq ? " teach" : "")}
              style={{ background: cell.state ? trialColor[cell.state] : "var(--surface-high)" }} onClick={() => onSeq(cell.seq)} />;
          })}
        </div>
        : <p className="muted hint">{batch.running ? "正在逐帧试测本段…" : "“试测当前帧”只测示教帧；“全段试测”测本段全部帧，得到逐帧状态条，之后可点任一帧看原图与测量叠加。"}</p>}
      {span && !batch.result && !batch.running && <p className="muted hint">点格子试测那一帧；“全段试测”给每一格标上状态。</p>}
      {batch.result && <div className="wp-trial-legend">
        {counts.map(([item, n]) => <span key={item}><i style={{ background: trialColor[item] }} aria-hidden="true" />{trialText[item]} {n}</span>)}
        <span><i className="teach" aria-hidden="true" />示教帧</span>
        {stale && <span className="c-warn">参数已修改，结果已过期，请重新试测</span>}
      </div>}
    </Surface>
    {seq != null && <GrayViewer controlsAt="footer" image={shown?.image ?? null} loading={!shown} error={shown?.error ?? ""} label={`${camera} · 段 ${segment} · 帧组 ${seq} · 试测`}
      overlay={{ path: view?.corridor ?? [], searchHalfPx: view?.searchHalfPx ?? undefined, segments: edges, heading: guide && { ...guide, measured: metrics?.headingDeg ?? null } }}
      tools={<div className="wp-corridor-seq" role="group" aria-label="试测帧组">
        <IconButton size="sm" aria-label="试测上一帧组" disabled={!span || seq <= span.start} onClick={() => step(-1)}><ChevronLeft size={15}/></IconButton>
        <span>帧组 {seq}{seq === teachSeq ? "（示教帧）" : ""}</span>
        <IconButton size="sm" aria-label="试测下一帧组" disabled={!span || seq >= span.end} onClick={() => step(1)}><ChevronRight size={15}/></IconButton>
      </div>}
      summary={metrics && state ? <span className="wp-trial-inline" aria-label="试测指标">
        <Badge tone={trialTone[state]}>{trialText[state]}</Badge>
        <span className="muted">胶宽 <b>{mm(metrics.widthMean)}</b>（{mm(metrics.widthMin)}–{mm(metrics.widthMax)}）· 有胶 <b>{Math.round(metrics.presentRatio * 100)}%</b> · 最长断胶 {mm(metrics.gapMaxMm, 1)}
          {metrics.headingDeg != null && <> · 方向 <b>{metrics.headingDeg.toFixed(1)}°</b></>}{metrics.edgeMm != null && <> · 胶边距 {mm(metrics.edgeMm)}</>}</span>
        {metrics.fallbackReason && <span className="c-warn">没找到胶条：{metrics.fallbackReason}</span>}
        {metrics.error && <span className="c-warn">{metrics.error}</span>}
      </span> : undefined}
      emptyTitle="读取中" emptyHint="正在读这一帧的原图并试测。" />}
  </>;
}
