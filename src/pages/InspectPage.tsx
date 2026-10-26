import CandidateDebugControls from "../features/cycle/components/CandidateDebugControls";
import { Fragment, useEffect, useRef, useState } from "react";
import { RotateCcw } from "lucide-react";
import { Alert, Button, Card, Chip, Dialog, EmptyState, Progress as ProgressBar, SegmentedControl, Select } from "qing-steel";
import "./inspect.css";
import { BurstRuntimeFrame } from "../features/workspace/RuntimeFrame";
import { CameraSegmentOverview } from "../features/plan/CameraSegmentOverview";
import { expectedText, flagBadges } from "../features/plan/vis";
import type { PartViewBurst } from "../features/plan/types";
import { displayReason, triggerModeLabel, verdictLabel, verdictTone } from "../features/history";
import InspectionReadiness from "../features/cycle/components/InspectionReadiness";
import InspectionDiagnostics, { type DiagnosticGroup } from "../features/cycle/components/InspectionDiagnostics";
import ContextHelp from "../features/cycle/components/ContextHelp";
import { CollectionProgressPanel, MaintenanceBanner, MaintenanceToggle } from "../features/cycle/components/MaintenanceMode";
import { errorText } from "../lib/errors";
import {
  cycleApi,
  FrameGroupStrip,
  RegionResultPanel,
  OfflineControls,
  useBurstSegments,
  useCycle,
  useLayout,
  useReadiness,
  useRecipes,
  useSiteStatus,
  type PartView,
  type ResultView,
  type Snapshot,
  type Verdict,
} from "../features/cycle";
import { usePlcOperationState } from "../features/plc/api";

function verdictParts(v: Verdict) {
  const [code, detail] = verdictLabel[v].split(" · ");
  return { code, detail: detail ?? "整件合格" };
}

const phaseLabels: Record<string, string> = { IDLE: "空闲", VALIDATE: "校验", ACQUIRE: "采集", DRAIN: "收尾", JUDGE: "判定", REPORT: "回写", RELEASE: "释放", FAULT: "故障" };

const viewOptions = [{ value: "image", label: "原图" }, { value: "part", label: "整件布置" }];

