import { useEffect, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { CalibPanel, FeasibilityCalc, useRigStatus, type FeasibilitySources } from "../features/camera";
import CameraList, { cameraState } from "../features/camera/components/CameraList";
import { useRigConfig } from "../features/camera/useRigConfig";
import { useCycle, useSiteStatus } from "../features/cycle";
import { desktopAvailable } from "../lib/desktop";
import { Badge, Notice } from "../features/workspace/components";
import StationCapture, { type StationView } from "../features/workspace/StationCapture";
import type { CaptureNavigationState } from "../features/workspace/captureNavigation";
import { ReturnButton } from "../features/workspace/returnNavigation";
import { productionBusy } from "../features/cycle/phase";

export default function FlyshotCalibrationPage() {
  const location=useLocation();
  const captureReturn=(location.state as CaptureNavigationState | null)?.captureReturn;
  const navigationState:CaptureNavigationState={...(location.state as CaptureNavigationState | null)};
  const {statuses}=useRigStatus();
  const {snapshot}=useCycle();
  const site=useSiteStatus();
  const offline=site?.mode==="offline";
  const {configs,cam,setCam,error,loading,reload}=useRigConfig({withReferences:false,preferredId:captureReturn?.cameraIds[0],reloadKey:offline});
  const [calibView,setCalibView]=useState(1);
  const [sample,setSample]=useState<StationView|null>(null);
  const [calibMm,setCalibMm]=useState<number|null>(null);
  useEffect(()=>setSample(null),[cam,configs[cam],calibView]);
  useEffect(()=>setCalibView(1),[cam]);
  const config=configs[cam]??null;
  const status=statuses.find(s=>s.cam===cam)??null;
  const busy=productionBusy(snapshot);
  const sampleSet=offline&&site?.source==="sampleSet"?site.sampleSet:null;
  const feasibility:FeasibilitySources={
    exposure:null,
    fps:status?.maxFps?{value:status.maxFps,from:"来自相机最高帧率"}:null,
    mmPerPx:sampleSet?.mmPerPx?{value:sampleSet.mmPerPx,from:"来自样本集声明"}:calibMm?{value:calibMm,from:"来自工位标定"}:null,
  };
  const state=config?cameraState(config,status):null;
  const notice=error?<Notice title="操作未完成" tone="warn">{error}</Notice>:busy?<Notice title="当前工件正在检测">取样与标定在工件结束后可操作。</Notice>:sampleSet?<Notice title="离线样本集可跳过工位标定">{sampleSet.mmPerPx?`样本集「${sampleSet.name}」已声明像素当量 ${sampleSet.mmPerPx} mm/px，采用采集轮次时自动作为当量依据；本页的工位标定不影响离线流程。`:`样本集「${sampleSet.name}」没有声明像素当量，示教时需要用已知尺寸人工验证当量；本页的工位标定不影响离线流程。`}</Notice>:null;

  return <div className="cam-page">
    <ReturnButton hint="标定完成后返回继续。"/>
    <ol className="flyshot-guide" aria-label="飞拍标定步骤">
      <li><strong>冻结标定样本</strong><span>把标定板放在胶路所在平面，取样或导入一张原图</span></li>
      <li><strong>工位标定</strong><span>识别棋盘格得到像素当量（mm/px），检测时据此把胶宽、偏移换算成毫米</span></li>
      <li><strong>飞拍可行性</strong><span>用当量、曝光、帧率和手填的速度、间距，核算会不会拖影、来不来得及拍</span></li>
    </ol>
    {notice&&<div className="cam-notice">{notice}</div>}
    <CameraList configs={configs} statuses={statuses} selected={cam} onSelect={setCam} onRefresh={()=>void reload()} refreshing={loading}/>
    {config&&state?<main className="cam-main">
      <header className="cam-head"><strong>{config.name}</strong><span className="mono muted">{config.id}</span><Badge tone={state.tone==="ok"?"ok":"warn"}>{state.label}</Badge><span className="muted cam-head-info">{state.label==="未绑定"?<>到<Link to="/camera" state={navigationState}>相机</Link>绑定设备</>:state.text}</span></header>
      <div className="cam-body">
        <fieldset disabled={busy||!desktopAvailable()} className="cam-action-area">
          <StationCapture key={config.id+":"+calibView} cam={cam} view={calibView} sample={sample} onSample={setSample}/>
        </fieldset>
        <fieldset disabled={busy||!desktopAvailable()} className="cam-action-area">
          {config.viewCount===3&&<label className="field"><span>标定图像</span><select className="input" aria-label="标定图像" value={calibView} onChange={e=>{setSample(null);setCalibView(Number(e.target.value));}}>{[1,2,3].map(v=><option key={v} value={v}>图 {v}</option>)}</select></label>}
          <CalibPanel key={config.id+":"+calibView} cam={cam} view={calibView} isSim={config.source==="sim"&&sample?.metadata.source!=="import"} imageId={sample?.metadata.id} onMmPerPx={setCalibMm}/>
          <FeasibilityCalc key={"feasibility:"+config.id} sources={feasibility} imageSize={sample?.image}/>
        </fieldset>
      </div>
    </main>:<main className="cam-main"><Notice title={loading?"正在读取相机配置":"尚未添加相机"} tone="warn">在<Link to="/camera" state={navigationState}>相机</Link>页添加相机并绑定设备。</Notice></main>}
  </div>;
}
