import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Eraser, Save, Trash2, Undo2, WandSparkles } from "lucide-react";
import { useCycle } from "../cycle/api";
import { pathLength, shotLabel, shotSegment } from "../cycle/vis";
import type { DetectParams, Polarity } from "../cycle/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, FrameRail, GrayViewer, KV, Notice, NumberField, Panel, Steps, useGrayImage, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { ShotTeach, WorkspaceView } from "./types";
import ImageImportButton from "./ImageImportButton";
import { sameTeach, shotTeach, teachError } from "./teach";

export default function TeachingPage() {
  const { data, doc, dirty, busy, frameDrafts, setFrameDraft, act, setError } = useWorkspace();
  const {snapshot} = useCycle();
  const [search] = useSearchParams();
  const requested=Number(search.get("frame"));
  const requestedFrame=Number.isSafeInteger(requested)&&requested>=0?requested:0;
  const frameCount=data?.workspace.frames.length??0;
  const [selected, setK] = useState(requestedFrame);
  const k=Math.min(selected,Math.max(0,frameCount-1));
  useEffect(() => { setK(Math.min(requestedFrame,Math.max(0,frameCount-1))); }, [doc?.id, search]);
  useEffect(()=>{setK(previous=>Math.min(previous,Math.max(0,frameCount-1)));},[frameCount]);
  const [working,setWorking]=useState(false),[reading,setReading]=useState(false);
  const [vertex,setVertex]=useState<number|null>(null);
  const pending=useRef(false),serial=useRef(0);
  const scope=(doc?.id??"")+":"+k;
  const current=useRef({scope,alive:true,revision:data?.workspace.revision});current.current.scope=scope;current.current.revision=data?.workspace.revision;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;serial.current++;};},[]);
  useEffect(()=>{serial.current++;pending.current=false;setWorking(false);setVertex(null);},[scope]);
  const perform=async(request:()=>Promise<WorkspaceView>,message:string)=>{
    if(pending.current||busy||dirty)return null;
    pending.current=true;setWorking(true);
    const action=++serial.current;
    const valid=()=>current.current.alive&&current.current.scope===scope&&action===serial.current;
    const revision=data?.workspace.revision;
    try{const result=await act(request,message);return valid()&&result&&(current.current.revision===revision||current.current.revision===result.workspace.revision)?result:null;}
    catch(e){if(valid())setError(String(e));return null;}
    finally{if(valid()){pending.current=false;setWorking(false);}}
  };
  const frame = data?.workspace.frames[k];
  // 中线、像素当量与检测参数存在候选配方的拍照点里；草稿没保存前只在本页
  const shot = data?.workspace.doc.shots[k];
  const {image,error,loading} = useGrayImage(doc?.id ?? null,frame?.image?.id ?? null);
  if (!data || !doc) return <WorkspaceEmpty />;
  if (!frame || !shot) return <div className="wp-page"><WorkspaceBar /><Steps /><Notice title="当前配方没有拍照点" tone="warn">请先完成拍照点规划并保存候选。</Notice><Link className="btn" to="/recipe/geometry">拍照点规划</Link></div>;
  const saved = shotTeach(shot);
  const t = frameDrafts[k] ?? saved;
  const editing = !sameTeach(t, saved);
  const invalid = teachError(t);
  const taught = shot.path.length >= 2 && shot.mmPerPx != null;
  const unlocked = !busy && !dirty && !working && !reading;
  const productionBusy = !!snapshot && !["IDLE","FAULT"].includes(snapshot.phase);
  const update = (next: ShotTeach) => {if(unlocked)setFrameDraft(k,next);};
  const setPath = (path: [number, number][]) => update({ ...t, path });
  const setDetect = (detect: DetectParams | undefined) => { const next: ShotTeach = { path: t.path, mmPerPx: t.mmPerPx }; if (detect) next.detect = detect; update(next); };
  const trial = frame.trial;
  const staleTrial = !!trial && !editing && (trial.imageId !== frame.image?.id || trial.geometryTag !== frame.image?.geometryTag);
  const trialCurrent = !editing && !staleTrial && !!trial?.passed;
  const measured = !!trial && trial.measurement != null;
  const own = shotSegment(data.layout, k)?.segment;
  const stations: [number, number][] = own ? Array.from({ length: own.count }, (_, i) => [data.layout.points.x[own.first + i], data.layout.points.y[own.first + i]]) : [];
  const lengthPx = pathLength(t.path);
  const canTrial = unlocked && !!frame.image && !editing && taught && !shot.skip;
  const run = () => canTrial && frame.image && perform(() => workspaceApi.trial(doc.id,data.workspace.revision,k,frame.image!.id), "");
  const saveLine = () => {
    if (!unlocked || !editing || invalid) return;
    const params: ShotTeach = { path: t.path, mmPerPx: t.mmPerPx };
    if (t.detect) params.detect = t.detect;
    void perform(() => workspaceApi.saveParams(doc.id,data.workspace.revision,k,params), "中线已保存到候选配方，请重新试测");
  };
  const save = async (next = false) => {
    if (!unlocked||!trialCurrent||!frame.image||pending.current) return;
    if(frame.saved){if(next)setK(Math.min(k+1,frameCount-1));return;}
    const result = await perform(() => workspaceApi.saveTeach(doc.id,data.workspace.revision,k,frame.image!.id), "本帧示教已保存，发布前仍需整体验证");
    if (result?.workspace.doc.id===doc.id && next && current.current.scope===scope && current.current.alive) setK(Math.min(k+1,Math.max(0,result.workspace.frames.length-1)));
  };
  const restore=()=>{if(unlocked&&frame.backup)void perform(()=>workspaceApi.restoreTeach(doc.id,data.workspace.revision,k),"原始图像已恢复，请重新试测");};
  const pendingReason = !!trial && !trial.passed && /尚未接入/.test(trial.reason);
  const hint = shot.skip ? "这个拍照点设为不检：只要求这一帧到达，不量不判，不需要示教中线。"
    : editing ? "中线或参数已改，先保存中线再试测；现有试测不用于保存或发布。"
    : !taught ? "在原图上从胶嘴一侧往外依次点出胶路中线，填好像素当量后保存中线。"
    : staleTrial ? "现有试测已过期，请重新试测当前帧。"
    : trial ? trial.passed ? trial.reason : "试测没有通过，本帧示教不能保存。"
    : frame.image ? "中线已保存，可以试测当前帧。" : "先取样或导入这个拍照点的原图。";
  const detect = t.detect;
  const defaults = data.workspace.doc.detect;
  const polarity: Record<Polarity, string> = { dark: "暗胶条", light: "亮胶条" };
  return <div className="wp-page"><WorkspaceBar /><Steps />
    <div className="wp-actions"><Badge tone={shot.skip?"neutral":frame.saved&&trialCurrent?"ok":trialCurrent?"info":"neutral"}>{shot.skip?"不检":editing ? "中线待保存" : staleTrial?"试测已过期":frame.saved ? "本帧已保存" : trial ? trial.passed ? "试测通过" : "试测未通过" : !taught ? "待点中线" : frame.image ? "待试测" : "待取样"}</Badge><span className="muted">已保存 {data.workspace.frames.filter(f => f.saved).length} / {data.workspace.frames.filter(f => !data.workspace.doc.shots[f.k]?.skip).length} 帧（不检的不用示教）</span><span className="spacer" /><Link className="btn" to="/recipe/overview">布置总览</Link></div>
    <div className="wp-teach">
      <Panel title={shotLabel(shot,k)} detail={"胶条 " + shot.bead + " · Pose " + shot.poseId}><FrameRail id={doc.id} frames={data.workspace.frames} selected={k} onSelect={setK} disabled={busy||working||reading} /></Panel>
      <div className="wp-stack"><Panel title={"k" + (k+1) + " · 单帧图像"} detail="冻结原图 → 点出中线并保存 → 试测 → 保存本帧" actions={<div className="wp-actions"><ImageImportButton scope={doc.id+":"+data.workspace.revision+":"+k} disabled={!unlocked||productionBusy}
        onReadingChange={setReading} onImport={bytes=>perform(()=>workspaceApi.importImage(doc.id,data.workspace.revision,k,bytes),"离线原图已绑定本帧，请重新试测")} onError={setError}/><button className="btn" disabled={!unlocked || productionBusy} onClick={() => void perform(() => workspaceApi.capture(doc.id,data.workspace.revision,k), "已冻结新的完整图像")}><Save size={15} />{busy||working ? "处理中…" : "取新样本"}</button></div>}>
        <GrayViewer image={image} loading={loading} error={error} label={frame.image ? "冻结图像 · " + frame.image.id : "等待取样"}
          overlay={{ path: t.path, stations, stale: editing, selected: vertex }}
          onEdit={unlocked && !shot.skip ? { add: p => setPath([...t.path, p]), move: (i, p) => setPath(t.path.map((q, j) => j === i ? p : q)), select: setVertex } : undefined} />
        {productionBusy && <p className="muted hint">工件正在检测，结束后可取示教样本。</p>}
        <div className="wp-actions" style={{marginTop:10}}>
          <span className="muted">中线 {t.path.length} 点 · {Number.isFinite(t.mmPerPx) && t.mmPerPx > 0 ? (lengthPx * t.mmPerPx).toFixed(1) + " mm" : lengthPx.toFixed(0) + " px"}{own && !editing ? " · " + own.count + " 站" : ""}</span><span className="spacer" />
          <button className="btn small" disabled={!unlocked || shot.skip || !t.path.length} onClick={() => { setVertex(null); setPath(t.path.slice(0, -1)); }}><Undo2 size={14} />删除末点</button>
          <button className="btn small" disabled={!unlocked || shot.skip || vertex === null || vertex >= t.path.length} onClick={() => { const i = vertex; setVertex(null); setPath(t.path.filter((_, j) => j !== i)); }}><Trash2 size={14} />删除选中点</button>
          <button className="btn small" disabled={!unlocked || shot.skip || !t.path.length} onClick={() => { setVertex(null); setPath([]); }}><Eraser size={14} />清空中线</button>
        </div>
        {frame.image && <div className="wp-kv"><span>取样来源</span><strong>{frame.image.source} · {new Date(frame.image.capturedAt).toLocaleString("zh-CN")}</strong></div>}
      </Panel>
      <Panel title="本帧试测结果" actions={trial && <Badge tone={trialCurrent?"ok":"warn"}>{staleTrial?"已过期":trial.passed ? "通过" : "未通过"}</Badge>}>
        <div className="wp-actions"><button className="btn" disabled={!canTrial} onClick={() => void run()}><WandSparkles size={15} />试测当前帧</button><button className="btn primary" disabled={!unlocked || !trialCurrent || frame.saved} onClick={() => void save()}><Save size={15} />保存本帧示教</button><button className="btn" disabled={!unlocked || !trialCurrent} onClick={() => void save(true)}>保存并示教下一帧</button></div>
        <div className="wp-rule-stat"><div><span>得分</span><strong>{measured ? trial!.score.toFixed(2) : "—"}</strong></div><div><span>量成比例</span><strong>{measured ? (trial!.coverage * 100).toFixed(1) + "%" : "—"}</strong></div><div><span>处理耗时</span><strong>{trial ? trial.elapsedMs + " ms" : "—"}</strong></div></div>
        {trial && !trial.passed && !staleTrial && !editing && <Notice title={pendingReason ? "图像测量暂不可用" : "试测未通过"} tone="warn">{trial.reason}</Notice>}
        <p className="muted">{hint}</p>
      </Panel></div>
      <div className="wp-stack"><Panel title="中线与像素当量" detail="保存中线后需要重新试测"><div className="wp-form-grid">
        <NumberField label="像素当量" value={t.mmPerPx} unit="mm/px" min={.0001} step={.0005} onChange={v => update({ ...t, mmPerPx: v })} disabled={!unlocked || shot.skip} />
      </div>
        <label className="check" style={{marginTop:12}}><input type="checkbox" checked={!!detect} disabled={!unlocked || shot.skip} onChange={e => setDetect(e.target.checked ? structuredClone(defaults) : undefined)} />单独设检测参数</label>
        {detect ? <div className="wp-form-grid" style={{marginTop:8}}>
          <NumberField label="搜索半宽" value={detect.searchMm} unit="mm" min={.1} step={.5} onChange={v => setDetect({ ...detect, searchMm: v })} disabled={!unlocked} />
          <label className="field wp-field"><span>极性</span><select className="input" aria-label="极性" value={detect.polarity} disabled={!unlocked} onChange={e => setDetect({ ...detect, polarity: e.target.value as Polarity })}><option value="dark">暗胶条</option><option value="light">亮胶条</option></select></label>
          <NumberField label="胶宽下限" value={detect.widthRange[0]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [v, detect.widthRange[1]] })} disabled={!unlocked} />
          <NumberField label="胶宽上限" value={detect.widthRange[1]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [detect.widthRange[0], v] })} disabled={!unlocked} />
        </div> : <p className="muted hint">用配方的检测参数：搜索半宽 {defaults.searchMm} mm · {polarity[defaults.polarity]} · 胶宽 {defaults.widthRange[0]}–{defaults.widthRange[1]} mm</p>}
        {editing && invalid && <Notice title="本帧中线无效" tone="warn">{invalid}</Notice>}
        <button className="btn primary" style={{marginTop:14}} disabled={!unlocked || !editing || !!invalid} onClick={saveLine}><Save size={15} />保存中线</button></Panel>
      <Panel title="样本绑定"><KV label="工作帧">k{k+1}</KV><KV label="图像">{frame.image?.id ?? "未冻结"}</KV><KV label="曝光">{frame.image?.exposureUs != null ? frame.image.exposureUs + " μs" : "未记录"}</KV><KV label="增益">{frame.image?.gainDb != null ? frame.image.gainDb + " dB" : "未记录"}</KV><KV label="原始示教备份">{frame.backup ? "可恢复" : "无"}</KV>{frame.backup && <button className="btn" style={{marginTop:14}} disabled={!unlocked} onClick={restore}>恢复原始示教</button>}</Panel>
      <Notice title="保存与发布分开">中线保存进候选配方，生产配方不变；保存本帧后，先验证代表性样本，再发布生产版本。</Notice></div>
    </div></div>;
}
