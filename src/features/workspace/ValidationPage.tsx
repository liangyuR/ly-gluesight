import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { AlertTriangle, CheckCircle2 } from "lucide-react";
import { Accordion, Button, Dialog } from "qing-steel";
import "./validation.css";
import { stepOfV2 } from "./progress";
import { expectedText } from "../plan/vis";
import { planPublishBlocker } from "./planPublish";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, KV, Notice, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";
import { PlanPublishBody } from "./PlcParams";
import type { Check } from "./types";
import { NavigationPublication } from "./WorkspaceNavigation";
import { corridorPath } from "./cameraTasks";
import { recipeHref } from "./recipeRoutes";
import { returnState } from "./returnNavigation";

export default function ValidationPage() {
  const location = useLocation();
  const {data,doc,dirty,frameDirty,navigationDirty,segmentDirty,saving,unsavable,busy,act,setError} = useWorkspace();
  const waitingForSave = dirty || frameDirty || navigationDirty || segmentDirty || saving;
  const candidate = data ? JSON.stringify([data.workspace.doc.id,data.workspace.instanceId,data.workspace.doc.version]) : null;
  const pendingRelease = data?.workspace.pending;
  const [confirmation, setConfirmation] = useState<{key: string; error: string} | null>(null);
  const [abandoning,setAbandoning] = useState(false);
  const [collect,setCollect] = useState<Check[]|null>(null);
  const publishLabel = "发布生产配方";
  const [checkAttempt,setCheckAttempt] = useState(0);
  const [publishChecks,setPublishChecks] = useState<{key:string;checks:Check[]|null;error:string}>({key:"",checks:null,error:""});
  const [operating,setOperating] = useState(false);
  const pending = useRef<{scope:string;sequence:number}|null>(null);
  const scope = `${candidate??""}:${data?.workspace.revision??""}:${!!pendingRelease}:${waitingForSave}`;
  const current = useRef({scope,alive:true,sequence:0});
  if(current.current.scope!==scope){current.current.scope=scope;current.current.sequence++;}
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;};},[]);
  useEffect(()=>{pending.current=null;setOperating(false);setAbandoning(false);setCollect(null);},[scope]);
  const checksKey=JSON.stringify([scope,current.current.sequence,checkAttempt]);
  useEffect(()=>{
    if(!data||!data.plan||pendingRelease||waitingForSave)return;
    let alive=true;
    workspaceApi.publishChecks(data.workspace.doc.id,data.workspace.revision)
      .then(checks=>alive&&setPublishChecks({key:checksKey,checks,error:""}))
      .catch(e=>alive&&setPublishChecks({key:checksKey,checks:null,error:errorText(e)}));
    return()=>{alive=false;};
  },[checksKey]);
  if(!data||!doc)return <WorkspaceEmpty/>;
  const plan=data.plan;
  const locked=busy||operating;
  const ready=!waitingForSave&&!locked;
  // 没有整件 ΣN_exp（多段时各段 N_exp）时不能发布
  const planBlocked=planPublishBlocker(plan);
  const checking = !pendingRelease && !waitingForSave && publishChecks.key !== checksKey;
  const liveChecks = pendingRelease ? { checks: pendingRelease.validation.checks, error: "" }
    : waitingForSave || checking ? { checks: null, error: "" } : publishChecks;
  const failedLive=liveChecks.checks?.find(c=>!c.passed&&!c.advisory);
  const advisories=liveChecks.checks?.filter(c=>!c.passed&&c.advisory)??[];
  const canPublish=ready&&!!liveChecks.checks&&!failedLive&&!data.workspace.pending&&!planBlocked;
  const publish=data.workspace.publish;
  const publishStep=stepOfV2(plan.progress,"publish");
  const stepCheck=(step:{label:string;status:string;detail:string;blockers:{message:string}[]})=>({name:step.label,passed:step.status==="done",detail:step.blockers.length?`${step.detail}；${step.blockers[0].message}`:step.detail});
  const stepChecks=(["capture","segments"] as const).flatMap(id=>{const step=stepOfV2(plan.progress,id);return step?[stepCheck(step)]:[];});
  // 整件示教帧数 ΣN_exp 是否已得出：由 DTO 得出，不依赖后端步骤文案
  const planCheck=[{name:"示教总帧数 ΣN_exp",passed:!planBlocked,detail:planBlocked||(plan.calibrated?`${expectedText(plan.expectedTotal)} 帧`:"")}];
  const liveRows = liveChecks.checks ?? (waitingForSave ? [] : [{
    name: "发布前检查", passed: false, detail: liveChecks.error || "正在核对设备、走廊示教与示教件基准…",
  }]);
  const checks:Check[]=[...stepChecks,...liveRows,...planCheck];
  const failedChecks=checks.filter(c=>!c.passed&&!c.advisory),passedChecks=checks.filter(c=>c.passed);
  const checkRow = (c: Check) => {
    const targets = !pendingRelease && !c.passed ? c.teachingTargets ?? [] : [];
    return <div key={c.name} className={"wp-check-row " + (c.passed ? "passed" : "")}>
      {c.passed ? <CheckCircle2 size={20} /> : <AlertTriangle size={20} />}
      <div><strong>{c.name}{!c.passed && c.advisory && "（提醒，不拦发布）"}</strong><p>{c.detail}</p>
        {targets.length > 0 && <div className="wp-actions wp-check-targets">{targets.map(({ camera, segment }) => {
          const path = corridorPath(camera, segment);
          return <Button asChild key={path} size="sm" variant="text">
            <Link to={recipeHref(doc.id, path)} state={returnState(location, "发布前检查")}>
              去示教 {camera} · {segment == null ? "相机参数" : `第 ${segment} 段`}
            </Link>
          </Button>;
        })}</div>}
      </div>
    </div>;
  };
  const saveBlocker = unsavable ? "走廊长度不足三站，回示教页修正或撤销后自动检查" : "候选修改尚未保存，保存后自动检查";
  const publishBlocked = pendingRelease ? "" : waitingForSave ? saveBlocker
    : planBlocked || liveChecks.error || (!liveChecks.checks ? "正在核对发布前检查" : failedLive ? `${failedLive.name}：${failedLive.detail}` : "");
  const action=async(request:()=>ReturnType<typeof workspaceApi.publish>,message:string)=>{
    if(pending.current||!ready)return null;
    const operation={scope,sequence:current.current.sequence};
    pending.current=operation;setOperating(true);
    const valid=()=>current.current.alive&&current.current.scope===scope&&current.current.sequence===operation.sequence&&pending.current===operation;
    try{const result=await act(request,message);return valid()?result:null;}
    catch(error){if(valid())setError(errorText(error));return null;}
    finally{if(valid()){pending.current=null;setOperating(false);}}
  };
  const confirmPublish = async () => {
    if (!canPublish) return;
    const result = await action(async () => {
      setConfirmation({key: checksKey, error: ""});
      try {
        return await workspaceApi.publish(doc.id, data.workspace.revision);
      } catch (error) {
        if (current.current.alive && current.current.scope === scope) {
          setConfirmation(open => open?.key === checksKey ? {...open, error: errorText(error)} : open);
        }
        throw error;
      }
    }, "已提交发布，将在工件边界生效");
    if (result && current.current.alive) {
      setConfirmation(open => open?.key === checksKey ? null : open);
    }
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
  const checkStatus = pendingRelease ? "已冻结" : waitingForSave ? "待保存后检查" : checking ? "检查中"
    : failedChecks.length ? `${failedChecks.length} 项未满足` : advisories.length ? `${advisories.length} 项提醒` : `${checks.length} 项全部满足`;
  const checkTone = pendingRelease || waitingForSave || checking ? "info" : failedChecks.length || advisories.length ? "warn" : "ok";
  return <div className="wp-page"><WorkspaceBar /><div className="wp-columns"><div className="wp-stack"><Panel title="发布前检查" help="设备、走廊示教与示教件基准满足即可发布；检测结果在历史页用候选重算历史记录核对" actions={<div className="wp-actions">
    <Badge tone={checkTone}>{checkStatus}</Badge>
    {!pendingRelease && <Button size="sm" disabled={locked || waitingForSave || checking} onClick={() => setCheckAttempt(attempt => attempt + 1)}>重新检查</Button>}
  </div>}>
    <div className="wp-checklist">{failedChecks.map(checkRow)}{advisories.map(checkRow)}{passedChecks.length>0&&<Accordion collapsible items={[{ value: "passed", title: <span>已满足 {passedChecks.length} 项：{passedChecks.map(c=>c.name).join("、")}</span>, content: <div className="wp-checklist">{passedChecks.map(checkRow)}</div> }]} />}</div>
    </Panel></div>
    <div className="wp-stack"><NavigationPublication /><Panel title="已发布版本"><KV label="当前发布">{data.productionVersion?"v"+data.productionVersion+productionStage:"未发布"}</KV><KV label="待发布候选">v{doc.version}</KV><KV label="候选修订">{data.workspace.revision}</KV><KV label="示教总帧数 ΣN_exp">{expectedText(plan.expectedTotal)}</KV><KV label="发布状态">{publishStep?.detail??""}</KV><Button variant="filled" className="wp-publish-button" disabled={!canPublish} onClick={() => setConfirmation({key: checksKey, error: ""})}>{publishLabel}</Button>{!canPublish&&publishBlocked&&<p className="wp-validate-reason" role="note">{publishBlocked}</p>}<Button className="wp-publish-button" disabled={!ready||!!data.workspace.pending} onClick={()=>void openCollect()}>发布采集版</Button><p className="muted">新产线还没有可用配方时，先发布采集版供维护模式跟着生产采图。</p></Panel><Notice title="在工件边界生效">已开始检测的工件继续使用原配方快照。候选发布后，等待当前工件完成，再供新工件使用。</Notice>{(publish.state==="failed"||publish.state==="uncertain") && <Button disabled={!ready} onClick={() => void action(() => workspaceApi.retryPublish(doc.id,data.workspace.revision), "已重试发布 / 切换")}>重试发布 / 切换</Button>}{publish.state==="failed" && <Button disabled={!ready} onClick={()=>setAbandoning(true)}>放弃待生效版本</Button>}<Button asChild><Link to="/inspect">查看在线检测</Link></Button></div>
  </div>
  {confirmation?.key === checksKey && <Dialog open title={publishLabel} closeDisabled={locked}
    onOpenChange={open => { if (!open && !locked) setConfirmation(null); }}
    footer={<>
      <Button disabled={locked} onClick={() => setConfirmation(null)}>取消</Button>
      <Button variant="filled" loading={operating} disabled={!canPublish} onClick={() => void confirmPublish()}>
        {operating ? "生成中…" : confirmation.error ? "重试发布" : `确认发布 v${doc.version}`}
      </Button>
    </>}>
    <p>将发布 <b>{doc.id} · v{doc.version}</b>，替换当前发布版本。</p>
    {operating && <p role="status">正在生成发布包，请稍候。提交成功后将在工件边界生效。</p>}
    {confirmation.error && <Notice tone="ng" title="发布未完成">{confirmation.error}</Notice>}
    {advisories.map(c => <Notice key={c.name} title={`${c.name}：发布后部分段不测`} tone="warn">{c.detail}</Notice>)}
    <PlanPublishBody plan={plan} />
  </Dialog>}
  {collect&&<Dialog open title="发布采集版" closeDisabled={locked} onOpenChange={open=>{if(!open)setCollect(null);}} footer={<><Button disabled={locked} onClick={()=>setCollect(null)}>取消</Button><Button variant="filled" disabled={!ready||!!data.workspace.pending||collectFailed.length>0} onClick={async()=>{const r=await action(()=>workspaceApi.publishCollect(doc.id,data.workspace.revision),"已提交采集版，将在工件边界生效");if(r&&current.current.alive&&current.current.scope===scope)setCollect(null);}}>确认发布采集版 v{doc.version}</Button></>}><div className="wp-stack"><Notice title="采集版只在维护模式下运行，只采图不判定" tone="warn">正常生产遇到这个型号会回检测错误。继续示教、试测后发布正式版；正式版生效后不能再发布采集版。</Notice><p>将发布 <b>{doc.id} · v{doc.version}</b>（采集版），PLC 型号 <b>{doc.productCode}</b>。不要求走廊示教和试测，不生成发布包。</p><Badge tone={collectFailed.length?"warn":"ok"}>{collectFailed.length?`${collectFailed.length} 项未满足`:`${collect.length} 项全部满足`}</Badge>{[...collectFailed,...collect.filter(c=>c.passed)].map(checkRow)}</div></Dialog>}
  {abandoning&&<Dialog open title="放弃待生效版本" closeDisabled={locked} onOpenChange={open=>{if(!open)setAbandoning(false);}} footer={<><Button disabled={locked} onClick={()=>setAbandoning(false)}>取消</Button><Button variant="filled" disabled={!ready||publish.state!=="failed"} onClick={async()=>{const r=await action(()=>workspaceApi.abandonPublish(doc.id,data.workspace.revision),"已放弃待生效版本，候选回到草稿");if(r&&current.current.alive&&current.current.scope===scope)setAbandoning(false);}}>确认放弃 v{data.workspace.pending?.doc.version??doc.version}</Button></>}><p>放弃后候选回到草稿，可修改后重新发布；当前生产版本不受影响。</p></Dialog>}
  </div>;
}
