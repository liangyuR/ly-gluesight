import { useEffect, useState, type KeyboardEvent } from "react";
import { Button, Select } from "qing-steel";
import { errorText } from "../../lib/errors";
import type { CalibrationFrameView, FrameMetrics, FrameState, WorkspacePlanView } from "../plan/types";
import { groupCellColor, groupCellText, segmentKindLabel, type GroupCellState } from "../plan/vis";
import { workspaceApi } from "./api";
import { Badge, GrayViewer, KV, Panel } from "./components";
import { pieceLabel } from "./pieces";
import type { GrayImage, WorkspaceView } from "./types";

const cellState: Record<FrameState, GroupCellState> = {
  measured: "ok", missing: "missing", error: "error", unmeasured: "unmeasured", outOfCorridor: "outOfCorridor", notInspected: "notInspected",
};
const mm = (value: number | null | undefined, digits = 2) => value == null ? "—" : `${value.toFixed(digits)} mm`;

/**
 * 多轮标定页的逐帧查看：选一件已加入标定的标定件，按帧组 × 计划相机列出加入标定时测得的每帧状态（帧组上方标出按已确认边界
 * 所属的段）；点一帧读原图，并用当前段走廊重测（只读），叠出走廊、搜索带与测到的两侧胶边，旁边列胶宽、x 等指标。
 */
