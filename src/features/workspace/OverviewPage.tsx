import { useEffect, useRef, useState, type PointerEvent } from "react";
import { Link } from "react-router-dom";
import { Save, Upload } from "lucide-react";
import type { PointVis, Recipe } from "../cycle/types";
import { shotLabel, shotState, shotStateLabel, teachStatus, visColor } from "../cycle/vis";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, GrayViewer, KV, Notice, Panel, shotOverlay, Steps, useGrayImage, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { Overview } from "./types";

/** 默认显示位置：拍照点按计划顺序排成均匀网格（归一化到 0–1）。工件坐标里没有拍照点位置，显示位置由人工拖到背景图上。 */
export function defaultPositions(layout: Pick<Recipe, "shots">): [number,number][] {
  const n = layout.shots.length;
  const cols = Math.max(1, Math.min(n, Math.ceil(Math.sqrt(n * 520 / 230))));
  const rows = Math.max(1, Math.ceil(n / cols));
  return layout.shots.map((_, i) => [(i % cols + .5) / cols, (Math.floor(i / cols) + .5) / rows]);
}
export function WorkpieceOverview({layout,overview,selected=0,onSelect,onMove,vis}:{
  layout:Recipe; overview:Overview|null; selected?:number; onSelect?:(k:number)=>void;
  onMove?:(k:number,position:[number,number])=>void; vis?:PointVis[];
}) {
  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<{k:number;start:[number,number];position:[number,number]} | null>(null);
  const positions = overview?.positions.length === layout.shots.length ? overview.positions : defaultPositions(layout);
  const point = (e:PointerEvent<SVGGElement>):[number,number] | null => {
    const matrix = svg.current?.getScreenCTM();
    if (!matrix) return null;
    const p = new DOMPoint(e.clientX,e.clientY).matrixTransform(matrix.inverse());
    return [p.x,p.y];
  };
  return <svg ref={svg} className={"wp-overview " + (onMove ? "editable" : "")} viewBox="0 0 800 420" aria-label="工件总览，选择帧查看原图">
    <rect width={800} height={420} fill="var(--bg)" />
    {overview?.background && <image href={overview.background} x={110} y={46} width={580} height={292} preserveAspectRatio="xMidYMid meet" opacity={.85} />}
    {positions.map(([x,y],k) => {
      const shot = layout.shots[k];
      const state = vis ? shotState(layout, vis, k) : null;
      const tone = state && state !== "skip" && state !== "untaught" && state !== "none" ? visColor[state] : null;
      const on = selected === k;
      return <g key={k} role="button" tabIndex={0} aria-label={"总览选择帧 k" + (k+1)} aria-pressed={on} data-state={state ?? undefined}
      onPointerDown={e => {onSelect?.(k);const p=point(e);if(onMove&&p){e.currentTarget.setPointerCapture(e.pointerId);drag.current={k,start:p,position:[x,y]};}}} onPointerMove={e => {
        const p=point(e),d=drag.current;if(!p||!d||!onMove)return;
        onMove(d.k,[Math.min(1,Math.max(0,d.position[0]+(p[0]-d.start[0])/520)),Math.min(1,Math.max(0,d.position[1]+(p[1]-d.start[1])/230))]);
      }} onPointerUp={() => {drag.current=null;}} onPointerCancel={() => {drag.current=null;}} onClick={() => onSelect?.(k)} onKeyDown={e => {
        if(e.key==="Enter"||e.key===" "){e.preventDefault();onSelect?.(k);}
        if(onMove&&e.key.startsWith("Arrow")){e.preventDefault();const step=e.shiftKey ? 0.04 : 0.01;onMove(k,[Math.min(1,Math.max(0,x+(e.key==="ArrowRight"?step:e.key==="ArrowLeft"?-step:0))),Math.min(1,Math.max(0,y+(e.key==="ArrowDown"?step:e.key==="ArrowUp"?-step:0)))]);}
      }}>
      <rect x={140+x*520-57} y={74+y*230-38} width={114} height={76} rx={5} fill={on ? "var(--accent-soft)" : "var(--bg-panel)"} fillOpacity={.84} stroke={on?"var(--accent)":tone ?? "var(--border-strong)"} strokeWidth={on||tone?2:1} strokeDasharray={shot?.skip||state==="untaught"?"5 4":undefined} />
      <text x={140+x*520} y={74+y*230-6} textAnchor="middle" fill={on?"var(--accent-text)":"var(--text-muted)"} fontSize={15}>k{k+1}</text>
      <text x={140+x*520} y={74+y*230+14} textAnchor="middle" fill="var(--text-muted)" fontSize={11} className="mono">{shotLabel(shot,k)}</text>
      {state && <text x={140+x*520} y={74+y*230+30} textAnchor="middle" fill={tone ?? "var(--text-muted)"} fontSize={11}>{shotStateLabel[state]}</text>}
    </g>;})}
    <text x={400} y={373} textAnchor="middle" fill="var(--text-muted)" fontSize={12}>{onMove ? "拖动帧框或用方向键微调显示位置；拍照与测量不受影响" : "显示布置与生产配方快照绑定"}</text>
  </svg>;
}

