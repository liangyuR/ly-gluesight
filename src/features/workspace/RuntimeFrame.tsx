import { useEffect, useState } from "react";
import type { Measured, PartView, PointVis, Recipe } from "../cycle/types";
import { workspaceApi } from "./api";
import { Badge, GrayViewer, KV, Panel, shotOverlay } from "./components";
import type { GrayImage, Overview } from "./types";

export function usePublishedOverview(layout:Recipe|null) {
  const [overview,setOverview]=useState<Overview|null>(null);
  useEffect(()=>{
    let alive=true;setOverview(null);
    if(layout)void workspaceApi.runtimeOverview(layout.id,layout.hash).then(v=>alive&&setOverview(v)).catch(()=>{});
    return()=>{alive=false;};
  },[layout?.id,layout?.hash]);
  return overview;
}

export default function RuntimeFrame({part,layout,k,measured,vis}:{part:PartView|null;layout:Recipe|null;k:number;measured:Measured[];vis?:PointVis[]}) {
  const [image,setImage]=useState<GrayImage|null>(null);
  const [error,setError]=useState("");
  const [loading,setLoading]=useState(false);
  const frame=part?.frames[k];
  useEffect(()=>{
    let alive=true;setImage(null);setError("");setLoading(false);
    if(!part||!layout||part.recipeHash!==layout.hash)return;
    setLoading(true);
    void workspaceApi.liveImage(part.sn,part.recipeHash,k).then(i=>alive&&setImage(i)).catch(e=>alive&&setError(String(e))).finally(()=>alive&&setLoading(false));
    return()=>{alive=false;};
  },[part?.sn,part?.recipeHash,k,frame?.status]);
  const m=measured.find(m=>m.k===k&&m.sn===part?.sn);
  return <Panel title={"选中帧 k"+(k+1)} detail="选帧只改变查看范围，右侧保留整件判定">
    <GrayViewer image={image} loading={loading} error={error} label={"SN "+(part?.sn??"—")+" · k"+(k+1)} overlay={shotOverlay(layout,k,vis)}/>
    <div className="wp-runtime-strip"><KV label="帧状态"><Badge tone={frame?.gapPoints?"ng":frame?.status==="done"?"ok":"warn"}>{frame?.gapPoints?"含缺胶":frame?.status==="done"?"测量完成":frame?.status==="error"?"测量出错":frame?.status==="locateFailed"?"定位失败":frame?.status==="missing"?"未到达":"等待测量"}</Badge></KV><KV label="得分">{m?.score.toFixed(3)??"—"}</KV><KV label="测量点 / 缺胶">{frame ? frame.points+" / "+frame.gapPoints : "—"}</KV><KV label="处理耗时">{m?m.ms+" ms":"—"}</KV></div>
  </Panel>;
}
