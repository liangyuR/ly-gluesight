import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { Button, Dialog, Select } from "qing-steel";
import type { PathSegment, WorkspacePlanView } from "../plan/types";
import { segmentKindLabel } from "../plan/vis";
import { errorText } from "../../lib/errors";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, GrayViewer, Notice, Panel } from "./components";
import { hasCalibration } from "./segmentEdit";
import { ignoreKey, stripGroups, Thumb, useThumbs } from "./thumbs";
import {
  initialDraft, moveCut, segmentsForSave, spans, structureChanged, toggleCut, toggleKind, valid,
  type WorkbenchDraft,
} from "./workbenchDraft";
import type { CaptureSummary, GrayImage } from "./types";

const reviewLabel = { draft: "边界草稿待确认", confirmed: "边界已确认", stale: "边界需复核" } as const;

/** 某台相机某个帧组的原图（预览用）。 */
function useFrame(roundId: string, camera: string, seq: number) {
  const key = `${roundId}|${camera}|${seq}`;
  const [state, setState] = useState<{ key: string; image: GrayImage | null; error: string }>({ key: "", image: null, error: "" });
  useEffect(() => {
    if (!roundId || seq < 1) return;
    let alive = true;
    workspaceApi.captureFrameImage(roundId, camera, seq)
      .then(image => alive && setState({ key, image, error: "" }))
      .catch(e => alive && setState({ key, image: null, error: errorText(e) }));
    return () => { alive = false; };
  }, [key]);
  return state.key === key ? { ...state, loading: false } : { image: null, error: "", loading: !!roundId && seq > 0 };
}

function FramePreview({ roundId, camera, seq, corridor, searchHalfPx }: { roundId: string; camera: string; seq: number; corridor: [number, number][]; searchHalfPx?: number }) {
  const { image, error, loading } = useFrame(roundId, camera, seq);
  return <GrayViewer image={image} loading={loading} error={error} label={`${camera} · 帧组 ${seq}`}
    overlay={corridor.length >= 2 ? { path: corridor, searchHalfPx } : undefined} />;
}

/**
 * 分段标定工作台（D-SB-44，段列表页顶部）：在一轮完整采集（模板轮）的帧组时间轴上定段结构与边界，一次保存为
 * 段列表 + 本轮已确认边界 + 分段模板。三台相机的缩略图按帧组排成三行；点帧间竖缝插入或删除边界，点段名切换直线 / 弯道，
 * 点缩略图在下方并排预览三台相机原图（叠加已示教走廊）。
 */
