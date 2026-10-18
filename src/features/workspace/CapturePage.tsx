import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Link, useLocation, useSearchParams } from "react-router-dom";
import { cameraApi } from "../camera/api";
import { flyshotSpeedLimits } from "../camera/flyshot";
import type { CameraStatus } from "../camera/types";
import { storageApi } from "../../lib/storage";
import { plcApi } from "../plc/api";
import { linkStateLabels } from "../plc/meta";
import type { LinkState } from "../plc/types";
import { cycleApi, useCycle, useSiteStatus } from "../cycle/api";
import SamplePartSelect from "../cycle/components/SamplePartSelect";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { AlertTriangle, ArrowRight, Cable, Camera, CheckCircle2, Crosshair, Play } from "lucide-react";
import { scrollBehavior } from "./motion";
import SelectionTrack from "../../components/SelectionTrack";
import { Badge, Notice, NumberField, Panel, Steps, WorkspaceBar, WorkspaceEmpty, GrayViewer } from "./components";
import "./capture.css";
import type { CaptureRound, GrayImage } from "./types";
import { DEFAULT_IMAGING, type CameraImaging, type Verdict } from "../cycle/types";
import { expectedChoices } from "./progress";
import type { CaptureNavigationState } from "./captureNavigation";
import { productionBusy as isProductionBusy } from "../cycle/phase";

