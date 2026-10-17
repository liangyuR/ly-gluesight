import { useEffect, useRef, useState, type ReactNode } from "react";
import { Activity, AlertTriangle, Check, Clock, Database, Info, Minus, Radio, Unplug, Zap } from "lucide-react";
import { cameraApi, usePreviewCanvas } from "../api";
import type { CameraConfig, CameraStatus, Frame } from "../types";
import { ago, clock, count, useNow } from "./health";

export default function FramePreview({ cam, status, lastFrame, config, blockedReason, actions }: { cam: number; status: CameraStatus | null; lastFrame: Frame | undefined; config: CameraConfig | null; blockedReason?: string; actions?: ReactNode }) {
  const scope=JSON.stringify([cam,config?.id,config?.source,config?.serial,config?.triggerSource,config?.viewCount]);
  const [selectedView, setSelectedView] = useState({scope, view: 1});
  const view = selectedView.scope === scope && selectedView.view <= (config?.viewCount ?? 1) ? selectedView.view : 1;
  const { img, canvas } = usePreviewCanvas(cam, lastFrame?.frameCounter,250,scope,view);
  const [error, setError] = useState("");
  const [busy,setBusy]=useState(false);
  const pending=useRef(false),serial=useRef(0),mounted=useRef(true);
  const currentScope=useRef(scope);currentScope.current=scope;
  const now=useNow(1000);
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;serial.current++;};},[]);
  useEffect(()=>{serial.current++;pending.current=false;setBusy(false);setError("");},[scope]);

  const mvs = config?.source === "mvs";
  const software = config?.triggerSource === "Software";
  const unbound = mvs && !config?.serial;
  const reason = blockedReason || (!status?.ready ? (status?.message || "相机未连接") : "");
  const canTrigger = !reason && mvs && software;
  const soft = async () => {
    if(pending.current||!canTrigger)return;
    const request=++serial.current;
    pending.current=true;setBusy(true);
    setError("");
    try{await cameraApi.softTrigger(cam);}
    catch(e){if(mounted.current&&currentScope.current===scope&&serial.current===request)setError(String(e));}
    finally{if(mounted.current&&currentScope.current===scope&&serial.current===request){pending.current=false;setBusy(false);}}
  };
  const age = lastFrame ? now - lastFrame.ts : null;
  const fault = !unbound && !status?.ready && !status?.released;
  const mode = !mvs ? <><Database size={12} />样本集 · 按节拍自动出图</> : software ? <><Zap size={12} />Software · 软触发取图</> : <><Radio size={12} />Line0 · 等机器人触发</>;
  const hint = reason ? reason : !mvs ? "离线图像源按节拍自动出图，不需要也不能软触发" : software ? "点「取一帧」软触发一次" : "触发源为 Line0 时由机器人到位触发自动出图；改为 Software 后可在此软触发取一帧。";

  return (
    <section className="panel" aria-label="出图验证">
      <div className="cg-panel-head">
        <h3 className="panel-title">出图验证</h3>
        <span className="cg-badge off">{mode}</span>
        {config?.viewCount === 3 && <div className="segmented" role="group" aria-label="预览视角">{[1, 2, 3].map(value => <button key={value} className={value === view ? "active" : ""} aria-pressed={value === view} onClick={() => setSelectedView({scope, view: value})}>视角 {value}</button>)}</div>}
        {status?.ready && <span className="cg-note mono">{status.fps.toFixed(1)} fps{status.maxFps ? ` · 上限 ${status.maxFps.toFixed(1)}` : ""}</span>}
      </div>
      {unbound ? <div className="cg-na"><AlertTriangle size={16} color="var(--warn)" /><div><strong>未绑定设备</strong>在左侧“绑定设备”里选择设备，保存并应用后在这里验证出图。</div></div> : <>
        {(img || !reason || fault) && <div className={"cg-preview" + (age != null && age < 1500 && !fault ? " fresh" : "")} style={{ minHeight: img ? undefined : 140 }}>
          <canvas aria-label="相机最新图像" role="img" ref={canvas} style={{ display: img ? "block" : "none", opacity: fault ? .28 : 1 }} />
          {!img && !fault && <span className="cg-thumb-empty">等待第一帧</span>}
          {img && !fault && age != null && <span className={"cg-tag " + (age < 3000 ? "fresh" : "stale")}>{age < 3000 ? <Activity size={11} /> : <Clock size={11} />}{ago(age)} · {clock(lastFrame!.ts, true)}</span>}
          {img && <span className="cg-tag size">{img.fullWidth}×{img.fullHeight} · 缩略图</span>}
          {fault && <div className="cg-thumb-fault"><strong><Unplug size={14} />{status?.message || "相机未连接"}</strong>{lastFrame && <span>最后一帧 {clock(lastFrame.ts)} · 后台每 2 秒自动重连</span>}</div>}
        </div>}
        <div className="cg-stats">
          <div className="cg-stat"><span>帧计数</span><b>{count(lastFrame?.frameCounter)}</b></div>
          <div className="cg-stat"><span>触发计数</span><b>{count(lastFrame?.triggerCounter)}</b></div>
          <div className={"cg-stat" + (fault ? " fault" : "")}><span>丢包 / 丢帧</span><b className={mvs ? undefined : "text"}>{mvs ? `${count(status?.lostPackets)} / ${count(status?.droppedFrames)}` : "不适用"}</b></div>
          <div className={"cg-stat" + (fault ? " fault" : "")}><span>最近出图</span><b className="text">{age == null ? "本页打开后未出图" : ago(age)}</b></div>
        </div>
      </>}
      <div className="cg-actions">
        {mvs && !unbound && <button className="btn" onClick={()=>void soft()} disabled={busy||!canTrigger}><Zap size={15} />{busy?"取图中…":"取一帧"}</button>}
        <span className={reason && !unbound ? "c-warn" : undefined}>{unbound ? "" : hint}</span>
        <span className="spacer" />
        {actions}
      </div>
      {error && <span className="cg-err">{error}</span>}
    </section>
  );
}

