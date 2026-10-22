import { resampleTeachingPath, type ImagePoint } from "./teachingPath";
import { errorText } from "../../lib/errors";
import { cloneElement, Fragment, useEffect, useId, useRef, useState, type ReactElement, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { ArrowRight, Brush, Camera, Crosshair, Eraser, Info, Lock, MousePointer2, Play, Save, SquareDashed, Trash2, TriangleAlert, Undo2, WandSparkles } from "lucide-react";
import { Button, Divider, EmptyState, IconButton, NumberField, Popover, SegmentedControl, Slider, Surface, Tooltip } from "qing-steel";
import { verdictLabel } from "../history";
import { useCycle } from "../cycle/api";
import { pathLength, shotSegment } from "../cycle/vis";
import type { FixtureSpec, RecipeDoc } from "../cycle/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, GrayViewer, KV, Notice, Steps, useGrayImage, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { FixtureCheck, FixtureTrial, FrozenImage, RotationConfirmation, ShotTeach, WorkspaceView } from "./types";
import { FIXTURE_DEFAULTS, FIXTURE_PAD_PX, fixtureError, fixtureOverlay, sameTeach, shotTeach, teachError } from "./teach";
import { movedBox, rawPoseTransform } from "../cycle/pose";
import { FixtureSettings, ROTATION_PROBE_DEG } from "./FixtureSettings";
import { FixtureTrialResult } from "./FixtureTrialResult";
import { TrialResult, trialPoints, trialStationColor } from "./TrialResult";
import { groupShortStatus, teachingGroups } from "./teachingGroups";
import { productionBusy as isProductionBusy } from "../cycle/phase";
import { TeachingSettings } from "./TeachingSettings";
import type { SnackMessage } from "./context";

function FrozenViewOption({ id, metadata, selected, conflict, disabled, onSelect }: { id: string; metadata: FrozenImage; selected: boolean; conflict: boolean; disabled: boolean; onSelect: () => void }) {
  const { image, loading, error } = useGrayImage(id, metadata.id);
  const label = metadata.camera + " · 图 " + metadata.view;
  return <button className={"wp-view-option" + (selected ? " selected" : "") + (conflict ? " conflict" : "")} aria-label={"选择检测图 " + label} aria-pressed={selected} disabled={disabled} onClick={onSelect}>
    <span className="wp-view-thumbnail">{image ? <img src={image.url} alt={label} /> : <span>{loading ? "读取中…" : error ? "图像不可用" : "等待图像"}</span>}</span>
    <span className="wp-view-caption"><strong>{metadata.camera}<span className="wp-view-no"> · 图 {metadata.view}</span></strong><span className="wp-view-choice">{selected ? "当前检测图" : conflict ? "已选 · 需重选" : "选用此图"}</span></span>
  </button>;
}

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
/** 定位框检查结果对应的草稿：中线、当量、检测参数、限值与框都没变才沿用 */
const fixtureCheckKey = (t: ShotTeach) => JSON.stringify([t.path, t.mmPerPx, t.detect ?? null, t.limits ?? null, t.fixture]);
const WIDEN_BRUSH_REASONS = new Set(["discontinuity", "missing_edges"]);

/** 刷选提取找不到连续两侧边缘时的下一步：先加大笔刷重刷，同一组再失败改用笔划生成中线。 */
function extractionAdvice(reason: string, failures: number, brushWidth: number) {
  if (!WIDEN_BRUSH_REASONS.has(reason)) return "";
  return failures >= 2 ? "加大笔刷后仍无法连续提取（常见于胶堆、断胶等局部异常）：请点「笔划直接生成中线」，再手工修正。" : `可加大笔刷宽度（当前 ${brushWidth} px，约为胶宽的 3 倍较稳）后沿胶路重刷，再自动提取。`;
}

/** 记录标定验证后的提示：勾选复用时说明实际复用到几个拍照点。 */
function calibrationNotice(next: WorkspaceView, k: number, view: number, reuse: boolean): SnackMessage {
  const check = next.workspace.frames[k]?.viewStates?.find(s => s.view === view)?.calibrationCheck;
  if (!check?.passed) return { text: "已记录标定验证：误差超出 ±0.1 mm，未通过", tone: "warn" };
  if (!reuse) return "已记录当前图的标定验证";
  const reused = next.workspace.frames.filter((frame, other) => other !== k && frame.viewStates?.some(s => s.view === view && s.calibrationCheck?.verificationId === check.verificationId)).length;
  return reused ? `已记录标定验证，并把当量与验证复用到 ${reused} 个拍照点` : { text: "未复用：没有同相机、同图号、设备与标定一致且未单独设当量的拍照点", tone: "warn" };
}

export default function TeachingPage() {
  const { data, doc, dirty, busy, frameDirty, frameDrafts, setFrameDraft, act, setError, rememberPosition } = useWorkspace();
  const {snapshot} = useCycle();
  const [search] = useSearchParams();
  const requested=Number(search.get("frame") ?? data?.workspace.lastPosition?.k ?? 0);
  const requestedFrame=Number.isSafeInteger(requested)&&requested>=0?requested:0;
  const frameCount=data?.workspace.frames.length??0;
  const [selected, setK] = useState(requestedFrame);
  const k=Math.min(selected,Math.max(0,frameCount-1));
  useEffect(() => { setK(Math.min(requestedFrame,Math.max(0,frameCount-1))); }, [doc?.id, search]);
  useEffect(()=>{setK(previous=>Math.min(previous,Math.max(0,frameCount-1)));},[frameCount]);
  const [working,setWorking]=useState(false);
  const reading=false;
  const [operation,setOperation]=useState<"extract" | "trial" | null>(null);
  const [extractionHint,setExtractionHint]=useState("");
  const [extractionFailure,setExtractionFailure]=useState<{ point: ImagePoint; reason: string; message: string } | null>(null);
  const [vertex,setVertex]=useState<number|null>(null);
  const [region, setRegion] = useState<[number,number,number,number] | null>(null);
  const [regionMode, setRegionMode] = useState(false);
  const [selectionTool, setSelectionTool] = useState<"brush" | "box">("brush");
  const [brushPoints, setBrushPoints] = useState<ImagePoint[]>([]);
  const [brushWidth, setBrushWidth] = useState(DEFAULT_BRUSH_PX);
  const [brushFailures, setBrushFailures] = useState(0);
  const [pointSpacing, setPointSpacing] = useState(20);
  const pending=useRef(false),serial=useRef(0);
  const scope=JSON.stringify([doc?.id,k,data?.workspace.doc.shots[k]?.view,data?.workspace.frames[k]?.image?.id]);
  useEffect(()=>setBrushFailures(0),[scope]);
  const current=useRef({scope,alive:true,revision:data?.workspace.revision});current.current.scope=scope;current.current.revision=data?.workspace.revision;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;serial.current++;};},[]);
  useEffect(()=>{serial.current++;pending.current=false;setWorking(false);setVertex(null);setRegion(null);setBrushPoints([]);setRegionMode(false);setOperation(null);setExtractionHint("");setExtractionFailure(null);},[scope]);
  const perform=async(request:()=>Promise<WorkspaceView>,message:SnackMessage | ((next: WorkspaceView) => SnackMessage))=>{
    if(pending.current||busy||dirty)return null;
    pending.current=true;setWorking(true);
    const action=++serial.current;
    const valid=()=>current.current.alive&&current.current.scope===scope&&action===serial.current;
    const revision=data?.workspace.revision;
    try{const result=await act(request,message);return valid()&&result&&(current.current.revision===revision||current.current.revision===result.workspace.revision)?result:null;}
    catch(e){if(valid()&&current.current.revision===revision)setError(errorText(e));return null;}
    finally{if(valid()){pending.current=false;setWorking(false);}}
  };
  const frame = data?.workspace.frames[k];
  // 中线、像素当量与检测参数存在候选配方的拍照点里；草稿没保存前只在本页
  const shot = data?.workspace.doc.shots[k];
  useEffect(() => {
    if (!doc || !shot || busy) return;
    rememberPosition(k,shot.view);
  }, [doc?.id,k,shot?.view,rememberPosition]);
  const {image,error,loading} = useGrayImage(doc?.id ?? null,frame?.image?.id ?? null);
  const groups = doc && data ? teachingGroups(doc.shots, data.progress) : [];
  const groupIndex = groups.findIndex(group => group.members.includes(k));
  const group = groups[groupIndex];
  const firstImage = group?.members.flatMap(member => data!.workspace.frames[member].views.map(view => ({ k: member, view: view.view, imageId: view.id })))[0];
  const defaultScope = JSON.stringify([doc?.id, group?.poseId, firstImage?.imageId]);
  const defaultSelection = useRef({ scope: "", attempted: false });
  useEffect(() => {
    if (defaultSelection.current.scope !== defaultScope) defaultSelection.current = { scope: defaultScope, attempted: false };
    if (!doc || !data || !group || group.selected.length || !firstImage || busy || dirty || frameDirty || working || data.workspace.pending || defaultSelection.current.attempted) return;
    defaultSelection.current.attempted = true;
    void act(() => workspaceApi.selectGroupImage(doc.id,data.workspace.revision,firstImage.k,firstImage.view)).then(result => {
      if (result && current.current.alive && current.current.scope === scope) setK(firstImage.k);
    });
  }, [defaultScope, doc, data, busy, dirty, frameDirty, working]);
  // 定位特征：画框（拖动中的框先放在本页）、没通过外圈与走廊检查的框（不进草稿）、实时检查、定位试测、喷嘴确认
  const [fixtureDrawing, setFixtureDrawing] = useState(false);
  const [fixtureBox, setFixtureBox] = useState<[number, number, number, number] | null>(null);
  const fixtureBoxRef = useRef<[number, number, number, number] | null>(null);
  const [pendingFixture, setPendingFixture] = useState<{ fixture: FixtureSpec; reason: string } | null>(null);
  const [liveCheck, setLiveCheck] = useState<{ key: string; result: FixtureCheck } | null>(null);
  const [fixtureTrial, setFixtureTrial] = useState<FixtureTrial | null>(null);
  const [fixtureTrialRunning, setFixtureTrialRunning] = useState(false);
  const [reviewed, setReviewed] = useState(false);
  const draftTeach = shot ? frameDrafts[k] ?? shotTeach(shot) : null;
  const liveKey = draftTeach?.fixture ? fixtureCheckKey(draftTeach) : "";
  useEffect(() => { setFixtureDrawing(false); setFixtureBox(null); fixtureBoxRef.current = null; setPendingFixture(null); }, [scope]);
  useEffect(() => {
    if (!liveKey || !doc || !data || !draftTeach?.fixture || liveCheck?.key === liveKey || fixtureError(draftTeach.fixture)) return;
    let alive = true;
    const timer = setTimeout(() => {
      workspaceApi.fixtureCheck(doc.id, data.workspace.revision, k, draftTeach).then(result => { if (alive) setLiveCheck({ key: liveKey, result }); }).catch(() => {});
    }, 250);
    return () => { alive = false; clearTimeout(timer); };
  }, [liveKey, doc?.id, data?.workspace.revision, k]);
  // 定位试测只对应试测时的框：换图、改框后清空
  useEffect(() => { setFixtureTrial(null); }, [scope, liveKey]);
  // 喷嘴确认跟着这次试测：重新试测或保存状态作废后要重新确认
  const reviewKey = JSON.stringify([scope, frame?.trial?.paramsTag ?? null, frame?.trial?.elapsedMs ?? null]);
  useEffect(() => { setReviewed(false); }, [reviewKey]);
  if (!data || !doc) return <WorkspaceEmpty />;
  if (!frame || !shot) return <div className="wp-page"><WorkspaceBar /><Steps /><Surface><EmptyState icon={<Camera size={32} aria-hidden="true" />} title="当前配方还没有拍照点" description="拍照点由整圈采集生成。完成一圈采集并采用本轮图像后，在这里逐组选图并示教中线。" action={<Link className="qs-btn" to="/recipe/capture">前往整圈采集<ArrowRight size={15} aria-hidden="true" /></Link>} /></Surface></div>;
  const saved = shotTeach(shot);
  const imagingHint = imagingMismatch(frame.image, data.workspace.doc);
  const t = frameDrafts[k] ?? saved;
  const editing = !sameTeach(t, saved);
  const invalid = teachError(t);
  const unlocked = !busy && !dirty && !working && !reading && !data.workspace.pending;
  const productionBusy = isProductionBusy(snapshot);
  const update = (next: ShotTeach) => {if(unlocked)setFrameDraft(k,next);};
  const setPath = (path: [number, number][]) => update({ ...t, path });
  const shotProgress = data.progress.shots[k];
  const viewProgress = shotProgress?.views.find(v => v.view === shot.view);
  const enabled = group.choice?.k === k && group.choice.view === shot.view;
  const nextGroup = groups[groupIndex + 1];
  const calibration = viewProgress?.calibration ?? { state: "unverified" as const };
  const viewState = viewProgress?.state ?? "unconfigured";
  const trial = frame.trial;
  const trialFresh = !editing && !viewProgress?.trialStale && trial?.imageId === frame.image?.id && ["trialPassed", "taught", "trialFailed"].includes(viewState);
  const trialCurrent = trialFresh && (viewState === "trialPassed" || viewState === "taught");
  const points = trialFresh ? trialPoints(trial?.measurement) : null;
  const measured = trialFresh && !!trial && trial.measurement != null;
  const own = shotSegment(data.layout, k)?.segment;
  const stations: [number, number][] = own ? Array.from({ length: own.count }, (_, i) => [data.layout.points.x[own.first + i], data.layout.points.y[own.first + i]]) : [];
  const lengthPx = pathLength(t.path);
  const trialDisabledReason = !enabled ? "请先选择本组检测图" : !frame.image ? "请先采用采集图像" : viewState === "recapture" ? "图像或设备已变化，请重新采集" : t.path.length < 2 ? "请先提取或绘制至少两个中线点" : invalid ? invalid : data.workspace.pending ? "候选版本正在等待生效" : !unlocked ? "正在处理或保存，请稍候" : "";
  const canTrial = !trialDisabledReason;
  const strokeValid = brushPoints.length >= 2 && pathLength(brushPoints) >= 8;
  const brushValid = Number.isFinite(brushWidth) && brushWidth >= 6 && brushWidth <= 300 && strokeValid;
  const spacingValid = Number.isFinite(pointSpacing) && pointSpacing >= 4 && pointSpacing <= 80;
  const selectionRegion = selectionTool === "box" ? region : brushPoints.length && Number.isFinite(brushWidth) ? (() => {
    const xs = brushPoints.map(p => p[0]), ys = brushPoints.map(p => p[1]);
    return [Math.min(...xs) - brushWidth / 2, Math.min(...ys) - brushWidth / 2, Math.max(...xs) - Math.min(...xs) + brushWidth, Math.max(...ys) - Math.min(...ys) + brushWidth];
  })() : null;
  const roi: [number, number, number, number] | null = selectionRegion && frame.image ? (() => {
    const x = Math.max(0, Math.floor(selectionRegion[0])), y = Math.max(0, Math.floor(selectionRegion[1]));
    return [x, y, Math.min(frame.image.size[0], Math.ceil(selectionRegion[0] + selectionRegion[2])) - x, Math.min(frame.image.size[1], Math.ceil(selectionRegion[1] + selectionRegion[3])) - y];
  })() : null;
  const roiValid = (selectionTool === "box" || brushValid) && !!roi && roi[2] >= 5 && roi[3] >= 5 && roi[2] * roi[3] <= 4_000_000;
  const extract = async () => {
    if (!unlocked || !enabled || !frame.image || pending.current) return;
    if (!roiValid || !roi || !spacingValid) { setRegionMode(true); setExtractionHint("请沿胶路刷过至少 8 px，并设置有效的笔刷宽度和中线点间距。"); return; }
    const capturedScope = scope;
    setOperation("extract"); setExtractionHint(""); setExtractionFailure(null);
    try {
      let candidate = data;
      if (editing) {
        const saved = await perform(() => workspaceApi.saveDraft(doc.id,data.workspace.revision,k,t), "");
        if (!saved) return;
        candidate = saved;
      }
      let extracted: Awaited<ReturnType<typeof workspaceApi.extractCenterline>> | null = null;
      const result = await perform(async () => {
        extracted = await workspaceApi.extractCenterline(doc.id,candidate.workspace.revision,k,frame.image!.id,roi,selectionTool === "brush" ? { points: brushPoints, radiusPx: brushWidth / 2 } : undefined,pointSpacing);
        return candidate;
      }, "");
      if (result && extracted && current.current.scope === capturedScope) {
        const output = extracted as Awaited<ReturnType<typeof workspaceApi.extractCenterline>>;
        if (output.failure) {
          const failures = selectionTool === "brush" && WIDEN_BRUSH_REASONS.has(output.failure.reason) ? brushFailures + 1 : 0;
          const advice = selectionTool === "brush" ? extractionAdvice(output.failure.reason, failures, brushWidth) : "";
          setBrushFailures(failures);
          setExtractionFailure(output.failure); setExtractionHint(advice ? output.failure.message + " " + advice : output.failure.message); setRegionMode(true); return;
        }
        setBrushFailures(0);
        setFrameDraft(k,{...t,path:output.path}); setRegionMode(false); setVertex(null);
        setExtractionHint(output.message || "中线已提取，请检查后运行试测。");
      }
    } finally { if (current.current.scope === capturedScope) setOperation(null); }
  };
  const fromStroke = () => {
    if (!unlocked || !enabled || !frame.image || !strokeValid || !spacingValid) return;
    setPath(resampleTeachingPath(brushPoints, pointSpacing));
    setRegionMode(false); setVertex(null); setExtractionFailure(null);
    setExtractionHint("已按笔划生成中线草稿，请检查并手工修正，再运行试测；尚未验证胶路边缘。");
  };
  const changeBrushWidth = (value: number) => { setBrushWidth(value); setExtractionFailure(null); setExtractionHint(""); };
  const saveAndTrial = async () => {
    if (!canTrial || !frame.image || pending.current) return;
    const capturedScope = scope;
    setOperation("trial"); setRegionMode(false); setFixtureDrawing(false);
    try {
      const imageId = frame.image.id;
      let revision = data.workspace.revision;
      // 模板来源过期时也先保存参数：这就是“确认框后重新保存示教参数”，把框改绑到当前示教图
      if (editing || staleSource) {
        const params: ShotTeach = { path: t.path, mmPerPx: t.mmPerPx, fixture: t.fixture };
        if (t.detect) params.detect = t.detect;
        if (t.limits) params.limits = t.limits;
        const saved = await perform(() => workspaceApi.saveParams(doc.id,revision,k,params).catch(e => { throw new Error(`保存参数未完成，未进行试测：${errorText(e)}`); }), "");
        if (!saved) return;
        revision = saved.workspace.revision;
      }
      await perform(() => workspaceApi.trial(doc.id,revision,k,imageId).catch(e => { throw new Error(`${editing ? "参数已保存到候选，" : ""}试测未完成：${errorText(e)}`); }), "");
    } finally { if (current.current.scope === capturedScope) setOperation(null); }
  };
  const selectImage = async (targetK: number, view: number) => {
    if (!unlocked || editing || frameDirty || pending.current) return;
    if (group.choice?.k === targetK && group.choice.view === view && doc.shots[targetK].view === view) { setK(targetK); return; }
    const result = await perform(() => workspaceApi.selectGroupImage(doc.id,data.workspace.revision,targetK,view), "已选为本组检测图，其余图仅采集、不检测");
    if (result) setK(targetK);
  };
  const selectGroup = (index: number) => {
    const target = groups[index];
    if (target) setK(target.choice?.k ?? target.members[0]);
  };
  const save = async (next = false) => {
    if (!enabled||!unlocked||!trialCurrent||!frame.image||pending.current) return;
    const advance = () => { if (nextGroup) selectGroup(groupIndex + 1); };
    if(viewState === "taught"){if(next)advance();return;}
    const result = await perform(() => (t.fixture ? workspaceApi.saveTeach(doc.id,data.workspace.revision,k,frame.image!.id,reviewed) : workspaceApi.saveTeach(doc.id,data.workspace.revision,k,frame.image!.id)), "本组示教已保存，发布前仍需整体验证");
    if (result?.workspace.doc.id===doc.id && next && current.current.scope===scope && current.current.alive) advance();
  };

  const defaults = data.workspace.doc.detect;
  const searchHalfPx = t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0 ? (t.detect ?? defaults).searchMm / t.mmPerPx : undefined;
  const hasPath = t.path.length >= 2;
  const taught = viewState === "taught" && !editing;
  const conflict = group.selected.length > 1;
  const [stateText, stateTone]: [string, "ok" | "warn" | "info" | "neutral"] = !enabled ? conflict ? ["需重选图", "warn"] : ["待选图", "neutral"] : data.workspace.pending ? ["候选待生效", "warn"]
    : editing ? ["中线已修改 · 需试测", "warn"] : taught ? ["已保存", "ok"] : trialCurrent ? ["试测通过 · 未保存", "warn"]
    : viewState === "trialFailed" ? ["试测异常", "warn"] : viewState === "recapture" ? ["需重新采集", "warn"] : viewProgress?.trialStale ? ["试测已过期", "warn"] : hasPath ? ["待试测", "info"] : ["待示教", "neutral"];
  const strokeAdvised = selectionTool === "brush" && brushFailures >= 2 && !!extractionFailure;
  const primary = strokeAdvised ? "stroke" : !hasPath ? "extract" : !trialCurrent ? "trial" : "save";
  const scaled = t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0;
  const [lengthValue, lengthUnit] = scaled ? [(lengthPx * t.mmPerPx!).toFixed(1), "mm"] : [lengthPx.toFixed(0), "px"];
  const groupTone = (item: typeof groups[number]) => item.complete ? "ok" : item.choice ? "warn" : "off";
  const groupsLocked = !unlocked || editing || frameDirty;
  const groupLockReason = editing || frameDirty ? "有未试测的修改，先试测或清空后再切组" : data.workspace.pending ? "候选版本正在等待生效" : "正在处理或保存，请稍候";
  const lockReason = data.workspace.pending ? "候选版本正在等待生效" : "正在处理或保存，请稍候";
  const unreviewed = !!t.fixture && !reviewed && viewState !== "taught";
  const reviewReason = "先在定位特征里确认框内没有胶、喷嘴、胶枪";
  const saveDisabled = !unlocked || !enabled || !trialCurrent || viewState === "taught" || unreviewed;
  const saveReason = !saveDisabled ? "" : !enabled ? "请先选择本组检测图" : viewState === "taught" && !editing ? "本组已保存，修改后需重新试测" : !trialCurrent ? "试测通过后可保存" : unreviewed ? reviewReason : lockReason;
  const nextDisabled = !unlocked || !enabled || !trialCurrent || !nextGroup || unreviewed;
  const nextReason = !nextDisabled ? "" : !enabled ? "请先选择本组检测图" : !nextGroup ? "已是最后一组" : !trialCurrent ? "试测通过后可保存" : unreviewed ? reviewReason : lockReason;
  const othersComplete = groups.every((item, index) => index === groupIndex || item.complete);
  const dockHint: ReactNode = !enabled ? conflict ? "本组选了多张图，请重新选择一张作为检测图" : "先选择本组检测图"
    : data.workspace.pending ? "等待当前工件结束后切换，期间不能修改示教"
    : working ? "正在处理或保存，请稍候"
    : viewState === "recapture" ? "图像或设备已变化：可修改中线，重新采集后才能试测"
    : editing && !shot.skip ? "有未试测的修改：先试测或清空后再切组、换图；试测会先保存中线与参数到候选，现有试测不用于保存或发布"
    : taught ? "本组已完成；可直接进入下一组，或修改中线后重新试测"
    : viewState === "trialFailed" && trialFresh && trial?.reason ? trial.reason.split(/[。；]/)[0]
    : trialCurrent ? nextGroup ? `保存进候选配方，已发布版本不变；下一组 ${nextGroup.poseId} ${groupShortStatus(nextGroup, data.progress)}`
      : <>已是最后一组；保存后{othersComplete ? ` ${groups.length} 组全部完成` : ""}，可到 <Link to="/recipe/overview">工件总览</Link> 核对位置</>
    : viewProgress?.hint ?? "";
  const editHint = "点击图像加点（绿圈为起点），拖动点可调整；密集中线打开“点柄”显示全部点，Ctrl＋滚轮缩放。";
  const regionHint = extractionHint || (selectionTool === "brush" ? "按住左键沿胶路从起点刷到终点，每笔重画选区；笔刷覆盖胶路和两侧少量背景，尽量避开阴影。" : "按住左键框选一段连续胶路，再自动提取中线；框选包含阴影时建议改用刷选。");
  const changeMode = (value: string) => {
    if (value === "fixture") { setFixtureDrawing(true); setRegionMode(false); return; }
    setFixtureDrawing(false);
    if (value === "edit") { setRegionMode(false); return; }
    const tool = value as "brush" | "box";
    if (tool !== selectionTool) { setSelectionTool(tool); setExtractionFailure(null); setExtractionHint(""); setBrushFailures(0); }
    setRegionMode(true);
  };
  const modeLabel = (icon: ReactNode, long: string, short: string) => <><span className="wp-seg-icon">{icon}</span><span className="wp-seg-long">{long}</span><span className="wp-seg-short">{short}</span></>;
  // 三态模式只是界面；selectionTool 保留上次选区工具，编辑点模式下提取仍用上次的选区
  const tools = <div className="wp-canvas-tools" role="group" aria-label="示教工具">
    <SegmentedControl size="sm" label="画布模式" value={fixtureDrawing ? "fixture" : regionMode ? selectionTool : "edit"} disabled={!unlocked || (!frame.image && !regionMode)} options={[
      { value: "edit", label: modeLabel(<MousePointer2 size={14} aria-hidden="true"/>, "编辑点", "编辑"), "aria-label": "编辑点" },
      { value: "brush", label: modeLabel(<Brush size={14} aria-hidden="true"/>, "沿胶路刷选", "刷选"), "aria-label": "沿胶路刷选" },
      { value: "box", label: modeLabel(<SquareDashed size={14} aria-hidden="true"/>, "矩形框选", "框选"), "aria-label": "矩形框选" },
      { value: "fixture", label: modeLabel(<Crosshair size={14} aria-hidden="true"/>, "定位框", "定位"), "aria-label": "画定位框" },
    ]} onValueChange={changeMode}/>
    <Button size="sm" variant={primary === "extract" ? "filled" : "tonal"} leadingIcon={<WandSparkles size={14}/>} disabled={!unlocked || !frame.image || loading || !!error} onClick={() => void extract()}>{operation === "extract" ? "提取中…" : "自动提取中线"}</Button>
    <Button size="sm" variant={primary === "stroke" ? "filled" : "tonal"} title={selectionTool !== "brush" ? "仅刷选时可用" : !strokeValid ? "先沿胶路刷过至少 8 px" : "按笔划生成中线草稿"} disabled={!unlocked || !enabled || !frame.image || selectionTool !== "brush" || !strokeValid || !spacingValid} onClick={fromStroke}>笔划直接生成中线</Button>
    <Divider vertical/>
    <IconButton size="sm" aria-label="删除末点" title="删除末点" disabled={!unlocked || !enabled || !t.path.length} onClick={() => { setVertex(null); setPath(t.path.slice(0, -1)); }}><Undo2 size={15}/></IconButton>
    <IconButton size="sm" aria-label="删除选中点" title="删除选中点" disabled={!unlocked || !enabled || vertex === null || vertex >= t.path.length} onClick={() => { const i = vertex; setVertex(null); setPath(t.path.filter((_, j) => j !== i)); }}><Trash2 size={15}/></IconButton>
    <IconButton size="sm" aria-label="清空中线" title="清空中线" disabled={!unlocked || !enabled || !t.path.length} onClick={() => { setVertex(null); setPath([]); }}><Eraser size={15}/></IconButton>
  </div>;
  const params = <div className="wp-canvas-params">
    {selectionTool === "brush" && <span className="wp-inline-num">笔刷宽度<NumberField aria-label="笔刷宽度" unit="px" min={6} max={300} step={2} value={Number.isFinite(brushWidth) ? brushWidth : ""} disabled={!unlocked} onValueChange={value => changeBrushWidth(value === "" ? NaN : value)}/>
      <Slider className="wp-brush-width" label="拖动调整笔刷宽度" min={6} max={300} step={2} value={Number.isFinite(brushWidth) ? Math.min(300, Math.max(6, brushWidth)) : DEFAULT_BRUSH_PX} disabled={!unlocked} formatValue={value => value + " px"} onValueChange={changeBrushWidth}/></span>}
    <span className="wp-inline-num" title={`检测站距 ${doc.spacing} mm；编辑点按目标最大间距调整，转弯处保留点`}>中线点间距<NumberField aria-label="中线点间距" unit="px" min={4} max={80} step={2} value={Number.isFinite(pointSpacing) ? pointSpacing : ""} disabled={!unlocked} onValueChange={value => setPointSpacing(value === "" ? NaN : value)}/></span>
    <Button size="sm" variant="text" disabled={!unlocked || !spacingValid || t.path.length < 2} onClick={() => { setVertex(null); setPath(resampleTeachingPath(t.path, pointSpacing)); }}>应用到当前中线</Button>
    {fixtureDrawing ? <p className="wp-canvas-hint" role="status">按住左键拖出定位框：框一块与胶同面、刚性、不被喷嘴挡住的孔或角点，避开胶检测走廊与高光；松手后检查外圈与走廊。</p>
      : regionMode ? <p className={"wp-canvas-hint" + (extractionFailure ? " warn" : "")} role="status">{extractionFailure && <TriangleAlert size={14} aria-hidden="true"/>}{regionHint}</p>
      : <p className="wp-canvas-hint" role="status">{extractionHint && <><span>{extractionHint}</span> </>}<span>{editHint}</span></p>}
  </div>;
  const staleSource = saved.fixture?.source && frame.image && saved.fixture.source !== frame.image.id
    ? `定位框是在示教图 ${saved.fixture.source} 上保存的，当前示教图是 ${frame.image.id}：确认框位置后运行试测（会把框改绑到当前示教图）` : null;
  const shownFixture = fixtureBox ? null : pendingFixture?.fixture ?? t.fixture;
  // 示教图自定位没通过时画出定位落点：未采用画候选框（故障色），已采用但偏移超出容差画移动后的框。
  // 通过时不画：保存门已保证偏移 ≤ 0.5 px，叠出来与原框重合，还会盖住正在编辑的框
  const selfLocateFailed = trialFresh && !!trial?.fixture?.checks.some(c => c.id === "selfLocate" && !c.passed);
  const landingPose = selfLocateFailed && shownFixture && !pendingFixture ? trial?.fixture?.pose ?? null : null;
  const landingT = landingPose && shownFixture ? rawPoseTransform(shownFixture.rect, landingPose) : null;
  const landing = landingT && landingPose && shownFixture ? landingPose.accepted
    ? { moved: { inner: movedBox(shownFixture.rect, 0, landingT), padded: movedBox(shownFixture.rect, FIXTURE_PAD_PX, landingT) } }
    : { candidate: movedBox(shownFixture.rect, 0, landingT) } : {};
  const fixtureChecks = trialFresh && trial?.fixture ? { checks: trial.fixture.checks, source: "trial" as const }
    : t.fixture && liveCheck?.key === fixtureCheckKey(t) ? { checks: liveCheck.result.checks, source: "live" as const } : null;
  const validation = data.workspace.validation;
  const rotationEvidence = validation?.revision === data.workspace.revision ? validation.fixture?.find(r => r.k === k && r.view === shot.view)?.rotation ?? null : null;
  /** 新框或改了框：先只读检查外圈与走廊，过了才写进草稿，免得草稿自动保存撞上配方校验 */
  const proposeFixture = async (next: FixtureSpec | null) => {
    if (!unlocked || !enabled) return;
    if (!next) { setPendingFixture(null); update({ ...t, fixture: null }); return; }
    const invalidBox = fixtureError(next);
    if (invalidBox) { setPendingFixture({ fixture: next, reason: invalidBox }); return; }
    const committed = t.fixture;
    if (committed && next.rect.every((v, i) => v === committed.rect[i])) { setPendingFixture(null); update({ ...t, fixture: next }); return; }
    const candidate: ShotTeach = { ...t, fixture: next };
    const capturedScope = scope;
    try {
      const result = await workspaceApi.fixtureCheck(doc.id, data.workspace.revision, k, candidate);
      if (current.current.scope !== capturedScope) return;
      const blocked = result.checks.filter(c => (c.id === "inside" || c.id === "corridor") && !c.passed);
      if (blocked.length) { setPendingFixture({ fixture: next, reason: blocked.map(c => c.detail).join("；") }); return; }
      setPendingFixture(null); setLiveCheck({ key: fixtureCheckKey(candidate), result }); setFrameDraft(k, candidate);
    } catch (e) { if (current.current.scope === capturedScope) setError(errorText(e)); }
  };
  const finishFixtureBox = () => {
    const box = fixtureBoxRef.current;
    fixtureBoxRef.current = null; setFixtureBox(null);
    if (!box || box[2] < 1 || box[3] < 1) return;
    // 松手后留在定位框模式：可以直接重画，误点也不会给中线加点；回编辑点由用户自己切
    void proposeFixture({ ...(t.fixture ?? { ...FIXTURE_DEFAULTS, source: "" }), rect: box.map(Math.round) as FixtureSpec["rect"] });
  };
  const runFixtureTrial = async () => {
    if (!unlocked || pending.current || fixtureTrialRunning) return;
    const capturedScope = scope;
    setFixtureTrialRunning(true);
    try { const result = await workspaceApi.fixtureTrial(doc.id, data.workspace.revision, k); if (current.current.scope === capturedScope) setFixtureTrial(result); }
    catch (e) { if (current.current.scope === capturedScope) setError(errorText(e)); }
    finally { if (current.current.scope === capturedScope) setFixtureTrialRunning(false); }
  };
  /** 开转角只走这里：带着独立证据保存示教参数（后端记进应用日志），之后要重新试测、重新验证 */
  const rotate = async (rotation: RotationConfirmation) => {
    if (!t.fixture || editing) return false;
    const params: ShotTeach & { rotation: RotationConfirmation } = { ...t, fixture: { ...t.fixture, angleRangeDeg: ROTATION_PROBE_DEG }, rotation };
    const result = await perform(() => workspaceApi.saveParams(doc.id, data.workspace.revision, k, params), "已开启转角并记进应用日志；请重新试测并重新验证");
    const savedShot = result?.workspace.doc.shots[k];
    if (savedShot && frameDrafts[k]) setFrameDraft(k, shotTeach(savedShot));
    return !!result;
  };
  const summary = <span className="wp-trial-inline">
    {trialFresh && trial ? <span className="wp-trial-result" aria-label="本次算法结果"><Badge tone={trialCurrent ? "ok" : "warn"}>{viewProgress?.trialStale ? "已过期" : trial.passed ? trial.verdict ? verdictLabel[trial.verdict] ?? trial.verdict : "已完成" : "执行异常"}</Badge>
      <span className="muted">量成 <b>{measured ? (trial.coverage * 100).toFixed(1) + "%" : "—"}</b><span className="wp-trial-ms"> · <b>{trial.elapsedMs} ms</b></span></span></span>
      : <Badge tone={viewProgress?.trialStale ? "warn" : "neutral"}>{viewProgress?.trialStale ? "试测已过期" : "未试测"}</Badge>}
    <span className="muted">中线 <b>{t.path.length}</b> 点 · <b>{lengthValue}</b> {lengthUnit}{own && !editing && <> · <b>{own.count}</b> 站</>}</span>
  </span>;
  const imageWidth = frame.image?.size[0] ?? Infinity;
  const labelEvery = points ? Math.max(1, Math.ceil(points.length / 4)) : 1;
  const recaptureLink = productionBusy
    ? <Link className="wp-text-link wp-locked" to="/recipe/capture" title="工件正在检测，重采需等待工件结束"><Lock size={12} aria-hidden="true"/>重新采集</Link>
    : <Link className="wp-text-link" to="/recipe/capture">重新采集</Link>;
  const imageInfo = <Popover side="bottom" align="end" label="图像采集信息" className="wp-image-info" title={`当前检测图 · ${group.poseId} · ${shot.camera} · 图 ${shot.view}`} trigger={<IconButton size="sm" aria-label="图像采集信息"><Info size={15}/></IconButton>}>
    <KV label="图像">{frame.image ? `${frame.image.id} · ${frame.image.size[0]} × ${frame.image.size[1]} px` : "未冻结"}</KV>
    {frame.image && <KV label="取样来源">{frame.image.source} · {new Date(frame.image.capturedAt).toLocaleString("zh-CN")}</KV>}
    <KV label="曝光">{frame.image?.exposureUs != null ? frame.image.exposureUs + " μs" : "未记录"}</KV><KV label="增益">{frame.image?.gainDb != null ? frame.image.gainDb + " dB" : "未记录"}</KV>
    <KV label="原图更新">仅通过整圈重新采集</KV>
    {imagingHint && <p className="c-warn" role="note">{imagingHint}；样本仍可用，需要时整圈重采</p>}
    <p className="muted hint">中线保存进候选配方，已发布版本不变。用于生产前仍需完成标定、代表性样本验证与发布。</p>
  </Popover>;
  const conflictCameras = group.selected.map(item => doc.shots[item.k].camera);
  return <div className="wp-page wp-teach-page"><WorkspaceBar /><Steps />
    <div className="wp-teach">
      <Surface role="region" className="wp-group-panel" aria-label="拍照组"><h2>拍照组 <small>{groups.length}</small></h2><nav className="wp-group-list" aria-label="拍照组选择">{groups.map((item,index) => {
        const button = <button className={"wp-group-item" + (index === groupIndex ? " selected" : "")} aria-label={"选择拍照组 " + item.poseId} aria-pressed={index === groupIndex} disabled={groupsLocked} title={item.choice ? doc.shots[item.choice.k].camera + " · 图 " + item.choice.view : "未选定检测图"} onClick={() => selectGroup(index)}>
          <i className={"dot " + groupTone(item)} aria-hidden="true"/><strong>{item.poseId}</strong><span>{groupShortStatus(item, data.progress)}</span>
        </button>;
        return groupsLocked ? <WithReason key={item.poseId} block reason={groupLockReason}>{button}</WithReason> : <Fragment key={item.poseId}>{button}</Fragment>;
      })}</nav></Surface>
      <div className="wp-stack wp-mid">
        <Surface role="region" className="wp-picker" aria-label="本组检测图选择">
          <h2>{group.poseId}<small>检测图</small></h2>
          <div className="wp-group-images" role="group" aria-label="本组图像">{group.members.flatMap(member => data.workspace.frames[member].views.map(view => <FrozenViewOption key={view.id} id={doc.id} metadata={view} selected={group.choice?.k === member && group.choice.view === view.view} conflict={conflict && group.selected.some(item => item.k === member && item.view === view.view)} disabled={!unlocked || editing || frameDirty} onSelect={() => void selectImage(member,view.view)} />))}</div>
          <div className="wp-picker-side">{enabled && imageInfo}{!(enabled && viewState === "recapture") && recaptureLink}</div>
        </Surface>
        {conflict && <p className="c-warn wp-picker-warn" role="alert">本组原先选择了多张图，请重新选择一张作为检测图。</p>}
        {conflict && <Notice title="本组原先选择了多张图" tone="warn">{conflictCameras.join(" 与 ")} 都被标为检测图（旧版本配方允许多选）。请重新点选一张作为 {group.poseId} 的检测图，其余图自动改为仅采集。</Notice>}
        {!enabled && !conflict && <Notice title="选择本组检测图">点上方一张图作为 {group.poseId} 的检测图开始示教，每组只需完成这一张；其余图仅采集、不检测。</Notice>}
        {enabled && viewState === "recapture" && <Notice title="需重新采集" tone="warn">{viewProgress?.hint} 中线仍可修改，重新采集后才能试测。 <Link className="qs-btn tonal sm" to="/recipe/capture">整圈重新采集</Link></Notice>}
        {enabled && viewState === "trialFailed" && !editing && <Notice title="试测异常" tone="warn">{trial?.reason}</Notice>}
        {enabled && <GrayViewer controlsAt="footer" summary={summary} image={image} loading={loading} error={error} label={frame.image ? group.poseId + " · " + shot.camera + " · 图 " + shot.view : "等待取样"} tools={tools} subtools={params} emptyTitle="本组检测图尚未就绪" emptyHint="整圈采集并采用本轮图像后，这里显示本组检测图。"
          region={fixtureDrawing ? fixtureBox : selectionTool === "box" ? region : null}
          onRegion={fixtureDrawing ? unlocked && enabled ? box => { fixtureBoxRef.current = box; setFixtureBox(box); } : undefined : regionMode && selectionTool === "box" && unlocked && enabled ? setRegion : undefined}
          onRegionDone={fixtureDrawing ? finishFixtureBox : undefined}
          brush={regionMode && selectionTool === "brush" && Number.isFinite(brushWidth) ? { points: brushPoints, radiusPx: brushWidth / 2 } : null} onBrush={regionMode && selectionTool === "brush" && unlocked && enabled ? points => { setBrushPoints(points); setExtractionFailure(null); setExtractionHint(""); } : undefined}
          extractionFailure={extractionFailure}
          overlay={{ path: t.path, stations: points?.filter(p => !p.edge && !p.candidateEdge).map(p => p.px) ?? stations, stationColors: points?.filter(p => !p.edge && !p.candidateEdge).map(p => trialStationColor(p.status)), segments: points?.flatMap((p, i) => {
            const edge = p.edge ?? p.candidateEdge;
            const width = p.edge ? p.width : p.candidateWidth;
            // 宽度标签沿横线（法向）放到胶路外侧；靠近左右边缘的站不出标签，避免被裁掉
            const labeled = i % labelEvery === 0 && width != null && p.px[0] > imageWidth * .08 && p.px[0] < imageWidth * .92;
            return edge ? [{ from: edge[0], to: edge[1], color: p.edge ? "var(--ok)" : "var(--ng)", label: labeled ? width!.toFixed(2) + (p.edge ? " mm" : " mm 候选") : undefined, labelOutward: true }] : [];
          }), searchHalfPx, stale: editing, selected: vertex, fixture: shownFixture ? { ...fixtureOverlay(shownFixture.rect, shownFixture.searchRadiusPx, t.path, !!pendingFixture), ...landing } : null }}
          editDisabled={!unlocked}
          onEdit={enabled && !regionMode && !fixtureDrawing ? { add: p => setPath([...t.path, p]), move: (i, p) => setPath(t.path.map((q, j) => j === i ? p : q)), select: setVertex } : undefined} />}
        {enabled && points && <TrialResult points={points} />}
        {enabled && fixtureTrial && fixtureTrial.revision === data.workspace.revision && <FixtureTrialResult trial={fixtureTrial} />}
      </div>
      {enabled ? <div className="wp-stack wp-teach-settings"><TeachingSettings key={scope} teach={t} savedMmPerPx={saved.mmPerPx} defaults={defaults} defaultLimits={doc.limits} calibration={calibration} disabled={!unlocked} editing={editing} onChange={update}
        onRecord={async (reference, measured, reuse) => !!await perform(() => workspaceApi.checkCalibration(doc.id,data.workspace.revision,k,reference,measured,reuse), next => calibrationNotice(next, k, shot.view, reuse))} />
        <FixtureSettings fixture={pendingFixture?.fixture ?? t.fixture} rejected={pendingFixture?.reason ?? null} checks={fixtureChecks?.checks ?? null} checksSource={fixtureChecks?.source ?? null}
          staleSource={staleSource} evidence={rotationEvidence} disabled={!unlocked} editing={editing} drawing={fixtureDrawing} reviewed={reviewed}
          canTrial={!!saved.fixture && !editing && !staleSource} trialRunning={fixtureTrialRunning}
          onChange={next => void proposeFixture(next)} onDraw={() => { setFixtureDrawing(true); setRegionMode(false); }} onReviewed={setReviewed}
          onTrial={() => void runFixtureTrial()} onRotate={rotate} /></div>
        : <div className="wp-teach-settings wp-rail-empty"><span>选定检测图后，这里显示<br/>标定 · 检测参数 · 判定限值</span></div>}
    </div>
    <div className="wp-teach-dock" role="group" aria-label="单帧示教操作">
      <WithReason reason={trialDisabledReason}><Button aria-label="运行试测" variant={primary === "trial" ? "filled" : "tonal"} leadingIcon={<Play size={15}/>} disabled={!canTrial} title={canTrial ? "保存当前参数，对本图运行一次检测算法" : undefined} onClick={() => void saveAndTrial()}>{operation === "trial" ? "计算中…" : trialFresh ? "重新试测" : "运行试测"}</Button></WithReason>
      <span className="wp-dock-mid"><Badge tone={stateTone}>{stateText}</Badge><span className="wp-dock-hint">{dockHint}</span></span>
      <span className="wp-dock-actions">
        <WithReason reason={saveReason}><Button variant={primary === "save" && !nextGroup ? "filled" : "text"} leadingIcon={<Save size={15}/>} disabled={saveDisabled} onClick={() => void save()}>保存本组示教</Button></WithReason>
        <WithReason reason={nextReason}><Button aria-label="保存并进入下一组" variant={primary === "save" && nextGroup ? "filled" : "tonal"} trailingIcon={<ArrowRight size={15}/>} disabled={nextDisabled} onClick={() => void save(true)}>{nextGroup ? (taught ? "进入下一组 " : "保存并进入下一组 ") + nextGroup.poseId : "保存并进入下一组"}</Button></WithReason>
      </span>
    </div>
  </div>;
}
