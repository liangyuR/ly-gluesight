import { resampleTeachingPath, type ImagePoint } from "./teachingPath";
import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Eraser, Play, Save, Trash2, Undo2, WandSparkles } from "lucide-react";
import { useCycle } from "../cycle/api";
import { pathLength, shotSegment } from "../cycle/vis";
import type { RecipeDoc } from "../cycle/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, GrayViewer, KV, Notice, NumberField, Panel, Steps, useGrayImage, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { FrozenImage, ShotTeach, WorkspaceView } from "./types";
import { sameTeach, shotTeach, teachError } from "./teach";
import { TrialResult, trialPoints } from "./TrialResult";
import { teachingGroups } from "./teachingGroups";
import { productionBusy as isProductionBusy } from "../cycle/phase";
import { TeachingSettings } from "./TeachingSettings";

function FrozenViewOption({ id, metadata, selected, disabled, onSelect }: { id: string; metadata: FrozenImage; selected: boolean; disabled: boolean; onSelect: () => void }) {
  const { image, loading, error } = useGrayImage(id, metadata.id);
  const label = metadata.camera + " · 图 " + metadata.view;
  return <button className={"wp-view-option" + (selected ? " selected" : "")} aria-label={"选择检测图 " + label} aria-pressed={selected} disabled={disabled} onClick={onSelect}>
    <span className="wp-view-thumbnail">{image ? <img src={image.url} alt={label} /> : <span>{loading ? "读取中…" : error ? "图像不可用" : "等待图像"}</span>}</span>
    <span className="wp-view-caption"><strong>{label}</strong><span className="wp-view-choice">{selected ? "当前检测图" : "选用此图"}</span></span>
  </button>;
}

