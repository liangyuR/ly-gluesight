import { useEffect, useRef, useState } from "react";
import { verdictLabel } from "../history/meta";
import { matchesPart } from "../cycle/identity";
import type { Measured, PartView, PointVis, Recipe } from "../cycle/types";
import { workspaceApi } from "./api";
import { Badge, GrayViewer, KV, Panel, shotOverlay } from "./components";
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

export default function RuntimeFrame({part,layout,k,measured,vis}:{part:PartView|null;layout:Recipe|null;k:number;measured:Measured[];vis?:PointVis[]}) {
  const frame=part?.frames[k];
  const scope=JSON.stringify([part?.cycleId,part?.bundleId,part?.recipeRevision,layout?.id,layout?.revisionId,k,frame?.shotId,frame?.camera,frame?.view,frame?.session,frame?.ordinal,frame?.status]);
  const current=useRef(scope);current.current=scope;
  const [preview,setPreview]=useState<{scope:string;image:GrayImage|null;error:string;loading:boolean}|null>(null);
  useEffect(()=>{
    let alive=true;
    const valid=()=>alive&&current.current===scope;
    if(!part||!layout||part.recipeId!==layout.id||part.recipeRevision!==layout.revisionId)return;
    setPreview({scope,image:null,error:"",loading:true});
    void workspaceApi.liveImage(part.cycleId,part.recipeRevision,k)
      .then(image=>{if(valid())setPreview({scope,image,error:"",loading:false});})
      .catch(error=>{if(valid())setPreview({scope,image:null,error:String(error),loading:false});});
    return()=>{alive=false;};
    // eslint-disable-next-line react-hooks/exhaustive-deps
  },[scope]);
  const image=preview?.scope===scope?preview:null;
  const m=measured.find(m=>m.k===k&&matchesPart(m,part));
  return <Panel className="wp-runtime-frame" title={"选中帧 k"+(k+1)} detail="选帧只改变查看范围，右侧保留整件判定">
    <GrayViewer image={image?.image??null} loading={image?.loading??false} error={image?.error??""} label={"SN "+(part?.sn??"—")+" · k"+(k+1)} overlay={shotOverlay(layout,k,vis)}/>
    <div className="wp-runtime-strip"><KV label="帧状态"><Badge tone={frame?.gapPoints?"ng":frame?.status==="done"?"ok":"warn"}>{frame?.gapPoints?"含缺胶":frame?.status==="done"?"测量完成":frame?.status==="error"?"测量出错":frame?.status==="locateFailed"?"定位失败":frame?.status==="missing"?"未到达":"等待测量"}</Badge></KV><KV label="得分">{m?.score.toFixed(3)??"—"}</KV><KV label="测量点 / 缺胶">{frame ? frame.points+" / "+frame.gapPoints : "—"}</KV><KV label="处理耗时">{m?m.ms+" ms":"—"}</KV></div>
    <div className="wp-runtime-strip"><KV label="本件设备内顺序">{frame?.ordinal??"—"}</KV><KV label="设备帧计数">{frame?.frameCounter??"—"}</KV><KV label="设备触发计数">{frame?.triggerCounter??"—"}</KV></div>
    {!!frame?.viewResults?.length && <div className="table-wrap"><table className="table"><thead><tr><th>检测图</th><th>独立结果</th><th>耗时</th><th>异常明细</th></tr></thead><tbody>{frame.viewResults.map(result=><tr key={result.view}><td>图 {result.view}</td><td>{verdictLabel[result.verdict]}</td><td>{result.ms == null ? "—" : result.ms + " ms"}</td><td>{result.error ?? "—"}</td></tr>)}</tbody></table><p className="muted hint">本点任意选中图 OK 则为 OK；其余图的 NG 和检测异常保留在明细中。采集异常独立处理。</p></div>}
    {frame?.error&&<p className="c-err">{frame.error}</p>}
  </Panel>;
}