export function CalibrationFrames({ data, plan }: { data: WorkspaceView; plan: WorkspacePlanView }) {
  const id = data.workspace.doc.id;
  const rounds = plan.calibration?.rounds ?? [];
  const reviews = data.workspace.captureReview?.rounds ?? {};
  const [roundId, setRoundId] = useState(rounds[0]?.roundId ?? "");
  const round = rounds.find(item => item.roundId === roundId) ?? rounds[0];
  const [features, setFeatures] = useState<{ round: string; frames: FrameMetrics[]; error: string }>({ round: "", frames: [], error: "" });
  const [selected, setSelected] = useState<{ camera: number; seq: number } | null>(null);
  const [detail, setDetail] = useState<{ key: string; image: GrayImage | null; view: CalibrationFrameView | null; error: string; loading: boolean }>({ key: "", image: null, view: null, error: "", loading: false });

  useEffect(() => {
    if (!round) return;
    let alive = true;
    setSelected(null);
    workspaceApi.calibrationRoundFeatures(id, round.roundId)
      .then(frames => alive && setFeatures({ round: round.roundId, frames, error: "" }))
      .catch(e => alive && setFeatures({ round: round.roundId, frames: [], error: errorText(e) }));
    return () => { alive = false; };
  }, [id, round?.roundId]);

  const camera = selected ? plan.cameras[selected.camera] : undefined;
  const key = JSON.stringify([id, round?.roundId ?? null, camera ?? null, selected?.seq ?? null]);
  useEffect(() => {
    if (!round || !selected || !camera) return;
    let alive = true;
    setDetail({ key, image: null, view: null, error: "", loading: true });
    Promise.allSettled([workspaceApi.captureFrameImage(round.roundId, camera, selected.seq), workspaceApi.calibrationFrame(id, round.roundId, camera, selected.seq)])
      .then(([image, view]) => alive && setDetail({ key, loading: false,
        image: image.status === "fulfilled" ? image.value : null,
        view: view.status === "fulfilled" ? view.value : null,
        error: [image, view].flatMap(r => r.status === "rejected" ? [errorText(r.reason)] : []).join("；") }));
    return () => { alive = false; };
  }, [key]);

  if (!round) return null;
  const groups = round.groupCount;
  const cuts = reviews[round.roundId]?.status === "confirmed" ? reviews[round.roundId].cuts : null;
  const segmentOf = (seq: number) => cuts ? plan.segments[cuts.filter(cut => cut < seq).length] : plan.segments.length === 1 ? plan.segments[0] : undefined;
  const loaded = features.round === round.roundId;
  const stored = (cam: number, seq: number) => loaded ? features.frames.find(f => f.camera === cam && f.seq === seq) : undefined;
  const label = (roundIdValue: string) => {
    const piece = plan.pieces.find(item => item.roundId === roundIdValue);
    return piece ? pieceLabel(piece) : "来源未记录";
  };
  const move = (event: KeyboardEvent) => {
    if (!selected) return;
    const delta: Record<string, [number, number]> = { ArrowLeft: [0, -1], ArrowRight: [0, 1], ArrowUp: [-1, 0], ArrowDown: [1, 0] };
    const step = delta[event.key];
    if (!step) return;
    event.preventDefault();
    setSelected({ camera: Math.min(plan.cameras.length - 1, Math.max(0, selected.camera + step[0])), seq: Math.min(groups, Math.max(1, selected.seq + step[1])) });
  };

  const shown = detail.key === key ? detail : { image: null, view: null, error: "", loading: !!selected };
  const view = shown.view;
  const before = selected ? stored(selected.camera, selected.seq) : undefined;
  const segment = selected ? segmentOf(selected.seq) : undefined;
  const edges = (view?.edges ?? []).flatMap(edge => edge ? [{ from: edge[0], to: edge[1], color: "var(--primary)" }] : []);
  const metrics = view?.metrics;

  return <Panel title="逐帧查看" help="加入标定时测得的每帧状态；点一帧看原图，并用当前段走廊重测（只读，不改标定）。方向键切换帧组、相机">
    <div className="wp-calib-frames">
      <Select label="标定件" value={round.roundId} options={rounds.map(item => ({ value: item.roundId, label: `${label(item.roundId)} · ${item.groupCount} 帧组${item.standard ? " · 标准件" : ""}` }))} onValueChange={setRoundId} />
      {features.error && loaded && <p className="c-warn" role="alert">{features.error}</p>}
      {!cuts && plan.segments.length > 1 && <p className="muted hint">这一件的边界已不是确认状态：帧组上方不标段。</p>}
      <div className="wp-calib-grid" role="grid" aria-label="逐帧状态" tabIndex={0} onKeyDown={move} style={{ gridTemplateColumns: `56px repeat(${groups}, minmax(14px, 1fr))` }}>
        <span className="wp-calib-grid-corner">段</span>
        {Array.from({ length: groups }, (_, i) => {
          const s = segmentOf(i + 1), first = s && (i === 0 || segmentOf(i)?.index !== s.index);
          return <span key={i} className={"wp-calib-grid-seg" + (s && s.index % 2 === 0 ? " even" : "")} title={s ? `第 ${s.index} 段（${segmentKindLabel[s.kind]}）· 帧组 ${i + 1}` : `帧组 ${i + 1}`}>{first ? s.index : ""}</span>;
        })}
        {plan.cameras.map((cameraId, cam) => <div key={cameraId} role="row" className="wp-calib-grid-row">
          <span className="wp-calib-grid-camera" role="rowheader">{cameraId}</span>
          {Array.from({ length: groups }, (_, i) => {
            const seq = i + 1, m = stored(cam, seq), state = m ? cellState[m.state] : "waiting";
            const active = selected?.camera === cam && selected.seq === seq;
            return <Button variant="text" key={seq} role="gridcell" aria-selected={active} className={"wp-calib-cell" + (active ? " selected" : "")}
              style={{ background: m ? groupCellColor[state] : "var(--surface-high)" }} aria-label={`${cameraId} 帧组 ${seq}：${m ? groupCellText[state] : "无记录"}`}
              title={`${cameraId} · 帧组 ${seq} · ${m ? groupCellText[state] : "无记录"}${m?.error ? `：${m.error}` : ""}`} onClick={() => setSelected({ camera: cam, seq })} />;
          })}
        </div>)}
      </div>
      {selected && camera && <div className="wp-calib-frame">
        <GrayViewer image={shown.image} loading={shown.loading} error={shown.error} label={`${camera} · 帧组 ${selected.seq}`}
          overlay={view && view.corridor.length >= 2 ? { path: view.corridor, searchHalfPx: view.searchHalfPx ?? undefined, segments: edges } : undefined}
          emptyTitle="读取中" emptyHint="正在读这一帧的原图并重测。" />
        <div className="wp-calib-frame-info" aria-label="这一帧的指标">
          <KV label="所在段">{segment ? `第 ${segment.index} 段 · ${segmentKindLabel[segment.kind]}` : "—"}</KV>
          <KV label="加入标定时">{before ? <Badge tone={before.state === "measured" ? "ok" : "neutral"}>{groupCellText[cellState[before.state]]}</Badge> : "—"}</KV>
          {view && <>
            <KV label="重测走廊">{view.segment == null ? "本段不由这台相机检测，不测" : `第 ${view.segment} 段走廊`}</KV>
            {metrics && view.segment != null && <>
              <KV label="重测状态"><Badge tone={metrics.state === "measured" ? "ok" : "warn"}>{groupCellText[cellState[metrics.state]]}</Badge></KV>
              <KV label="胶宽 最小 / 均值 / 最大">{`${mm(metrics.widthMin)} / ${mm(metrics.widthMean)} / ${mm(metrics.widthMax)}`}</KV>
              <KV label="胶中心偏移 x">{mm(metrics.xMm)}</KV>
              {metrics.headingDeg != null && <KV label="胶条方向">{`${metrics.headingDeg.toFixed(1)}°`}</KV>}
              {metrics.headingFallback && <KV label="定向"><Badge tone="warn">定向回退</Badge>{metrics.fallbackReason && <span className="muted"> {metrics.fallbackReason}</span>}</KV>}
              {metrics.edgeMm != null && <KV label="胶边距">{mm(metrics.edgeMm)}</KV>}
              <KV label="最长断胶">{mm(metrics.gapMaxMm, 1)}</KV>
              <KV label="有胶站">{`${Math.round(metrics.presentRatio * 100)}%（${metrics.widths.filter(w => w != null).length} / ${metrics.widths.length} 站，站距 ${metrics.spacingMm} mm）`}</KV>
              {metrics.error && <p className="muted hint">{metrics.error}</p>}
              {before && before.state !== metrics.state && <p className="muted hint">与加入标定时的状态不同：走廊或参数在加入标定后改过。</p>}
            </>}
          </>}
        </div>
      </div>}
    </div>
  </Panel>;
}
