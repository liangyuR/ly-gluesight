import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState } from "react";
import { Camera } from "lucide-react";
import { Button, Card } from "qing-steel";
import { workspaceApi } from "./api";
import { FrameViewer, Notice } from "./components";
import type { FrozenImage, FrameImage } from "./types";
import ImageImportButton from "./ImageImportButton";
import { desktopAvailable } from "../../lib/desktop";

export interface StationView {metadata:FrozenImage;image:FrameImage}
/** 三目拼接相机（旧配方）才标出视角（multiView）；单视角相机与段配方不带“图 N”。挂载时没有样本则载入当前标定所用原图。 */
export default function StationCapture({cam,cameraId,view=1,multiView=false,sample,onSample,cameraReady,marks}:{cam:number;cameraId:string;view?:number;multiView?:boolean;sample:StationView|null;onSample:(sample:StationView|null)=>void;cameraReady:boolean;marks?:{at:[number,number];color:string}[]|null}) {
  const [busy,setBusy]=useState(false),[error,setError]=useState(""),[reading,setReading]=useState(false),[restoring,setRestoring]=useState(false);
  const readingRef=useRef(false);
  const scope = JSON.stringify([cam, cameraId, view]);
  const current=useRef({scope,alive:true});current.current.scope=scope;
  const pending=useRef(false);
  const serial=useRef(0);
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;serial.current++;};},[]);
  useEffect(()=>{serial.current++;pending.current=false;setBusy(false);setRestoring(false);setError("");},[scope]);
  const begin = () => {
    pending.current = true;
    const request = ++serial.current;
    return () => current.current.alive && current.current.scope === scope && serial.current === request;
  };
  const loadSample = async (metadata: FrozenImage, valid: () => boolean) => {
    if (!valid()) return;
    if (metadata.camera !== cameraId || metadata.view !== view) throw new Error("返回的样本与当前相机或视角不一致");
    const image = await workspaceApi.stationImage(cam, cameraId, metadata.id, view);
    if (valid()) onSample({ metadata, image });
  };
  useEffect(()=>{
    if(!desktopAvailable()||sample||pending.current)return;
    // 载入会改写后端本相机的冻结样本：与拍照、导入互斥，载入结束前不放行
    const valid = begin();
    setRestoring(true);
    void (async()=>{
      try { const metadata = await workspaceApi.stationRestore(cam, cameraId, view); if (metadata) await loadSample(metadata, valid); }
      catch{/* 读不到标定原图时保持空态，照常拍照或导入 */}
      finally{if(valid()){pending.current=false;setRestoring(false);}}
    })();
  },[scope]);
  const capture=async()=>{
    if(pending.current||readingRef.current)return;
    const valid = begin();
    setBusy(true);setError("");onSample(null);
    try { await loadSample(await workspaceApi.stationCapture(cam, cameraId, view), valid); }
    catch(e){if(valid())setError(errorText(e));}finally{if(valid()){pending.current=false;setBusy(false);}}
  };
  const importImage=async(bytes:number[])=>{
    if(pending.current)return;
    const valid = begin();
    setBusy(true);setError("");onSample(null);
    try { await loadSample(await workspaceApi.stationImport(cam, cameraId, bytes, view), valid); }
    finally{if(valid()){pending.current=false;setBusy(false);}}
  };
  return <Card level="low" className="panel wp-panel wp-station-capture" title={multiView?`图 ${view}`:undefined} actions={<div className="wp-actions wp-station-actions"><ImageImportButton label="导入离线图" scope={scope} disabled={busy||restoring} onImport={importImage} onError={setError} onReadingChange={value=>{readingRef.current=value;setReading(value);}}/><Button variant={sample||!cameraReady?"tonal":"filled"} leadingIcon={<Camera size={15}/>} disabled={busy||reading||restoring||!cameraReady} title={!cameraReady?"相机未就绪，无法拍照":undefined} onClick={()=>void capture()}>{busy?"拍照中…":"拍照"}</Button></div>}>
    <FrameViewer image={sample?.image??null} marks={marks} loading={busy||reading||restoring} error={error} label={sample?.metadata.id??""} emptyTitle="尚无标定样本" emptyHint={cameraReady?"将标定板放在胶路测量平面后拍照，或导入本工位的离线图。":"相机未就绪，可连接相机后拍照，或导入本工位的离线图。"}/>
    {sample&&<p className="muted mono">{sample.metadata.camera} · {sample.metadata.source==="import"?"离线原图":(sample.metadata.source==="calib"?"标定原图 · ":"")+new Date(sample.metadata.capturedAt).toLocaleString("zh-CN")}{sample.metadata.exposureUs!=null&&" · 曝光 "+sample.metadata.exposureUs+" μs"}</p>}
    {error&&sample&&<Notice title="未能取样" tone="warn">{error}</Notice>}
  </Card>;
}
