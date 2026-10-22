import { imageFailure } from "../../lib/pendingBackend";
import { useEffect, useRef, useState } from "react";
import { Button, Chip, Table, type TableColumn } from "qing-steel";
import { verdictLabel } from "../history/meta";
import { matchesPart } from "../cycle/identity";
import { referenceBadge } from "../cycle/vis";
import { framePose, partFrames, poseSummary, viewSpec } from "../cycle/pose";
import type { Measured, PartView, PointVis, Recipe } from "../cycle/types";
import type { FrameGroupView } from "../plan/types";
import { groupCellState, groupCellText, type GroupCellState } from "../plan/vis";
import { workspaceApi } from "./api";
import { burstImageApi } from "./burstImages";
import { GrayViewer, KV, Panel, shotOverlay } from "./components";
import "../cycle/components/burst.css";
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

type ViewResult = NonNullable<PartView["frames"][number]["viewResults"]>[number];
const viewColumns: TableColumn<ViewResult>[] = [
  { key: "view", header: "检测图", render: result => "图 " + result.view },
  { key: "verdict", header: "独立结果", render: result => verdictLabel[result.verdict] },
  { key: "ms", header: "耗时", render: result => result.ms == null ? "—" : result.ms + " ms" },
  { key: "error", header: "异常明细", render: result => result.error ?? "—" },
];

/** 一次读图请求：scope 变了就要读新图，part 相同的上一张在新图到之前保留。 */
interface LiveJob { scope:string; part:string }
interface LiveRequest extends LiveJob { cycleId:string; revisionId:string; k:number }
interface LiveResult<R extends LiveJob> { scope:string; part:string; job:R; image:GrayImage|null; error:string; pending:string }
const loadShot=(job:LiveRequest)=>workspaceApi.liveImage(job.cycleId,job.revisionId,job.k);

