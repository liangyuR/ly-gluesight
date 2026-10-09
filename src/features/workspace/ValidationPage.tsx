import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { AlertTriangle, CheckCircle2, Upload } from "lucide-react";
import { displayReason, historyApi, verdictLabel } from "../history";
import type { PartSummary } from "../history/types";
import type { Verdict } from "../cycle/types";
import Modal from "../plc/components/Modal";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, KV, Notice, Panel, Steps, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { Sample } from "./types";
import { readImageFile, validateImageFile } from "./ImageImportButton";

export default function ValidationPage() {
  const {data,doc,dirty,frameDirty,busy,act,setError} = useWorkspace();
  const [records,setRecords] = useState<PartSummary[]>([]);
  const [historyScope,setHistoryScope] = useState<"current"|"all">("current");
  const candidate = data ? JSON.stringify([data.workspace.doc.id,data.workspace.doc.version,data.workspace.baseHash]) : null;
  const [selection,setSelection] = useState<{candidate:string|null;samples:Sample[]}>(()=>({candidate,samples:data?.workspace.samples??[]}));
  const selectedSamples = selection.candidate===candidate?selection.samples:data?.workspace.samples??[];
  const availableSamples = new Set(data?.workspace.sampleBank.map(sample=>sample.id));
  const samples = selectedSamples.filter(sample=>sample.sampleId==null||availableSamples.has(sample.sampleId));
  // Imports advance the revision before local selections are saved by validation.
  // Keep those edits within a candidate; discard missing images and old-candidate edits.
  if(selection.candidate!==candidate||samples.length!==selectedSamples.length)setSelection({candidate,samples});
  const [confirm,setConfirm] = useState(false);
  const [importing,setImporting] = useState(false);
  const [sampleName,setSampleName] = useState("代表性样本");
  const [expected,setExpected] = useState<Verdict>("OK");
  const [files,setFiles] = useState<Record<number,File>>({});
  const [reading,setReading] = useState(false);
  const [operating,setOperating] = useState(false);
  const pending = useRef<{scope:string;sequence:number}|null>(null);
  const scope = `${candidate??""}:${data?.workspace.revision??""}`;
  const current = useRef({scope,alive:true,sequence:0});
  if(current.current.scope!==scope){current.current.scope=scope;current.current.sequence++;}
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;};},[]);
  useEffect(()=>{pending.current=null;setReading(false);setOperating(false);setConfirm(false);setImporting(false);setFiles({});},[scope]);
  useEffect(()=>{
    setHistoryScope(data?.workspace.baseHash?"current":"all");
  },[data?.workspace.doc.id,data?.workspace.baseHash]);
  useEffect(()=>{
    setRecords([]);
    if(!doc)return;let alive=true;
    historyApi.query({recipeId:historyScope==="current"?doc.id:undefined,limit:50}).then(r=>alive&&setRecords(r.items)).catch(e=>alive&&setError(String(e)));
    return()=>{alive=false;};
  },[doc?.id,historyScope,setError]);
  if(!data||!doc)return <WorkspaceEmpty/>;
  const validation=data.workspace.validation;
  const measuredFrames=data.workspace.frames.filter(f=>!data.workspace.doc.shots[f.k]?.skip);
  const selectionDirty=JSON.stringify(samples)!==JSON.stringify(data.workspace.samples);
  const locked=busy||reading||operating;
  const ready=!dirty&&!frameDirty&&!locked;
  const validationCurrent=!!validation?.passed&&validation.revision===data.workspace.revision&&!selectionDirty;
  const canPublish=ready&&validationCurrent&&!data.workspace.pending;
  const choose=(key:Sample,enabled:boolean)=>setSelection(previous=>({candidate,samples:enabled?[...previous.samples.filter(s=>!(s.historyId===key.historyId&&s.sampleId===key.sampleId)),key]:previous.samples.filter(s=>!(s.historyId===key.historyId&&s.sampleId===key.sampleId))}));
  const action=async(request:()=>ReturnType<typeof workspaceApi.publish>,message:string)=>{
    if(pending.current||!ready)return null;
    const operation={scope,sequence:current.current.sequence};
    pending.current=operation;setOperating(true);
    const valid=()=>current.current.alive&&current.current.scope===scope&&current.current.sequence===operation.sequence&&pending.current===operation;
    try{const result=await act(request,message);return valid()?result:null;}
    catch(error){if(valid())setError(String(error));return null;}
    finally{if(valid()){pending.current=null;setOperating(false);}}
  };
  const importSample=async()=>{
    if(pending.current||!ready||!sampleName.trim()||!data.workspace.frames.length||data.workspace.frames.some(f=>!files[f.k]))return;
    if(data.workspace.frames.reduce((size,f)=>size+files[f.k].size,0)>80_000_000){setError("整组原图不得超过 80 MB");return;}
    const operation={scope,sequence:current.current.sequence};
    pending.current=operation;
    setReading(true);
    const valid=()=>current.current.alive&&current.current.scope===scope&&current.current.sequence===operation.sequence&&pending.current===operation;
    try{
      const images=await Promise.all(data.workspace.frames.map(async f=>({k:f.k,bytes:await readImageFile(files[f.k])})));
      if(!valid())return;
      const result=await act(()=>workspaceApi.importSample(doc.id,data.workspace.revision,sampleName.trim(),expected,images),"代表性原图样本组已保存");
      if(result&&valid()){setImporting(false);setFiles({});}
    }catch(e){if(valid())setError(String(e));}finally{if(valid()){pending.current=null;setReading(false);}}
  };
  return <div className="wp-page"><WorkspaceBar/><Steps/><div className="wp-columns"><div className="wp-stack"><Panel title="发布前检查" detail="设备、胶路示教、单帧示教、总览和代表性样本都需满足">
    <div className="wp-checklist">{(validation?.checks??[
      {name:"设备与采集",passed:false,detail:"验证时读取实际相机状态与采集方式"},
      {name:"胶路示教",passed:data.coverage>=99.995,detail:"要检的拍照点里已示教中线 "+data.coverage.toFixed(0)+"%"},
      {name:"示教与标定",passed:measuredFrames.every(f=>f.saved),detail:"已保存 "+measuredFrames.filter(f=>f.saved).length+"/"+measuredFrames.length+" 帧（不检的拍照点不用示教）"},
      {name:"代表性样本",passed:false,detail:"至少选择一件合格样本和一件缺陷样本"},
    ]).map(c=><div key={c.name} className={"wp-check-row "+(c.passed?"passed":"")}>{c.passed?<CheckCircle2 size={20}/>:<AlertTriangle size={20}/>}<div><strong>{c.name}</strong><p>{c.detail}</p></div></div>)}</div>
    </Panel><Panel title="代表性验证样本" detail="历史样本按存储的测量数据验证规则；导入原图样本组重新运行图像测量" actions={<button className="btn" disabled={!ready} onClick={()=>{setFiles({});setImporting(true);}}><Upload size={15}/>导入原图样本组</button>}>
      <label className="field"><span>历史样本来源</span><select className="input" aria-label="历史样本来源" value={historyScope} disabled={locked} onChange={e=>setHistoryScope(e.target.value as "current"|"all")}><option value="current">当前配方</option><option value="all">全部配方</option></select></label>
      <p className="muted hint">仅测点布局与当前候选一致的完整历史测量可用于规则验证；选择其他配方的样本时，验证会再次检查兼容性。</p>
      <div className="table-wrap"><table className="table"><thead><tr><th>选用</th><th>样本</th><th>来源</th><th>人工期望</th></tr></thead><tbody>
      {data.workspace.sampleBank.map(b=>{
        const selected=samples.find(s=>s.sampleId===b.id);
        return <tr key={b.id}><td><input type="checkbox" aria-label={"选用样本 "+b.name} checked={!!selected} disabled={locked} onChange={e=>choose({historyId:null,sampleId:b.id,expected:b.expected},e.target.checked)}/></td><td>{b.name}</td><td>整组原图</td><td><select className="input" aria-label={b.name+"期望结论"} value={selected?.expected??b.expected} disabled={!selected||locked} onChange={e=>choose({historyId:null,sampleId:b.id,expected:e.target.value as Verdict},true)}>{Object.entries(verdictLabel).map(([v,l])=><option key={v} value={v}>{l}</option>)}</select></td></tr>;
      })}
      {records.map(r=>{
        const selected=samples.find(s=>s.historyId===r.id);
        return <tr key={r.id}><td><input type="checkbox" aria-label={"选用历史 SN "+r.sn} checked={!!selected} disabled={locked} onChange={e=>choose({historyId:r.id,sampleId:null,expected:r.verdict},e.target.checked)}/></td><td><Link to={"/history/"+r.id}>SN {r.sn}</Link></td><td>历史测量 · {r.recipeId} · v{r.recipeVersion}</td><td><select className="input" aria-label={"SN "+r.sn+"期望结论"} value={selected?.expected??r.verdict} disabled={!selected||locked} onChange={e=>choose({historyId:r.id,sampleId:null,expected:e.target.value as Verdict},true)}>{Object.entries(verdictLabel).map(([v,l])=><option key={v} value={v}>{l}</option>)}</select></td></tr>;
      })}
      {!records.length&&!data.workspace.sampleBank.length&&<tr><td colSpan={4} className="muted center">尚无代表性样本；可导入各拍照点的原图建立完整样本组。</td></tr>}
      </tbody></table></div><p className="muted hint">建议覆盖良品、断胶、偏位、胶宽异常和无法测量等现场情况。修改人工期望后，需要重新验证。</p>
      <button className="btn primary" disabled={!ready||!samples.length} onClick={()=>void action(()=>workspaceApi.validate(doc.id,data.workspace.revision,samples),"当前候选的验证已完成")}>{operating||busy?"验证中…":"运行规则与图像验证"}</button>
    </Panel>{validation&&<Panel title="验证结果" actions={<Badge tone={validationCurrent?"ok":"warn"}>{validationCurrent?"通过":"未通过或待重验"}</Badge>}><div className="table-wrap"><table className="table"><thead><tr><th>样本</th><th>人工期望</th><th>候选结论</th><th>结果</th></tr></thead><tbody>{validation.samples.map((s,i)=><tr key={i} title={displayReason(s.reason)}><td>{s.name}</td><td>{verdictLabel[s.expected]}</td><td>{s.actual?verdictLabel[s.actual]:"未量成"}</td><td className={s.passed?"c-ok":"c-warn"}>{s.passed?"一致":displayReason(s.reason)}</td></tr>)}</tbody></table></div>{selectionDirty&&<Notice title="样本选用或期望已改变" tone="warn">请重新运行验证；现有结果不用于发布。</Notice>}</Panel>}</div>
    <div className="wp-stack"><Panel title="生产版本"><KV label="当前生产">{data.productionVersion?"v"+data.productionVersion:"未发布"}</KV><KV label="待发布候选">v{doc.version}</KV><KV label="候选修订">{data.workspace.revision}</KV><KV label="发布状态">{data.workspace.pending?"等待工件边界":canPublish?"可发布":"待验证"}</KV><button className="btn primary" style={{marginTop:16,width:"100%"}} disabled={!canPublish} onClick={()=>setConfirm(true)}>发布生产配方</button></Panel><Notice title="在工件边界生效">已开始检测的工件继续使用原配方快照。候选发布后，等待当前工件完成，再供新工件使用。</Notice><Link className="btn" to="/inspect">查看在线检测</Link></div>
  </div>
  {confirm&&<Modal title="发布生产配方" onClose={()=>!locked&&setConfirm(false)} footer={<><button className="btn" disabled={locked} onClick={()=>setConfirm(false)}>取消</button><button className="btn primary" disabled={!canPublish} onClick={async()=>{const r=await action(()=>workspaceApi.publish(doc.id,data.workspace.revision),"已提交发布，将在工件边界生效");if(r&&current.current.alive&&current.current.scope===scope)setConfirm(false);}}>确认发布 v{doc.version}</button></>}><p>将发布 <b>{doc.id} · v{doc.version}</b>，替换当前生产版本。</p><p className="muted">发布内容包含当前已验证的拍照点、示教中线、判定规则、各帧示教和总览布置。等待期间修改候选，不会改变已提交的发布快照。</p></Modal>}
  {importing&&<Modal title="导入代表性原图样本组" width={650} onClose={()=>!locked&&setImporting(false)} footer={<><button className="btn" disabled={locked} onClick={()=>setImporting(false)}>取消</button><button className="btn primary" disabled={!ready||!sampleName.trim()||!data.workspace.frames.length||data.workspace.frames.some(f=>!files[f.k])} onClick={()=>void importSample()}>{reading?"正在导入…":"保存样本组"}</button></>}><div className="wp-stack"><div className="wp-form-grid"><label className="field"><span>样本名称</span><input className="input" aria-label="样本名称" disabled={locked} value={sampleName} onChange={e=>setSampleName(e.target.value)}/></label><label className="field"><span>人工确认的期望结论</span><select className="input" aria-label="人工确认的期望结论" disabled={locked} value={expected} onChange={e=>setExpected(e.target.value as Verdict)}>{Object.entries(verdictLabel).map(([v,l])=><option key={v} value={v}>{l}</option>)}</select></label></div>{data.workspace.frames.map(f=><label className="field" key={f.k}><span>k{f.k+1} · {files[f.k]?.name??"选择该拍照点的原始图像"}</span><input className="input" aria-label={"k"+(f.k+1)+" 原图"} type="file" accept=".png,.jpg,.jpeg,.pgm,.bmp,.tif,.tiff" disabled={locked} onChange={e=>{const file=e.target.files?.[0];e.target.value="";if(file){try{validateImageFile(file);setFiles(previous=>({...previous,[f.k]:file}));}catch(error){setFiles(previous=>{const next={...previous};delete next[f.k];return next;});setError(String(error));}}}}/></label>)}<Notice title="整组图像需对应同一件工件">每个拍照点一张原图，图像尺寸和工位应与当前示教一致；单图不超过 15 MB，整组不超过 80 MB。期望结论由现场人员标注。</Notice></div></Modal>}
  </div>;
}
