import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { AlertTriangle, CheckCircle2, Upload } from "lucide-react";
import { displayReason, historyApi, verdictLabel } from "../history";
import { expectedChoices, expectedLabel, expectedValue, stepOf } from "./progress";
import { useSiteStatus } from "../cycle/api";
import { returnState } from "./returnNavigation";
import type { PartSummary } from "../history/types";
import type { Verdict } from "../cycle/types";
import Modal from "../plc/components/Modal";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, KV, Notice, Panel, Steps, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { Sample } from "./types";
import { canImportSampleGroup } from "./teach";
import { readImageBase64, validateImageFile } from "./ImageImportButton";

export default function ValidationPage() {
  const {data,doc,dirty,frameDirty,busy,act,setError} = useWorkspace();
  const location = useLocation();
  const [records,setRecords] = useState<PartSummary[]>([]);
  const [historyScope,setHistoryScope] = useState<"current"|"all">("current");
  const candidate = data ? JSON.stringify([data.workspace.doc.id,data.workspace.doc.version,data.workspace.baseRevision]) : null;
  const [selection,setSelection] = useState<{candidate:string|null;samples:Sample[]}>(()=>({candidate,samples:data?.workspace.samples??[]}));
  const selectedSamples = selection.candidate===candidate?selection.samples:data?.workspace.samples??[];
  const availableSamples = new Set(data?.workspace.sampleBank.map(sample=>sample.id));
  const samples = selectedSamples.filter(sample=>sample.sampleId==null||availableSamples.has(sample.sampleId));
  // Imports advance the revision before local selections are saved by validation.
  // Keep those edits within a candidate; discard missing images and old-candidate edits.
  if(selection.candidate!==candidate||samples.length!==selectedSamples.length)setSelection({candidate,samples});
  const [confirm,setConfirm] = useState(false);
  const [abandoning,setAbandoning] = useState(false);
  const site = useSiteStatus();
  const publishLabel = site?.mode === "offline" ? "发布离线回放配方" : site?.mode === "real" ? "发布生产配方" : "发布配方（模式确认中）";
  const recordScope = site?.mode === "offline" ? "offline" : "real";
  const [sourceConfirmed,setSourceConfirmed] = useState(false);
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
  useEffect(()=>{pending.current=null;setReading(false);setOperating(false);setConfirm(false);setAbandoning(false);setImporting(false);setFiles({});},[scope]);
  useEffect(()=>{
    setSourceConfirmed(false);
    setHistoryScope(data?.workspace.baseRevision?"current":"all");
  },[data?.workspace.doc.id,data?.workspace.baseRevision]);
  useEffect(()=>{
    setRecords([]);
    if(!doc)return;let alive=true;
    historyApi.query({recipeId:historyScope==="current"?doc.id:undefined,limit:50,scope:recordScope}).then(r=>alive&&setRecords(r.items)).catch(e=>alive&&setError(errorText(e)));
    return()=>{alive=false;};
  },[doc?.id,historyScope,recordScope,setError]);
  if(!data||!doc)return <WorkspaceEmpty/>;
  const validation=data.workspace.validation;
  const selectionDirty=JSON.stringify(samples)!==JSON.stringify(data.workspace.samples);
  const locked=busy||reading||operating;
  const samplesLocked=locked||!!data.workspace.pending;
  const ready=!dirty&&!frameDirty&&!locked;
  const canImportImages=canImportSampleGroup(doc.shots);
  const validationCurrent=!!validation?.passed&&validation.revision===data.workspace.revision&&!selectionDirty;
  const canPublish=ready&&validationCurrent&&!data.workspace.pending;
  const publish=data.workspace.publish;
  const publishStep=stepOf(data.progress,"publish");
  const checks=validation&&validation.revision===data.workspace.revision?validation.checks:(["capture","teach","calibration","validation"] as const).map(id=>{const step=stepOf(data.progress,id);return {name:step.label,passed:step.status==="done",detail:step.blockers.length?`${step.detail}；${step.blockers[0].message}`:step.detail};});
  const expectedOptions=expectedChoices.map(([v,l])=><option key={v} value={v}>{l}</option>);
  const choose=(key:Sample,enabled:boolean)=>{setSourceConfirmed(false);setSelection(previous=>({candidate,samples:enabled?[...previous.samples.filter(s=>!(s.historyId===key.historyId&&s.sampleId===key.sampleId)),key]:previous.samples.filter(s=>!(s.historyId===key.historyId&&s.sampleId===key.sampleId))}));};
  const action=async(request:()=>ReturnType<typeof workspaceApi.publish>,message:string)=>{
    if(pending.current||!ready)return null;
    const operation={scope,sequence:current.current.sequence};
    pending.current=operation;setOperating(true);
    const valid=()=>current.current.alive&&current.current.scope===scope&&current.current.sequence===operation.sequence&&pending.current===operation;
    try{const result=await act(request,message);return valid()?result:null;}
    catch(error){if(valid())setError(errorText(error));return null;}
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
      const images:{k:number;bytes:string}[]=[];
      for(const frame of data.workspace.frames){
        if(!valid())return;
        const bytes=await readImageBase64(files[frame.k]);
        if(!valid())return;
        images.push({k:frame.k,bytes});
      }
      if(!valid())return;
      const result=await act(()=>workspaceApi.importSample(doc.id,data.workspace.revision,sampleName.trim(),expected,images),"代表性原图样本组已保存");
      if(result&&valid()){setImporting(false);setFiles({});}
    }catch(e){if(valid())setError(errorText(e));}finally{if(valid()){pending.current=null;setReading(false);}}
  };
  return <div className="wp-page"><WorkspaceBar/><Steps/><div className="wp-columns"><div className="wp-stack"><Panel title="发布前检查" detail="设备、胶路示教、单帧示教和独立正常样本都需满足">
    <div className="wp-checklist">{checks.map(c=><div key={c.name} className={"wp-check-row "+(c.passed?"passed":"")}>{c.passed?<CheckCircle2 size={20}/>:<AlertTriangle size={20}/>}<div><strong>{c.name}</strong><p>{c.detail}</p></div></div>)}</div>
    </Panel><Panel title="代表性验证样本" detail="使用完整原图重新运行当前候选检测；正常样本必须独立于示教采集" actions={<div className="wp-actions"><Link className="btn" to="/recipe/capture?purpose=validation">重新实拍样本</Link><button className="btn" disabled={!ready || !!data.workspace.pending || !canImportImages} title={canImportImages?undefined:"有拍照点选中了多幅图，请改用独立整圈采集或历史原图"} onClick={()=>{setFiles({});setImporting(true);}}><Upload size={15}/>导入原图样本组</button></div>}>
      <label className="field"><span>历史样本来源</span><select className="input" aria-label="历史样本来源" value={historyScope} disabled={samplesLocked} onChange={e=>setHistoryScope(e.target.value as "current"|"all")}><option value="current">当前配方</option><option value="all">全部配方</option></select></label>
      <p className="muted hint">历史样本必须保留完整整圈原图，轨迹顺序与成像条件适用；验证时重新运行当前候选。</p>
      <div className="table-wrap"><table className="table"><thead><tr><th>选用</th><th>样本</th><th>来源</th><th>人工期望</th></tr></thead><tbody>
      {data.workspace.sampleBank.map(b=>{
        const selected=samples.find(s=>s.sampleId===b.id);
        return <tr key={b.id}><td><input type="checkbox" aria-label={"选用样本 "+b.name} checked={!!selected} disabled={samplesLocked} onChange={e=>choose({historyId:null,sampleId:b.id,expected:b.expected},e.target.checked)}/></td><td>{b.name}</td><td>整组原图</td><td><select className="input" aria-label={b.name+"期望结论"} value={expectedValue(selected?.expected??b.expected)} disabled={!selected||samplesLocked} onChange={e=>choose({historyId:null,sampleId:b.id,expected:e.target.value as Verdict},true)}>{expectedOptions}</select></td></tr>;
      })}
      {records.map(r=>{
        const selected=samples.find(s=>s.historyId===r.id);
        return <tr key={r.id}><td><input type="checkbox" aria-label={"选用历史 SN "+r.sn} checked={!!selected} disabled={samplesLocked} onChange={e=>choose({historyId:r.id,sampleId:null,expected:expectedValue(r.verdict)},e.target.checked)}/></td><td><Link to={"/history/"+r.id}>SN {r.sn}</Link></td><td>历史整圈 · {r.recipeId} · v{r.recipeVersion}</td><td><select className="input" aria-label={"SN "+r.sn+"期望结论"} value={expectedValue(selected?.expected??r.verdict)} disabled={!selected||samplesLocked} onChange={e=>choose({historyId:r.id,sampleId:null,expected:e.target.value as Verdict},true)}>{expectedOptions}</select></td></tr>;
      })}
      {!records.length&&!data.workspace.sampleBank.length&&<tr><td colSpan={4} className="muted center">尚无代表性样本；请重新实拍独立整圈，或选择完整历史原图。</td></tr>}
      </tbody></table></div><p className="muted hint">建议覆盖良品、断胶、偏位、胶宽异常和无法测量等现场情况。期望“合格”时，候选结论 OK 或 OK · 局部超差都算符合。修改人工期望后，需要重新验证。</p>
      <label className="check"><input type="checkbox" checked={sourceConfirmed} disabled={samplesLocked} onChange={e=>setSourceConfirmed(e.target.checked)}/>已确认所选样本的拍摄条件、轨迹和触发顺序适用于当前候选</label>
      <button className="btn primary" disabled={!ready||!!data.workspace.pending||!samples.length||!sourceConfirmed} onClick={()=>void action(()=>workspaceApi.validate(doc.id,data.workspace.revision,samples,sourceConfirmed),"当前候选的验证已完成")}>{operating||busy?"验证中…":"运行整套图像验证"}</button>
    </Panel>{validation&&<Panel title="验证结果" actions={<Badge tone={validationCurrent?"ok":"warn"}>{validationCurrent?"通过":"未通过或待重验"}</Badge>}><div className="table-wrap"><table className="table"><thead><tr><th>样本</th><th>人工期望</th><th>候选结论</th><th>结果</th></tr></thead><tbody>{validation.samples.map((s,i)=><tr key={i} title={displayReason(s.reason)}><td>{s.name}</td><td>{expectedLabel(s.expected)}</td><td>{s.actual?verdictLabel[s.actual]:"未量成"}</td><td className={s.passed?"c-ok":"c-warn"}>{s.passed?"符合":displayReason(s.reason)}</td></tr>)}</tbody></table></div>{selectionDirty&&<Notice title="样本选用或期望已改变" tone="warn">请重新运行验证；现有结果不用于发布。</Notice>}</Panel>}</div>
    <div className="wp-stack"><Panel title="已发布版本"><KV label="当前发布">{data.productionVersion?"v"+data.productionVersion:"未发布"}</KV><KV label="待发布候选">v{doc.version}</KV><KV label="候选修订">{data.workspace.revision}</KV><KV label="PLC 计划版本">{data.workspace.pending?.doc.planVersion?data.workspace.pending.doc.planVersion+"（待生效）":!data.planChanged&&data.productionPlanVersion?data.productionPlanVersion+"（不变）":"发布时分配新号"}</KV><KV label="发布状态">{publishStep.detail}</KV>{site?.mode==="offline"&&<Notice title="当前为离线模式" tone="warn">离线验证发布的版本只能在离线运行，真机需重新验证发布。</Notice>}<button className="btn primary" style={{marginTop:16,width:"100%"}} disabled={!canPublish} onClick={()=>setConfirm(true)}>{publishLabel}</button></Panel><Notice title="在工件边界生效">已开始检测的工件继续使用原配方快照。候选发布后，等待当前工件完成，再供新工件使用。</Notice>{(publish.state==="failed"||publish.state==="uncertain") && <button className="btn" disabled={!ready} onClick={() => void action(() => workspaceApi.retryPublish(doc.id,data.workspace.revision), "已重试发布 / 切换")}>重试发布 / 切换</button>}{publish.state==="failed" && <button className="btn" disabled={!ready} onClick={()=>setAbandoning(true)}>放弃待生效版本</button>}<Link className="btn" to="/inspect">查看在线检测</Link>{site?.mode === "offline" && site.source === "sampleSet" && <Notice title="候选配方离线试运行"><p>使用已保存候选跑一圈，结果标记为调试；不要求发布，也不替代独立样本验证。</p><Link className="btn" to={`/inspect?candidate=${encodeURIComponent(doc.id)}`}>前往在线检测调试候选</Link></Notice>}</div>
  </div>
  {confirm&&<Modal title={publishLabel} onClose={()=>!locked&&setConfirm(false)} footer={<><button className="btn" disabled={locked} onClick={()=>setConfirm(false)}>取消</button><button className="btn primary" disabled={!canPublish} onClick={async()=>{const r=await action(()=>workspaceApi.publish(doc.id,data.workspace.revision),"已提交发布，将在工件边界生效");if(r&&current.current.alive&&current.current.scope===scope)setConfirm(false);}}>确认发布 v{doc.version}</button></>}><p>将发布 <b>{doc.id} · v{doc.version}</b>，替换当前发布版本。{site?.mode==="offline"&&"该版本仅用于离线回放，不能用于真机生产。"}</p><p className="muted">发布内容包含当前已验证的拍照点、示教中线、判定规则、各帧示教和总览布置。等待期间候选已冻结，生效或完成失败处理后再继续编辑。</p>{data.planChanged?<Notice tone="warn" title="拍照计划已变，PLC 要同步">产品代码或拍照点的顺序、Pose、相机与生产不同，发布时分配新的计划版本。生效后 PLC 侧要同步 planVersion 与各相机拍照点数，否则拒绝布防（故障 100）。<Link className="btn small" to="/plc?panel=s7-plan" state={returnState(location,"验证与发布")}>查看 PLC 配方握手计划</Link></Notice>:<p className="muted">拍照计划未变，沿用计划版本 {data.productionPlanVersion}，PLC 不用改。</p>}</Modal>}
  {abandoning&&<Modal title="放弃待生效版本" onClose={()=>!locked&&setAbandoning(false)} footer={<><button className="btn" disabled={locked} onClick={()=>setAbandoning(false)}>取消</button><button className="btn primary" disabled={!ready||publish.state!=="failed"} onClick={async()=>{const r=await action(()=>workspaceApi.abandonPublish(doc.id,data.workspace.revision),"已放弃待生效版本，候选回到草稿");if(r&&current.current.alive&&current.current.scope===scope)setAbandoning(false);}}>确认放弃 v{data.workspace.pending?.doc.version??doc.version}</button></>}><p>放弃后候选回到草稿，可修改后重新验证发布；当前生产版本不受影响。</p></Modal>}
  {importing&&<Modal title="导入代表性原图样本组" width={650} onClose={()=>!locked&&setImporting(false)} footer={<><button className="btn" disabled={locked} onClick={()=>setImporting(false)}>取消</button><button className="btn primary" disabled={!ready||!sampleName.trim()||!data.workspace.frames.length||data.workspace.frames.some(f=>!files[f.k])} onClick={()=>void importSample()}>{reading?"正在导入…":"保存样本组"}</button></>}><div className="wp-stack"><div className="wp-form-grid"><label className="field"><span>样本名称</span><input className="input" aria-label="样本名称" disabled={locked} value={sampleName} onChange={e=>setSampleName(e.target.value)}/></label><label className="field"><span>人工确认的期望结论</span><select className="input" aria-label="人工确认的期望结论" disabled={locked} value={expected} onChange={e=>setExpected(e.target.value as Verdict)}>{expectedOptions}</select></label></div>{data.workspace.frames.map(f=><label className="field" key={f.k}><span>k{f.k+1} · {files[f.k]?.name??"选择该拍照点的原始图像"}</span><input className="input" aria-label={"k"+(f.k+1)+" 原图"} type="file" accept=".png,.jpg,.jpeg,.pgm,.bmp,.tif,.tiff" disabled={locked} onChange={e=>{const file=e.target.files?.[0];e.target.value="";if(file){try{validateImageFile(file);setFiles(previous=>({...previous,[f.k]:file}));}catch(error){setFiles(previous=>{const next={...previous};delete next[f.k];return next;});setError(errorText(error));}}}}/></label>)}<Notice title="整组图像需对应同一件工件">每个拍照点一张原图，图像尺寸和工位应与当前示教一致；单图不超过 15 MB，整组不超过 80 MB。期望结论由现场人员标注。</Notice></div></Modal>}
  </div>;
}