/** 同一时间只读一张：读图期间选帧又变了，读完后只补读最新的一张，中间的跳过；新图到之前保留本件上一张。 */
function useLiveImage<R extends LiveJob>(request:R|null,load:(job:R)=>Promise<GrayImage>) {
  const [result,setResult]=useState<LiveResult<R>|null>(null);
  const [shown,setShown]=useState<LiveResult<R>|null>(null);
  const wanted=useRef<R|null>(null);
  const busy=useRef(false);
  const alive=useRef(true);
  wanted.current=request;
  useEffect(()=>{alive.current=true;return()=>{alive.current=false;};},[]);
  const read=()=>{
    const job=wanted.current;
    if(busy.current||!job||!alive.current)return;
    busy.current=true;
    void load(job)
      .then(image=>({image,error:"",pending:""}),error=>({image:null,...imageFailure(error)}))
      .then(({image,error,pending})=>{
        busy.current=false;
        if(!alive.current)return;
        const done={scope:job.scope,part:job.part,job,image,error,pending};
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
  const frames=partFrames(layout,part);
  const matched=!!part&&!!frames;
  const live=useLiveImage(matched?{scope,part:partScope,cycleId:part.cycleId,revisionId:part.recipeRevision,k}:null,loadShot);
  const image=live.current?.image?live.current:live.held;
  const m=measured.find(m=>m.k===k&&matchesPart(m,part));
  const attention=frame?.viewResults?.filter(result=>result.verdict!=="OK"||!!result.error).length??0;
  const frameState:[string,string]=frame?.gapPoints?["含缺胶","ng"]:frame?.status==="done"?["测量完成","ok"]:frame?.status==="error"?["测量出错","fault"]:frame?.status==="missing"?["未到达","fault"]:frame?.status==="measuring"?["测量中",""]:["等待测量",""];
  const reference=referenceBadge(frame?.referenceFailed,(frame?.referenceFailed?.length??0)>1);
  const labelOf=(i:number)=>{
    const f=part?.frames[i],s=layout?.shots[i];
    return compact?[f?.shotId??s?.id,s?.poseId&&s.poseId!==s.id?"Pose "+s.poseId:null,f?.camera??s?.camera,"视角 "+(f?.view??s?.view??"—"),"k"+(i+1)].filter(Boolean).join(" · "):"SN "+(part?.sn||"—")+" · k"+(i+1);
  };
  const shownK=image?.job.k??k;
  // 位姿只和测出它的快照框配对：件与布局的修订不同时不按位姿叠图
  const shownFrame=frames?.[shownK]??null;
  const shownShot=layout?.shots[shownK];
  const located=!!shownShot&&!!viewSpec(shownShot,shownShot.view)?.fixture;
  const pose=located?framePose(shownFrame,shownShot.view):null;
  const contents=<>
    <GrayViewer image={image?.image??null} loading={live.loading} error={live.current?.error??""} label={labelOf(shownK)} caption={compact?undefined:<>SN <span className="sn">{part?.sn||"—"}</span> · k{shownK+1}</>} overlay={shotOverlay(layout,shownK,vis,shownFrame)} emptyTitle={compact?"等待本拍照点图像":undefined} emptyHint={compact?"":"本拍照点采集完成后显示原图；可在下方切换拍照点查看。"}/>
    <div className="wp-runtime-strip"><KV label="帧状态"><Chip className={"frame-state "+frameState[1]}>{frameState[0]}</Chip>{reference&&<Chip className="frame-state fault" title="该视图没有位置基准：有胶站不判位置，胶宽与断胶照判">{reference}</Chip>}</KV>{located&&<KV label="定位" className={pose&&!pose.accepted?"c-err":undefined} title={"特征定位："+poseSummary(pose)}>{poseSummary(pose,compact)}</KV>}<KV label="得分">{m?.score.toFixed(3)??"—"}</KV><KV label="测量点 / 缺胶">{frame ? frame.points+" / "+frame.gapPoints : "—"}</KV><KV label="处理耗时">{m?m.ms+" ms":"—"}</KV></div>
    {!compact&&<div className="wp-runtime-strip"><KV label="本件设备内顺序">{frame?.ordinal??"—"}</KV><KV label="设备帧计数">{frame?.frameCounter??"—"}</KV><KV label="设备触发计数">{frame?.triggerCounter??"—"}</KV></div>}
    {compact&&attention>0&&onDiagnostics&&<Button size="sm" className="runtime-view-attention" onClick={onDiagnostics}>检测图异常 / 注意 {attention}</Button>}
    {(!compact||!onDiagnostics)&&!!frame?.viewResults?.length && <div><Table caption="检测图独立结果" columns={viewColumns} rows={frame.viewResults} rowKey={result=>String(result.view)} /><p className="muted hint">本点任意选中图 OK 则为 OK；其余图的 NG 和检测异常保留在明细中。采集异常独立处理。</p></div>}
    {frame?.error&&<p className="c-err">{frame.error}</p>}
  </>;
  return compact?<div className="wp-runtime-frame compact">{contents}</div>:<Panel className="wp-runtime-frame" title={"选中帧 k"+(k+1)} detail="选帧只改变查看范围，右侧保留整件判定">{contents}</Panel>;
}

// ───────────── 段配方（分段连拍飞拍）：选中帧组的三台相机原图 ─────────────
// 旧拍照点模型的 RuntimeFrame 保持原样；段配方按 (相机, seq) 读图，没有本件设备内顺序，也没有“任一图 OK 即 OK”——每台相机每帧单独判。

interface BurstLiveRequest extends LiveJob { cycleId:string; revisionId:string; camera:number; seq:number }
const loadBurstFrame=(job:BurstLiveRequest)=>burstImageApi.live(job.cycleId,job.revisionId,job.camera,job.seq);
const burstStateTone:Partial<Record<GroupCellState,string>>={ ok:"ok", ng:"ng", error:"fault" };

/** 一台相机在选中帧组里的那一帧：到达（含测量中、出错）才读图，缺帧的相机不读图，只写明本组按其余相机判。 */
function BurstCameraFrame({part,camera,name,seq,group}:{part:PartView;camera:number;name:string;seq:number;group:FrameGroupView|null}) {
  const cell=group?.cells[camera];
  const arrived=!!cell&&cell.status!=="waiting"&&cell.status!=="missing";
  const partScope=JSON.stringify([part.cycleId,part.bundleId,part.recipeRevision]);
  const scope=JSON.stringify([partScope,camera,seq,arrived]);
  const live=useLiveImage(arrived?{scope,part:partScope,cycleId:part.cycleId,revisionId:part.recipeRevision,camera,seq}:null,loadBurstFrame);
  const image=live.current?.image?live.current:live.held;
  const state:GroupCellState=group?groupCellState(group,camera):"waiting";
  const shownSeq=image?.job.seq??seq;
  const missing=state==="missing";
  const pending=live.current?.pending??"";
  return <section className="wp-burst-cam" aria-label={`${name} 帧组 ${seq}`}>
    <GrayViewer image={arrived?image?.image??null:null} loading={arrived&&live.loading} error={live.current?.error??""} label={`${name} · 帧组 ${shownSeq}`}
      emptyTitle={missing?"缺帧":pending?"原图读取待后端实现":"等待本帧组图像"} emptyHint={missing?"这台相机没有出本帧组的图，本组按其余相机判（检测不完整，不影响 OK/NG）。":pending}/>
    <div className="wp-runtime-strip">
      <KV label="帧状态"><Chip className={"frame-state "+(burstStateTone[state]??"")}>{groupCellText[state]}</Chip></KV>
      <KV label="单帧结论">{cell?.verdict?verdictLabel[cell.verdict]:"—"}</KV>
      <KV label="设备帧计数">{cell?.frameCounter??"—"}</KV>
    </div>
  </section>;
}

/** 在线检测的原图区（段配方）：选中帧组 seq 的三台计划相机各一幅，每幅附该帧的状态、单帧结论与设备帧计数。 */
export function BurstRuntimeFrame({part,seq}:{part:PartView|null;seq:number|null}) {
  const burst=part?.burst;
  if(!part||!burst||seq==null)return <div className="wp-runtime-frame compact"><div className="wp-image-empty"><strong>等待本件图像</strong><span>帧组到达后显示三台相机的原图；可在下方帧组条切换。</span></div></div>;
  const group=burst.groups.find(g=>g.seq===seq)??null;
  return <div className="wp-runtime-frame compact burst">
    <div className="wp-burst-cams" style={{gridTemplateColumns:`repeat(${Math.max(1,burst.cameras.length)}, minmax(0, 1fr))`}}>{burst.cameras.map((name,camera)=><BurstCameraFrame key={name} part={part} camera={camera} name={name} seq={seq} group={group}/>)}</div>
  </div>;
}
