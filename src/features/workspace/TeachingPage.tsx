import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState } from "react";
import { Link, useLocation, useSearchParams } from "react-router-dom";
import { Eraser, Play, Save, Trash2, Undo2, WandSparkles } from "lucide-react";
import { useCycle } from "../cycle/api";
import { pathLength, shotSegment } from "../cycle/vis";
import type { DetectParams, Polarity, JudgeParams, RecipeDoc, ShotLimits } from "../cycle/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, GrayViewer, KV, Notice, NumberField, Panel, Steps, useGrayImage, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { FrozenImage, ShotTeach, WorkspaceView } from "./types";
import { sameTeach, shotTeach, teachError } from "./teach";
import { calibrationText } from "./progress";
import { returnState } from "./returnNavigation";
import { TrialResult, trialPoints } from "./TrialResult";
import { teachingGroups } from "./teachingGroups";
import { productionBusy as isProductionBusy } from "../cycle/phase";

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
  const location = useLocation();
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
  const [vertex,setVertex]=useState<number|null>(null);
  const [region, setRegion] = useState<[number,number,number,number] | null>(null);
  const [regionMode, setRegionMode] = useState(false);
  const [referenceMm, setReferenceMm] = useState(10), [measuredMm, setMeasuredMm] = useState(NaN);
  const [reuseCalibration,setReuseCalibration] = useState(false);
  const pending=useRef(false),serial=useRef(0);
  const scope=JSON.stringify([doc?.id,k,data?.workspace.doc.shots[k]?.view,data?.workspace.frames[k]?.image?.id]);
  const current=useRef({scope,alive:true,revision:data?.workspace.revision});current.current.scope=scope;current.current.revision=data?.workspace.revision;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;serial.current++;};},[]);
  useEffect(()=>{serial.current++;pending.current=false;setWorking(false);setVertex(null);setRegion(null);setRegionMode(false);setReferenceMm(10);setMeasuredMm(NaN);setReuseCalibration(false);setOperation(null);setExtractionHint("");},[scope]);
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
  const setDetect = (detect: DetectParams | undefined) => { const next: ShotTeach = { ...t }; if (detect) next.detect = detect; else delete next.detect; update(next); };
  const shotProgress = data.progress.shots[k];
  const viewProgress = shotProgress?.views.find(v => v.view === shot.view);
  const groups = teachingGroups(doc.shots, data.progress);
  const groupIndex = groups.findIndex(group => group.members.includes(k));
  const group = groups[groupIndex];
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
  const roi: [number, number, number, number] | null = region && frame.image ? (() => {
    const x = Math.max(0, Math.floor(region[0])), y = Math.max(0, Math.floor(region[1]));
    return [x, y, Math.min(frame.image.size[0], Math.ceil(region[0] + region[2])) - x, Math.min(frame.image.size[1], Math.ceil(region[1] + region[3])) - y];
  })() : null;
  const roiValid = !!roi && roi[2] >= 5 && roi[3] >= 5 && roi[2] * roi[3] <= 4_000_000;
  const extract = async () => {
    if (!unlocked || !enabled || !frame.image || pending.current) return;
    if (!roiValid || !roi) { setRegionMode(true); setExtractionHint("请在图像上按住左键，框选一段连续胶路（至少 5×5 像素，最多 400 万像素），再点自动提取中线。"); return; }
    const capturedScope = scope;
    setOperation("extract"); setExtractionHint("");
    try {
      let candidate = data;
      if (editing) {
        const saved = await perform(() => workspaceApi.saveDraft(doc.id,data.workspace.revision,k,t), "");
        if (!saved) return;
        candidate = saved;
      }
      let extracted: Awaited<ReturnType<typeof workspaceApi.extractCenterline>> | null = null;
      const result = await perform(async () => {
        extracted = await workspaceApi.extractCenterline(doc.id,candidate.workspace.revision,k,frame.image!.id,roi);
        return candidate;
      }, "");
      if (result && extracted && current.current.scope === capturedScope) {
        const output = extracted as Awaited<ReturnType<typeof workspaceApi.extractCenterline>>;
        setFrameDraft(k,{...t,path:output.path}); setRegionMode(false); setVertex(null);
        setExtractionHint(output.message || "中线已提取，请检查后运行一次算法。");
      }
    } finally { if (current.current.scope === capturedScope) setOperation(null); }
  };
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
  const detect = t.detect;
  const defaults = data.workspace.doc.detect;
  const searchHalfPx = t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0 ? (detect ?? defaults).searchMm / t.mmPerPx : undefined;
  const changeLimits = (limits: ShotLimits | undefined) => { const next={...t}; if(limits) next.limits=limits; else delete next.limits; update(next); };
  const limitGroup = (key: "position" | "width", title: string) => {
    const value=t.limits?.[key];
    return <div key={key}><label className="check"><input type="checkbox" checked={!!value} disabled={!unlocked} onChange={e => t.limits && changeLimits({...t.limits,[key]:e.target.checked ? structuredClone(doc.limits[key] ?? {nominal:0,tolUpper:.1,tolLower:.1,absMin:-1,absMax:1,maxExcursionLen:1}) : null})}/>{title}</label>{value && <div className="wp-form-grid">{([["nominal","名义值"],["tolUpper","上公差"],["tolLower","下公差"],["absMin","绝对下限"],["absMax","绝对上限"],["maxExcursionLen","允许超差长度"]] as [keyof JudgeParams,string][]).map(([field,label]) => <NumberField key={field} label={title + label} value={value[field]} unit="mm" step={.1} disabled={!unlocked} onChange={v => t.limits && changeLimits({...t.limits,[key]:{...value,[field]:v}})}/>)}</div>}</div>;
  };
  const polarity: Record<Polarity, string> = { dark: "暗胶条", light: "亮胶条" };
  return <div className="wp-page"><WorkspaceBar /><Steps />
    <div className="wp-actions wp-teach-summary"><strong>拍照组 {groupIndex+1}／{groups.length}</strong><Badge tone={group.complete ? "ok" : "neutral"}>{group.status}</Badge><span className="muted">已完成 {completedGroups}／{groups.length} 组 · 每组示教一张图</span><span className="spacer" /><Link className="btn" to="/recipe/overview">工件总览（可选）</Link></div>
    <div className="wp-teach">
      <Panel title="拍照组" detail="同一位置的相机图像为一组" className="wp-group-panel"><nav className="wp-group-list" aria-label="拍照组选择">{groups.map((item,index) => <button key={item.poseId} className={"wp-group-item" + (index === groupIndex ? " selected" : "")} aria-label={"选择拍照组 " + item.poseId} aria-pressed={index === groupIndex} disabled={!unlocked || editing || frameDirty} onClick={() => selectGroup(index)}>
        <strong>{item.poseId}<span>{item.complete ? "已完成" : `${item.members.reduce((n,member) => n + data.workspace.frames[member].views.length,0)} 张图`}</span></strong>
        <small>{item.choice ? doc.shots[item.choice.k].camera + " · 图 " + item.choice.view : "未选定检测图"}</small><span>{item.status}</span>
      </button>)}</nav></Panel>
      <div className="wp-stack"><section className="panel wp-image-picker" aria-label="本组检测图选择">
        <div className="wp-image-picker-bar"><h2>{group.poseId}<span>检测图</span></h2>
          <div className="wp-group-images" role="group" aria-label="本组图像" aria-describedby="image-picker-hint">{group.members.flatMap(member => data.workspace.frames[member].views.map(view => <FrozenViewOption key={view.id} id={doc.id} metadata={view} selected={group.choice?.k === member && group.choice.view === view.view} disabled={!unlocked || editing || frameDirty} onSelect={() => void selectImage(member,view.view)} />))}</div>
          <Link className="btn small" to="/recipe/capture">重新采集</Link>
        </div>
        <p id="image-picker-hint" className="muted hint">选择胶路清晰的一张图用于示教与检测，其余图仅采集。{(editing || frameDirty) && "当前有未保存的示教修改，暂不能切换检测图。"}</p>
        {group.selected.length > 1 && <p className="c-warn" role="alert">本组原先选择了多张图，请重新选择一张作为检测图。</p>}
      </section>
      {enabled ? <><Panel title={enabled ? shot.camera + " · 中线示教" : "中线示教"} detail={enabled ? "框选胶路 → 提取与修正中线 → 试测 → 保存本组" : "先从上方选择本组唯一的检测图"}>
        <div className="wp-actions"><button className="btn" aria-pressed={regionMode} disabled={!unlocked || !frame.image} onClick={() => setRegionMode(v => !v)}>{regionMode ? "结束框选 / 手工修正" : "框选胶路区域"}</button>
          <button className="btn" disabled={!unlocked || !frame.image || loading || !!error} onClick={() => void extract()}><WandSparkles size={15}/>{operation === "extract" ? "提取中…" : "自动提取中线"}</button>
          <button className="btn primary" disabled={!canTrial} title={trialDisabledReason || "保存当前参数，对本图运行一次检测算法"} onClick={() => void saveAndTrial()}><Play size={15}/>{operation === "trial" ? "计算中…" : "运行一次算法"}</button>
        </div>
        <p className="muted hint" role="status">{regionMode ? roiValid ? "区域已选好，点击自动提取中线；提取结果可继续拖动修正。" : "在图像上按住左键框选一段连续胶路（至少 5×5 像素，最多 400 万像素），再点击自动提取中线。" : extractionHint || "自动提取需先框选胶路区域，也可直接在图上逐点绘制中线。"}</p>
        {trialDisabledReason && <p className="muted hint">暂不能运行：{trialDisabledReason}。单次试测会保存候选参数，不会发布生产配方。</p>}
        {trialFresh && <div className="wp-actions" aria-label="本次算法结果"><Badge tone={trialCurrent ? "info" : "warn"}>{trial?.verdict ?? "执行异常"}</Badge><span>量成比例 {measured ? (trial!.coverage * 100).toFixed(1) + "%" : "—"}</span><span>耗时 {trial?.elapsedMs} ms</span><span className="muted">绿色：有效胶宽 · 红色：未通过候选 / 无候选</span></div>}
        <GrayViewer image={image} loading={loading} error={error} label={frame.image ? group.poseId + " · " + shot.camera + " · 图 " + shot.view : "等待取样"}
          region={region} onRegion={regionMode && unlocked && enabled ? setRegion : undefined}
          overlay={{ path: t.path, stations: points?.filter(p => !p.edge && !p.candidateEdge).map(p => p.px) ?? stations, stationColors: points?.filter(p => !p.edge && !p.candidateEdge).map(p => p.status === "ok" ? "var(--ok)" : "var(--ng)"), segments: points?.flatMap((p, i) => {
            const edge = p.edge ?? p.candidateEdge;
            const width = p.edge ? p.width : p.candidateWidth;
            return edge ? [{ from: edge[0], to: edge[1], color: p.edge ? "var(--ok)" : "var(--ng)", label: i % Math.max(1, Math.ceil(points.length / 4)) === 0 && width != null ? width.toFixed(2) + (p.edge ? " mm" : " mm 候选") : undefined }] : [];
          }), searchHalfPx, stale: editing, selected: vertex }}
          onEdit={unlocked && enabled && !regionMode ? { add: p => setPath([...t.path, p]), move: (i, p) => setPath(t.path.map((q, j) => j === i ? p : q)), select: setVertex } : undefined} />
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
        <div className="wp-actions"><button className="btn primary" disabled={!unlocked || !enabled || !trialCurrent || viewState === "taught"} onClick={() => void save()}><Save size={15} />保存本组示教</button><button className="btn" disabled={!unlocked || !enabled || !trialCurrent || !nextGroup} onClick={() => void save(true)}>保存并进入下一组</button></div>
        <div className="wp-rule-stat"><div><span>量成比例</span><strong>{measured ? (trial!.coverage * 100).toFixed(1) + "%" : "—"}</strong></div><div><span>处理耗时</span><strong>{trialFresh && trial ? trial.elapsedMs + " ms" : "—"}</strong></div></div>
        {viewState === "trialFailed" && !editing && <Notice title="试测异常" tone="warn">{trial?.reason}</Notice>}
        {viewState === "recapture" && <Notice title="需重新采集" tone="warn">{viewProgress?.hint} <Link className="btn small" to="/recipe/capture">整圈重新采集</Link></Notice>}
        <p className="muted">{hint}</p>
        {points && <TrialResult points={points} />}
      </Panel></> : <Notice title="选择本组检测图">点击上方一张图开始示教，每组只需完成这一张。</Notice>}</div>
      {enabled && <div className="wp-stack"><Panel title="像素当量与标定验证" detail="标定误差目标 ±0.1 mm；修改当量后需重新试测和验证"><div className="wp-form-grid">
        <NumberField label="像素当量" value={t.mmPerPx} unit="mm/px" min={.0001} step={.0005} onChange={v => update({ ...t, mmPerPx: Number.isFinite(v) ? v : null })} disabled={!unlocked || !enabled} />
      </div>
        <Link className="btn small" to="/camera/calibration" state={returnState(location, "单帧示教")}>飞拍标定</Link>
        <p className="muted">{calibrationText(calibration)}</p>
        {calibration.state === "sampleSet" ? <p className="muted hint">当量来自样本集声明，标定验证以此为依据；改动当量后需按人工流程重新验证。</p> : <>
        <div className="wp-form-grid"><NumberField label="已知参考尺寸" value={referenceMm} unit="mm" onChange={setReferenceMm} disabled={!unlocked}/><NumberField label="实测尺寸" value={measuredMm} unit="mm" onChange={setMeasuredMm} disabled={!unlocked}/></div>
        <label className="check"><input type="checkbox" checked={reuseCalibration} disabled={!unlocked || editing} onChange={e=>setReuseCalibration(e.target.checked)}/>已确认同设备同图的其他拍照点成像几何适用，复用此次代表点验证</label><button className="btn" disabled={!unlocked || editing || !(referenceMm > 0 && measuredMm > 0)} onClick={() => void perform(() => workspaceApi.checkCalibration(doc.id,data.workspace.revision,k,referenceMm,measuredMm,reuseCalibration), "已保存当前图的标定验证")}>记录人工标定验证</button>
        <p className="muted">记录已知参考件的实测误差（人工验证记录），请填写实际测量结果。</p></>}
        <label className="check" style={{marginTop:12}}><input type="checkbox" checked={!!detect} disabled={!unlocked || !enabled} onChange={e => setDetect(e.target.checked ? structuredClone(defaults) : undefined)} />本图自定义参数（取消可恢复配方默认）</label>
        {detect ? <div className="wp-form-grid" style={{marginTop:8}}>
          <NumberField label="搜索半宽" value={detect.searchMm} unit="mm" min={.1} step={.5} onChange={v => setDetect({ ...detect, searchMm: v })} disabled={!unlocked} />
          <label className="field wp-field"><span>极性</span><select className="input" aria-label="极性" value={detect.polarity} disabled={!unlocked} onChange={e => setDetect({ ...detect, polarity: e.target.value as Polarity })}><option value="dark">暗胶条</option><option value="light">亮胶条</option></select></label>
          <NumberField label="胶宽下限" value={detect.widthRange[0]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [v, detect.widthRange[1]] })} disabled={!unlocked} />
          <NumberField label="胶宽上限" value={detect.widthRange[1]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [detect.widthRange[0], v] })} disabled={!unlocked} />
        </div> : <p className="muted hint">用配方的检测参数：搜索半宽 {defaults.searchMm} mm · {polarity[defaults.polarity]} · 胶宽 {defaults.widthRange[0]}–{defaults.widthRange[1]} mm</p>}
        <label className="check"><input type="checkbox" checked={!!t.limits} disabled={!unlocked || !enabled} onChange={e=>changeLimits(e.target.checked ? structuredClone(doc.limits) : undefined)}/>本图自定义判定限值（取消可恢复默认）</label>
        {t.limits ? <div className="wp-stack">{limitGroup("position","位置")}{limitGroup("width","胶宽")}<div className="wp-form-grid"><NumberField label="允许连续缺胶长度" value={t.limits.maxGapLen} unit="mm" min={0} step={.1} disabled={!unlocked} onChange={v=>t.limits && changeLimits({...t.limits,maxGapLen:v})}/><NumberField label="最低有胶比例" value={t.limits.minPresent} min={0} max={1} step={.05} disabled={!unlocked} onChange={v=>t.limits && changeLimits({...t.limits,minPresent:v})}/></div></div> : <p className="muted hint">判定限值沿用配方默认值。测量误差目标与胶宽合格公差分别设置。</p>}
        {editing && invalid && <Notice title="本组中线无效" tone="warn">{invalid}</Notice>}
        </Panel>
      <details className="wp-sample-details"><summary>图像采集信息</summary><Panel title="当前检测图"><KV label="拍照组">{group.poseId}</KV><KV label="相机">{shot.camera}</KV><KV label="当前编辑图">图 {shot.view}</KV><KV label="图像">{frame.image?.id ?? "未冻结"}</KV><KV label="曝光">{frame.image?.exposureUs != null ? frame.image.exposureUs + " μs" : "未记录"}</KV><KV label="增益">{frame.image?.gainDb != null ? frame.image.gainDb + " dB" : "未记录"}</KV><KV label="原图更新">仅通过整圈重新采集</KV>{imagingHint && <p className="c-warn" role="note">{imagingHint}；样本仍可用，需要时整圈重采</p>}</Panel></details>
      <Notice title="保存与发布分开">中线保存进候选配方，生产配方不变；保存本组后，先验证代表性样本，再发布生产版本。</Notice></div>}
    </div></div>;
}
