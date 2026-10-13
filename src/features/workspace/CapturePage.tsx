import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Link, useLocation, useSearchParams } from "react-router-dom";
import { cameraApi } from "../camera/api";
import { plcApi } from "../plc/api";
import { cycleApi, useCycle, useSiteStatus } from "../cycle/api";
import SamplePartSelect from "../cycle/components/SamplePartSelect";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, KV, Notice, NumberField, Panel, Steps, WorkspaceBar, WorkspaceEmpty, GrayViewer } from "./components";
import type { CaptureRound, GrayImage } from "./types";
import type { Verdict } from "../cycle/types";
import { expectedChoices } from "./progress";
import type { CaptureNavigationState } from "./captureNavigation";
import { productionBusy as isProductionBusy } from "../cycle/phase";

const stateLabels: Record<CaptureRound["state"],string> = { waitingStart:"等待现场启动", receiving:"接收中", draining:"PLC 已结束，等待在途图像", complete:"采集完成", failed:"采集异常" };
function captureCountText(round: Pick<CaptureRound,"state"|"receivedCount"|"plannedCount">) {
  if(round.state==="waitingStart") return `已接收 ${round.receivedCount} 张 · 等待 PLC 开始信号`;
  if(round.state==="complete") return `已接收 ${round.receivedCount} 张 · 数量核对通过`;
  return round.plannedCount == null ? `已接收 ${round.receivedCount} 张` : `已接收 ${round.receivedCount} / ${round.plannedCount} 张`;
}
type CaptureFrame = CaptureRound["frames"][number];
function frameSpec(frame: CaptureFrame) {
  const sizes = [...new Set(frame.views.map(v=>`${v.width}×${v.height}`))].join(" / ");
  return frame.views.length > 1 ? `每点 ${frame.views.length} 图 · ${sizes}` : sizes;
}
function roundSpec(frames: CaptureFrame[]) {
  const counts = new Map<string,number>();
  frames.forEach(f=>counts.set(frameSpec(f),(counts.get(frameSpec(f)) ?? 0)+1));
  const main = [...counts].sort((a,b)=>b[1]-a[1])[0]?.[0] ?? null;
  return { main, uniform: counts.size <= 1, differs: (f: CaptureFrame) => frameSpec(f) !== main };
}
export default function CapturePage() {
  const {data,doc,cameras,busy,dirty,frameDirty,act,setError} = useWorkspace();
  const {snapshot} = useCycle();
  const site = useSiteStatus();
  const [offlinePart,setOfflinePart] = useState(""), [offlineSending,setOfflineSending] = useState(false);
  const [search] = useSearchParams();
  const location = useLocation();
  const captureReturn = (location.state as CaptureNavigationState | null)?.captureReturn;
  const validation = search.get("purpose") === "validation";
  const [cameraId,setCameraId] = useState("");
  const [drain,setDrain] = useState(1500);
  const [round,setRound] = useState<CaptureRound | null>(null), [working,setWorking] = useState(false);
  const [history,setHistory] = useState<Awaited<ReturnType<typeof workspaceApi.captureList>>>([]);
  const [roundId,setRoundId] = useState<string | undefined>();
  const [previewK,setPreviewK] = useState(0), [preview,setPreview] = useState<GrayImage | null>(null), [previewError,setPreviewError] = useState("");
  const [plc,setPlc] = useState("正在读取 PLC 状态"), [plcConnected,setPlcConnected] = useState(false), [deviceReady,setDeviceReady] = useState(false);
  const [sampleExpected,setSampleExpected] = useState<Verdict>("OK");
  const [reuse,setReuse] = useState(false), [confirmed,setConfirmed] = useState(false);
  const selectedCamera = cameras.some(c=>c.id===cameraId) ? cameraId : cameras[0]?.id || "";
  const current = useRef({id:doc?.id,alive:true}); current.current.id=doc?.id;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;};},[]);
  useEffect(() => {
    let alive = true, pending = false;
    const refresh = async () => {
      if(pending) return; pending=true;
      try {
        const [r,status,devices,rounds] = await Promise.all([workspaceApi.captureGet(roundId),plcApi.getStatus(),cameraApi.rigStatus(),doc ? workspaceApi.captureList(doc.id) : Promise.resolve([])]);
        if(alive) { setHistory(rounds); setRound(r?.recipeId === doc?.id ? r : null); setPlc(status.message || status.state); setPlcConnected(status.state === "connected"); setDeviceReady(devices.some(d => d.id === selectedCamera && d.ready)); }
      } catch(e) { if(alive) setError(errorText(e)); } finally { pending=false; }
    };
    void refresh(); const timer=setInterval(() => void refresh(),700);
    return () => {alive=false;clearInterval(timer);};
  },[doc?.id,selectedCamera,setError,roundId]);
  useEffect(() => { setConfirmed(false); },[round?.roundId,validation]);
  useEffect(() => {
    const restored = captureReturn?.recipeId === doc?.id ? captureReturn : undefined;
    setRoundId(restored?.roundId); setPreviewK(restored?.previewK ?? 0); setWorking(false);
    if(restored) { setCameraId(restored.cameraId); setDrain(restored.drain); }
  },[doc?.id,captureReturn]);
  useEffect(() => {
    let alive=true; setPreview(null); setPreviewError("");
    if(round?.frames[previewK]) void workspaceApi.captureImage(round.roundId,previewK).then(image=>{if(alive)setPreview(image);}).catch(e=>{if(alive)setPreviewError(errorText(e));});
    return () => {alive=false;};
  },[round?.roundId,round?.frames.length,previewK]);
  if(!doc || !data) return <WorkspaceEmpty/>;
  const active=!!round && ["waitingStart","receiving","draining"].includes(round.state);
  const locked=busy || working || dirty || frameDirty || !!data.workspace.pending;
  const productionBusy=isProductionBusy(snapshot);
  const sampleSourceInvalid=validation && (!data.workspace.captureId || data.workspace.captureId===round?.roundId);
  const captureState = {captureReturn:{recipeId:doc.id,search:location.search,cameraId:selectedCamera,drain,roundId,previewK}};
  const navigationState: CaptureNavigationState = {...captureState, returnTo:{pathname:location.pathname,search:location.search,label:validation ? "独立样本采集" : "整圈采集",state:captureState}};
  const startBlocked = !plcConnected ? "PLC 未连接：整圈采集依赖 PLC 的开始 / 结束信号与触发计数。" : !deviceReady ? "采集设备未就绪。" : productionBusy ? "当前工件结束后才能占用设备采集。" : "";
  const start=async () => {
    if(locked || active) return; setWorking(true); setConfirmed(false);
    try { const next=await workspaceApi.captureStart(doc.id,selectedCamera,drain); if(current.current.alive && current.current.id===doc.id) {setRoundId(next.roundId); setRound(next); setPreviewK(0);} }
    catch(e) {if(current.current.alive && current.current.id===doc.id)setError(errorText(e));} finally {if(current.current.alive && current.current.id===doc.id)setWorking(false);}
  };
  const offlineSampleSet = site?.mode==="offline" && site.source==="sampleSet" ? site.sampleSet : null;
  const offlineReady = !!offlineSampleSet && !site?.running && round?.state==="waitingStart";
  const sendOffline=async () => {
    if(offlineSending) return; setOfflineSending(true);
    try { await cycleApi.offlineCapture(offlinePart && offlineSampleSet?.parts.some(p=>p.partId===offlinePart) ? offlinePart : null); }
    catch(e) {if(current.current.alive && current.current.id===doc.id)setError(errorText(e));} finally {if(current.current.alive)setOfflineSending(false);}
  };
  const stop=async () => { if(locked) return; setWorking(true); try {const next=await workspaceApi.captureStop();if(current.current.alive && current.current.id===doc.id)setRound(next);} catch(e) {if(current.current.alive && current.current.id===doc.id)setError(errorText(e));} finally {if(current.current.alive && current.current.id===doc.id)setWorking(false);} };
  const spec = round ? roundSpec(round.frames) : null;
  const shotKey=(e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.key==="ArrowDown" || e.key==="ArrowRight" ? 1 : e.key==="ArrowUp" || e.key==="ArrowLeft" ? -1 : 0;
    if(!step || !round?.frames.length) return;
    e.preventDefault();
    const next=Math.min(round.frames.length-1,Math.max(0,previewK+step));
    setPreviewK(next); (e.currentTarget.children[next] as HTMLElement | undefined)?.focus();
  };
  return <div className="wp-page"><WorkspaceBar/><Steps/><div className="wp-columns"><div className="wp-stack"><Panel title={validation ? "实拍独立验证样本" : "设备准备与整圈采集"} detail="选择一个 SDK device；每次触发的一张拼接图生成一个拍照点">
    <label className="field"><span>采集 device</span><select className="input" aria-label="采集 device" value={selectedCamera} disabled={locked || active} onChange={e=>setCameraId(e.target.value)}>{cameras.map(c=><option key={c.id} value={c.id}>{c.name} · {c.serial || c.source}</option>)}</select></label>
    <KV label="设备">{deviceReady ? "就绪" : "未就绪"}</KV><KV label="PLC">{plc}</KV>
    <div className="wp-form-grid"><NumberField label="在途图像等待时间" value={drain} min={200} max={30000} unit="ms" onChange={setDrain} disabled={locked || active}/></div>
    <div className="wp-actions"><Link className="btn" to="/camera" state={navigationState}>配置设备</Link><Link className="btn" to="/plc" state={navigationState}>PLC 通讯</Link><Link className="btn" to="/camera/calibration" state={navigationState}>飞拍工位标定</Link></div>
    <Notice title="现场启动机械臂">先确认合格涂胶工件、轨迹和触发顺序。GS 开始接收并就绪后，由现场启动机械臂；触发数量由 PLC 自动提供，拍照点数量依据实际收图生成。</Notice>
    <div className="wp-actions"><button className="btn primary" disabled={locked || active || !!startBlocked || !selectedCamera || !Number.isFinite(drain) || drain<200 || drain>30000} onClick={() => void start()}>开始接收 / 整圈重采</button><button className="btn danger" disabled={locked || !active} onClick={() => void stop()}>中止本轮</button></div>
    {startBlocked && !active && <Notice title="暂不能开始接收" tone="warn"><span>{startBlocked}</span>{!plcConnected ? <Link className="btn small" to="/plc" state={navigationState}>前往 PLC 通讯</Link> : !deviceReady ? <Link className="btn small" to="/camera" state={navigationState}>前往设备与采集</Link> : null}</Notice>}
    {offlineSampleSet && <div className="wp-stack wp-offline-feed"><strong>离线样本集送一圈</strong><p className="muted">① 先点“开始接收”；② 选择样本件；③ 点“离线产线送一圈”，虚拟 PLC 发开始 / 结束边沿，图像来自所选样本件。</p><div className="wp-actions"><SamplePartSelect sampleSet={offlineSampleSet} nextPart={site?.nextPart ?? null} value={offlinePart} onChange={setOfflinePart} disabled={offlineSending || !offlineReady} label="离线样本件"/><button className="btn" disabled={offlineSending || !offlineReady} onClick={() => void sendOffline()}>离线产线送一圈</button></div>{!offlineReady && <p className="muted hint">{site?.running ? "离线产线正在连续运行，停止后再送一圈。" : "开始接收并等待现场启动后，才能送一圈。"}</p>}</div>}
  </Panel><Panel title="采集历史" detail="保留异常轮次供追溯，重新采集不会混用旧图"><select className="input" aria-label="采集历史" value={roundId ?? ""} disabled={active || working} onChange={e=>{setRoundId(e.target.value || undefined);setPreviewK(0);}}><option value="">最近轮次</option>{history.map(r=><option key={r.roundId} value={r.roundId}>{new Date(r.createdAt).toLocaleString("zh-CN")} · {stateLabels[r.state]} · {captureCountText(r)}</option>)}</select></Panel>{round && <Panel title="采集结果" actions={<Badge tone={round.state==="complete" ? "ok" : round.state==="failed" ? "warn" : "info"}>{stateLabels[round.state]}</Badge>}>
    <div className="wp-capture-summary">
      <span className="wp-capture-chip"><small>轮次</small><strong className="mono">{round.roundId}</strong></span>
      <span className="wp-capture-chip"><small>数量核对</small><strong>{captureCountText(round)}</strong></span>
      {round.plcActualCount != null && <span className="wp-capture-chip"><small>PLC 实际触发</small><strong>{round.plcActualCount} 次</strong></span>}
      {spec?.main && <span className={"wp-capture-chip" + (spec.uniform ? "" : " warn")}><small>图像规格</small><strong>{spec.uniform ? spec.main : `尺寸不一致 · 多数为 ${spec.main}`}</strong></span>}
      <Badge tone={round.simulated ? "warn" : "ok"}>{round.simulated ? "离线 / 模拟（不代表现场硬件验收）" : "设备实拍"}</Badge>
    </div>
    {round.error && <Notice title="本轮异常，整圈重采" tone="warn">{round.error}</Notice>}
    <div className="wp-capture-result">
    <GrayViewer image={preview} loading={!!round.frames[previewK] && !preview && !previewError} error={previewError} label={"拍照点 " + String(previewK+1).padStart(2,"0") + " · 完整拼接原图"}/>
    <div className="wp-capture-side">
    <div className="wp-capture-side-head"><strong>拍照点 · {round.frames.length}</strong><small>↑↓ 切换</small></div>
    {round.frames.length ? <div className="wp-capture-shots" aria-label="拍照点选择" onKeyDown={shotKey}>{round.frames.map((f,i)=>{
      const odd=!!spec?.differs(f), warn=odd || f.triggerCounter == null;
      return <button key={f.shotId} className={"wp-capture-shot" + (previewK===i ? " selected" : "")} aria-pressed={previewK===i} onClick={()=>setPreviewK(i)}>
        <strong>{String(i+1).padStart(2,"0")}</strong><i className={"wp-capture-dot" + (warn ? " warn" : "")}/><span>帧 {f.frameCounter} · 触发 {f.triggerCounter ?? "未提供"}</span>{odd && <small>{frameSpec(f)}</small>}
      </button>;
    })}</div> : <p className="muted">尚未收到图像</p>}
    {round.state==="complete" && <div className="wp-capture-actions">{!validation && data.workspace.frames.length>0 && <><label className="check"><input type="checkbox" checked={reuse} disabled={locked} onChange={e=>setReuse(e.target.checked)}/>带入旧中线与参数作为草稿</label>{reuse && <label className="check"><input type="checkbox" checked={confirmed} disabled={locked} onChange={e=>setConfirmed(e.target.checked)}/>已确认轨迹和触发顺序保持一致，拍照点对应正确</label>}</>}
      <label className="check" hidden={!validation}><input type="checkbox" checked={confirmed} disabled={locked} onChange={e=>setConfirmed(e.target.checked)}/>已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致</label>
      {validation && <label className="field"><span>样本人工期望</span><select className="input" aria-label="样本人工期望" value={sampleExpected} disabled={locked} onChange={e=>setSampleExpected(e.target.value as Verdict)}>{expectedChoices.map(([v,l])=><option key={v} value={v}>{l}</option>)}</select></label>}
      {sampleSourceInvalid && <Notice title="需要独立于示教的采集轮次" tone="warn">{data.workspace.captureId ? "当前轮次已用于示教，请重新采集一圈作为验证样本。" : "请先采用示教采集轮次，再采集独立验证样本。"}</Notice>}
      <button className="btn primary" disabled={locked || productionBusy || sampleSourceInvalid || ((validation || reuse) && !confirmed)} onClick={() => void act(() => validation ? workspaceApi.captureSample(doc.id,data.workspace.revision,round.roundId,sampleExpected,confirmed) : workspaceApi.adoptCapture(doc.id,data.workspace.revision,round.roundId,reuse,confirmed),validation ? "独立验证样本已加入样本库" : "已采用本轮图像，请逐点选择检测图并示教")}>{validation ? "保存为独立验证样本" : "采用本轮图像"}</button>
      <Link className="btn" to={validation ? "/recipe/validation" : "/recipe/teach"}>{validation ? "进入验证" : "进入单帧示教"}</Link>
    </div>}
    </div></div>
  </Panel>}</div><div className="wp-stack"><Notice title="数量相同不代表对应相同">每轮独立保存。缺帧、重复、超量、断连或无法确定归属时整圈重采；不会把下一张图补到缺失位置。</Notice><Notice title="结束条件">收齐计划数量后仍等待 PLC 结束通知，再等待在途图像。PLC 实际计划和完成计数均由后端核对。</Notice><Link className="btn" to="/recipe/progress">返回进度总览</Link></div></div></div>;
}
