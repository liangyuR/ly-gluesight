import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, Camera, Scale, ScanEye } from "lucide-react";
import { computeVis, shotLabel, ShotTiles, UnrolledCurve, type Measured, type PartView, type Recipe } from "../features/cycle";
import { displayReason, formatTime, historyApi, triggerModeLabel, verdictClass, verdictLabel, type PartDetail } from "../features/history";
import PartTrace from "../features/history/components/PartTrace";
import { subscribe } from "../features/plc";
import { workspaceApi } from "../features/workspace/api";
import { useWorkspace } from "../features/workspace/context";
import { Badge, GrayViewer, Notice, Panel, shotOverlay, useGrayImage } from "../features/workspace/components";
import { WorkpieceOverview } from "../features/workspace/OverviewPage";
import { usePublishedOverview } from "../features/workspace/RuntimeFrame";
import type { Comparison, RecordImages } from "../features/workspace/types";

const comparisonLabel = { rules: "规则重判", raw: "候选原图复测", original: "原发布包重现" };
export default function HistoryDetailPage() {
  const {id}=useParams(), navigate=useNavigate(), ws=useWorkspace();
  const [detail,setDetail]=useState<PartDetail|null>(null),[layout,setLayout]=useState<Recipe|null>(null);
  const [error,setError]=useState(""),[working,setWorking]=useState(false);
  const [comparison,setComparison]=useState<Comparison|null>(null),[raw,setRaw]=useState<RecordImages|null>(null),[selected,setSelected]=useState(0);
  const [viewSelection,setViewSelection]=useState<{k:number;view:number}|null>(null);
  const selectedView=viewSelection?.k===selected?viewSelection.view:layout?.shots[selected]?.view??1;
  const [savedComparisons,setSavedComparisons]=useState<Comparison[]>([]);
  const [visual,setVisual]=useState<"shots"|"part">("shots");
  const pending=useRef(false),savedSerial=useRef(0);
  const scope=`${id??""}:${ws.data?.workspace.doc.id??""}:${ws.data?.workspace.revision??""}`;
  const current=useRef({scope,selected,selectedView,historyId:id,alive:true});
  current.current.scope=scope;current.current.selected=selected;current.current.selectedView=selectedView;current.current.historyId=id;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;savedSerial.current++;};},[]);
  const overview=usePublishedOverview(layout);
  const available=raw?.frames.some(f=>f.k===selected&&f.view===selectedView&&f.available);
  const image=useGrayImage(null,null,available?detail?.summary.id:null,selected,selectedView);
  useEffect(()=>{
    let alive=true;const serial=++savedSerial.current;setSavedComparisons([]);setComparison(null);
    if(detail?.summary.recipeId)void workspaceApi.comparisons(detail.summary.recipeId,detail.summary.id).then(v=>alive&&serial===savedSerial.current&&setSavedComparisons(v)).catch(e=>alive&&serial===savedSerial.current&&setError(String(e)));
    return()=>{alive=false;};
  },[detail?.summary.id,ws.doc?.id,ws.data?.workspace.revision]);

  useEffect(()=>{
    let alive=true;setDetail(null);setLayout(null);setComparison(null);setRaw(null);setError("");setSelected(0);setViewSelection(null);
    void historyApi.detail(Number(id)).then(async d=>{
      if(!alive)return;setDetail(d);
      void workspaceApi.recordImages(d.summary.id).then(v=>alive&&setRaw(v)).catch(e=>alive&&setRaw({historyId:d.summary.id,frames:[],complete:false,message:String(e)}));
      const r=await historyApi.recipe(d.summary.recipeHash,d.summary.recipeId);if(alive)setLayout(r);
    }).catch(e=>alive&&setError(String(e)));
    return()=>{alive=false;};
  },[id]);
  const result=useMemo(()=>{
    if(!detail||!layout)return null;
    const pts=detail.points,idx:number[]=[];pts?.st.forEach((st,j)=>st!==3&&idx.push(j));
    const cycleId=detail.summary.cycleId??`history:${detail.summary.id}`;
    const measured:Measured[]=pts?layout.shots.map((shot,k)=>{
      const owned=idx.filter(j=>layout.points.k[j]===k);
      return {cycleId,shotId:shot.id,camera:shot.camera,bundleHash:detail.summary.bundleHash,sn:detail.summary.sn,k,cam:detail.frames[k]?.cam??0,located:true,score:0,ms:0,error:null,idx:owned,d:owned.map(j=>pts.d[j]),w:owned.map(j=>pts.w?.[j]??null),st:owned.map(j=>pts.st[j]),px:[]};
    }):[];
    const frames=layout.shots.map((shot,k)=>({...detail.frames[k]??{status:"waiting" as const,cam:0,arrivedMs:null,frameCounter:null,triggerCounter:null,counterJump:false,score:null,points:0,gapPoints:0,ms:null,session:null,ordinal:null,error:null},shotId:shot.id,camera:shot.camera,view:shot.view}));
    const part:PartView={cycleId,bundleHash:detail.summary.bundleHash,sn:detail.summary.sn,recipeId:layout.id,recipeHash:layout.hash,n:layout.shots.length,received:detail.summary.framesReceived,triggers:detail.triggers,queue:0,filled:idx.length,total:layout.points.k.length,frames,measuredFrames:detail.frames.length};
    return {measured,vis:computeVis(layout,part,measured,detail.judgement)};
  },[detail,layout]);

  const runCompare=async(fromRaw:boolean)=>{
    if(pending.current||!detail||!ws.data||ws.busy||ws.dirty||ws.frameDirty||ws.data.workspace.doc.id!==layout?.id)return;
    if(fromRaw?(!raw?.complete||!ws.data.workspace.frames.every(f=>f.saved)):(!detail.points?.st.length||!detail.points.st.every(st=>st<2)))return;
    pending.current=true;savedSerial.current++;setWorking(true);setError("");
    const valid=()=>current.current.alive&&current.current.scope===scope;
    try{const result=await workspaceApi.compare(ws.data.workspace.doc.id,ws.data.workspace.revision,detail.summary.id,fromRaw);if(valid()){setComparison(result);setSavedComparisons(v=>[result,...v.filter(c=>c.id!==result.id)]);}}
    catch(e){if(valid())setError(String(e));}finally{pending.current=false;if(current.current.alive)setWorking(false);}
  };
  const runOriginal=async()=>{
    if(pending.current||!detail||!raw?.complete||!detail.summary.bundleHash)return;
    pending.current=true;savedSerial.current++;setWorking(true);setError("");
    const valid=()=>current.current.alive&&current.current.historyId===id;
    try{const result=await workspaceApi.compareOriginal(detail.summary.id);if(valid()){setComparison(result);setSavedComparisons(v=>[result,...v.filter(c=>c.id!==result.id)]);}}
    catch(error){if(valid())setError(String(error));}finally{pending.current=false;if(current.current.alive)setWorking(false);}
  };
  useEffect(()=>{
    let alive=true;
    const stop=subscribe<string>("history://updated",cycleId=>{
      if(!detail||cycleId!==detail.summary.cycleId)return;
      void historyApi.detail(detail.summary.id).then(next=>alive&&setDetail(next)).catch(error=>alive&&setError(String(error)));
      void workspaceApi.recordImages(detail.summary.id).then(next=>alive&&setRaw(next)).catch(error=>alive&&setError(String(error)));
    });
    return()=>{alive=false;stop();};
  },[detail?.summary.id,detail?.summary.cycleId]);
  const useForTeach=async()=>{
    if(pending.current||!detail||!ws.data||ws.busy||ws.dirty||ws.frameDirty||ws.data.workspace.doc.id!==layout?.id||!available||selectedView!==layout?.shots[selected]?.view)return;
    pending.current=true;setWorking(true);setError("");
    const valid=()=>current.current.alive&&current.current.scope===scope&&current.current.selected===selected&&current.current.selectedView===selectedView;
    try{
      const next=await ws.act(()=>workspaceApi.historyCapture(ws.data!.workspace.doc.id,ws.data!.workspace.revision,detail.summary.id,selected),"历史原图已绑定到候选示教；原示教资料可在该帧恢复");
      if(next&&current.current.alive&&current.current.selected===selected&&
        (valid()||current.current.scope===`${id??""}:${next.workspace.doc.id}:${next.workspace.revision}`))navigate("/recipe/teach?frame="+selected);
    }catch(e){if(valid())setError(String(e));}finally{pending.current=false;if(current.current.alive)setWorking(false);}
  };
  if(!detail)return <div className={error?"notice error":"muted"}>{error||"加载中…"}</div>;
  const s=detail.summary,j=detail.judgement;
  const candidateReady=!!ws.data&&ws.data.workspace.doc.id===layout?.id&&!ws.dirty&&!ws.frameDirty;
  const disabled=working||ws.busy||!candidateReady;
  const complete=!!detail.points?.st.length&&detail.points.st.every(st=>st<2);
  const imageLabel="原始 SN "+s.sn+" · k"+(selected+1)+" · 视角 "+selectedView;
  return <div className="stack hist-detail">
    <div className="panel detail-head"><button className="icon-btn" onClick={()=>navigate("/history")} aria-label="返回历史列表"><ArrowLeft size={18}/></button><b className="mono">SN {s.sn}</b><span className="muted">{formatTime(s.ts)} · {s.recipeId??"无配方"} · v{s.recipeVersion??"—"} · #{s.recipeHash?.slice(0,6)??"—"} · {triggerModeLabel(s.triggerMode)} · 帧 {s.framesReceived}/{s.framesExpected}</span><span className="spacer"/><span className={"vt big "+verdictClass(s.verdict)}>{verdictLabel[s.verdict]} · PLC {s.plcCode}{s.faultCode?" / "+s.faultCode:""}</span></div>
    {error&&<Notice title="操作未完成" tone="warn">{error}</Notice>}
    <Panel title="复测与候选对照" detail="原包重现使用本件冻结资源；候选复测使用当前示教；所有结果另存，原始判定保留">
      <div className="wp-actions"><Badge tone="neutral">原始生产 v{s.recipeVersion??"—"}</Badge><Badge>候选 {ws.data&&ws.data.workspace.doc.id===layout?.id?"v"+ws.data.workspace.doc.version+" · 修订 "+ws.data.workspace.revision:"尚未选择此配方"}</Badge><span className="spacer"/>
        <button className="btn" disabled={working||ws.busy||!layout||(!ws.list.some(r=>r.id===layout.id)&&!ws.drafts.some(r=>r.doc.id===layout.id))} onClick={()=>layout&&void ws.select(layout.id)}>使用该配方候选</button>
        <button className="btn" disabled={working||!raw?.complete||!s.bundleHash} onClick={()=>void runOriginal()}><ScanEye size={15}/>按原发布包重现</button>
        <button className="btn" disabled={disabled||!complete} onClick={()=>void runCompare(false)}><Scale size={15}/>按候选规则重判</button>
        <button className="btn primary" disabled={disabled||!raw?.complete||!ws.data?.workspace.frames.every(f=>f.saved)} onClick={()=>void runCompare(true)}><ScanEye size={15}/>从原图复测整件</button>
      </div>
      {ws.dirty||ws.frameDirty?<p className="c-warn">先保存候选配置和示教中线，再执行对照。</p>:!candidateReady&&<p className="muted">选择此配方的候选后，可以调整规则并对照原始结论。</p>}
      {ws.error&&<p className="c-warn">{ws.error}</p>}<p className="muted">{raw?.message??"正在检查原图保存状态…"} · 对照结果单独保存，原始生产记录保留。</p>
      {!!savedComparisons.length&&<label className="field"><span>已保存的对照结果</span><select className="input" aria-label="已保存对照结果" disabled={working} value={comparison?.id??""} onChange={e=>setComparison(savedComparisons.find(c=>c.id===e.target.value)??null)}><option value="">选择记录</option>{savedComparisons.map(c=><option key={c.id} value={c.id}>{formatTime(c.createdAt)} · {comparisonLabel[c.source]} · 修订 {c.candidateRevision} · {verdictLabel[c.judgement.verdict]}</option>)}</select></label>}
    </Panel>
    {comparison&&<Panel title={comparisonLabel[comparison.source]+"结果"} detail={(comparison.source==="original"?"原发布 v"+comparison.candidateRecipe.version+" · 包 "+comparison.bundleHash:"候选 v"+comparison.candidateRecipe.version+" · 修订 "+comparison.candidateRevision)+" · "+formatTime(comparison.createdAt)}>
      <div className="wp-actions"><Badge tone={comparison.originalVerdict.startsWith("OK")?"ok":"ng"}>原始 {verdictLabel[comparison.originalVerdict]}</Badge><span>→</span><Badge tone={comparison.judgement.verdict.startsWith("OK")?"ok":comparison.judgement.verdict==="ERR_INSPECT"?"warn":"ng"}>{comparison.source==="original"?"重现":"候选"} {verdictLabel[comparison.judgement.verdict]}</Badge><b>{displayReason(comparison.judgement.reason)}</b></div>
      <UnrolledCurve layout={comparison.candidateRecipe} measured={comparison.measurements.length?comparison.measurements:(result?.measured??[])} vis={[]}/>
    </Panel>}
    <div className="detail-grid">
      <Panel title="逐拍照点结果与原图" detail="帧选择只改变查看范围，原始整件判定保留" actions={<div className="segmented"><button className={visual==="shots"?"active":""} aria-pressed={visual==="shots"} onClick={()=>setVisual("shots")}>逐拍照点</button><button className={visual==="part"?"active":""} aria-pressed={visual==="part"} onClick={()=>setVisual("part")}>整件</button></div>}>
        {layout&&result?visual==="part"?<WorkpieceOverview layout={layout} overview={overview} selected={selected} onSelect={setSelected} vis={result.vis}/>:<ShotTiles layout={layout} vis={result.vis} selected={selected} onSelect={setSelected}/>:<p className="muted">配方快照缺失，无法绘制。</p>}
        {layout&&<div className="wp-stack"><div className="wp-actions"><Badge>{"选中 k"+(selected+1)}</Badge><select className="input" aria-label="历史帧选择" disabled={working} style={{width:260}} value={selected} onChange={e=>setSelected(Number(e.target.value))}>{layout.shots.map((shot,k)=><option key={k} value={k}>k{k+1} · {shotLabel(shot,k)} · {raw?.frames.some(f=>f.k===k&&f.available)?"原图可用":"无原图"}</option>)}</select><select className="input" aria-label="历史视角选择" disabled={working} value={selectedView} onChange={e=>setViewSelection({k:selected,view:Number(e.target.value)})}>{[...new Set([layout.shots[selected]?.view??1,...(raw?.frames.filter(f=>f.k===selected).map(f=>f.view)??[])])].sort().map(view=><option key={view} value={view}>视角 {view}{view===layout.shots[selected]?.view?" · 检测所选":""}</option>)}</select><span className="spacer"/><button className="btn" disabled={disabled||!available||selectedView!==layout.shots[selected]?.view} onClick={()=>void useForTeach()}><Camera size={15}/>将此帧用于示教</button></div><GrayViewer image={image.image} loading={image.loading} error={image.error||(!available?"本帧原图未保存或已按保留策略清理；完整测量数据仍可用于规则重判":"")} label={imageLabel} overlay={selectedView===layout.shots[selected]?.view?shotOverlay(layout,selected,result?.vis):undefined}/></div>}
      </Panel>
      <Panel title="原始整件判定" className="side"><p className="reason-box">{displayReason(j.reason)}</p>
        {j.gaps.map((g,i)=><div key={i} className="ng-item"><div className="ng-title c-ng">断胶 · {layout?.segments[g.segment]?.name??"段 "+g.segment}</div><p>段内 s {g.s0.toFixed(1)} – {g.s1.toFixed(1)} mm · 长度 {g.len.toFixed(1)} mm（允许 ≤ {layout?.segments[g.segment]?.maxGapLen??"—"} mm）</p><div className="wp-actions">{g.frames.map(k=><button className="btn small" key={k} onClick={()=>setSelected(k)}>k{k+1}{layout?.shots[k]?" · "+layout.shots[k].id:""}</button>)}</div></div>)}
        {j.segments.map((r,i)=><div key={i} className="ng-item"><div className={"ng-title "+(r.verdict.startsWith("OK")?"c-ok":"c-ng")}>{verdictLabel[r.verdict]} · {layout?.segments[i]?.name??"段 "+i}</div><div className="kv2"><span>横向偏移</span><b>{r.min?.toFixed(2)??"—"} – {r.max?.toFixed(2)??"—"} mm</b><span>连续超差</span><b>{r.excursionLen.toFixed(1)} mm</b>{r.wMin!=null&&<><span>胶宽</span><b>{r.wMin.toFixed(2)} – {r.wMax?.toFixed(2)} mm</b><span>胶宽连续超差</span><b>{(r.wExcursionLen??0).toFixed(1)} mm</b></>}</div></div>)}
        <div className="kv2"><span>记录号</span><b>#{s.id}</b><span>配置哈希</span><b>{s.recipeHash?.slice(0,12)??"—"}</b><span>软件版本</span><b>{detail.softwareVersion}</b><span>收尾耗时</span><b>{s.drainMs??"—"} ms</b><span>触发计数</span><b>{detail.triggers}</b><span>复检自</span><b>{s.retestOf?<Link to={"/history/"+s.retestOf}>#{s.retestOf}</Link>:"—"}</b><span>后续复检</span><b>{detail.retests.length?detail.retests.map(r=><Link key={r} to={"/history/"+r}>#{r} </Link>):"—"}</b></div>
      </Panel>
      <Panel title="原始测量曲线" className="curve-panel" detail="原始横向偏移、胶宽和原版公差，按拍照点分段">
        {layout&&result&&<UnrolledCurve layout={layout} measured={result.measured} vis={result.vis}/>}
      </Panel>
      <PartTrace detail={detail} layout={layout} raw={raw} selected={selected} selectedView={selectedView} onSelect={(k,view)=>{setSelected(k);setViewSelection({k,view});}}/>
    </div>
  </div>;
}
