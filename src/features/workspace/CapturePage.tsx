import { errorText } from "../../lib/errors";
import { imageFailure } from "../../lib/pendingBackend";
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
import { AlertTriangle, ArrowRight, Cable, Camera, CheckCircle2, ChevronLeft, ChevronRight, Crosshair, Play } from "lucide-react";
import { scrollBehavior } from "./motion";
import { Button, Checkbox, IconButton, SegmentedControl, Select, Surface } from "qing-steel";
import { Badge, Notice, NumberField, Panel, WorkspaceBar, WorkspaceEmpty, GrayViewer } from "./components";
import "./capture.css";
import type { CaptureRound, GrayImage } from "./types";
import { frameArrived, lossAlarmText, readRoundFrame, roundView, type AnyRound } from "./captureRound";
import { DEFAULT_IMAGING, type CameraImaging, type Verdict } from "../cycle/types";
import { expectedChoices } from "./progress";
import type { CaptureNavigationState } from "./captureNavigation";
import { productionBusy as isProductionBusy } from "../cycle/phase";
import LegacyCapturePage from "./LegacyCapturePage";

const stateLabels: Record<CaptureRound["state"],string> = { waitingStart:"等待现场启动", receiving:"接收中", draining:"PLC 已结束，等待在途图像", complete:"采集完成", failed:"采集异常" };
const MAX_LISTED_GROUPS = 12;
const imagingInvalid = (imaging: CameraImaging) => !(imaging.exposureUs>=1 && imaging.exposureUs<=1_000_000) ? "曝光时间需在 1–1000000 µs 之间" : !(Number.isFinite(imaging.gainDb) && imaging.gainDb>=0) ? "增益需为不小于 0 的有限数" : "";

/** 整圈采集（路由 /recipe/capture）：旧拍照点配方（WorkspaceView 没有 plan）仍走 LegacyCapturePage——按拍照点核对数量、采用本轮图像；段配方按帧组展示。 */
export default function CapturePage() {
  const { data } = useWorkspace();
  return data && !data.plan ? <LegacyCapturePage/> : <BurstCapturePage/>;
}

