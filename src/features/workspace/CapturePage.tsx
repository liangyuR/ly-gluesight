import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { cameraApi } from "../camera/api";
import { plcApi } from "../plc/api";
import { useCycle } from "../cycle/api";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, KV, Notice, NumberField, Panel, Steps, WorkspaceBar, WorkspaceEmpty, GrayViewer } from "./components";
import type { CaptureRound, GrayImage } from "./types";

const stateLabels: Record<CaptureRound["state"],string> = { waitingStart:"等待现场启动", receiving:"接收中", draining:"PLC 已结束，等待在途图像", complete:"采集完成", failed:"采集异常" };
export default function CapturePage() {
  const {data,doc,cameras,busy,dirty,frameDirty,act,setError} = useWorkspace();
  const {snapshot} = useCycle();
  const [search] = useSearchParams();
  const validation = search.get("purpose") === "validation";
  const [cameraId,setCameraId] = useState("");
  const [planned,setPlanned] = useState(20), [drain,setDrain] = useState(1500);
  const [round,setRound] = useState<CaptureRound | null>(null), [working,setWorking] = useState(false);
  const [history,setHistory] = useState<Awaited<ReturnType<typeof workspaceApi.captureList>>>([]);
  const [roundId,setRoundId] = useState<string | undefined>();
  const [previewK,setPreviewK] = useState(0), [preview,setPreview] = useState<GrayImage | null>(null), [previewError,setPreviewError] = useState("");
  const [plc,setPlc] = useState("正在读取 PLC 状态"), [deviceReady,setDeviceReady] = useState(false);
  const [reuse,setReuse] = useState(false), [confirmed,setConfirmed] = useState(false);
  const selectedCamera = cameraId || cameras[0]?.id || "";
  const current = useRef({id:doc?.id,alive:true}); current.current.id=doc?.id;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;};},[]);
  useEffect(() => {
    let alive = true, pending = false;
    const refresh = async () => {
      if(pending) return; pending=true;
      try {
        const [r,status,devices,rounds] = await Promise.all([workspaceApi.captureGet(roundId),plcApi.getStatus(),cameraApi.rigStatus(),doc ? workspaceApi.captureList(doc.id) : Promise.resolve([])]);
        if(alive) { setHistory(rounds); setRound(r?.recipeId === doc?.id ? r : null); setPlc(status.message || status.state); setDeviceReady(devices.some(d => d.id === selectedCamera && d.ready)); }
      } catch(e) { if(alive) setError(String(e)); } finally { pending=false; }
    };
    void refresh(); const timer=setInterval(() => void refresh(),700);
    return () => {alive=false;clearInterval(timer);};
  },[doc?.id,selectedCamera,setError,roundId]);
  useEffect(() => { setConfirmed(false); },[round?.roundId,validation]);
  useEffect(() => { setRoundId(undefined); setPreviewK(0); setWorking(false); },[doc?.id]);
  useEffect(() => {
    let alive=true; setPreview(null); setPreviewError("");
    if(round?.frames[previewK]) void workspaceApi.captureImage(round.roundId,previewK).then(image=>{if(alive)setPreview(image);}).catch(e=>{if(alive)setPreviewError(String(e));});
    return () => {alive=false;};
  },[round?.roundId,round?.frames.length,previewK]);
  if(!doc || !data) return <WorkspaceEmpty/>;
  const active=!!round && ["waitingStart","receiving","draining"].includes(round.state);
  const locked=busy || working || dirty || frameDirty || !!data.workspace.pending;
  const productionBusy=!!snapshot && !["IDLE","FAULT"].includes(snapshot.phase);
  const sampleSourceInvalid=validation && (!data.workspace.captureId || data.workspace.captureId===round?.roundId);
  const start=async () => {
    if(locked || active) return; setWorking(true); setConfirmed(false);
    try { const next=await workspaceApi.captureStart(doc.id,selectedCamera,planned,drain); if(current.current.alive && current.current.id===doc.id) {setRoundId(next.roundId); setRound(next); setPreviewK(0);} }
    catch(e) {if(current.current.alive && current.current.id===doc.id)setError(String(e));} finally {if(current.current.alive && current.current.id===doc.id)setWorking(false);}
  };
  const stop=async () => { if(locked) return; setWorking(true); try {const next=await workspaceApi.captureStop();if(current.current.alive && current.current.id===doc.id)setRound(next);} catch(e) {if(current.current.alive && current.current.id===doc.id)setError(String(e));} finally {if(current.current.alive && current.current.id===doc.id)setWorking(false);} };
  return <div className="wp-page"><WorkspaceBar/><Steps/><div className="wp-columns"><div className="wp-stack"><Panel title={validation ? "实拍独立验证样本" : "设备准备与整圈采集"} detail="选择一个 SDK device；每次触发的一张拼接图生成一个拍照点">
    <label className="field"><span>采集 device</span><select className="input" aria-label="采集 device" value={selectedCamera} disabled={locked || active} onChange={e=>setCameraId(e.target.value)}>{cameras.map(c=><option key={c.id} value={c.id}>{c.name} · {c.serial || c.source}</option>)}</select></label>
    <KV label="设备">{deviceReady ? "就绪" : "未就绪"}</KV><KV label="PLC">{plc}</KV>
    <div className="wp-form-grid"><NumberField label="PLC 计划触发次数" value={planned} min={1} max={64} step={1} onChange={setPlanned} disabled={locked || active}/><NumberField label="在途图像等待时间" value={drain} min={200} max={30000} unit="ms" onChange={setDrain} disabled={locked || active}/></div>
    <div className="wp-actions"><Link className="btn" to="/camera">配置设备</Link><Link className="btn" to="/plc">配置 PLC</Link><Link className="btn" to="/camera/calibration">毫米标定</Link></div>
    <Notice title="现场启动机械臂">先确认合格涂胶工件、轨迹和触发顺序。GS 开始接收并就绪后，由现场启动机械臂；拍照点数量依据实际收图生成。</Notice>
    <div className="wp-actions"><button className="btn primary" disabled={locked || active || productionBusy || !deviceReady || !selectedCamera || !Number.isInteger(planned) || planned<1 || planned>64 || !Number.isFinite(drain) || drain<200 || drain>30000} onClick={() => void start()}>开始接收 / 整圈重采</button><button className="btn danger" disabled={locked || !active} onClick={() => void stop()}>中止本轮</button></div>
    {productionBusy && <p className="muted">当前工件结束后才能占用设备采集。</p>}
  </Panel><Panel title="采集历史" detail="保留异常轮次供追溯，重新采集不会混用旧图"><select className="input" aria-label="采集历史" value={roundId ?? ""} disabled={active || working} onChange={e=>{setRoundId(e.target.value || undefined);setPreviewK(0);}}><option value="">最近轮次</option>{history.map(r=><option key={r.roundId} value={r.roundId}>{new Date(r.createdAt).toLocaleString("zh-CN")} · {stateLabels[r.state]} · {r.receivedCount}/{r.plannedCount}</option>)}</select></Panel>{round && <Panel title="采集结果" actions={<Badge tone={round.state==="complete" ? "ok" : round.state==="failed" ? "warn" : "info"}>{stateLabels[round.state]}</Badge>}>
    <KV label="轮次">{round.roundId}</KV><KV label="数量核对">实收 {round.receivedCount} / 配置计划 {round.plannedCount}</KV><KV label="PLC 实际证据">计划 {round.plcPlannedCount ?? "待收到"} · 完成 {round.plcActualCount ?? "待收到"}</KV><KV label="数据来源">{round.simulated ? "模拟 / 回放（不代表现场硬件验收）" : "设备实拍"}</KV>
    {round.error && <Notice title="本轮异常，整圈重采" tone="warn">{round.error}</Notice>}
    <div className="table-wrap"><table className="table"><thead><tr><th>拍照点</th><th>帧号</th><th>触发计数</th><th>图像</th></tr></thead><tbody>{round.frames.map((f,i)=><tr key={f.shotId}><td><button className="btn small" onClick={()=>setPreviewK(i)}>{String(i+1).padStart(2,"0")}</button></td><td>{f.frameCounter}</td><td>{f.triggerCounter ?? "未提供"}</td><td>{f.views.map(v=>`图 ${v.view} · ${v.width}×${v.height}`).join(" / ")}</td></tr>)}</tbody></table></div>
    <GrayViewer image={preview} loading={!!round.frames[previewK] && !preview && !previewError} error={previewError} label={"拍照点 " + (previewK+1) + " · 完整拼接原图"}/>
    {round.state==="complete" && <div className="wp-stack">{!validation && data.workspace.frames.length>0 && <><label className="check"><input type="checkbox" checked={reuse} disabled={locked} onChange={e=>setReuse(e.target.checked)}/>带入旧中线与参数作为草稿</label>{reuse && <label className="check"><input type="checkbox" checked={confirmed} disabled={locked} onChange={e=>setConfirmed(e.target.checked)}/>已确认轨迹和触发顺序保持一致，拍照点对应正确</label>}</>}
      <label className="check" hidden={!validation}><input type="checkbox" checked={confirmed} disabled={locked} onChange={e=>setConfirmed(e.target.checked)}/>已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致</label>
      {sampleSourceInvalid && <Notice title="需要独立于示教的采集轮次" tone="warn">{data.workspace.captureId ? "当前轮次已用于示教，请重新采集一圈作为验证样本。" : "请先采用示教采集轮次，再采集独立验证样本。"}</Notice>}
      <button className="btn primary" disabled={locked || productionBusy || sampleSourceInvalid || ((validation || reuse) && !confirmed)} onClick={() => void act(() => validation ? workspaceApi.captureSample(doc.id,data.workspace.revision,round.roundId,"OK",confirmed) : workspaceApi.adoptCapture(doc.id,data.workspace.revision,round.roundId,reuse,confirmed),validation ? "正常验证样本已加入样本库" : "已采用本轮图像，请逐点选择检测图并示教")}>{validation ? "保存为独立正常验证样本" : "采用本轮图像"}</button>
      <Link className="btn" to={validation ? "/recipe/validation" : "/recipe/teach"}>{validation ? "进入验证" : "进入单帧示教"}</Link>
    </div>}
  </Panel>}</div><div className="wp-stack"><Notice title="数量相同不代表对应相同">每轮独立保存。缺帧、重复、超量、断连或无法确定归属时整圈重采；不会把下一张图补到缺失位置。</Notice><Notice title="结束条件">收齐计划数量后仍等待 PLC 结束通知，再等待在途图像。PLC 实际计划和完成计数均由后端核对。</Notice><Link className="btn" to="/recipe/progress">返回进度总览</Link></div></div></div>;
}
