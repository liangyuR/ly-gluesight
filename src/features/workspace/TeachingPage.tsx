import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Eraser, Save, Trash2, Undo2, WandSparkles } from "lucide-react";
import { useCycle } from "../cycle/api";
import { pathLength, shotLabel, shotSegment } from "../cycle/vis";
import type { DetectParams, Polarity, JudgeParams, ShotLimits } from "../cycle/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, FrameRail, GrayViewer, KV, Notice, NumberField, Panel, Steps, useGrayImage, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { FrozenImage, ShotTeach, WorkspaceView } from "./types";
import { sameJsonValue, sameTeach, shotTeach, teachError, selectedViews, teachingCounts } from "./teach";

function FrozenViewOption({ id, metadata, selected, disabled, onSelect }: { id: string; metadata: FrozenImage; selected: boolean; disabled: boolean; onSelect: () => void }) {
  const { image, loading, error } = useGrayImage(id, metadata.id);
  return <button className={"wp-view-option" + (selected ? " selected" : "")} aria-label={"查看图 " + metadata.view} aria-pressed={selected} disabled={disabled} onClick={onSelect}>
    <span className="wp-view-thumbnail">{image ? <img src={image.url} alt={"图 " + metadata.view} /> : <span>{loading ? "读取中…" : error ? "图像不可用" : "等待图像"}</span>}</span>
    <strong>图 {metadata.view}{selected ? " · 已选" : ""}</strong><small>{metadata.size[0]} × {metadata.size[1]}</small>
  </button>;
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
  const [vertex,setVertex]=useState<number|null>(null);
  const [region, setRegion] = useState<[number,number,number,number] | null>(null);
  const [regionMode, setRegionMode] = useState(false);
  const [referenceMm, setReferenceMm] = useState(10), [measuredMm, setMeasuredMm] = useState(NaN);
  const [reuseCalibration,setReuseCalibration] = useState(false);
  const pending=useRef(false),serial=useRef(0);
  const scope=JSON.stringify([doc?.id,k,data?.workspace.doc.shots[k]?.view,data?.workspace.frames[k]?.image?.id]);
  const current=useRef({scope,alive:true,revision:data?.workspace.revision});current.current.scope=scope;current.current.revision=data?.workspace.revision;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;serial.current++;};},[]);
  useEffect(()=>{serial.current++;pending.current=false;setWorking(false);setVertex(null);setRegion(null);setRegionMode(false);setReferenceMm(10);setMeasuredMm(NaN);setReuseCalibration(false);},[scope]);
  const perform=async(request:()=>Promise<WorkspaceView>,message:string)=>{
    if(pending.current||busy||dirty)return null;
    pending.current=true;setWorking(true);
    const action=++serial.current;
    const valid=()=>current.current.alive&&current.current.scope===scope&&action===serial.current;
    const revision=data?.workspace.revision;
    try{const result=await act(request,message);return valid()&&result&&(current.current.revision===revision||current.current.revision===result.workspace.revision)?result:null;}
    catch(e){if(valid()&&current.current.revision===revision)setError(String(e));return null;}
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
  const t = frameDrafts[k] ?? saved;
  const editing = !sameTeach(t, saved);
  const invalid = teachError(t);
  const taught = shot.path.length >= 2 && shot.mmPerPx != null;
  const unlocked = !busy && !dirty && !working && !reading && !data.workspace.pending;
  const productionBusy = !!snapshot && !["IDLE","FAULT"].includes(snapshot.phase);
  const update = (next: ShotTeach) => {if(unlocked)setFrameDraft(k,next);};
  const setPath = (path: [number, number][]) => update({ ...t, path });
  const setDetect = (detect: DetectParams | undefined) => { const next: ShotTeach = { ...t }; if (detect) next.detect = detect; else delete next.detect; update(next); };
  const enabledViews = selectedViews(shot);
  const counts = teachingCounts([shot],[frame]);
  const allCounts = teachingCounts(doc.shots,data.workspace.frames);
  const enabled = !shot.skip && enabledViews.includes(shot.view);
  const calibration = frame.viewStates?.find(v => v.view === shot.view)?.calibrationCheck;
  const trial = frame.trial;
  const staleTrial = !!trial && !editing && (trial.imageId !== frame.image?.id || !sameJsonValue(trial.geometryTag, frame.image?.geometryTag));
  const trialCurrent = !editing && !staleTrial && !!trial?.passed;
  const measured = !!trial && trial.measurement != null;
  const own = shotSegment(data.layout, k)?.segment;
  const stations: [number, number][] = own ? Array.from({ length: own.count }, (_, i) => [data.layout.points.x[own.first + i], data.layout.points.y[own.first + i]]) : [];
  const lengthPx = pathLength(t.path);
  const canTrial = unlocked && !!frame.image && !editing && taught && enabled;
  const run = () => canTrial && frame.image && perform(() => workspaceApi.trial(doc.id,data.workspace.revision,k,frame.image!.id), "");
  const selectView = (view: number) => {
    if (!unlocked || editing || frameDirty || pending.current || view === shot.view || !frame.views.some(image => image.view === view)) return;
    void perform(() => workspaceApi.selectView(doc.id,data.workspace.revision,k,view), "已切换编辑图，其他图的示教保持保存");
  };
  const saveLine = () => {
    if (!unlocked || !editing || invalid) return;
    const params: ShotTeach = { path: t.path, mmPerPx: t.mmPerPx };
    if (t.detect) params.detect = t.detect;
    if (t.limits) params.limits = t.limits;
    void perform(() => workspaceApi.saveParams(doc.id,data.workspace.revision,k,params), "中线已保存到候选配方，请重新试测");
  };
  const save = async (next = false) => {
    if (!unlocked||!trialCurrent||!frame.image||pending.current) return;
    const advance = async (result: WorkspaceView) => {
      const remaining = enabledViews.find(v => v !== shot.view && !result.workspace.frames[k]?.viewStates?.find(state => state.view === v)?.saved);
      if (remaining != null) await act(() => workspaceApi.selectView(doc.id,result.workspace.revision,k,remaining));
      else setK(Math.min(k+1,frameCount-1));
    };
    if(frame.saved){if(next)await advance(data);return;}
    const result = await perform(() => workspaceApi.saveTeach(doc.id,data.workspace.revision,k,frame.image!.id), "本帧示教已保存，发布前仍需整体验证");
    if (result?.workspace.doc.id===doc.id && next && current.current.scope===scope && current.current.alive) await advance(result);
  };

  const hint = shot.skip ? "这个拍照点设为不检：只要求这一帧到达，不量不判，不需要示教中线。"
    : editing ? "中线或参数已改，先保存中线再试测；现有试测不用于保存或发布。"
    : !taught ? "在原图上从胶嘴一侧往外依次点出胶路中线，填好像素当量后保存中线。"
    : staleTrial ? "现有试测已过期，请重新试测当前帧。"
    : trial ? trial.passed ? trial.reason : "试测执行异常，只保存草稿。"
    : frame.image ? "中线已保存，可以试测当前帧。" : "先取样或导入这个拍照点的原图。";
  const detect = t.detect;
  const defaults = data.workspace.doc.detect;
  const changeLimits = (limits: ShotLimits | undefined) => { const next={...t}; if(limits) next.limits=limits; else delete next.limits; update(next); };
  const limitGroup = (key: "position" | "width", title: string) => {
    const value=t.limits?.[key];
    return <div key={key}><label className="check"><input type="checkbox" checked={!!value} disabled={!unlocked} onChange={e => t.limits && changeLimits({...t.limits,[key]:e.target.checked ? structuredClone(doc.limits[key] ?? {nominal:0,tolUpper:.1,tolLower:.1,absMin:-1,absMax:1,maxExcursionLen:1}) : null})}/>{title}</label>{value && <div className="wp-form-grid">{([["nominal","名义值"],["tolUpper","上公差"],["tolLower","下公差"],["absMin","绝对下限"],["absMax","绝对上限"],["maxExcursionLen","允许超差长度"]] as [keyof JudgeParams,string][]).map(([field,label]) => <NumberField key={field} label={title + label} value={value[field]} unit="mm" step={.1} disabled={!unlocked} onChange={v => t.limits && changeLimits({...t.limits,[key]:{...value,[field]:v}})}/>)}</div>}</div>;
  };
  const polarity: Record<Polarity, string> = { dark: "暗胶条", light: "亮胶条" };
  return <div className="wp-page"><WorkspaceBar /><Steps />
    <div className="wp-actions"><Badge tone={shot.skip?"neutral":frame.saved&&trialCurrent?"ok":trialCurrent?"info":"neutral"}>{shot.skip?"不检":editing ? "中线待保存" : staleTrial?"试测已过期":frame.saved ? "本帧已保存" : trial ? trial.passed ? "有效试测" : "试测异常" : !taught ? "待点中线" : frame.image ? "待试测" : "待取样"}</Badge><span className="muted">拍照点 {k+1}／{frameCount} · 本点已完成 {counts.completed}／{counts.total} 幅 · 全部 {allCounts.completed}／{allCounts.total} 幅</span><span className="spacer" /><Link className="btn" to="/recipe/overview">布置总览</Link></div>
    <div className="wp-teach">
      <Panel title={shotLabel(shot,k)} detail="按本轮采集顺序逐点示教"><FrameRail id={doc.id} frames={data.workspace.frames} selected={k} onSelect={setK} disabled={busy||working||reading||frameDirty} /></Panel>
      <div className="wp-stack"><Panel title={"k" + (k+1) + " · 单帧图像"} detail="选择图像 → 框选胶路 → 提取中线 → 修正 → 试测 → 完成" actions={<Link className="btn" to="/recipe/capture">整圈重新采集</Link>}>
        <label className="check"><input type="checkbox" checked={shot.skip} disabled={!unlocked || editing || frameDirty} onChange={e => void perform(() => workspaceApi.setViews(doc.id,data.workspace.revision,k,enabledViews,e.target.checked), "已更新拍照点检测设置")} />本拍照点不检测（仍要求收图）</label>
        <div className="wp-view-list" aria-label="图像选择">{frame.views.map(view => <div key={view.id}>
          <label className="check"><input type="checkbox" aria-label={"检测图 " + view.view} checked={enabledViews.includes(view.view)} disabled={!unlocked || editing || frameDirty || shot.skip} onChange={e => void perform(() => workspaceApi.setViews(doc.id,data.workspace.revision,k,e.target.checked ? [...enabledViews,view.view] : enabledViews.filter(v => v !== view.view),false), "选图已保存，请重新整套验证")} />检测图 {view.view}</label>
          <FrozenViewOption id={doc.id} metadata={view} selected={shot.view === view.view} disabled={!unlocked || editing || frameDirty || shot.view === view.view} onSelect={() => selectView(view.view)} />
          <small>{frame.viewStates?.find(s => s.view === view.view)?.saved ? "示教已完成" : "待示教"}</small>
        </div>)}</div>
        <p className="muted hint">勾选参与检测的图，点击缩略图编辑。各图分别保存中线和参数；未选图不等于本点不检测。</p>
        <div className="wp-actions"><button className="btn" aria-pressed={regionMode} disabled={!unlocked || !enabled || !frame.image} onClick={() => setRegionMode(v => !v)}>{regionMode ? "结束框选 / 手工修正" : "框选胶路区域"}</button><button className="btn" disabled={!unlocked || !enabled || !frame.image || !region || region[2] < 2 || region[3] < 2} onClick={async () => {
          if (!frame.image || !region) return;
          const capturedScope=scope; setWorking(true);
          try { const result = await workspaceApi.extractCenterline(doc.id,data.workspace.revision,k,frame.image.id,region); if (current.current.alive && current.current.scope===capturedScope) { setFrameDraft(k,{...t,path:result.path}); setRegionMode(false); } }
          catch(e) { if(current.current.scope===capturedScope) setError(String(e)); }
          finally { if(current.current.scope===capturedScope) setWorking(false); }
        }}><WandSparkles size={15}/>自动提取中线</button></div>
        <GrayViewer image={image} loading={loading} error={error} label={frame.image ? "冻结图像 · " + frame.image.id : "等待取样"}
          region={region} onRegion={regionMode && unlocked && enabled ? setRegion : undefined}
          overlay={{ path: t.path, stations, stale: editing, selected: vertex }}
          onEdit={unlocked && enabled && !regionMode ? { add: p => setPath([...t.path, p]), move: (i, p) => setPath(t.path.map((q, j) => j === i ? p : q)), select: setVertex } : undefined} />
        {productionBusy && <p className="muted hint">工件正在检测，当前可编辑已保存图像；重采需等待工件结束。</p>}
        <div className="wp-actions" style={{marginTop:10}}>
          <span className="muted">中线 {t.path.length} 点 · {t.mmPerPx != null && Number.isFinite(t.mmPerPx) && t.mmPerPx > 0 ? (lengthPx * t.mmPerPx).toFixed(1) + " mm" : lengthPx.toFixed(0) + " px"}{own && !editing ? " · " + own.count + " 站" : ""}</span><span className="spacer" />
          <button className="btn small" disabled={!unlocked || shot.skip || !t.path.length} onClick={() => { setVertex(null); setPath(t.path.slice(0, -1)); }}><Undo2 size={14} />删除末点</button>
          <button className="btn small" disabled={!unlocked || shot.skip || vertex === null || vertex >= t.path.length} onClick={() => { const i = vertex; setVertex(null); setPath(t.path.filter((_, j) => j !== i)); }}><Trash2 size={14} />删除选中点</button>
          <button className="btn small" disabled={!unlocked || shot.skip || !t.path.length} onClick={() => { setVertex(null); setPath([]); }}><Eraser size={14} />清空中线</button>
        </div>
        {frame.image && <div className="wp-kv"><span>取样来源</span><strong>{frame.image.source} · {new Date(frame.image.capturedAt).toLocaleString("zh-CN")}</strong></div>}
      </Panel>
      <Panel title="本帧试测结果" actions={trial && <Badge tone={trialCurrent?"ok":"warn"}>{staleTrial?"已过期":trial.passed ? "有效 · " + (trial.verdict ?? "已完成") : "执行异常"}</Badge>}>
        <div className="wp-actions"><button className="btn" disabled={!canTrial} onClick={() => void run()}><WandSparkles size={15} />试测当前帧</button><button className="btn primary" disabled={!unlocked || !trialCurrent || frame.saved} onClick={() => void save()}><Save size={15} />保存本帧示教</button><button className="btn" disabled={!unlocked || !trialCurrent} onClick={() => void save(true)}>保存并示教下一幅</button></div>
        <div className="wp-rule-stat"><div><span>量成比例</span><strong>{measured ? (trial!.coverage * 100).toFixed(1) + "%" : "—"}</strong></div><div><span>处理耗时</span><strong>{trial ? trial.elapsedMs + " ms" : "—"}</strong></div></div>
        {trial && !trial.passed && !staleTrial && !editing && <Notice title="试测异常" tone="warn">{trial.reason}</Notice>}
        <p className="muted">{hint}</p>
      </Panel></div>
      <div className="wp-stack"><Panel title="标定与本图参数" detail="标定误差目标 ±0.1 mm；修改后重新试测"><div className="wp-form-grid">
        <NumberField label="像素当量" value={t.mmPerPx} unit="mm/px" min={.0001} step={.0005} onChange={v => update({ ...t, mmPerPx: Number.isFinite(v) ? v : null })} disabled={!unlocked || shot.skip} />
      </div>
        <Link className="btn small" to="/camera/calibration">打开设备标定</Link>
        <div className="wp-form-grid"><NumberField label="已知参考尺寸" value={referenceMm} unit="mm" onChange={setReferenceMm} disabled={!unlocked}/><NumberField label="实测尺寸" value={measuredMm} unit="mm" onChange={setMeasuredMm} disabled={!unlocked}/></div>
        <label className="check"><input type="checkbox" checked={reuseCalibration} disabled={!unlocked || editing} onChange={e=>setReuseCalibration(e.target.checked)}/>已确认同设备同图的其他拍照点成像几何适用，复用此次代表点验证</label><button className="btn" disabled={!unlocked || editing || !(referenceMm > 0 && measuredMm > 0)} onClick={() => void perform(() => workspaceApi.checkCalibration(doc.id,data.workspace.revision,k,referenceMm,measuredMm,reuseCalibration), "已保存当前图的标定验证")}>记录人工标定验证</button>
        <p className="muted">记录已知参考件的实测误差（人工验证记录），请填写实际测量结果。</p><p className="muted">{calibration ? `标定验证${calibration.passed ? "通过" : "未通过"} · 误差 ${calibration.errorMm.toFixed(3)} mm` : "尚未记录标定精度验证"}</p>
        <label className="check" style={{marginTop:12}}><input type="checkbox" checked={!!detect} disabled={!unlocked || shot.skip} onChange={e => setDetect(e.target.checked ? structuredClone(defaults) : undefined)} />本图自定义参数（取消可恢复配方默认）</label>
        {detect ? <div className="wp-form-grid" style={{marginTop:8}}>
          <NumberField label="搜索半宽" value={detect.searchMm} unit="mm" min={.1} step={.5} onChange={v => setDetect({ ...detect, searchMm: v })} disabled={!unlocked} />
          <label className="field wp-field"><span>极性</span><select className="input" aria-label="极性" value={detect.polarity} disabled={!unlocked} onChange={e => setDetect({ ...detect, polarity: e.target.value as Polarity })}><option value="dark">暗胶条</option><option value="light">亮胶条</option></select></label>
          <NumberField label="胶宽下限" value={detect.widthRange[0]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [v, detect.widthRange[1]] })} disabled={!unlocked} />
          <NumberField label="胶宽上限" value={detect.widthRange[1]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [detect.widthRange[0], v] })} disabled={!unlocked} />
        </div> : <p className="muted hint">用配方的检测参数：搜索半宽 {defaults.searchMm} mm · {polarity[defaults.polarity]} · 胶宽 {defaults.widthRange[0]}–{defaults.widthRange[1]} mm</p>}
        <label className="check"><input type="checkbox" checked={!!t.limits} disabled={!unlocked || shot.skip} onChange={e=>changeLimits(e.target.checked ? structuredClone(doc.limits) : undefined)}/>本图自定义判定限值（取消可恢复默认）</label>
        {t.limits ? <div className="wp-stack">{limitGroup("position","位置")}{limitGroup("width","胶宽")}<div className="wp-form-grid"><NumberField label="允许连续缺胶长度" value={t.limits.maxGapLen} unit="mm" min={0} step={.1} disabled={!unlocked} onChange={v=>t.limits && changeLimits({...t.limits,maxGapLen:v})}/><NumberField label="最低有胶比例" value={t.limits.minPresent} min={0} max={1} step={.05} disabled={!unlocked} onChange={v=>t.limits && changeLimits({...t.limits,minPresent:v})}/></div></div> : <p className="muted hint">判定限值沿用配方默认值。测量误差目标与胶宽合格公差分别设置。</p>}
        {editing && invalid && <Notice title="本帧中线无效" tone="warn">{invalid}</Notice>}
        <button className="btn primary" style={{marginTop:14}} disabled={!unlocked || !editing || !!invalid} onClick={saveLine}><Save size={15} />保存中线</button></Panel>
      <Panel title="样本绑定"><KV label="工作帧">k{k+1}</KV><KV label="当前编辑图">图 {shot.view}</KV><KV label="图像">{frame.image?.id ?? "未冻结"}</KV><KV label="曝光">{frame.image?.exposureUs != null ? frame.image.exposureUs + " μs" : "未记录"}</KV><KV label="增益">{frame.image?.gainDb != null ? frame.image.gainDb + " dB" : "未记录"}</KV><KV label="原图更新">仅通过整圈重新采集</KV></Panel>
      <Notice title="保存与发布分开">中线保存进候选配方，生产配方不变；保存本帧后，先验证代表性样本，再发布生产版本。</Notice></div>
    </div></div>;
}
