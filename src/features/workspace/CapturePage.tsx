import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Link, useLocation } from "react-router-dom";
import { useCycle } from "../cycle/api";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { ArrowRight, CheckCircle2, ChevronLeft, ChevronRight } from "lucide-react";
import { Button, IconButton, SegmentedControl, Select } from "qing-steel";
import { Badge, Notice, NumberField, Panel, WorkspaceBar, WorkspaceEmpty, GrayViewer } from "./components";
import "./capture.css";
import type { CaptureRoundV2, CaptureState, GrayImage } from "./types";
import { frameArrived, lossAlarmText, readRoundFrame, roundView } from "./captureRound";
import type { CaptureNavigationState } from "./captureNavigation";
import { productionBusy as isProductionBusy } from "../cycle/phase";
import CaptureReviewPanel from "./CaptureReviewPanel";
import HistoryImportPanel from "./HistoryImportPanel";

const stateLabels: Record<CaptureState,string> = { waitingStart:"等待 PLC 接受", receiving:"接收中", draining:"PLC 已结束，等待在途图像", complete:"采集完成", failed:"采集异常" };
const MAX_LISTED_GROUPS = 12;

/** 段配方的采集轮次：从历史件导入，按帧号配组、缺帧与丢帧只告警，用于走廊示教或加入多轮标定。 */
export default function CapturePage() {
  const {data,doc,cameras,busy,dirty,frameDirty,act,setError} = useWorkspace();
  const {snapshot} = useCycle();
  const location = useLocation();
  const captureReturn = (location.state as CaptureNavigationState | null)?.captureReturn;
  const [round,setRound] = useState<CaptureRoundV2 | null>(null);
  const [history,setHistory] = useState<Awaited<ReturnType<typeof workspaceApi.captureList>>>([]);
  const [roundId,setRoundId] = useState<string | undefined>();
  const [previewK,setPreviewK] = useState(0), [previewCamera,setPreviewCamera] = useState(""), [preview,setPreview] = useState<{k: number; camera: string; image: GrayImage} | null>(null), [previewError,setPreviewError] = useState("");
  const current = useRef({id:doc?.id,alive:true}); current.current.id=doc?.id;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;};},[]);
  useEffect(() => {
    let alive = true;
    void Promise.all([workspaceApi.captureGet(roundId),doc ? workspaceApi.captureList(doc.id) : Promise.resolve([])])
      .then(([r,rounds]) => { if(alive) { setHistory(rounds); setRound(r?.recipeId === doc?.id ? r : null); } })
      .catch(e => { if(alive) setError(errorText(e)); });
    return () => {alive=false;};
  },[doc?.id,setError,roundId]);
  useEffect(() => {
    const restored = captureReturn?.recipeId === doc?.id ? captureReturn : undefined;
    setRoundId(restored?.roundId); setPreviewK(restored?.previewK ?? 0);
  },[doc?.id,captureReturn]);
  useEffect(() => { setPreview(null); },[round?.roundId]);
  const view = round ? roundView(round) : null;
  const shownCamera = view?.cameras.some(c=>c.cameraId===previewCamera) ? previewCamera : view?.cameras[0]?.cameraId ?? "";
  const previewArrived = !!view && frameArrived(view,shownCamera,previewK+1);
  useEffect(() => {
    let alive=true; setPreviewError("");
    if(round && previewArrived) void readRoundFrame(round,shownCamera,previewK+1).then(async image=>{
      const decoded=new Image(); decoded.src=image.url; await Promise.race([decoded.decode?.().catch(()=>{}),new Promise(r=>setTimeout(r,150))]);
      if(alive)setPreview({k:previewK,camera:shownCamera,image});
    }).catch(e=>{if(alive){setPreview(null);setPreviewError(errorText(e));}});
    return () => {alive=false;};
  },[round?.roundId,previewArrived,previewK,shownCamera]);
  const groupCount=view?.groupCount ?? 0;
  if(!doc || !data) return <WorkspaceEmpty/>;
  const locked=busy || dirty || frameDirty || !!data.workspace.pending;
  const productionBusy=isProductionBusy(snapshot);
  const plan = data.plan;
  const calibrationEntry = round ? plan.calibration?.rounds.find(r => r.roundId===round.roundId) : undefined;
  const teachedCameras = round ? plan.cameraTeaching.filter(t => t.segments.some(s => s.frame.source.includes(round.roundId))).map(t => t.camera) : [];
  const bankedAsSample=!!round && data.workspace.sampleBank.some(sample => sample.captureId===round.roundId);
  const nameOf = (id: string) => cameras.find(c=>c.id===id)?.name ?? id;
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
  const markedStandard=!!round?.origin?.standard;
  const addToCalibration=(standard: boolean) => { if(round) void act(() => workspaceApi.calibrationAddRound(doc.id,data.workspace.revision,round.roundId,standard),standard ? "已加入标定（标准件轮次）" : "已加入多轮标定"); };
  const removeFromCalibration=() => { if(round) void act(() => workspaceApi.calibrationRemoveRound(doc.id,data.workspace.revision,round.roundId),"已移出标定，请重新应用标定"); };
  const historyPanel = <Panel key="history" title="采集历史" detail="保留异常轮次供追溯">{history.length ? <Select aria-label="采集历史" value={roundId ?? ""} options={[{value:"",label:"最近轮次"},...history.map(r=>({value:r.roundId,label:`${r.roundId} · ${new Date(r.createdAt).toLocaleString("zh-CN")} · ${r.origin ? "历史件导入" : stateLabels[r.state]} · ${r.cameraIds.length} 台相机 · ${r.origin ? "" : "已接收 "}${r.receivedCount} 帧组`}))]} onValueChange={value=>{setRoundId(value || undefined);setPreviewK(0);}}/> :<p className="wp-capture-empty">暂无采集轮次。从历史选用导入后，可在此回看各轮结果。</p>}</Panel>;
  const lossTexts = view?.lossAlarms.map(alarm => lossAlarmText(alarm,cameraName)) ?? [];
  const incompleteShown = view?.incompleteGroups.slice(0,MAX_LISTED_GROUPS) ?? [];
  const resultPanel = round && view && <div key="result" className="wp-capture-anchor"><Panel title="采集结果" actions={<Badge tone={round.state==="complete" ? "ok" : round.state==="failed" ? "warn" : "info"}>{stateLabels[round.state]}</Badge>}>
    <div className="wp-capture-summary">
      <span className="wp-capture-chip"><small>轮次</small><strong className="mono">{round.roundId}</strong></span>
      <span className="wp-capture-chip"><small>帧组</small><strong>{groupCount} 个</strong></span>
      {view.cameras.map(c=><span key={c.cameraId} className={"wp-capture-chip" + (c.frames!==groupCount ? " warn" : "")}><small>{nameOf(c.cameraId)}</small><strong>{c.frames} 帧</strong></span>)}
      {view.incompleteGroups.length>0 && <span className="wp-capture-chip warn"><small>不完整帧组</small><strong>{view.incompleteGroups.length} 个</strong></span>}
      {view.plcEstimate != null && <span className="wp-capture-chip"><small>PLC 预期总数（仅供参考）</small><strong>{view.plcEstimate}</strong></span>}
      {round.origin && <span className="wp-capture-chip"><small>历史件</small><strong className="mono">{round.origin.cycleId}</strong></span>}
      <Badge tone={round.simulated ? "warn" : "ok"}>{round.origin ? (markedStandard ? "历史件导入 · 标准件" : "历史件导入") : round.simulated ? "离线 / 模拟（不代表现场硬件验收）" : "设备实拍"}</Badge>
    </div>
    {view.incompleteGroups.length>0 && <p className="muted hint">缺帧只告警：不完整的帧组仍会用其余相机的图检测，并标“检测不完整”。</p>}
    {multi && <SegmentedControl size="sm" label="查看相机" value={shownCamera} onValueChange={setPreviewCamera} options={view.cameras.map(c=>({ value: c.cameraId, label: nameOf(c.cameraId) }))}/>}
    <div className="wp-capture-result">
    <GrayViewer image={shown?.image ?? null} loading={previewArrived && !shown && !previewError} error={previewError || (groupCount>0 && !previewArrived ? "该相机本帧组缺帧" : "")} label={"帧组 " + (shownK+1) + (multi ? " · " + nameOf(shownCamera) : "") + " · 原图" + (shownK!==previewK ? " · 正在读取帧组 " + (previewK+1) : "")}/>
    <div className="wp-capture-side">
    <div className="wp-capture-side-head"><strong>帧组 · {groupCount}</strong><small>↑↓ 切换</small></div>
    {groupCount ? <div className="wp-capture-groups" role="group" aria-label="帧组选择" tabIndex={0} onKeyDown={groupKey}>
      <IconButton size="sm" aria-label="上一帧组" disabled={previewK<=0} onClick={()=>gotoGroup(previewK-1)}><ChevronLeft size={15}/></IconButton>
      <NumberField label="帧组序号" value={previewK+1} min={1} max={groupCount} onChange={value=>gotoGroup(value-1)}/>
      <IconButton size="sm" aria-label="下一帧组" disabled={previewK>=groupCount-1} onClick={()=>gotoGroup(previewK+1)}><ChevronRight size={15}/></IconButton>
    </div> : <p className="muted">本轮没有图像</p>}
    {view.incompleteGroups.length>0 && <div className="wp-capture-shots" role="group" aria-label="不完整帧组">
      {incompleteShown.map(seq=><button key={seq} className={"wp-capture-shot" + (previewK===seq-1 ? " selected" : "")} aria-pressed={previewK===seq-1} onClick={()=>setPreviewK(seq-1)}>
        <strong>{seq}</strong><i className="wp-capture-dot warn"/><span>缺 {view.missing.filter(m=>m.seq===seq).map(m=>cameraName(m.camera)).join("、")}</span>
      </button>)}
      {view.incompleteGroups.length>incompleteShown.length && <small className="muted">另有 {view.incompleteGroups.length-incompleteShown.length} 个不完整帧组</small>}
    </div>}
    {round.state==="complete" && <div className="wp-capture-actions">
      {teachedCameras.length>0 && <p className="wp-capture-done" role="status"><CheckCircle2 size={15} aria-hidden="true"/>已用于 {teachedCameras.map(nameOf).join("、")} 的走廊示教</p>}
      {calibrationEntry && <p className="wp-capture-done" role="status"><CheckCircle2 size={15} aria-hidden="true"/>{calibrationEntry.standard ? "已加入标定（标准件轮次）" : "已加入多轮标定"}</p>}
      {bankedAsSample && <Notice title="需要独立于验证样本的采集轮次" tone="warn">本轮已作为独立验证样本加入样本库，不能再用于走廊示教或标定，请另选轮次。</Notice>}
      {bankedAsSample ? <Button variant="tonal" data-variant="tonal" disabled>用于走廊示教</Button>
        : <Link className={"qs-btn" + (teachedCameras.length ? "" : " tonal")} data-variant={teachedCameras.length ? "filled" : "tonal"} to={`/recipe/teach?round=${encodeURIComponent(round.roundId)}`}>用于走廊示教<ArrowRight size={15} aria-hidden="true"/></Link>}
      {calibrationEntry
        ? <Button variant="tonal" data-variant="tonal" disabled={locked || productionBusy} onClick={removeFromCalibration}>移出标定</Button>
        : <>{!markedStandard && <Button variant="tonal" data-variant="tonal" disabled={locked || productionBusy || bankedAsSample} onClick={() => addToCalibration(false)}>加入标定</Button>}
          <Button variant="tonal" data-variant="tonal" disabled={locked || productionBusy || bankedAsSample} onClick={() => addToCalibration(true)}>加入标定（标准件）</Button></>}
      <Link className="wp-text-link" to="/recipe/calibration">前往多轮标定</Link>
    </div>}
    </div></div>
  </Panel>{groupCount > 0 && <CaptureReviewPanel key={`${doc.id}:${round.roundId}`} roundId={round.roundId} groupCount={groupCount} currentSeq={previewK + 1} currentFrameReady={!!shown && shownK === previewK && previewArrived && !previewError} segments={doc.segments ?? []} disabled={locked || productionBusy || round.state !== "complete" || bankedAsSample} inCalibration={!!calibrationEntry} onNavigate={seq => gotoGroup(seq - 1)}/>}</div>;
  const statusPanel = (round?.error || lossTexts.length>0) && <section className="wp-capture-status" aria-label="轮次状态">
    {round?.error && <Notice title="本轮异常，不能用于示教或标定" tone="warn">{round.error}</Notice>}
    {lossTexts.length>0 && <Notice title="疑似丢帧（只告警）" tone="warn"><ul>{lossTexts.map(text=><li key={text}>{text}</li>)}</ul></Notice>}
  </section>;
  const importPanel = <HistoryImportPanel key="import" recipeId={doc.id} revision={data.workspace.revision} disabled={locked} nameOf={nameOf} onError={setError} onImported={rounds=>{ if(current.current.alive && current.current.id===doc.id && rounds.length) {setRoundId(rounds[0]); setPreviewK(0);} }}/>;
  return <div className="wp-page"><WorkspaceBar /><div className="wp-columns"><div className="wp-stack">{importPanel}{resultPanel}{historyPanel}</div><div className="wp-capture-aside">{statusPanel}<section className="wp-capture-rules" aria-label="轮次规则"><h3>轮次规则</h3><div><strong>轮次来源</strong><p>维护模式跟着生产采图，再从历史选用导入为轮次；原图硬链接，不停线。手动配置的数据通过候选导入包带入轮次。</p></div><div><strong>按帧号配组，不按到达顺序</strong><p>同一触发序号的各相机帧配成一个帧组。某台相机缺帧只告警：该组标“检测不完整”，其余相机照常检测，也不会把下一张图补到缺失位置。相机帧号缺口及相机间帧数差异用于丢帧诊断。</p></div><div><strong>曝光与增益</strong><p>在<Link className="wp-text-link" to="/camera">相机页</Link>设定并保存，相机打开或重连时写入；轮次只记录采集时的值，不参与一致性比较。</p></div></section></div></div></div>;
}
