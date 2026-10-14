import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { Plus, Trash2 } from "lucide-react";
import { CalibPanel, cameraApi, CameraConfigPanel, defaultCameraConfig, DryRunPanel, FeasibilityCalc, FramePreview, useRigStatus, type CameraConfig, type FeasibilitySources } from "../features/camera";
import { OfflineManagedNotice, recipeApi, useCycle, useSiteStatus } from "../features/cycle";
import { shotCameras } from "../features/cycle/vis";
import { desktopAvailable } from "../lib/desktop";
import { Badge, Notice, Panel } from "../features/workspace/components";
import StationCapture, { type StationView } from "../features/workspace/StationCapture";
import { workspaceApi } from "../features/workspace/api";
import type { CaptureNavigationState } from "../features/workspace/captureNavigation";
import { ReturnButton } from "../features/workspace/returnNavigation";
import { productionBusy } from "../features/cycle/phase";
import { errorText } from "../lib/errors";

type CameraView = "device" | "calibration";
const sourceText = { mvs:"海康 MVS",sim:"合成图（离线）",sampleSet:"样本集（离线）" } as const;
export default function CameraPage({view="device"}:{view?:CameraView}) {
  const location=useLocation();
  const captureReturn=(location.state as CaptureNavigationState | null)?.captureReturn;
  const navigationState:CaptureNavigationState={...(location.state as CaptureNavigationState | null)};
  const {statuses,lastFrame}=useRigStatus();
  const {snapshot}=useCycle();
  const site=useSiteStatus();
  const offline=site?.mode==="offline";
  const readOnly=offline&&view==="device";
  const [configs,setConfigs]=useState<CameraConfig[]>([]);
  const [cam,setCam]=useState(0);
  const [error,setError]=useState("");
  const [calibView,setCalibView]=useState(1);
  const [sample,setSample]=useState<StationView|null>(null);
  const [calibMm,setCalibMm]=useState<number|null>(null);
  const [loading,setLoading]=useState(false);
  const [action,setAction]=useState("");
  const [configSaving,setConfigSaving]=useState(false);
  const [references,setReferences]=useState<Record<string,string[]>>({});
  const [referencesReady,setReferencesReady]=useState(false);
  const mounted=useRef(true),loadSerial=useRef(0),pending=useRef(false);
  const current=useRef({cam,configs});current.current={cam,configs};
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;loadSerial.current++;};},[]);
  useEffect(()=>setSample(null),[cam,configs[cam],view,calibView]);
  useEffect(()=>setCalibView(1),[cam]);
  const reload=useCallback(async()=>{
    const serial=++loadSerial.current;
    setLoading(true);setReferencesReady(false);
    const referenceRequest=view==="device"&&desktopAvailable()?Promise.all([recipeApi.list(),workspaceApi.list()]).then(([production,drafts])=>{
      if(production.errors.length)throw new Error(production.errors.join("；"));
      const found:Record<string,string[]>={};
      const remember=(ids:string[],name:string)=>ids.forEach(id=>{const values=found[id]??=[];if(!values.includes(name))values.push(name);});
      production.recipes.forEach(r=>remember(r.cameras,`生产配方 ${r.name}（${r.id}）`));
      drafts.forEach(w=>{
        for(const [doc,label] of [[w.doc,"候选配方"],[w.pending?.doc,"待发布配方"]] as const){
          if(doc)remember(shotCameras(doc.shots),`${label} ${doc.name}（${doc.id}）`);
        }
      });
      return {found,error:""};
    }).catch(e=>({found:{},error:`相机引用读取失败：${errorText(e)}。刷新后再移除相机。`})):Promise.resolve({found:{},error:""});
    try{
      const [next,refs]=await Promise.all([cameraApi.rigConfig(),referenceRequest]);
      if(!mounted.current||serial!==loadSerial.current)return null;
      const selected=current.current.configs[current.current.cam]?.id ?? captureReturn?.cameraId;
      setConfigs(next);
      if(selected)setCam(Math.max(0,next.findIndex(c=>c.id===selected)));
      setReferences(refs.found);setReferencesReady(!refs.error);setError(refs.error);
      return next;
    }catch(e){if(mounted.current&&serial===loadSerial.current)setError(errorText(e));return null;}
    finally{if(mounted.current&&serial===loadSerial.current)setLoading(false);}
  },[view,captureReturn?.cameraId]);
  // 切换相机只切换已加载的配置；刷新与标定页面切换才重新读配置。
  const reloadRef=useRef(reload);reloadRef.current=reload;
  useEffect(()=>{void reloadRef.current();},[view,offline]);
  const unpinned=statuses.some(s=>s.device&&configs[s.cam]?.source==="mvs"&&!configs[s.cam]?.serial);
  useEffect(()=>{if(unpinned)void reloadRef.current();},[unpinned]);
  useEffect(()=>{
    if(view==="device"||!configs.length)return;
    const eligible=configs.findIndex(c=>c.acquisition==="triggered");
    if(eligible>=0&&configs[cam]?.acquisition!=="triggered")setCam(eligible);
  },[configs,view,cam]);
  const eligible=configs.map((c,i)=>({c,i})).filter(({c})=>view==="device"||c.acquisition==="triggered");
  const config=eligible.some(e=>e.i===cam)?configs[cam]:null;
  const status=statuses.find(s=>s.cam===cam)??null;
  const busy=productionBusy(snapshot);
  const referencedBy=config?references[config.id]??[]:[];
  const saved=()=>void reload();
  const sampleSet=offline&&site?.source==="sampleSet"?site.sampleSet:null;
  const feasibility:FeasibilitySources={
    exposure:config?{value:config.exposureUs,from:"来自相机参数"}:null,
    fps:status?.maxFps?{value:status.maxFps,from:"来自相机最高帧率"}:null,
    mmPerPx:sampleSet?.mmPerPx?{value:sampleSet.mmPerPx,from:"来自样本集声明"}:calibMm?{value:calibMm,from:"来自工位标定"}:null,
  };
  const add=async()=>{
    if(pending.current||loading||configSaving||busy||readOnly||configs.length>=8||!desktopAvailable())return;
    pending.current=true;setAction("add");
    setError("");
    try {
      const base=configs.at(-1)??defaultCameraConfig;
      let n=configs.length+1;while(configs.some(c=>c.name==="相机 "+n))n++;
      const i=await cameraApi.add({...base,name:"相机 "+n,serial:""});
      if(!mounted.current)return;
      const next=await reload();if(next&&mounted.current)setCam(Math.min(i,next.length-1));
    }catch(e){if(mounted.current)setError(errorText(e));}
    finally{pending.current=false;if(mounted.current)setAction("");}
  };
  const remove=async()=>{
    if(!config||pending.current||loading||configSaving||busy||readOnly||!desktopAvailable()||!referencesReady||referencedBy.length||configs.length<=1)return;
    if(!window.confirm("从相机组移除「"+config.name+"」？"))return;
    pending.current=true;setAction("remove");setError("");
    try{await cameraApi.remove(cam);if(!mounted.current)return;const next=await reload();if(next&&mounted.current)setCam(Math.min(Math.max(0,cam-1),next.length-1));}
    catch(e){if(mounted.current)setError(errorText(e));}
    finally{pending.current=false;if(mounted.current)setAction("");}
  };
  return <div className="cam-page">
    <ReturnButton disabled={configSaving||!!action} hint="配置后保存并应用，再返回继续。"/>
    <div className="cam-tabs">{eligible.map(({c,i})=><button key={c.id} aria-label={`${c.name} ${c.id}`} className={"tab"+(i===cam?" active":"")} disabled={!!action||configSaving} onClick={()=>setCam(i)}><i className={statuses.find(s=>s.cam===i)?.ready?"ok":""}/>{c.name}<span className="muted mono">{c.id}</span></button>)}
      {view==="device"&&<><button className="btn" onClick={()=>void reload()} disabled={loading||!!action||configSaving}>刷新配置</button><button className="btn" onClick={()=>void add()} disabled={busy||readOnly||loading||!!action||configSaving||!desktopAvailable()||configs.length>=8}><Plus size={15}/>{action==="add"?"添加中…":"添加相机"}</button>{configs.length>1&&<button className="btn" onClick={()=>void remove()} disabled={busy||readOnly||loading||!!action||configSaving||!desktopAvailable()||!referencesReady||!!referencedBy.length}><Trash2 size={15}/>{action==="remove"?"移除中…":"移除当前"}</button>}</>}
      {view!=="device"&&<Link className="btn" to="/camera" state={navigationState}>相机与采集</Link>}
    </div>
    <div className="dev-bar"><Badge tone={status?.ready?"ok":"warn"}>{status?.ready?"已连接":"未就绪"}</Badge>{status&&<Badge tone="neutral">{sourceText[status.source]} · {status.acquisition==="freeRun"?"连续采集":"触发采集"}</Badge>}{status?.device&&<span className="chip-static">{status.device.model} · {status.device.serial}</span>}<span className="chip-static">帧 {status?.frames??0} · {status?.fps?status.fps.toFixed(1)+" fps":"—"}</span>{!!status?.lostPackets&&<Badge tone="warn">丢包 {status.lostPackets}</Badge>}<span className="muted">{status?.message}</span></div>
    {view==="calibration"&&<ol className="flyshot-guide" aria-label="飞拍标定步骤">
      <li><strong>冻结标定样本</strong><span>把标定板放在胶路所在平面，取样或导入一张原图</span></li>
      <li><strong>工位标定</strong><span>识别棋盘格得到像素当量（mm/px），检测时据此把胶宽、偏移换算成毫米</span></li>
      <li><strong>飞拍可行性</strong><span>用当量、曝光、帧率和手填的速度、间距，核算会不会拖影、来不来得及拍</span></li>
      <li><strong>触发空跑</strong><span>机器人不带工件走一遍，实测 PLC 触发与收图是否一一对应</span></li>
    </ol>}
    {view==="calibration"&&sampleSet&&<div style={{gridColumn:"1/-1"}}><Notice title="离线样本集可跳过工位标定">{sampleSet.mmPerPx?`样本集「${sampleSet.name}」已声明像素当量 ${sampleSet.mmPerPx} mm/px，采用采集轮次时自动作为当量依据；本页的工位标定和触发空跑不影响离线流程。`:`样本集「${sampleSet.name}」没有声明像素当量，示教时需要用已知尺寸人工验证当量；本页的工位标定和触发空跑不影响离线流程。`}</Notice></div>}
    {error&&<div style={{gridColumn:"1/-1"}}><Notice title="操作未完成" tone="warn">{error}</Notice></div>}
    {view==="device"&&referencedBy.length>0&&<div style={{gridColumn:"1/-1"}}><Notice title="当前相机被配方引用">先在 {referencedBy.join("、")} 中改选相机并保存，再移除此相机。</Notice></div>}
    {readOnly&&<div style={{gridColumn:"1/-1"}}><OfflineManagedNotice>相机组按离线来源临时生成，离线期间不能修改或增删相机。</OfflineManagedNotice></div>}
    {busy&&<div style={{gridColumn:"1/-1"}}><Notice title="当前工件正在检测">相机配置、取样与标定在工件结束后可操作。</Notice></div>}
    {config?<><div className="col"><fieldset disabled={busy||readOnly||!!action||!desktopAvailable()} className="cam-action-area">
      {view==="device"?<CameraConfigPanel key={config.id+":"+config.source} cam={cam} initial={config} status={status} onSaved={saved} onSavingChange={setConfigSaving}/>:<StationCapture key={config.id+":"+calibView} cam={cam} view={calibView} sample={sample} onSample={setSample}/>}
    </fieldset>{view==="device"&&<Panel title="实际参数状态"><Badge tone={status?.warnings.length?"warn":status?.ready?"ok":"neutral"}>{status?.warnings.length?"存在未接受参数":status?.ready?"参数已应用":"等待相机连接"}</Badge>{status?.warnings.map(w=><p key={w} className="c-warn">{w}</p>)}<p className="muted">连接状态和参数接受状态分别确认。保存后以相机实际返回结果为准。</p></Panel>}</div>
    <div className="col"><fieldset disabled={busy||!!action||configSaving||!desktopAvailable()} className="cam-action-area">
      {view==="device"&&<FramePreview key={config.id} cam={cam} status={status} lastFrame={lastFrame[cam]} config={config}/>}
      {view==="calibration"&&<>{config.viewCount===3&&<label className="field"><span>标定图像</span><select className="input" aria-label="标定图像" value={calibView} onChange={e=>{setSample(null);setCalibView(Number(e.target.value));}}>{[1,2,3].map(v=><option key={v} value={v}>图 {v}</option>)}</select></label>}<CalibPanel key={config.id+":"+calibView} cam={cam} view={calibView} isSim={config.source==="sim"&&sample?.metadata.source!=="import"} imageId={sample?.metadata.id} onMmPerPx={setCalibMm}/><FeasibilityCalc key={"feasibility:"+config.id} sources={feasibility} imageSize={sample?.image}/><DryRunPanel key={"dry-run:"+config.id}cam={cam} frameMs={status?.maxFps?1000/status.maxFps:null}/></>}
    </fieldset>{view==="device"&&<Panel title="继续建站"><p className="muted">保存采集参数后，完成飞拍标定。</p><div className="wp-actions"><Link className="btn" to="/camera/calibration" state={navigationState}>飞拍标定</Link></div></Panel>}</div></>:<div style={{gridColumn:"1/-1"}}><Notice title={view==="device"?"尚未加载采集设备":"没有触发采集相机"} tone="warn">{view==="device"?"在桌面软件中添加或连接相机。":"在相机与采集页添加相机，采集方式选择触发采集。"}</Notice></div>}
  </div>;
}
export function FlyshotCalibrationPage(){return <CameraPage view="calibration"/>;}
