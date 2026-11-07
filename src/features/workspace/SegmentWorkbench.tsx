import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { Link } from "react-router-dom";
import { ArrowRight } from "lucide-react";
import { Button, Dialog } from "qing-steel";
import type { PathSegment, WorkspacePlanView } from "../plan/types";
import { segmentLabel } from "../plan/vis";
import { errorText } from "../../lib/errors";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, FrameViewer, Notice, Panel } from "./components";
import { ignoreKey, stripGroups, Thumb, useThumbs } from "./thumbs";
import {
  initialDraft, moveCut, passCells, segmentsForSave, spans, structureChanged, toggleCamera, toggleCut, toggleKind, valid,
  type WorkbenchDraft,
} from "./workbenchDraft";
import type { FrameImage, SegmentSaveImpact } from "./types";
import WorkspaceNavigation from "./WorkspaceNavigation";
import SegmentCoveragePreview from "./SegmentCoveragePreview";
import { useRecipeHref } from "./recipeRoutes";

const reviewLabel = { draft: "边界草稿待确认", confirmed: "边界已确认", stale: "边界需复核" } as const;

/** 某台相机某个帧组的原图（预览用）。 */
function useFrame(roundId: string, camera: string, seq: number) {
  const key = `${roundId}|${camera}|${seq}`;
  const [state, setState] = useState<{ key: string; image: FrameImage | null; error: string }>({ key: "", image: null, error: "" });
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
  return <FrameViewer image={image} loading={loading} error={error} label={`${camera} · 帧组 ${seq}`}
    overlay={corridor.length >= 2 ? { path: corridor, searchHalfPx } : undefined} />;
}

/**
 * 分段标定工作台（D-SB-44、RB-1，段列表页顶部）：在示教件的帧组时间轴上定段结构与边界，一次保存为
 * 段列表 + 示教件已确认边界。三台相机的缩略图按帧组排成三行；点帧间竖缝插入或删除边界，点段名切换直线 / 弯道，
 * 点缩略图在下方并排预览三台相机原图（叠加已示教走廊）。
 */