function imagingMismatch(image: FrozenImage | null | undefined, doc: RecipeDoc) {
  const current = image && doc.cameras?.[image.camera];
  if (!image || !current || image.exposureUs == null || image.gainDb == null) return null;
  if (image.exposureUs === current.exposureUs && image.gainDb === current.gainDb) return null;
  return `采集时曝光 ${image.exposureUs} µs / 增益 ${image.gainDb} dB，与配方当前值（${current.exposureUs} µs / ${current.gainDb} dB）不同`;
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
  const [brushWidth, setBrushWidth] = useState(80);
  const [pointSpacing, setPointSpacing] = useState(20);
  const pending=useRef(false),serial=useRef(0);
  const scope=JSON.stringify([doc?.id,k,data?.workspace.doc.shots[k]?.view,data?.workspace.frames[k]?.image?.id]);
  const current=useRef({scope,alive:true,revision:data?.workspace.revision});current.current.scope=scope;current.current.revision=data?.workspace.revision;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;serial.current++;};},[]);
  useEffect(()=>{serial.current++;pending.current=false;setWorking(false);setVertex(null);setRegion(null);setBrushPoints([]);setRegionMode(false);setOperation(null);setExtractionHint("");setExtractionFailure(null);},[scope]);
  const perform=async(request:()=>Promise<WorkspaceView>,message:string)=>{
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
  if (!data || !doc) return <WorkspaceEmpty />;
  if (!frame || !shot) return <div className="wp-page"><WorkspaceBar /><Steps /><Notice title="当前配方没有拍照点" tone="warn">请先完成整圈采集并采用本轮图像。</Notice><Link className="btn" to="/recipe/capture">整圈采集</Link></div>;
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
  const completedGroups = groups.filter(group => group.complete).length;
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
          setExtractionFailure(output.failure); setExtractionHint(output.failure.message); setRegionMode(true); return;
        }
        setFrameDraft(k,{...t,path:output.path}); setRegionMode(false); setVertex(null);
        setExtractionHint(output.message || "中线已提取，请检查后运行一次算法。");
      }
    } finally { if (current.current.scope === capturedScope) setOperation(null); }
  };
  const fromStroke = () => {
    if (!unlocked || !enabled || !frame.image || !strokeValid || !spacingValid) return;
    setPath(resampleTeachingPath(brushPoints, pointSpacing));
    setRegionMode(false); setVertex(null); setExtractionFailure(null);
    setExtractionHint("已按笔划生成中线草稿，请检查并手工修正，再运行一次算法；尚未验证胶路边缘。");
  };
  const changeBrushWidth = (value: number) => { setBrushWidth(value); setExtractionFailure(null); setExtractionHint(""); };
  const saveAndTrial = async () => {
    if (!canTrial || !frame.image || pending.current) return;
    const capturedScope = scope;
    setOperation("trial"); setRegionMode(false);
    try {
      const imageId = frame.image.id;
      let revision = data.workspace.revision;
      if (editing) {
        const params: ShotTeach = { path: t.path, mmPerPx: t.mmPerPx };
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
    const result = await perform(() => workspaceApi.saveTeach(doc.id,data.workspace.revision,k,frame.image!.id), "本组示教已保存，发布前仍需整体验证");
    if (result?.workspace.doc.id===doc.id && next && current.current.scope===scope && current.current.alive) advance();
  };

  const hint = editing && !shot.skip ? "中线或参数已改，点“运行一次算法”；现有试测不用于保存或发布。" : viewProgress?.hint ?? "";
  const defaults = data.workspace.doc.detect;
  const searchHalfPx = t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0 ? (t.detect ?? defaults).searchMm / t.mmPerPx : undefined;
  return <div className="wp-page"><WorkspaceBar /><Steps />
    <div className="wp-actions wp-teach-summary"><strong>拍照组 {groupIndex+1}／{groups.length}</strong><Badge tone={group.complete ? "ok" : "neutral"}>{group.status}</Badge><span className="muted">已完成 {completedGroups}／{groups.length} 组 · 每组示教一张图</span><span className="spacer" /><Link className="btn" to="/recipe/overview">工件总览（可选）</Link></div>
    <div className="wp-teach">
      <Panel title="拍照组" detail="同一位置的相机图像为一组" className="wp-group-panel"><nav className="wp-group-list" aria-label="拍照组选择">{groups.map((item,index) => <button key={item.poseId} className={"wp-group-item" + (index === groupIndex ? " selected" : "")} aria-label={"选择拍照组 " + item.poseId} aria-pressed={index === groupIndex} disabled={!unlocked || editing || frameDirty} onClick={() => selectGroup(index)}>
        <strong>{item.poseId}<span>{item.complete ? "已完成" : `${item.members.reduce((n,member) => n + data.workspace.frames[member].views.length,0)} 张图`}</span></strong>
        <small>{item.choice ? doc.shots[item.choice.k].camera + " · 图 " + item.choice.view : "未选定检测图"}</small><span>{item.status}</span>
      </button>)}</nav></Panel>
      <div className="wp-stack"><section className="panel wp-image-picker" aria-label="本组检测图选择">
        <div className="wp-image-picker-bar"><h2>{group.poseId}<span>检测图</span></h2>
          <div className="wp-group-images" role="group" aria-label="本组图像">{group.members.flatMap(member => data.workspace.frames[member].views.map(view => <FrozenViewOption key={view.id} id={doc.id} metadata={view} selected={group.choice?.k === member && group.choice.view === view.view} disabled={!unlocked || editing || frameDirty} onSelect={() => void selectImage(member,view.view)} />))}</div>
          <Link className="btn small" to="/recipe/capture">重新采集</Link>
        </div>
        {group.selected.length > 1 && <p className="c-warn" role="alert">本组原先选择了多张图，请重新选择一张作为检测图。</p>}
      </section>
      {enabled ? <><Panel title={shot.camera + " · 中线示教"}>
        <div className="wp-actions" role="group" aria-label="单帧示教操作"><button className="btn" aria-pressed={regionMode} disabled={!unlocked || !frame.image} onClick={() => setRegionMode(v => !v)}>{regionMode ? "结束选区 / 手工修正" : "刷选胶路区域"}</button>
          <button className="btn" disabled={!unlocked || !frame.image || loading || !!error} onClick={() => void extract()}><WandSparkles size={15}/>{operation === "extract" ? "提取中…" : "自动提取中线"}</button>
          <button className="btn" disabled={!unlocked || !enabled || !frame.image || selectionTool !== "brush" || !strokeValid || !spacingValid} onClick={fromStroke}>笔划直接生成中线</button>
          <button className="btn primary" disabled={!canTrial} title={trialDisabledReason || "保存当前参数，对本图运行一次检测算法"} onClick={() => void saveAndTrial()}><Play size={15}/>{operation === "trial" ? "计算中…" : "运行一次算法"}</button>
          <button className="btn primary" disabled={!unlocked || !enabled || !trialCurrent || viewState === "taught"} onClick={() => void save()}><Save size={15} />保存本组示教</button>
          <button className="btn" disabled={!unlocked || !enabled || !trialCurrent || !nextGroup} onClick={() => void save(true)}>保存并进入下一组</button>
        </div>
        <div className="wp-actions wp-teaching-tools">
          <label className="field wp-field"><span>选区工具</span><select className="input" aria-label="选区工具" value={selectionTool} disabled={!unlocked} onChange={e => { setSelectionTool(e.target.value as "brush" | "box"); setRegionMode(true); setExtractionFailure(null); setExtractionHint(""); }}><option value="brush">沿胶路刷选</option><option value="box">矩形框选</option></select></label>
          {selectionTool === "brush" && <div className="wp-brush-width">
            <NumberField label="笔刷宽度" value={brushWidth} unit="px" min={6} max={300} step={2} disabled={!unlocked} onChange={changeBrushWidth} />
            <input className="wp-brush-width-slider" type="range" aria-label="拖动调整笔刷宽度" min={6} max={300} step={2} value={Number.isFinite(brushWidth) ? Math.min(300, Math.max(6, brushWidth)) : 80} disabled={!unlocked} onChange={event => changeBrushWidth(Number(event.target.value))} />
          </div>}
          <NumberField label="中线点间距" value={pointSpacing} unit="px" min={4} max={80} step={2} disabled={!unlocked} onChange={setPointSpacing} />
          <button className="btn small" disabled={!unlocked || !spacingValid || t.path.length < 2} onClick={() => { setVertex(null); setPath(resampleTeachingPath(t.path, pointSpacing)); }}>应用到当前中线</button>
          <span className="muted">检测站距 {doc.spacing} mm · 编辑点按目标最大间距调整，转弯处保留点</span>
        </div>
        {(regionMode || extractionHint) && <p className="muted hint" role="status">{extractionHint || (regionMode ? selectionTool === "brush" ? "按住左键沿胶路从起点刷到终点，每笔重画选区；笔刷覆盖胶路和两侧少量背景，尽量避开阴影。选好后可自动提取，或直接按笔划生成中线草稿。" : "按住左键框选一段连续胶路，再自动提取中线。框选包含阴影时建议改用刷选。" : "")}</p>}
        {trialFresh && <div className="wp-actions" aria-label="本次算法结果"><Badge tone={trialCurrent ? "info" : "warn"}>{trial?.verdict ?? "执行异常"}</Badge><span>量成比例 {measured ? (trial!.coverage * 100).toFixed(1) + "%" : "—"}</span><span>耗时 {trial?.elapsedMs} ms</span><span className="muted">绿色：有效胶宽 · 红色：未通过候选 / 无候选</span></div>}
        <GrayViewer image={image} loading={loading} error={error} label={frame.image ? group.poseId + " · " + shot.camera + " · 图 " + shot.view : "等待取样"}
          region={selectionTool === "box" ? region : null} onRegion={regionMode && selectionTool === "box" && unlocked && enabled ? setRegion : undefined}
          brush={regionMode && selectionTool === "brush" && Number.isFinite(brushWidth) ? { points: brushPoints, radiusPx: brushWidth / 2 } : null} onBrush={regionMode && selectionTool === "brush" && unlocked && enabled ? points => { setBrushPoints(points); setExtractionFailure(null); setExtractionHint(""); } : undefined}
          extractionFailure={extractionFailure}
          overlay={{ path: t.path, stations: points?.filter(p => !p.edge && !p.candidateEdge).map(p => p.px) ?? stations, stationColors: points?.filter(p => !p.edge && !p.candidateEdge).map(p => p.status === "ok" ? "var(--ok)" : "var(--ng)"), segments: points?.flatMap((p, i) => {
            const edge = p.edge ?? p.candidateEdge;
            const width = p.edge ? p.width : p.candidateWidth;
            return edge ? [{ from: edge[0], to: edge[1], color: p.edge ? "var(--ok)" : "var(--ng)", label: i % Math.max(1, Math.ceil(points.length / 4)) === 0 && width != null ? width.toFixed(2) + (p.edge ? " mm" : " mm 候选") : undefined }] : [];
          }), searchHalfPx, stale: editing, selected: vertex }}
          editDisabled={!unlocked}
          onEdit={enabled && !regionMode ? { add: p => setPath([...t.path, p]), move: (i, p) => setPath(t.path.map((q, j) => j === i ? p : q)), select: setVertex } : undefined} />
        {productionBusy && <p className="muted hint">工件正在检测，当前可编辑已保存图像；重采需等待工件结束。</p>}
        <div className="wp-actions" style={{marginTop:10}}>
          <span className="muted">中线 {t.path.length} 点 · {t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0 ? (lengthPx * t.mmPerPx).toFixed(1) + " mm" : lengthPx.toFixed(0) + " px"}{own && !editing ? " · " + own.count + " 站" : ""}</span><span className="spacer" />
          <button className="btn small" disabled={!unlocked || !enabled || !t.path.length} onClick={() => { setVertex(null); setPath(t.path.slice(0, -1)); }}><Undo2 size={14} />删除末点</button>
          <button className="btn small" disabled={!unlocked || !enabled || vertex === null || vertex >= t.path.length} onClick={() => { const i = vertex; setVertex(null); setPath(t.path.filter((_, j) => j !== i)); }}><Trash2 size={14} />删除选中点</button>
          <button className="btn small" disabled={!unlocked || !enabled || !t.path.length} onClick={() => { setVertex(null); setPath([]); }}><Eraser size={14} />清空中线</button>
        </div>
        {frame.image && <div className="wp-kv"><span>取样来源</span><strong>{frame.image.source} · {new Date(frame.image.capturedAt).toLocaleString("zh-CN")}</strong></div>}
      </Panel>
      <Panel title="本组试测结果" actions={trialFresh && trial && <Badge tone={trialCurrent?"ok":"warn"}>{viewProgress?.trialStale?"已过期":trial.passed ? "有效 · " + (trial.verdict ?? "已完成") : "执行异常"}</Badge>}>
        <div className="wp-rule-stat"><div><span>量成比例</span><strong>{measured ? (trial!.coverage * 100).toFixed(1) + "%" : "—"}</strong></div><div><span>处理耗时</span><strong>{trialFresh && trial ? trial.elapsedMs + " ms" : "—"}</strong></div></div>
        {viewState === "trialFailed" && !editing && <Notice title="试测异常" tone="warn">{trial?.reason}</Notice>}
        {viewState === "recapture" && <Notice title="需重新采集" tone="warn">{viewProgress?.hint} <Link className="btn small" to="/recipe/capture">整圈重新采集</Link></Notice>}
        <p className="muted">{hint}</p>
        {points && <TrialResult points={points} />}
      </Panel></> : <Notice title="选择本组检测图">点击上方一张图开始示教，每组只需完成这一张。</Notice>}</div>
      {enabled && <div className="wp-stack wp-teach-settings"><TeachingSettings key={scope} teach={t} savedMmPerPx={saved.mmPerPx} defaults={defaults} defaultLimits={doc.limits} calibration={calibration} disabled={!unlocked} editing={editing} onChange={update}
        onRecord={async (reference, measured, reuse) => !!await perform(() => workspaceApi.checkCalibration(doc.id,data.workspace.revision,k,reference,measured,reuse), "已保存当前图的标定验证")} />
      <details className="wp-sample-details"><summary>图像采集信息</summary><Panel title="当前检测图"><KV label="拍照组">{group.poseId}</KV><KV label="相机">{shot.camera}</KV><KV label="当前编辑图">图 {shot.view}</KV><KV label="图像">{frame.image?.id ?? "未冻结"}</KV><KV label="曝光">{frame.image?.exposureUs != null ? frame.image.exposureUs + " μs" : "未记录"}</KV><KV label="增益">{frame.image?.gainDb != null ? frame.image.gainDb + " dB" : "未记录"}</KV><KV label="原图更新">仅通过整圈重新采集</KV>{imagingHint && <p className="c-warn" role="note">{imagingHint}；样本仍可用，需要时整圈重采</p>}</Panel></details>
      <Notice title="保存与发布分开">中线保存进候选配方，已发布版本不变。离线样本模式下可直接试运行已保存候选；用于生产前仍需完成标定、代表性样本验证与发布。</Notice></div>}
    </div></div>;
}
