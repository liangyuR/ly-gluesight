import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState } from "react";
import { Chip } from "qing-steel";
import { verdictLabel } from "../history/meta";
import type { PartView } from "../cycle/types";
import type { FrameGroupView } from "../plan/types";
import { groupCellState, groupCellText, type GroupCellState } from "../plan/vis";
import { burstImageApi } from "./burstImages";
import { GrayViewer, KV } from "./components";
import "../cycle/components/burst.css";
import type { GrayImage } from "./types";

/** 一次读图请求：scope 变了就要读新图，part 相同的上一张在新图到之前保留。 */
interface LiveJob { scope:string; part:string }
interface LiveResult<R extends LiveJob> { scope:string; part:string; job:R; image:GrayImage|null; error:string }

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
      .then(image=>({image,error:""}),error=>({image:null,error:errorText(error)}))
      .then(({image,error})=>{
        busy.current=false;
        if(!alive.current)return;
        const done={scope:job.scope,part:job.part,job,image,error};
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

// ───────────── 选中帧组的三台相机原图 ─────────────

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
  return <section className="wp-burst-cam" aria-label={`${name} 帧组 ${seq}`}>
    <GrayViewer image={arrived?image?.image??null:null} loading={arrived&&live.loading} error={live.current?.error??""} label={`${name} · 帧组 ${shownSeq}`}
      emptyTitle={missing?"缺帧":"等待本帧组图像"} emptyHint={missing?"这台相机没有出本帧组的图，本组按其余相机判（检测不完整，不影响 OK/NG）。":""}/>
    <div className="wp-runtime-strip">
      <KV label="帧状态"><Chip className={"frame-state "+(burstStateTone[state]??"")}>{groupCellText[state]}</Chip></KV>
      <KV label="单帧结论">{cell?.verdict?verdictLabel[cell.verdict]:"—"}</KV>
      <KV label="设备帧计数">{cell?.frameCounter??"—"}</KV>
    </div>
  </section>;
}

/** 在线检测的原图区（段配方）：选中帧组 seq 的三台计划相机各一幅，每幅附该帧的状态、单帧结论与设备帧计数。 */
export function BurstRuntimeFrame({part,seq}:{part:PartView|null;seq:number|null}) {
  if(!part||seq==null)return <div className="wp-runtime-frame compact"><div className="wp-image-empty"><strong>等待本件图像</strong><span>帧组到达后显示三台相机的原图；可在下方帧组条切换。</span></div></div>;
  const burst=part.burst;
  const group=burst.groups.find(g=>g.seq===seq)??null;
  return <div className="wp-runtime-frame compact burst">
    <div className="wp-burst-cams" style={{gridTemplateColumns:`repeat(${Math.max(1,burst.cameras.length)}, minmax(0, 1fr))`}}>{burst.cameras.map((name,camera)=><BurstCameraFrame key={name} part={part} camera={camera} name={name} seq={seq} group={group}/>)}</div>
  </div>;
}