export default function InspectPage() {
  const { snapshot, logs } = useCycle();
  const recipes = useRecipes();
  const site = useSiteStatus();
  const { handshake } = usePlcOperationState();
  // 真机握手待复位时沿用后端的复位前置检查：string 为阻塞原因，null 为可复位，undefined 为尚未核对。
  const s7Reset = site?.mode !== "offline" && (handshake?.phase === "fault" || handshake?.phase === "resetRequired") ? handshake : null;
  const resetBlockedReason = s7Reset?.resetBlockedReason || "";
  const resetPendingCheck = !!s7Reset && s7Reset.resetBlockedReason === undefined;
  const resetInFlight = handshake?.requestSeq != null;
  // 会话空闲而 PLC 处于故障（plcState=90）时，后端判定可复位，同样提供软件复位
  const plcFaultReset = site?.mode !== "offline" && handshake?.phase === "idle" && handshake.resetBlockedReason === null;
  const [confirmReset, setConfirmReset] = useState(false);
  const [view, setView] = useState<"image" | "part">("image");
  const [diagnostics, setDiagnostics] = useState<DiagnosticGroup | null>(null);
  const [seqSelection, setSeqSelection] = useState<{ scope: string; seq: number } | null>(null);
  const [pendingRecipe, setPendingRecipe] = useState<string | null>(null);
  const [switchError, setSwitchError] = useState("");
  const [action, setAction] = useState<"switch" | "reset" | null>(null);
  const [resetError, setResetError] = useState("");
  const activeAction = useRef<"switch" | "reset" | null>(null);
  const mounted = useRef(false);
  const switchRequest = useRef(0);

  const part = snapshot?.part ?? null;
  const layoutId = part?.recipeId ?? snapshot?.activeRecipeId ?? recipes[0]?.id;
  const summary = recipes.find((r) => r.id === layoutId);
  const layout = useLayout(layoutId, part ? part.recipeRevision : summary?.revisionId);
  const phase = snapshot?.phase ?? "IDLE";
  const showReset = phase === "FAULT" || plcFaultReset;
  const settled = phase === "REPORT" || phase === "RELEASE" || phase === "IDLE" || phase === "FAULT";
  const candidateResult = settled ? (snapshot?.result ?? null) : null;
  const result = candidateResult && (!part || (candidateResult.cycleId === part.cycleId && candidateResult.recipeId === part.recipeId)) ? candidateResult : null;
  const partResult = result && part && result.cycleId === part.cycleId ? result : null;
  const burst = part?.burst ?? null;
  const burstJudgement = result?.burst ?? null;
  const burstSegments = useBurstSegments(layoutId, part?.recipeRevision ?? summary?.revisionId);
  const burstScope = `${part?.cycleId ?? "idle"}:${part?.bundleId ?? ""}:${part?.recipeRevision ?? ""}`;
  const latestSeq = burst?.groups.reduce<number | null>((latest, group) => latest == null || group.seq > latest ? group.seq : latest, null) ?? null;
  const burstManual = seqSelection?.scope === burstScope && !!burst?.groups.some(group => group.seq === seqSelection.seq);
  const burstSeq = burstManual ? seqSelection!.seq : latestSeq;
  const operationScope = `${part?.cycleId ?? "idle"}:${snapshot?.since}:${snapshot?.fault ?? ""}`;
  const { readiness, status: readinessStatus, error: readinessError, refresh: refreshReadiness } = useReadiness(`${phase}:${snapshot?.fault ?? ""}:${snapshot?.activeRecipeId ?? ""}:${snapshot?.productSource ?? ""}:${snapshot?.alarms.length ?? 0}`);
  const currentOperation = useRef({ phase, scope: operationScope });
  currentOperation.current = { phase, scope: operationScope };

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; switchRequest.current++; };
  }, []);
  useEffect(() => setResetError(""), [phase, operationScope]);

  const resumeFollowing = () => setSeqSelection(null);
  const selectGroup = (seq: number) => {
    if (!burst?.groups.some(group => group.seq === seq)) return;
    setSeqSelection({ scope: burstScope, seq });
    setView("image");
  };
  const trigger = snapshot?.triggerMode ?? layout?.triggerMode;
  const canSwitch = (phase === "IDLE" || phase === "FAULT") && snapshot?.productSource === "manual";
  const switchProblem = !canSwitch ? "当前工件已开始或型号由 PLC 下发，暂时不能切换配方" : pendingRecipe && !recipes.some(recipe => recipe.id === pendingRecipe) ? "该配方已不在配方库中，请重新选择" : "";

  const closeSwitch = () => {
    if (activeAction.current === "switch") return;
    switchRequest.current++;
    setPendingRecipe(null); setSwitchError("");
  };
  const requestSwitch = (id: string) => {
    if (activeAction.current || !canSwitch || !id || id === snapshot?.activeRecipeId) return;
    switchRequest.current++;
    setPendingRecipe(id); setSwitchError("");
  };

  const confirmSwitch = async () => {
    if (!pendingRecipe || activeAction.current || switchProblem) return;
    const request = ++switchRequest.current;
    activeAction.current = "switch"; setAction("switch"); setSwitchError("");
    try {
      await cycleApi.selectRecipe(pendingRecipe);
      if (mounted.current && request === switchRequest.current) {
        setPendingRecipe(null); setSwitchError("");
      }
    } catch (e) {
      if (mounted.current && request === switchRequest.current) setSwitchError(errorText(e));
    } finally {
      activeAction.current = null;
      if (mounted.current) setAction(null);
    }
  };
  const resetFault = async () => {
    if (activeAction.current || !showReset || resetBlockedReason) return;
    const scope = operationScope;
    activeAction.current = "reset"; setAction("reset"); setResetError("");
    try { await cycleApi.reset(); }
    catch (error) {
      if (mounted.current && currentOperation.current.phase === "FAULT" && currentOperation.current.scope === scope) setResetError(errorText(error));
    } finally {
      activeAction.current = null;
      if (mounted.current) setAction(null);
    }
  };

  const offlineSample = site?.mode === "offline" && site.source === "sampleSet";
  const activeDebug = snapshot?.candidateDebug;
  const debug = activeDebug ?? part?.candidateDebug ?? result?.candidateDebug;
  const busy = !!snapshot && phase !== "IDLE" && phase !== "FAULT";
  return (
    <div className="prod">
      <MaintenanceBanner maintenance={snapshot?.maintenance} />
      {debug && <div className="qs-banner tertiary" role="status"><span><strong>候选配方调试结果</strong> · {debug.recipeId} · 修订 {debug.revision} · 未发布，仅用于离线测试，不作为生产验收。</span></div>}
      <CandidateDebugControls site={site} snapshot={snapshot} />
      <div className="prod-bar">
        <Chip>{triggerModeLabel(trigger)}</Chip>
        <Chip className={site?.mode === "offline" ? "tertiary" : undefined}>{site?.mode === "offline" ? "离线" : site ? "真机" : "模式确认中"}</Chip>
        {!activeDebug && snapshot?.productSource === "manual" ? (
          <Select
            id="manual-recipe"
            aria-label="当前检测配方"
            className="prod-recipe"
            value={snapshot.activeRecipeId ?? ""}
            onValueChange={requestSwitch}
            disabled={!canSwitch || action !== null}
            placeholder={snapshot.activeRecipeId ? undefined : "请选择"}
            options={recipes.map((r) => ({ value: r.id, label: `${r.id} · ${r.name}` }))}
          />
        ) : (
          <Chip className="prod-recipe" title="按 PLC 下发的型号匹配配方">
            {debug ? `${debug.recipeId} · 候选修订 ${debug.revision}` : layout ? `${layout.id} · v${layout.version}` : "等待 PLC 下发型号"}
          </Chip>
        )}
        <Chip className="prod-sn" title={part?.sn || result?.sn || undefined}>
          SN <b className="sn">{part?.sn || result?.sn || "—"}</b>
        </Chip>
        <span className="spacer" />
        <span className={`qs-badge prod-phase${phase === "FAULT" ? " fault" : busy ? " run" : ""}`}><span className="dot" />{snapshot ? phaseLabels[phase] : "状态确认中"}</span>
        {snapshot && <MaintenanceToggle maintenance={snapshot.maintenance} />}
        <Button onClick={() => setDiagnostics("readiness")}>诊断</Button>
        <OfflineControls readiness={readiness} refresh={refreshReadiness} publishedSection={offlineSample} candidateRunning={!!snapshot?.candidateDebug} />
        {showReset && (
          <>
            <ContextHelp label="复位条件">{site?.mode === "offline" ? "离线：由离线产线自动复位" : "真机：软件复位由 PC 置复位请求，PLC 在 10 s 内回空闲即完成，未响应可重新发起；PLC 也可在现场发起复位。复位条件不满足时右侧显示原因"}</ContextHelp>
            <Button onClick={() => setConfirmReset(true)} disabled={action !== null || !!resetBlockedReason} title={resetBlockedReason || undefined} loading={action === "reset"} leadingIcon={<RotateCcw size={15} />}>
              {action === "reset" ? "复位中…" : "软件复位"}
            </Button>
            {(resetBlockedReason || resetPendingCheck) && <span className="muted" role="note">{resetBlockedReason ? `不能复位：${resetBlockedReason}` : "复位条件核对中"}</span>}
          </>
        )}
      </div>
      {resetError && <Alert tone="ng">{resetError}</Alert>}
      {!!snapshot?.measurementWorkers.timedOut && <Alert tone="ng">
        测量线程超时未返回：{snapshot.measurementWorkers.timedOut} / {snapshot.measurementWorkers.capacity}，当前可用容量 {snapshot.measurementWorkers.availableCapacity}。
        {snapshot.measurementWorkers.timedOut>=snapshot.measurementWorkers.capacity&&"全部测量容量被占用，当前不可布防。"}
      </Alert>}
      {snapshot?.recordingAlarm && <Alert tone="warn">
        原图录制失败：连续 {snapshot.recordingAlarm.consecutive} 件原图未完整保存（最近 <span className="mono">{snapshot.recordingAlarm.cycleId}</span>：{snapshot.recordingAlarm.reason}）。检测与判定照常；请检查资料盘空间与录制目录写权限，下一件完整保存后自动消除。
      </Alert>}
      <CollectionProgressPanel collection={snapshot?.collection} recipes={recipes} />
      <InspectionReadiness offlineSample={offlineSample} snapshot={snapshot} readiness={readiness} status={readinessStatus} error={readinessError} onDiagnostics={() => setDiagnostics("readiness")} />

      <div className="prod-body">
        <div className="prod-main burst">
          <Card className="prod-view" role="region" aria-label="检测主视图">
            <div className="panel-head">
              <h3 className="panel-title">检测视图</h3>
              <ContextHelp label="查看范围说明">选择帧组后进入手动查看；返回当前帧组可恢复跟随。新工件自动恢复跟随，右侧始终保留整件判定。</ContextHelp>
              <span className="spacer" />
              {burstManual && <><span className="view-mode">手动查看</span><Button size="sm" onClick={resumeFollowing}>返回当前帧组</Button></>}
              <SegmentedControl size="sm" label="检测视图切换" value={view} onValueChange={next => setView(next as typeof view)} options={viewOptions} />
            </div>
            {view === "part"
              ? <CameraSegmentOverview cameras={burst?.cameras ?? summary?.cameras ?? []} segments={burstSegments} regions={burstJudgement?.regions} />
              : <BurstRuntimeFrame part={part} seq={burstSeq} />}
          </Card>

          <Card className="prod-shots">
            <div className="panel-head">
              <h3 className="panel-title">帧组</h3>
              <ContextHelp label="帧组说明">同一触发序号（相机 SDK 帧号）的三台相机各出一帧，配成一个帧组；每格是一台相机的一帧。绿色为合格，黄色为允许范围内的局部超差，红色为 NG，紫色为测量失败，蓝色为测量中；灰色为缺帧、未测或走出走廊，只算检测不完整，不影响 OK/NG。</ContextHelp>
              <span className="spacer" />
              <span className="muted mono">{burst ? `收到 ${burst.groupCount} 帧组 / 预期 ${expectedText(burst.expectedTotal, burst.toleranceTotal)}` : "—"}</span>
            </div>
            {burst ? <FrameGroupStrip groups={burst.groups} cameras={burst.cameras} selected={burstSeq} onSelect={selectGroup} /> : <EmptyState title="等待工件" description="新工件开始后按帧组显示三台相机的帧。" />}
          </Card>
        </div>

        <aside className="prod-side" aria-label="整件结果">
          <VerdictCard phase={phase} result={result} part={part} />
          <Stats snapshot={snapshot} />
          <Progress snapshot={snapshot} result={partResult} onDiagnostics={() => setDiagnostics("capture")} />
          <Card className="prod-segments" role="region" aria-label="检测区域结果">
            <div className="panel-head">
              <h3 className="panel-title">检测区域结果</h3>
              <ContextHelp label="区域结果说明">整件按区域（直线、弯道、未分段）分别判；每个区域写帧数是否在预期容差内、胶宽范围、胶偏或未判原因。未分段区域只跑每帧独立检测、不判胶偏。缺帧只标检测不完整，帧数异常只告警，都不改 OK/NG。</ContextHelp>
            </div>
            <RegionResultPanel judgement={burstJudgement} segments={burstSegments} cameras={burst?.cameras ?? summary?.cameras}
              expectedTotal={burst ? burst.expectedTotal : undefined} toleranceTotal={burst?.toleranceTotal} onSelectGroup={burst ? selectGroup : undefined} />
          </Card>
        </aside>
      </div>

      {diagnostics && <InspectionDiagnostics offlineSample={offlineSample} group={diagnostics} onGroup={setDiagnostics} onClose={() => setDiagnostics(null)} snapshot={snapshot} readiness={readiness} readinessError={readinessError} layout={layout} part={part} selectedSeq={burstSeq} logs={logs} />}
      {confirmReset && showReset && (
        <Dialog
          open
          title="软件复位"
          onOpenChange={open => { if (!open) setConfirmReset(false); }}
          footer={
            <>
              <Button onClick={() => setConfirmReset(false)}>取消</Button>
              <Button variant="filled" disabled={action !== null || !!resetBlockedReason} onClick={() => { setConfirmReset(false); void resetFault(); }}>确认复位</Button>
            </>
          }
        >
          <p>软件复位：PC 请求复位，等待 PLC 响应（10 s 内）。</p>
          {resetInFlight && <p role="note">在途件将记为未检后复位。</p>}
        </Dialog>
      )}
      {pendingRecipe && (
        <Dialog
          open
          title="切换配方"
          closeDisabled={action === "switch"}
          onOpenChange={open => { if (!open) closeSwitch(); }}
          footer={
            <>
              {(switchError || switchProblem) && <span className="form-error" role="alert">{switchError || switchProblem}</span>}
              <Button disabled={action === "switch"} onClick={closeSwitch}>取消</Button>
              <Button variant="filled" disabled={action !== null || !!switchProblem} loading={action === "switch"} onClick={confirmSwitch}>{action === "switch" ? "切换中…" : "确认切换"}</Button>
            </>
          }
        >
          <p>
            切换到 <b>{pendingRecipe}</b>，从下一个工件开始生效。请确认现场上料的型号与之一致。
          </p>
        </Dialog>
      )}
    </div>
  );
}