/** 段配方的整圈采集：按帧号配组、缺帧与丢帧只告警，完成后用于走廊示教或加入多轮标定。 */
function BurstCapturePage() {
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
  const [round,setRound] = useState<AnyRound | null>(null), [working,setWorking] = useState(false);
  const [history,setHistory] = useState<Awaited<ReturnType<typeof workspaceApi.captureList>>>([]);
  const [roundId,setRoundId] = useState<string | undefined>();
  const [previewK,setPreviewK] = useState(0), [previewCamera,setPreviewCamera] = useState(""), [preview,setPreview] = useState<{k: number; camera: string; image: GrayImage} | null>(null), [previewFailure,setPreviewFailure] = useState({error:"",pending:""});
  const [plcState,setPlcState] = useState<LinkState | null>(null);
  const [sampleExpected,setSampleExpected] = useState<Verdict>("OK");
  const [confirmed,setConfirmed] = useState(false);
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
  const view = round ? roundView(round) : null;
  const shownCamera = view?.cameras.some(c=>c.cameraId===previewCamera) ? previewCamera : view?.cameras[0]?.cameraId ?? "";
  const previewArrived = !!view && frameArrived(view,shownCamera,previewK+1);
  useEffect(() => {
    let alive=true; setPreviewFailure({error:"",pending:""});
    if(round && previewArrived) void readRoundFrame(round,shownCamera,previewK+1).then(async image=>{
      const decoded=new Image(); decoded.src=image.url; await Promise.race([decoded.decode?.().catch(()=>{}),new Promise(r=>setTimeout(r,150))]);
      if(alive)setPreview({k:previewK,camera:shownCamera,image});
    }).catch(e=>{if(alive){setPreview(null);setPreviewFailure(imageFailure(e));}});
    return () => {alive=false;};
  },[round?.roundId,previewArrived,previewK,shownCamera]);
  const active=!!round && ["waitingStart","receiving","draining"].includes(round.state);
  const groupCount=view?.groupCount ?? 0;
  const [prepOpenFor,setPrepOpenFor] = useState<string | null>(null), [imagingOpen,setImagingOpen] = useState(false);
  const resultRef = useRef<HTMLDivElement>(null), wasActive = useRef(false);
  useEffect(() => {
    if(wasActive.current && !active && round) resultRef.current?.scrollIntoView?.({block:"start",behavior:scrollBehavior()});
    wasActive.current=active;
  },[active,round?.roundId]);
  if(!doc || !data) return <WorkspaceEmpty/>;
  const locked=busy || working || dirty || frameDirty || !!data.workspace.pending;
  const productionBusy=isProductionBusy(snapshot);
  // 段配方：本轮已用于走廊示教或加入标定就不能再当独立验证样本；旧配方沿用“示教采集轮次”的判断
  const plan = data.plan ?? null;
  const calibrationEntry = round ? plan?.calibration?.rounds.find(r => r.roundId===round.roundId) : undefined;
  const teachedCameras = round && plan ? plan.cameraTeaching.filter(t => t.frame?.source.includes(round.roundId)).map(t => t.camera) : [];
  const roundUsed = !!calibrationEntry || teachedCameras.length>0;
  const sampleSourceInvalid=validation && (plan ? roundUsed : (!data.workspace.captureId || data.workspace.captureId===round?.roundId));
  const bankedAsSample=!validation && !!round && data.workspace.sampleBank.some(sample => sample.captureId===round.roundId);
  const recaptureNext=(validation && (round?.state!=="complete" || sampleSourceInvalid)) || bankedAsSample;
  const captureState = {captureReturn:{recipeId:doc.id,search:location.search,cameraIds:chosen,excludedCameraIds:excluded,drain,roundId,previewK}};
  const navigationState: CaptureNavigationState = {...captureState, returnTo:{pathname:location.pathname,search:location.search,label:validation ? "独立样本采集" : "整圈采集",state:captureState}};
  const nameOf = (id: string) => cameras.find(c=>c.id===id)?.name ?? id;
  const readyIds = rig.filter(d=>d.ready).map(d=>d.id);
  const notReady = chosen.filter(id=>!readyIds.includes(id));
  const plcConnected = plcState === "connected", plc = plcState ? linkStateLabels[plcState] : "读取中";
  const startBlocked = !plcConnected ? `PLC ${plcState === "connecting" ? "连接中" : "未连接"}：整圈采集依赖 PLC 的开始 / 结束信号。` : !chosen.length ? "请至少选择一台采集相机。" : notReady.length ? `采集相机未就绪：${notReady.map(nameOf).join("、")}。` : productionBusy ? "当前工件结束后才能占用设备采集。" : "";
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
  const savedSample = validation && !!round && data.workspace.sampleBank.some(sample => sample.id === round.roundId);
  const gotoGroup=(k: number) => setPreviewK(Math.min(Math.max(0,groupCount-1),Math.max(0,Number.isFinite(k) ? Math.trunc(k) : 0)));
  const groupKey=(e: KeyboardEvent<HTMLDivElement>) => {
    if((e.target as HTMLElement).tagName==="INPUT") return;
    const step = e.key==="ArrowDown" || e.key==="ArrowRight" ? 1 : e.key==="ArrowUp" || e.key==="ArrowLeft" ? -1 : 0;
    if(!step || !groupCount) return;
    e.preventDefault(); gotoGroup(previewK+step);
  };
  const shown=preview?.camera===shownCamera ? preview : null, shownK=shown?.k ?? previewK;
  const multi=(view?.cameras.length ?? 0) > 1;
  const cameraName=(index: number) => nameOf(view?.cameras[index]?.cameraId ?? "");
  const addToCalibration=(standard: boolean) => { if(round) void act(() => workspaceApi.calibrationAddRound(doc.id,data.workspace.revision,round.roundId,standard),standard ? "已加入标定（标准件轮次）" : "已加入多轮标定"); };
  const removeFromCalibration=() => { if(round) void act(() => workspaceApi.calibrationRemoveRound(doc.id,data.workspace.revision,round.roundId),"已移出多轮标定"); };
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
  const prepPanel = <Panel key="prep" title={validation ? "实拍独立验证样本" : "设备准备与整圈采集"} detail="PLC 使能为高时连续发脉冲，各相机同时触发；同一触发序号的各相机帧配成一个帧组" actions={<>{!active && <Badge tone={pendingChecks ? "warn" : "ok"}>{pendingChecks ? `${pendingChecks} 项未就绪` : "可以开始"}</Badge>}{finished && <Button size="sm" onClick={()=>setPrepOpenFor(null)}>收起</Button>}</>}>
    <ul className="wp-capture-checks" aria-label="开始前检查">
      <li className={cameraProblem ? "bad" : ""}><span><Camera size={14} aria-hidden="true"/>相机</span><strong>{cameraProblem || `${chosen.length} 台就绪`}</strong><Link to="/camera" state={navigationState}>配置设备<ArrowRight size={12} aria-hidden="true"/></Link></li>
      <li className={plcConnected ? "" : "bad"}><span><Cable size={14} aria-hidden="true"/>PLC</span><strong>{plc}</strong><Link to="/plc" state={navigationState}>{plcConnected ? "PLC 通讯" : "前往 PLC 通讯"}<ArrowRight size={12} aria-hidden="true"/></Link></li>
      <li className={tooFast ? "bad" : ""} title={maxSpeed == null ? "需要标定当量、相机帧率、曝光与飞拍最小拍照点间距后核算" : tooFast ? `当前速度 ${flyPrefs.speed} mm/s 超出允许值` : undefined}><span><Crosshair size={14} aria-hidden="true"/>飞拍</span><strong>{maxSpeed == null ? "未核算" : `${tooFast ? "超速 · " : ""}最高 ${maxSpeed} mm/s`}</strong><Link to="/camera/calibration" state={navigationState}>飞拍标定<ArrowRight size={12} aria-hidden="true"/></Link></li>
      <li className={drainInvalid ? "bad" : ""}><NumberField label="在途图像等待时间" value={drain} min={200} max={30000} unit="ms" onChange={setDrain} disabled={locked || active}/></li>
    </ul>
    <div className="wp-capture-start">
      <Button variant="filled" leadingIcon={<Play size={15} aria-hidden="true"/>} disabled={startDisabled} onClick={() => void start()}>开始接收</Button>
      {active ? <Button danger disabled={locked} onClick={() => void stop()}>中止本轮</Button> : startReason && <p className="wp-capture-reason"><AlertTriangle size={14} aria-hidden="true"/><span>{startReason}</span></p>}
    </div>
    {offlineSampleSet && <div className="wp-stack wp-offline-feed"><strong>离线样本集送一圈</strong><p className="muted">① 先点“开始接收”；② 选择样本件；③ 点“离线产线送一圈”，虚拟 PLC 发开始 / 结束边沿，图像来自所选样本件。</p><div className="wp-actions"><SamplePartSelect sampleSet={offlineSampleSet} nextPart={site?.nextPart ?? null} value={offlinePart} onChange={setOfflinePart} disabled={offlineSending || !offlineReady} label="离线样本件"/><Button disabled={offlineSending || !offlineReady} onClick={() => void sendOffline()}>离线产线送一圈</Button></div>{!offlineReady && <p className="muted hint">{site?.running ? "离线产线正在连续运行，停止后再送一圈。" : "开始接收并等待现场启动后，才能送一圈。"}</p>}</div>}
    <details className="wp-capture-more">
      <summary>现场操作步骤</summary>
      <ol><li>确认合格涂胶工件、轨迹和触发顺序。</li><li>点“开始接收”，等待 GS 就绪。</li><li>现场启动机械臂；帧数由实际触发决定，不预先给定。PLC 写来的预期总数只作参考，不核对。</li></ol>
    </details>
    <details className="wp-capture-more" open={imagingOpen || imagingProblem} onToggle={e=>setImagingOpen(e.currentTarget.open)}>
      <summary>采集相机与成像参数<small>{cameras.length ? `已选 ${chosen.length} / ${cameras.length} 台` : "尚未配置相机"}</small></summary>
      <div className="wp-capture-cameras" role="group" aria-label="采集相机">{cameras.map(c=>{
        const on=chosen.includes(c.id), imaging=imagingOf(c.id), invalid=on ? imagingInvalid(imaging) : "", ready=readyIds.includes(c.id);
        return <div key={c.id} className={"wp-capture-camera" + (on ? "" : " off")}>
          <div className="wp-capture-camera-head"><Checkbox label={`${c.name} · ${c.serial || c.source}`} checked={on} disabled={locked || active} onCheckedChange={checked=>toggleCamera(c.id,checked)}/><Badge tone={ready ? "ok" : "warn"}>{ready ? "就绪" : "未就绪"}</Badge></div>
          {on && <div className="wp-form-grid"><NumberField label={`${c.name} 曝光时间（µs）`} value={imaging.exposureUs} min={1} max={1_000_000} onChange={v=>editImaging(c.id,"exposureUs",v)} disabled={locked || active}/><NumberField label={`${c.name} 增益（dB）`} value={imaging.gainDb} min={0} step={0.5} onChange={v=>editImaging(c.id,"gainDb",v)} disabled={locked || active}/></div>}
          {invalid ? <p className="c-ng" role="alert">{c.name}：{invalid}</p> : on && applyErrors[c.id] && <Notice title={`${c.name} 已写入候选配方，但没能下发到相机`} tone="warn">{applyErrors[c.id]}</Notice>}
        </div>;
      })}{!cameras.length && <p className="muted">配置相机后，可在此选择参与采集的相机并调整曝光与增益。</p>}</div>
      {cameras.length > 0 && <div className="wp-actions"><Button disabled={locked || active || !changedImaging.length || changedImaging.some(id=>!!imagingInvalid(imagingOf(id)))} onClick={applyImaging}>应用</Button><span className="muted">按相机分别写入候选配方并立即下发</span></div>}
    </details>
  </Panel>;
  const historyPanel = <Panel key="history" title="采集历史" detail="保留异常轮次供追溯，重新采集不会混用旧图">{history.length ? <Select aria-label="采集历史" value={roundId ?? ""} disabled={active || working} options={[{value:"",label:"最近轮次"},...history.map(r=>({value:r.roundId,label:`${new Date(r.createdAt).toLocaleString("zh-CN")} · ${stateLabels[r.state]} · ${r.cameraIds.length} 台相机 · 已接收 ${r.receivedCount} 帧组`}))]} onValueChange={value=>{setRoundId(value || undefined);setPreviewK(0);}}/> :<p className="wp-capture-empty">暂无采集轮次。完成第一圈采集后，可在此回看各轮结果。</p>}</Panel>;
  const lossTexts = view?.lossAlarms.map(alarm => lossAlarmText(alarm,cameraName)) ?? [];
  const incompleteShown = view?.incompleteGroups.slice(0,MAX_LISTED_GROUPS) ?? [];
  const resultPanel = round && view && <div key="result" ref={resultRef} className="wp-capture-anchor"><Panel title="采集结果" actions={<Badge tone={round.state==="complete" ? "ok" : round.state==="failed" ? "warn" : "info"}>{stateLabels[round.state]}</Badge>}>
    {active && <div className="wp-capture-live" role="status" aria-label="采集进度"><strong>{round.state==="waitingStart" ? "等待 PLC 开始信号" : `已收到 ${groupCount} 个帧组`}</strong><span className="muted">{round.state==="waitingStart" ? "现场启动后开始接收；需要停止时点“中止本轮”" : round.state==="draining" ? "PLC 已结束，正在等待在途图像" : "PLC 使能为高时连续接收；收到 PLC 结束信号后再等待在途图像"}</span></div>}
    <div className="wp-capture-summary">
      <span className="wp-capture-chip"><small>轮次</small><strong className="mono">{round.roundId}</strong></span>
      <span className="wp-capture-chip"><small>帧组</small><strong>{groupCount} 个</strong></span>
      {view.cameras.map(c=><span key={c.cameraId} className={"wp-capture-chip" + (!active && c.frames!==groupCount ? " warn" : "")}><small>{nameOf(c.cameraId)}</small><strong>{c.frames} 帧</strong></span>)}
      {view.incompleteGroups.length>0 && <span className="wp-capture-chip warn"><small>不完整帧组</small><strong>{view.incompleteGroups.length} 个</strong></span>}
      {view.plcEstimate != null && <span className="wp-capture-chip"><small>PLC 预期总数（仅供参考）</small><strong>{view.plcEstimate}</strong></span>}
      {view.deviceTriggers != null && <span className="wp-capture-chip"><small>PLC 累计触发（仅供参考）</small><strong>{view.deviceTriggers}</strong></span>}
      <Badge tone={round.simulated ? "warn" : "ok"}>{round.simulated ? "离线 / 模拟（不代表现场硬件验收）" : "设备实拍"}</Badge>
    </div>
    {round.error && <Notice title="本轮异常，需要重新采集" tone="warn">{round.error}</Notice>}
    {lossTexts.length>0 && <Notice title="疑似丢帧（只告警）" tone="warn"><ul>{lossTexts.map(text=><li key={text}>{text}</li>)}</ul></Notice>}
    {!active && view.incompleteGroups.length>0 && <p className="muted hint">缺帧只告警：不完整的帧组仍会用其余相机的图检测，并标“检测不完整”；不整轮重采。</p>}
    {multi && <SegmentedControl size="sm" label="查看相机" value={shownCamera} onValueChange={setPreviewCamera} options={view.cameras.map(c=>({ value: c.cameraId, label: nameOf(c.cameraId) }))}/>}
    <div className="wp-capture-result">
    <GrayViewer image={shown?.image ?? null} loading={previewArrived && !shown && !previewFailure.error && !previewFailure.pending} error={previewFailure.error || (groupCount>0 && !previewArrived ? "该相机本帧组缺帧或尚未到图" : "")} emptyTitle={previewFailure.pending ? "原图读取待后端实现" : undefined} emptyHint={previewFailure.pending || undefined} label={"帧组 " + (shownK+1) + (multi ? " · " + nameOf(shownCamera) : "") + " · 原图" + (shownK!==previewK ? " · 正在读取帧组 " + (previewK+1) : "")}/>
    <div className="wp-capture-side">
    <div className="wp-capture-side-head"><strong>帧组 · {groupCount}</strong><small>↑↓ 切换</small></div>
    {groupCount ? <div className="wp-capture-groups" role="group" aria-label="帧组选择" tabIndex={0} onKeyDown={groupKey}>
      <IconButton size="sm" aria-label="上一帧组" disabled={previewK<=0} onClick={()=>gotoGroup(previewK-1)}><ChevronLeft size={15}/></IconButton>
      <NumberField label="帧组序号" value={previewK+1} min={1} max={groupCount} onChange={value=>gotoGroup(value-1)}/>
      <IconButton size="sm" aria-label="下一帧组" disabled={previewK>=groupCount-1} onClick={()=>gotoGroup(previewK+1)}><ChevronRight size={15}/></IconButton>
    </div> : <p className="muted">尚未收到图像</p>}
    {view.incompleteGroups.length>0 && <div className="wp-capture-shots" role="group" aria-label="不完整帧组">
      {incompleteShown.map(seq=><button key={seq} className={"wp-capture-shot" + (previewK===seq-1 ? " selected" : "")} aria-pressed={previewK===seq-1} onClick={()=>setPreviewK(seq-1)}>
        <strong>{seq}</strong><i className="wp-capture-dot warn"/><span>缺 {view.missing.filter(m=>m.seq===seq).map(m=>cameraName(m.camera)).join("、")}</span>
      </button>)}
      {view.incompleteGroups.length>incompleteShown.length && <small className="muted">另有 {view.incompleteGroups.length-incompleteShown.length} 个不完整帧组</small>}
    </div>}
    {round.state==="complete" && <div className="wp-capture-actions">
      {validation ? <>
        {savedSample && <p className="wp-capture-done" role="status"><CheckCircle2 size={15} aria-hidden="true"/>已加入验证样本库</p>}
        <Checkbox label="已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致" checked={confirmed} disabled={locked} onCheckedChange={setConfirmed}/>
        <Select label="样本人工期望" value={sampleExpected} disabled={locked} options={expectedChoices.map(([value,label])=>({value,label}))} onValueChange={value=>setSampleExpected(value as Verdict)}/>
        {sampleSourceInvalid && <Notice title="需要独立于示教的采集轮次" tone="warn">{plan ? "本轮已用于走廊示教或加入标定，请重新采集一轮作为验证样本。" : data.workspace.captureId ? "当前轮次已用于示教，请重新采集一圈作为验证样本。" : "请先采用示教采集轮次，再采集独立验证样本。"}</Notice>}
        <Button variant={savedSample || sampleSourceInvalid ? "tonal" : "filled"} data-variant={savedSample || sampleSourceInvalid ? "tonal" : "filled"} disabled={locked || productionBusy || sampleSourceInvalid || savedSample || !confirmed} title={savedSample ? "本轮已加入验证样本库" : undefined} onClick={() => void act(() => workspaceApi.captureSample(doc.id,data.workspace.revision,round.roundId,sampleExpected,confirmed),"独立验证样本已加入样本库")}>保存为独立验证样本</Button>
        <Link className={"qs-btn" + (savedSample ? "" : " tonal")} data-variant={savedSample ? "filled" : "tonal"} to="/recipe/validation">进入验证<ArrowRight size={15} aria-hidden="true"/></Link>
      </> : plan ? <>
        {teachedCameras.length>0 && <p className="wp-capture-done" role="status"><CheckCircle2 size={15} aria-hidden="true"/>已用于 {teachedCameras.map(nameOf).join("、")} 的走廊示教</p>}
        {calibrationEntry && <p className="wp-capture-done" role="status"><CheckCircle2 size={15} aria-hidden="true"/>{calibrationEntry.standard ? "已加入标定（标准件轮次）" : "已加入多轮标定"}</p>}
        {bankedAsSample && <Notice title="需要独立于验证样本的采集轮次" tone="warn">本轮已作为独立验证样本加入样本库，不能再用于走廊示教或标定，请重新采集。</Notice>}
        {bankedAsSample ? <Button variant="tonal" data-variant="tonal" disabled>用于走廊示教</Button>
          : <Link className={"qs-btn" + (teachedCameras.length ? "" : " tonal")} data-variant={teachedCameras.length ? "filled" : "tonal"} to={`/recipe/teach?round=${encodeURIComponent(round.roundId)}`}>用于走廊示教<ArrowRight size={15} aria-hidden="true"/></Link>}
        {calibrationEntry
          ? <Button variant="tonal" data-variant="tonal" disabled={locked || productionBusy} onClick={removeFromCalibration}>移出标定</Button>
          : <><Button variant="tonal" data-variant="tonal" disabled={locked || productionBusy || bankedAsSample} onClick={() => addToCalibration(false)}>加入标定</Button>
            <Button variant="tonal" data-variant="tonal" disabled={locked || productionBusy || bankedAsSample} onClick={() => addToCalibration(true)}>加入标定（标准件）</Button></>}
        <Link className="wp-text-link" to="/recipe/calibration">前往多轮标定</Link>
      </> : null}
    </div>}
    </div></div>
  </Panel></div>;
  const prepBar = <Surface key="prep-bar" role="region" className="wp-panel wp-capture-prep-bar" aria-label="设备准备">
    <strong>{validation ? "实拍独立验证样本" : "设备准备"}</strong>
    <span className={"wp-capture-chip" + (notReady.length || !chosen.length ? " warn" : "")}><small>相机</small><strong>{(chosen.length ? chosen.map(nameOf).join("、") : "未选择") + " · " + (chosen.length && !notReady.length ? "就绪" : "未就绪")}</strong></span>
    <span className={"wp-capture-chip" + (plcConnected ? "" : " warn")}><small>PLC</small><strong>{plc}</strong></span>
    <span className="wp-capture-chip"><small>在途图像等待</small><strong>{drain} ms</strong></span>
    <div className="wp-actions"><Button onClick={()=>setPrepOpenFor(round?.roundId ?? null)}>展开设备准备</Button><Button variant={recaptureNext ? "filled" : "tonal"} data-variant={recaptureNext ? "filled" : "tonal"} disabled={startDisabled} title={startReason || undefined} onClick={() => void start()}>{validation ? "采集新一圈样本" : "整圈重采"}</Button></div>
  </Surface>;
  return <div className="wp-page"><WorkspaceBar /><div className="wp-columns"><div className="wp-stack">{collapsed ? <>{prepBar}{resultPanel}{historyPanel}</> : <>{prepPanel}{resultPanel}{historyPanel}</>}</div><section className="wp-capture-rules" aria-label="采集规则"><h3>采集规则</h3><div><strong>按帧号配组，不按到达顺序</strong><p>同一触发序号的各相机帧配成一个帧组。某台相机缺帧只告警：该组标“检测不完整”，其余相机照常检测，不整轮重采，也不会把下一张图补到缺失位置。</p></div><div><strong>结束条件</strong><p>收到 PLC 结束通知，等待在途图像到期，已收到的帧全部写盘后本轮完成。PLC 的预期总数与累计触发只作参考；某台相机帧数与累计触发或另一台相机相差超过 1 时提示疑似丢帧。</p></div></section></div></div>;
}
