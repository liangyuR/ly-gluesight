import { useEffect, useMemo, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { CalibPanel, cameraApi, FeasibilityCalc, useRigStatus, type CameraConfig, type FeasibilitySources } from "../features/camera";
import CameraList, { type CalibrationSummary } from "../features/camera/components/CameraList";
import type { CalibInfo } from "../features/camera/components/CalibPanel";
import { useRigConfig } from "../features/camera/useRigConfig";
import { useCycle } from "../features/cycle";
import { desktopAvailable } from "../lib/desktop";
import { Notice } from "../features/workspace/components";
import StationCapture, { type StationView } from "../features/workspace/StationCapture";
import type { CaptureNavigationState } from "../features/workspace/captureNavigation";
import { ReturnButton } from "../features/workspace/returnNavigation";
import { productionBusy } from "../features/cycle/phase";

async function calibrationSummary(config: CameraConfig, cam: number): Promise<CalibrationSummary> {
  if(config.source==="sim")return {tone:"ok",text:"内置标定 · 0.08 mm/px"};
  const views=config.viewCount===3?[1,2,3]:[1];
  try{
    const infos=await Promise.all(views.map(view=>cameraApi.calibInfo(cam,view)));
    const done=infos.filter(info=>info?.mmPerPx!=null);
    if(!done.length)return {tone:"warn",text:"未标定"};
    if(done.length<views.length)return {tone:"warn",text:`已标定 ${done.length}/${views.length}`};
    return {tone:"ok",text:views.length>1?`已标定 ${views.length}/${views.length}`:`已标定 · ${done[0]!.mmPerPx!.toFixed(4)} mm/px`};
  }catch{
    return {tone:"off",text:"标定读取失败"};
  }
}

export default function FlyshotCalibrationPage() {
  const location=useLocation();
  const captureReturn=(location.state as CaptureNavigationState | null)?.captureReturn;
  const navigationState:CaptureNavigationState={...(location.state as CaptureNavigationState | null)};
  const {statuses}=useRigStatus();
  const {snapshot}=useCycle();
  const {configs,cam,setCam,error,loading,reload}=useRigConfig({withReferences:false,preferredId:captureReturn?.cameraIds[0]});
  const [calibView,setCalibView]=useState(1);
  const [sample,setSample]=useState<StationView|null>(null);
  const [calibMm,setCalibMm]=useState<number|null>(null);
  const [calibInfo,setCalibInfo]=useState<CalibInfo|null>(null);
  const [calibration,setCalibration]=useState<Record<string,CalibrationSummary>>();
  const [calibReload,setCalibReload]=useState(0),[calibLoading,setCalibLoading]=useState(false);
  useEffect(()=>setSample(null),[cam,configs[cam],calibView]);
  useEffect(()=>setCalibView(1),[cam]);
  useEffect(()=>{
    if(!desktopAvailable())return;
    let alive=true;
    setCalibLoading(true);
    void Promise.all(configs.map(async(config,i)=>[config.id,await calibrationSummary(config,i)] as const))
      .then(entries=>{if(alive)setCalibration(Object.fromEntries(entries));})
      .finally(()=>{if(alive)setCalibLoading(false);});
    return()=>{alive=false;};
  },[configs,calibMm,calibReload]);
  const config=configs[cam]??null;
  // 切换相机、视图时新的取样区会先于清空样本挂载：只认属于当前相机与视图的样本，取样区才会去载入标定原图
  const current=sample&&sample.metadata.camera===config?.id&&sample.metadata.view===calibView?sample:null;
  const corners=calibInfo?.sampleId!=null&&calibInfo.sampleId===current?.metadata.id?calibInfo.corners:null;
  const marks=useMemo(()=>corners?.map((at,i)=>({at,color:i===0?"var(--ov-ok)":"var(--ov-line)"})),[corners]);
  const status=statuses.find(s=>s.cam===cam)??null;
  const busy=productionBusy(snapshot);
  const feasibility:FeasibilitySources={
    exposure:status&&config?.source==="mvs"?{value:status.imaging.exposureUs,from:"来自相机页保存的曝光"}:null,
    fps:status?.configuredFrameRate?{value:status.configuredFrameRate,from:"来自相机设定帧率回读"}:null,
    mmPerPx:calibMm?{value:calibMm,from:"来自工位标定"}:null,
  };
  const notice=error?<Notice title="操作未完成" tone="warn">{error}</Notice>:busy?<Notice title="当前工件正在检测">取样与标定在工件结束后可操作。</Notice>:null;

  return <div className="cam-page calib-page">
    <ReturnButton hint="标定完成后返回继续。"/>
    {notice&&<div className="cam-notice">{notice}</div>}
    <CameraList configs={configs} statuses={statuses} selected={cam} onSelect={setCam} onRefresh={()=>{void reload();setCalibReload(n=>n+1);}} refreshing={loading||calibLoading} calibration={calibration}/>
    {config?<main className="cam-main">
      <div className="cam-body">
        <fieldset disabled={busy||!desktopAvailable()} className="cam-action-area cam-image-area">
          <StationCapture key={config.id+":"+calibView} cam={cam} view={calibView} multiView={config.viewCount===3} sample={current} onSample={setSample} cameraReady={status?.ready ?? false} marks={marks}/>
        </fieldset>
        <fieldset disabled={busy||!desktopAvailable()} className="cam-action-area">
          {config.viewCount===3&&<label className="field"><span>标定图像</span><select className="input" aria-label="标定图像" value={calibView} onChange={e=>{setSample(null);setCalibView(Number(e.target.value));}}>{[1,2,3].map(v=><option key={v} value={v}>图 {v}</option>)}</select></label>}
          <CalibPanel key={config.id+":"+calibView} cam={cam} view={calibView} multiView={config.viewCount===3} isSim={config.source==="sim"&&current?.metadata.source!=="import"&&current?.metadata.source!=="calib"} imageId={current?.metadata.id} onMmPerPx={setCalibMm} onCalibration={setCalibInfo}/>
          <FeasibilityCalc key={"feasibility:"+config.id} sources={feasibility} imageSize={current?.image}/>
        </fieldset>
      </div>
    </main>:<main className="cam-main"><Notice title={loading?"正在读取相机配置":"尚未添加相机"} tone="warn">在<Link to="/camera" state={navigationState}>相机</Link>页添加相机并绑定设备。</Notice></main>}
  </div>;
}
