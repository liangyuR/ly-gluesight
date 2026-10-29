import { pathLength, resampleTeachingPath, type ImagePoint } from "./teachingPath";
import { errorText } from "../../lib/errors";
import { cloneElement, useEffect, useId, useRef, useState, type CSSProperties, type ReactElement, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { ArrowRight, Brush, Camera, Crosshair, Eraser, Info, MousePointer2, Save, SquareDashed, Trash2, TriangleAlert, Undo2, WandSparkles } from "lucide-react";
import { Button, Divider, EmptyState, IconButton, NumberField, Popover, SegmentedControl, Slider, Surface, Tooltip } from "qing-steel";
import { cameraApi } from "../camera/api";
import type { CameraImaging } from "../camera/types";
import { useCycle } from "../cycle/api";
import { productionBusy as isProductionBusy } from "../cycle/phase";
import type { RecipeDoc } from "../cycle/types";
import type { PathSegment, WorkspacePlanView } from "../plan/types";
import { segmentKindLabel } from "../plan/vis";
import { workspaceApi } from "./api";
import { useWorkspace, type SnackMessage } from "./context";
import { Badge, GrayViewer, KV, Notice, useGrayImage, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { CaptureRoundV2, FrozenImage, GrayImage, WorkspaceView } from "./types";
import { cameraParamsError, confirmedSegmentRange, CORNER_CORRIDOR_MM, corridorError, spacingError, draftKey, parseDraftKey, sameCorridor, savedDraft, withCorridor } from "./teach";
import { cameraParamsTasks, corridorTasks, type CameraParamsTask, type CorridorTask } from "./cameraTasks";
import { frameArrived, readRoundFrame, roundView, type RoundView } from "./captureRound";
import { SegmentTrial } from "./TeachTrial";
import { FrameGrid } from "./FrameGrid";
import { pieceLabel } from "./pieces";
import { CameraHeadingCard, ExpertCard, MeasureCard, SegmentChecksCard, SegmentHeadingCard, type CameraParams } from "./CorridorSettings";
import type { HeadingGuide } from "./headingOverlay";
import "./corridor.css";
import WorkspaceNavigation from "./WorkspaceNavigation";

/** 禁用控件不发指针事件：Tooltip 挂在外包的可聚焦 span 上，原因同时作为控件的无障碍描述。 */
function WithReason({ reason, block = false, children }: { reason: string; block?: boolean; children: ReactElement<{ "aria-describedby"?: string }> }) {
  const id = useId();
  if (!reason) return children;
  return <Tooltip content={reason}><span tabIndex={0} className={"wp-tip-anchor" + (block ? " block" : "")}>{cloneElement(children, { "aria-describedby": id })}<span id={id} className="qs-sr-only">{reason}</span></span></Tooltip>;
}

function imagingMismatch(image: FrozenImage | null | undefined, current: CameraImaging | undefined) {
  if (!image || !current || image.exposureUs == null || image.gainDb == null) return null;
  if (image.exposureUs === current.exposureUs && image.gainDb === current.gainDb) return null;
  return `采集时曝光 ${image.exposureUs} µs / 增益 ${image.gainDb} dB，与相机当前值（${current.exposureUs} µs / ${current.gainDb} dB）不同`;
}

const DEFAULT_BRUSH_PX = 120;
const WIDEN_BRUSH_REASONS = new Set(["discontinuity", "missing_edges"]);

/** 刷选提取找不到连续两侧边缘时的下一步：先加大笔刷重刷，同一组再失败改用笔划生成走廊。 */
function extractionAdvice(reason: string, failures: number, brushWidth: number) {
  if (!WIDEN_BRUSH_REASONS.has(reason)) return "";
  return failures >= 2 ? "加大笔刷后仍无法连续提取（常见于胶堆、断胶等局部异常）：请点「笔划直接生成走廊」，再手工修正。" : `可加大笔刷宽度（当前 ${brushWidth} px，约为胶宽的 3 倍较稳）后沿胶路重刷，再自动提取。`;
}

const taskKey = (task: Pick<CorridorTask, "camera" | "segment">) => draftKey(task.camera, task.segment);

/** 打开页面时的默认目标：第一条未完成的段走廊，其次第一台缺像素当量的相机。 */
function firstOpen(cameras: CameraParamsTask[], tasks: CorridorTask[]) {
  const open = tasks.find(task => task.status !== "done");
  if (open) return taskKey(open);
  return cameras.find(item => item.mmPerPx == null)?.camera ?? (tasks[0] ? taskKey(tasks[0]) : cameras[0]?.camera ?? "");
}

/** URL 指定的目标：camera + segment 定位到这一段的走廊；只给相机时缺像素当量去相机参数，否则去它第一条未完成的段走廊。 */
function requestedTarget(camera: string | null, segment: string | null, cameras: CameraParamsTask[], tasks: CorridorTask[]) {
  const item = cameras.find(c => c.camera === camera);
  if (!item) return null;
  const exact = tasks.find(task => task.camera === item.camera && task.segment === Number(segment || NaN));
  if (exact) return taskKey(exact);
  if (item.mmPerPx == null) return item.camera;
  const open = tasks.find(task => task.camera === item.camera && task.status !== "done");
  return open ? taskKey(open) : item.camera;
}

/** 示教件里某台相机某个帧组的原图（候选示教帧，还没有冻结进候选配方）。 */
function useRoundFrame(round: CaptureRoundV2 | null, camera: string, seq: number, enabled: boolean) {
  const key = JSON.stringify([round?.roundId ?? null, camera, seq, enabled]);
  const [state, setState] = useState<{ key: string; image: GrayImage | null; error: string; loading: boolean }>({ key, image: null, error: "", loading: false });
  useEffect(() => {
    if (!enabled || !round) { setState({ key, image: null, error: "", loading: false }); return; }
    let alive = true;
    setState({ key, image: null, error: "", loading: true });
    readRoundFrame(round, camera, seq)
      .then(image => alive && setState({ key, image, error: "", loading: false }))
      .catch(e => alive && setState({ key, image: null, error: errorText(e), loading: false }));
    return () => { alive = false; };
  }, [key]);
  return state.key === key ? state : { image: null, error: "", loading: enabled && !!round };
}

const RAIL_KEY = "gluesight.teach.railWidth";
const RAIL_MIN = 220, RAIL_MAX = 640;
const clampRail = (w: number) => Math.round(Math.min(RAIL_MAX, Math.max(RAIL_MIN, w)));

/** 右栏宽度：拖动分隔条修改，本机记住；null 为样式默认宽度。 */
function useRailWidth() {
  const [width, setWidth] = useState<number | null>(() => {
    try { const v = Number(localStorage.getItem(RAIL_KEY)); return v > 0 ? clampRail(v) : null; } catch { return null; }
  });
  const save = (next: number | null) => {
    setWidth(next);
    try { if (next == null) localStorage.removeItem(RAIL_KEY); else localStorage.setItem(RAIL_KEY, String(next)); } catch { /* 存不了只影响下次打开 */ }
  };
  return [width, save] as const;
}

function RailResizer({ width, onChange }: { width: number | null; onChange: (next: number | null) => void }) {
  const drag = useRef<{ x: number; width: number; page: Element | null } | null>(null);
  const [dragging, setDragging] = useState(false);
  const current = (e: Element) => width ?? (e.parentElement?.getBoundingClientRect().width ?? 254);
  const end = () => { drag.current?.page?.classList.remove("wp-resizing"); drag.current = null; setDragging(false); };
  return <div className={"wp-rail-resizer" + (dragging ? " dragging" : "")} role="separator" aria-orientation="vertical" aria-label="拖动调整右栏宽度，双击恢复默认" title="拖动调整右栏宽度，双击恢复默认" tabIndex={0}
    aria-valuemin={RAIL_MIN} aria-valuemax={RAIL_MAX} aria-valuenow={width ?? undefined}
    onPointerDown={e => {
      if (e.button !== 0) return;
      e.preventDefault();
      const page = e.currentTarget.closest(".wp-teach-page");
      page?.classList.add("wp-resizing");
      drag.current = { x: e.clientX, width: current(e.currentTarget), page };
      setDragging(true);
      e.currentTarget.setPointerCapture?.(e.pointerId);
    }}
    onPointerMove={e => { const d = drag.current; if (d) onChange(clampRail(d.width - (e.clientX - d.x))); }}
    onPointerUp={end} onPointerCancel={end} onLostPointerCapture={end}
    onDoubleClick={() => onChange(null)}
    onKeyDown={e => {
      if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
      e.preventDefault();
      onChange(clampRail(current(e.currentTarget) + (e.key === "ArrowLeft" ? 16 : -16)));
    }} />;
}

export default function TeachingPage() {
  const { data, doc } = useWorkspace();
  if (!data || !doc) return <WorkspaceEmpty />;
  return <CorridorTeaching data={data} doc={doc} plan={data.plan} />;
}

function CorridorTeaching({ data, doc, plan }: { data: WorkspaceView; doc: RecipeDoc; plan: WorkspacePlanView }) {
  const { dirty, busy, saving, frameDirty, navigationDirty, corridorDrafts, setCorridorDraft, setDoc, act, setError } = useWorkspace();
  const { snapshot } = useCycle();
  const [search] = useSearchParams();
  const cameraItems = cameraParamsTasks(plan);
  const tasks = corridorTasks(plan);
  const requestedCamera = search.get("camera");
  const requestedSegment = search.get("segment");
  const [selected, setSelected] = useState(() => requestedTarget(requestedCamera, requestedSegment, cameraItems, tasks) ?? firstOpen(cameraItems, tasks));
  useEffect(() => { const key = requestedTarget(requestedCamera, requestedSegment, cameraItems, tasks); if (key) setSelected(key); }, [doc.id, requestedCamera, requestedSegment]);
  const target = parseDraftKey(selected);
  const task = target ? tasks.find(item => item.camera === target.camera && item.segment === target.segment) : undefined;
  const camera = task?.camera ?? cameraItems.find(item => item.camera === (target?.camera ?? selected))?.camera ?? cameraItems[0]?.camera ?? "";
  const segment = task?.segment ?? null;
  const key = task ? taskKey(task) : camera;
  const segmentItem = segment == null ? undefined : plan.segments.find(item => item.index === segment);
  const segmentTeaching = segment == null ? undefined : plan.cameraTeaching.find(item => item.camera === camera)?.segments.find(item => item.segment === segment);
  const frame = segmentTeaching?.frame;
  const [cameraImaging, setCameraImaging] = useState<CameraImaging>();
  useEffect(() => {
    let alive = true;
    setCameraImaging(undefined);
    if (frame) void cameraApi.rigStatus().then(list => { if (alive) setCameraImaging(list.find(s => s.id === frame.camera)?.imaging); }).catch(() => {});
    return () => { alive = false; };
  }, [frame?.id, frame?.camera]);
  const targetLabel = segment == null ? camera : `${camera} · 段 ${segment}`;
  const [railWidth, setRailWidth] = useRailWidth();
  const [working, setWorking] = useState(false);
  const [choosing, setChoosing] = useState(false);
  const [operation, setOperation] = useState<"extract" | null>(null);
  const [extractionHint, setExtractionHint] = useState("");
  const [extractionFailure, setExtractionFailure] = useState<{ point: ImagePoint; reason: string; message: string } | null>(null);
  const [vertex, setVertex] = useState<number | null>(null);
  const [region, setRegion] = useState<[number, number, number, number] | null>(null);
  const [regionMode, setRegionMode] = useState(false);
  const [nozzleMode, setNozzleMode] = useState(false);
  const [selectionTool, setSelectionTool] = useState<"brush" | "box">("brush");
  const [brushPoints, setBrushPoints] = useState<ImagePoint[]>([]);
  const [brushWidth, setBrushWidth] = useState(DEFAULT_BRUSH_PX);
  const [brushFailures, setBrushFailures] = useState(0);
  const [pointSpacing, setPointSpacing] = useState(20);
  const [trialSeq, setTrialSeq] = useState<number | null>(null);
  const mapTarget = useRef<{ key: string; seq: number } | null>(null);
  const pending = useRef(false), serial = useRef(0);
  const scope = JSON.stringify([doc.id, key, frame?.id ?? null]);
  useEffect(() => setBrushFailures(0), [scope]);
  const current = useRef({ scope, alive: true, revision: data.workspace.revision }); current.current.scope = scope; current.current.revision = data.workspace.revision;
  useEffect(() => { current.current.alive = true; return () => { current.current.alive = false; serial.current++; }; }, []);
  useEffect(() => { serial.current++; pending.current = false; setWorking(false); setChoosing(mapTarget.current?.key === key); setVertex(null); setRegion(null); setBrushPoints([]); setRegionMode(false); setNozzleMode(false); setOperation(null); setExtractionHint(""); setExtractionFailure(null); setTrialSeq(null); }, [scope]);
  const perform = async (request: () => Promise<WorkspaceView>, message: SnackMessage | ((next: WorkspaceView) => SnackMessage)) => {
    if (pending.current || busy || dirty) return null;
    pending.current = true; setWorking(true);
    const action = ++serial.current;
    const valid = () => current.current.alive && current.current.scope === scope && action === serial.current;
    const revision = data.workspace.revision;
    try { const result = await act(request, message); return valid() && result && (current.current.revision === revision || current.current.revision === result.workspace.revision) ? result : null; }
    catch (e) { if (valid() && current.current.revision === revision) setError(errorText(e)); return null; }
    finally { if (valid()) { pending.current = false; setWorking(false); } }
  };
  const { image, error, loading } = useGrayImage(doc.id, frame?.id ?? null);

  // 示教帧选择（RB-1）：只在示教件里选这台相机落在本段内的一个帧组
  const teachingPiece = data.workspace.teachingPiece;
  const teachingRoundId = teachingPiece?.roundId;
  const [picked, setPicked] = useState<{ round: CaptureRoundV2; view: RoundView } | null>(null);
  const [pickSeq, setPickSeq] = useState(1);
  const [pickError, setPickError] = useState("");
  const choosingNow = segment != null && (!frame || choosing);
  const reviews = data.workspace.captureReview?.rounds ?? {};
  useEffect(() => {
    setPicked(null); setPickError("");
    if (!teachingRoundId) return;
    let alive = true;
    void workspaceApi.captureGet(teachingRoundId).then(round => { if (alive) setPicked(round?.recipeId === doc.id && round.state === "complete" ? { round, view: roundView(round) } : null); }).catch(e => { if (alive) setPickError(errorText(e)); });
    return () => { alive = false; };
  }, [teachingRoundId, doc.id]);
  const pickGroups = picked?.view.groupCount ?? 0;
  const pickSeqSafe = Math.min(Math.max(1, pickSeq), Math.max(1, pickGroups));
  // 单段配方所有帧都在第 1 段，不需要先确认手动分段（与后端 apply_segment_teaching 一致）
  const range = segment == null || !picked ? null : plan.segments.length === 1 ? { start: 1, end: picked.view.groupCount } : confirmedSegmentRange(reviews[picked.view.roundId], plan.segments, segment);
  useEffect(() => {
    if (!range) return;
    const target = mapTarget.current;
    setPickSeq(target?.key === key ? target.seq : Math.round((range.start + range.end) / 2));
    if (target?.key === key) mapTarget.current = null;
  }, [key, range?.start, range?.end, choosingNow]);
  const inSegment = !!range && pickSeqSafe >= range.start && pickSeqSafe <= range.end;
  const arrived = !!picked && frameArrived(picked.view, camera, pickSeqSafe);
  const candidate = useRoundFrame(picked?.round ?? null, camera, pickSeqSafe, choosingNow && arrived);

  const saved = savedDraft(data.workspace.doc, key);
  const t = segment == null ? saved : corridorDrafts[key] ?? saved;
  const editing = segment != null && !sameCorridor(t, saved);
  const invalid = corridorError(t, doc.spacing);
  const unlocked = !busy && !dirty && !working && !data.workspace.pending && !navigationDirty;
  // 画走廊、标出胶点、调参数在等待保存与自动保存期间都可继续，改动并入下一次保存；提交后端的操作仍要 unlocked
  const editable = (!busy || saving) && !working && !data.workspace.pending && !navigationDirty;
  const productionBusy = isProductionBusy(snapshot);
  const update = (next: typeof t) => { if (editable && segment != null) setCorridorDraft(key, next); };
  const setPath = (corridor: [number, number][]) => update({ ...t, corridor });
  const canEdit = editable && !!frame && !choosingNow;
  const lengthPx = pathLength(t.corridor);
  const strokeValid = brushPoints.length >= 2 && pathLength(brushPoints) >= 8;
  const brushValid = Number.isFinite(brushWidth) && brushWidth >= 6 && brushWidth <= 300 && strokeValid;
  const spacingValid = Number.isFinite(pointSpacing) && pointSpacing >= 4 && pointSpacing <= 80;
  const selectionRegion = selectionTool === "box" ? region : brushPoints.length && Number.isFinite(brushWidth) ? (() => {
    const xs = brushPoints.map(p => p[0]), ys = brushPoints.map(p => p[1]);
    return [Math.min(...xs) - brushWidth / 2, Math.min(...ys) - brushWidth / 2, Math.max(...xs) - Math.min(...xs) + brushWidth, Math.max(...ys) - Math.min(...ys) + brushWidth];
  })() : null;
  const roi: [number, number, number, number] | null = selectionRegion && frame ? (() => {
    const x = Math.max(0, Math.floor(selectionRegion[0])), y = Math.max(0, Math.floor(selectionRegion[1]));
    return [x, y, Math.min(frame.size[0], Math.ceil(selectionRegion[0] + selectionRegion[2])) - x, Math.min(frame.size[1], Math.ceil(selectionRegion[1] + selectionRegion[3])) - y];
  })() : null;
  const roiValid = (selectionTool === "box" || brushValid) && !!roi && roi[2] >= 5 && roi[3] >= 5 && roi[2] * roi[3] <= 4_000_000;

  /** 把这一段的走廊草稿并入候选配方并整份保存（段走廊没有单独的草稿命令）。 */
  const commit = (message: SnackMessage | ((next: WorkspaceView) => SnackMessage)) => perform(() => workspaceApi.saveDoc(doc.id, data.workspace.revision, withCorridor(data.workspace.doc, key, t)), message);
  const extract = async () => {
    if (!canEdit || !unlocked || !frame || pending.current) return;
    if (!roiValid || !roi || !spacingValid) { setRegionMode(true); setExtractionHint("请沿胶路刷过至少 8 px，并设置有效的笔刷宽度和走廊点间距。"); return; }
    const capturedScope = scope;
    setOperation("extract"); setExtractionHint(""); setExtractionFailure(null);
    try {
      let candidateView = data;
      if (editing) {
        const savedView = await commit("");
        if (!savedView) return;
        candidateView = savedView;
      }
      let extracted: Awaited<ReturnType<typeof workspaceApi.extractCorridor>> | null = null;
      // 按相机 id 在这一段冻结的示教帧上提取（workspace_extract_corridor）
      const result = await perform(async () => {
        extracted = await workspaceApi.extractCorridor(doc.id, candidateView.workspace.revision, camera, frame.id, roi, selectionTool === "brush" ? { points: brushPoints, radiusPx: brushWidth / 2 } : undefined, pointSpacing);
        return candidateView;
      }, "");
      if (result && extracted && current.current.scope === capturedScope) {
        const output = extracted as Awaited<ReturnType<typeof workspaceApi.extractCorridor>>;
        if (output.failure) {
          const failures = selectionTool === "brush" && WIDEN_BRUSH_REASONS.has(output.failure.reason) ? brushFailures + 1 : 0;
          const advice = selectionTool === "brush" ? extractionAdvice(output.failure.reason, failures, brushWidth) : "";
          setBrushFailures(failures);
          setExtractionFailure(output.failure); setExtractionHint(advice ? output.failure.message + " " + advice : output.failure.message); setRegionMode(true); return;
        }
        setBrushFailures(0);
        setCorridorDraft(key, { ...t, corridor: output.path }); setRegionMode(false); setVertex(null);
        setExtractionHint(output.message || "走廊已提取，请检查后保存。");
      }
    } finally { if (current.current.scope === capturedScope) setOperation(null); }
  };
  const fromStroke = () => {
    if (!canEdit || !strokeValid || !spacingValid) return;
    setPath(resampleTeachingPath(brushPoints, pointSpacing));
    setRegionMode(false); setVertex(null); setExtractionFailure(null);
    setExtractionHint("已按笔划生成走廊草稿，请检查并手工修正；尚未验证胶路边缘。");
  };
  const changeBrushWidth = (value: number) => { setBrushWidth(value); setExtractionFailure(null); setExtractionHint(""); };
  const chooseFrame = async () => {
    if (!unlocked || segment == null || !picked || !arrived || !inSegment || pending.current) return;
    const result = await perform(() => workspaceApi.teachSegmentCorridor(doc.id, data.workspace.revision, camera, segment, picked.view.roundId, pickSeqSafe), `${targetLabel} 的示教帧已选定，请在图上示教走廊`);
    if (result) setChoosing(false);
  };
  const saveCorridor = () => { if (unlocked && editing && !invalid) void commit(`${targetLabel} 的走廊已保存到候选配方`); };
  const clearSegment = () => { if (unlocked && segment != null && !editing) void perform(() => workspaceApi.clearSegmentCorridor(doc.id, data.workspace.revision, camera, segment), `${targetLabel} 的走廊与示教帧已清除`); };
  const changeSegments = (segments: PathSegment[]) => setDoc({ ...doc, segments });
  const inspect = doc.inspect?.find(item => item.camera === camera);
  const params: CameraParams = { mmPerPx: inspect?.mmPerPx ?? null, detect: inspect?.detect ?? null, window: inspect?.window ?? null, nozzle: inspect?.nozzle ?? null, expert: inspect?.expert ?? {} };
  const setParams = (next: CameraParams) => setDoc({ ...doc, inspect: (doc.inspect ?? []).map(item => item.camera === camera ? { ...item, ...next } : item) });
  // 出胶点与检测窗口跟着候选配方即时显示；方向范围、扇区与回退方向由后端按已保存的走廊算
  const savedNozzle = plan.cameraTeaching.find(item => item.camera === camera)?.nozzle ?? null;
  const derivedNozzle = savedNozzle && !savedNozzle.calibrated ? savedNozzle.point : null;
  const headingOn = segment != null && !!doc.segments?.find(item => item.index === segment)?.heading;
  const cameraScale = params.mmPerPx != null && Number.isFinite(params.mmPerPx) && params.mmPerPx > 0 ? params.mmPerPx : null;
  const nozzlePoint = params.nozzle ?? derivedNozzle;
  const headingGuide: HeadingGuide | null = headingOn && nozzlePoint ? {
    nozzle: nozzlePoint, calibrated: !!params.nozzle,
    zone: params.window && cameraScale ? [params.window.startMm / cameraScale, (params.window.startMm + params.window.lengthMm) / cameraScale] : null,
    ...(segmentTeaching?.heading && !editing ? { range: segmentTeaching.heading.range, sector: segmentTeaching.heading.sector, fallback: segmentTeaching.heading.fallback } : {}),
  } : null;

  if (!cameraItems.length) return <div className="wp-page"><WorkspaceBar /><Surface><EmptyState icon={<Camera size={32} aria-hidden="true" />} title="段配方还没有计划相机" description="先在“段列表”里确认计划相机，再回来示教走廊。" action={<Button asChild><Link to="/recipe/segments">前往段列表<ArrowRight size={15} aria-hidden="true" /></Link></Button>} /></Surface></div>;

  const imagingHint = imagingMismatch(frame, cameraImaging);
  const order = cameraItems.flatMap(item => [item.camera, ...tasks.filter(t => t.camera === item.camera).map(taskKey)]);
  const nextTask = tasks.find(item => order.indexOf(taskKey(item)) > order.indexOf(key));
  const allDone = cameraItems.every(item => item.ready);
  const doneCount = tasks.filter(item => item.status === "done").length;
  const defaults = data.workspace.doc.detect;
  const scaled = t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0;
  const searchHalfPx = scaled ? (t.detect ?? defaults).searchMm / t.mmPerPx! : undefined;
  const hasPath = t.corridor.length >= 2;
  const [lengthValue, lengthUnit] = scaled ? [(lengthPx * t.mmPerPx!).toFixed(1), "mm"] : [lengthPx.toFixed(0), "px"];
  const cornerLong = segmentItem?.kind === "corner" && scaled && lengthPx * t.mmPerPx! > CORNER_CORRIDOR_MM;
  const done = !!segmentTeaching?.saved;
  const groupsLocked = !unlocked || editing || frameDirty;
  const groupLockReason = navigationDirty ? "请先保存或撤销导航图修改" : editing || frameDirty ? "走廊有未保存的修改，先保存或清空后再切换" : dirty ? "候选配置正在保存，请稍候" : data.workspace.pending ? "候选版本正在等待生效" : "正在处理或保存，请稍候";
  const mapReview = teachingRoundId ? reviews[teachingRoundId] : undefined;
  const mapRanges = plan.segments.map(item => plan.segments.length === 1 && pickGroups > 0 ? { start: 1, end: pickGroups } : confirmedSegmentRange(mapReview, plan.segments, item.index));
  const navigationRanges: [number, number][] | null = mapRanges.length && mapRanges.every(item => item != null) ? mapRanges.map(item => [item!.start, item!.end]) : null;
  const navigateMap = (nextSegment: number, nextSeq: number) => {
    if (groupsLocked) return;
    const next = tasks.find(item => item.segment === nextSegment && item.camera === camera) ?? tasks.find(item => item.segment === nextSegment);
    if (!next) return;
    const nextKey = taskKey(next);
    mapTarget.current = nextKey === key && choosingNow ? null : { key: nextKey, seq: nextSeq };
    setSelected(nextKey); setChoosing(true); setPickSeq(nextSeq); setTrialSeq(null);
  };
  const nextLabel = nextTask && `${nextTask.camera} · 段 ${nextTask.segment}`;
  const [stateText, stateTone]: [string, "ok" | "warn" | "info" | "neutral"] = segment == null ? (cameraItems.find(item => item.camera === camera)?.mmPerPx == null ? ["缺像素当量", "warn"] : ["像素当量已设", "ok"])
    : !frame ? ["待选帧", "neutral"] : data.workspace.pending ? ["候选待生效", "warn"]
    : choosing ? ["重新选帧中", "info"] : editing ? [invalid ? "走廊已修改 · 未完成" : "走廊已修改 · 待保存", "warn"] : done ? ["已示教", "ok"] : hasPath ? ["已保存 · 待检查", "info"] : ["待画走廊", "neutral"];
  // 刷选连续两次找不到连续边缘后，笔划直接生成走廊是唯一的主操作
  const strokeAdvised = selectionTool === "brush" && brushFailures >= 2 && !!extractionFailure;
  const primary = segment == null ? "next" : strokeAdvised ? "stroke" : !hasPath ? "extract" : editing ? "save" : "next";
  const saveReason = !editing ? (done ? "走廊已保存" : "先提取或绘制走廊") : invalid ? invalid : !unlocked ? (data.workspace.pending ? "候选版本正在等待生效" : "正在处理或保存，请稍候") : "";
  const doneHint: ReactNode = nextLabel ? `已完成；下一条 ${nextLabel}` : allDone ? <>各段走廊都已示教，可到 <Link to="/recipe/calibration">多轮标定</Link></> : "已完成";
  const dockHint: ReactNode = segment == null ? (params.mmPerPx == null ? `${camera} 缺像素当量：在右侧填写或到相机标定，各段走廊都按它换算` : `像素当量与检测参数改动自动保存到候选配方；走廊在左侧按段逐条示教`)
    : !frame ? `先在示教件第 ${segment} 段内选一帧`
    : data.workspace.pending ? "等待当前工件结束后切换，期间不能修改示教"
      : working ? "正在处理或保存，请稍候"
        : choosing ? "选定新帧后，这一段现有走廊原样保留"
          : editing ? (invalid ? invalid : "有未保存的修改：保存后写入候选配方，已发布版本不变；也会在停顿后自动保存")
            : cornerLong ? `弯道走廊超过 ${CORNER_CORRIDOR_MM} mm：胶条绕出胶点转动，建议只从出胶点起画一小段`
              : !scaled ? `${camera} 缺像素当量：先在相机参数里设置，再保存走廊`
                : done ? doneHint
                  : segmentItem?.kind === "corner" ? `弯道：从出胶点起沿胶条画一小段（不超过 ${CORNER_CORRIDOR_MM} mm）`
                    : "直线段：从出胶点一侧起，沿胶条方向画整段走廊";
  const picking = nozzleMode && headingOn;
  const editHint = picking ? `点击胶嘴出胶处标定 ${camera} 的出胶点（${camera} 各段共用，自动保存到候选配方）；叠加图中环带为检测窗口，实线扇形为本段方向范围，虚线扇形为搜索扇区，绿色虚线为回退方向。`
    : "点击图像加点（绿圈为起点），拖动点可调整；密集走廊打开“点柄”显示全部点，Ctrl＋滚轮缩放。";
  const regionHint = extractionHint || (selectionTool === "brush" ? "按住左键沿胶路从起点刷到终点，每笔重画选区；笔刷覆盖胶路和两侧少量背景，尽量避开阴影。" : "按住左键框选一段连续胶路，再自动提取走廊；框选包含阴影时建议改用刷选。");
  const changeMode = (value: string) => {
    setNozzleMode(value === "nozzle");
    if (value === "edit" || value === "nozzle") { setRegionMode(false); return; }
    const tool = value as "brush" | "box";
    if (tool !== selectionTool) { setSelectionTool(tool); setExtractionFailure(null); setExtractionHint(""); setBrushFailures(0); }
    setRegionMode(true);
  };
  const modeLabel = (icon: ReactNode, long: string, short: string) => <><span className="wp-seg-icon">{icon}</span><span className="wp-seg-long">{long}</span><span className="wp-seg-short">{short}</span></>;
  const tools = <div className="wp-canvas-tools" role="group" aria-label="示教工具">
    <SegmentedControl size="sm" label="画布模式" value={regionMode ? selectionTool : nozzleMode && headingOn ? "nozzle" : "edit"} disabled={!canEdit || (!frame && !regionMode)} options={[
      { value: "edit", label: modeLabel(<MousePointer2 size={14} aria-hidden="true"/>, "编辑点", "编辑"), "aria-label": "编辑点" },
      { value: "brush", label: modeLabel(<Brush size={14} aria-hidden="true"/>, "沿胶路刷选", "刷选"), "aria-label": "沿胶路刷选" },
      { value: "box", label: modeLabel(<SquareDashed size={14} aria-hidden="true"/>, "矩形框选", "框选"), "aria-label": "矩形框选" },
      ...(headingOn ? [{ value: "nozzle", label: modeLabel(<Crosshair size={14} aria-hidden="true"/>, "标出胶点", "出胶点"), "aria-label": "标出胶点" }] : []),
    ]} onValueChange={changeMode}/>
    <Button size="sm" variant={primary === "extract" ? "filled" : "tonal"} data-variant={primary === "extract" ? "filled" : "tonal"} leadingIcon={<WandSparkles size={14}/>} disabled={!canEdit || !unlocked || loading || !!error} onClick={() => void extract()}>{operation === "extract" ? "提取中…" : "自动提取走廊"}</Button>
    <Button size="sm" variant={primary === "stroke" ? "filled" : "tonal"} data-variant={primary === "stroke" ? "filled" : "tonal"} title={selectionTool !== "brush" ? "仅刷选时可用" : !strokeValid ? "先沿胶路刷过至少 8 px" : "按笔划生成走廊草稿"} disabled={!canEdit || selectionTool !== "brush" || !strokeValid || !spacingValid} onClick={fromStroke}>笔划直接生成走廊</Button>
    <Divider vertical/>
    <IconButton size="sm" aria-label="删除末点" title="删除末点" disabled={!canEdit || !t.corridor.length} onClick={() => { setVertex(null); setPath(t.corridor.slice(0, -1)); }}><Undo2 size={15}/></IconButton>
    <IconButton size="sm" aria-label="删除选中点" title="删除选中点" disabled={!canEdit || vertex === null || vertex >= t.corridor.length} onClick={() => { const i = vertex; setVertex(null); setPath(t.corridor.filter((_, j) => j !== i)); }}><Trash2 size={15}/></IconButton>
    <IconButton size="sm" aria-label="清空走廊" title="清空走廊" disabled={!canEdit || !t.corridor.length} onClick={() => { setVertex(null); setPath([]); }}><Eraser size={15}/></IconButton>
  </div>;
  const canvasParams = <div className="wp-canvas-params">
    {selectionTool === "brush" && <span className="wp-inline-num">笔刷宽度<NumberField aria-label="笔刷宽度" unit="px" min={6} max={300} step={2} value={Number.isFinite(brushWidth) ? brushWidth : ""} disabled={!canEdit} onValueChange={value => changeBrushWidth(value === "" ? NaN : value)}/>
      <Slider className="wp-brush-width" label="拖动调整笔刷宽度" min={6} max={300} step={2} value={Number.isFinite(brushWidth) ? Math.min(300, Math.max(6, brushWidth)) : DEFAULT_BRUSH_PX} disabled={!canEdit} formatValue={value => value + " px"} onValueChange={changeBrushWidth}/></span>}
    <span className="wp-inline-num" title="编辑点按目标最大间距调整，转弯处保留点">走廊点间距<NumberField aria-label="走廊点间距" unit="px" min={4} max={80} step={2} value={Number.isFinite(pointSpacing) ? pointSpacing : ""} disabled={!canEdit} onValueChange={value => setPointSpacing(value === "" ? NaN : value)}/></span>
    <Button size="sm" variant="text" disabled={!canEdit || !spacingValid || t.corridor.length < 2} onClick={() => { setVertex(null); setPath(resampleTeachingPath(t.corridor, pointSpacing)); }}>应用到当前走廊</Button>
    {regionMode ? <p className={"wp-canvas-hint" + (extractionFailure ? " warn" : "")} role="status">{extractionFailure && <TriangleAlert size={14} aria-hidden="true"/>}{regionHint}</p>
      : <p className="wp-canvas-hint" role="status">{extractionHint && <><span>{extractionHint}</span> </>}<span>{editHint}</span></p>}
  </div>;
  const summary = <span className="wp-trial-inline">
    <Badge tone={done && !editing ? "ok" : "neutral"}>{editing ? "走廊未保存" : done ? "段走廊已示教" : "待画走廊"}</Badge>
    <span className="muted">走廊 <b>{t.corridor.length}</b> 点 · <b>{lengthValue}</b> {lengthUnit}</span>
    {cornerLong && <Badge tone="warn">弯道走廊偏长</Badge>}
  </span>;
  const imageInfo = frame && <Popover side="bottom" align="end" label="图像采集信息" className="wp-image-info" title={`示教帧 · ${targetLabel}`} trigger={<IconButton size="sm" aria-label="图像采集信息"><Info size={15}/></IconButton>}>
    <KV label="图像">{`${frame.id} · ${frame.size[0]} × ${frame.size[1]} px`}</KV>
    <KV label="取样来源">{frame.source} · {new Date(frame.capturedAt).toLocaleString("zh-CN")}</KV>
    <KV label="曝光">{frame.exposureUs != null ? frame.exposureUs + " μs" : "未记录"}</KV><KV label="增益">{frame.gainDb != null ? frame.gainDb + " dB" : "未记录"}</KV>
    <KV label="示教帧更换">仅通过“重新选帧”</KV>
    {imagingHint && <p className="c-warn" role="note">{imagingHint}；只作提示，示教与标定仍然有效</p>}
    <p className="muted hint">走廊保存进候选配方，已发布版本不变。用于生产前仍需完成多轮标定与发布。</p>
  </Popover>;
  const pickReason = !teachingPiece ? "还没有示教件：先选示教件" : !picked ? "正在读取示教件" : !range ? "示教件的分段边界还没确认：先在分段标定里保存分段"
    : !inSegment ? `第 ${segment} 段在示教件为帧组 ${range.start}–${range.end}` : !arrived ? "该相机本帧组缺帧，请换一个帧组" : !unlocked ? "正在处理或保存，请稍候" : "";
  const pickPanel = <Surface role="region" className="wp-picker wp-corridor-picker" aria-label="示教帧选择">
    <h2>{targetLabel}<small>示教帧</small></h2>
    <div className="wp-corridor-pick">
      {teachingPiece ? <span className="muted">示教件 · {pieceLabel(teachingPiece)}</span>
        : <span className="muted">还没有示教件，请先<Link className="wp-text-link" to="/recipe/capture">选示教件</Link>。</span>}
      {range && <span className="muted">第 {segment} 段在示教件为帧组 {range.start}–{range.end}</span>}
    </div>
    {picked && pickGroups > 0 && <FrameGrid label="帧组选择" cameras={[{ id: camera, name: camera }]} groupCount={pickGroups} camera={0} seq={pickSeqSafe} range={range}
      missing={(_, s) => !frameArrived(picked.view, camera, s)} onSelect={(_, s) => setPickSeq(s)} />}
    <div className="wp-picker-side">
      <WithReason reason={pickReason}>
        <Button variant="filled" data-variant="filled" disabled={!!pickReason} onClick={() => void chooseFrame()}>用此帧示教 {targetLabel}</Button>
      </WithReason>
      {frame && <Button variant="text" onClick={() => setChoosing(false)}>取消</Button>}
    </div>
  </Surface>;
  const cameraTasks = tasks.filter(item => item.camera === camera);
  const trialReview = segmentTeaching ? reviews[segmentTeaching.round] : undefined;
  const trialRange = segment == null ? null : plan.segments.length === 1 ? (trialReview ? { start: 1, end: trialReview.groupCount } : null) : confirmedSegmentRange(trialReview, plan.segments, segment);
  const trialLocked = productionBusy ? "工件正在检测，结束后再试测" : editing ? "走廊有未保存的修改：保存后再试测" : dirty ? "参数正在保存，保存后再试测"
    : !unlocked ? "正在处理或保存，请稍候" : !hasPath ? "这一段还没有保存的走廊" : "";
  const paramsError = cameraParamsError(params);
  const paramsDisabled = !editable;
  const middle = segment == null
    ? <Surface role="region" className="wp-seg-corridor-note" aria-label="相机参数">
      <h3>{camera} · 相机参数</h3>
      <p className="muted">像素当量与检测参数按相机设置，{camera} 检测的各段走廊都按它们测量；改动自动保存到候选配方。走廊在左侧按段逐条示教。</p>
      {paramsError && <p className="c-warn" role="alert">{paramsError}</p>}
      {cameraTasks.length ? <ul className="wp-camera-segments" aria-label={`${camera} 检测的段`}>{cameraTasks.map(item => <li key={item.segment}><span>段 {item.segment} · {segmentKindLabel[item.kind]}</span><Badge tone={item.status === "done" ? "ok" : item.frame ? "warn" : "neutral"}>{item.label}</Badge></li>)}</ul>
        : <p className="muted">没有段把 {camera} 选为检测相机。</p>}
    </Surface>
    : <>
      {choosingNow && pickPanel}
      {pickError && choosingNow && <p className="c-warn wp-picker-warn" role="alert">{pickError}</p>}
      {!choosingNow && frame && <Surface role="region" className="wp-picker wp-corridor-frame" aria-label="示教帧">
        <h2>{targetLabel}<small>示教帧</small></h2>
        <span className="muted wp-corridor-source">{frame.source} · {frame.id}</span>
        <div className="wp-picker-side">{imageInfo}<Button variant="text" disabled={!unlocked || editing || frameDirty} onClick={() => { setTrialSeq(null); setChoosing(true); }}>重新选帧</Button></div>
      </Surface>}
      {!choosingNow && frame && segmentTeaching && <SegmentTrial key={scope} id={doc.id} camera={camera} segment={segment!} round={segmentTeaching.round} teachSeq={segmentTeaching.seq}
        range={trialRange} revision={data.workspace.revision} locked={trialLocked} guide={headingGuide} seq={trialSeq} onSeq={setTrialSeq} />}
      {choosingNow
        ? <GrayViewer controlsAt="footer" image={candidate.image} loading={candidate.loading} error={candidate.error || (picked && !arrived ? "该相机本帧组缺帧，请换一个帧组" : "")} label={`${camera} · 帧组 ${pickSeqSafe} · 候选示教帧`}
          emptyTitle="选择示教帧" emptyHint="在示教件这一段内选这台相机的帧组；缺帧的帧组不能选。" summary={<span className="muted">{`选定后冻结为第 ${segment} 段的示教帧，这条走廊只用于这一段的帧。`}</span>}/>
        : trialSeq != null ? null : <GrayViewer controlsAt="footer" summary={summary} image={image} loading={loading} error={error} label={frame ? `${targetLabel} · 示教帧` : "等待取样"} tools={tools} subtools={canvasParams} emptyTitle="示教帧尚未就绪" emptyHint="选定示教帧后，这里显示这一帧。"
          region={selectionTool === "box" ? region : null}
          onRegion={regionMode && selectionTool === "box" && canEdit ? setRegion : undefined}
          brush={regionMode && selectionTool === "brush" && Number.isFinite(brushWidth) ? { points: brushPoints, radiusPx: brushWidth / 2 } : null} onBrush={regionMode && selectionTool === "brush" && canEdit ? points => { setBrushPoints(points); setExtractionFailure(null); setExtractionHint(""); } : undefined}
          extractionFailure={extractionFailure}
          overlay={{ path: t.corridor, searchHalfPx, selected: vertex, heading: headingGuide }}
          editDisabled={!canEdit}
          onPoint={picking && frame ? p => setParams({ ...params, nozzle: p }) : undefined}
          onEdit={frame && !regionMode && !picking ? { add: p => setPath([...t.corridor, p]), move: (i, p) => setPath(t.corridor.map((q, j) => j === i ? p : q)), select: setVertex } : undefined} />}
    </>;
  const checksDisabled = !editable;
  const sharing = plan.cameras.filter(item => item !== camera && !doc.inspect?.find(i => i.camera === item)?.detect);
  const cameraScaleText = cameraScale != null ? `${Number(cameraScale.toPrecision(6))} mm/px` : "缺像素当量";
  const navigation = <WorkspaceNavigation segments={plan.segments} ranges={navigationRanges} seq={choosingNow ? pickSeqSafe : trialSeq ?? segmentTeaching?.seq ?? 1}
    currentSegment={segment ?? undefined} frameStatus={navigationRanges ? "confirmed" : "unavailable"}
    locked={busy || dirty || working || editing || frameDirty || data.workspace.pending ? groupLockReason : ""} onNavigate={navigateMap} />;
  const rail = segment == null
    ? <div className="wp-stack wp-teach-settings">
      {navigation}
      <MeasureCard key={camera} camera={camera} params={params} defaults={doc.detect} spacing={doc.spacing} sharing={sharing} disabled={paramsDisabled || frameDirty} spacingError={value => spacingError(doc, value)}
        onParams={setParams} onDefaults={detect => setDoc({ ...doc, detect })} onSpacing={spacing => setDoc({ ...doc, spacing })} />
      <CameraHeadingCard key={`heading-${camera}`} camera={camera} params={params} derivedNozzle={derivedNozzle} disabled={paramsDisabled || frameDirty} onChange={setParams}
        headingSegments={cameraTasks.filter(item => doc.segments?.find(s => s.index === item.segment)?.heading).map(item => item.segment)} />
      <ExpertCard key={`expert-${camera}`} camera={camera} own={params.expert ?? {}} defaults={doc.expert ?? {}} disabled={paramsDisabled || frameDirty}
        onCamera={expert => setParams({ ...params, expert })} onDefaults={expert => setDoc({ ...doc, expert })} />
      <p className="wp-settings-hint wp-rail-note">段的逐帧定向与检测项：在左侧选段后设置。</p></div>
    : <div className="wp-stack wp-teach-settings">
      {navigation}
      <Surface role="region" className="wp-seg-corridor-note" aria-label="段走廊说明">
        <div className="wp-seg-head"><h3>第 {segment} 段 · {segmentItem?.kind === "corner" ? "弯道" : "直线"}</h3><Badge tone={stateTone}>{stateText}</Badge><span className="muted">{camera}</span></div>
        <p className="muted">{segmentItem?.kind === "corner" ? `弯道：从出胶点起画一小段（不超过 ${CORNER_CORRIDOR_MM} mm），只判胶宽、断胶、溢胶` : "直线：从出胶点一侧起，沿胶条画整段"}</p>
        <WithReason reason={editing ? "走廊有未保存的修改" : !unlocked ? "正在处理或保存，请稍候" : !frame && !hasPath ? "这一段还没有示教帧和走廊" : ""}><Button variant="text" size="sm" disabled={editing || !unlocked || (!frame && !hasPath)} onClick={clearSegment}>清除本段走廊</Button></WithReason>
      </Surface>
      {segmentItem && <SegmentHeadingCard key={`heading-${segment}`} segments={doc.segments ?? []} index={segment} camera={camera} params={params} derivedNozzle={derivedNozzle}
        disabled={checksDisabled} sharedLocked={editing || frameDirty ? "走廊有未保存的修改：保存后再改检测窗口与出胶点。" : ""} onSegments={changeSegments} onParams={setParams} />}
      {segmentItem && <SegmentChecksCard key={`checks-${segment}`} segments={doc.segments ?? []} index={segment} cameras={plan.cameras} disabled={checksDisabled} onChange={changeSegments} />}
      <WithReason block reason={groupsLocked ? groupLockReason : ""}><Button variant="outlined" className="wp-rail-link" disabled={groupsLocked} onClick={() => setSelected(camera)}>
        <strong>测量</strong><span>{cameraScaleText} · {params.detect ? "本相机自定义" : "配方默认"}</span><span className="wp-rail-link-go">去 {camera} 改<ArrowRight size={13} aria-hidden="true" /></span>
      </Button></WithReason></div>;
  return <div className="wp-page wp-teach-page"><WorkspaceBar />
    <div className="wp-teach" style={railWidth != null ? { "--wp-rail-w": railWidth + "px" } as CSSProperties : undefined}>
      <Surface role="region" className="wp-group-panel" aria-label="走廊示教目标"><h2>段走廊 <small>{doneCount} / {tasks.length}</small></h2><nav className="wp-group-list" aria-label="相机与段选择">{cameraItems.flatMap(item => {
        const slot = (itemKey: string, button: ReactElement<{ "aria-describedby"?: string }>) => groupsLocked && itemKey !== key ? <WithReason key={itemKey} block reason={groupLockReason}>{button}</WithReason> : <span key={itemKey} className="wp-group-slot">{button}</span>;
        const cameraButton = <Button variant={item.camera === key ? "tonal" : "text"} size="sm" className={"wp-group-item" + (item.camera === key ? " selected" : "")} aria-label={"选择相机 " + item.camera} aria-pressed={item.camera === key} disabled={groupsLocked && item.camera !== key} title={item.mmPerPx != null ? `${item.camera} · ${Number(item.mmPerPx.toPrecision(6))} mm/px` : "缺像素当量"} onClick={() => setSelected(item.camera)}>
          <i className={"dot " + (item.mmPerPx != null ? "ok" : "warn")} aria-hidden="true"/><strong>{item.camera}</strong><span>{item.label}</span>
        </Button>;
        const segmentButtons = tasks.filter(s => s.camera === item.camera).map(s => {
          const itemKey = taskKey(s);
          return slot(itemKey, <Button variant={itemKey === key ? "tonal" : "text"} size="sm" className={"wp-group-item wp-group-sub" + (itemKey === key ? " selected" : "")} aria-label={`选择 ${item.camera} 段 ${s.segment} 走廊`} aria-pressed={itemKey === key} disabled={groupsLocked && itemKey !== key} onClick={() => setSelected(itemKey)}>
            <i className={"dot " + (s.status === "done" ? "ok" : s.frame ? "warn" : "off")} aria-hidden="true"/><strong>段 {s.segment}</strong>{s.kind === "corner" ? "弯道" : "直线"}<span>{s.label}</span>
          </Button>);
        });
        return [slot(item.camera, cameraButton), ...segmentButtons];
      })}</nav></Surface>
      <div className="wp-stack wp-mid">
        {plan.cameras.length !== 3 && <Notice title="段配方需要 3 台相机" tone="warn">当前计划相机 {plan.cameras.length} 台：三台相机共用一个触发，缺相机的配方不能发布。</Notice>}
        {middle}
      </div>
      <div className="wp-rail-wrap"><RailResizer width={railWidth} onChange={setRailWidth} />
        {rail}</div>
    </div>
    <div className="wp-teach-dock" role="group" aria-label="走廊示教操作">
      <span className="wp-dock-mid"><Badge tone={stateTone}>{stateText}</Badge><span className="wp-dock-hint">{dockHint}</span></span>
      <span className="wp-dock-actions">
        {segment != null && <WithReason reason={saveReason}><Button variant={primary === "save" ? "filled" : "text"} data-variant={primary === "save" ? "filled" : "text"} leadingIcon={<Save size={15}/>} disabled={!!saveReason} onClick={saveCorridor}>保存走廊</Button></WithReason>}
        {nextTask
          ? <Button aria-label="下一条走廊" variant={primary === "next" ? "filled" : "tonal"} data-variant={primary === "next" ? "filled" : "tonal"} trailingIcon={<ArrowRight size={15}/>} disabled={groupsLocked} onClick={() => setSelected(taskKey(nextTask))}>下一条 {nextLabel}</Button>
          : <Button asChild variant={allDone && !editing ? "filled" : "tonal"}><Link to="/recipe/calibration">前往多轮标定<ArrowRight size={15} aria-hidden="true"/></Link></Button>}
      </span>
    </div>
  </div>;
}
