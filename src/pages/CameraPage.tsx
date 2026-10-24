import { useEffect, useRef, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { Camera, ChevronLeft, ChevronRight, ExternalLink, Plus, RefreshCw, Trash2 } from "lucide-react";
import { Alert, Button, EmptyState, StatusBadge } from "qing-steel";
import { cameraApi, CameraConfigPanel, defaultCameraConfig, FramePreview, useRigStatus } from "../features/camera";
import CameraOverview from "../features/camera/components/CameraOverview";
import { AppliedPanel } from "../features/camera/components/FramePreview";
import { cameraHealth, StateBadge } from "../features/camera/components/health";
import type { CameraConfigHandle } from "../features/camera/components/CameraConfigPanel";
import { useRigConfig } from "../features/camera/useRigConfig";
import { useUnsavedGuard } from "../features/camera/useUnsavedGuard";
import { OfflineManagedNotice, useCycle, useSiteStatus } from "../features/cycle";
import { desktopAvailable } from "../lib/desktop";
import { Notice } from "../features/workspace/components";
import type { CaptureNavigationState } from "../features/workspace/captureNavigation";
import { ReturnButton } from "../features/workspace/returnNavigation";
import { productionBusy } from "../features/cycle/phase";
import { errorText } from "../lib/errors";
import "../features/camera/camera-group.css";

export default function CameraPage() {
  const location=useLocation();
  const captureReturn=(location.state as CaptureNavigationState | null)?.captureReturn;
  const navigationState:CaptureNavigationState={...(location.state as CaptureNavigationState | null)};
  const {statuses,lastFrame}=useRigStatus();
  const {snapshot}=useCycle();
  const site=useSiteStatus();
  const readOnly=site?.mode==="offline";
  const {configs,error,setError,loading,references,referencesReady,referenceError,reload}=useRigConfig({withReferences:true,preferredId:captureReturn?.cameraIds[0],reloadKey:readOnly});
  const [detailId,setDetailId]=useState<string|null>(captureReturn?.cameraIds[0]??null);
  const [action,setAction]=useState("");
  const [removeError,setRemoveError]=useState("");
  const [handError,setHandError]=useState("");
  const [configSaving,setConfigSaving]=useState(false);
  const [dirty,setDirty]=useState(false);
  const form=useRef<CameraConfigHandle>(null);
  const mounted=useRef(true),pending=useRef(false);
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;};},[]);
  const {guard,dialog}=useUnsavedGuard({dirty,save:()=>form.current?.save()??Promise.resolve(true),discard:()=>form.current?.discard()});

  const statusOf=(i:number)=>statuses.find(s=>s.cam===i)??null;
  const cam=detailId==null?-1:configs.findIndex(c=>c.id===detailId);
  const config=configs[cam]??null;
  const status=config?statusOf(cam):null;
  const busy=productionBusy(snapshot);
  const desktop=desktopAvailable();
  const referencedBy=config?references[config.id]??[]:[];
  const lockedReason=busy?"工件结束后可修改":readOnly?"离线期间由站点模式管理":!desktop?"需要桌面后端":"";
  const locked=!!lockedReason||!!action;
  const open=(i:number)=>{const id=configs[i]?.id;if(id&&id!==detailId)guard(()=>{setRemoveError("");setHandError("");setDetailId(id);});};
  const back=()=>guard(()=>{setRemoveError("");setHandError("");setDetailId(null);});

  const add=async()=>{
    if(pending.current||loading||configSaving||locked||configs.length>=8)return;
    pending.current=true;setAction("add");
    setError("");
    try {
      const base=configs.at(-1)??defaultCameraConfig;
      let n=configs.length+1;while(configs.some(c=>c.name==="相机 "+n))n++;
      const i=await cameraApi.add({...base,name:"相机 "+n,serial:""});
      if(!mounted.current)return;
      const next=await reload();if(next&&mounted.current)setDetailId(next[Math.min(i,next.length-1)]?.id??null);
    }catch(e){if(mounted.current)setError(errorText(e));}
    finally{pending.current=false;if(mounted.current)setAction("");}
  };
  const removeBlocked=!referencesReady?(referenceError||"相机引用尚未读取"):referencedBy.length?`被 ${referencedBy.join("、")} 引用，先在配方中改选相机`:configs.length<=1?"至少保留一台相机":"";
  const remove=async()=>{
    if(!config||pending.current||loading||configSaving||locked||removeBlocked)return;
    if(!window.confirm("从相机组移除「"+config.name+"」？"))return;
    pending.current=true;setAction("remove");setRemoveError("");
    try{await cameraApi.remove(cam);if(!mounted.current)return;await reload();if(mounted.current)setDetailId(null);}
    catch(e){if(mounted.current)setRemoveError(errorText(e));}
    finally{pending.current=false;if(mounted.current)setAction("");}
  };
  const released=!!status?.released;
  const handOverBlocked=!desktop||readOnly||busy||!!action||configSaving||loading;
  const handOver=async()=>{
    if(!config||config.source!=="mvs"||!config.serial||pending.current||handOverBlocked)return;
    pending.current=true;setAction(released?"reconnect":"release");setHandError("");
    try{if(released)await cameraApi.reconnect(cam);else await cameraApi.release(cam);}
    catch(e){if(mounted.current)setHandError(errorText(e));}
    finally{pending.current=false;if(mounted.current)setAction("");}
  };

  const notice=busy?<Notice title="当前工件正在检测，配置在工件结束后可修改">总览与帧计数继续实时刷新；添加 / 移除相机、绑定、触发接线、取一帧和释放给 MVS 暂不可用，避免打断在途采集。</Notice>:readOnly?<OfflineManagedNotice>相机组按离线来源临时生成，离线期间不能修改或增删相机。</OfflineManagedNotice>:null;
  const calibration=(text:string)=><Link className="cg-link quiet" to="/camera/calibration" state={navigationState}>{text}<ChevronRight size={14}/></Link>;

  if(!configs.length)return <div className="cg-page">
    <ReturnButton disabled={!!action} guard={guard} hint="配置后保存并应用，再返回继续。"/>
    {notice}
    <EmptyState icon={<Camera size={32}/>} title={loading?"正在读取相机配置":"尚未添加相机"} description={desktop?"添加相机后绑定具体设备，配置触发接线并测试出图。":"请在桌面软件中添加或连接相机。"}
      action={<>{error&&<Alert tone="ng">{error}</Alert>}{desktop&&!readOnly&&!loading&&<Button variant="filled" leadingIcon={<Plus size={15}/>} onClick={()=>void add()} disabled={locked}>添加相机</Button>}</>}/>
  </div>;

  if(!config)return <div className="cg-page">
    <ReturnButton disabled={!!action} guard={guard} hint="配置后保存并应用，再返回继续。"/>
    {notice}
    <CameraOverview configs={configs} statuses={statuses} lastFrame={lastFrame} lockedReason={lockedReason} canReconnect={desktop&&!readOnly&&!busy} loading={loading} adding={action==="add"} error={error}
      onOpen={open} onRefresh={()=>void reload()} onAdd={readOnly?undefined:()=>void add()} onReconnect={i=>cameraApi.reconnect(i)}
      footer={<div className="cg-row" style={{justifyContent:"flex-end"}}>{calibration("前往飞拍标定")}</div>}/>
    {dialog}
  </div>;

  const health=cameraHealth(config,status);
  const warnings=status?.warnings??[];
  const mvs=config.source==="mvs";
  const handOverButton=!readOnly&&mvs&&!!config.serial&&<Button leadingIcon={released?<RefreshCw size={15}/>:<ExternalLink size={15}/>} onClick={()=>void handOver()} disabled={handOverBlocked} title={released?"收回相机并重新打开":"关闭相机，供海康 MVS 客户端连接调焦"}>{action==="release"?"释放中…":action==="reconnect"?"连接中…":released?"重新连接":"释放给 MVS"}</Button>;
  const leading=!readOnly&&<div className="cg-remove">
    <Button size="sm" leadingIcon={<Trash2 size={13}/>} onClick={()=>void remove()} disabled={locked||loading||configSaving||!!removeBlocked}>{action==="remove"?"移除中…":"移除相机"}</Button>
    <small className={removeError?"cg-err":undefined}>{removeError||removeBlocked||(referencedBy.length?"":"未被任何配方引用，可移除。")}{!referencesReady&&referenceError&&<> <Link className="cg-link" to="/recipe">在配方库处理</Link></>}</small>
  </div>;

  return <div className="cg-page">
    <div className="cg-crumb-row">
      <Button variant="text" leadingIcon={<ChevronLeft size={16}/>} onClick={back} disabled={configSaving||!!action}>相机组总览</Button>
      <span className="muted">/</span>
      <strong>{config.name}</strong><span className="mono">{config.id}</span>
      <StateBadge {...health}/>
      {warnings.length>0&&<StatusBadge tone="warn">{warnings.length} 项参数未接受</StatusBadge>}
      {locked&&lockedReason&&<StatusBadge tone="off">只读</StatusBadge>}
      {configs.length>1&&<div className="cg-switch" role="group" aria-label="切换相机">{configs.map((c,i)=><button key={c.id} aria-current={i===cam?"true":undefined} aria-label={`${c.name} ${c.id}`} disabled={configSaving||!!action} onClick={()=>open(i)}><span className={"dot "+cameraHealth(c,statusOf(i)).tone}/>{c.id}</button>)}</div>}
      <span className="spacer"/>
      <ReturnButton disabled={configSaving||!!action} guard={guard} hint="配置后保存并应用，再返回继续。"/>
    </div>
    {notice}
    <CameraConfigPanel ref={form} key={config.id+":"+config.source} cam={cam} initial={config} status={status} group={{configs,statuses}} locked={locked} saveBlocked={released?"已释放给 MVS，重新连接后再保存":""}
      onSaved={()=>void reload()} onGroupSaved={()=>void reload()} onSavingChange={setConfigSaving} onDirtyChange={setDirty} leading={leading} footer={calibration(locked?"前往飞拍标定":"保存后前往飞拍标定")}
      side={<>
        <FramePreview key={config.id} cam={cam} status={status} lastFrame={lastFrame[cam]} config={config} actions={handOverButton}
          blockedReason={busy?"工件检测中":!desktop?"需要桌面后端":mvs&&!config.serial?"未绑定设备":released?"已释放给 MVS，在海康 MVS 客户端调焦、调光圈，完成后关闭 MVS 再重新连接":dirty?"先保存并应用再测试":""}/>
        {handError&&<Alert tone="ng">{handError}</Alert>}
        <AppliedPanel config={config} status={status}/>
      </>}/>
    {dialog}
  </div>;
}
