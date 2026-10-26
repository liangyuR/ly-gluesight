import { pathLength, resampleTeachingPath, type ImagePoint } from "./teachingPath";
import { errorText } from "../../lib/errors";
import { cloneElement, useEffect, useId, useRef, useState, type ReactElement, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { ArrowRight, Brush, Camera, ChevronLeft, ChevronRight, Eraser, Info, Lock, MousePointer2, Save, SquareDashed, Trash2, TriangleAlert, Undo2, WandSparkles } from "lucide-react";
import { Button, Divider, EmptyState, IconButton, NumberField, Popover, SegmentedControl, Select, Slider, Surface, Tooltip } from "qing-steel";
import { useCycle } from "../cycle/api";
import { productionBusy as isProductionBusy } from "../cycle/phase";
import type { RecipeDoc } from "../cycle/types";
import type { SegmentChecks, WorkspacePlanView } from "../plan/types";
import { segmentKindLabel } from "../plan/vis";
import { workspaceApi } from "./api";
import { useWorkspace, type SnackMessage } from "./context";
import { Badge, GrayViewer, KV, Notice, useGrayImage, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { CaptureSummary, FrozenImage, GrayImage, WorkspaceView } from "./types";
import { cameraParamsError, confirmedSegmentRange, CORNER_CORRIDOR_MM, corridorError, draftKey, parseDraftKey, sameCorridor, savedDraft, withCorridor } from "./teach";
import { cameraParamsTasks, corridorTasks, type CameraParamsTask, type CorridorTask } from "./cameraTasks";
import { frameArrived, readRoundFrame, roundView, type AnyRound, type RoundView } from "./captureRound";
import { CameraParamsSettings, SegmentChecksSettings, type CameraParams } from "./CorridorSettings";
import { MeasureDefaults } from "./DocParams";
import "./corridor.css";

/** 禁用控件不发指针事件：Tooltip 挂在外包的可聚焦 span 上，原因同时作为控件的无障碍描述。 */
function WithReason({ reason, block = false, children }: { reason: string; block?: boolean; children: ReactElement<{ "aria-describedby"?: string }> }) {
  const id = useId();
  if (!reason) return children;
  return <Tooltip content={reason}><span tabIndex={0} className={"wp-tip-anchor" + (block ? " block" : "")}>{cloneElement(children, { "aria-describedby": id })}<span id={id} className="qs-sr-only">{reason}</span></span></Tooltip>;
}

function imagingMismatch(image: FrozenImage | null | undefined, doc: RecipeDoc) {
  const current = image && doc.cameras?.[image.camera];
  if (!image || !current || image.exposureUs == null || image.gainDb == null) return null;
  if (image.exposureUs === current.exposureUs && image.gainDb === current.gainDb) return null;
  return `采集时曝光 ${image.exposureUs} µs / 增益 ${image.gainDb} dB，与配方当前值（${current.exposureUs} µs / ${current.gainDb} dB）不同`;
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

/** 采集轮次里某台相机某个帧组的原图（候选示教帧，还没有冻结进候选配方）。 */
function useRoundFrame(round: AnyRound | null, camera: string, seq: number, enabled: boolean) {
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

export default function TeachingPage() {
  const { data, doc } = useWorkspace();
  if (!data || !doc) return <WorkspaceEmpty />;
  if (!data.plan) return <Notice title="该草稿不是段配方" tone="warn">请新建段配方后配置走廊，旧拍照点草稿不再支持。</Notice>;
  return <CorridorTeaching data={data} doc={doc} plan={data.plan} />;
}

function CorridorTeaching({ data, doc, plan }: { data: WorkspaceView; doc: RecipeDoc; plan: WorkspacePlanView }) {
  const { dirty, busy, frameDirty, corridorDrafts, setCorridorDraft, setDoc, act, setError } = useWorkspace();
  const { snapshot } = useCycle();
  const [search] = useSearchParams();
  const cameraItems = cameraParamsTasks(plan);
  const tasks = corridorTasks(plan);
  const requestedCamera = search.get("camera");
  const requestedSegment = search.get("segment");
  const requestedRound = search.get("round");
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
  const targetLabel = segment == null ? camera : `${camera} · 段 ${segment}`;
  const [working, setWorking] = useState(false);
  const [choosing, setChoosing] = useState(!!requestedRound);
  const [operation, setOperation] = useState<"extract" | null>(null);
  const [extractionHint, setExtractionHint] = useState("");
  const [extractionFailure, setExtractionFailure] = useState<{ point: ImagePoint; reason: string; message: string } | null>(null);
  const [vertex, setVertex] = useState<number | null>(null);
  const [region, setRegion] = useState<[number, number, number, number] | null>(null);
  const [regionMode, setRegionMode] = useState(false);
  const [selectionTool, setSelectionTool] = useState<"brush" | "box">("brush");
  const [brushPoints, setBrushPoints] = useState<ImagePoint[]>([]);
  const [brushWidth, setBrushWidth] = useState(DEFAULT_BRUSH_PX);
  const [brushFailures, setBrushFailures] = useState(0);
  const [pointSpacing, setPointSpacing] = useState(20);
  const pending = useRef(false), serial = useRef(0);
  const scope = JSON.stringify([doc.id, key, frame?.id ?? null]);
  useEffect(() => setBrushFailures(0), [scope]);
  const current = useRef({ scope, alive: true, revision: data.workspace.revision }); current.current.scope = scope; current.current.revision = data.workspace.revision;
  useEffect(() => { current.current.alive = true; return () => { current.current.alive = false; serial.current++; }; }, []);
  useEffect(() => { serial.current++; pending.current = false; setWorking(false); setChoosing(false); setVertex(null); setRegion(null); setBrushPoints([]); setRegionMode(false); setOperation(null); setExtractionHint(""); setExtractionFailure(null); }, [scope]);
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

  // 示教帧选择：从一轮已确认手动分段的完整采集里选这台相机落在本段内的一个帧组
  const [rounds, setRounds] = useState<CaptureSummary[]>([]);
  const [pickRoundId, setPickRoundId] = useState(requestedRound ?? "");
  const [picked, setPicked] = useState<{ round: AnyRound; view: RoundView } | null>(null);
  const [pickSeq, setPickSeq] = useState(1);
  const [roundsError, setRoundsError] = useState("");
  const [pickError, setPickError] = useState("");
  const choosingNow = segment != null && (!frame || choosing);
  useEffect(() => {
    if (!requestedRound) return;
    setPickRoundId(requestedRound); setPickSeq(1); setChoosing(true);
  }, [doc.id, camera, requestedRound]);
  useEffect(() => {
    if (!choosingNow) return;
    let alive = true, pending = false;
    setRounds([]); setRoundsError("");
    const refresh = async () => {
      if (pending) return;
      pending = true;
      try {
        const list = await workspaceApi.captureList(doc.id);
        if (alive) { setRounds(list.filter(item => item.state === "complete")); setRoundsError(""); }
      } catch (e) { if (alive) setRoundsError(errorText(e)); }
      finally { pending = false; }
    };
    void refresh(); const timer = setInterval(() => void refresh(), 700);
    return () => { alive = false; clearInterval(timer); };
  }, [doc.id, choosingNow]);
  const reviews = data.workspace.captureReview?.rounds ?? {};
  useEffect(() => {
    if (pickRoundId || !rounds.length) return;
    const template = data.workspace.captureReview?.template?.roundId;
    const preferred = rounds.find(item => item.roundId === template);
    setPickRoundId(preferred ? preferred.roundId : [...rounds].sort((a, b) => b.createdAt - a.createdAt)[0].roundId);
  }, [rounds, pickRoundId]);
  const selectedRoundId = rounds.find(item => item.roundId === pickRoundId)?.roundId;
  useEffect(() => {
    setPicked(null); setPickError("");
    if (!choosingNow || !selectedRoundId) return;
    let alive = true;
    void workspaceApi.captureGet(selectedRoundId).then(round => { if (alive) setPicked(round?.recipeId === doc.id && round.state === "complete" ? { round, view: roundView(round) } : null); }).catch(e => { if (alive) setPickError(errorText(e)); });
    return () => { alive = false; };
  }, [selectedRoundId, choosingNow, doc.id]);
  const pickGroups = picked?.view.groupCount ?? 0;
  const pickSeqSafe = Math.min(Math.max(1, pickSeq), Math.max(1, pickGroups));
  // 单段配方所有帧都在第 1 段，不需要先确认手动分段（与后端 apply_segment_teaching 一致）
  const range = segment == null || !picked ? null : plan.segments.length === 1 ? { start: 1, end: picked.view.groupCount } : confirmedSegmentRange(reviews[picked.view.roundId], plan.segments, segment);
  useEffect(() => { if (range) setPickSeq(Math.round((range.start + range.end) / 2)); }, [range?.start, range?.end]);
  const inSegment = !!range && pickSeqSafe >= range.start && pickSeqSafe <= range.end;
  const arrived = !!picked && frameArrived(picked.view, camera, pickSeqSafe);
  const candidate = useRoundFrame(picked?.round ?? null, camera, pickSeqSafe, choosingNow && arrived);

  const saved = savedDraft(data.workspace.doc, key);
  const t = segment == null ? saved : corridorDrafts[key] ?? saved;
  const editing = segment != null && !sameCorridor(t, saved);
  const invalid = corridorError(t);
  const unlocked = !busy && !dirty && !working && !data.workspace.pending;
  const productionBusy = isProductionBusy(snapshot);
  const update = (next: typeof t) => { if (unlocked && segment != null) setCorridorDraft(key, next); };
  const setPath = (corridor: [number, number][]) => update({ ...t, corridor });
  const canEdit = unlocked && !!frame && !choosingNow;
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
    if (!canEdit || !frame || pending.current) return;
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
  const changeSegmentChecks = (segment: number, checks: SegmentChecks) => setDoc({ ...doc, segments: (doc.segments ?? []).map(item => item.index === segment ? { ...item, checks } : item) });
  const inspect = doc.inspect?.find(item => item.camera === camera);
  const params: CameraParams = { mmPerPx: inspect?.mmPerPx ?? null, detect: inspect?.detect ?? null };
  const setParams = (next: CameraParams) => setDoc({ ...doc, inspect: (doc.inspect ?? []).map(item => item.camera === camera ? { ...item, ...next } : item) });

  if (!cameraItems.length) return <div className="wp-page"><WorkspaceBar /><Surface><EmptyState icon={<Camera size={32} aria-hidden="true" />} title="段配方还没有计划相机" description="先在“段列表”里确认计划相机，再回来示教走廊。" action={<Link className="qs-btn" to="/recipe/segments">前往段列表<ArrowRight size={15} aria-hidden="true" /></Link>} /></Surface></div>;

  const imagingHint = imagingMismatch(frame, data.workspace.doc);
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
  const groupLockReason = editing || frameDirty ? "走廊有未保存的修改，先保存或清空后再切换" : dirty ? "候选配置正在保存，请稍候" : data.workspace.pending ? "候选版本正在等待生效" : "正在处理或保存，请稍候";
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
    : !frame ? `先在已确认分段的轮次里选第 ${segment} 段内的一帧`
    : data.workspace.pending ? "等待当前工件结束后切换，期间不能修改示教"
      : working ? "正在处理或保存，请稍候"
        : choosing ? "选定新帧后，这一段现有走廊原样保留"
          : editing ? (invalid ? invalid : "有未保存的修改：保存后写入候选配方，已发布版本不变；也会在停顿后自动保存")
            : cornerLong ? `弯道走廊超过 ${CORNER_CORRIDOR_MM} mm：胶条绕出胶点转动，建议只从出胶点起画一小段`
              : !scaled ? `${camera} 缺像素当量：先在相机参数里设置，再保存走廊`
                : done ? doneHint
                  : segmentItem?.kind === "corner" ? `弯道：从出胶点起沿胶条画一小段（不超过 ${CORNER_CORRIDOR_MM} mm）`
                    : "直线段：从出胶点一侧起，沿胶条方向画整段走廊";
  const editHint = "点击图像加点（绿圈为起点），拖动点可调整；密集走廊打开“点柄”显示全部点，Ctrl＋滚轮缩放。";
  const regionHint = extractionHint || (selectionTool === "brush" ? "按住左键沿胶路从起点刷到终点，每笔重画选区；笔刷覆盖胶路和两侧少量背景，尽量避开阴影。" : "按住左键框选一段连续胶路，再自动提取走廊；框选包含阴影时建议改用刷选。");
  const changeMode = (value: string) => {
    if (value === "edit") { setRegionMode(false); return; }
    const tool = value as "brush" | "box";
    if (tool !== selectionTool) { setSelectionTool(tool); setExtractionFailure(null); setExtractionHint(""); setBrushFailures(0); }
    setRegionMode(true);
  };
  const modeLabel = (icon: ReactNode, long: string, short: string) => <><span className="wp-seg-icon">{icon}</span><span className="wp-seg-long">{long}</span><span className="wp-seg-short">{short}</span></>;
  const tools = <div className="wp-canvas-tools" role="group" aria-label="示教工具">
    <SegmentedControl size="sm" label="画布模式" value={regionMode ? selectionTool : "edit"} disabled={!canEdit || (!frame && !regionMode)} options={[
      { value: "edit", label: modeLabel(<MousePointer2 size={14} aria-hidden="true"/>, "编辑点", "编辑"), "aria-label": "编辑点" },
      { value: "brush", label: modeLabel(<Brush size={14} aria-hidden="true"/>, "沿胶路刷选", "刷选"), "aria-label": "沿胶路刷选" },
      { value: "box", label: modeLabel(<SquareDashed size={14} aria-hidden="true"/>, "矩形框选", "框选"), "aria-label": "矩形框选" },
    ]} onValueChange={changeMode}/>
    <Button size="sm" variant={primary === "extract" ? "filled" : "tonal"} data-variant={primary === "extract" ? "filled" : "tonal"} leadingIcon={<WandSparkles size={14}/>} disabled={!canEdit || loading || !!error} onClick={() => void extract()}>{operation === "extract" ? "提取中…" : "自动提取走廊"}</Button>
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
  const recaptureLink = productionBusy
    ? <Link className="wp-text-link wp-locked" to="/recipe/capture" title="工件正在检测，重采需等待工件结束"><Lock size={12} aria-hidden="true"/>重新采集</Link>
    : <Link className="wp-text-link" to="/recipe/capture">重新采集</Link>;
  const imageInfo = frame && <Popover side="bottom" align="end" label="图像采集信息" className="wp-image-info" title={`示教帧 · ${targetLabel}`} trigger={<IconButton size="sm" aria-label="图像采集信息"><Info size={15}/></IconButton>}>
    <KV label="图像">{`${frame.id} · ${frame.size[0]} × ${frame.size[1]} px`}</KV>
    <KV label="取样来源">{frame.source} · {new Date(frame.capturedAt).toLocaleString("zh-CN")}</KV>
    <KV label="曝光">{frame.exposureUs != null ? frame.exposureUs + " μs" : "未记录"}</KV><KV label="增益">{frame.gainDb != null ? frame.gainDb + " dB" : "未记录"}</KV>
    <KV label="示教帧更换">仅通过“重新选帧”</KV>
    {imagingHint && <p className="c-warn" role="note">{imagingHint}；样本仍可用，需要时重新采集</p>}
    <p className="muted hint">走廊保存进候选配方，已发布版本不变。用于生产前仍需完成多轮标定与发布。</p>
  </Popover>;
  const pickOptions = rounds.map(item => ({ value: item.roundId, label: `${new Date(item.createdAt).toLocaleString("zh-CN")} · ${item.receivedCount} 帧组${reviews[item.roundId]?.status === "confirmed" ? " · 已确认分段" : ""}` }));
  const pickReason = !picked ? "先选一轮完整的采集" : !range ? "这一轮还没有确认手动分段：先在分段标定里确认边界，或换一轮"
    : !inSegment ? `第 ${segment} 段在这一轮为帧组 ${range.start}–${range.end}` : !arrived ? "该相机本帧组缺帧，请换一个帧组" : !unlocked ? "正在处理或保存，请稍候" : "";
  const pickPanel = <Surface role="region" className="wp-picker wp-corridor-picker" aria-label="示教帧选择">
    <h2>{targetLabel}<small>示教帧</small></h2>
    <div className="wp-corridor-pick">
      <Select label="采集轮次" value={pickRoundId} disabled={!unlocked || !rounds.length} options={pickOptions.length ? pickOptions : [{ value: "", label: "没有完整的采集轮次" }]} onValueChange={value => { setPickRoundId(value); setPickSeq(1); }}/>
      <div className="wp-corridor-seq" role="group" aria-label="帧组选择">
        <IconButton size="sm" aria-label="上一帧组" disabled={!picked || pickSeqSafe <= 1} onClick={() => setPickSeq(pickSeqSafe - 1)}><ChevronLeft size={15}/></IconButton>
        <NumberField aria-label="帧组序号" min={1} max={Math.max(1, pickGroups)} step={1} value={pickGroups ? pickSeqSafe : ""} disabled={!picked} onValueChange={value => setPickSeq(value === "" ? 1 : Math.trunc(value))}/>
        <IconButton size="sm" aria-label="下一帧组" disabled={!picked || pickSeqSafe >= pickGroups} onClick={() => setPickSeq(pickSeqSafe + 1)}><ChevronRight size={15}/></IconButton>
        <span className="muted">/ {pickGroups || "—"}</span>
      </div>
      {range && <span className="muted">第 {segment} 段在这一轮为帧组 {range.start}–{range.end}</span>}
      {!rounds.length && <span className="muted">还没有完整的采集轮次，请先<Link className="wp-text-link" to="/recipe/capture">前往整圈采集</Link>。</span>}
    </div>
    <div className="wp-picker-side">
      <WithReason reason={pickReason}>
        <Button variant="filled" data-variant="filled" disabled={!!pickReason} onClick={() => void chooseFrame()}>用此帧示教 {targetLabel}</Button>
      </WithReason>
      {frame && <Button variant="text" onClick={() => setChoosing(false)}>取消</Button>}
    </div>
  </Surface>;
  const cameraTasks = tasks.filter(item => item.camera === camera);
  const paramsError = cameraParamsError(params);
  const paramsDisabled = busy || working || !!data.workspace.pending;
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
      {(roundsError || pickError) && choosingNow && <p className="c-warn wp-picker-warn" role="alert">{roundsError || pickError}</p>}
      {!choosingNow && frame && <Surface role="region" className="wp-picker wp-corridor-frame" aria-label="示教帧">
        <h2>{targetLabel}<small>示教帧</small></h2>
        <span className="muted wp-corridor-source">{frame.source} · {frame.id}</span>
        <div className="wp-picker-side">{imageInfo}<Button variant="text" disabled={!unlocked || editing || frameDirty} onClick={() => setChoosing(true)}>重新选帧</Button>{recaptureLink}</div>
      </Surface>}
      {choosingNow
        ? <GrayViewer controlsAt="footer" image={candidate.image} loading={candidate.loading} error={candidate.error || (picked && !arrived ? "该相机本帧组缺帧，请换一个帧组" : "")} label={`${camera} · 帧组 ${pickSeqSafe} · 候选示教帧`}
          emptyTitle="选择示教帧" emptyHint="先选一轮已确认手动分段的完整采集，再选这一段内这台相机的帧组；缺帧的帧组不能选。" summary={<span className="muted">{`选定后冻结为第 ${segment} 段的示教帧，这条走廊只用于这一段的帧。`}</span>}/>
        : <GrayViewer controlsAt="footer" summary={summary} image={image} loading={loading} error={error} label={frame ? `${targetLabel} · 示教帧` : "等待取样"} tools={tools} subtools={canvasParams} emptyTitle="示教帧尚未就绪" emptyHint="选定示教帧后，这里显示这一帧。"
          region={selectionTool === "box" ? region : null}
          onRegion={regionMode && selectionTool === "box" && canEdit ? setRegion : undefined}
          brush={regionMode && selectionTool === "brush" && Number.isFinite(brushWidth) ? { points: brushPoints, radiusPx: brushWidth / 2 } : null} onBrush={regionMode && selectionTool === "brush" && canEdit ? points => { setBrushPoints(points); setExtractionFailure(null); setExtractionHint(""); } : undefined}
          extractionFailure={extractionFailure}
          overlay={{ path: t.corridor, searchHalfPx, selected: vertex }}
          editDisabled={!canEdit}
          onEdit={frame && !regionMode ? { add: p => setPath([...t.corridor, p]), move: (i, p) => setPath(t.corridor.map((q, j) => j === i ? p : q)), select: setVertex } : undefined} />}
    </>;
  const lockedDefaults = editing || frameDirty ? "走廊有未保存的修改，先保存后再改默认参数。" : "";
  const checksDisabled = busy || working || !!data.workspace.pending;
  const rail = segment == null
    ? <div className="wp-stack wp-teach-settings"><MeasureDefaults locked={lockedDefaults} /><CameraParamsSettings key={camera} params={params} defaults={defaults} disabled={paramsDisabled || frameDirty} onChange={setParams} />
      <SegmentChecksSettings key={`checks-${camera}`} segments={doc.segments ?? []} disabled={checksDisabled} onChange={changeSegmentChecks} /></div>
    : <div className="wp-stack wp-teach-settings"><MeasureDefaults locked={lockedDefaults} />
      <Surface role="region" className="wp-seg-corridor-note" aria-label="段走廊说明"><h3>第 {segment} 段 · {segmentItem?.kind === "corner" ? "弯道" : "直线"} · {camera}</h3>
        <p className="muted">{segmentItem?.kind === "corner" ? `弯道里胶条绕出胶点转动，只从出胶点起画一小段（不超过 ${CORNER_CORRIDOR_MM} mm），弯道只判胶宽、断胶、溢胶。` : "直线段胶条方向不变：从出胶点一侧起，沿胶条画整段。"}</p>
        <p className="muted">这一段的帧由 {camera} 用这条走廊测量；像素当量（{scaled ? `${Number(t.mmPerPx!.toPrecision(6))} mm/px` : "未设置"}）与检测参数在相机 {camera} 里改。</p>
        <WithReason reason={editing ? "走廊有未保存的修改" : !unlocked ? "正在处理或保存，请稍候" : !frame && !hasPath ? "这一段还没有示教帧和走廊" : ""}><Button variant="text" disabled={editing || !unlocked || (!frame && !hasPath)} onClick={clearSegment}>清除本段走廊</Button></WithReason>
      </Surface>
      {segmentItem && <SegmentChecksSettings key={`checks-${segment}`} segments={(doc.segments ?? []).filter(item => item.index === segment)} disabled={checksDisabled} onChange={changeSegmentChecks} />}</div>;
  return <div className="wp-page wp-teach-page"><WorkspaceBar />
    <div className="wp-teach">
      <Surface role="region" className="wp-group-panel" aria-label="走廊示教目标"><h2>段走廊 <small>{doneCount} / {tasks.length}</small></h2><nav className="wp-group-list" aria-label="相机与段选择">{cameraItems.flatMap(item => {
        const slot = (itemKey: string, button: ReactElement<{ "aria-describedby"?: string }>) => groupsLocked && itemKey !== key ? <WithReason key={itemKey} block reason={groupLockReason}>{button}</WithReason> : <span key={itemKey} className="wp-group-slot">{button}</span>;
        const cameraButton = <button className={"wp-group-item" + (item.camera === key ? " selected" : "")} aria-label={"选择相机 " + item.camera} aria-pressed={item.camera === key} disabled={groupsLocked && item.camera !== key} title={item.mmPerPx != null ? `${item.camera} · ${Number(item.mmPerPx.toPrecision(6))} mm/px` : "缺像素当量"} onClick={() => setSelected(item.camera)}>
          <i className={"dot " + (item.mmPerPx != null ? "ok" : "warn")} aria-hidden="true"/><strong>{item.camera}</strong><span>{item.label}</span>
        </button>;
        const segmentButtons = tasks.filter(s => s.camera === item.camera).map(s => {
          const itemKey = taskKey(s);
          return slot(itemKey, <button className={"wp-group-item wp-group-sub" + (itemKey === key ? " selected" : "")} aria-label={`选择 ${item.camera} 段 ${s.segment} 走廊`} aria-pressed={itemKey === key} disabled={groupsLocked && itemKey !== key} onClick={() => setSelected(itemKey)}>
            <i className={"dot " + (s.status === "done" ? "ok" : s.frame ? "warn" : "off")} aria-hidden="true"/><strong>段 {s.segment}</strong>{s.kind === "corner" ? "弯道" : "直线"}<span>{s.label}</span>
          </button>);
        });
        return [slot(item.camera, cameraButton), ...segmentButtons];
      })}</nav></Surface>
      <div className="wp-stack wp-mid">
        {plan.cameras.length !== 3 && <Notice title="段配方需要 3 台相机" tone="warn">当前计划相机 {plan.cameras.length} 台：三台相机共用一个触发，缺相机的配方不能发布。</Notice>}
        {middle}
      </div>
      {segment == null || (frame && !choosingNow) ? rail : <div className="wp-teach-settings wp-rail-empty"><MeasureDefaults /><span>选定示教帧后，这里显示<br/>本段走廊说明 · 检测项</span></div>}
    </div>
    <div className="wp-teach-dock" role="group" aria-label="走廊示教操作">
      <span className="wp-dock-mid"><Badge tone={stateTone}>{stateText}</Badge><span className="wp-dock-hint">{dockHint}</span></span>
      <span className="wp-dock-actions">
        {segment != null && <WithReason reason={saveReason}><Button variant={primary === "save" ? "filled" : "text"} data-variant={primary === "save" ? "filled" : "text"} leadingIcon={<Save size={15}/>} disabled={!!saveReason} onClick={saveCorridor}>保存走廊</Button></WithReason>}
        {nextTask
          ? <Button aria-label="下一条走廊" variant={primary === "next" ? "filled" : "tonal"} data-variant={primary === "next" ? "filled" : "tonal"} trailingIcon={<ArrowRight size={15}/>} disabled={groupsLocked} onClick={() => setSelected(taskKey(nextTask))}>下一条 {nextLabel}</Button>
          : <Link className={"qs-btn" + (allDone && !editing ? "" : " tonal")} data-variant={allDone && !editing ? "filled" : "tonal"} to="/recipe/calibration">前往多轮标定<ArrowRight size={15} aria-hidden="true"/></Link>}
      </span>
    </div>
  </div>;
}