export default function SegmentWorkbench({ plan, locked, onDirty }: { plan: WorkspacePlanView; locked: string; onDirty: (dirty: boolean) => void }) {
  const href = useRecipeHref();
  const { data, doc, act, busy, navigationDirty, setError } = useWorkspace();
  const roundId = data?.workspace.teachingPiece?.roundId ?? "";
  const [edit, setEdit] = useState<{ roundId: string; draft: WorkbenchDraft } | null>(null);
  const [seq, setSeq] = useState(1);
  const [confirming, setConfirming] = useState<SegmentSaveImpact | null>(null);
  const [previewing, setPreviewing] = useState(false);
  // 按住一个已有边界拖到另一条竖缝上松开即移动它（指针事件，不用 HTML5 拖放：WebView2 里不可靠）
  const [dragging, setDragging] = useState<number | null>(null);
  useEffect(() => {
    if (dragging == null) return;
    const end = () => setDragging(null);
    window.addEventListener("pointerup", end);
    return () => window.removeEventListener("pointerup", end);
  }, [dragging]);
  const review = data?.workspace.captureReview;
  const revision = data?.workspace.revision ?? 0;
  const frozen = !!data?.workspace.pending || navigationDirty;

  const thumbs = useThumbs(roundId, plan.cameras);
  const ready = thumbs.strips;
  const groupCount = stripGroups(ready);
  const saved = review?.rounds[roundId] ?? null;
  const baseline = useMemo(
    () => groupCount ? initialDraft(groupCount, plan.segments, plan.cameras, saved) : null,
    [groupCount, JSON.stringify(plan.segments.map(s => [s.kind, s.cameras, !!s.skip])), plan.cameras.join("|"), JSON.stringify(saved?.cuts), saved?.status],
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
    } else if ((e.key === "b" || e.key === "B") && !frozen) {
      e.preventDefault();
      change(toggleCut(draft, seq));
    }
  };
  useEffect(() => { setSeq(1); }, [roundId]);

  if (!roundId) {
    return <><WorkspaceNavigation segments={plan.segments} ranges={null} seq={1} frameStatus="unavailable" editable locked={locked} onNavigate={() => {}} />
    <Panel title="分段标定" help="在示教件的帧组时间轴上划分直线段与弯道段">
      <Notice title="还没有示教件" tone="warn">先选一件历史件设为示教件，再回到这里按它的图像划分段。<Link className="wp-text-link" to={href("/recipe/capture")}>前往示教件<ArrowRight size={12} aria-hidden="true" /></Link></Notice>
    </Panel></>;
  }

  const segmentSpans = draft ? spans(draft) : [];
  const restructured = !!draft && structureChanged(plan.segments, draft.kinds);
  const isConfirmed = saved?.status === "confirmed";
  const blocked = !draft || !valid(draft) || busy || previewing || frozen || !!locked || !doc;
  const save = async () => {
    if (!draft || !doc) return;
    const segments: PathSegment[] = segmentsForSave(plan.segments, draft, plan.segmentDefaults, plan.cameras);
    const next = await act(() => workspaceApi.segmentTemplateSave(doc.id, revision, segments, draft.cuts), "分段已保存，示教件边界已确认");
    setConfirming(null);
    if (next) setEdit(null);
  };
  // 先预演检测覆盖及保存影响，再确认保存
  const requestSave = async () => {
    if (!draft || !doc) return;
    const segments: PathSegment[] = segmentsForSave(plan.segments, draft, plan.segmentDefaults, plan.cameras);
    setPreviewing(true); setError("");
    let impact: SegmentSaveImpact;
    try { impact = await workspaceApi.segmentTemplatePreview(doc.id, revision, segments, draft.cuts); }
    catch (e) { setError(errorText(e)); return; }
    finally { setPreviewing(false); }
    if (impact.coverage || impact.restructured || impact.dropped.length || impact.measurementCleared) setConfirming(impact);
    else void save();
  };
  const inspect = (camera: string) => doc?.inspect?.find(item => item.camera === camera);
  // 预览叠加选中帧组所在段的走廊（段结构有改动时还没对应到新段，不叠加）
  const seqSegment = restructured ? undefined : plan.segments[segmentSpans.findIndex(([first, last]) => seq >= first && seq <= last)];
  const corridorOf = (camera: string) => inspect(camera)?.segments?.find(item => item.segment === seqSegment?.index)?.corridor ?? [];
  const halfPx = (camera: string) => {
    const item = inspect(camera);
    const mmPerPx = item?.mmPerPx;
    return mmPerPx && mmPerPx > 0 && doc ? (item?.detect ?? doc.detect).searchMm / mmPerPx : undefined;
  };
  const columns = `56px repeat(${groupCount}, var(--swb-cell))`;
  // 示教件的笔结构（D-SB-62）：笔间的竖缝加粗，便于把段边界放在笔间（笔间边界不留过渡区）；每笔首尾跳过的帧组按跳过标出
  const passOf = passCells(data?.workspace.teachingPiece?.passes, groupCount, doc?.passSkip);
  const passSkipText = { head: "开头跳过", tail: "结尾跳过", short: "太短，整笔不检" } as const;
  const navigation = <WorkspaceNavigation segments={plan.segments} ranges={!restructured && draft && valid(draft) ? segmentSpans : null}
    seq={seq} currentSegment={seqSegment?.index} frameStatus={restructured || !draft ? "unavailable" : dirty || !isConfirmed ? "draft" : "confirmed"}
    editable locked={locked || (dirty ? "先保存或撤销分段草稿，再编辑地图" : "")}
    onNavigate={(_, nextSeq) => setSeq(nextSeq)} />;

  return <Panel title="分段标定" help="在示教件的帧组时间轴上划分直线段与弯道段：点缩略图选中帧组（下方并排显示三台相机原图），再点“在帧组 N 后插入边界”（或按 B，或点时间轴下方对应的 ＋）；点段名按 直线 → 弯道 → 不检测 循环切换（不检测的段照常计帧，不测不判，结果列为未检区域）；点段下方的相机名选这段由哪几台相机检测（没选中的相机变暗，判定时不用）；← → 切换帧组">
    {thumbs.error && <Notice title="读取示教件图像失败" tone="warn">{thumbs.error}</Notice>}
    {!!data?.workspace.pending && <Notice title="待生效，候选已冻结">等待当前工件结束后切换；生效或放弃后再修改分段。</Notice>}
    {!ready && !thumbs.error && <p className="muted">正在读取缩略图…</p>}
    {draft && ready && <div className="swb-body" onKeyDown={onKey}>
      {/* 操作放在时间轴上方：下方原图预览加载时会改变高度，按钮放在下面会被挤动而点错 */}
      <div className="swb-toolbar">
        <div className="swb-summary" role="status" aria-label="分段草稿">
          <span>{groupCount} 个帧组{passOf && ` · ${data?.workspace.teachingPiece?.passes?.length} 笔`} · {segmentSpans.length} 段 · 边界 {draft.cuts.length ? draft.cuts.join(" / ") : "无"}</span>
          {saved && <Badge tone={saved.status === "confirmed" ? "ok" : "warn"}>{reviewLabel[saved.status]}</Badge>}
          {restructured && <Badge tone="warn">段结构有改动</Badge>}
          {!restructured && dirty && JSON.stringify([draft.cameras, draft.skips]) !== JSON.stringify([baseline?.cameras, baseline?.skips]) && <Badge tone="warn">检测相机有改动</Badge>}
          {!valid(draft) && draft.skips.every(Boolean) && <Badge tone="warn">至少要有一段检测</Badge>}
        </div>
        <div className="wp-actions">
          {locked && <span className="muted">{locked}</span>}
          {seq < groupCount && <Button disabled={frozen} onClick={() => change(toggleCut(draft, seq))}>
            {draft.cuts.includes(seq) ? `删除帧组 ${seq} 后的边界` : `在帧组 ${seq} 后插入边界`}
          </Button>}
          <Button disabled={!dirty || busy} onClick={() => setEdit(null)}>撤销改动</Button>
          <Button variant="filled" disabled={blocked} onClick={() => void requestSave()}>{isConfirmed && !dirty ? "重新保存分段" : "保存分段"}</Button>
        </div>
      </div>
      <div className="swb-scroll">
        <div className="swb-grid" style={{ gridTemplateColumns: columns }}>
          <span className="swb-label">段</span>
          {segmentSpans.map(([first, last], i) => {
            const kind = draft.kinds[i], skip = draft.skips[i];
            const label = segmentLabel({ kind, skip });
            return <Button variant="text" size="sm" key={`${first}-${kind}-${skip}`} type="button" className={`swb-seg ${skip ? "skip" : kind}`} style={{ gridColumn: `${first + 1} / ${last + 2}` }}
              aria-label={`段 ${i + 1}（${label}，帧组 ${first}–${last}），点击切换类型`} disabled={frozen} onClick={() => change(toggleKind(draft, i))}>
              段 {i + 1} · {label} · {last - first + 1} 组
            </Button>;
          })}
          <span className="swb-label" title="点相机名决定该段由哪几台相机检测（至少一台）；不检测的段不用相机">检测相机</span>
          {segmentSpans.map(([first, last], i) => <div key={`cams-${first}`} className="swb-cams" style={{ gridColumn: `${first + 1} / ${last + 2}` }}
            role="group" aria-label={`段 ${i + 1} 检测相机`}>
            {plan.cameras.map(camera => {
              const on = !draft.skips[i] && (draft.cameras[i]?.includes(camera) ?? false);
              return <Button variant="text" size="sm" key={camera} type="button" className={`swb-cam${on ? " on" : ""}`} aria-pressed={on} disabled={frozen || draft.skips[i]}
                title={draft.skips[i] ? `段 ${i + 1} 不检测：点段名切回直线后可选相机` : on ? `段 ${i + 1} 由 ${camera} 检测，点击取消` : `点击让 ${camera} 检测段 ${i + 1}`}
                onClick={() => change(toggleCamera(draft, i, camera, plan.cameras))}>{camera}</Button>;
            })}
          </div>)}
          {plan.cameras.map(camera => {
            const strip = ready[camera];
            return [<span key={`${camera}-label`} className="swb-label">{camera}</span>,
              ...Array.from({ length: groupCount }, (_, k) => {
                const group = k + 1;
                const pixels = strip?.frames[k] ?? null;
                const cut = draft.cuts.includes(group);
                const segment = segmentSpans.findIndex(([first, last]) => group >= first && group <= last);
                const used = !draft.skips[segment] && (draft.cameras[segment]?.includes(camera) ?? true);
                // 段首尾跳过的帧（段参数里设置；段结构有改动时还没对应到新段，先不标）
                const params = restructured ? null : plan.segments[segment];
                const [first, last] = segmentSpans[segment] ?? [0, 0];
                const segmentSkipped = !!params && (group < first + params.skipHead || group > last - params.skipTail);
                const pass = passOf?.[k];
                const skipped = segmentSkipped || !!pass?.skip;
                const title = segmentSkipped ? `帧组 ${group} 在段 ${segment + 1} 的跳过范围内，不判定`
                  : pass?.skip ? `帧组 ${group}：第 ${pass.pass} 笔${passSkipText[pass.skip]}，不判定`
                  : pass ? `帧组 ${group} · 第 ${pass.pass} 笔` : undefined;
                return <Button variant="text" size="sm" key={`${camera}-${group}`} type="button" aria-label={`${camera} 帧组 ${group}`} aria-pressed={group === seq}
                  className={`swb-cell${group === seq ? " selected" : ""}${cut ? " cut" : ""}${used ? "" : " unused"}${skipped ? " skipped" : ""}${pass?.end ? " pass-end" : ""}`} onClick={() => setSeq(group)}
                  title={title}>
                  <Thumb pixels={pixels} width={strip?.width ?? 0} height={strip?.height ?? 0} channels={strip?.channels ?? 1} />
                </Button>;
              })];
          })}
          <span className="swb-label" title="点 ＋ 在该帧组之后插入边界，点 × 删除">边界</span>
          {Array.from({ length: groupCount }, (_, k) => {
            const group = k + 1;
            const at = draft.cuts.indexOf(group);
            if (group === groupCount) return <span key="end" className="swb-seam-end">{group}</span>;
            const passEnd = passOf?.[k]?.end ? passOf[k].pass : null;
            return <Button variant="text" size="sm" key={`seam-${group}`} type="button" className={`swb-seam${at >= 0 ? " on" : ""}${passEnd ? " pass" : ""}${dragging != null && dragging !== at ? " target" : ""}`} disabled={frozen}
              title={passEnd ? `第 ${passEnd} 笔与第 ${passEnd + 1} 笔之间：边界放这里不留过渡区` : undefined}
              aria-label={at >= 0 ? `删除帧组 ${group} 之后的边界` : `在帧组 ${group} 之后插入边界`}
              onPointerDown={() => { if (at >= 0) setDragging(at); }}
              onPointerUp={() => { if (dragging != null && draft.cuts[dragging] !== group) change(moveCut(draft, dragging, group)); }}
              onClick={() => change(toggleCut(draft, group))}><span>{group}</span><b aria-hidden="true">{at >= 0 ? "×" : "＋"}</b></Button>;
          })}
        </div>
      </div>
    </div>}
    <div className="swb-preview" aria-label={`帧组 ${seq} 三台相机原图`} onKeyDown={onKey}>
      {navigation}
      {draft && ready && plan.cameras.map(camera => <FramePreview key={camera} roundId={roundId} camera={camera} seq={seq}
        corridor={corridorOf(camera)} searchHalfPx={halfPx(camera)} />)}
    </div>
    {confirming && <Dialog open title={confirming.restructured ? "段结构将改变" : "保存分段的影响"} onOpenChange={open => { if (!open) setConfirming(null); }}
      footer={<><Button disabled={busy} onClick={() => setConfirming(null)}>取消</Button><Button variant="filled" disabled={busy} onClick={() => void save()}>保存分段</Button></>}>
      <div className="wp-stack">
        {confirming.coverage && <SegmentCoveragePreview coverage={confirming.coverage} />}
        <p>没动的段连同走廊原样保留（按示教件上的帧范围对应）。保存后：</p>
        <ul>
          <li>各段预期帧数按新的示教件分段重新得出；</li>
          {confirming.measurementCleared && <li>示教件的测量失效，需在试测页重新测量示教件；</li>}
          {confirming.dropped.length > 0 && <li>这些走廊会清掉（示教帧已不在原段，或段类型变了）：{confirming.dropped.join("、")}；</li>}
          {confirming.untaught.length > 0 && <li>保存后还要示教走廊：{confirming.untaught.join("、")}。</li>}
        </ul>
      </div>
    </Dialog>}
  </Panel>;
}
