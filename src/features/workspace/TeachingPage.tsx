import { pathLength, resampleTeachingPath, type ImagePoint } from "./teachingPath";
import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { ArrowRight, Brush, Camera, Crosshair, Eraser, Hand, Info, Maximize2, Minimize2, MousePointer2, Save, SquareDashed, Trash2, TriangleAlert, Undo2, WandSparkles, Workflow } from "lucide-react";
import { Button, EmptyState, IconButton, NumberField, Popover, SegmentedControl, Surface } from "qing-steel";
import { cameraApi } from "../camera/api";
import type { CameraImaging } from "../camera/types";
import { useCycle } from "../cycle/api";
import { productionBusy as isProductionBusy } from "../cycle/phase";
import type { RecipeDoc } from "../cycle/types";
import type { MeasureMode, PathSegment, WorkspacePlanView } from "../plan/types";
import { workspaceApi } from "./api";
import { useWorkspace, type SnackMessage } from "./context";
import { Badge, FrameViewer, KV, Notice, useFrameImage, WithReason, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { CaptureRoundV2, FrozenImage, FrameImage, WorkspaceView } from "./types";
import { cameraParamsError, confirmedSegmentRange, CORNER_CORRIDOR_MM, corridorError, spacingError, draftKey, headingWindow, parseDraftKey, measureMode, sameCorridor, savedDraft, segmentDetect, withCorridor, withMeasure, withSectorDeg } from "./teach";
import { cameraParamsTasks, corridorTasks, type CameraParamsTask, type CorridorTask } from "./cameraTasks";
import { frameArrived, readRoundFrame, roundView, type RoundView } from "./captureRound";
import { SegmentTrial } from "./TeachTrial";
import { pieceLabel } from "./pieces";
import { ColorCard, HeadingCard, MeasureCard, SegmentChecksCard, SegmentDetectCard, type CameraParams } from "./CorridorSettings";
import { searchSector, type HeadingGuide, type HeadingPart } from "./headingOverlay";
import "../plan/plan.css";
import "./segments.css";
import "./corridor.css";
import WorkspaceNavigation from "./WorkspaceNavigation";
import { useRecipeHref } from "./recipeRoutes";
import { ReturnButton } from "./returnNavigation";
import { segmentLocateOf, teachingSeqFor } from "./segmentLocate";
import { BoundaryGuardField, PassSkipFields } from "./DocParams";
import PassAlignmentPanel from "./PassAlignmentPanel";
import PassStrip from "./PassStrip";
import SegmentTree from "./SegmentTree";
import SegmentParams from "./SegmentParams";
import { segmentSaveText, useSegmentDraft } from "./useSegmentDraft";
import { passTree, segmentCameras } from "./workbenchDraft";

function imagingMismatch(image: FrozenImage | null | undefined, current: CameraImaging | undefined) {
  if (!image || !current || image.exposureUs == null || image.gainDb == null) return null;
  if (image.exposureUs === current.exposureUs && image.gainDb === current.gainDb) return null;
  return `采集时曝光 ${image.exposureUs} µs / 增益 ${image.gainDb} dB，与相机当前值（${current.exposureUs} µs / ${current.gainDb} dB）不同`;
}

const DEFAULT_BRUSH_PX = 120;
/** 重刷可能提取成功的失败：灰度找不到两侧胶边、胶边连不上，彩色按胶色没吸附上 */
const RETRY_REASONS = new Set(["discontinuity", "missing_edges", "color"]);

/** 刷选提取失败后的下一步：按失败原因重刷，同一帧连续两次失败改用笔划生成走廊。胶比背景暗还是亮由后端两种都试，不用调笔刷去凑。 */
function extractionAdvice(reason: string, failures: number, brushWidth: number) {
  if (!RETRY_REASONS.has(reason)) return "";
  if (failures >= 2) return "仍无法提取（常见于胶堆、断胶等局部异常）：请点「笔划直接生成走廊」，再手工修正。";
  if (reason === "color") return `请让笔划压在胶条上重刷：笔刷宽度的一半（当前 ±${brushWidth / 2} px）是允许偏离胶中心的距离。`;
  if (reason === "discontinuity") return "可在标记处把胶路分成两段分别刷选，避开胶堆、断胶或急弯。";
  return `请沿胶条中心重刷，让笔刷盖住整条胶宽和两侧少量背景（当前 ${brushWidth} px），避开阴影和邻近胶条。`;
}

type DeferredAction = "extract" | "choose" | "clear" | "color";

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
  const [state, setState] = useState<{ key: string; image: FrameImage | null; error: string; loading: boolean }>({ key, image: null, error: "", loading: false });
  useEffect(() => {
    if (!enabled || !round) { setState({ key, image: null, error: "", loading: false }); return; }
    let alive = true;
    const request = new AbortController();
    setState({ key, image: null, error: "", loading: true });
    readRoundFrame(round, camera, seq, request.signal)
      .then(image => alive && setState({ key, image, error: "", loading: false }))
      .catch(e => alive && setState({ key, image: null, error: errorText(e), loading: false }));
    return () => { alive = false; request.abort(); };
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

/**
 * 分段与示教（段配方，路由 /recipe/<编号>/teach）：一边在示教件上按笔分段，一边示教各段走廊。
 * 左栏按笔列段与走廊状态，中栏上方是当前笔的帧条（定段边界、选候选示教帧），下面是示教帧画布，右栏是当前段的参数。
 * 段只在笔内，笔末固定是段边界；界面上段叫“笔-段”（如 2-2），内部仍是全局段号。
 */
export default function TeachingPage() {
  const { data, doc } = useWorkspace();
  if (!data || !doc) return <WorkspaceEmpty />;
  return <SegmentTeaching key={doc.id} data={data} doc={doc} plan={data.plan} />;
}

type Selection = { seq: number; camera: string; params: string | null; choosing: boolean };

function SegmentTeaching({ data, doc, plan }: { data: WorkspaceView; doc: RecipeDoc; plan: WorkspacePlanView }) {
  const href = useRecipeHref();
  const { dirty, busy, saving, acting, pendingSave, unsavable, saveNow, saveState, frameDirty, navigationDirty, corridorDrafts, setCorridorDraft, setDoc, act, setError } = useWorkspace();
  const { snapshot } = useCycle();
  const navigate = useNavigate();
  const location = useLocation();
  const [search] = useSearchParams();
  const seg = useSegmentDraft(data, plan);
  const cameraItems = cameraParamsTasks(plan);
  const tasks = corridorTasks(plan);
  const teachingPiece = data.workspace.teachingPiece;
  const teachingRoundId = teachingPiece?.roundId;
  const passSkip = data.workspace.doc.passSkip;
  const tree = seg.draft ? passTree(seg.draft, seg.passes, passSkip) : [];
  const treeSegments = tree.flatMap(node => node.segments);
  const posOf = (index: number) => seg.restructured ? -1 : plan.segments.findIndex(item => item.index === index);
  const labelOf = (index: number) => treeSegments.find(item => item.pos === posOf(index))?.label ?? `段 ${index}`;
  const teachSeqOf = (camera: string, index: number) => {
    const item = plan.cameraTeaching.find(c => c.camera === camera)?.segments.find(s => s.segment === index);
    return item && item.round === teachingRoundId ? item.seq : null;
  };
  const middleOf = (pos: number, fallback: number) => { const span = seg.spans[pos]; return span ? Math.round((span[0] + span[1]) / 2) : fallback; };
  const targetOf = (key: string, fallback: number): Selection => {
    const target = parseDraftKey(key);
    if (target) return { seq: teachSeqOf(target.camera, target.segment) ?? middleOf(posOf(target.segment), fallback), camera: target.camera, params: null, choosing: false };
    return { seq: fallback, camera: key, params: key || null, choosing: false };
  };
  const requestedCamera = search.get("camera");
  const requestedSegment = search.get("segment");
  const [sel, setSel] = useState<Selection>(() => targetOf(requestedTarget(requestedCamera, requestedSegment, cameraItems, tasks) ?? firstOpen(cameraItems, tasks), 1));
  useEffect(() => { const key = requestedTarget(requestedCamera, requestedSegment, cameraItems, tasks); if (key) setSel(current => targetOf(key, current.seq)); }, [requestedCamera, requestedSegment]);
  // 从历史件跳来时按元数据定位到示教件的帧组，不等待缩略图
  const locate = segmentLocateOf(location);
  const mapped = locate && seg.groupCount ? teachingSeqFor(locate, seg.groupCount, teachingPiece?.passes) : null;
  const located = useRef("");
  useEffect(() => {
    if (!mapped || !locate) return;
    const key = `${seg.roundId}:${locate.historyId}:${locate.seq}`;
    if (located.current === key) return;
    located.current = key;
    setSel(current => ({ ...current, seq: mapped.seq, params: null, choosing: false }));
  }, [mapped?.seq, seg.roundId]);

  const seq = Math.min(Math.max(1, sel.seq), Math.max(1, seg.groupCount));
  const pos = seg.spans.findIndex(([first, last]) => seq >= first && seq <= last);
  const node = tree.find(item => seq >= item.first && seq <= item.last);
  const treeSegment = treeSegments.find(item => item.pos === pos);
  const planSegment = !seg.restructured && pos >= 0 ? plan.segments[pos] : undefined;
  const label = treeSegment?.label ?? (planSegment ? `段 ${planSegment.index}` : "");
  const segCameras = planSegment ? segmentCameras(planSegment, plan.cameras) : [];
  const tabCameras = segCameras.length ? segCameras : plan.cameras;
  const paramsMode = sel.params != null || !seg.draft;
  const camera = sel.params ?? (tabCameras.includes(sel.camera) ? sel.camera : tabCameras[0] ?? cameraItems[0]?.camera ?? "");
  const task = !paramsMode && planSegment ? tasks.find(item => item.camera === camera && item.segment === planSegment.index) : undefined;
  const mode: "params" | "corridor" | "skip" | "pending" = paramsMode ? "params" : task ? "corridor" : planSegment?.skip || treeSegment?.skip ? "skip" : "pending";
  const segment = task?.segment ?? null;
  const key = task ? taskKey(task) : camera;
  const segmentItem = segment == null ? undefined : plan.segments.find(item => item.index === segment);
  const segmentTeaching = segment == null ? undefined : plan.cameraTeaching.find(item => item.camera === camera)?.segments.find(item => item.segment === segment);
  const frame = segmentTeaching?.frame;
  const teachSeq = segment == null ? null : teachSeqOf(camera, segment);
  const [cameraImaging, setCameraImaging] = useState<CameraImaging>();
  useEffect(() => {
    let alive = true;
    setCameraImaging(undefined);
    if (frame) void cameraApi.rigStatus().then(list => { if (alive) setCameraImaging(list.find(s => s.id === frame.camera)?.imaging); }).catch(() => {});
    return () => { alive = false; };
  }, [frame?.id, frame?.camera]);
  const targetLabel = segment == null ? camera : `${camera} · ${label}`;
  const [railWidth, setRailWidth] = useRailWidth();
  /** 专注画布：收起左栏与右栏，只留中栏 */
  const [focus, setFocus] = useState(false);
  const [working, setWorking] = useState(false);
  const [operation, setOperation] = useState<"extract" | "color" | null>(null);
  const [extractionHint, setExtractionHint] = useState("");
  const [extractionFailure, setExtractionFailure] = useState<{ point: ImagePoint; reason: string; message: string } | null>(null);
  const [vertex, setVertex] = useState<number | null>(null);
  const [region, setRegion] = useState<[number, number, number, number] | null>(null);
  const [regionMode, setRegionMode] = useState(false);
  /** 画布不在刷选、框选时点图做什么：默认只看（不加点，防误触），选“编辑点”才加点、拖点，“标出胶点”点出胶点 */
  const [pointMode, setPointMode] = useState<"view" | "edit" | "nozzle">("view");
  const [headingFocus, setHeadingFocus] = useState<HeadingPart | null>(null);
  const [selectionTool, setSelectionTool] = useState<"brush" | "box">("brush");
  const [brushPoints, setBrushPoints] = useState<ImagePoint[]>([]);
  const [brushWidth, setBrushWidth] = useState(DEFAULT_BRUSH_PX);
  const [brushFailures, setBrushFailures] = useState(0);
  const [pointSpacing, setPointSpacing] = useState(20);
  const [trialSeq, setTrialSeq] = useState<number | null>(null);
  const pending = useRef(false), serial = useRef(0);
  const scope = JSON.stringify([doc.id, key, frame?.id ?? null]);
  useEffect(() => setBrushFailures(0), [scope]);
  const current = useRef({ scope, alive: true, revision: data.workspace.revision }); current.current.scope = scope; current.current.revision = data.workspace.revision;
  useEffect(() => { current.current.alive = true; return () => { current.current.alive = false; serial.current++; }; }, []);
  useEffect(() => { serial.current++; pending.current = false; setWorking(false); setVertex(null); setRegion(null); setBrushPoints([]); setRegionMode(false); setPointMode("view"); setOperation(null); setExtractionHint(""); setExtractionFailure(null); setTrialSeq(null); }, [scope]);
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
  const { image, error, loading } = useFrameImage(doc.id, frame?.id ?? null);

  // 示教帧选择（RB-1）：帧条上点的帧组就是候选示教帧，只能选这台相机落在本段内的帧组
  const [picked, setPicked] = useState<{ round: CaptureRoundV2; view: RoundView } | null>(null);
  const [pickError, setPickError] = useState("");
  const choosingNow = segment != null && (!frame || (sel.choosing && seq !== teachSeq));
  const reviews = data.workspace.captureReview?.rounds ?? {};
  useEffect(() => {
    setPicked(null); setPickError("");
    if (!teachingRoundId) return;
    let alive = true;
    void workspaceApi.captureGet(teachingRoundId).then(round => { if (alive) setPicked(round?.recipeId === doc.id && round.state === "complete" ? { round, view: roundView(round) } : null); }).catch(e => { if (alive) setPickError(errorText(e)); });
    return () => { alive = false; };
  }, [teachingRoundId, doc.id]);
  // 单段配方所有帧都在第 1 段，不需要先确认手动分段（与后端 apply_segment_teaching 一致）
  const range = segment == null || !picked ? null : plan.segments.length === 1 ? { start: 1, end: picked.view.groupCount } : confirmedSegmentRange(reviews[picked.view.roundId], plan.segments, segment);
  const inSegment = !!range && seq >= range.start && seq <= range.end;
  const arrived = !!picked && frameArrived(picked.view, camera, seq);
  const previewing = choosingNow || mode === "skip" || mode === "pending";
  const candidate = useRoundFrame(picked?.round ?? null, camera, seq, previewing && arrived);

  const saved = savedDraft(data.workspace.doc, key);
  const t = segment == null ? saved : corridorDrafts[key] ?? saved;
  const editing = segment != null && !sameCorridor(t, saved);
  const invalid = corridorError(t, doc.spacing);
  // ready：没有显式操作在进行，按钮据此可点；自动保存不算，期间界面不变灰。
  // 提交后端的操作要等改动都存好（unlocked）：点下去时有待存的改动就先立即保存，存好再执行（deferAction）
  const alignmentPending = data.workspace.passAlignment && !data.workspace.passAlignment.confirmed ? "换示教件的逐笔对照还没确认：先在上方确认对照" : "";
  const ready = !alignmentPending && !acting && !working && !data.workspace.pending && !navigationDirty;
  const unlocked = ready && !busy && !dirty;
  const editable = !alignmentPending && (!busy || saving) && !working && !data.workspace.pending && !navigationDirty;
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
  /** 按胶色测量的段在彩色示教帧上的刷选按胶色吸附（与示教 MCP 同口径），走廊与胶色一并保存；按灰度测量的段与框选按两侧胶边提取 */
  const colorFrame = image?.channels === 3;
  const segmentCorridor = segment == null ? undefined : doc.inspect?.find(item => item.camera === camera)?.segments?.find(item => item.segment === segment);
  const measure = measureMode(segmentCorridor, colorFrame);
  const snapByColor = selectionTool === "brush" && colorFrame && measure === "color";

  /** 把这一段的走廊草稿并入候选配方并整份保存（段走廊没有单独的草稿命令）。 */
  const commit = (message: SnackMessage | ((next: WorkspaceView) => SnackMessage)) => perform(() => workspaceApi.saveDoc(doc.id, data.workspace.revision, withCorridor(data.workspace.doc, key, t)), message);
  const extract = async () => {
    if (!canEdit || !frame || pending.current) return;
    if (!roiValid || !roi || !spacingValid) { setRegionMode(true); setExtractionHint("请沿胶路刷过至少 8 px，并设置有效的笔刷宽度和走廊点间距。"); return; }
    if (snapByColor && !(t.mmPerPx != null && t.mmPerPx > 0)) { setRegionMode(true); setExtractionHint(`彩色示教帧按胶色吸附，要用像素当量把胶宽换成像素：${camera} 还没有像素当量，先在左侧“相机参数”里填写或采用设备标定当量，再自动提取。`); return; }
    if (!unlocked) return;
    const capturedScope = scope;
    setOperation("extract"); setExtractionHint(""); setExtractionFailure(null);
    try {
      let candidateView = data;
      if (editing) {
        const savedView = await commit("");
        if (!savedView) return;
        candidateView = savedView;
      }
      if (snapByColor && segment != null) {
        let snapped: Awaited<ReturnType<typeof workspaceApi.snapCorridor>> | null = null;
        const result = await perform(async () => {
          snapped = await workspaceApi.snapCorridor(doc.id, candidateView.workspace.revision, camera, segment, frame.id, { points: brushPoints, radiusPx: brushWidth / 2 });
          return snapped.view ?? candidateView;
        }, "");
        if (!result || !snapped || current.current.scope !== capturedScope) return;
        const output = snapped as Awaited<ReturnType<typeof workspaceApi.snapCorridor>>;
        if (!output.view) {
          const failures = brushFailures + 1;
          setBrushFailures(failures);
          // 胶色吸附不报位置：失败标记只用于提示样式与“笔划直接生成走廊”主操作，画布上不画点
          setExtractionFailure({ point: brushPoints[0], reason: "color", message: output.message });
          setExtractionHint(output.message + " " + extractionAdvice("color", failures, brushWidth)); setRegionMode(true); return;
        }
        setBrushFailures(0);
        setCorridorDraft(key, savedDraft(output.view.workspace.doc, key)); setRegionMode(false); setVertex(null);
        setExtractionHint(output.message);
        return;
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
          const failures = selectionTool === "brush" && RETRY_REASONS.has(output.failure.reason) ? brushFailures + 1 : 0;
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
    if (!unlocked || segment == null || !picked || !arrived || !inSegment || seg.dirty || pending.current) return;
    const result = await perform(() => workspaceApi.teachSegmentCorridor(doc.id, data.workspace.revision, camera, segment, picked.view.roundId, seq), `${targetLabel} 的示教帧已选定，请在图上示教走廊`);
    if (result) setSel(current => ({ ...current, choosing: false }));
  };
  const saveCorridor = () => { if (unlocked && editing && !invalid) void commit(`${targetLabel} 的走廊已保存到候选配方`); };
  /** 要提交后端的操作：有待存的改动时先立即保存，存好再执行（执行时取那时最新的修订与配置）；保存失败就不执行 */
  const [deferred, setDeferred] = useState<DeferredAction | null>(null);
  const runAction = (action: DeferredAction) => action === "extract" ? void extract() : action === "choose" ? void chooseFrame() : action === "color" ? void fitColor() : clearSegment();
  // 还没刷选就点提取：直接进刷选模式说明怎么做，不用等保存
  const startExtract = () => { if (!roiValid || !roi || !spacingValid) void extract(); else deferAction("extract"); };
  const deferAction = (action: DeferredAction) => { if (pendingSave) { setDeferred(action); saveNow(); } else runAction(action); };
  useEffect(() => {
    if (!deferred) return;
    if (saveState === "failed") { setDeferred(null); return; }
    if (!pendingSave && !acting) { setDeferred(null); runAction(deferred); }
  }, [deferred, pendingSave, acting, saveState]);
  const fitColor = async () => {
    if (!unlocked || segment == null || editing || pending.current) return;
    const capturedScope = scope;
    setOperation("color");
    try { await perform(() => workspaceApi.fitColorModel(doc.id, data.workspace.revision, camera, segment), `${targetLabel} 已取色，这一段按胶色测量`); }
    finally { if (current.current.scope === capturedScope) setOperation(null); }
  };
  const clearSegment = () => { if (unlocked && segment != null && !editing) void perform(() => workspaceApi.clearSegmentCorridor(doc.id, data.workspace.revision, camera, segment), `${targetLabel} 的走廊与示教帧已清除`); };
  const changeSegments = (segments: PathSegment[]) => setDoc({ ...doc, segments });
  const inspect = doc.inspect?.find(item => item.camera === camera);
  const params: CameraParams = { mmPerPx: inspect?.mmPerPx ?? null, window: inspect?.window ?? null, nozzle: inspect?.nozzle ?? null };
  const setParams = (next: CameraParams) => setDoc({ ...doc, inspect: (doc.inspect ?? []).map(item => item.camera === camera ? { ...item, ...next } : item) });
  // 搜索扇区方向按（相机, 段）存在这一段的走廊项上，拖动只改本段
  const sectorDeg = segment == null ? null : inspect?.segments?.find(item => item.segment === segment)?.sectorDeg ?? null;
  const colorModel = segmentCorridor?.colorModel;
  const changeMeasure = (next: MeasureMode) => { if (segment != null && next !== measure) setDoc(withMeasure(doc, camera, segment, next)); };
  const colorBlocked = saved.corridor.length < 2 ? "这一段还没有保存的走廊：先画好走廊并保存，再取色"
    : editing ? "走廊有未保存的修改：保存后再取色"
      : !(saved.mmPerPx != null && saved.mmPerPx > 0) ? `${camera} 缺像素当量：先在相机参数里设置`
        : data.workspace.pending ? "候选版本正在等待生效" : !ready ? "正在处理，请稍候" : "";
  const setSectorDeg = (deg: number | null) => { if (segment != null && deg !== sectorDeg) setDoc(withSectorDeg(doc, camera, segment, deg)); };
  // 出胶点、测量区间与搜索扇区都按候选配方即时算，保存前后显示不换来源
  const savedNozzle = plan.cameraTeaching.find(item => item.camera === camera)?.nozzle ?? null;
  const derivedNozzle = savedNozzle && !savedNozzle.calibrated ? savedNozzle.point : null;
  const nozzleWarning = !params.nozzle && savedNozzle && !savedNozzle.calibrated ? savedNozzle.warning : null;
  const segmentDoc = segment == null ? undefined : doc.segments?.find(item => item.index === segment);
  const headingOn = !!segmentDoc?.heading;
  const zoneWindow = headingWindow(segmentDoc, params).window;
  const cameraScale = params.mmPerPx != null && Number.isFinite(params.mmPerPx) && params.mmPerPx > 0 ? params.mmPerPx : null;
  const nozzlePoint = params.nozzle ?? derivedNozzle;
  const headingGuide: HeadingGuide | null = headingOn && nozzlePoint ? {
    nozzle: nozzlePoint, calibrated: !!params.nozzle, focus: headingFocus, turned: sectorDeg != null,
    zone: zoneWindow && cameraScale ? [zoneWindow.startMm / cameraScale, (zoneWindow.startMm + zoneWindow.lengthMm) / cameraScale] : null,
    sector: frame ? searchSector(nozzlePoint, frame.size, sectorDeg) : null,
  } : null;

  if (!cameraItems.length) return <div className="wp-page"><WorkspaceBar /><Surface><EmptyState icon={<Camera size={32} aria-hidden="true" />} title="段配方还没有计划相机" description="计划相机取自配方的检测相机，配好相机后再分段与示教走廊。" action={<Button asChild><Link to={href("/recipe/progress")}>返回总览<ArrowRight size={15} aria-hidden="true" /></Link></Button>} /></Surface></div>;

  // 能自动保存的草稿切走后照常保存，不拦；按站距不足三站的草稿存不了，切走会以为丢了，先拦住
  const groupsLocked = !ready || unsavable;
  const groupLockReason = alignmentPending || (navigationDirty ? "请先保存或撤销导航图修改" : unsavable ? "走廊有未保存的修改（按站距不足三站，不会自动保存），先改好或清空后再切换" : data.workspace.pending ? "候选版本正在等待生效" : "正在处理，请稍候");
  const segmentOfSeq = (s: number) => seg.spans.findIndex(([first, last]) => s >= first && s <= last);
  const focusPos = (target: number, cam?: string) => {
    const item = seg.restructured ? undefined : plan.segments[target];
    const cams = item ? segmentCameras(item, plan.cameras) : [];
    const nextCamera = cam ?? (cams.includes(camera) ? camera : cams[0] ?? camera);
    const taught = item ? teachSeqOf(nextCamera, item.index) : null;
    setSel({ seq: taught ?? middleOf(target, seq), camera: nextCamera, params: null, choosing: false });
    setTrialSeq(null);
  };
  const focusTask = (item: CorridorTask) => focusPos(posOf(item.segment), item.camera);
  const selectSeq = (next: number) => {
    const target = Math.min(Math.max(1, next), Math.max(1, seg.groupCount));
    if (groupsLocked && segmentOfSeq(target) !== pos) return;
    setSel(current => ({ ...current, seq: target, params: null, choosing: true }));
    setTrialSeq(null);
  };
  const openParams = (cam: string) => { setSel(current => ({ ...current, params: cam, choosing: false })); setTrialSeq(null); };

  const imagingHint = imagingMismatch(frame, cameraImaging);
  const at = tasks.findIndex(item => taskKey(item) === key);
  const open = tasks.filter(item => item.status !== "done" && taskKey(item) !== key);
  const nextTask = open.find(item => tasks.indexOf(item) > at) ?? open[0];
  const allDone = cameraItems.every(item => item.ready);
  const scaled = t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0;
  const searchHalfPx = scaled ? segmentDetect(doc.detect, segmentDoc).searchMm / t.mmPerPx! : undefined;
  const hasPath = t.corridor.length >= 2;
  const [lengthValue, lengthUnit] = scaled ? [(lengthPx * t.mmPerPx!).toFixed(1), "mm"] : [lengthPx.toFixed(0), "px"];
  const cornerLong = segmentItem?.kind === "corner" && scaled && lengthPx * t.mmPerPx! > CORNER_CORRIDOR_MM;
  const done = !!segmentTeaching?.saved;
  const navigationRanges: [number, number][] | null = !seg.restructured && seg.spans.length === plan.segments.length && seg.spans.length ? seg.spans : null;
  const navigateMap = (nextSegment: number, nextSeq: number) => {
    if (groupsLocked) return;
    const target = tasks.find(item => item.segment === nextSegment && item.camera === camera) ?? tasks.find(item => item.segment === nextSegment);
    setSel({ seq: nextSeq, camera: target?.camera ?? camera, params: null, choosing: true }); setTrialSeq(null);
  };
  const nextLabel = nextTask && `${labelOf(nextTask.segment)} ${nextTask.camera}`;
  const [stateText, stateTone]: [string, "ok" | "warn" | "info" | "neutral"] = mode === "params" ? (cameraItems.find(item => item.camera === camera)?.mmPerPx == null ? ["缺像素当量", "warn"] : ["像素当量已设", "ok"])
    : mode === "skip" ? ["不检测", "neutral"] : mode === "pending" ? ["分段待保存", "warn"]
      : !frame ? ["待选帧", "neutral"] : data.workspace.pending ? ["候选待生效", "warn"]
        : choosingNow ? ["选帧中", "info"] : editing && invalid ? ["走廊已修改 · 未完成", "warn"] : done ? ["已示教", "ok"] : hasPath ? ["已保存 · 待检查", "info"] : ["待画走廊", "neutral"];
  // 刷选连续两次找不到连续边缘后，笔划直接生成走廊是唯一的主操作
  const strokeAdvised = selectionTool === "brush" && brushFailures >= 2 && !!extractionFailure;
  const primary = segment == null ? "next" : strokeAdvised ? "stroke" : !hasPath ? "extract" : "next";
  const saveReason = !editing ? (done ? "走廊已保存" : "先提取或绘制走廊") : invalid ? invalid : !ready ? (data.workspace.pending ? "候选版本正在等待生效" : "正在处理，请稍候") : "";
  const doneHint: ReactNode = nextLabel ? `已完成；下一条 ${nextLabel}` : allDone ? <>各段走廊都已示教，可到 <Link to={href("/recipe/calibration")}>试测</Link></> : "已完成";
  const dockHint: ReactNode = mode === "params" ? (params.mmPerPx == null ? `${camera} 缺像素当量：在右侧填写或采用设备标定当量，各段走廊都按它换算` : `像素当量与检测参数改动自动保存到候选配方；走廊在左侧按笔、段逐条示教`)
    : mode === "skip" ? `${label} 不检测：不示教走廊；要检测时在右侧把类型改为直线或弯道`
      : mode === "pending" ? "分段改动保存后再示教走廊"
        : !frame ? `在上方帧条里点 ${label} 内的一个帧组，再用它示教 ${camera}`
          : data.workspace.pending ? "等待当前工件结束后切换，期间不能修改示教"
            : working ? "正在处理或保存，请稍候"
              : choosingNow ? "选定新帧后，这一段现有走廊原样保留"
                : editing && invalid ? invalid
                  : cornerLong ? `弯道走廊超过 ${CORNER_CORRIDOR_MM} mm：胶条绕出胶点转动，建议只从出胶点起画一小段`
                    : !scaled ? `${camera} 缺像素当量：先在相机参数里设置，再保存走廊`
                      : done ? doneHint
                        : segmentItem?.kind === "corner" ? `弯道：从出胶点起沿胶条画一小段（不超过 ${CORNER_CORRIDOR_MM} mm）`
                          : "直线段：从出胶点一侧起，沿胶条方向画整段走廊";
  const picking = pointMode === "nozzle" && headingOn;
  const pointing = pointMode === "edit";
  const editHint = !picking && !pointing ? "只看：点图不改走廊，可缩放、平移、拖动扇区把手；要加点或拖点请选“编辑点”。" : picking ? `点击胶嘴出胶处标定 ${camera} 的出胶点（${camera} 各段共用，自动保存到候选配方）；叠加图上的编号与右侧“逐帧定向”面板各行对应，指着面板某一行时图上只突出它。`
    : "点击图像加点（绿圈为起点），拖动点可调整；密集走廊打开“点柄”显示全部点，Ctrl＋滚轮缩放。";
  const regionHint = extractionHint || (snapByColor ? "彩色示教帧：按住左键沿胶条从起点刷到终点，自动提取按胶色吸附到胶中心，走廊与胶色一并保存；笔刷宽度的一半是允许偏离胶中心的距离。"
    : selectionTool === "brush" ? "按住左键沿胶路从起点刷到终点，每笔重画选区；笔刷覆盖胶路和两侧少量背景，尽量避开阴影。胶比背景暗还是亮自动判断。" : "按住左键框选一段连续胶路，再自动提取走廊；框选包含阴影时建议改用刷选。");
  const changeMode = (value: string) => {
    if (value === "view" || value === "edit" || value === "nozzle") { setPointMode(value); setRegionMode(false); return; }
    const tool = value as "brush" | "box";
    if (tool !== selectionTool) { setSelectionTool(tool); setExtractionFailure(null); setExtractionHint(""); setBrushFailures(0); }
    setRegionMode(true);
  };
  const modeLabel = (icon: ReactNode, long: string, short: string) => <span className="wp-seg-label" title={long}><span className="wp-seg-icon">{icon}</span><span className="wp-seg-short">{short}</span></span>;
  const brushing = regionMode && selectionTool === "brush";
  const focusToggle = <IconButton size="sm" aria-label="专注画布" aria-pressed={focus} title={focus ? "退出专注：恢复左栏与右栏" : "专注画布：收起左栏与右栏"} onClick={() => setFocus(v => !v)}>{focus ? <Minimize2 size={15}/> : <Maximize2 size={15}/>}</IconButton>;
  const tools = <div className="wp-canvas-tools" role="group" aria-label="示教工具">
    <SegmentedControl size="sm" label="画布模式" value={regionMode ? selectionTool : picking ? "nozzle" : pointMode === "edit" ? "edit" : "view"} disabled={!canEdit || (!frame && !regionMode)} options={[
      { value: "view", label: modeLabel(<Hand size={14} aria-hidden="true"/>, "只看：点图不改走廊", "只看"), "aria-label": "只看" },
      { value: "edit", label: modeLabel(<MousePointer2 size={14} aria-hidden="true"/>, "编辑点：点图加点、拖动点", "编辑"), "aria-label": "编辑点" },
      { value: "brush", label: modeLabel(<Brush size={14} aria-hidden="true"/>, "沿胶路刷选", "刷选"), "aria-label": "沿胶路刷选" },
      { value: "box", label: modeLabel(<SquareDashed size={14} aria-hidden="true"/>, "矩形框选", "框选"), "aria-label": "矩形框选" },
      ...(headingOn ? [{ value: "nozzle", label: modeLabel(<Crosshair size={14} aria-hidden="true"/>, "标出胶点", "出胶点"), "aria-label": "标出胶点" }] : []),
    ]} onValueChange={changeMode}/>
    {brushing && <span className="wp-inline-num" title={snapByColor ? "笔刷宽度：一半是允许偏离胶中心的距离" : "笔刷宽度：约为胶宽的 3 倍较稳"}>笔刷<NumberField aria-label="笔刷宽度" unit="px" min={6} max={300} step={2} value={Number.isFinite(brushWidth) ? brushWidth : ""} disabled={!canEdit} onValueChange={value => changeBrushWidth(value === "" ? NaN : value)}/></span>}
    {(regionMode || pointing) && <span className="wp-inline-num" title="走廊点间距：提取与按笔划生成都按它取点，转弯处保留点">点距<NumberField aria-label="走廊点间距" unit="px" min={4} max={80} step={2} value={Number.isFinite(pointSpacing) ? pointSpacing : ""} disabled={!canEdit} onValueChange={value => setPointSpacing(value === "" ? NaN : value)}/></span>}
    {pointing && <Button size="sm" variant="text" aria-label="应用到当前走廊" title="按点距重排当前走廊的点" disabled={!canEdit || !spacingValid || t.corridor.length < 2} onClick={() => { setVertex(null); setPath(resampleTeachingPath(t.corridor, pointSpacing)); }}>按点距重排</Button>}
    <Button size="sm" variant={primary === "extract" ? "filled" : "tonal"} data-variant={primary === "extract" ? "filled" : "tonal"} leadingIcon={<WandSparkles size={14}/>} disabled={!canEdit || !ready || loading || !!error} onClick={startExtract}>{operation === "extract" ? "提取中…" : "自动提取走廊"}</Button>
    {brushing && <Button size="sm" variant={primary === "stroke" ? "filled" : "tonal"} data-variant={primary === "stroke" ? "filled" : "tonal"} title={!strokeValid ? "先沿胶路刷过至少 8 px" : "按笔划生成走廊草稿"} disabled={!canEdit || !strokeValid || !spacingValid} onClick={fromStroke}>笔划直接生成走廊</Button>}
    <span className="wp-canvas-edit">
      <IconButton size="sm" aria-label="删除末点" title="删除末点" disabled={!canEdit || !t.corridor.length} onClick={() => { setVertex(null); setPath(t.corridor.slice(0, -1)); }}><Undo2 size={15}/></IconButton>
      <IconButton size="sm" aria-label="删除选中点" title="删除选中点" disabled={!canEdit || vertex === null || vertex >= t.corridor.length} onClick={() => { const i = vertex; setVertex(null); setPath(t.corridor.filter((_, j) => j !== i)); }}><Trash2 size={15}/></IconButton>
      <IconButton size="sm" aria-label="清空走廊" title="清空走廊" disabled={!canEdit || !t.corridor.length} onClick={() => { setVertex(null); setPath([]); }}><Eraser size={15}/></IconButton>
    </span>
  </div>;
  const canvasHint = regionMode ? <p className={"wp-canvas-hint" + (extractionFailure ? " warn" : "")} role="status">{extractionFailure && <TriangleAlert size={14} aria-hidden="true"/>}<span>{regionHint}</span></p>
    : <p className="wp-canvas-hint" role="status">{extractionHint && <><span>{extractionHint}</span> </>}<span>{editHint}</span></p>;
  const summary = <span className="wp-trial-inline">
    <Badge tone={done && !editing ? "ok" : "neutral"}>{editing ? "走廊未保存" : done ? "段走廊已示教" : "待画走廊"}</Badge>
    <span className="muted">走廊 <b>{t.corridor.length}</b> 点 · <b>{lengthValue}</b> {lengthUnit}</span>
    {cornerLong && <Badge tone="warn">弯道走廊偏长</Badge>}
  </span>;
  const imageInfo = frame && <Popover side="bottom" align="end" label="图像采集信息" className="wp-image-info" title={`示教帧 · ${targetLabel}`} trigger={<IconButton size="sm" aria-label="图像采集信息"><Info size={15}/></IconButton>}>
    <KV label="图像">{`${frame.id} · ${frame.size[0]} × ${frame.size[1]} px`}</KV>
    <KV label="取样来源">{frame.source} · {new Date(frame.capturedAt).toLocaleString("zh-CN")}</KV>
    <KV label="曝光">{frame.exposureUs != null ? frame.exposureUs + " μs" : "未记录"}</KV><KV label="增益">{frame.gainDb != null ? frame.gainDb + " dB" : "未记录"}</KV>
    <KV label="示教帧更换">在帧条上点本段的另一帧组，再“用此帧示教”</KV>
    {teachSeq == null && <p className="muted" role="note">示教帧来自原示教件，帧条上没有对应位置</p>}
    {imagingHint && <p className="c-warn" role="note">{imagingHint}；只作提示，示教与标定仍然有效</p>}
    <p className="muted hint">走廊保存进候选配方，已发布版本不变。用于生产前仍需试测与发布。</p>
  </Popover>;
  const frameTitle = <h2 className="wp-trial-title" title={frame ? `${frame.source} · ${frame.id}` : undefined}>{targetLabel}<small>示教帧{teachSeq != null ? ` · 帧组 ${teachSeq}` : ""}</small></h2>;
  const pickReason = alignmentPending || (!teachingPiece ? "还没有示教件：先选示教件" : !picked ? "正在读取示教件" : seg.dirty ? "分段还没保存：保存后再选示教帧" : !range ? "示教件的分段边界还没确认"
    : !inSegment ? `${label} 在示教件为帧组 ${range.start}–${range.end}` : !arrived ? "该相机本帧组缺帧，请换一个帧组" : !ready ? "正在处理，请稍候" : "");
  const pickBar = <Surface role="region" className="wp-picker wp-corridor-picker" aria-label="示教帧选择">
    <h2>{targetLabel}<small>候选示教帧 · 帧组 {seq}</small></h2>
    <div className="wp-corridor-pick">{range && <span className="muted">{label} 在示教件为帧组 {range.start}–{range.end}</span>}</div>
    <div className="wp-picker-side">
      <WithReason reason={pickReason}>
        <Button variant="filled" data-variant="filled" disabled={!!pickReason} onClick={() => deferAction("choose")}>用此帧示教 {label} {camera}</Button>
      </WithReason>
      {frame && <Button variant="text" onClick={() => setSel(current => ({ ...current, seq: teachSeq ?? current.seq, choosing: false }))}>回到示教帧</Button>}
    </div>
  </Surface>;
  const cameraTasks = tasks.filter(item => item.camera === camera);
  const trialReview = segmentTeaching ? reviews[segmentTeaching.round] : undefined;
  const trialGroups = picked && picked.round.roundId === segmentTeaching?.round ? picked.view.groupCount : trialReview?.groupCount ?? 0;
  const savedSegments = data.workspace.doc.segments ?? [];
  const trialRangeFor = (index: number) => savedSegments.length === 1
    ? (trialGroups > 0 ? { start: 1, end: trialGroups } : null)
    : confirmedSegmentRange(trialReview, savedSegments, index);
  const trialRange = segment == null ? null : trialRangeFor(segment);
  const guard = data.workspace.doc.boundaryGuard ?? 0;
  const guardRanges = guard > 0 ? savedSegments.filter(item => item.skip).flatMap(item => {
    const span = trialRangeFor(item.index);
    return span ? [{ start: Math.max(1, span.start - guard), end: Math.min(trialGroups, span.end + guard), segment: item.index }] : [];
  }) : [];
  const trialLocked = productionBusy ? "工件正在检测，结束后再试测" : editing && invalid ? `走廊未完成：${invalid}` : !ready ? "正在处理，请稍候" : !hasPath ? "这一段还没有走廊" : "";
  const paramsError = cameraParamsError(params);
  const paramsDisabled = !editable;
  const cameraTabs = mode !== "params" && <div className="st-cam-tabs">
    <SegmentedControl size="sm" label="示教相机" value={camera} disabled={groupsLocked} options={tabCameras.map(id => ({ value: id, label: id }))}
      onValueChange={id => mode === "corridor" ? focusPos(pos, id) : setSel(current => ({ ...current, camera: id }))} />
    {mode === "corridor" && <span className="muted">{label} 由 {segCameras.join("、")} 检测</span>}
  </div>;
  const previewViewer = <FrameViewer controlsAt="footer" image={candidate.image} loading={candidate.loading} error={candidate.error || (picked && !arrived ? "该相机本帧组缺帧" : "")} label={`${camera} · 帧组 ${seq}${choosingNow ? " · 候选示教帧" : ""}`}
    emptyTitle={teachingPiece ? "示教件原图" : "还没有示教件"} emptyHint="在上方帧条里点帧组查看这台相机的原图。"
    summary={<span className="muted">{choosingNow ? `选定后冻结为 ${label} 的示教帧，这条走廊只用于这一段的帧。` : `帧组 ${seq}${label ? ` · ${label}` : ""}`}</span>}/>;
  const middle = mode === "params"
    ? <Surface role="region" className="wp-seg-corridor-note" aria-label="相机参数">
      <h3>{camera} · 相机参数</h3>
      <p className="muted">像素当量、出胶点与测量区间按相机设置，{camera} 检测的各段都按它们测量；检测参数取配方默认，段可单独设搜索半宽与胶宽范围。改动自动保存到候选配方。走廊在左侧按笔、段逐条示教。</p>
      {paramsError && <p className="c-warn" role="alert">{paramsError}</p>}
      {cameraTasks.length ? <ul className="wp-camera-segments" aria-label={`${camera} 检测的段`}>{cameraTasks.map(item => <li key={item.segment}><span>{labelOf(item.segment)} · {item.kind === "corner" ? "弯道" : "直线"}</span><Badge tone={item.status === "done" ? "ok" : item.frame ? "warn" : "neutral"}>{item.label}</Badge></li>)}</ul>
        : <p className="muted">没有段把 {camera} 选为检测相机。</p>}
      <WithReason reason={cameraTasks.some(item => item.status === "done") ? "" : `${camera} 还没有保存的段走廊`}><Button variant="text" size="sm" leadingIcon={<Workflow size={14}/>} disabled={!cameraTasks.some(item => item.status === "done")}
        onClick={() => navigate(href(`/recipe/flow?camera=${encodeURIComponent(camera)}`))}>流程调试</Button></WithReason>
    </Surface>
    : mode !== "corridor" ? <>
      {cameraTabs}
      <Notice title={mode === "skip" ? `${label} 不检测` : "分段改动还没保存"}>{mode === "skip" ? "这一段照常计帧，不测不判，结果列为未检区域；不用示教走廊。" : "分段保存后再按新的段示教走廊。"}</Notice>
      {previewViewer}
    </>
    : <>
      {cameraTabs}
      {choosingNow && pickBar}
      {pickError && choosingNow && <p className="c-warn wp-picker-warn" role="alert">{pickError}</p>}
      {!choosingNow && frame && (segmentTeaching
        ? <SegmentTrial key={scope} id={doc.id} camera={camera} segment={segment!} round={segmentTeaching.round} teachSeq={segmentTeaching.seq}
          skipRules={{ passes: data.workspace.teachingPiece?.passes, groupCount: trialGroups, passSkip: data.workspace.doc.passSkip, segment: savedSegments.find(item => item.index === segment), guardRanges }}
          range={trialRange} revision={data.workspace.revision} locked={trialLocked} waiting={pendingSave} onFlush={saveNow} guide={headingGuide} seq={trialSeq} onSeq={setTrialSeq}
          lead={frameTitle} side={imageInfo} />
        : <Surface role="region" className="wp-trial" aria-label="示教帧"><div className="wp-trial-head">{frameTitle}<span className="wp-trial-actions">{imageInfo}</span></div></Surface>)}
      {choosingNow
        ? previewViewer
        : trialSeq != null ? null : <FrameViewer controlsAt="footer" summary={summary} image={image} loading={loading} error={error} label={frame ? `${targetLabel} · 示教帧` : "等待取样"} tools={tools} notice={canvasHint} emptyTitle="示教帧尚未就绪" emptyHint="选定示教帧后，这里显示这一帧。"
          region={selectionTool === "box" ? region : null}
          onRegion={regionMode && selectionTool === "box" && canEdit ? setRegion : undefined}
          brush={regionMode && selectionTool === "brush" && Number.isFinite(brushWidth) ? { points: brushPoints, radiusPx: brushWidth / 2 } : null} onBrush={regionMode && selectionTool === "brush" && canEdit ? points => { setBrushPoints(points); setExtractionFailure(null); setExtractionHint(""); } : undefined}
          extractionFailure={extractionFailure?.reason === "color" ? null : extractionFailure}
          overlay={{ path: t.corridor, searchHalfPx, selected: vertex, heading: headingGuide }}
          editDisabled={!canEdit}
          onPoint={picking && frame ? p => setParams({ ...params, nozzle: p }) : undefined}
          onSectorTurn={frame && !regionMode && !picking ? setSectorDeg : undefined}
          onEdit={frame && !regionMode && pointing ? { add: p => setPath([...t.corridor, p]), move: (i, p) => setPath(t.corridor.map((q, j) => j === i ? p : q)), select: setVertex } : undefined} />}
    </>;
  const checksDisabled = !editable;
  const sharing = plan.cameras.filter(item => item !== camera);
  const cameraScaleText = cameraScale != null ? `${Number(cameraScale.toPrecision(6))} mm/px` : "缺像素当量";
  const navigation = <details className="st-nav" open={navigationDirty || undefined}><summary>工件导航图</summary>
    <WorkspaceNavigation segments={plan.segments} ranges={navigationRanges} seq={seq}
      currentSegment={planSegment?.index} frameStatus={navigationRanges ? (seg.dirty ? "draft" : "confirmed") : "unavailable"} editable
      locked={acting || working || data.workspace.pending || unsavable ? groupLockReason : seg.dirty ? "分段还没保存：保存后再编辑地图" : ""} onNavigate={navigateMap} />
  </details>;
  const rail = mode === "params"
    ? <div className="wp-stack wp-teach-settings">
      <MeasureCard key={camera} params={params} calibMmPerPx={plan.cameraTeaching.find(item => item.camera === camera)?.calibMmPerPx ?? null} defaults={doc.detect} spacing={doc.spacing} sharing={sharing} disabled={paramsDisabled || frameDirty} spacingError={value => spacingError(doc, value)}
        onParams={setParams} onDefaults={detect => setDoc({ ...doc, detect })} onSpacing={spacing => setDoc({ ...doc, spacing })} />
      <HeadingCard key={`heading-${camera}`} camera={camera} params={params} derivedNozzle={derivedNozzle} nozzleWarning={nozzleWarning} segments={doc.segments ?? []} index={null} sector={null}
        headingSegments={(doc.segments ?? []).filter(item => item.heading && cameraTasks.some(task => task.segment === item.index))}
        disabled={paramsDisabled || frameDirty} sharedLocked="" onFocus={setHeadingFocus}
        onSegments={changeSegments} onParams={setParams} />
      <p className="wp-settings-hint wp-rail-note">段的逐帧定向、检测参数覆盖与检测项：在左侧选段后设置。</p>
      {navigation}</div>
    : <div className="wp-stack wp-teach-settings">
      {pos >= 0 && <SegmentParams key={pos} seg={seg} pos={pos} label={label} segments={doc.segments ?? []} cameras={plan.cameras} disabled={!editable} onSegments={changeSegments}><Badge tone={stateTone}>{stateText}</Badge></SegmentParams>}
      {mode === "corridor" && <>
        <Surface role="region" className="wp-seg-corridor-note" aria-label="段走廊说明">
          <div className="wp-seg-head"><h3>{camera} 走廊</h3></div>
          <p className="muted">{segmentItem?.kind === "corner" ? `弯道：从出胶点起画一小段（不超过 ${CORNER_CORRIDOR_MM} mm），只判胶宽、断胶、溢胶` : "直线：从出胶点一侧起，沿胶条画整段"}</p>
          <WithReason reason={editing && invalid ? "走廊有未完成的修改" : !ready ? "正在处理，请稍候" : !frame && !hasPath ? "这一段还没有示教帧和走廊" : ""}><Button variant="text" size="sm" disabled={(editing && !!invalid) || !ready || (!frame && !hasPath)} onClick={() => deferAction("clear")}>清除本段走廊</Button></WithReason>
          <WithReason reason={editing ? "走廊有未保存的修改：保存后再调试" : !done ? "这一段还没有保存的走廊" : ""}><Button variant="text" size="sm" leadingIcon={<Workflow size={14}/>} disabled={editing || !done}
            onClick={() => navigate(href(`/recipe/flow?camera=${encodeURIComponent(camera)}&segment=${segment}&seq=${trialSeq ?? segmentTeaching?.seq ?? ""}`))}>流程调试</Button></WithReason>
        </Surface>
        {segmentItem && <HeadingCard key={`heading-${camera}-${segment}`} camera={camera} params={params} derivedNozzle={derivedNozzle} nozzleWarning={nozzleWarning} segments={doc.segments ?? []} index={segment} headingSegments={[]}
          sector={{ range: headingGuide?.sector ?? null, turned: sectorDeg != null, onReset: () => setSectorDeg(null) }} disabled={checksDisabled}
          sharedLocked={editing || frameDirty ? "走廊有未保存的修改：保存后再改出胶点与测量区间。" : ""} onFocus={setHeadingFocus}
          onSegments={changeSegments} onParams={setParams} />}
        {segmentItem && frame && <ColorCard key={`color-${camera}-${segment}-${image?.channels ?? "-"}`} camera={camera} segment={segment!} colorModel={colorModel} color={image ? image.channels === 3 : null}
          measure={measure} measureLocked={checksDisabled ? "当前不能修改" : editing ? "走廊有未保存的修改：保存后再改测量方式" : ""} blocked={colorBlocked} edited={editing} working={operation === "color"} onMeasure={changeMeasure} onFit={() => deferAction("color")} />}
        {segmentItem && <SegmentDetectCard key={`detect-${segment}`} segments={doc.segments ?? []} index={segment!} defaults={doc.detect} disabled={checksDisabled} onChange={changeSegments} />}
        {segmentItem && <SegmentChecksCard key={`checks-${segment}`} segments={doc.segments ?? []} index={segment!} cameras={plan.cameras} disabled={checksDisabled} onChange={changeSegments} />}
        <WithReason block reason={groupsLocked ? groupLockReason : ""}><Button variant="outlined" className="wp-rail-link" disabled={groupsLocked} onClick={() => openParams(camera)}>
          <strong>测量</strong><span>{cameraScaleText}</span><span className="wp-rail-link-go">去 {camera} 改<ArrowRight size={13} aria-hidden="true" /></span>
        </Button></WithReason>
      </>}
      {navigation}</div>;
  const corridorsOf = (target: number) => {
    const item = seg.restructured ? undefined : plan.segments[target];
    return item ? tasks.filter(task => task.segment === item.index).map(task => ({ camera: task.camera, status: task.status })) : null;
  };
  const alignment = data.workspace.passAlignment;
  const alignmentPanel = alignment && <PassAlignmentPanel alignment={alignment} plan={plan} locked={seg.edited || dirty || frameDirty || navigationDirty ? "有未保存的改动：保存或撤销后再改对照" : ""} />;
  const segmentFailed = seg.status === "failed";
  const topBar = <Surface role="group" className="st-topbar" aria-label="示教件与分段">
    <span className="st-piece">{teachingPiece ? <>示教件 · {pieceLabel(teachingPiece)}</> : <>还没有示教件，<Link className="wp-text-link" to={href("/recipe/capture")}>选示教件</Link></>}</span>
    {seg.draft && <span className="muted" aria-label="笔、帧组与段数">{seg.passes.length} 笔 · {seg.groupCount} 帧组 · {seg.spans.length} 段</span>}
    <PassSkipFields locked={alignmentPending} />
    <BoundaryGuardField locked={alignmentPending} />
    <span className="st-save" role="status" aria-label="分段保存状态"><Badge tone={segmentFailed || seg.status === "confirm" || seg.status === "invalid" ? "warn" : "neutral"}>{segmentSaveText[seg.status]}</Badge>
      {seg.status === "invalid" && <span className="c-warn">至少要有一段检测</span>}
      {segmentFailed && <Button size="sm" onClick={seg.retry}>重试保存分段</Button>}</span>
  </Surface>;
  return <div className={"wp-page wp-teach-page st-page" + (focus ? " wp-teach-focus" : "")}><WorkspaceBar />
    <ReturnButton disabled={seg.dirty} hint={seg.dirty ? "分段还没保存完：保存后再返回。" : locate ? "调整好分段后返回，在历史件上重新复测。" : undefined} />
    {alignmentPanel && (alignment!.confirmed ? <details className="st-align"><summary>按笔对照（已确认）</summary>{alignmentPanel}</details> : alignmentPanel)}
    {topBar}
    <div className="wp-teach" style={railWidth != null ? { "--wp-rail-w": railWidth + "px" } as CSSProperties : undefined}>
      <SegmentTree seg={seg} tree={tree} currentPos={mode === "params" ? null : pos} camera={camera} paramsCamera={mode === "params" ? camera : null}
        corridors={corridorsOf} cameraItems={cameraItems} locked={groupsLocked ? groupLockReason : ""}
        onSegment={target => focusPos(target)} onCorridor={(target, id) => focusPos(target, id)} onParams={openParams} />
      <div className="wp-stack wp-mid">
        {plan.cameras.length !== 3 && <Notice title="段配方需要 3 台相机" tone="warn">当前计划相机 {plan.cameras.length} 台：三台相机共用一个触发，缺相机的配方不能发布。</Notice>}
        {!teachingPiece && <Notice title="还没有示教件" tone="warn">先选一件历史件设为示教件，再按它的图像分段与示教走廊。<Link className="wp-text-link" to={href("/recipe/capture")}>前往示教件<ArrowRight size={12} aria-hidden="true" /></Link></Notice>}
        {locate && mapped && <Notice title={`从历史件 SN ${locate.sn || "—"} 跳来`}>
          历史件帧组 {locate.seq} 对应示教件帧组 {mapped.seq}，{mapped.byPass ? "按所在笔的笔内进度" : "按整件进度"}估算，只用于定位。调整边界保存后返回历史件，重新复测看结果。
        </Notice>}
        {seg.draft && node && <PassStrip seg={seg} cameras={plan.cameras} node={node} passCount={tree.length} segments={plan.segments} passes={teachingPiece?.passes} passSkip={passSkip}
          seq={seq} teachSeq={mode === "corridor" ? teachSeq : null} camera={camera} onSeq={selectSeq} />}
        {middle}
      </div>
      <div className="wp-rail-wrap"><RailResizer width={railWidth} onChange={setRailWidth} />
        {rail}</div>
    </div>
    <div className="wp-teach-dock" role="group" aria-label="走廊示教操作">
      {focusToggle}
      <span className="wp-dock-mid"><Badge tone={stateTone}>{stateText}</Badge><span className="wp-dock-hint">{dockHint}</span></span>
      <span className="wp-dock-actions">
        {segment != null && <WithReason reason={saveReason}><Button variant="text" data-variant="text" leadingIcon={<Save size={15}/>} disabled={!!saveReason} onClick={saveCorridor}>保存走廊</Button></WithReason>}
        {nextTask
          ? <Button aria-label="下一条走廊" variant={primary === "next" ? "filled" : "tonal"} data-variant={primary === "next" ? "filled" : "tonal"} trailingIcon={<ArrowRight size={15}/>} disabled={groupsLocked || seg.restructured} onClick={() => focusTask(nextTask)}>下一条 {nextLabel}</Button>
          : <Button asChild variant={allDone && !editing ? "filled" : "tonal"}><Link to={href("/recipe/calibration")}>前往试测<ArrowRight size={15} aria-hidden="true"/></Link></Button>}
      </span>
    </div>
  </div>;
}