export default function SegmentWorkbench({ plan, locked, onDirty }: { plan: WorkspacePlanView; locked: string; onDirty: (dirty: boolean) => void }) {
  const { data, doc, act, busy } = useWorkspace();
  const [rounds, setRounds] = useState<CaptureSummary[] | null>(null);
  const [roundId, setRoundId] = useState("");
  const [loadError, setLoadError] = useState("");
  const [edit, setEdit] = useState<{ roundId: string; draft: WorkbenchDraft } | null>(null);
  const [seq, setSeq] = useState(1);
  const [confirming, setConfirming] = useState(false);
  // 按住一个已有边界拖到另一条竖缝上松开即移动它（指针事件，不用 HTML5 拖放：WebView2 里不可靠）
  const [dragging, setDragging] = useState<number | null>(null);
  useEffect(() => {
    if (dragging == null) return;
    const end = () => setDragging(null);
    window.addEventListener("pointerup", end);
    return () => window.removeEventListener("pointerup", end);
  }, [dragging]);
  const review = data?.workspace.captureReview;
  const template = review?.template ?? null;
  const docId = doc?.id ?? "";
  const revision = data?.workspace.revision ?? 0;
  const frozen = !!data?.workspace.pending;

  useEffect(() => {
    if (!docId) return;
    let alive = true;
    workspaceApi.captureList(docId)
      .then(list => {
        if (!alive) return;
        const complete = list.filter(round => round.state === "complete").sort((a, b) => a.createdAt - b.createdAt);
        setRounds(complete);
        setRoundId(current => current && complete.some(r => r.roundId === current) ? current
          : complete.find(r => r.roundId === template?.roundId)?.roundId ?? complete[0]?.roundId ?? "");
      })
      .catch(e => alive && setLoadError(errorText(e)));
    return () => { alive = false; };
  }, [docId]);

  const thumbs = useThumbs(roundId, plan.cameras);
  const ready = thumbs.strips;
  const groupCount = stripGroups(ready);
  const saved = review?.rounds[roundId] ?? null;
  const baseline = useMemo(
    () => groupCount ? initialDraft(groupCount, plan.segments.map(s => s.kind), saved) : null,
    [groupCount, JSON.stringify(plan.segments.map(s => s.kind)), JSON.stringify(saved?.cuts), saved?.status],
  );
  const draft = edit?.roundId === roundId ? edit.draft : baseline;
  const dirty = !!draft && !!baseline && JSON.stringify(draft) !== JSON.stringify(baseline);
  useEffect(() => { onDirty(dirty); }, [dirty]);
  useEffect(() => () => onDirty(false), []);

  const change = (next: WorkbenchDraft) => setEdit({ roundId, draft: next });
  // 快捷键只在工作台内有焦点时生效（下方对齐复核也用方向键）
  const onKey = (e: KeyboardEvent) => {
    if (!draft || ignoreKey(e)) return;
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      setSeq(Math.min(groupCount, Math.max(1, seq + (e.key === "ArrowLeft" ? -1 : 1))));
    } else if (e.key === "b" || e.key === "B") {
      e.preventDefault();
      change(toggleCut(draft, seq));
    }
  };
  useEffect(() => { setSeq(1); }, [roundId]);

  if (rounds && !rounds.length) {
    return <Panel title="分段标定" detail="在一轮完整采集的帧组时间轴上划分直线段与弯道段">
      <Notice title="还没有完整的整圈采集" tone="warn">先在“整圈采集”完成至少一轮，再回到这里按图像划分段。</Notice>
    </Panel>;
  }

  const segmentSpans = draft ? spans(draft) : [];
  const restructured = !!draft && structureChanged(plan.segments, draft.kinds);
  const affected = Object.entries(review?.rounds ?? {}).filter(([id, r]) => id !== roundId && r.status !== "stale").map(([id]) => id);
  const calibrated = hasCalibration(plan.segments, plan.partCalib != null);
  const isTemplate = template?.roundId === roundId && saved?.status === "confirmed";
  const blocked = !draft || !valid(draft) || busy || frozen || !!locked || !doc;
  const save = async () => {
    if (!draft || !doc) return;
    const segments: PathSegment[] = segmentsForSave(plan.segments, draft.kinds, plan.segmentDefaults);
    const next = await act(() => workspaceApi.segmentTemplateSave(doc.id, revision, roundId, segments, draft.cuts), "分段已保存，本轮设为分段模板");
    setConfirming(false);
    if (next) setEdit(null);
  };
  const requestSave = () => {
    if (restructured && (affected.length > 0 || calibrated)) setConfirming(true);
    else void save();
  };
  const inspect = (camera: string) => doc?.inspect?.find(item => item.camera === camera);
  const halfPx = (camera: string) => {
    const item = inspect(camera);
    const mmPerPx = item?.mmPerPx;
    return mmPerPx && mmPerPx > 0 && doc ? (item?.detect ?? doc.detect).searchMm / mmPerPx : undefined;
  };
  const columns = `56px repeat(${groupCount}, var(--swb-cell))`;
  const roundOptions = (rounds ?? []).map(round => ({
    value: round.roundId,
    label: `${round.roundId} · ${new Date(round.createdAt).toLocaleString("zh-CN")} · ${round.receivedCount} 帧组${round.roundId === template?.roundId ? " · 模板" : ""}`,
  }));

  return <Panel title="分段标定" detail="在模板轮的帧组时间轴上划分直线段与弯道段：点帧间竖缝插入或删除边界，点段名切换类型，点缩略图预览三台相机原图；← → 切换帧组，B 在当前帧组后插入或删除边界"
    actions={<Select aria-label="模板轮" value={roundId} options={roundOptions} disabled={busy || dirty} onValueChange={value => { setEdit(null); setRoundId(value); }} />}>
    {(loadError || thumbs.error) && <Notice title="读取采集图像失败" tone="warn">{loadError || thumbs.error}</Notice>}
    {frozen && <Notice title="待生效，候选已冻结">等待当前工件结束后切换；生效或放弃后再修改分段。</Notice>}
    {!ready && !loadError && !thumbs.error && <p className="muted">正在读取缩略图…</p>}
    {draft && ready && <div className="swb-body" onKeyDown={onKey}>
      <div className="swb-summary" role="status" aria-label="分段草稿">
        <span>{groupCount} 个帧组 · {segmentSpans.length} 段 · 边界 {draft.cuts.length ? draft.cuts.join(" / ") : "无"}</span>
        {saved && <Badge tone={saved.status === "confirmed" ? "ok" : "warn"}>{reviewLabel[saved.status]}</Badge>}
        {isTemplate && !dirty && <Badge tone="ok">分段模板</Badge>}
        {restructured && <Badge tone="warn">段结构有改动</Badge>}
      </div>
      <div className="swb-scroll">
        <div className="swb-grid" style={{ gridTemplateColumns: columns }}>
          <span className="swb-label">段</span>
          {segmentSpans.map(([first, last], i) => {
            const kind = draft.kinds[i];
            return <button key={`${first}-${kind}`} type="button" className={`swb-seg ${kind}`} style={{ gridColumn: `${first + 1} / ${last + 2}` }}
              aria-label={`段 ${i + 1}（${segmentKindLabel[kind]}，帧组 ${first}–${last}），点击切换类型`} disabled={frozen} onClick={() => change(toggleKind(draft, i))}>
              段 {i + 1} · {segmentKindLabel[kind]} · {last - first + 1} 组
            </button>;
          })}
          {plan.cameras.map(camera => {
            const strip = ready[camera];
            return [<span key={`${camera}-label`} className="swb-label">{camera}</span>,
              ...Array.from({ length: groupCount }, (_, k) => {
                const group = k + 1;
                const pixels = strip?.frames[k] ?? null;
                const cut = draft.cuts.includes(group);
                return <button key={`${camera}-${group}`} type="button" aria-label={`${camera} 帧组 ${group}`} aria-pressed={group === seq}
                  className={`swb-cell${group === seq ? " selected" : ""}${cut ? " cut" : ""}`} onClick={() => setSeq(group)}>
                  <Thumb pixels={pixels} width={strip?.width ?? 0} height={strip?.height ?? 0} />
                </button>;
              })];
          })}
          <span className="swb-label">边界</span>
          {Array.from({ length: groupCount }, (_, k) => {
            const group = k + 1;
            const at = draft.cuts.indexOf(group);
            if (group === groupCount) return <span key="end" className="swb-seam-end">{group}</span>;
            return <button key={`seam-${group}`} type="button" className={`swb-seam${at >= 0 ? " on" : ""}${dragging != null && dragging !== at ? " target" : ""}`} disabled={frozen}
              aria-label={at >= 0 ? `删除帧组 ${group} 之后的边界` : `在帧组 ${group} 之后插入边界`}
              onPointerDown={() => { if (at >= 0) setDragging(at); }}
              onPointerUp={() => { if (dragging != null && draft.cuts[dragging] !== group) change(moveCut(draft, dragging, group)); }}
              onClick={() => change(toggleCut(draft, group))}>{group}</button>;
          })}
        </div>
      </div>
      <div className="swb-preview" aria-label={`帧组 ${seq} 三台相机原图`}>
        {plan.cameras.map(camera => <FramePreview key={camera} roundId={roundId} camera={camera} seq={seq}
          corridor={inspect(camera)?.corridor ?? []} searchHalfPx={halfPx(camera)} />)}
      </div>
      {locked && <p className="muted">{locked}</p>}
      <div className="wp-actions">
        <Button disabled={!dirty || busy} onClick={() => setEdit(null)}>撤销改动</Button>
        <Button variant="filled" disabled={blocked} onClick={requestSave}>{isTemplate && !dirty ? "重新保存分段" : "保存分段并设为模板"}</Button>
      </div>
    </div>}
    {confirming && <Dialog open title="段结构将改变" onOpenChange={open => { if (!open) setConfirming(false); }}
      footer={<><Button disabled={busy} onClick={() => setConfirming(false)}>取消</Button><Button variant="filled" disabled={busy} onClick={() => void save()}>保存并设为模板</Button></>}>
      <div className="wp-stack">
        <p>段数、顺序或类型与现有段列表不同，保存后：</p>
        <ul>
          {affected.length > 0 && <li>其余 {affected.length} 轮已整理的边界变为“需复核”，需按新结构重新套用模板并确认；</li>}
          {calibrated && <li>各段预期帧数、整件预期总帧数与胶偏参考清空，需重新应用多轮标定；</li>}
          <li>本轮边界确认并设为分段模板。</li>
        </ul>
      </div>
    </Dialog>}
  </Panel>;
}
