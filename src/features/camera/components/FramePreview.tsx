import { useEffect, useRef, useState } from "react";
import { Zap } from "lucide-react";
import { cameraApi, usePreviewCanvas } from "../api";
import type { CameraConfig, CameraStatus, Frame } from "../types";

export default function FramePreview({ cam, status, lastFrame, config }: { cam: number; status: CameraStatus | null; lastFrame: Frame | undefined; config: CameraConfig | null }) {
  const scope=JSON.stringify([cam,config?.id,config?.source,config?.acquisition,config?.serial,config?.replayDir,config?.replayChannel,config?.triggerSource,config?.viewCount]);
  const [selectedView, setSelectedView] = useState({scope, view: 1});
  const view = selectedView.scope === scope && selectedView.view <= (config?.viewCount ?? 1) ? selectedView.view : 1;
  const { img, canvas } = usePreviewCanvas(cam, lastFrame?.frameCounter,250,scope,view);
  const [error, setError] = useState("");
  const [busy,setBusy]=useState(false);
  const pending=useRef(false),serial=useRef(0),mounted=useRef(true);
  const currentScope=useRef(scope);currentScope.current=scope;
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;serial.current++;};},[]);
  useEffect(()=>{serial.current++;pending.current=false;setBusy(false);setError("");},[scope]);

  const soft = async () => {
    if(pending.current||!canTrigger)return;
    const request=++serial.current;
    pending.current=true;setBusy(true);
    setError("");
    try{await cameraApi.softTrigger(cam);}
    catch(e){if(mounted.current&&currentScope.current===scope&&serial.current===request)setError(String(e));}
    finally{if(mounted.current&&currentScope.current===scope&&serial.current===request){pending.current=false;setBusy(false);}}
  };
  const canTrigger = !!status?.ready&&(config?.source === "replay" || (config?.acquisition === "triggered" && config.source === "mvs" && config.triggerSource === "Software"));

  return (
    <div className="panel">
      <div className="panel-head">
        <h3 className="panel-title">最近一帧</h3>
        {lastFrame && (
          <span className="muted mono">
            帧 {lastFrame.frameCounter} · 触发 {lastFrame.triggerCounter}
            {lastFrame.lostPackets ? ` · 丢包 ${lastFrame.lostPackets}` : ""}
            {img && ` · ${img.fullWidth}×${img.fullHeight}`}
          </span>
        )}
        <span className="spacer" />
        {config?.viewCount === 3 && <label className="field"><select aria-label="预览视角" className="input" value={view} onChange={e => setSelectedView({scope, view: Number(e.target.value)})}>
          {[1, 2, 3].map(value => <option key={value} value={value}>视角 {value}</option>)}
        </select></label>}
        <button className="btn" onClick={()=>void soft()} disabled={busy||!canTrigger} title="回放相机，或触发源为 Software 的触发采集海康相机">
          <Zap size={15} />
          {busy?"取图中…":config?.source === "replay" ? "下一张" : "软触发一次"}
        </button>
      </div>
      <div className="preview-box">
        <canvas aria-label="相机最新图像" role="img" ref={canvas} style={{ display: img ? "block" : "none" }} />
        {!img && <span className="muted">{status?.ready ? "暂无图像（仅显示 Mono8）" : status?.message||"等待相机连接"}</span>}
      </div>
      {error && <span className="c-ng" style={{ fontSize: 12 }}>{error}</span>}
    </div>
  );
}
