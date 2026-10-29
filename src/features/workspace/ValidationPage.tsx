import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { AlertTriangle, CheckCircle2 } from "lucide-react";
import { Accordion, Button, Checkbox, Dialog, Select, Table, type TableColumn } from "qing-steel";
import { scrollBehavior } from "./motion";
import "./validation.css";
import { displayReason, historyApi, verdictLabel } from "../history";
import { expectedChoices, expectedLabel, expectedValue, stepOfV2 } from "./progress";
import { expectedText } from "../plan/vis";
import { pendingSegmentsNote, planPublishBlocker } from "./planPublish";
import { useSiteStatus } from "../cycle/api";
import type { PartSummary } from "../history/types";
import type { Verdict } from "../cycle/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, KV, Notice, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";
import { PlanPublishBody } from "./PlcParams";
import type { Check, Sample } from "./types";
import { NavigationPublication } from "./WorkspaceNavigation";

const expectedOptions=expectedChoices.map(([value,label])=>({value,label}));
interface SampleRow { key:string; sample:Sample; name:ReactNode; source:string; checkLabel:string; expectedLabel:string; selected:boolean; expected:Verdict }
type ResultRow = { key:string; name:string; expected:Verdict; actual:Verdict|null; passed:boolean; reason:string };
const resultColumns:TableColumn<ResultRow>[]=[
  {key:"name",header:"样本",render:s=>s.name},
  {key:"expected",header:"人工期望",render:s=>expectedLabel(s.expected)},
  {key:"actual",header:"候选结论",render:s=>s.actual?verdictLabel[s.actual]:"未量成"},
  {key:"result",header:"结果",render:s=><span className={s.passed?"c-ok":"c-warn"}>{s.passed?"符合":displayReason(s.reason)}</span>},
];

