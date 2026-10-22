import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState } from "react";
import { Camera } from "lucide-react";
import { Button } from "qing-steel";
import { workspaceApi } from "./api";
import { GrayViewer, Notice, Panel } from "./components";
import type { FrozenImage, GrayImage } from "./types";
import ImageImportButton from "./ImageImportButton";

export interface StationView {metadata:FrozenImage;image:GrayImage}
/** 三目拼接相机（旧配方）才标出视角（multiView）；单视角相机与段配方不带“图 N”。 */
export default function StationCapture({cam,view=1,multiView=false,sample,onSample}:{cam:number;view?:number;multiView?:boolean;sample:StationView|null;onSample:(sample:StationView|null)=>void}) {
  const [busy,setBusy]=useState(false),[error,setError]=useState(""),[reading,setReading]=useState(false);
  const readingRef=useRef(false);
  const current=useRef({cam,view,alive:true});current.current.cam=cam;current.current.view=view;
  const pending=useRef(false);
  const serial=useRef(0);
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;serial.current++;};},[]);
  useEffect(()=>{serial.current++;pending.current=false;setBusy(false);setError("");},[cam,view]);
  const capture=async()=>{
    if(pending.current||readingRef.current)return;pending.current=true;
    const request=++serial.current;
    const valid=()=>current.current.alive&&current.current.cam===cam&&current.current.view===view&&serial.current===request;
    setBusy(true);setError("");onSample(null);
    try{const metadata=await workspaceApi.stationCapture(cam,view);if(!valid())return;const image=await workspaceApi.stationImage(cam,metadata.id,view);if(valid())onSample({metadata,image});}
    catch(e){if(valid())setError(errorText(e));}finally{if(valid()){pending.current=false;setBusy(false);}}
  };
  const importImage=async(bytes:number[])=>{
    if(pending.current)return;pending.current=true;
    const request=++serial.current;
    const valid=()=>current.current.alive&&current.current.cam===cam&&current.current.view===view&&serial.current===request;
    setBusy(true);setError("");onSample(null);
    try{const metadata=await workspaceApi.stationImport(cam,bytes,view);if(!valid())return;const image=await workspaceApi.stationImage(cam,metadata.id,view);if(valid())onSample({metadata,image});}
    finally{if(valid()){pending.current=false;setBusy(false);}}
  };
  return <Panel title={(multiView?"图 "+view+" · ":"")+"冻结标定样本"} detail="取样或导入本工位的离线原图，再在同一张图像上标定和试测" actions={<div className="wp-actions"><ImageImportButton scope={"station:"+cam+":"+view} disabled={busy} onImport={importImage} onError={setError} onReadingChange={value=>{readingRef.current=value;setReading(value);}}/><Button variant={sample?"tonal":"filled"} leadingIcon={<Camera size={15}/>} disabled={busy||reading} onClick={()=>void capture()}>{busy?"取样中…":"取新样本"}</Button></div>}>
    <GrayViewer image={sample?.image??null} loading={busy||reading} error={error} label={sample?.metadata.id??"等待标定板样本"} emptyTitle="尚无标定样本" emptyHint="把标定板放在胶路测量平面后点“取新样本”，或导入本工位的离线原图。"/>
    {sample&&<p className="muted mono">{sample.metadata.camera} · {sample.metadata.source==="import"?"离线原图":new Date(sample.metadata.capturedAt).toLocaleString("zh-CN")}{sample.metadata.exposureUs!=null&&" · 曝光 "+sample.metadata.exposureUs+" μs"}</p>}
    {error&&<Notice title="未能取样" tone="warn">{error}</Notice>}
  </Panel>;
}