function VerdictCard({ phase, result, part }: { phase: string; result: ResultView | null; part: PartView | null }) {
  const card = useRef<HTMLElement>(null);
  const running = !result && phase !== "IDLE" && phase !== "FAULT";
  const stateKey = result ? `${result.cycleId}:${result.ts}` : running ? `run:${part?.cycleId ?? ""}` : "idle";
  const tone = result ? (result.mode === "maintenanceCollect" ? "idle" : verdictTone(result.verdict)) : running ? "run" : "idle";
  const initial = useRef(stateKey);
  const animate = useRef(false);
  if (initial.current !== stateKey) animate.current = true;
  const [emphasis, setEmphasis] = useState(false);
  useEffect(() => {
    if (animate.current && tone === "ng") setEmphasis(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stateKey]);
  const parts = result ? (result.mode === "maintenanceCollect" ? { code: "未判定", detail: "维护采集，只录图" }
    : { ...verdictParts(result.verdict), ...(result.mode === "maintenanceShadow" ? { detail: `试判 · ${verdictParts(result.verdict).detail}` } : {}) }) : null;
  const reason = result ? displayReason(result.reason) : "";
  // 整件标记：检测不完整、帧数异常、胶偏未判、含未分段、疑似提前终止。只告警，不改上面的结论
  const flags = flagBadges(result?.burst?.flags);
  const progress = part ? burstProgressText(part.burst) : "—";
  return (
    <section ref={card} aria-label="整件判定" aria-live="polite"
      className={`fly-verdict qs-verdict xl ${tone}${animate.current ? " in" : ""}${emphasis ? " emph" : ""}`}
      onAnimationEnd={event => { if (event.target === card.current) setEmphasis(false); }}>
      <div className="vh">
        <span title={result ? `SN ${result.sn}` : undefined}>{result ? <>判定结果 · SN <span className="sn">{result.sn}</span></> : "判定结果"}</span>
        <span className="mono">PLC {result ? `${result.plcCode}${result.faultCode ? ` / ${result.faultCode}` : ""}` : "—"}</span>
      </div>
      <Fragment key={stateKey}>
        {result && parts ? <>
          <div className="big">{parts.code}</div>
          <div className="t">{parts.detail}</div>
          <div className="d" title={reason}>{reason}</div>
          {flags.length > 0 && <div className="foot">
            {flags.map(flag => <span key={flag.key} data-flag={flag.key}>{flag.label}</span>)}
          </div>}
        </> : running ? <>
          <div className="big"><span className="pulse" /></div>
          <div className="t">检测中…</div>
          <div className="d">SN <span className="sn">{part?.sn || "—"}</span> · 帧组 {progress}</div>
        </> : <>
          <div className="big">—</div>
          <div className="t">暂无检测结果</div>
          <div className="d">{phase === "FAULT" ? "设备故障，复位后继续接收工件" : "等待工件"}</div>
        </>}
      </Fragment>
    </section>
  );
}

/** 在途件的进度：帧组数 / 预期总数 ± 容差；预期待标定时只写已收到的帧组数。 */
function burstProgressText(burst: PartViewBurst) {
  return burst.expectedTotal == null ? String(burst.groupCount) : `${burst.groupCount} / ${expectedText(burst.expectedTotal, burst.toleranceTotal)}`;
}

/** 本件进度：帧组数对预期总数，已测帧数对已收帧数，各台相机收到的帧数分别列出（缺帧的相机一眼可见）。 */
function BurstProgress({ burst, result, strayFrames, onDiagnostics }: { burst: PartViewBurst | null; result: ResultView | null; strayFrames: number; onDiagnostics: () => void }) {
  const received = burst?.cameraFrames.reduce((sum, n) => sum + n, 0) ?? 0;
  return (
    <Card className="prod-progress" role="region" aria-label="本件进度">
      <div className="panel-head">
        <h3 className="panel-title">本件进度</h3>
        <span className="spacer" />
        <Button size="sm" onClick={onDiagnostics}>采集详情{strayFrames ? ` · 游离帧 ${strayFrames}` : ""}</Button>
      </div>
      <div className="prod-meters">
        <span>帧组</span>
        <ProgressBar label="帧组进度" value={burst?.groupCount ?? 0} max={burst?.expectedTotal || burst?.groupCount || 1} />
        <b>{burst ? burstProgressText(burst) : "—"}</b>
        <span>已测帧</span>
        <ProgressBar label="已测帧进度" value={burst?.measuredFrames ?? 0} max={received || 1} />
        <b>{burst ? `${burst.measuredFrames} / ${received}` : "—"}</b>
        {burst && <>
          <span>各相机帧数</span>
          <span />
          <b title={burst.cameras.map((camera, c) => `${camera} ${burst.cameraFrames[c] ?? 0}`).join(" · ")}>{burst.cameraFrames.join(" / ")}</b>
        </>}
        {burst?.deviceTriggers != null && <><span>触发计数（参考）</span><span /><b>{burst.deviceTriggers}</b></>}
        <span>收尾耗时</span>
        <span />
        <b>{result?.drainMs != null ? `${result.drainMs} ms` : "—"}</b>
      </div>
    </Card>
  );
}

function Progress({ snapshot, result, onDiagnostics }: { snapshot: Snapshot | null; result: ResultView | null; onDiagnostics: () => void }) {
  return <BurstProgress burst={snapshot?.part?.burst ?? null} result={result} strayFrames={snapshot?.strayFrames ?? 0} onDiagnostics={onDiagnostics} />;
}

function Stats({ snapshot }: { snapshot: Snapshot | null }) {
  const s = snapshot?.stats ?? { total: 0, ok: 0, ng: 0, err: 0 };
  const debug = snapshot?.statsScope === "debug";
  return (
    <section className="qs-stats prod-stats" aria-label={debug ? "今日调试统计" : "今日统计"}>
      <div className="qs-stat wide"><small>{debug ? "今日调试" : "今日"}</small><b>{s.total}</b></div>
      <div className="qs-stat wide"><small>{debug ? "调试符合率" : "良率"}</small><b>{s.total ? <>{((s.ok / s.total) * 100).toFixed(1)}<small>%</small></> : "--"}</b></div>
      <div className="qs-stat ok"><small>OK</small><b>{s.ok}</b></div>
      <div className="qs-stat ng"><small>NG</small><b>{s.ng}</b></div>
      <div className="qs-stat fault"><small>ERR</small><b>{s.err}</b></div>
    </section>
  );
}