export default function ValidationPage() {
  const {data,doc,dirty,frameDirty,navigationDirty,busy,act,setError} = useWorkspace();
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
  const [collect,setCollect] = useState<Check[]|null>(null);
  const site = useSiteStatus();
  const publishLabel = site?.mode === "offline" ? "发布离线回放配方" : site?.mode === "real" ? "发布生产配方" : "发布配方（模式确认中）";
  const recordScope = site?.mode === "offline" ? "offline" : "real";
  const [sourceConfirmed,setSourceConfirmed] = useState(false);
  const [publishChecks,setPublishChecks] = useState<{key:string;checks:Check[]|null;error:string}>({key:"",checks:null,error:""});
  const [operating,setOperating] = useState(false);
  const pending = useRef<{scope:string;sequence:number}|null>(null);
  const resultRef = useRef<HTMLDivElement>(null);
  const scope = `${candidate??""}:${data?.workspace.revision??""}`;
  const current = useRef({scope,alive:true,sequence:0});
  if(current.current.scope!==scope){current.current.scope=scope;current.current.sequence++;}
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;};},[]);
  useEffect(()=>{pending.current=null;setOperating(false);setConfirm(false);setAbandoning(false);setCollect(null);},[scope]);
  useEffect(()=>{
    setSourceConfirmed(false);
    setHistoryScope(data?.workspace.baseRevision?"current":"all");
  },[data?.workspace.doc.id,data?.workspace.baseRevision]);
  // 发布前检查（D-SB-46）：每次候选修订或待生效状态变化时当场核对一次
  const checksKey=JSON.stringify([data?.workspace.doc.id??null,data?.workspace.revision??null,!!data?.workspace.pending]);
  useEffect(()=>{
    if(!data||!data.plan)return;let alive=true;
    workspaceApi.publishChecks(data.workspace.doc.id,data.workspace.revision)
      .then(checks=>alive&&setPublishChecks({key:checksKey,checks,error:""}))
      .catch(e=>alive&&setPublishChecks({key:checksKey,checks:null,error:errorText(e)}));
    return()=>{alive=false;};
  },[checksKey]);
  useEffect(()=>{
    setRecords([]);
    if(!doc)return;let alive=true;
    historyApi.query({recipeId:historyScope==="current"?doc.id:undefined,limit:50,scope:recordScope}).then(r=>alive&&setRecords(r.items)).catch(e=>alive&&setError(errorText(e)));
    return()=>{alive=false;};
  },[doc?.id,historyScope,recordScope,setError]);
  if(!data||!doc)return <WorkspaceEmpty/>;
  const plan=data.plan;
  const validation=data.workspace.validation;
  const selectionDirty=JSON.stringify(samples)!==JSON.stringify(data.workspace.samples);
  const locked=busy||operating;
  const samplesLocked=locked||!!data.workspace.pending;
  const ready=!dirty&&!frameDirty&&!navigationDirty&&!locked;
  const validationCurrent=!!validation?.passed&&validation.revision===data.workspace.revision&&!selectionDirty;
  // 整件 ΣN_exp（多段时各段 N_exp）未标定时不能发布
  const planBlocked=planPublishBlocker(plan);
  const liveChecks=publishChecks.key===checksKey?publishChecks:{checks:null,error:""};
  const failedLive=liveChecks.checks?.find(c=>!c.passed);
  const canPublish=ready&&!!liveChecks.checks&&!failedLive&&!data.workspace.pending&&!planBlocked;
  const publish=data.workspace.publish;
  const publishStep=stepOfV2(plan.progress,"publish");
  const stepCheck=(step:{label:string;status:string;detail:string;blockers:{message:string}[]})=>({name:step.label,passed:step.status==="done",detail:step.blockers.length?`${step.detail}；${step.blockers[0].message}`:step.detail});
  const stepChecks=(["capture","segments"] as const).flatMap(id=>{const step=stepOfV2(plan.progress,id);return step?[stepCheck(step)]:[];});
  // 整件预期帧数 ΣN_exp 是否已标定：由 DTO 得出，不依赖后端步骤文案
  const planCheck=[{name:"预期总帧数 ΣN_exp",passed:!planBlocked,detail:planBlocked||(plan.calibrated?`已标定 ${expectedText(plan.expectedTotal,plan.toleranceTotal)} 帧${pendingSegmentsNote(plan)?"；"+pendingSegmentsNote(plan):""}`:"")}];
  const liveRows=liveChecks.checks??(liveChecks.error?[{name:"发布前检查",passed:false,detail:liveChecks.error}]:[{name:"发布前检查",passed:false,detail:"正在核对设备、走廊示教与标定…"}]);
  const checks=[...stepChecks,...liveRows,...planCheck];
  const failedChecks=checks.filter(c=>!c.passed),passedChecks=checks.filter(c=>c.passed);
  const checkRow=(c:typeof checks[number])=><div key={c.name} className={"wp-check-row "+(c.passed?"passed":"")}>{c.passed?<CheckCircle2 size={20}/>:<AlertTriangle size={20}/>}<div><strong>{c.name}</strong><p>{c.detail}</p></div></div>;
  const runBlocked=data.workspace.pending?"候选版本正在等待生效，生效或放弃后再验证":locked?"正在处理，请稍候":!samples.length?"先选用至少一组代表性样本":!sourceConfirmed?"勾选上方确认后才能运行":dirty||frameDirty||navigationDirty?"候选配置有未保存修改，保存后再验证":"";
  const publishBlocked=data.workspace.pending?"":planBlocked?planBlocked:liveChecks.error?liveChecks.error:!liveChecks.checks?"正在核对发布前检查":failedLive?`${failedLive.name}：${failedLive.detail}`:dirty||frameDirty||navigationDirty?"候选配置有未保存修改，保存后再发布":"";
  const sampleRows:SampleRow[]=[
    ...data.workspace.sampleBank.map(b=>{
      const selected=samples.find(s=>s.sampleId===b.id);
      return {key:"sample:"+b.id,sample:{historyId:null,sampleId:b.id,expected:b.expected},name:b.name,source:"整组原图",checkLabel:"选用样本 "+b.name,expectedLabel:b.name+"期望结论",selected:!!selected,expected:expectedValue(selected?.expected??b.expected)};
    }),
    ...records.map(r=>{
      const selected=samples.find(s=>s.historyId===r.id);
      return {key:"history:"+r.id,sample:{historyId:r.id,sampleId:null,expected:expectedValue(r.verdict)},name:<Link className="sn" to={"/history/"+r.id}>SN{" "}{r.sn}</Link>,source:`历史整圈 · ${r.recipeId} · v${r.recipeVersion}`,checkLabel:"选用历史 SN "+r.sn,expectedLabel:"SN "+r.sn+"期望结论",selected:!!selected,expected:expectedValue(selected?.expected??r.verdict)};
    }),
  ];
  const sampleColumns:TableColumn<SampleRow>[]=[
    {key:"use",header:"选用",render:row=><Checkbox label={<span className="qs-sr-only">{row.checkLabel}</span>} checked={row.selected} disabled={samplesLocked} onCheckedChange={on=>choose(row.sample,on)}/>},
    {key:"name",header:"样本",render:row=>row.name},
    {key:"source",header:"来源",render:row=>row.source},
    {key:"expected",header:"人工期望",render:row=><Select aria-label={row.expectedLabel} value={row.expected} options={expectedOptions} disabled={!row.selected||samplesLocked} onValueChange={value=>choose({...row.sample,expected:value as Verdict},true)}/>},
  ];
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
  const productionStage=data.productionStage==="collect"?"（采集版）":data.productionStage==="formal"?"（正式版）":"";
  const collectFailed=collect?.filter(c=>!c.passed)??[];
  const openCollect=async()=>{
    if(pending.current||!ready||data.workspace.pending)return;
    const operation={scope,sequence:current.current.sequence};
    pending.current=operation;setOperating(true);
    const valid=()=>current.current.alive&&current.current.scope===scope&&current.current.sequence===operation.sequence&&pending.current===operation;
    try{const checks=await workspaceApi.collectChecks(doc.id,data.workspace.revision);if(valid())setCollect(checks);}
    catch(error){if(valid())setError(errorText(error));}
    finally{if(valid()){pending.current=null;setOperating(false);}}
  };
  return <div className="wp-page"><WorkspaceBar /><div className="wp-columns"><div className="wp-stack"><Panel title="发布前检查" help="设备、走廊示教与多轮标定满足即可发布；独立样本验证可选" actions={<Badge tone={failedChecks.length?"warn":"ok"}>{failedChecks.length?`${failedChecks.length} 项未满足`:`${checks.length} 项全部满足`}</Badge>}>
    <div className="wp-checklist">{failedChecks.map(checkRow)}{passedChecks.length>0&&<Accordion collapsible items={[{ value: "passed", title: <span>已满足 {passedChecks.length} 项：{passedChecks.map(c=>c.name).join("、")}</span>, content: <div className="wp-checklist">{passedChecks.map(checkRow)}</div> }]} />}</div>
    </Panel><Panel title="独立样本验证（可选）" help={<><p>不是发布前提：使用完整原图重新运行当前候选检测，看结论是否符合预期；正常样本必须独立于示教与标定采集。</p><p>历史样本必须保留完整整圈原图，轨迹顺序与成像条件适用；验证时重新运行当前候选。</p><p>建议覆盖良品、断胶、偏位、胶宽异常和无法测量等现场情况。期望“合格”时，候选结论 OK 或 OK · 局部超差都算符合。修改人工期望后，需要重新验证。</p></>}>
      <Select className="wp-sample-source" label="历史样本来源" value={historyScope} disabled={samplesLocked} options={[{value:"current",label:"当前配方"},{value:"all",label:"全部配方"}]} onValueChange={value=>setHistoryScope(value as "current"|"all")}/>
      <Table caption="代表性验证样本选用" columns={sampleColumns} rows={sampleRows} rowKey={row=>row.key} emptyState={<p className="muted center">尚无代表性样本；请选择完整历史原图。</p>}/>
      <div className="wp-validate-actions">
        <Checkbox label="已确认所选样本的拍摄条件、轨迹和触发顺序适用于当前候选" checked={sourceConfirmed} disabled={samplesLocked} onCheckedChange={setSourceConfirmed}/>
        <span className="spacer"/>
        {runBlocked&&!operating&&<span className="wp-validate-reason" role="note">{runBlocked}</span>}
        <Button variant="filled" disabled={!ready||!!data.workspace.pending||!samples.length||!sourceConfirmed} onClick={()=>void action(()=>workspaceApi.validate(doc.id,data.workspace.revision,samples,sourceConfirmed),"当前候选的验证已完成").then(r=>{if(r)resultRef.current?.scrollIntoView?.({block:"start",behavior:scrollBehavior()});})}>{operating||busy?"验证中…":"运行整套图像验证"}</Button>
      </div>
    </Panel>{validation&&<div ref={resultRef} className="wp-validate-anchor"><Panel title="验证结果" actions={<Badge tone={validationCurrent?"ok":"warn"}>{validationCurrent?"通过":"未通过或待重验"}</Badge>}><Table caption="逐样本验证结果" columns={resultColumns} rows={validation.samples.map((s,i)=>({...s,key:String(i)}))} rowKey={row=>row.key}/>{selectionDirty&&<Notice title="样本选用或期望已改变" tone="warn">请重新运行验证；现有结果不用于发布。</Notice>}</Panel></div>}</div>
    <div className="wp-stack"><NavigationPublication /><Panel title="已发布版本"><KV label="当前发布">{data.productionVersion?"v"+data.productionVersion+productionStage:"未发布"}</KV><KV label="待发布候选">v{doc.version}</KV><KV label="候选修订">{data.workspace.revision}</KV><KV label="计划版本">{data.workspace.pending?.doc.planVersion?data.workspace.pending.doc.planVersion+"（待生效）":!data.planChanged&&data.productionPlanVersion?data.productionPlanVersion+"（不变）":"发布时分配新号"}</KV><KV label="预期总帧数 ΣN_exp">{expectedText(plan.expectedTotal,plan.toleranceTotal)}</KV><KV label="发布状态">{publishStep?.detail??""}</KV>{site?.mode==="offline"&&<Notice title="当前为离线模式" tone="warn">离线验证发布的版本只能在离线运行，真机需重新验证发布。</Notice>}<Button variant="filled" className="wp-publish-button" disabled={!canPublish} onClick={()=>setConfirm(true)}>{publishLabel}</Button>{!canPublish&&publishBlocked&&<p className="wp-validate-reason" role="note">{publishBlocked}</p>}<Button className="wp-publish-button" disabled={!ready||!!data.workspace.pending} onClick={()=>void openCollect()}>发布采集版</Button><p className="muted">新产线还没有可用配方时，先发布采集版供维护模式跟着生产采图。</p></Panel><Notice title="在工件边界生效">已开始检测的工件继续使用原配方快照。候选发布后，等待当前工件完成，再供新工件使用。</Notice>{(publish.state==="failed"||publish.state==="uncertain") && <Button disabled={!ready} onClick={() => void action(() => workspaceApi.retryPublish(doc.id,data.workspace.revision), "已重试发布 / 切换")}>重试发布 / 切换</Button>}{publish.state==="failed" && <Button disabled={!ready} onClick={()=>setAbandoning(true)}>放弃待生效版本</Button>}<Button asChild><Link to="/inspect">查看在线检测</Link></Button>{site?.mode === "offline" && site.source === "sampleSet" && <Notice title="候选配方离线试运行"><p>使用已保存候选跑一圈，结果标记为调试；不要求发布。</p><Button asChild><Link to={`/inspect?candidate=${encodeURIComponent(doc.id)}`}>前往在线检测调试候选</Link></Button></Notice>}</div>
  </div>
  {confirm&&<Dialog open title={publishLabel} closeDisabled={locked} onOpenChange={open=>{if(!open)setConfirm(false);}} footer={<><Button disabled={locked} onClick={()=>setConfirm(false)}>取消</Button><Button variant="filled" disabled={!canPublish} onClick={async()=>{const r=await action(()=>workspaceApi.publish(doc.id,data.workspace.revision),"已提交发布，将在工件边界生效");if(r&&current.current.alive&&current.current.scope===scope)setConfirm(false);}}>确认发布 v{doc.version}</Button></>}><p>将发布 <b>{doc.id} · v{doc.version}</b>，替换当前发布版本。{site?.mode==="offline"&&"该版本仅用于离线回放，不能用于真机生产。"}</p><PlanPublishBody plan={plan}/></Dialog>}
  {collect&&<Dialog open title="发布采集版" closeDisabled={locked} onOpenChange={open=>{if(!open)setCollect(null);}} footer={<><Button disabled={locked} onClick={()=>setCollect(null)}>取消</Button><Button variant="filled" disabled={!ready||!!data.workspace.pending||collectFailed.length>0} onClick={async()=>{const r=await action(()=>workspaceApi.publishCollect(doc.id,data.workspace.revision),"已提交采集版，将在工件边界生效");if(r&&current.current.alive&&current.current.scope===scope)setCollect(null);}}>确认发布采集版 v{doc.version}</Button></>}><div className="wp-stack"><Notice title="采集版只在维护模式下运行，只采图不判定" tone="warn">正常生产遇到这个型号会拒收。继续示教、标定后发布正式版；正式版生效后不能再发布采集版。</Notice><p>将发布 <b>{doc.id} · v{doc.version}</b>（采集版），PLC 型号 <b>{doc.productCode}</b>。不要求走廊示教、多轮标定和独立验证，不生成发布包。</p><Badge tone={collectFailed.length?"warn":"ok"}>{collectFailed.length?`${collectFailed.length} 项未满足`:`${collect.length} 项全部满足`}</Badge>{[...collectFailed,...collect.filter(c=>c.passed)].map(checkRow)}</div></Dialog>}
  {abandoning&&<Dialog open title="放弃待生效版本" closeDisabled={locked} onOpenChange={open=>{if(!open)setAbandoning(false);}} footer={<><Button disabled={locked} onClick={()=>setAbandoning(false)}>取消</Button><Button variant="filled" disabled={!ready||publish.state!=="failed"} onClick={async()=>{const r=await action(()=>workspaceApi.abandonPublish(doc.id,data.workspace.revision),"已放弃待生效版本，候选回到草稿");if(r&&current.current.alive&&current.current.scope===scope)setAbandoning(false);}}>确认放弃 v{data.workspace.pending?.doc.version??doc.version}</Button></>}><p>放弃后候选回到草稿，可修改后重新发布；当前生产版本不受影响。</p></Dialog>}
  </div>;
}
