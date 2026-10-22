import { pathLength, resampleTeachingPath, type ImagePoint } from "./teachingPath";
import { errorText } from "../../lib/errors";
import { imageFailure, isPendingBackend } from "../../lib/pendingBackend";
import { cloneElement, useEffect, useId, useRef, useState, type ReactElement, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { ArrowRight, Brush, Camera, ChevronLeft, ChevronRight, Eraser, Info, Lock, MousePointer2, Save, SquareDashed, Trash2, TriangleAlert, Undo2, WandSparkles } from "lucide-react";
import { Button, Divider, EmptyState, IconButton, NumberField, Popover, SegmentedControl, Select, Slider, Surface, Tooltip } from "qing-steel";
import { useCycle } from "../cycle/api";
import { productionBusy as isProductionBusy } from "../cycle/phase";
import type { RecipeDoc } from "../cycle/types";
import type { SegmentChecks, WorkspacePlanView } from "../plan/types";
import { workspaceApi } from "./api";
import { useWorkspace, type SnackMessage } from "./context";
import { awaitingPlan, Badge, GrayViewer, KV, Notice, PlanPending, useGrayImage, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { CaptureSummary, FrozenImage, GrayImage, WorkspaceView } from "./types";
import { corridorDraft, corridorError, sameCorridor, withCorridor } from "./teach";
import { cameraTeachingTasks } from "./cameraTasks";
import { frameArrived, readRoundFrame, roundView, type AnyRound, type RoundView } from "./captureRound";
import { CorridorSettings } from "./CorridorSettings";
import LegacyTeachingPage from "./LegacyTeachingPage";
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

/** 采集轮次里某台相机某个帧组的原图（候选示教帧，还没有冻结进候选配方）。 */
function useRoundFrame(round: AnyRound | null, camera: string, seq: number, enabled: boolean) {
  const key = JSON.stringify([round?.roundId ?? null, camera, seq, enabled]);
  const [state, setState] = useState<{ key: string; image: GrayImage | null; error: string; pending: string; loading: boolean }>({ key, image: null, error: "", pending: "", loading: false });
  useEffect(() => {
    if (!enabled || !round) { setState({ key, image: null, error: "", pending: "", loading: false }); return; }
    let alive = true;
    setState({ key, image: null, error: "", pending: "", loading: true });
    readRoundFrame(round, camera, seq)
      .then(image => alive && setState({ key, image, error: "", pending: "", loading: false }))
      .catch(e => alive && setState({ key, image: null, ...imageFailure(e), loading: false }));
    return () => { alive = false; };
  }, [key]);
  return state.key === key ? state : { image: null, error: "", pending: "", loading: enabled && !!round };
}

/**
 * 走廊示教（段配方，路由 /recipe/teach）：三台相机各示教一条走廊，所有帧共用；每台相机从一轮采集里选一帧，
 * 在这一帧上刷选 / 框选提取走廊（边缘提取能力保留，不再与示教中线逐点比对），设像素当量与检测参数。
 * 旧拍照点配方（segments 为空）仍走 LegacyTeachingPage：按 Pose 分组选图、逐点试测、特征定位与标定核对，行为不变。
 * 段配方但后端还没输出 plan（D3 之前）显示待后端占位，不落到旧页面。
 */
export default function TeachingPage() {
  const { data, doc } = useWorkspace();
  if (!data || !doc) return <WorkspaceEmpty />;
  if (awaitingPlan(data, doc)) return <PlanPending title="走廊示教" />;
  if (!data.plan) return <LegacyTeachingPage />;
  return <CorridorTeaching data={data} doc={doc} plan={data.plan} />;
}

function CorridorTeaching({ data, doc, plan }: { data: WorkspaceView; doc: RecipeDoc; plan: WorkspacePlanView }) {
  const { dirty, busy, frameDirty, corridorDrafts, setCorridorDraft, setDoc, act, setError } = useWorkspace();
  const { snapshot } = useCycle();
  const [search] = useSearchParams();
  const tasks = cameraTeachingTasks(plan);
  const requestedCamera = search.get("camera");
  const [selected, setSelected] = useState(requestedCamera ?? (tasks.find(item => item.status !== "done") ?? tasks[0])?.camera ?? "");
  useEffect(() => { if (requestedCamera) setSelected(requestedCamera); }, [doc.id, requestedCamera]);
  const index = Math.max(0, tasks.findIndex(item => item.camera === selected));
  const task = tasks[index];
  const camera = task?.camera ?? "";
  const frame = task?.frame;
  const [working, setWorking] = useState(false);
  const [choosing, setChoosing] = useState(false);
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
  const scope = JSON.stringify([doc.id, camera, frame?.id ?? null]);
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

  // 示教帧选择：从一轮完整采集里选这台相机的一个帧组
  const [rounds, setRounds] = useState<CaptureSummary[]>([]);
  const [pickRoundId, setPickRoundId] = useState(search.get("round") ?? "");
  const [picked, setPicked] = useState<{ round: AnyRound; view: RoundView } | null>(null);
  const [pickSeq, setPickSeq] = useState(1);
  const [pickError, setPickError] = useState("");
  const choosingNow = !!task && (!frame || choosing);
  useEffect(() => {
    if (!choosingNow) return;
    let alive = true;
    void workspaceApi.captureList(doc.id).then(list => { if (alive) setRounds(list.filter(item => item.state === "complete")); }).catch(e => { if (alive) setPickError(errorText(e)); });
    return () => { alive = false; };
  }, [doc.id, choosingNow]);
  useEffect(() => {
    if (!pickRoundId && rounds.length) setPickRoundId([...rounds].sort((a, b) => b.createdAt - a.createdAt)[0].roundId);
  }, [rounds, pickRoundId]);
  useEffect(() => {
    if (!choosingNow || !pickRoundId) { setPicked(null); return; }
    let alive = true;
    setPickError("");
    void workspaceApi.captureGet(pickRoundId).then(round => { if (alive) setPicked(round ? { round, view: roundView(round) } : null); }).catch(e => { if (alive) { setPicked(null); setPickError(errorText(e)); } });
    return () => { alive = false; };
  }, [pickRoundId, choosingNow, doc.id]);
  const pickGroups = picked?.view.groupCount ?? 0;
  const pickSeqSafe = Math.min(Math.max(1, pickSeq), Math.max(1, pickGroups));
  const arrived = !!picked && frameArrived(picked.view, camera, pickSeqSafe);
  const candidate = useRoundFrame(picked?.round ?? null, camera, pickSeqSafe, choosingNow && arrived);

  const saved = corridorDraft(data.workspace.doc.inspect?.find(item => item.camera === camera));
  const t = corridorDrafts[camera] ?? saved;
  const editing = !sameCorridor(t, saved);
  const invalid = corridorError(t);
  const unlocked = !busy && !dirty && !working && !data.workspace.pending;
  const productionBusy = isProductionBusy(snapshot);
  const update = (next: typeof t) => { if (unlocked && camera) setCorridorDraft(camera, next); };
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

  /** 把草稿并入候选配方的这台相机并整份保存（段配方的走廊没有单独的草稿命令）。 */
  const commit = (message: SnackMessage | ((next: WorkspaceView) => SnackMessage)) => perform(() => workspaceApi.saveDoc(doc.id, data.workspace.revision, withCorridor(data.workspace.doc, camera, t)), message);
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
      let pendingText = "";
      // 段配方没有拍照点：按相机 id 在这台相机冻结的示教帧上提取（workspace_extract_corridor，D2），不复用旧命令的 k
      const result = await perform(async () => {
        try { extracted = await workspaceApi.extractCorridor(doc.id, candidateView.workspace.revision, camera, frame.id, roi, selectionTool === "brush" ? { points: brushPoints, radiusPx: brushWidth / 2 } : undefined, pointSpacing); }
        catch (e) { if (!isPendingBackend(e)) throw e; pendingText = e.message; }
        return candidateView;
      }, "");
      if (result && pendingText && current.current.scope === capturedScope) {
        setExtractionHint(`走廊自动提取：${pendingText}。可先用“笔划直接生成走廊”，再手工修正。`); setRegionMode(true); return;
      }
      if (result && extracted && current.current.scope === capturedScope) {
        const output = extracted as Awaited<ReturnType<typeof workspaceApi.extractCorridor>>;
        if (output.failure) {
          const failures = selectionTool === "brush" && WIDEN_BRUSH_REASONS.has(output.failure.reason) ? brushFailures + 1 : 0;
          const advice = selectionTool === "brush" ? extractionAdvice(output.failure.reason, failures, brushWidth) : "";
          setBrushFailures(failures);
          setExtractionFailure(output.failure); setExtractionHint(advice ? output.failure.message + " " + advice : output.failure.message); setRegionMode(true); return;
        }
        setBrushFailures(0);
        setCorridorDraft(camera, { ...t, corridor: output.path }); setRegionMode(false); setVertex(null);
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
    if (!unlocked || !picked || !arrived || pending.current) return;
    const result = await perform(() => workspaceApi.teachCamera(doc.id, data.workspace.revision, camera, picked.view.roundId, pickSeqSafe, t), `${camera} 的示教帧已选定，请在图上示教走廊`);
    if (result) setChoosing(false);
  };
  const saveCorridor = () => { if (unlocked && editing && !invalid) void commit(`${camera} 的走廊已保存到候选配方`); };
  const changeSegmentChecks = (segment: number, checks: SegmentChecks) => setDoc({ ...doc, segments: (doc.segments ?? []).map(item => item.index === segment ? { ...item, checks } : item) });

  if (!task) return <div className="wp-page"><WorkspaceBar /><Surface><EmptyState icon={<Camera size={32} aria-hidden="true" />} title="段配方还没有计划相机" description="先在“段列表”里确认计划相机，再回来示教走廊。" action={<Link className="qs-btn" to="/recipe/segments">前往段列表<ArrowRight size={15} aria-hidden="true" /></Link>} /></Surface></div>;

  const imagingHint = imagingMismatch(frame, data.workspace.doc);
  const nextTask = tasks[index + 1];
  const allDone = tasks.every(item => item.status === "done");
  const doneCount = tasks.filter(item => item.status === "done").length;
  const defaults = data.workspace.doc.detect;
  const searchHalfPx = t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0 ? (t.detect ?? defaults).searchMm / t.mmPerPx : undefined;
  const hasPath = t.corridor.length >= 2;
  const scaled = t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0;
  const [lengthValue, lengthUnit] = scaled ? [(lengthPx * t.mmPerPx!).toFixed(1), "mm"] : [lengthPx.toFixed(0), "px"];
  const groupsLocked = !unlocked || editing || frameDirty;
  const groupLockReason = editing || frameDirty ? "走廊有未保存的修改，先保存或清空后再切换相机" : data.workspace.pending ? "候选版本正在等待生效" : "正在处理或保存，请稍候";
  const [stateText, stateTone]: [string, "ok" | "warn" | "info" | "neutral"] = !frame ? ["待选帧", "neutral"] : data.workspace.pending ? ["候选待生效", "warn"]
    : choosing ? ["重新选帧中", "info"] : editing ? [invalid ? "走廊已修改 · 未完成" : "走廊已修改 · 待保存", "warn"] : task.status === "done" ? ["已示教", "ok"] : task.status === "scale" ? ["缺像素当量", "warn"] : hasPath ? ["已保存 · 待检查", "info"] : ["待示教走廊", "neutral"];
  // 刷选连续两次找不到连续边缘后，笔划直接生成走廊是唯一的主操作
  const strokeAdvised = selectionTool === "brush" && brushFailures >= 2 && !!extractionFailure;
  const primary = strokeAdvised ? "stroke" : !hasPath ? "extract" : editing ? "save" : "next";
  const saveReason = !editing ? (task.status === "done" ? "走廊已保存" : "先提取或绘制走廊") : invalid ? invalid : !unlocked ? (data.workspace.pending ? "候选版本正在等待生效" : "正在处理或保存，请稍候") : "";
  const dockHint: ReactNode = !frame ? "先为这台相机选一个示教帧"
    : data.workspace.pending ? "等待当前工件结束后切换，期间不能修改示教"
      : working ? "正在处理或保存，请稍候"
        : choosing ? "选定新帧后，现有走廊与参数原样保留"
          : editing ? (invalid ? invalid : "有未保存的修改：保存后写入候选配方，已发布版本不变；也会在停顿后自动保存")
            : task.status === "scale" ? "走廊已保存，还缺像素当量"
              : task.status === "done" ? (nextTask ? `本相机已完成；下一台 ${nextTask.camera}（${nextTask.label}）` : allDone ? <>三台相机都已示教，可到 <Link to="/recipe/calibration">多轮标定</Link></> : "本相机已完成")
                : "沿胶路刷过一段，自动提取走廊；走廊按直线段几何，所有帧共用";
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
  const params = <div className="wp-canvas-params">
    {selectionTool === "brush" && <span className="wp-inline-num">笔刷宽度<NumberField aria-label="笔刷宽度" unit="px" min={6} max={300} step={2} value={Number.isFinite(brushWidth) ? brushWidth : ""} disabled={!canEdit} onValueChange={value => changeBrushWidth(value === "" ? NaN : value)}/>
      <Slider className="wp-brush-width" label="拖动调整笔刷宽度" min={6} max={300} step={2} value={Number.isFinite(brushWidth) ? Math.min(300, Math.max(6, brushWidth)) : DEFAULT_BRUSH_PX} disabled={!canEdit} formatValue={value => value + " px"} onValueChange={changeBrushWidth}/></span>}
    <span className="wp-inline-num" title="编辑点按目标最大间距调整，转弯处保留点">走廊点间距<NumberField aria-label="走廊点间距" unit="px" min={4} max={80} step={2} value={Number.isFinite(pointSpacing) ? pointSpacing : ""} disabled={!canEdit} onValueChange={value => setPointSpacing(value === "" ? NaN : value)}/></span>
    <Button size="sm" variant="text" disabled={!canEdit || !spacingValid || t.corridor.length < 2} onClick={() => { setVertex(null); setPath(resampleTeachingPath(t.corridor, pointSpacing)); }}>应用到当前走廊</Button>
    {regionMode ? <p className={"wp-canvas-hint" + (extractionFailure ? " warn" : "")} role="status">{extractionFailure && <TriangleAlert size={14} aria-hidden="true"/>}{regionHint}</p>
      : <p className="wp-canvas-hint" role="status">{extractionHint && <><span>{extractionHint}</span> </>}<span>{editHint}</span></p>}
  </div>;
  const summary = <span className="wp-trial-inline">
    <Badge tone={task.status === "done" && !editing ? "ok" : "neutral"}>{editing ? "走廊未保存" : task.label}</Badge>
    <span className="muted">走廊 <b>{t.corridor.length}</b> 点 · <b>{lengthValue}</b> {lengthUnit}</span>
  </span>;
  const recaptureLink = productionBusy
    ? <Link className="wp-text-link wp-locked" to="/recipe/capture" title="工件正在检测，重采需等待工件结束"><Lock size={12} aria-hidden="true"/>重新采集</Link>
    : <Link className="wp-text-link" to="/recipe/capture">重新采集</Link>;
  const imageInfo = frame && <Popover side="bottom" align="end" label="图像采集信息" className="wp-image-info" title={`示教帧 · ${camera}`} trigger={<IconButton size="sm" aria-label="图像采集信息"><Info size={15}/></IconButton>}>
    <KV label="图像">{`${frame.id} · ${frame.size[0]} × ${frame.size[1]} px`}</KV>
    <KV label="取样来源">{frame.source} · {new Date(frame.capturedAt).toLocaleString("zh-CN")}</KV>
    <KV label="曝光">{frame.exposureUs != null ? frame.exposureUs + " μs" : "未记录"}</KV><KV label="增益">{frame.gainDb != null ? frame.gainDb + " dB" : "未记录"}</KV>
    <KV label="示教帧更换">仅通过“重新选帧”</KV>
    {imagingHint && <p className="c-warn" role="note">{imagingHint}；样本仍可用，需要时重新采集</p>}
    <p className="muted hint">走廊保存进候选配方，已发布版本不变。用于生产前仍需完成多轮标定、代表性样本验证与发布。</p>
  </Popover>;
  const pickOptions = rounds.map(item => ({ value: item.roundId, label: `${new Date(item.createdAt).toLocaleString("zh-CN")} · ${item.receivedCount} 帧组` }));
  const pickPanel = <Surface role="region" className="wp-picker wp-corridor-picker" aria-label="示教帧选择">
    <h2>{camera}<small>示教帧</small></h2>
    <div className="wp-corridor-pick">
      <Select label="采集轮次" value={pickRoundId} disabled={!unlocked || !rounds.length} options={pickOptions.length ? pickOptions : [{ value: "", label: "没有完整的采集轮次" }]} onValueChange={value => { setPickRoundId(value); setPickSeq(1); }}/>
      <div className="wp-corridor-seq" role="group" aria-label="帧组选择">
        <IconButton size="sm" aria-label="上一帧组" disabled={!picked || pickSeqSafe <= 1} onClick={() => setPickSeq(pickSeqSafe - 1)}><ChevronLeft size={15}/></IconButton>
        <NumberField aria-label="帧组序号" min={1} max={Math.max(1, pickGroups)} step={1} value={pickGroups ? pickSeqSafe : ""} disabled={!picked} onValueChange={value => setPickSeq(value === "" ? 1 : Math.trunc(value))}/>
        <IconButton size="sm" aria-label="下一帧组" disabled={!picked || pickSeqSafe >= pickGroups} onClick={() => setPickSeq(pickSeqSafe + 1)}><ChevronRight size={15}/></IconButton>
        <span className="muted">/ {pickGroups || "—"}</span>
      </div>
      {!rounds.length && <span className="muted">还没有完整的采集轮次，请先<Link className="wp-text-link" to="/recipe/capture">前往整圈采集</Link>。</span>}
    </div>
    <div className="wp-picker-side">
      <WithReason reason={!picked ? "先选一轮完整的采集" : !arrived ? "该相机本帧组缺帧，请换一个帧组" : !unlocked ? "正在处理或保存，请稍候" : ""}>
        <Button variant="filled" data-variant="filled" disabled={!unlocked || !picked || !arrived} onClick={() => void chooseFrame()}>用此帧示教 {camera}</Button>
      </WithReason>
      {frame && <Button variant="text" onClick={() => setChoosing(false)}>取消</Button>}
    </div>
  </Surface>;
  return <div className="wp-page wp-teach-page"><WorkspaceBar />
    <div className="wp-teach">
      <Surface role="region" className="wp-group-panel" aria-label="相机走廊"><h2>相机 <small>{doneCount} / {tasks.length}</small></h2><nav className="wp-group-list" aria-label="相机选择">{tasks.map(item => {
        const button = <button className={"wp-group-item" + (item.camera === camera ? " selected" : "")} aria-label={"选择相机 " + item.camera} aria-pressed={item.camera === camera} disabled={groupsLocked && item.camera !== camera} title={item.frame ? `${item.camera} · ${item.frame.id}` : "尚未选示教帧"} onClick={() => setSelected(item.camera)}>
          <i className={"dot " + (item.status === "done" ? "ok" : item.frame ? "warn" : "off")} aria-hidden="true"/><strong>{item.camera}</strong><span>{item.label}</span>
        </button>;
        return groupsLocked && item.camera !== camera ? <WithReason key={item.camera} block reason={groupLockReason}>{button}</WithReason> : <span key={item.camera} className="wp-group-slot">{button}</span>;
      })}</nav></Surface>
      <div className="wp-stack wp-mid">
        {plan.cameras.length !== 3 && <Notice title="段配方需要 3 台相机" tone="warn">当前计划相机 {plan.cameras.length} 台：三台相机共用一个触发，缺相机的配方不能发布。</Notice>}
        {choosingNow && pickPanel}
        {pickError && choosingNow && <p className="c-warn wp-picker-warn" role="alert">{pickError}</p>}
        {!choosingNow && frame && <Surface role="region" className="wp-picker wp-corridor-frame" aria-label="示教帧">
          <h2>{camera}<small>示教帧</small></h2>
          <span className="muted wp-corridor-source">{frame.source} · {frame.id}</span>
          <div className="wp-picker-side">{imageInfo}<Button variant="text" disabled={!unlocked || editing || frameDirty} onClick={() => setChoosing(true)}>重新选帧</Button>{recaptureLink}</div>
        </Surface>}
        {choosingNow
          ? <GrayViewer controlsAt="footer" image={candidate.image} loading={candidate.loading} error={candidate.error || (picked && !arrived ? "该相机本帧组缺帧，请换一个帧组" : "")} label={`${camera} · 帧组 ${pickSeqSafe} · 候选示教帧`}
            emptyTitle={candidate.pending ? "原图读取待后端实现" : "选择示教帧"} emptyHint={candidate.pending || "先选一轮完整的采集，再选这台相机要用的帧组；缺帧的帧组不能选。"} summary={<span className="muted">选定后冻结为这台相机的示教帧，所有帧共用这一条走廊。</span>}/>
          : <GrayViewer controlsAt="footer" summary={summary} image={image} loading={loading} error={error} label={frame ? `${camera} · 示教帧` : "等待取样"} tools={tools} subtools={params} emptyTitle="示教帧尚未就绪" emptyHint="选定示教帧后，这里显示这一帧。"
            region={selectionTool === "box" ? region : null}
            onRegion={regionMode && selectionTool === "box" && canEdit ? setRegion : undefined}
            brush={regionMode && selectionTool === "brush" && Number.isFinite(brushWidth) ? { points: brushPoints, radiusPx: brushWidth / 2 } : null} onBrush={regionMode && selectionTool === "brush" && canEdit ? points => { setBrushPoints(points); setExtractionFailure(null); setExtractionHint(""); } : undefined}
            extractionFailure={extractionFailure}
            overlay={{ path: t.corridor, searchHalfPx, selected: vertex }}
            editDisabled={!canEdit}
            onEdit={frame && !regionMode ? { add: p => setPath([...t.corridor, p]), move: (i, p) => setPath(t.corridor.map((q, j) => j === i ? p : q)), select: setVertex } : undefined} />}
      </div>
      {frame && !choosingNow ? <div className="wp-stack wp-teach-settings"><CorridorSettings key={scope} teach={t} defaults={defaults} segments={doc.segments ?? []} disabled={!canEdit} checksDisabled={busy || working || !!data.workspace.pending} onChange={update} onSegmentChecks={changeSegmentChecks} /></div>
        : <div className="wp-teach-settings wp-rail-empty"><span>选定示教帧后，这里显示<br/>像素当量 · 检测参数 · 检测项</span></div>}
    </div>
    <div className="wp-teach-dock" role="group" aria-label="走廊示教操作">
      <span className="wp-dock-mid"><Badge tone={stateTone}>{stateText}</Badge><span className="wp-dock-hint">{dockHint}</span></span>
      <span className="wp-dock-actions">
        <WithReason reason={saveReason}><Button variant={primary === "save" ? "filled" : "text"} data-variant={primary === "save" ? "filled" : "text"} leadingIcon={<Save size={15}/>} disabled={!!saveReason} onClick={saveCorridor}>保存走廊</Button></WithReason>
        {nextTask
          ? <Button aria-label="下一台相机" variant={primary === "next" ? "filled" : "tonal"} data-variant={primary === "next" ? "filled" : "tonal"} trailingIcon={<ArrowRight size={15}/>} disabled={groupsLocked} onClick={() => setSelected(nextTask.camera)}>下一台 {nextTask.camera}</Button>
          : <Link className={"qs-btn" + (allDone && !editing ? "" : " tonal")} data-variant={allDone && !editing ? "filled" : "tonal"} to="/recipe/calibration">前往多轮标定<ArrowRight size={15} aria-hidden="true"/></Link>}
      </span>
    </div>
  </div>;
}
