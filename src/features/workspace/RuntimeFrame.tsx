import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState } from "react";
import { verdictLabel } from "../history/meta";
import { matchesPart } from "../cycle/identity";
import type { Measured, PartView, PointVis, Recipe } from "../cycle/types";
import { workspaceApi } from "./api";
import { GrayViewer, KV, Panel, shotOverlay } from "./components";
import type { GrayImage, Overview } from "./types";

export function usePublishedOverview(layout:Recipe|null) {
  const [overview,setOverview]=useState<Overview|null>(null);
  useEffect(()=>{
    let alive=true;setOverview(null);
    if(layout)void workspaceApi.runtimeOverview(layout.id,layout.revisionId).then(v=>alive&&setOverview(v)).catch(()=>{});
    return()=>{alive=false;};
  },[layout?.id,layout?.revisionId]);
  return overview;
}

interface LiveRequest { scope:string; part:string; cycleId:string; revisionId:string; k:number }
interface LiveResult { scope:string; part:string; k:number; image:GrayImage|null; error:string }

/** 同一时间只读一张：读图期间选帧又变了，读完后只补读最新的一张，中间的跳过；新图到之前保留本件上一张。 */
function useLiveImage(request:LiveRequest|null) {
  const [result,setResult]=useState<LiveResult|null>(null);
  const [shown,setShown]=useState<LiveResult|null>(null);
  const wanted=useRef<LiveRequest|null>(null);
  const busy=useRef(false);
  const alive=useRef(true);
  wanted.current=request;
  useEffect(()=>{alive.current=true;return()=>{alive.current=false;};},[]);
  const read=()=>{
    const job=wanted.current;
    if(busy.current||!job||!alive.current)return;
    busy.current=true;
    void workspaceApi.liveImage(job.cycleId,job.revisionId,job.k)
      .then(image=>({image,error:""}),error=>({image:null,error:errorText(error)}))
      .then(({image,error})=>{
        busy.current=false;
        if(!alive.current)return;
        const done={scope:job.scope,part:job.part,k:job.k,image,error};
        setResult(done);
        if(image)setShown(done);
        if(wanted.current&&wanted.current.scope!==job.scope)read();
      });
  };
  useEffect(read,[request?.scope]);
  const current=request&&result?.scope===request.scope?result:null;
  const held=!current&&request&&shown?.part===request.part?shown:null;
  return {current,held,loading:!!request&&!current};
}

export default function RuntimeFrame({part,layout,k,measured,vis,compact=false,onDiagnostics}:{part:PartView|null;layout:Recipe|null;k:number;measured:Measured[];vis?:PointVis[];compact?:boolean;onDiagnostics?:()=>void}) {
  const frame=part?.frames[k];
  const arrived=!!frame&&frame.status!=="waiting"&&frame.status!=="missing";
  const partScope=JSON.stringify([part?.cycleId,part?.bundleId,part?.recipeRevision,layout?.id,layout?.revisionId]);
  const scope=JSON.stringify([partScope,k,frame?.shotId,frame?.camera,frame?.view,frame?.session,frame?.ordinal,arrived]);
  const matched=!!part&&!!layout&&part.recipeId===layout.id&&part.recipeRevision===layout.revisionId;
  const live=useLiveImage(matched?{scope,part:partScope,cycleId:part.cycleId,revisionId:part.recipeRevision,k}:null);
  const image=live.current?.image?live.current:live.held;
  const m=measured.find(m=>m.k===k&&matchesPart(m,part));
  const attention=frame?.viewResults?.filter(result=>result.verdict!=="OK"||!!result.error).length??0;
  const frameState:[string,string]=frame?.gapPoints?["含缺胶","ng"]:frame?.status==="done"?["测量完成","ok"]:frame?.status==="error"?["测量出错","fault"]:frame?.status==="locateFailed"?["定位失败","fault"]:frame?.status==="missing"?["未到达","fault"]:frame?.status==="measuring"?["测量中",""]:["等待测量",""];
  const labelOf=(i:number)=>{
    const f=part?.frames[i],s=layout?.shots[i];
    return compact?[f?.shotId??s?.id,s?.poseId&&s.poseId!==s.id?"Pose "+s.poseId:null,f?.camera??s?.camera,"视角 "+(f?.view??s?.view??"—"),"k"+(i+1)].filter(Boolean).join(" · "):"SN "+(part?.sn||"—")+" · k"+(i+1);
  };
  const shownK=image?.k??k;
  const contents=<>
    <GrayViewer image={image?.image??null} loading={live.loading} error={live.current?.error??""} label={labelOf(shownK)} overlay={shotOverlay(layout,shownK,vis)} emptyTitle={compact?"等待本拍照点图像":undefined} emptyHint={compact?"":"本拍照点采集完成后显示原图；可在下方切换拍照点查看。"}/>
    <div className="wp-runtime-strip"><KV label="帧状态"><span className={"qs-chip frame-state "+frameState[1]}>{frameState[0]}</span></KV><KV label="得分">{m?.score.toFixed(3)??"—"}</KV><KV label="测量点 / 缺胶">{frame ? frame.points+" / "+frame.gapPoints : "—"}</KV><KV label="处理耗时">{m?m.ms+" ms":"—"}</KV></div>
    {!compact&&<div className="wp-runtime-strip"><KV label="本件设备内顺序">{frame?.ordinal??"—"}</KV><KV label="设备帧计数">{frame?.frameCounter??"—"}</KV><KV label="设备触发计数">{frame?.triggerCounter??"—"}</KV></div>}
    {compact&&attention>0&&onDiagnostics&&<button className="btn small runtime-view-attention" onClick={onDiagnostics}>检测图异常 / 注意 {attention}</button>}
    {(!compact||!onDiagnostics)&&!!frame?.viewResults?.length && <div className="table-wrap"><table className="table"><thead><tr><th>检测图</th><th>独立结果</th><th>耗时</th><th>异常明细</th></tr></thead><tbody>{frame.viewResults.map(result=><tr key={result.view}><td>图 {result.view}</td><td>{verdictLabel[result.verdict]}</td><td>{result.ms == null ? "—" : result.ms + " ms"}</td><td>{result.error ?? "—"}</td></tr>)}</tbody></table><p className="muted hint">本点任意选中图 OK 则为 OK；其余图的 NG 和检测异常保留在明细中。采集异常独立处理。</p></div>}
    {frame?.error&&<p className="c-err">{frame.error}</p>}
  </>;
  return compact?<div className="wp-runtime-frame compact">{contents}</div>:<Panel className="wp-runtime-frame" title={"选中帧 k"+(k+1)} detail="选帧只改变查看范围，右侧保留整件判定">{contents}</Panel>;
}