/** 参数是否生效：设备不回读逐字段的值，只能以保存返回与状态里的“未接受”列表为准。 */
export function AppliedPanel({ config, status }: { config: CameraConfig; status: CameraStatus | null }) {
  const warnings = status?.warnings ?? [];
  const body = config.source !== "mvs"
    ? <><span className="cg-state off"><Minus size={14} />不适用</span><p>离线图像源没有设备参数可写入，也不会产生“未接受”列表。</p></>
    : !config.serial ? <><span className="cg-state off"><Minus size={14} />未绑定设备</span><p>绑定并保存后由设备确认参数。</p></>
    : !status?.ready ? <><span className="cg-state off"><Info size={14} />{status?.released ? "已释放给 MVS" : "相机未连接"}</span><p>重新连接时按已保存的参数写入相机，写入结果在这里显示。</p></>
    : warnings.length ? <><span className="cg-state warn"><AlertTriangle size={14} />已应用，{warnings.length} 项参数相机未接受</span><ul>{warnings.map(w => <li key={w}>{w}</li>)}</ul><p>其余参数已生效；这些项相机保持写入前的值。配置已按新值保存，修正后重新“保存并应用”，或在海康 MVS 客户端核对该型号支持的范围。</p></>
    : <><span className="cg-state ok"><Check size={14} />已应用，相机全部接受</span><p>设备不回读逐字段的值；是否生效以保存返回和状态里的“未接受”列表为准，列表为空即全部生效。调试曝光 / 增益不在此列表内。</p></>;
  return <section className="panel" aria-label="参数生效情况"><h3 className="panel-title">参数生效情况</h3><div className="cg-applied">{body}</div></section>;
}
