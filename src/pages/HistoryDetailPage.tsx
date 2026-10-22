import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, Camera, ChevronLeft, ChevronRight, Scale, ScanEye } from "lucide-react";
import { Alert, Button, Card, Chip, Dialog, IconButton, SegmentedControl, Select, Stack, StatusBadge, Text } from "qing-steel";
import { computeVis, partReferenceBadge, shotLabel, ShotTiles, UnrolledCurve, type Measured, type PartView, type Recipe } from "../features/cycle";
import { displayReason, formatTime, historyApi, sampleLabelText, sourceLabel, triggerModeLabel, verdictLabel, verdictTone, type HistoryQuery, type PartDetail } from "../features/history";
import BurstHistoryDetail from "../features/history/components/BurstDetail";
import PartTrace from "../features/history/components/PartTrace";
import ReplayDiff from "../features/history/components/ReplayDiff";
import { poseDiffers, replayEdges, replayPose, rulesRejudgeable, shotDiffs } from "../features/history/replay";
import { framePose, movedBox, poseSummary, poseTransform, viewSpec } from "../features/cycle/pose";
import { subscribe } from "../features/plc";
import { workspaceApi } from "../features/workspace/api";
import { useWorkspace } from "../features/workspace/context";
import { GrayViewer, Panel, shotOverlay, useGrayImage } from "../features/workspace/components";
import { WorkpieceOverview } from "../features/workspace/OverviewPage";
import { usePublishedOverview } from "../features/workspace/RuntimeFrame";
import type { Comparison, RecordImages, WorkspaceView } from "../features/workspace/types";
import { CycleTimeline } from "../features/logs";
import { errorText } from "../lib/errors";

const candidateTaught=(v:WorkspaceView,sampleReplay=false)=>{
  if(!sampleReplay)return v.progress.shots.length===v.workspace.doc.shots.length&&v.progress.shots.every(s=>s.state==="taught"||s.state==="skipped");
  return v.workspace.frames.length===v.workspace.doc.shots.length&&v.workspace.doc.shots.every((shot,k)=>{
    if(shot.skip)return true;
    const frame=v.workspace.frames[k];
    const views=shot.views?.length?shot.views.filter(view=>view.enabled).map(view=>view.view):[shot.view];
    return views.length>0&&views.every(view=>frame.viewStates?.length?frame.viewStates.some(state=>state.view===view&&state.saved):view===shot.view&&frame.saved);
  });
};