export default function OverviewPage() {
  const {data,doc,dirty,busy,act,setError} = useWorkspace();
  const [overview,setOverview] = useState<Overview>({background:null,positions:[],saved:false});
  const [selected,setSelected] = useState(0);
  const [reading,setReading] = useState(false);
  const [saving,setSaving]=useState(false);
  const importSerial=useRef(0),saveSerial=useRef(0),readingRef=useRef(false),savingRef=useRef(false);
  const scope=(data?.workspace.doc.id??"")+":"+(data?.workspace.revision??0);
  const current=useRef({scope,alive:true});current.current.scope=scope;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;importSerial.current++;saveSerial.current++;};},[]);
  useEffect(() => {
    importSerial.current++;saveSerial.current++;readingRef.current=false;savingRef.current=false;setReading(false);setSaving(false);
    if (data) {const matches=data.workspace.overview.positions.length===data.layout.shots.length;setOverview({...data.workspace.overview,saved:matches&&data.workspace.overview.saved,positions:matches ? data.workspace.overview.positions : defaultPositions(data.layout)});setSelected(0);}
  },[data?.workspace.revision,data?.workspace.doc.id]);
  const frame = data?.workspace.frames[selected];
  const {image,error,loading} = useGrayImage(doc?.id ?? null,frame?.image?.id ?? null);
  if(!data||!doc)return <WorkspaceEmpty />;
  const displayDirty = JSON.stringify(overview) !== JSON.stringify({...data.workspace.overview,positions:data.workspace.overview.positions.length===data.layout.shots.length ? data.workspace.overview.positions : defaultPositions(data.layout)});
  const unlocked=!busy&&!dirty&&!reading&&!saving;
  const importFile = (file:File) => {
    if(!unlocked||readingRef.current||savingRef.current)return;
    if(!file.size || file.size>1_000_000 || !["image/png","image/jpeg","image/webp"].includes(file.type))return setError("请导入非空且 1 MB 以内的 PNG、JPEG、WebP 总览背景");
    const serial=++importSerial.current;readingRef.current=true;setReading(true);
    const valid=()=>current.current.alive&&current.current.scope===scope&&serial===importSerial.current;
    const finish=(error?:string)=>{if(!valid())return;if(error)setError(error);readingRef.current=false;setReading(false);};
    try{
      const reader=new FileReader();
      reader.onload=()=>{
        if(!valid())return;
        const background=reader.result;
        if(typeof background!=="string"||!/^data:image\/(png|jpeg|webp);base64,.+/.test(background)){finish("总览背景不是可读取的图像");return;}
        const decoded=new Image();
        decoded.onload=()=>{if(!valid())return;if(!decoded.naturalWidth||!decoded.naturalHeight){finish("总览背景不是可读取的图像");return;}setOverview(v=>({...v,background,saved:false}));finish();};
        decoded.onerror=()=>finish("总览背景不是可读取的图像");
        decoded.src=background;
      };
      reader.onerror=()=>finish("总览图读取失败");reader.onabort=()=>finish("总览图读取已取消");
      reader.readAsDataURL(file);
    }catch{finish("总览图读取失败");}
  };
  const save=async()=>{
    if(!unlocked||readingRef.current||savingRef.current)return;
    savingRef.current=true;setSaving(true);const serial=++saveSerial.current;
    const valid=()=>current.current.alive&&current.current.scope===scope&&serial===saveSerial.current;
    try{await act(()=>workspaceApi.saveOverview(doc.id,data.workspace.revision,overview),"总览布置已保存，拍照与测量不受影响");}
    catch(e){if(valid())setError(String(e));}
    finally{if(valid()){savingRef.current=false;setSaving(false);}}
  };
  return <div className="wp-page"><WorkspaceBar /><Steps /><div className="wp-actions"><Badge tone={data.workspace.overview.saved&&!displayDirty?"ok":"warn"}>{data.workspace.overview.saved&&!displayDirty?"总览已保存":"总览待保存"}</Badge><span className="spacer" /><Link className="btn" to="/recipe/validation">验证与发布</Link></div><div className="wp-columns"><Panel title="工件总览" detail="在背景图上布置各拍照点的显示框；点击帧框查看绑定原图" actions={<button className="btn primary" disabled={!unlocked} onClick={()=>void save()}><Save size={15}/>保存总览</button>}>
      <WorkpieceOverview layout={data.layout} overview={overview} selected={selected} onSelect={setSelected} onMove={unlocked ? (k,p)=>setOverview(v=>({...v,saved:false,positions:v.positions.map((previous,i)=>i===k?p:previous)})) : undefined}/>
      <div className="wp-actions" style={{marginTop:14}}><label className="btn"><Upload size={15}/>{reading?"正在读取总览图…":"导入总览图"}<input type="file" aria-label="导入总览图" hidden accept="image/png,image/jpeg,image/webp" disabled={!unlocked} onChange={e=>{if(e.target.files?.[0])void importFile(e.target.files[0]);e.target.value="";}}/></label><button className="btn" disabled={!unlocked} onClick={()=>setOverview(v=>({...v,saved:false,positions:defaultPositions(data.layout)}))}>自动布置</button>{overview.background&&<button className="btn" disabled={!unlocked} onClick={()=>setOverview(v=>({...v,background:null,saved:false}))}>移除背景</button>}</div>
    </Panel><div className="wp-stack"><Panel title={"选中 k"+(selected+1)+" · 原图"}><GrayViewer image={image} loading={loading} error={error} label={frame?.image?.id??"尚未取样"} overlay={shotOverlay(data.layout,selected)}/></Panel><Panel title="拍照点"><KV label="胶路示教">{data.coverage.toFixed(0)}%</KV><KV label="拍照点">{data.layout.shots.length} 个</KV><KV label="编号 · 相机">{shotLabel(data.layout.shots[selected],selected)}</KV><KV label="胶条">{data.layout.shots[selected]?.bead ?? "—"}</KV><KV label="Pose">{data.layout.shots[selected]?.poseId ?? "—"}</KV><KV label="示教状态">{data.layout.shots[selected] ? teachStatus(data.layout.shots[selected]) : "—"}</KV></Panel><Notice title="显示布置独立保存">拖动帧框只改变显示位置，不影响机器人拍照位置和示教中线。</Notice></div></div></div>;
}
