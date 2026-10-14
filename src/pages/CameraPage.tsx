import { useEffect, useRef, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { Camera, ChevronRight, MoreHorizontal, Plus, Trash2 } from "lucide-react";
import { cameraApi, CameraConfigPanel, defaultCameraConfig, FramePreview, useRigStatus } from "../features/camera";
import CameraList, { cameraState } from "../features/camera/components/CameraList";
import type { CameraConfigHandle } from "../features/camera/components/CameraConfigPanel";
import { useRigConfig } from "../features/camera/useRigConfig";
import { useUnsavedGuard } from "../features/camera/useUnsavedGuard";
import { OfflineManagedNotice, useCycle, useSiteStatus } from "../features/cycle";
import { desktopAvailable } from "../lib/desktop";
import { Badge, Notice } from "../features/workspace/components";
import type { CaptureNavigationState } from "../features/workspace/captureNavigation";
import { ReturnButton } from "../features/workspace/returnNavigation";
import { productionBusy } from "../features/cycle/phase";
import { errorText } from "../lib/errors";

export default function CameraPage() {
  const location=useLocation();
  const captureReturn=(location.state as CaptureNavigationState | null)?.captureReturn;
  const navigationState:CaptureNavigationState={...(location.state as CaptureNavigationState | null)};
  const {statuses,lastFrame}=useRigStatus();
  const {snapshot}=useCycle();
  const site=useSiteStatus();
  const readOnly=site?.mode==="offline";
  const {configs,cam,setCam,error,setError,loading,references,referencesReady,reload}=useRigConfig({withReferences:true,preferredId:captureReturn?.cameraId,reloadKey:readOnly});
  const [action,setAction]=useState("");
  const [configSaving,setConfigSaving]=useState(false);
  const [dirty,setDirty]=useState(false);
  const form=useRef<CameraConfigHandle>(null);
  const mounted=useRef(true),pending=useRef(false);
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;};},[]);
  const {guard,dialog}=useUnsavedGuard({dirty,save:()=>form.current?.save()??Promise.resolve(true),discard:()=>form.current?.discard()});

  const config=configs[cam]??null;
  const status=statuses.find(s=>s.cam===cam)??null;
  const busy=productionBusy(snapshot);
  const desktop=desktopAvailable();
  const referencedBy=config?references[config.id]??[]:[];
  const locked=busy||readOnly||!desktop||!!action;
  const add=async()=>{
    if(pending.current||loading||configSaving||locked||configs.length>=8)return;
    pending.current=true;setAction("add");
    setError("");
    try {
      const base=configs.at(-1)??defaultCameraConfig;
      let n=configs.length+1;while(configs.some(c=>c.name==="相机 "+n))n++;
      const i=await cameraApi.add({...base,name:"相机 "+n,serial:""});
      if(!mounted.current)return;
      const next=await reload();if(next&&mounted.current)setCam(Math.min(i,next.length-1));
    }catch(e){if(mounted.current)setError(errorText(e));}
    finally{pending.current=false;if(mounted.current)setAction("");}
  };
  const removeBlocked=!referencesReady?"相机引用尚未读取":referencedBy.length?`被 ${referencedBy.join("、")} 引用，先在配方中改选相机`:configs.length<=1?"至少保留一台相机":"";
  const remove=async()=>{
    if(!config||pending.current||loading||configSaving||locked||removeBlocked)return;
    if(!window.confirm("从相机组移除「"+config.name+"」？"))return;
    pending.current=true;setAction("remove");setError("");
    try{await cameraApi.remove(cam);if(!mounted.current)return;const next=await reload();if(next&&mounted.current)setCam(Math.min(Math.max(0,cam-1),next.length-1));}
    catch(e){if(mounted.current)setError(errorText(e));}
    finally{pending.current=false;if(mounted.current)setAction("");}
  };
  const released=!!status?.released;
  const handOver=async()=>{
    if(!config||pending.current||configSaving)return;
    pending.current=true;setAction(released?"reconnect":"release");setError("");
    try{if(released)await cameraApi.reconnect(cam);else await cameraApi.release(cam);}
    catch(e){if(mounted.current)setError(errorText(e));}
    finally{pending.current=false;if(mounted.current)setAction("");}
  };
  const state=config?cameraState(config,status):null;
  const notice=error?<Notice title="操作未完成" tone="warn">{error}</Notice>:busy?<Notice title="当前工件正在检测">相机配置与出图测试在工件结束后可操作。</Notice>:readOnly?<OfflineManagedNotice>相机组按离线来源临时生成，离线期间不能修改或增删相机。</OfflineManagedNotice>:null;

  return <div className="cam-page">
    <ReturnButton disabled={configSaving||!!action} guard={guard} hint="配置后保存并应用，再返回继续。"/>
    {notice&&<div className="cam-notice">{notice}</div>}
    <CameraList configs={configs} statuses={statuses} selected={cam} disabled={!!action||configSaving} onSelect={i=>{if(i!==cam)guard(()=>setCam(i));}} onRefresh={()=>guard(()=>void reload())} refreshing={loading}
      footer={!readOnly&&<button className="btn cam-list-add" onClick={()=>void add()} disabled={locked||loading||configSaving||configs.length>=8}><Plus size={15}/>{action==="add"?"添加中…":"添加相机"}</button>}/>
    {config&&state?<main className="cam-main">
      <header className="cam-head">
        <strong>{config.name}</strong><span className="mono muted">{config.id}</span>
        <Badge tone={state.tone==="ok"?"ok":"warn"}>{state.label}</Badge>
        <span className="muted cam-head-info">{status?.ready?[status.device&&`${status.device.model} · ${status.device.serial}${status.device.ip?" · "+status.device.ip:""}`,`${status.fps?status.fps.toFixed(1):"0.0"} fps`,`丢包 ${status.lostPackets}`].filter(Boolean).join(" · "):released?"在海康 MVS 客户端调焦、调光圈，完成后关闭 MVS 再重新连接":state.label==="未绑定"?"在下方选择要绑定的设备":state.text}</span>
        <span className="spacer"/>
        {!readOnly&&config.source==="mvs"&&!!config.serial&&<button className="btn" onClick={()=>void handOver()} disabled={!desktop||!!action||configSaving||(!released&&busy)} title={released?"收回相机并重新打开":"关闭相机，供海康 MVS 客户端连接调焦"}>{action==="release"?"释放中…":action==="reconnect"?"连接中…":released?"重新连接":"释放给 MVS"}</button>}
        {!readOnly&&<details className="cam-more"><summary className="icon-btn" aria-label="更多操作"><MoreHorizontal size={18}/></summary><div className="cam-more-menu"><button className="btn" onClick={()=>void remove()} disabled={locked||loading||configSaving||!!removeBlocked}><Trash2 size={15}/>{action==="remove"?"移除中…":"移除相机"}</button>{removeBlocked&&<small className="muted">{removeBlocked}</small>}</div></details>}
      </header>
      <div className="cam-body">
        <div className="cam-col">
          <CameraConfigPanel ref={form} key={config.id+":"+config.source} cam={cam} initial={config} status={status} locked={locked} saveBlocked={released?"已释放给 MVS，重新连接后再保存":""} onSaved={()=>void reload()} onSavingChange={setConfigSaving} onDirtyChange={setDirty}/>
          <Link className="cam-next" to="/camera/calibration" state={navigationState}>保存后前往飞拍标定<ChevronRight size={14}/></Link>
        </div>
        <FramePreview key={config.id} cam={cam} status={status} lastFrame={lastFrame[cam]} config={config} blockedReason={busy?"工件检测中":!desktop?"需要桌面后端":config.source==="mvs"&&!config.serial?"未绑定设备":released?"已释放给 MVS":dirty?"先保存并应用再测试":""}/>
      </div>
    </main>:<main className="cam-main"><div className="wp-empty"><Camera size={32}/><h2>{loading?"正在读取相机配置":"尚未添加相机"}</h2><p>{desktop?"添加相机后绑定具体设备，配置触发接线并测试出图。":"请在桌面软件中添加或连接相机。"}</p>{desktop&&!readOnly&&!loading&&<button className="btn primary" onClick={()=>void add()} disabled={locked}><Plus size={15}/>添加相机</button>}</div></main>}
    {dialog}
  </div>;
}