const comparisonLabel = { rules: "规则重判", raw: "候选原图复测", original: "原发布包重现" };
const recordingReason = { off: "录制关闭，本件未保存原图", notRetained: "按录制策略本件未保存原图", pending: "原图仍在落盘", incomplete: "原图不完整", failed: "原图录制失败", complete: "原图已按保留策略清理或无法核验" };
export default function HistoryDetailPage() {
  const {id}=useParams(), navigate=useNavigate(), ws=useWorkspace(), location=useLocation();
  const listQuery=(location.state as {query?:HistoryQuery}|null)?.query??null;
  const [neighbors,setNeighbors]=useState<{older:number|null;newer:number|null}>({older:null,newer:null});
  const [detail,setDetail]=useState<PartDetail|null>(null),[layout,setLayout]=useState<Recipe|null>(null),[recipeMissing,setRecipeMissing]=useState(false);
  const [error,setError]=useState(""),[working,setWorking]=useState(false);
  const [comparison,setComparison]=useState<Comparison|null>(null),[raw,setRaw]=useState<RecordImages|null>(null),[selected,setSelected]=useState(0);
  const [viewSelection,setViewSelection]=useState<{k:number;view:number}|null>(null);
  const selectedView=viewSelection?.k===selected?viewSelection.view:layout?.shots[selected]?.view??1;
  const [savedComparisons,setSavedComparisons]=useState<Comparison[]>([]);
  const [visual,setVisual]=useState<"shots"|"part">("shots");
  const [switching,setSwitching]=useState(false);
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
    // 段配方记录（detail.burst）的对照结果由 BurstHistoryDetail 自己读
    if(detail?.summary.recipeId&&!detail.burst)void workspaceApi.comparisons(detail.summary.recipeId,detail.summary.id).then(v=>alive&&serial===savedSerial.current&&setSavedComparisons(v)).catch(e=>alive&&serial===savedSerial.current&&setError(errorText(e)));
    return()=>{alive=false;};
  },[detail?.summary.id,ws.doc?.id,ws.data?.workspace.revision]);

  useEffect(()=>{
    let alive=true;setDetail(null);setLayout(null);setRecipeMissing(false);setComparison(null);setRaw(null);setError("");setSelected(0);setViewSelection(null);
    void historyApi.detail(Number(id)).then(async d=>{
      if(!alive)return;setDetail(d);
      if(d.burst)return;
      void workspaceApi.recordImages(d.summary.id).then(v=>alive&&setRaw(v)).catch(e=>alive&&setRaw({imageSource:d.summary.source==="offlineCandidate"?"offlineSample":"recording",historyId:d.summary.id,frames:[],complete:false,message:errorText(e)}));
      const r=await historyApi.recipe(d.summary.recipeRevision,d.summary.recipeId);if(alive){setLayout(r);setRecipeMissing(!r);setSelected(Math.max(0,r?.shots.findIndex(shot=>!shot.skip)??0));}
    }).catch(e=>alive&&setError(errorText(e)));
    return()=>{alive=false;};
  },[id]);
  const result=useMemo(()=>{
    if(!detail||!layout)return null;
    const pts=detail.points,idx:number[]=[];pts?.st.forEach((st,j)=>st!==3&&idx.push(j));
    const cycleId=detail.summary.cycleId??`history:${detail.summary.id}`;
    const measured:Measured[]=pts?layout.shots.map((shot,k)=>{
      const owned=idx.filter(j=>layout.points.k[j]===k);
      return {cycleId,shotId:shot.id,camera:shot.camera,bundleId:detail.summary.bundleId,sn:detail.summary.sn,k,cam:detail.frames[k]?.cam??0,referenceFailed:detail.frames[k]?.referenceFailed??[],score:0,ms:0,queueMs:null,engineMs:null,coreMs:null,error:null,idx:owned,d:owned.map(j=>pts.d[j]),w:owned.map(j=>pts.w?.[j]??null),st:owned.map(j=>pts.st[j]),px:[]};
    }):[];
    const frames=layout.shots.map((shot,k)=>({...detail.frames[k]??{status:"waiting" as const,cam:0,arrivedMs:null,frameCounter:null,triggerCounter:null,counterJump:false,score:null,points:0,gapPoints:0,ms:null,session:null,ordinal:null,error:null,referenceFailed:[]},shotId:shot.id,camera:shot.camera,view:shot.view}));
    const part:PartView={cycleId,bundleId:detail.summary.bundleId,sn:detail.summary.sn,recipeId:layout.id,recipeRevision:layout.revisionId,n:layout.shots.length,received:detail.summary.framesReceived,triggers:detail.triggers,queue:0,filled:idx.length,total:layout.points.k.length,frames,measuredFrames:detail.frames.length};
    return {measured,vis:computeVis(layout,part,measured,detail.judgement)};
  },[detail,layout]);
  const diffs=useMemo(()=>comparison&&layout&&detail?shotDiffs(layout,detail,comparison):null,[comparison,layout,detail]);
  const edges=comparison&&comparison.source!=="rules"?replayEdges(comparison,selected,selectedView,diffs?.find(d=>d.k===selected)?.changed):[];
  useEffect(()=>{
    let alive=true;setNeighbors({older:null,newer:null});
    void historyApi.neighbors(Number(id),listQuery).then(v=>alive&&setNeighbors(v)).catch(()=>{});
    return()=>{alive=false;};
  },[id,JSON.stringify(listQuery)]);
  const openPart=(target:number|null)=>{if(target!=null)navigate("/history/"+target,{state:location.state});};
  const stepShot=(delta:number)=>{
    if(!layout)return;
    const measured=layout.shots.flatMap((shot,k)=>shot.skip?[]:[k]);
    const order=measured.length?measured:layout.shots.map((_,k)=>k);
    const at=order.indexOf(selected);
    const next=at>=0?order[Math.min(order.length-1,Math.max(0,at+delta))]:delta>0?order.find(k=>k>selected):order.filter(k=>k<selected).at(-1);
    if(next!=null){setSelected(next);setViewSelection(null);}
  };
  useEffect(()=>{
    const key=(e:KeyboardEvent)=>{
      if(detail?.burst||e.altKey||e.ctrlKey||e.metaKey||e.defaultPrevented)return;
      if((e.target as HTMLElement|null)?.closest?.("input,select,textarea,[contenteditable='true'],[role='dialog'],[role='combobox'],[role='listbox']"))return;
      if(e.key==="ArrowLeft"||e.key==="ArrowRight"){e.preventDefault();stepShot(e.key==="ArrowLeft"?-1:1);}
      else if(e.key==="["||e.key==="]"){e.preventDefault();openPart(e.key==="["?neighbors.older:neighbors.newer);}
    };
    window.addEventListener("keydown",key);
    return()=>window.removeEventListener("keydown",key);
  });

  const runCompare=async(fromRaw:boolean)=>{
    if(pending.current||!detail||!ws.data||ws.busy||ws.dirty||ws.frameDirty||ws.data.workspace.doc.id!==layout?.id)return;
    if(fromRaw?(!raw?.complete||!candidateTaught(ws.data,raw?.imageSource==="offlineSample")):!rulesRejudgeable(detail.points))return;
    pending.current=true;savedSerial.current++;setWorking(true);setError("");
    const valid=()=>current.current.alive&&current.current.scope===scope;
    try{const result=await workspaceApi.compare(ws.data.workspace.doc.id,ws.data.workspace.revision,detail.summary.id,fromRaw);if(valid()){setComparison(result);setSavedComparisons(v=>[result,...v.filter(c=>c.id!==result.id)]);}}
    catch(e){if(valid())setError(errorText(e));}finally{pending.current=false;if(current.current.alive)setWorking(false);}
  };
  const runOriginal=async()=>{
    if(pending.current||!detail||!raw?.complete||!detail.summary.bundleId)return;
    pending.current=true;savedSerial.current++;setWorking(true);setError("");
    const valid=()=>current.current.alive&&current.current.historyId===id;
    try{const result=await workspaceApi.compareOriginal(detail.summary.id);if(valid()){setComparison(result);setSavedComparisons(v=>[result,...v.filter(c=>c.id!==result.id)]);}}
    catch(error){if(valid())setError(errorText(error));}finally{pending.current=false;if(current.current.alive)setWorking(false);}
  };
  useEffect(()=>{
    let alive=true;
    const stop=subscribe<string>("history://updated",cycleId=>{
      if(!detail||cycleId!==detail.summary.cycleId)return;
      void historyApi.detail(detail.summary.id).then(next=>alive&&setDetail(next)).catch(error=>alive&&setError(errorText(error)));
      if(detail.burst)return;
      void workspaceApi.recordImages(detail.summary.id).then(next=>alive&&setRaw(next)).catch(error=>alive&&setError(errorText(error)));
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
    }catch(e){if(valid())setError(errorText(e));}finally{pending.current=false;if(current.current.alive)setWorking(false);}
  };
  const confirmSwitch=async()=>{if(!layout||ws.dirty||ws.frameDirty)return;const next=await ws.select(layout.id);if(next&&current.current.alive)setSwitching(false);};
  if(!detail)return error?<Alert tone="ng">{error}</Alert>:<Text muted>加载中…</Text>;
  // 段配方记录按区域与帧组展示（复测、原图、逐帧追溯都在 BurstHistoryDetail）；旧拍照点记录继续走下面原有的展示
  if(detail.burst)return <BurstHistoryDetail detail={detail} burst={detail.burst} neighbors={neighbors} onOpen={openPart} onBack={()=>navigate("/history")}/>;
  const s=detail.summary,j=detail.judgement,reference=partReferenceBadge(detail);
  const candidateReady=!!ws.data&&ws.data.workspace.doc.id===layout?.id&&!ws.dirty&&!ws.frameDirty;
  const disabled=working||ws.busy||!candidateReady;
  const complete=rulesRejudgeable(detail.points);
  const sampleImages=raw?.imageSource==="offlineSample";
  const imageSn=sampleImages?s.sample?.sn??"—":s.sn;
  const imageTail=" · k"+(selected+1)+" · 视角 "+selectedView;
  const imageLabel=(sampleImages?"离线样本原图 · 样本 SN ":"原始 SN ")+imageSn+imageTail;
  const imageCaption=<>{sampleImages?"离线样本原图 · 样本 SN ":"原始 SN "}<span className="sn">{imageSn}</span>{imageTail}</>;
  const unavailableMessage=sampleImages?raw?.frames.find(frame=>frame.k===selected&&frame.view===selectedView)?.error??raw?.message??"所选离线样本原图不可用":"本帧原图未保存或已按保留策略清理；完整测量数据仍可用于规则重判";
  const blockers=s.source==="offlineCandidate"?[]:[
    !s.bundleId&&"本件没有不可变发布包（布防前被拒或非生产记录），无法按原版本重跑",
    recipeMissing&&"原配方快照 "+(s.recipeRevision??"（未记录）")+" 缺失，不能用当前配方代替",
    raw&&!raw.complete&&recordingReason[detail.recording.state]+(raw.message?"："+raw.message:""),
  ].filter((v):v is string=>!!v);
  // 历史快照按修订读取，与记录的位姿天然配对；仍只在检测所选视角上叠图
  const shot=layout?.shots[selected];
  const ownView=!!shot&&selectedView===shot.view;
  const located=ownView&&!!viewSpec(shot,shot.view)?.fixture;
  const recordedPose=located?framePose(detail.frames[selected],selectedView):null;
  const rerunPose=located&&comparison&&comparison.source!=="rules"?replayPose(comparison,selected,selectedView):null;
  const rerunShot=comparison?.candidateRecipe.shots[selected];
  const rerunRect=rerunShot?viewSpec(rerunShot,selectedView)?.fixture?.rect:undefined;
  // 两次的 offset 各相对自己的框（从原图复测用工作台候选，框可能改过），按各自的框在图像上比
  const rerunDiffers=poseDiffers(recordedPose,shot?viewSpec(shot,selectedView)?.fixture?.rect:undefined,rerunPose,rerunRect);
  const rerunT=rerunDiffers&&rerunRect?poseTransform(rerunRect,rerunPose):null;
  const baseOverlay=layout&&ownView?shotOverlay(layout,selected,result?.vis,detail.frames[selected]):undefined;
  const rerunBox=baseOverlay?.fixture&&rerunT&&rerunRect?{...baseOverlay.fixture,replay:movedBox(rerunRect,0,rerunT)}:null;
  const viewOverlay=edges.length||rerunBox?{...(baseOverlay??{path:[]}),...(rerunBox?{fixture:rerunBox}:{}),...(edges.length?{segments:edges}:{})}:baseOverlay;
  return <div className="stack hist-detail">
    <Card className="detail-head">
      <Stack gap={12} justify="space-between">
        <Stack gap={12}>
          <IconButton onClick={()=>navigate("/history")} aria-label="返回历史列表"><ArrowLeft size={18}/></IconButton>
          <span className="detail-step">
            <IconButton aria-label="前一件" title="前一件（[ 键）" disabled={neighbors.older==null} onClick={()=>openPart(neighbors.older)}><ChevronLeft size={18}/></IconButton>
            <IconButton aria-label="后一件" title="后一件（] 键）" disabled={neighbors.newer==null} onClick={()=>openPart(neighbors.newer)}><ChevronRight size={18}/></IconButton>
          </span>
          <b className="sn" title={s.sn||undefined}>SN {s.sn||"—"}</b>
          {s.source!=="real"&&<StatusBadge tone="off">{sourceLabel[s.source]}</StatusBadge>}
          <Text muted>{formatTime(s.ts)} · {s.recipeId??"无配方"} · {s.source==="offlineCandidate"?"候选修订 "+(s.recipeRevision??"未记录"):"v"+(s.recipeVersion??"—")+" · #"+(s.recipeRevision?.slice(0,6)??"—")} · {triggerModeLabel(s.triggerMode)} · 帧 {s.framesReceived}/{s.framesExpected}</Text>
        </Stack>
        <StatusBadge tone={verdictTone(s.verdict)}>{verdictLabel[s.verdict]} · PLC {s.plcCode}{s.faultCode?" / "+s.faultCode:""}</StatusBadge>
        {reference&&<StatusBadge tone="fault" aria-label={"位置基准："+reference}>{reference}</StatusBadge>}
      </Stack>
    </Card>
    {s.source!=="real"&&<Alert title={s.source==="offlineCandidate"?"候选离线调试记录":"离线回放记录"}>结果来自离线环境，不作为真机生产验收；切换当前站点模式不会改变此记录的来源。</Alert>}
    {error&&<Alert tone="warn" title="操作未完成">{error}</Alert>}
    <Panel title="复测与候选对照" detail={s.source==="offlineCandidate"?"此记录没有生产发布包；可使用当前候选复测，结果另存，原始调试判定保留":"原包重现使用本件冻结资源；候选复测使用当前示教；所有结果另存，原始判定保留"}>
      <div className="wp-actions"><Chip>{s.source==="offlineCandidate"?"原始候选 "+(s.recipeRevision??"未记录"):"原始发布 v"+(s.recipeVersion??"—")}</Chip><StatusBadge>候选 {ws.data&&ws.data.workspace.doc.id===layout?.id?"v"+ws.data.workspace.doc.version+" · 修订 "+ws.data.workspace.revision:"尚未选择此配方"}</StatusBadge><span className="spacer"/>
        <Button disabled={working||ws.busy||!layout||(!ws.list.some(r=>r.id===layout.id)&&!ws.drafts.some(r=>r.doc.id===layout.id))} onClick={()=>{ws.setError("");setSwitching(true);}}>使用该配方候选</Button>
        {s.source!=="offlineCandidate"&&<Button leadingIcon={<ScanEye size={15}/>} disabled={working||!raw?.complete||blockers.length>0} onClick={()=>void runOriginal()}>按原发布包重现</Button>}
        <Button leadingIcon={<Scale size={15}/>} disabled={disabled||!complete} onClick={()=>void runCompare(false)}>按候选规则重判</Button>
        <Button variant="filled" leadingIcon={<ScanEye size={15}/>} disabled={disabled||!raw?.complete||!(ws.data&&candidateTaught(ws.data,raw?.imageSource==="offlineSample"))} onClick={()=>void runCompare(true)}>从原图复测整件</Button>
      </div>
      {ws.dirty||ws.frameDirty?<Alert tone="warn">先保存候选配置和示教中线，再执行对照。</Alert>:!candidateReady&&<Text as="p" muted>选择此配方的候选后，可以调整规则并对照原始结论。</Text>}
      {ws.error&&<Alert tone="warn">{ws.error}</Alert>}<Text as="p" muted>{raw?.message??"正在检查原图保存状态…"} · 对照结果单独保存，原始记录保留。</Text>
      {blockers.length>0&&<Alert tone="warn" title="不能按原发布包重跑">{blockers.join("；")}</Alert>}
      {!!savedComparisons.length&&<Select label="已保存的对照结果" aria-label="已保存对照结果" disabled={working} value={comparison?.id??""} onValueChange={id=>setComparison(savedComparisons.find(c=>c.id===id)??null)} options={[{value:"",label:"选择记录"},...savedComparisons.map(c=>({value:c.id,label:formatTime(c.createdAt)+" · "+comparisonLabel[c.source]+" · 修订 "+c.candidateRevision+" · "+verdictLabel[c.judgement.verdict]}))]}/>}
    </Panel>
    {comparison&&<Panel title={comparisonLabel[comparison.source]+"结果"} detail={(comparison.source==="original"?"原发布 v"+comparison.candidateRecipe.version+" · 包 "+comparison.bundleId:"候选 v"+comparison.candidateRecipe.version+" · 修订 "+comparison.candidateRevision)+" · "+formatTime(comparison.createdAt)}>
      <div className="wp-actions"><StatusBadge tone={comparison.originalVerdict.startsWith("OK")?"ok":"ng"}>原始 {verdictLabel[comparison.originalVerdict]}</StatusBadge><span>→</span><StatusBadge tone={comparison.judgement.verdict.startsWith("OK")?"ok":comparison.judgement.verdict==="ERR_INSPECT"?"warn":"ng"}>{comparison.source==="original"?"重现":"候选"} {verdictLabel[comparison.judgement.verdict]}</StatusBadge><b>{displayReason(comparison.judgement.reason)}</b></div>
      <ReplayDiff diffs={diffs} source={comparison.source} selected={selected} onSelect={k=>{setSelected(k);setViewSelection(null);}}/>
      <UnrolledCurve layout={comparison.candidateRecipe} measured={comparison.measurements.length?comparison.measurements:(result?.measured??[])} vis={[]}/>
    </Panel>}
    <div className="detail-grid">
      <Panel title="逐拍照点结果与原图" detail="上方为中线示意，选中帧后在下方查看原图；原始整件判定保留" actions={<SegmentedControl label="结果视图" size="sm" options={[{value:"shots",label:"逐拍照点"},{value:"part",label:"整件"}]} value={visual} onValueChange={value=>setVisual(value as "shots"|"part")}/>}>
        {layout&&result?visual==="part"?<WorkpieceOverview layout={layout} overview={overview} selected={selected} onSelect={setSelected} vis={result.vis}/>:<ShotTiles layout={layout} vis={result.vis} frames={detail.frames} selected={selected} onSelect={setSelected}/>:<Text as="p" muted>配方快照缺失，无法绘制。</Text>}
        {layout&&<div className="wp-stack"><div className="wp-actions"><StatusBadge>{"选中 k"+(selected+1)}</StatusBadge>{sampleImages&&<StatusBadge>离线样本原图</StatusBadge>}<Select aria-label="历史帧选择" className="detail-frame-select" disabled={working} value={String(selected)} onValueChange={value=>setSelected(Number(value))} options={layout.shots.map((shot,k)=>({value:String(k),label:"k"+(k+1)+" · "+shotLabel(shot,k)+" · "+(raw?.frames.some(f=>f.k===k&&f.available)?"原图可用":"无原图")}))}/><Select aria-label="历史视角选择" disabled={working} value={String(selectedView)} onValueChange={value=>setViewSelection({k:selected,view:Number(value)})} options={[...new Set([layout.shots[selected]?.view??1,...(raw?.frames.filter(f=>f.k===selected).map(f=>f.view)??[])])].sort().map(view=>({value:String(view),label:"视角 "+view+(view===layout.shots[selected]?.view?" · 检测所选":"")}))}/><span className="spacer"/><Button leadingIcon={<Camera size={15}/>} disabled={disabled||!available||selectedView!==layout.shots[selected]?.view} onClick={()=>void useForTeach()}>将此帧用于示教</Button></div><GrayViewer image={image.image} loading={image.loading} error={image.error||(!available?unavailableMessage:"")} label={imageLabel} caption={imageCaption} overlay={viewOverlay}/>{located&&<Text as="p" muted aria-label="定位摘要">定位：{poseSummary(recordedPose)}{recordedPose&&" · 中线、站点与定位框按记录的位姿画；淡虚线为示教位置"}</Text>}{rerunDiffers&&<Text as="p" muted aria-label="复测定位">复测定位与原记录不同：原记录 {poseSummary(recordedPose)}；{comparisonLabel[comparison!.source]} {poseSummary(rerunPose)}{rerunT?"（橙色虚线为复测定位框）":""}。中线与站点仍按原记录位姿画。</Text>}{edges.length>0&&<Text as="p" muted>横线为{comparisonLabel[comparison!.source]}测得的胶边与胶宽；橙色为与原结果状态不同的站。站点颜色仍表示原始判定。</Text>}</div>}
      </Panel>
      <Panel title="原始整件判定" className="side"><p className="reason-box">{displayReason(j.reason)}</p>
        {j.gaps.map((g,i)=><div key={i} className="ng-item"><div className="ng-title c-ng">断胶 · {layout?.segments[g.segment]?.name??"段 "+g.segment}</div><p>段内 s {g.s0.toFixed(1)} – {g.s1.toFixed(1)} mm · 长度 {g.len.toFixed(1)} mm（允许 ≤ {layout?.segments[g.segment]?.maxGapLen??"—"} mm）</p><div className="wp-actions">{g.frames.map(k=><Button size="sm" key={k} onClick={()=>setSelected(k)}>k{k+1}{layout?.shots[k]?" · "+layout.shots[k].id:""}</Button>)}</div></div>)}
        {j.segments.map((r,i)=><div key={i} className="history-segment" data-tone={verdictTone(r.verdict)}><div className="ng-title">{verdictLabel[r.verdict]} · {layout?.segments[i]?.name??"段 "+i}{r.referenceInvalid&&" · 位置基准无效"}</div><div className="kv2"><span>横向偏移</span><b>{r.min?.toFixed(2)??"—"} – {r.max?.toFixed(2)??"—"} mm</b><span>连续超差</span><b>{r.excursionLen.toFixed(1)} mm</b>{r.wMin!=null&&<><span>胶宽</span><b>{r.wMin.toFixed(2)} – {r.wMax?.toFixed(2)} mm</b><span>胶宽连续超差</span><b>{(r.wExcursionLen??0).toFixed(1)} mm</b></>}</div></div>)}
        <div className="kv2"><span>记录号</span><b>#{s.id}</b>{s.sample&&<><span>离线样本</span><b>{s.sample.partId} · SN <span className="sn">{s.sample.sn}</span> · 期望 {sampleLabelText[s.sample.label]}</b></>}<span>配方修订</span><b>{s.recipeRevision??"—"}</b><span>软件版本</span><b>{detail.softwareVersion}</b><span>收尾耗时</span><b>{s.drainMs??"—"} ms</b><span>触发计数</span><b>{detail.triggers}</b><span>复检自</span><b>{s.retestOf?<Link to={"/history/"+s.retestOf}>#{s.retestOf}</Link>:"—"}</b><span>后续复检</span><b>{detail.retests.length?detail.retests.map(r=><Link key={r} to={"/history/"+r}>#{r} </Link>):"—"}</b></div>
      </Panel>
      <Panel title="原始测量曲线" className="curve-panel" detail="原始横向偏移、胶宽和原版公差，按拍照点分段">
        {layout&&result&&<UnrolledCurve layout={layout} measured={result.measured} vis={result.vis}/>}
      </Panel>
      {switching&&layout&&<SwitchCandidate from={ws.data?.workspace.doc.id??null} to={layout.id} unsaved={ws.dirty||ws.frameDirty} saveFailed={ws.saveState==="failed"} busy={ws.busy} error={ws.error} onCancel={()=>setSwitching(false)} onConfirm={()=>void confirmSwitch()}/>}
      <PartTrace detail={detail} layout={layout} raw={raw} selected={selected} selectedView={selectedView} onSelect={(k,view)=>{setSelected(k);setViewSelection({k,view});}}/>
      <CycleTimeline historyId={s.id} cycleId={s.cycleId} ts={s.ts} selected={selected} onSelectShot={k=>{setSelected(k);setViewSelection(null);}}/>
    </div>
  </div>;
}

function SwitchCandidate({from,to,unsaved,saveFailed,busy,error,onCancel,onConfirm}:{from:string|null;to:string;unsaved:boolean;saveFailed:boolean;busy:boolean;error:string;onCancel:()=>void;onConfirm:()=>void}){
  const same=from===to;
  return <Dialog open onOpenChange={open=>{if(!open)onCancel();}} title="切换配方工作台的当前配方" closeDisabled={busy} footer={<>
    <Button disabled={busy} onClick={onCancel}>取消</Button>
    <Button variant="filled" loading={busy} disabled={unsaved||same} onClick={onConfirm}>{busy?"切换中…":"切换到 "+to}</Button>
  </>}>
    {error&&!same&&<Alert tone="ng">{error}</Alert>}
    {same?<p>配方工作台当前已是 <b>{to}</b> 的候选，可以直接对照。</p>:<>
      <p>配方工作台是全局的：确认后，工作台当前配方{from?<>从 <b>{from}</b> </>:""}切换为 <b>{to}</b>，示教、验证与发布等各页随之显示 <b>{to}</b> 的候选。{from&&<> <b>{from}</b> 已保存的内容不受影响。</>}</p>
      {unsaved&&<Alert tone="warn"><b>{from}</b> 还有未保存的修改，{saveFailed?"自动保存失败；请回到配方工作台重试保存":"正在自动保存"}，保存完成前不能切换，以免丢失这些修改。</Alert>}
    </>}
  </Dialog>;
}