const stateLabels: Record<CaptureRound["state"],string> = { waitingStart:"等待现场启动", receiving:"接收中", draining:"PLC 已结束，等待在途图像", complete:"采集完成", failed:"采集异常" };
function captureCountText(round: {state: CaptureRound["state"]; receivedCount: number; plannedCount: number | null}) {
  if(round.state==="waitingStart") return `已接收 ${round.receivedCount} 张 · 等待 PLC 开始信号`;
  if(round.state==="complete") return `已接收 ${round.receivedCount} 张 · 数量核对通过`;
  return round.plannedCount == null ? `已接收 ${round.receivedCount} 张` : `已接收 ${round.receivedCount} / ${round.plannedCount} 张`;
}
const completeShots = (round: CaptureRound) => round.frames.filter(f=>f.cameras.length===round.cameras.length).length;
type CaptureFrame = CaptureRound["frames"][number];
function frameSpec(frame: CaptureFrame) {
  const sizes = [...new Set(frame.cameras.flatMap(c=>c.views).map(v=>`${v.width}×${v.height}`))].join(" / ");
  const counts = [...new Set(frame.cameras.map(c=>c.views.length))].join(" / ");
  if(frame.cameras.length > 1) return `每台 ${counts} 图 · ${sizes}`;
  return counts !== "1" ? `每点 ${counts} 图 · ${sizes}` : sizes;
}
function roundSpec(frames: CaptureFrame[]) {
  const counts = new Map<string,number>();
  frames.forEach(f=>counts.set(frameSpec(f),(counts.get(frameSpec(f)) ?? 0)+1));
  const main = [...counts].sort((a,b)=>b[1]-a[1])[0]?.[0] ?? null;
  return { main, uniform: counts.size <= 1, differs: (f: CaptureFrame) => frameSpec(f) !== main };
}
const imagingInvalid = (imaging: CameraImaging) => !(imaging.exposureUs>=1 && imaging.exposureUs<=1_000_000) ? "曝光时间需在 1–1000000 µs 之间" : !(Number.isFinite(imaging.gainDb) && imaging.gainDb>=0) ? "增益需为不小于 0 的有限数" : "";
export default function CapturePage() {
  const {data,doc,cameras,busy,dirty,frameDirty,act,setError} = useWorkspace();
  const {snapshot} = useCycle();
  const site = useSiteStatus();
  const [offlinePart,setOfflinePart] = useState(""), [offlineSending,setOfflineSending] = useState(false);
  const [search] = useSearchParams();
  const location = useLocation();
  const captureReturn = (location.state as CaptureNavigationState | null)?.captureReturn;
  const validation = search.get("purpose") === "validation";
  const [excluded,setExcluded] = useState<string[]>([]), [rig,setRig] = useState<CameraStatus[]>([]);
  const [flyPrefs,setFlyPrefs] = useState<{speed: number | null; spacing: number | null}>({speed:null,spacing:null}), [calib,setCalib] = useState<Record<string,number | null>>({});
  const [drain,setDrain] = useState(1500);
  const [round,setRound] = useState<CaptureRound | null>(null), [working,setWorking] = useState(false);
  const [history,setHistory] = useState<Awaited<ReturnType<typeof workspaceApi.captureList>>>([]);
  const [roundId,setRoundId] = useState<string | undefined>();
  const [previewK,setPreviewK] = useState(0), [previewCamera,setPreviewCamera] = useState(""), [preview,setPreview] = useState<{k: number; camera: string; image: GrayImage} | null>(null), [previewError,setPreviewError] = useState("");
  const [plcState,setPlcState] = useState<LinkState | null>(null);
  const [sampleExpected,setSampleExpected] = useState<Verdict>("OK");
  const [reuse,setReuse] = useState(false), [confirmed,setConfirmed] = useState(false);
  const [imagingDrafts,setImagingDrafts] = useState<Record<string,CameraImaging>>({}), [applyErrors,setApplyErrors] = useState<Record<string,string>>({});
  const chosen = cameras.filter(c=>!excluded.includes(c.id)).map(c=>c.id);
  const current = useRef({id:doc?.id,alive:true}); current.current.id=doc?.id;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;};},[]);
  useEffect(() => {
    let alive = true, pending = false;
    const refresh = async () => {
      if(pending) return; pending=true;
      try {
        const [r,status,devices,rounds] = await Promise.all([workspaceApi.captureGet(roundId),plcApi.getStatus(),cameraApi.rigStatus(),doc ? workspaceApi.captureList(doc.id) : Promise.resolve([])]);
        if(alive) { setHistory(rounds); setRound(r?.recipeId === doc?.id ? r : null); setPlcState(status.state); setRig(devices); }
      } catch(e) { if(alive) setError(errorText(e)); } finally { pending=false; }
    };
    void refresh(); const timer=setInterval(() => void refresh(),700);
    return () => {alive=false;clearInterval(timer);};
  },[doc?.id,setError,roundId]);
  useEffect(() => { setConfirmed(false); },[round?.roundId,validation]);
  useEffect(() => { let alive=true; void storageApi.preferences().then(p=>{ if(alive) setFlyPrefs({speed:p.flyshotSpeedMmS ?? null,spacing:p.flyshotSpacingMm ?? null}); }).catch(()=>{}); return () => {alive=false;}; },[]);
  const rigCams = JSON.stringify(rig.map(d=>[d.id,d.cam]));
  useEffect(() => {
    let alive=true;
    for(const [id,cam] of JSON.parse(rigCams) as [string,number][]) void cameraApi.calibInfo(cam,1).then(info=>{ if(alive) setCalib(c=>({...c,[id]:info?.mmPerPx ?? null})); }).catch(()=>{});
    return () => {alive=false;};
  },[rigCams]);
  useEffect(() => {
    const restored = captureReturn?.recipeId === doc?.id ? captureReturn : undefined;
    setRoundId(restored?.roundId); setPreviewK(restored?.previewK ?? 0); setWorking(false);
    if(restored) { setExcluded(restored.excludedCameraIds); setDrain(restored.drain); }
  },[doc?.id,captureReturn]);
  useEffect(() => { setPreview(null); },[round?.roundId]);
  const shownCamera = round?.cameras.some(c=>c.cameraId===previewCamera) ? previewCamera : round?.cameras[0]?.cameraId ?? "";
  const previewArrived = !!round?.frames[previewK]?.cameras.some(c=>c.cameraId===shownCamera);
  useEffect(() => {
    let alive=true; setPreviewError("");
    if(round && previewArrived) void workspaceApi.captureImage(round.roundId,previewK,shownCamera).then(async image=>{
      const decoded=new Image(); decoded.src=image.url; await Promise.race([decoded.decode?.().catch(()=>{}),new Promise(r=>setTimeout(r,150))]);
      if(alive)setPreview({k:previewK,camera:shownCamera,image});
    }).catch(e=>{if(alive){setPreview(null);setPreviewError(errorText(e));}});
    return () => {alive=false;};
  },[round?.roundId,previewArrived,previewK,shownCamera]);
  const active=!!round && ["waitingStart","receiving","draining"].includes(round.state);
  const [prepOpenFor,setPrepOpenFor] = useState<string | null>(null), [imagingOpen,setImagingOpen] = useState(false);
  const resultRef = useRef<HTMLDivElement>(null), wasActive = useRef(false);
  useEffect(() => {
    if(wasActive.current && !active && round) resultRef.current?.scrollIntoView?.({block:"start",behavior:scrollBehavior()});
    wasActive.current=active;
  },[active,round?.roundId]);
  if(!doc || !data) return <WorkspaceEmpty/>;
  const locked=busy || working || dirty || frameDirty || !!data.workspace.pending;
  const productionBusy=isProductionBusy(snapshot);
  const sampleSourceInvalid=validation && (!data.workspace.captureId || data.workspace.captureId===round?.roundId);
  const captureState = {captureReturn:{recipeId:doc.id,search:location.search,cameraIds:chosen,excludedCameraIds:excluded,drain,roundId,previewK}};
  const navigationState: CaptureNavigationState = {...captureState, returnTo:{pathname:location.pathname,search:location.search,label:validation ? "独立样本采集" : "整圈采集",state:captureState}};
  const nameOf = (id: string) => cameras.find(c=>c.id===id)?.name ?? id;
  const readyIds = rig.filter(d=>d.ready).map(d=>d.id);
  const notReady = chosen.filter(id=>!readyIds.includes(id));
  const plcConnected = plcState === "connected", plc = plcState ? linkStateLabels[plcState] : "读取中";
  const startBlocked = !plcConnected ? `PLC ${plcState === "connecting" ? "连接中" : "未连接"}：整圈采集依赖 PLC 的开始 / 结束信号与触发计数。` : !chosen.length ? "请至少选择一台采集相机。" : notReady.length ? `采集相机未就绪：${notReady.map(nameOf).join("、")}。` : productionBusy ? "当前工件结束后才能占用设备采集。" : "";
  const start=async () => {
    if(locked || active) return; setWorking(true); setConfirmed(false);
    try { const next=await workspaceApi.captureStart(doc.id,chosen,drain); if(current.current.alive && current.current.id===doc.id) {setRoundId(next.roundId); setRound(next); setPreviewK(0);} }
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
  const adopted = !validation && !!round && data.workspace.captureId === round.roundId;
  const savedSample = validation && !!round && data.workspace.sampleBank.some(sample => sample.id === round.roundId);
  const received = round ? completeShots(round) : 0;
  const progress = round?.plannedCount ? Math.min(1, received / round.plannedCount) : null;
  const shotKey=(e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.key==="ArrowDown" || e.key==="ArrowRight" ? 1 : e.key==="ArrowUp" || e.key==="ArrowLeft" ? -1 : 0;
    if(!step || !round?.frames.length) return;
    e.preventDefault();
    const next=Math.min(round.frames.length-1,Math.max(0,previewK+step));
    setPreviewK(next);
    const item=e.currentTarget.children[next] as HTMLElement | undefined;
    item?.focus({preventScroll:true}); item?.scrollIntoView({block:"nearest"});
  };
  const pad=(k: number) => "S" + String(k+1).padStart(3,"0");
  const shown=preview?.camera===shownCamera ? preview : null, shownK=shown?.k ?? previewK;
  const multi=(round?.cameras.length ?? 0) > 1;
  const finished=!!round && !active, collapsed=finished && prepOpenFor!==round.roundId;
  const savedImaging=(id: string) => data.workspace.doc.cameras?.[id] ?? data.workspace.unboundImaging?.[id] ?? DEFAULT_IMAGING;
  const imagingOf=(id: string) => imagingDrafts[id] ?? savedImaging(id);
  const changedImaging=chosen.filter(id=>{ const now=imagingOf(id), saved=savedImaging(id); return now.exposureUs!==saved.exposureUs || now.gainDb!==saved.gainDb || !!applyErrors[id]; });
  const editImaging=(id: string, key: keyof CameraImaging, value: number) => setImagingDrafts(d=>({...d,[id]:{...imagingOf(id),[key]:value}}));
  const applyImaging=() => {
    const next=Object.fromEntries(changedImaging.map(id=>{ const {exposureUs,gainDb}=imagingOf(id); return [id,{exposureUs,gainDb}]; }));
    void act(async () => { const saved=await workspaceApi.setCameraImaging(doc.id,data.workspace.revision,next); setApplyErrors(saved.applyErrors ?? {}); setImagingDrafts({}); return saved.view; },"曝光与增益已写入候选配方");
  };
  const toggleCamera=(id: string, on: boolean) => setExcluded(x=>on ? x.filter(c=>c!==id) : [...x.filter(c=>c!==id),id]);
  const startDisabled=locked || active || !!startBlocked || !chosen.length || !Number.isFinite(drain) || drain<200 || drain>30000;
  const drainInvalid = !Number.isFinite(drain) || drain<200 || drain>30000;
  const speedLimits = chosen.map(id => { const mm = offlineSampleSet?.mmPerPx ?? calib[id], fps = rig.find(d=>d.id===id)?.maxFps, exposure = imagingOf(id).exposureUs; return mm && fps && exposure && flyPrefs.spacing ? flyshotSpeedLimits({exposure,mmPerPx:mm,fps,spacing:flyPrefs.spacing}).max : null; });
  const maxSpeed = chosen.length && speedLimits.every(v=>v != null && Number.isFinite(v)) ? Math.floor(Math.min(...speedLimits as number[])) : null;
  const tooFast = maxSpeed != null && flyPrefs.speed != null && flyPrefs.speed > maxSpeed;
  const cameraProblem = !cameras.length ? "尚未配置" : !chosen.length ? "未选择相机" : notReady.length ? `${notReady.length} 台未就绪` : "";
  const imagingProblem = chosen.some(id=>!!imagingInvalid(imagingOf(id)) || !!applyErrors[id]);
  const pendingChecks = [!!cameraProblem, !plcConnected, drainInvalid].filter(Boolean).length;
  const startReason = startBlocked || (drainInvalid ? "在途图像等待时间需在 200–30000 ms 之间。" : "");
  const prepPanel = <Panel key="prep" title={validation ? "实拍独立验证样本" : "设备准备与整圈采集"} detail="每次 PLC 触发各相机各出一张图，共同组成一个拍照点" actions={<>{!active && <Badge tone={pendingChecks ? "warn" : "ok"}>{pendingChecks ? `${pendingChecks} 项未就绪` : "可以开始"}</Badge>}{finished && <button className="btn small" onClick={()=>setPrepOpenFor(null)}>收起</button>}</>}>
    <ul className="wp-capture-checks" aria-label="开始前检查">
      <li className={cameraProblem ? "bad" : ""}><span><Camera size={14} aria-hidden="true"/>相机</span><strong>{cameraProblem || `${chosen.length} 台就绪`}</strong><Link to="/camera" state={navigationState}>配置设备<ArrowRight size={12} aria-hidden="true"/></Link></li>
      <li className={plcConnected ? "" : "bad"}><span><Cable size={14} aria-hidden="true"/>PLC</span><strong>{plc}</strong><Link to="/plc" state={navigationState}>{plcConnected ? "PLC 通讯" : "前往 PLC 通讯"}<ArrowRight size={12} aria-hidden="true"/></Link></li>
      <li className={tooFast ? "bad" : ""} title={maxSpeed == null ? "需要标定当量、相机帧率、曝光与飞拍最小拍照点间距后核算" : tooFast ? `当前速度 ${flyPrefs.speed} mm/s 超出允许值` : undefined}><span><Crosshair size={14} aria-hidden="true"/>飞拍</span><strong>{maxSpeed == null ? "未核算" : `${tooFast ? "超速 · " : ""}最高 ${maxSpeed} mm/s`}</strong><Link to="/camera/calibration" state={navigationState}>飞拍标定<ArrowRight size={12} aria-hidden="true"/></Link></li>
      <li className={drainInvalid ? "bad" : ""}><NumberField label="在途图像等待时间" value={drain} min={200} max={30000} unit="ms" onChange={setDrain} disabled={locked || active}/></li>
    </ul>
    <div className="wp-capture-start">
      <button className="btn primary" disabled={startDisabled} onClick={() => void start()}><Play size={15} aria-hidden="true"/>开始接收</button>
      {active ? <button className="btn danger" disabled={locked} onClick={() => void stop()}>中止本轮</button> : startReason && <p className="wp-capture-reason"><AlertTriangle size={14} aria-hidden="true"/><span>{startReason}</span></p>}
    </div>
    {offlineSampleSet && <div className="wp-stack wp-offline-feed"><strong>离线样本集送一圈</strong><p className="muted">① 先点“开始接收”；② 选择样本件；③ 点“离线产线送一圈”，虚拟 PLC 发开始 / 结束边沿，图像来自所选样本件。</p><div className="wp-actions"><SamplePartSelect sampleSet={offlineSampleSet} nextPart={site?.nextPart ?? null} value={offlinePart} onChange={setOfflinePart} disabled={offlineSending || !offlineReady} label="离线样本件"/><button className="btn" disabled={offlineSending || !offlineReady} onClick={() => void sendOffline()}>离线产线送一圈</button></div>{!offlineReady && <p className="muted hint">{site?.running ? "离线产线正在连续运行，停止后再送一圈。" : "开始接收并等待现场启动后，才能送一圈。"}</p>}</div>}
    <details className="wp-capture-more">
      <summary>现场操作步骤</summary>
      <ol><li>确认合格涂胶工件、轨迹和触发顺序。</li><li>点“开始接收”，等待 GS 就绪。</li><li>现场启动机械臂；触发数量由 PLC 自动提供，每台相机都必须收到同样数量的图，拍照点数量依据实际收图生成。</li></ol>
    </details>
    <details className="wp-capture-more" open={imagingOpen || imagingProblem} onToggle={e=>setImagingOpen(e.currentTarget.open)}>
      <summary>采集相机与成像参数<small>{cameras.length ? `已选 ${chosen.length} / ${cameras.length} 台` : "尚未配置相机"}</small></summary>
      <div className="wp-capture-cameras" role="group" aria-label="采集相机">{cameras.map(c=>{
        const on=chosen.includes(c.id), imaging=imagingOf(c.id), invalid=on ? imagingInvalid(imaging) : "", ready=readyIds.includes(c.id);
        return <div key={c.id} className={"wp-capture-camera" + (on ? "" : " off")}>
          <div className="wp-capture-camera-head"><label className="check"><input type="checkbox" checked={on} disabled={locked || active} onChange={e=>toggleCamera(c.id,e.target.checked)}/>{c.name} · {c.serial || c.source}</label><Badge tone={ready ? "ok" : "warn"}>{ready ? "就绪" : "未就绪"}</Badge></div>
          {on && <div className="wp-form-grid"><NumberField label={`${c.name} 曝光时间（µs）`} value={imaging.exposureUs} min={1} max={1_000_000} onChange={v=>editImaging(c.id,"exposureUs",v)} disabled={locked || active}/><NumberField label={`${c.name} 增益（dB）`} value={imaging.gainDb} min={0} step={0.5} onChange={v=>editImaging(c.id,"gainDb",v)} disabled={locked || active}/></div>}
          {invalid ? <p className="c-ng" role="alert">{c.name}：{invalid}</p> : on && applyErrors[c.id] && <Notice title={`${c.name} 已写入候选配方，但没能下发到相机`} tone="warn">{applyErrors[c.id]}</Notice>}
        </div>;
      })}{!cameras.length && <p className="muted">配置相机后，可在此选择参与采集的相机并调整曝光与增益。</p>}</div>
      {cameras.length > 0 && <div className="wp-actions"><button className="btn" disabled={locked || active || !changedImaging.length || changedImaging.some(id=>!!imagingInvalid(imagingOf(id)))} onClick={applyImaging}>应用</button><span className="muted">按相机分别写入候选配方并立即下发</span></div>}
    </details>
  </Panel>;
  const historyPanel = <Panel key="history" title="采集历史" detail="保留异常轮次供追溯，重新采集不会混用旧图">{history.length ? <select className="input" aria-label="采集历史" value={roundId ?? ""} disabled={active || working} onChange={e=>{setRoundId(e.target.value || undefined);setPreviewK(0);}}><option value="">最近轮次</option>{history.map(r=><option key={r.roundId} value={r.roundId}>{new Date(r.createdAt).toLocaleString("zh-CN")} · {stateLabels[r.state]} · {r.cameraIds.length} 台相机 · {captureCountText(r)}</option>)}</select> : <p className="wp-capture-empty">暂无采集轮次。完成第一圈采集后，可在此回看各轮结果。</p>}</Panel>;
  const resultPanel = round && <div key="result" ref={resultRef} className="wp-capture-anchor"><Panel title="采集结果" actions={<Badge tone={round.state==="complete" ? "ok" : round.state==="failed" ? "warn" : "info"}>{stateLabels[round.state]}</Badge>}>
    {active && <div className="wp-capture-live" role="status" aria-label="采集进度">{progress == null ? <i className="wp-capture-breath" aria-hidden="true"/> : <span className="wp-capture-meter" aria-hidden="true"><span style={{ transform: `scaleX(${progress})` }}/></span>}<strong>{captureCountText({state:round.state,plannedCount:round.plannedCount,receivedCount:received})}</strong><span className="muted">{round.state==="waitingStart" ? "现场启动后开始计数；需要停止时点“中止本轮”" : round.state==="draining" ? "PLC 已结束，正在等待在途图像" : "各相机收齐后等待 PLC 结束信号"}</span></div>}
    <div className="wp-capture-summary">
      <span className="wp-capture-chip"><small>轮次</small><strong className="mono">{round.roundId}</strong></span>
      <span className="wp-capture-chip"><small>数量核对</small><strong>{captureCountText({state:round.state,plannedCount:round.plannedCount,receivedCount:received})}</strong></span>
      {multi && round.cameras.map(c=><span key={c.cameraId} className={"wp-capture-chip" + (round.plannedCount != null && c.receivedCount !== round.plannedCount && !active ? " warn" : "")}><small>{nameOf(c.cameraId)}</small><strong>{c.receivedCount}{round.plannedCount != null ? ` / ${round.plannedCount}` : ""} 张</strong></span>)}
      {round.plcActualCount != null && <span className="wp-capture-chip"><small>PLC 实际触发</small><strong>{round.plcActualCount} 次</strong></span>}
      {spec?.main && <span className={"wp-capture-chip" + (spec.uniform ? "" : " warn")}><small>图像规格</small><strong>{spec.uniform ? spec.main : `尺寸不一致 · 多数为 ${spec.main}`}</strong></span>}
      <Badge tone={round.simulated ? "warn" : "ok"}>{round.simulated ? "离线 / 模拟（不代表现场硬件验收）" : "设备实拍"}</Badge>
    </div>
    {round.error && <Notice title="本轮异常，整圈重采" tone="warn">{round.error}</Notice>}
    {multi && <SelectionTrack className="wp-capture-cams" role="group" aria-label="查看相机" activeKey={shownCamera}>{round.cameras.map(c=><button key={c.cameraId} type="button" className={c.cameraId===shownCamera ? "active" : undefined} aria-pressed={c.cameraId===shownCamera} onClick={()=>setPreviewCamera(c.cameraId)}>{nameOf(c.cameraId)}</button>)}</SelectionTrack>}
    <div className="wp-capture-result">
    <GrayViewer image={shown?.image ?? null} loading={previewArrived && !shown && !previewError} error={previewError || (round.frames[previewK] && !previewArrived ? "该相机本次触发尚未到图" : "")} label={"拍照点 " + pad(shownK) + (multi ? " · " + nameOf(shownCamera) : "") + " · 完整拼接原图" + (shownK!==previewK ? " · 正在读取 " + pad(previewK) : "")}/>
    <div className="wp-capture-side">
    <div className="wp-capture-side-head"><strong>拍照点 · {round.frames.length}</strong><small>↑↓ 切换</small></div>
    {round.frames.length ? <div className="wp-capture-shots" aria-label="拍照点选择" onKeyDown={shotKey}>{round.frames.map((f,i)=>{
      const own=f.cameras.find(c=>c.cameraId===shownCamera), missing=round.cameras.length-f.cameras.length;
      const odd=!!spec?.differs(f), warn=odd || !own || own.triggerCounter == null || missing>0;
      return <button key={f.shotId} className={"wp-capture-shot" + (previewK===i ? " selected" : "")} aria-pressed={previewK===i} onClick={()=>setPreviewK(i)}>
        <strong>{pad(i)}</strong><i className={"wp-capture-dot" + (warn ? " warn" : "")}/><span>{own ? `帧 ${own.frameCounter} · 触发 ${own.triggerCounter ?? "未提供"}` : "该相机未到图"}</span>{odd && <small>{frameSpec(f)}</small>}{missing>0 && <small>缺 {missing} 台相机</small>}
      </button>;
    })}</div> : <p className="muted">尚未收到图像</p>}
    {round.state==="complete" && <div className="wp-capture-actions">{(adopted || savedSample) && <p className="wp-capture-done" role="status"><CheckCircle2 size={15} aria-hidden="true"/>{adopted ? `已采用为示教图像 · ${data.workspace.frames.length} 个拍照点` : "已加入验证样本库"}</p>}{!validation && data.workspace.frames.length>0 && <><label className="check"><input type="checkbox" checked={reuse} disabled={locked} onChange={e=>setReuse(e.target.checked)}/>带入旧中线与参数作为草稿</label>{reuse && <label className="check"><input type="checkbox" checked={confirmed} disabled={locked} onChange={e=>setConfirmed(e.target.checked)}/>已确认相机、轨迹和触发顺序保持一致，拍照点对应正确</label>}</>}
      {validation && <label className="check"><input type="checkbox" checked={confirmed} disabled={locked} onChange={e=>setConfirmed(e.target.checked)}/>已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致</label>}
      {validation && <label className="field"><span>样本人工期望</span><select className="input" aria-label="样本人工期望" value={sampleExpected} disabled={locked} onChange={e=>setSampleExpected(e.target.value as Verdict)}>{expectedChoices.map(([v,l])=><option key={v} value={v}>{l}</option>)}</select></label>}
      {sampleSourceInvalid && <Notice title="需要独立于示教的采集轮次" tone="warn">{data.workspace.captureId ? "当前轮次已用于示教，请重新采集一圈作为验证样本。" : "请先采用示教采集轮次，再采集独立验证样本。"}</Notice>}
      <button className={"btn" + (adopted || savedSample ? "" : " primary")} disabled={locked || productionBusy || sampleSourceInvalid || savedSample || ((validation || reuse) && !confirmed)} title={savedSample ? "本轮已加入验证样本库" : undefined} onClick={() => void act(() => validation ? workspaceApi.captureSample(doc.id,data.workspace.revision,round.roundId,sampleExpected,confirmed) : workspaceApi.adoptCapture(doc.id,data.workspace.revision,round.roundId,reuse,confirmed),validation ? "独立验证样本已加入样本库" : "已采用本轮图像，请逐点选择检测图并示教")}>{validation ? "保存为独立验证样本" : adopted ? "重新采用本轮图像" : "采用本轮图像"}</button>
      <Link className={"btn" + (adopted || savedSample ? " primary" : "")} to={validation ? "/recipe/validation" : "/recipe/teach"}>{validation ? "进入验证" : "进入单帧示教"}<ArrowRight size={15} aria-hidden="true"/></Link>
    </div>}
    </div></div>
  </Panel></div>;
  const prepBar = <section key="prep-bar" className="panel wp-panel wp-capture-prep-bar" aria-label="设备准备">
    <strong>{validation ? "实拍独立验证样本" : "设备准备"}</strong>
    <span className={"wp-capture-chip" + (notReady.length || !chosen.length ? " warn" : "")}><small>相机</small><strong>{(chosen.length ? chosen.map(nameOf).join("、") : "未选择") + " · " + (chosen.length && !notReady.length ? "就绪" : "未就绪")}</strong></span>
    <span className={"wp-capture-chip" + (plcConnected ? "" : " warn")}><small>PLC</small><strong>{plc}</strong></span>
    <span className="wp-capture-chip"><small>在途图像等待</small><strong>{drain} ms</strong></span>
    <div className="wp-actions"><button className="btn" onClick={()=>setPrepOpenFor(round?.roundId ?? null)}>展开设备准备</button><button className={"btn" + (validation && !savedSample ? " primary" : "")} disabled={startDisabled} title={startReason || undefined} onClick={() => void start()}>{validation ? "采集新一圈样本" : "整圈重采"}</button></div>
  </section>;
  return <div className="wp-page"><WorkspaceBar/><Steps/><div className="wp-columns"><div className="wp-stack">{collapsed ? <>{prepBar}{resultPanel}{historyPanel}</> : <>{prepPanel}{resultPanel}{historyPanel}</>}</div><section className="wp-capture-rules" aria-label="采集规则"><h3>采集规则</h3><div><strong>数量相同不代表对应相同</strong><p>每轮独立保存。任一相机缺帧、重复、超量、断连或无法确定归属时整圈重采；不会把下一张图补到缺失位置。</p></div><div><strong>结束条件</strong><p>各相机都收齐计划数量后仍等待 PLC 结束通知，再等待在途图像；超时会写明是哪台相机缺图。PLC 实际计划和完成计数均由后端按相机核对。</p></div></section></div></div>;
}
