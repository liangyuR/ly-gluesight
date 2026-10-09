import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, Camera, Scale, ScanEye } from "lucide-react";
import { computeVis, shotLabel, UnrolledCurve, type Measured, type PartView, type Recipe } from "../features/cycle";
import { displayReason, formatTime, historyApi, triggerModeLabel, verdictClass, verdictLabel, type PartDetail } from "../features/history";
import { workspaceApi } from "../features/workspace/api";
import { useWorkspace } from "../features/workspace/context";
import { Badge, GrayViewer, Notice, Panel, useGrayImage } from "../features/workspace/components";
import { WorkpieceOverview } from "../features/workspace/OverviewPage";
import { usePublishedOverview } from "../features/workspace/RuntimeFrame";
import type { Comparison, RecordImages } from "../features/workspace/types";

const frameStatus:Record<string,string>={waiting:"未到达",measuring:"未完成",done:"测量完成",locateFailed:"定位失败",error:"测量出错",missing:"未收到"};
export default function HistoryDetailPage() {
  const {id}=useParams(), navigate=useNavigate(), ws=useWorkspace();
  const [detail,setDetail]=useState<PartDetail|null>(null),[layout,setLayout]=useState<Recipe|null>(null);
  const [error,setError]=useState(""),[working,setWorking]=useState(false);
  const [comparison,setComparison]=useState<Comparison|null>(null),[raw,setRaw]=useState<RecordImages|null>(null),[selected,setSelected]=useState(0);
  const [savedComparisons,setSavedComparisons]=useState<Comparison[]>([]);
  const pending=useRef(false),savedSerial=useRef(0);
  const scope=`${id??""}:${ws.data?.workspace.doc.id??""}:${ws.data?.workspace.revision??""}`;
  const current=useRef({scope,selected,alive:true});
  current.current.scope=scope;current.current.selected=selected;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;savedSerial.current++;};},[]);
  const overview=usePublishedOverview(layout);
  const available=raw?.frames.some(f=>f.k===selected&&f.available);
  const image=useGrayImage(null,null,available?detail?.summary.id:null,selected);
  useEffect(()=>{
    let alive=true;const serial=++savedSerial.current;setSavedComparisons([]);setComparison(null);
    if(detail&&ws.doc?.id===detail.summary.recipeId)void workspaceApi.comparisons(ws.doc.id,detail.summary.id).then(v=>alive&&serial===savedSerial.current&&setSavedComparisons(v)).catch(e=>alive&&serial===savedSerial.current&&setError(String(e)));
    return()=>{alive=false;};
  },[detail?.summary.id,ws.doc?.id,ws.data?.workspace.revision]);

  useEffect(()=>{
    let alive=true;setDetail(null);setLayout(null);setComparison(null);setRaw(null);setError("");setSelected(0);
    void historyApi.detail(Number(id)).then(async d=>{
      if(!alive)return;setDetail(d);
      void workspaceApi.recordImages(d.summary.id).then(v=>alive&&setRaw(v)).catch(e=>alive&&setRaw({historyId:d.summary.id,frames:[],complete:false,message:String(e)}));
      const r=await historyApi.recipe(d.summary.recipeHash,d.summary.recipeId);if(alive)setLayout(r);
    }).catch(e=>alive&&setError(String(e)));
    return()=>{alive=false;};
  },[id]);
  const view=useMemo(()=>{
    if(!detail||!layout)return null;
    const pts=detail.points,idx:number[]=[];pts?.st.forEach((st,j)=>st!==3&&idx.push(j));
    const measured:Measured[]=pts?[{sn:detail.summary.sn,k:-1,cam:0,located:true,score:0,ms:0,error:null,idx,d:idx.map(j=>pts.d[j]),w:idx.map(j=>pts.w?.[j]??null),st:idx.map(j=>pts.st[j]),px:[]}]:[];
    const part:PartView={sn:detail.summary.sn,recipeId:layout.id,recipeHash:layout.hash,n:layout.shots.length,received:detail.summary.framesReceived,triggers:detail.triggers,queue:0,filled:idx.length,total:layout.points.k.length,frames:detail.frames,measuredFrames:detail.frames.length};
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
  const useForTeach=async()=>{
    if(pending.current||!detail||!ws.data||ws.busy||ws.dirty||ws.frameDirty||ws.data.workspace.doc.id!==layout?.id||!available)return;
    pending.current=true;setWorking(true);setError("");
    const valid=()=>current.current.alive&&current.current.scope===scope&&current.current.selected===selected;
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
  const imageLabel="原始 SN "+s.sn+" · k"+(selected+1);
  return <div className="stack hist-detail">
    <div className="panel detail-head"><button className="icon-btn" onClick={()=>navigate("/history")} aria-label="返回历史列表"><ArrowLeft size={18}/></button><b className="mono">SN {s.sn}</b><span className="muted">{formatTime(s.ts)} · {s.recipeId??"无配方"} · v{s.recipeVersion??"—"} · #{s.recipeHash?.slice(0,6)??"—"} · {triggerModeLabel(s.triggerMode)} · 帧 {s.framesReceived}/{s.framesExpected}</span><span className="spacer"/><span className={"vt big "+verdictClass(s.verdict)}>{verdictLabel[s.verdict]} · PLC {s.plcCode}{s.faultCode?" / "+s.faultCode:""}</span></div>
    {error&&<Notice title="操作未完成" tone="warn">{error}</Notice>}
    <Panel title="候选对照" detail="规则重判使用保存的距离和胶宽；原图复测重新运行定位、测量及整件判定">
      <div className="wp-actions"><Badge tone="neutral">原始生产 v{s.recipeVersion??"—"}</Badge><Badge>候选 {ws.data&&ws.data.workspace.doc.id===layout?.id?"v"+ws.data.workspace.doc.version+" · 修订 "+ws.data.workspace.revision:"尚未选择此配方"}</Badge><span className="spacer"/>
        <button className="btn" disabled={working||ws.busy||!layout||(!ws.list.some(r=>r.id===layout.id)&&!ws.drafts.some(r=>r.doc.id===layout.id))} onClick={()=>layout&&void ws.select(layout.id)}>使用该配方候选</button>
        <button className="btn" disabled={disabled||!complete} onClick={()=>void runCompare(false)}><Scale size={15}/>按候选规则重判</button>
        <button className="btn primary" disabled={disabled||!raw?.complete||!ws.data?.workspace.frames.every(f=>f.saved)} onClick={()=>void runCompare(true)}><ScanEye size={15}/>从原图复测整件</button>
      </div>
      {ws.dirty||ws.frameDirty?<p className="c-warn">先保存候选配置和帧参数，再执行对照。</p>:!candidateReady&&<p className="muted">选择此配方的候选后，可以调整规则并对照原始结论。</p>}
      {ws.error&&<p className="c-warn">{ws.error}</p>}<p className="muted">{raw?.message??"正在检查原图保存状态…"} · 对照结果单独保存，原始生产记录保留。</p>
      {!!savedComparisons.length&&<label className="field"><span>已保存的对照结果</span><select className="input" aria-label="已保存对照结果" disabled={working} value={comparison?.id??""} onChange={e=>setComparison(savedComparisons.find(c=>c.id===e.target.value)??null)}><option value="">选择记录</option>{savedComparisons.map(c=><option key={c.id} value={c.id}>{formatTime(c.createdAt)} · {c.source==="raw"?"原图复测":"规则重判"} · 修订 {c.candidateRevision} · {verdictLabel[c.judgement.verdict]}</option>)}</select></label>}
    </Panel>
    {comparison&&<Panel title={comparison.source==="raw"?"原图复测结果":"规则重判结果"} detail={"候选 v"+comparison.candidateRecipe.version+" · 修订 "+comparison.candidateRevision+" · "+formatTime(comparison.createdAt)}>
      <div className="wp-actions"><Badge tone={comparison.originalVerdict.startsWith("OK")?"ok":"ng"}>原始 {verdictLabel[comparison.originalVerdict]}</Badge><span>→</span><Badge tone={comparison.judgement.verdict.startsWith("OK")?"ok":comparison.judgement.verdict==="ERR_INSPECT"?"warn":"ng"}>候选 {verdictLabel[comparison.judgement.verdict]}</Badge><b>{displayReason(comparison.judgement.reason)}</b></div>
      <UnrolledCurve layout={comparison.candidateRecipe} measured={comparison.measurements.length?comparison.measurements:(view?.measured??[])} vis={[]}/>
    </Panel>}
    <div className="detail-grid">
      <Panel title="工件总览与原图" detail="帧选择只改变查看范围，原始整件判定保留">
        {layout&&view?<WorkpieceOverview layout={layout} overview={overview} selected={selected} onSelect={setSelected} vis={view.vis}/>:<p className="muted">配方快照缺失，无法绘制。</p>}
        {layout&&<div className="wp-stack"><div className="wp-actions"><Badge>{"选中 k"+(selected+1)}</Badge><select className="input" aria-label="历史帧选择" disabled={working} style={{width:260}} value={selected} onChange={e=>setSelected(Number(e.target.value))}>{layout.shots.map((shot,k)=><option key={k} value={k}>k{k+1} · {shotLabel(shot,k)} · {raw?.frames.some(f=>f.k===k&&f.available)?"原图可用":"无原图"}</option>)}</select><span className="spacer"/><button className="btn" disabled={disabled||!available} onClick={()=>void useForTeach()}><Camera size={15}/>将此帧用于示教</button></div><GrayViewer image={image.image} loading={image.loading} error={image.error||(!available?"本帧原图未保存或已按保留策略清理；完整测量数据仍可用于规则重判":"")} label={imageLabel}/></div>}
      </Panel>
      <Panel title="原始整件判定" className="side"><p className="reason-box">{displayReason(j.reason)}</p>
        {j.gaps.map((g,i)=><div key={i} className="ng-item"><div className="ng-title c-ng">断胶 · {layout?.segments[g.segment]?.name??"段 "+g.segment}</div><p>s {g.s0.toFixed(1)} – {g.s1.toFixed(1)} mm · 长度 {g.len.toFixed(1)} mm（允许 ≤ {layout?.maxGapLen??"—"}）</p><div className="wp-actions">{g.frames.map(k=><button className="btn small" key={k} onClick={()=>setSelected(k)}>k{k+1}</button>)}{g.frames.length>1&&<span className="muted">跨帧合并</span>}</div></div>)}
        {j.segments.map((r,i)=><div key={i} className="ng-item"><div className={"ng-title "+(r.verdict.startsWith("OK")?"c-ok":"c-ng")}>{verdictLabel[r.verdict]} · {layout?.segments[i]?.name??"段 "+i}</div><div className="kv2"><span>距离</span><b>{r.min?.toFixed(2)??"—"} – {r.max?.toFixed(2)??"—"} mm</b><span>连续超差</span><b>{r.excursionLen.toFixed(1)} mm</b>{r.wMin!=null&&<><span>胶宽</span><b>{r.wMin.toFixed(2)} – {r.wMax?.toFixed(2)} mm</b><span>胶宽连续超差</span><b>{(r.wExcursionLen??0).toFixed(1)} mm</b></>}</div></div>)}
        <div className="kv2"><span>记录号</span><b>#{s.id}</b><span>配置哈希</span><b>{s.recipeHash?.slice(0,12)??"—"}</b><span>软件版本</span><b>{detail.softwareVersion}</b><span>收尾耗时</span><b>{s.drainMs??"—"} ms</b><span>触发计数</span><b>{detail.triggers}</b><span>复检自</span><b>{s.retestOf?<Link to={"/history/"+s.retestOf}>#{s.retestOf}</Link>:"—"}</b><span>后续复检</span><b>{detail.retests.length?detail.retests.map(r=><Link key={r} to={"/history/"+r}>#{r} </Link>):"—"}</b></div>
      </Panel>
      <Panel title="原始测量曲线" className="curve-panel" detail="原始距离、胶宽和原版公差">
        {layout&&view&&<UnrolledCurve layout={layout} measured={view.measured} vis={view.vis}/>}
      </Panel>
      <Panel title="逐帧记录" className="frames-panel"><div className="table-wrap"><table className="table"><thead><tr><th>帧</th><th>相机</th><th>到达</th><th>间隔</th><th>Chunk 帧 / 触发</th><th>分数</th><th>测量 / 缺胶点</th><th>耗时</th><th>状态</th></tr></thead><tbody>
        {detail.frames.map((f,k)=>{const prev=k?detail.frames[k-1].arrivedMs:null;return <tr key={k} aria-selected={selected===k}><td><button className="btn small" aria-pressed={selected===k} onClick={()=>setSelected(k)}>k{k+1}</button></td><td>{f.camera||"#"+(f.cam+1)}</td><td>{f.arrivedMs??"—"} ms</td><td>{f.arrivedMs!=null&&prev!=null?f.arrivedMs-prev:"—"} ms</td><td className={f.counterJump?"c-err":""}>{f.frameCounter??"—"} / {f.triggerCounter??"—"}{f.counterJump?" 跳号":""}</td><td>{f.score?.toFixed(3)??"—"}</td><td>{f.points} / {f.gapPoints}</td><td>{f.ms??"—"} ms</td><td className={f.gapPoints?"c-ng":f.status==="done"?"c-ok":"c-err"}>{f.gapPoints?"含缺胶":frameStatus[f.status]??"—"}</td></tr>;})}
        {!detail.frames.length&&<tr><td colSpan={9} className="muted center">本件未布防，没有帧记录</td></tr>}
      </tbody></table></div></Panel>
    </div>
  </div>;
}
