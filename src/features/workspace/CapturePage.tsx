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
  const views = frame.cameras.flatMap(c=>c.views);
  const sizes = [...new Set(views.map(v=>`${v.width}×${v.height}`))].join(" / ");
  return views.length > 1 ? `每点 ${views.length} 图 · ${sizes}` : sizes;
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
  const [excluded,setExcluded] = useState<string[]>([]), [readyIds,setReadyIds] = useState<string[]>([]);
  const [drain,setDrain] = useState(1500);
  const [round,setRound] = useState<CaptureRound | null>(null), [working,setWorking] = useState(false);
  const [history,setHistory] = useState<Awaited<ReturnType<typeof workspaceApi.captureList>>>([]);
  const [roundId,setRoundId] = useState<string | undefined>();
  const [previewK,setPreviewK] = useState(0), [previewCamera,setPreviewCamera] = useState(""), [preview,setPreview] = useState<{k: number; camera: string; image: GrayImage} | null>(null), [previewError,setPreviewError] = useState("");
  const [plc,setPlc] = useState("正在读取 PLC 状态"), [plcConnected,setPlcConnected] = useState(false);
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
        if(alive) { setHistory(rounds); setRound(r?.recipeId === doc?.id ? r : null); setPlc(status.message || status.state); setPlcConnected(status.state === "connected"); setReadyIds(devices.filter(d=>d.ready).map(d=>d.id)); }
      } catch(e) { if(alive) setError(errorText(e)); } finally { pending=false; }
    };
    void refresh(); const timer=setInterval(() => void refresh(),700);
    return () => {alive=false;clearInterval(timer);};
  },[doc?.id,setError,roundId]);
  useEffect(() => { setConfirmed(false); },[round?.roundId,validation]);
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
  const [prepOpenFor,setPrepOpenFor] = useState<string | null>(null);
  const resultRef = useRef<HTMLDivElement>(null), wasActive = useRef(false);
  useEffect(() => {
    if(wasActive.current && !active && round) resultRef.current?.scrollIntoView?.({block:"start",behavior:"smooth"});
    wasActive.current=active;
  },[active,round?.roundId]);
  if(!doc || !data) return <WorkspaceEmpty/>;
  const locked=busy || working || dirty || frameDirty || !!data.workspace.pending;
  const productionBusy=isProductionBusy(snapshot);
  const sampleSourceInvalid=validation && (!data.workspace.captureId || data.workspace.captureId===round?.roundId);
  const captureState = {captureReturn:{recipeId:doc.id,search:location.search,cameraIds:chosen,excludedCameraIds:excluded,drain,roundId,previewK}};
  const navigationState: CaptureNavigationState = {...captureState, returnTo:{pathname:location.pathname,search:location.search,label:validation ? "独立样本采集" : "整圈采集",state:captureState}};
  const nameOf = (id: string) => cameras.find(c=>c.id===id)?.name ?? id;
  const notReady = chosen.filter(id=>!readyIds.includes(id));
  const startBlocked = !plcConnected ? "PLC 未连接：整圈采集依赖 PLC 的开始 / 结束信号与触发计数。" : !chosen.length ? "请至少选择一台采集相机。" : notReady.length ? `采集相机未就绪：${notReady.map(nameOf).join("、")}。` : productionBusy ? "当前工件结束后才能占用设备采集。" : "";
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
  const shotKey=(e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.key==="ArrowDown" || e.key==="ArrowRight" ? 1 : e.key==="ArrowUp" || e.key==="ArrowLeft" ? -1 : 0;
    if(!step || !round?.frames.length) return;
    e.preventDefault();
    const next=Math.min(round.frames.length-1,Math.max(0,previewK+step));
    setPreviewK(next);
    const item=e.currentTarget.children[next] as HTMLElement | undefined;
    item?.focus({preventScroll:true}); item?.scrollIntoView({block:"nearest"});
  };
  const pad=(k: number) => String(k+1).padStart(2,"0");
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
  const prepPanel = <Panel key="prep" title={validation ? "实拍独立验证样本" : "设备准备与整圈采集"} detail="选择参与采集的相机（默认全选已配置相机）；每次 PLC 触发各相机各出一张图，共同组成一个拍照点" actions={finished && <button className="btn small" onClick={()=>setPrepOpenFor(null)}>收起</button>}>
    <div className="wp-capture-cameras" role="group" aria-label="采集相机">{cameras.map(c=>{
      const on=chosen.includes(c.id), imaging=imagingOf(c.id), invalid=on ? imagingInvalid(imaging) : "", ready=readyIds.includes(c.id);
      return <div key={c.id} className={"wp-capture-camera" + (on ? "" : " off")}>
        <div className="wp-capture-camera-head"><label className="check"><input type="checkbox" checked={on} disabled={locked || active} onChange={e=>toggleCamera(c.id,e.target.checked)}/>{c.name} · {c.serial || c.source}</label><Badge tone={ready ? "ok" : "warn"}>{ready ? "就绪" : "未就绪"}</Badge></div>
        {on && <div className="wp-form-grid"><NumberField label={`${c.name} 曝光时间（µs）`} value={imaging.exposureUs} min={1} max={1_000_000} onChange={v=>editImaging(c.id,"exposureUs",v)} disabled={locked || active}/><NumberField label={`${c.name} 增益（dB）`} value={imaging.gainDb} min={0} step={0.5} onChange={v=>editImaging(c.id,"gainDb",v)} disabled={locked || active}/></div>}
        {invalid ? <p className="c-ng" role="alert">{c.name}：{invalid}</p> : on && applyErrors[c.id] && <Notice title={`${c.name} 已写入候选配方，但没能下发到相机`} tone="warn">{applyErrors[c.id]}</Notice>}
      </div>;
    })}{!cameras.length && <p className="muted">尚未配置相机</p>}</div>
    <div className="wp-actions"><button className="btn" disabled={locked || active || !changedImaging.length || changedImaging.some(id=>!!imagingInvalid(imagingOf(id)))} onClick={applyImaging}>应用</button><span className="muted">按相机分别写入候选配方并立即下发</span></div>
    <KV label="PLC">{plc}</KV>
    <div className="wp-form-grid"><NumberField label="在途图像等待时间" value={drain} min={200} max={30000} unit="ms" onChange={setDrain} disabled={locked || active}/></div>
    <div className="wp-actions"><Link className="btn" to="/camera" state={navigationState}>配置设备</Link><Link className="btn" to="/plc" state={navigationState}>PLC 通讯</Link><Link className="btn" to="/camera/calibration" state={navigationState}>飞拍标定</Link></div>
    <Notice title="现场启动机械臂">先确认合格涂胶工件、轨迹和触发顺序。GS 开始接收并就绪后，由现场启动机械臂；触发数量由 PLC 自动提供，每台相机都必须收到同样数量的图，拍照点数量依据实际收图生成。</Notice>
    <div className="wp-actions"><button className="btn primary" disabled={startDisabled} onClick={() => void start()}>开始接收 / 整圈重采</button><button className="btn danger" disabled={locked || !active} onClick={() => void stop()}>中止本轮</button></div>
    {startBlocked && !active && <Notice title="暂不能开始接收" tone="warn"><span>{startBlocked}</span>{!plcConnected ? <Link className="btn small" to="/plc" state={navigationState}>前往 PLC 通讯</Link> : notReady.length ? <Link className="btn small" to="/camera" state={navigationState}>前往相机与采集</Link> : null}</Notice>}
    {offlineSampleSet && <div className="wp-stack wp-offline-feed"><strong>离线样本集送一圈</strong><p className="muted">① 先点“开始接收”；② 选择样本件；③ 点“离线产线送一圈”，虚拟 PLC 发开始 / 结束边沿，图像来自所选样本件。</p><div className="wp-actions"><SamplePartSelect sampleSet={offlineSampleSet} nextPart={site?.nextPart ?? null} value={offlinePart} onChange={setOfflinePart} disabled={offlineSending || !offlineReady} label="离线样本件"/><button className="btn" disabled={offlineSending || !offlineReady} onClick={() => void sendOffline()}>离线产线送一圈</button></div>{!offlineReady && <p className="muted hint">{site?.running ? "离线产线正在连续运行，停止后再送一圈。" : "开始接收并等待现场启动后，才能送一圈。"}</p>}</div>}
  </Panel>;
  const historyPanel = <Panel key="history" title="采集历史" detail="保留异常轮次供追溯，重新采集不会混用旧图"><select className="input" aria-label="采集历史" value={roundId ?? ""} disabled={active || working} onChange={e=>{setRoundId(e.target.value || undefined);setPreviewK(0);}}><option value="">最近轮次</option>{history.map(r=><option key={r.roundId} value={r.roundId}>{new Date(r.createdAt).toLocaleString("zh-CN")} · {stateLabels[r.state]} · {r.cameraIds.length} 台相机 · {captureCountText(r)}</option>)}</select></Panel>;
  const resultPanel = round && <div key="result" ref={resultRef} className="wp-capture-anchor"><Panel title="采集结果" actions={<Badge tone={round.state==="complete" ? "ok" : round.state==="failed" ? "warn" : "info"}>{stateLabels[round.state]}</Badge>}>
    <div className="wp-capture-summary">
      <span className="wp-capture-chip"><small>轮次</small><strong className="mono">{round.roundId}</strong></span>
      <span className="wp-capture-chip"><small>数量核对</small><strong>{captureCountText({state:round.state,plannedCount:round.plannedCount,receivedCount:completeShots(round)})}</strong></span>
      {multi && round.cameras.map(c=><span key={c.cameraId} className={"wp-capture-chip" + (round.plannedCount != null && c.receivedCount !== round.plannedCount && !active ? " warn" : "")}><small>{nameOf(c.cameraId)}</small><strong>{c.receivedCount}{round.plannedCount != null ? ` / ${round.plannedCount}` : ""} 张</strong></span>)}
      {round.plcActualCount != null && <span className="wp-capture-chip"><small>PLC 实际触发</small><strong>{round.plcActualCount} 次</strong></span>}
      {spec?.main && <span className={"wp-capture-chip" + (spec.uniform ? "" : " warn")}><small>图像规格</small><strong>{spec.uniform ? spec.main : `尺寸不一致 · 多数为 ${spec.main}`}</strong></span>}
      <Badge tone={round.simulated ? "warn" : "ok"}>{round.simulated ? "离线 / 模拟（不代表现场硬件验收）" : "设备实拍"}</Badge>
    </div>
    {round.error && <Notice title="本轮异常，整圈重采" tone="warn">{round.error}</Notice>}
    {multi && <div className="wp-actions" role="group" aria-label="查看相机">{round.cameras.map(c=><button key={c.cameraId} className={"btn small" + (c.cameraId===shownCamera ? " primary" : "")} aria-pressed={c.cameraId===shownCamera} onClick={()=>setPreviewCamera(c.cameraId)}>{nameOf(c.cameraId)}</button>)}</div>}
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
    {round.state==="complete" && <div className="wp-capture-actions">{!validation && data.workspace.frames.length>0 && <><label className="check"><input type="checkbox" checked={reuse} disabled={locked} onChange={e=>setReuse(e.target.checked)}/>带入旧中线与参数作为草稿</label>{reuse && <label className="check"><input type="checkbox" checked={confirmed} disabled={locked} onChange={e=>setConfirmed(e.target.checked)}/>已确认相机、轨迹和触发顺序保持一致，拍照点对应正确</label>}</>}
      <label className="check" hidden={!validation}><input type="checkbox" checked={confirmed} disabled={locked} onChange={e=>setConfirmed(e.target.checked)}/>已确认本轮样本与示教轨迹、触发顺序及拍摄条件一致</label>
      {validation && <label className="field"><span>样本人工期望</span><select className="input" aria-label="样本人工期望" value={sampleExpected} disabled={locked} onChange={e=>setSampleExpected(e.target.value as Verdict)}>{expectedChoices.map(([v,l])=><option key={v} value={v}>{l}</option>)}</select></label>}
      {sampleSourceInvalid && <Notice title="需要独立于示教的采集轮次" tone="warn">{data.workspace.captureId ? "当前轮次已用于示教，请重新采集一圈作为验证样本。" : "请先采用示教采集轮次，再采集独立验证样本。"}</Notice>}
      <button className="btn primary" disabled={locked || productionBusy || sampleSourceInvalid || ((validation || reuse) && !confirmed)} onClick={() => void act(() => validation ? workspaceApi.captureSample(doc.id,data.workspace.revision,round.roundId,sampleExpected,confirmed) : workspaceApi.adoptCapture(doc.id,data.workspace.revision,round.roundId,reuse,confirmed),validation ? "独立验证样本已加入样本库" : "已采用本轮图像，请逐点选择检测图并示教")}>{validation ? "保存为独立验证样本" : "采用本轮图像"}</button>
      <Link className="btn" to={validation ? "/recipe/validation" : "/recipe/teach"}>{validation ? "进入验证" : "进入单帧示教"}</Link>
    </div>}
    </div></div>
  </Panel></div>;
  const prepBar = <section key="prep-bar" className="panel wp-panel wp-capture-prep-bar" aria-label="设备准备">
    <strong>{validation ? "实拍独立验证样本" : "设备准备"}</strong>
    <span className={"wp-capture-chip" + (notReady.length || !chosen.length ? " warn" : "")}><small>相机</small><strong>{(chosen.length ? chosen.map(nameOf).join("、") : "未选择") + " · " + (chosen.length && !notReady.length ? "就绪" : "未就绪")}</strong></span>
    <span className={"wp-capture-chip" + (plcConnected ? "" : " warn")}><small>PLC</small><strong>{plc}</strong></span>
    <span className="wp-capture-chip"><small>在途图像等待</small><strong>{drain} ms</strong></span>
    <div className="wp-actions"><button className="btn" onClick={()=>setPrepOpenFor(round?.roundId ?? null)}>展开设备准备</button><button className="btn" disabled={startDisabled} title={startBlocked || undefined} onClick={() => void start()}>开始接收 / 整圈重采</button></div>
  </section>;
  return <div className="wp-page"><WorkspaceBar/><Steps/><div className="wp-columns"><div className="wp-stack">{collapsed ? <>{prepBar}{resultPanel}{historyPanel}</> : <>{prepPanel}{historyPanel}{resultPanel}</>}</div><div className="wp-stack"><Notice title="数量相同不代表对应相同">每轮独立保存。任一相机缺帧、重复、超量、断连或无法确定归属时整圈重采；不会把下一张图补到缺失位置。</Notice><Notice title="结束条件">各相机都收齐计划数量后仍等待 PLC 结束通知，再等待在途图像；超时会写明是哪台相机缺图。PLC 实际计划和完成计数均由后端按相机核对。</Notice><Link className="btn" to="/recipe/progress">返回进度总览</Link></div></div></div>;
}
