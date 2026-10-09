import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Save, WandSparkles } from "lucide-react";
import { useCycle } from "../cycle/api";
import { shotLabel } from "../cycle/vis";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, FrameRail, GrayViewer, KV, Notice, NumberField, Panel, Steps, useGrayImage, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { FrameParams, WorkspaceView } from "./types";
import ImageImportButton from "./ImageImportButton";

export default function TeachingPage() {
  const { data, doc, dirty, busy, frameDrafts, setFrameParams, act, setError } = useWorkspace();
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
  const pending=useRef(false),serial=useRef(0);
  const scope=(doc?.id??"")+":"+k;
  const current=useRef({scope,alive:true,revision:data?.workspace.revision});current.current.scope=scope;current.current.revision=data?.workspace.revision;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;serial.current++;};},[]);
  useEffect(()=>{serial.current++;pending.current=false;setWorking(false);},[scope]);
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
  const params = frame ? (frameDrafts[k] ?? frame.params) : null;
  const {image,error,loading} = useGrayImage(doc?.id ?? null,frame?.image?.id ?? null);
  if (!data || !doc) return <WorkspaceEmpty />;
  if (!frame || !params) return <div className="wp-page"><WorkspaceBar /><Steps /><Notice title="当前胶路没有拍照点" tone="warn">请先完成拍照规划并保存候选。</Notice><Link className="btn" to="/recipe/geometry">胶路与拍照规划</Link></div>;
  const p = params;
  const editingParams = JSON.stringify(p) !== JSON.stringify(frame.params);
  const unlocked = !busy && !dirty && !working && !reading;
  const productionBusy = !!snapshot && !["IDLE","FAULT"].includes(snapshot.phase);
  const update = (key: keyof FrameParams, value: number) => {if(unlocked)setFrameParams(k,{ ...p, [key]:value });};
  const paramsError=![p.dx,p.dy,p.deg,p.mmPerPx,p.searchMm,p.minContrast,p.minScore].every(Number.isFinite)||p.mmPerPx<=0||p.searchMm<=0||p.minContrast<0||p.minContrast>255||p.minScore<0||p.minScore>1?"像素当量、搜索余量、对比度或定位分数无效":"";
  const rectFormatInvalid=!p.rect.every(v=>Number.isSafeInteger(v)&&v>=0&&v<=0xffffffff);
  const rectError=rectFormatInvalid||p.rect[2]<16||p.rect[3]<16||!!frame.image&&(p.rect[0]+p.rect[2]>frame.image.size[0]||p.rect[1]+p.rect[3]>frame.image.size[1])?"请在冻结图像内框选至少 16×16 px 的定位模板":"";
  const trialCurrent = !editingParams && !paramsError && !rectError && frame.trial?.passed && frame.trial.imageId === frame.image?.id && frame.trial.geometryTag===frame.image?.geometryTag;
  const staleTrial=!!frame.trial?.passed&&!trialCurrent;
  const run = () => unlocked&&!paramsError&&!rectError&&frame.image && perform(() => workspaceApi.trial(doc.id,data.workspace.revision,k,frame.image!.id,p), "当前冻结图像的试测已完成");
  const save = async (next = false) => {
    if (!unlocked||!trialCurrent||!frame.image||pending.current) return;
    if(frame.saved){if(next)setK(Math.min(k+1,frameCount-1));return;}
    const saved = await perform(() => workspaceApi.saveTeach(doc.id,data.workspace.revision,k,frame.image!.id,p), "本帧示教已保存，发布前仍需整体验证");
    if (saved?.workspace.doc.id===doc.id && next && current.current.scope===scope && current.current.alive) setK(Math.min(k+1,Math.max(0,saved.workspace.frames.length-1)));
  };
  const restore=async()=>{
    if(!unlocked||!frame.backup)return;
    const restored=await perform(()=>workspaceApi.restoreTeach(doc.id,data.workspace.revision,k),"原始图像与参数已恢复，请重新试测");
    const restoredFrame=restored?.workspace.frames[k];
    if(restored?.workspace.doc.id===doc.id&&restoredFrame?.k===k&&current.current.alive&&current.current.scope===scope)setFrameParams(k,restoredFrame.params);
  };
  return <div className="wp-page"><WorkspaceBar /><Steps />
    <div className="wp-actions"><Badge tone={frame.saved&&trialCurrent?"ok":trialCurrent?"info":"neutral"}>{editingParams ? "参数待试测" : staleTrial?"试测已过期":frame.saved ? "本帧已保存" : frame.trial ? frame.trial.passed ? "试测通过" : "试测未通过" : frame.image ? "待试测" : "待取样"}</Badge><span className="muted">已保存 {data.workspace.frames.filter(f => f.saved).length} / {data.workspace.frames.length} 帧</span><span className="spacer" /><Link className="btn" to="/recipe/overview">布置总览</Link></div>
    <div className="wp-teach">
      <Panel title={shotLabel(data.layout.shots[k],k)} detail="每帧独立绑定图像与参数"><FrameRail id={doc.id} frames={data.workspace.frames} selected={k} onSelect={setK} disabled={busy||working||reading} /></Panel>
      <div className="wp-stack"><Panel title={"k" + (k+1) + " · 单帧图像"} detail="冻结样本 → 试测 → 保存本帧" actions={<div className="wp-actions"><ImageImportButton scope={doc.id+":"+data.workspace.revision+":"+k} disabled={!unlocked||productionBusy}
        onReadingChange={setReading} onImport={bytes=>perform(()=>workspaceApi.importImage(doc.id,data.workspace.revision,k,bytes),"离线原图已绑定本帧，请重新试测")} onError={setError}/><button className="btn" disabled={!unlocked || productionBusy} onClick={() => void perform(() => workspaceApi.capture(doc.id,data.workspace.revision,k), "已冻结新的完整图像")}><Save size={15} />{busy||working ? "处理中…" : "取新样本"}</button></div>}>
        <GrayViewer image={image} loading={loading} error={error} label={frame.image ? "冻结图像 · " + frame.image.id : "等待取样"} params={p} layout={data.layout} k={k} onRect={unlocked ? rect => setFrameParams(k,{...p,rect}) : undefined} />
        {productionBusy && <p className="muted hint">工件正在检测，结束后可取示教样本。</p>}
        {frame.image && <div className="wp-kv"><span>取样来源</span><strong>{frame.image.source} · {new Date(frame.image.capturedAt).toLocaleString("zh-CN")}</strong></div>}
      </Panel>
      <Panel title="本帧试测结果" actions={frame.trial && <Badge tone={trialCurrent?"ok":"warn"}>{staleTrial?"已过期":frame.trial.passed ? "通过" : "未通过"}</Badge>}>
        <div className="wp-actions"><button className="btn" disabled={!unlocked || !frame.image || !!paramsError || !!rectError} onClick={() => void run()}><WandSparkles size={15} />试测当前帧</button><button className="btn primary" disabled={!unlocked || !trialCurrent || frame.saved} onClick={() => void save()}><Save size={15} />保存本帧示教</button><button className="btn" disabled={!unlocked || !trialCurrent} onClick={() => void save(true)}>保存并示教下一帧</button></div>
        <div className="wp-rule-stat"><div><span>定位分数</span><strong>{frame.trial?.score.toFixed(2) ?? "—"}</strong></div><div><span>测量覆盖</span><strong>{frame.trial ? (frame.trial.coverage * 100).toFixed(1) + "%" : "—"}</strong></div><div><span>处理耗时</span><strong>{frame.trial ? frame.trial.elapsedMs + " ms" : "—"}</strong></div></div>
        <p className="muted">{editingParams ? "参数已变更，现有试测不用于保存或发布。" : staleTrial?"现有试测已过期，请重新试测当前帧。":frame.trial?.reason ?? "在原图上框选模板，再试测当前帧。"}</p>
        {(paramsError||rectError)&&<Notice title="本帧参数无效" tone="warn">{paramsError||rectError}</Notice>}
      </Panel></div>
      <div className="wp-stack"><Panel title="本帧参数" detail="修改后需要重新试测"><div className="wp-form-grid">
        <NumberField label="搜索余量" value={p.searchMm} unit="mm" min={.1} step={.1} onChange={v => update("searchMm",v)} disabled={!unlocked} />
        <NumberField label="最低灰度对比" value={p.minContrast} unit="级" min={0} max={255} onChange={v => update("minContrast",v)} disabled={!unlocked} />
        <NumberField label="最低定位分数" value={p.minScore} min={0} max={1} step={.01} onChange={v => update("minScore",v)} disabled={!unlocked} />
        <NumberField label="像素当量" value={p.mmPerPx} unit="mm/px" min={.0001} step={.0005} onChange={v => update("mmPerPx",v)} disabled={!unlocked} />
        <NumberField label="平移 X" value={p.dx} unit="px" step={1} onChange={v => update("dx",v)} disabled={!unlocked} />
        <NumberField label="平移 Y" value={p.dy} unit="px" step={1} onChange={v => update("dy",v)} disabled={!unlocked} />
        <NumberField label="旋转角度" value={p.deg} unit="°" step={.1} onChange={v => update("deg",v)} disabled={!unlocked} />
      </div><button className="btn" style={{marginTop:14}} disabled={!unlocked || !editingParams || !!paramsError || rectFormatInvalid} onClick={() => void perform(() => workspaceApi.saveParams(doc.id,data.workspace.revision,k,p), "参数草稿已保存，请重新试测")}>保存参数草稿</button></Panel>
      <Panel title="样本绑定"><KV label="工作帧">k{k+1}</KV><KV label="图像">{frame.image?.id ?? "未冻结"}</KV><KV label="曝光">{frame.image?.exposureUs != null ? frame.image.exposureUs + " μs" : "未记录"}</KV><KV label="增益">{frame.image?.gainDb != null ? frame.image.gainDb + " dB" : "未记录"}</KV><KV label="原始示教备份">{frame.backup ? "可恢复" : "无"}</KV>{frame.backup && <button className="btn" style={{marginTop:14}} disabled={!unlocked} onClick={() => void restore()}>恢复原始示教</button>}</Panel>
      <Notice title="保存与发布分开">保存本帧后，先验证代表性样本，再发布生产版本。</Notice></div>
    </div></div>;
}
