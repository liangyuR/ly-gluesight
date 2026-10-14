import { useEffect, useRef, useState } from "react";
import { Zap } from "lucide-react";
import { cameraApi, usePreviewCanvas } from "../api";
import type { CameraConfig, CameraStatus, Frame } from "../types";

function useNow(ms: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const t = setInterval(() => setNow(Date.now()), ms); return () => clearInterval(t); }, [ms]);
  return now;
}

function ago(ms: number) {
  if (ms < 2000) return "刚刚";
  if (ms < 60_000) return `${Math.floor(ms / 1000)} 秒前`;
  return `${Math.floor(ms / 60_000)} 分钟前`;
}

export default function FramePreview({ cam, status, lastFrame, config, blockedReason }: { cam: number; status: CameraStatus | null; lastFrame: Frame | undefined; config: CameraConfig | null; blockedReason?: string }) {
  const scope=JSON.stringify([cam,config?.id,config?.source,config?.serial,config?.triggerSource,config?.viewCount]);
  const [selectedView, setSelectedView] = useState({scope, view: 1});
  const view = selectedView.scope === scope && selectedView.view <= (config?.viewCount ?? 1) ? selectedView.view : 1;
  const { img, canvas } = usePreviewCanvas(cam, lastFrame?.frameCounter,250,scope,view);
  const [error, setError] = useState("");
  const [busy,setBusy]=useState(false);
  const [arrived,setArrived]=useState<{key:string;at:number}|null>(null);
  const pending=useRef(false),serial=useRef(0),mounted=useRef(true);
  const currentScope=useRef(scope);currentScope.current=scope;
  const now=useNow(1000);
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;serial.current++;};},[]);
  const seen=useRef(lastFrame?.frameCounter);
  useEffect(()=>{serial.current++;pending.current=false;setBusy(false);setError("");setArrived(null);},[scope]);
  useEffect(()=>{
    if(lastFrame==null||lastFrame.frameCounter===seen.current)return;
    seen.current=lastFrame.frameCounter;
    setArrived({key:scope,at:Date.now()});
  },[lastFrame?.frameCounter]);

  const software = config?.triggerSource === "Software";
  const reason = blockedReason || (!status?.ready ? (status?.message || "相机未连接") : "");
  const canTrigger = !reason && config?.source === "mvs" && software;
  const soft = async () => {
    if(pending.current||!canTrigger)return;
    const request=++serial.current;
    pending.current=true;setBusy(true);
    setError("");
    try{await cameraApi.softTrigger(cam);}
    catch(e){if(mounted.current&&currentScope.current===scope&&serial.current===request)setError(String(e));}
    finally{if(mounted.current&&currentScope.current===scope&&serial.current===request){pending.current=false;setBusy(false);}}
  };
  const fresh = arrived?.key === scope ? arrived : null;
  const hint = reason ? reason : config?.source !== "mvs" ? "离线图像源按节拍自动出图" : software ? "点「软触发一次」取一帧" : "触发源为 Line0：机器人到位触发后自动显示";

  return (
    <section className="panel cam-test" aria-label="出图测试">
      <div className="panel-head">
        <h3 className="panel-title">出图测试</h3>
        <span className="spacer" />
        {config?.viewCount === 3 && <div className="segmented" role="group" aria-label="预览视角">{[1, 2, 3].map(value => <button key={value} className={value === view ? "active" : ""} aria-pressed={value === view} onClick={() => setSelectedView({scope, view: value})}>视角 {value}</button>)}</div>}
        {config?.source === "mvs" && software && <button className="btn" onClick={()=>void soft()} disabled={busy||!canTrigger}><Zap size={15} />{busy?"取图中…":"软触发一次"}</button>}
      </div>
      <p className={reason ? "c-warn cam-test-hint" : "muted cam-test-hint"}>{hint}</p>
      <div className={"preview-box"+(fresh&&now-fresh.at<1500?" fresh":"")}>
        <canvas aria-label="相机最新图像" role="img" ref={canvas} style={{ display: img ? "block" : "none" }} />
        {!img && <span className="muted">{reason ? "暂无图像" : "等待第一帧"}</span>}
      </div>
      {lastFrame && <span className="muted mono cam-test-stats">
        {fresh ? ago(now - fresh.at) : "打开本页前"} · 帧 {lastFrame.frameCounter} · 触发 {lastFrame.triggerCounter}
        {` · 丢包 ${lastFrame.lostPackets}`}
        {img && ` · ${img.fullWidth}×${img.fullHeight}`}
      </span>}
      {error && <span className="c-ng" style={{ fontSize: 12 }}>{error}</span>}
    </section>
  );
}
