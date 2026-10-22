import CandidateDebugControls from "../features/cycle/components/CandidateDebugControls";
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { RotateCcw } from "lucide-react";
import { Alert, Button, Card, Chip, Dialog, EmptyState, Progress as ProgressBar, SegmentedControl, Select, StatusBadge } from "qing-steel";
import "./inspect.css";
import RuntimeFrame, { usePublishedOverview } from "../features/workspace/RuntimeFrame";
import { WorkpieceOverview } from "../features/workspace/OverviewPage";
import { displayReason, triggerModeLabel, verdictLabel, verdictTone } from "../features/history";
import { matchesPart } from "../features/cycle/identity";
import InspectionReadiness from "../features/cycle/components/InspectionReadiness";
import InspectionDiagnostics, { type DiagnosticGroup } from "../features/cycle/components/InspectionDiagnostics";
import ContextHelp from "../features/cycle/components/ContextHelp";
import { errorText } from "../lib/errors";
import {
  computeVis,
  currentFrame,
  cycleApi,
  ShotStrip,
  ShotTiles,
  OfflineControls,
  partReferenceBadge,
  UnrolledCurve,
  useCycle,
  useLayout,
  useReadiness,
  useRecipes,
  useSiteStatus,
  type Measured,
  type PartView,
  type Recipe,
  type ResultView,
  type Snapshot,
  type Verdict,
} from "../features/cycle";
import { ST } from "../features/cycle/types";
import { partFrames } from "../features/cycle/pose";
import { usePlcOperationState } from "../features/plc/api";

function verdictParts(v: Verdict) {
  const [code, detail] = verdictLabel[v].split(" · ");
  return { code, detail: detail ?? "整件合格" };
}

const phaseLabels: Record<string, string> = { IDLE: "空闲", VALIDATE: "校验", ACQUIRE: "采集", DRAIN: "收尾", JUDGE: "判定", REPORT: "回写", RELEASE: "释放", FAULT: "故障" };

const viewOptions = [{ value: "image", label: "原图" }, { value: "part", label: "整件布置" }, { value: "frame", label: "逐拍照点" }];
const quantityOptions = [{ value: "d", label: "偏移" }, { value: "w", label: "胶宽" }];

export default function InspectPage() {
  const { snapshot, logs, measured } = useCycle();
  const recipes = useRecipes();
  const site = useSiteStatus();
  const { handshake } = usePlcOperationState();
  // 真机握手待复位时沿用后端的复位前置检查：string 为阻塞原因，null 为可复位，undefined 为尚未核对。
  const s7Reset = site?.mode !== "offline" && (handshake?.phase === "fault" || handshake?.phase === "resetRequired") ? handshake : null;
  const resetBlockedReason = s7Reset?.resetBlockedReason || "";
  const resetPendingCheck = !!s7Reset && s7Reset.resetBlockedReason === undefined;
  const [view, setView] = useState<"image" | "part" | "frame">("image");
  const [diagnostics, setDiagnostics] = useState<DiagnosticGroup | null>(null);
  const [quantity, setQuantity] = useState<"d" | "w">("d");
  const [frameSelection, setFrameSelection] = useState<{ scope: string; k: number } | null>(null);
  const [pointSelection, setPointSelection] = useState<{ scope: string; j: number } | null>(null);
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
  const settled = phase === "REPORT" || phase === "RELEASE" || phase === "IDLE" || phase === "FAULT";
  const candidateResult = settled ? (snapshot?.result ?? null) : null;
  const result = candidateResult && (!part || (candidateResult.cycleId === part.cycleId && candidateResult.recipeId === part.recipeId)) ? candidateResult : null;
  const partResult = result && part && result.cycleId === part.cycleId ? result : null;
  const ownLayout = !!layout && !!part && layout.id === part.recipeId && layout.revisionId === part.recipeRevision;
  const shown = useMemo(() => (ownLayout ? measured.filter(m => matchesPart(m, part)) : []), [ownLayout, measured, part]);
  const overview=usePublishedOverview(layout);
  const selectionScope = `${part?.cycleId ?? "idle"}:${part?.bundleId ?? ""}:${part?.recipeRevision ?? layout?.revisionId}:${layout?.id}:${layout?.shots.length}`;
  const operationScope = `${part?.cycleId ?? "idle"}:${snapshot?.since}:${snapshot?.fault ?? ""}`;
  const { readiness, status: readinessStatus, error: readinessError, refresh: refreshReadiness } = useReadiness(`${phase}:${snapshot?.fault ?? ""}:${snapshot?.activeRecipeId ?? ""}:${snapshot?.productSource ?? ""}:${snapshot?.alarms.length ?? 0}`);
  const currentOperation = useRef({ phase, scope: operationScope });
  currentOperation.current = { phase, scope: operationScope };

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; switchRequest.current++; };
  }, []);
  useEffect(() => setResetError(""), [phase, operationScope]);

  const resultKey = partResult ? `${partResult.cycleId}:${partResult.ts}` : null;
  const vis = useMemo(
    () => (layout ? computeVis(layout, ownLayout ? part : null, shown, ownLayout ? partResult : null) : []),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [layout, ownLayout, part, shown, resultKey],
  );
  const cur = phase === "ACQUIRE" || phase === "DRAIN" ? currentFrame(part) : -1;
  const lastShot = Math.max(0, (layout?.shots.length ?? 1) - 1);
  const selected = frameSelection?.scope === selectionScope ? Math.min(lastShot, Math.max(0, frameSelection.k)) : Math.min(lastShot, Math.max(0, currentFrame(ownLayout ? part : null)));
  const selectedPoint = pointSelection?.scope === selectionScope && pointSelection.j < (layout?.points.k.length ?? 0) ? pointSelection.j : null;
  const manualView = frameSelection?.scope === selectionScope;
  const resumeFollowing = () => { setFrameSelection(null); setPointSelection(null); };
  const selectFrame = (k: number) => {
    if (!Number.isInteger(k) || k < 0 || k > lastShot) return;
    setFrameSelection({ scope: selectionScope, k });
    setPointSelection(null);
    setView("image");
  };
  const selectPoint = (j: number | null) => {
    if (j === null) { setPointSelection(null); return; }
    if (!layout || !Number.isInteger(j) || j < 0 || j >= layout.points.k.length) return;
    setPointSelection({ scope: selectionScope, j });
    const k = layout.points.k[j];
    if (k >= 0 && k <= lastShot) { setFrameSelection({ scope: selectionScope, k }); setView("image"); }
  };
  const trigger = snapshot?.triggerMode ?? layout?.triggerMode;
  const curveQuantity = layout?.segments.some(segment => segment.width) ? quantity : "d";
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
    if (activeAction.current || phase !== "FAULT" || resetBlockedReason) return;
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
          <Chip className="prod-recipe" title="型号由 PLC 下发的产品代码匹配">
            {debug ? `${debug.recipeId} · 候选修订 ${debug.revision}` : layout ? `${layout.id} · v${layout.version}` : "等待 PLC 下发型号"}
          </Chip>
        )}
        <Chip className="prod-sn" title={part?.sn || result?.sn || undefined}>
          SN <b className="sn">{part?.sn || result?.sn || "—"}</b>
        </Chip>
        <span className="spacer" />
        <span className={`qs-badge prod-phase${phase === "FAULT" ? " fault" : busy ? " run" : ""}`}><span className="dot" />{snapshot ? phaseLabels[phase] : "状态确认中"}</span>
        <Button onClick={() => setDiagnostics("readiness")}>诊断</Button>
        <OfflineControls readiness={readiness} refresh={refreshReadiness} publishedSection={offlineSample} candidateRunning={!!snapshot?.candidateDebug} />
        {phase === "FAULT" && (
          <>
            <ContextHelp label="复位条件">{site?.mode === "offline" ? "离线：由离线产线自动复位" : "真机：由 PLC 发 faultReset 复位；PLC 已连接、心跳在翻转、设备与追溯就绪且无在途工件时也可点此复位，不满足时右侧显示原因"}</ContextHelp>
            <Button onClick={resetFault} disabled={action !== null || !!resetBlockedReason} title={resetBlockedReason || undefined} loading={action === "reset"} leadingIcon={<RotateCcw size={15} />}>
              {action === "reset" ? "复位中…" : "复位故障"}
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
      {!!snapshot?.locateAlarms?.length && <Alert tone="warn" title="特征定位连续失败" aria-label="连续定位失败告警">
        {/* 告警按配方计数，换配方后旧配方的告警还在；拍照点编号在配方间会重名，所以每条写明配方与修订，不在生产的配方排后并标出 */}
        <ul className="locate-alarms">{[...snapshot.locateAlarms].sort((a, b) => Number(a.recipeId !== snapshot.activeRecipeId) - Number(b.recipeId !== snapshot.activeRecipeId)).map(alarm => {
          const idle = alarm.recipeId !== snapshot.activeRecipeId;
          return <li key={`${alarm.recipeId}:${alarm.shotId}:${alarm.view}`} className={idle ? "idle" : undefined}>
            配方 {alarm.recipeId}（修订 <span className="mono">{alarm.revisionId}</span>）{idle && <b>（当前未在生产）</b>} · 拍照点 {alarm.shotId} · 图 {alarm.view} · 连续 {alarm.consecutive} 件没有位置基准 · 最近 <span className="mono">{alarm.cycleId}</span>：{alarm.reason}
          </li>;
        })}</ul>
        这些件有胶站不判位置。请检查上料、定位特征是否被遮挡或污染、照明与相机；该配方下一件这幅图定位成功后自动消除。
        {snapshot.locateAlarms.some(alarm => alarm.recipeId !== snapshot.activeRecipeId) && "标“当前未在生产”的是之前生产的配方留下的，换配方不会清除它：要等该配方再生产且定位成功、该配方新修订的第一件测完，或软件重启才消除。"}
      </Alert>}

      <InspectionReadiness offlineSample={offlineSample} snapshot={snapshot} readiness={readiness} status={readinessStatus} error={readinessError} onDiagnostics={() => setDiagnostics("readiness")} />

      <div className="prod-body">
        <div className="prod-main">
          <Card className="prod-view" role="region" aria-label="检测主视图">
            <div className="panel-head">
              <h3 className="panel-title">检测视图</h3>
              <ContextHelp label="查看范围说明">选择拍照点或曲线测量点后进入手动查看；返回当前拍照点可恢复跟随。新工件自动恢复跟随，右侧始终保留整件判定。</ContextHelp>
              <span className="spacer" />
              {manualView && <><span className="view-mode">手动查看</span><Button size="sm" onClick={resumeFollowing}>返回当前拍照点</Button></>}
              <SegmentedControl size="sm" label="检测视图切换" value={view} onValueChange={next => setView(next as typeof view)} options={viewOptions} />
            </div>
            {view === "image" ? <RuntimeFrame compact onDiagnostics={() => setDiagnostics("capture")} part={ownLayout ? part : null} layout={layout} k={selected} measured={shown} vis={vis}/> :
              layout ? view === "frame" ? <ShotTiles layout={layout} vis={vis} frames={partFrames(layout, part)} current={cur} selected={selected} onSelect={selectFrame} selectedPoint={selectedPoint} /> :
                <WorkpieceOverview layout={layout} overview={overview} selected={selected} onSelect={selectFrame} vis={vis}/> : <EmptyState title="等待配方" description="PLC 下发型号或手动选择配方后显示。" />}
          </Card>

          <Card className="prod-shots">
            <div className="panel-head">
              <h3 className="panel-title">拍照点</h3>
              <ContextHelp label="拍照点匹配说明">拍照点按本件设备内触发顺序匹配；设备原始计数在采集诊断中显示。绿色为合格，黄色为允许范围内的局部超差，红色为 NG 或断胶，紫色为采集或测量异常，灰色为待测。</ContextHelp>
              <span className="spacer" />
              <span className="muted mono">
                收到 {part?.received ?? 0} / {layout?.shots.length ?? 0}
              </span>
            </div>
            {layout && <ShotStrip compact layout={layout} part={part} vis={vis} selected={selected} onSelect={selectFrame}/>}
          </Card>

          <Card className="prod-curve">
            <div className="panel-head">
              <h3 className="panel-title">{curveQuantity === "d" ? "横向偏移" : "胶宽"} <span className="muted">mm</span></h3>
              <ContextHelp label="曲线读法">横轴按检测段展开，段内为沿示教中线的弧长，段与段之间不连接。绿色带为公差，红虚线为绝对限；点击曲线或用方向键选择测量点，可查看对应原图。横向偏移相对于示教中线。</ContextHelp>
              <div className="legend"><span><i className="tol" />公差带</span><span><i className="abs" />绝对限</span></div>
              {layout?.segments.some(segment => segment.width) && <SegmentedControl size="sm" label="曲线数据切换" value={curveQuantity} onValueChange={next => setQuantity(next as typeof quantity)} options={quantityOptions} />}
            </div>
            {layout && <UnrolledCurve compact key={`${selectionScope}:${curveQuantity}`} quantity={curveQuantity} layout={layout} measured={shown} vis={vis} selected={selectedPoint} onSelect={selectPoint} />}
          </Card>
        </div>

        <aside className="prod-side" aria-label="整件结果">
          <VerdictCard phase={phase} result={result} part={part} />
          <Stats snapshot={snapshot} />
          <Progress snapshot={snapshot} result={partResult} onDiagnostics={() => setDiagnostics("capture")} />
          <SegmentPanel layout={layout} measured={shown} result={partResult} />
        </aside>
      </div>

      {diagnostics && <InspectionDiagnostics offlineSample={offlineSample} group={diagnostics} onGroup={setDiagnostics} onClose={() => setDiagnostics(null)} snapshot={snapshot} readiness={readiness} readinessError={readinessError} layout={layout} part={ownLayout ? part : null} selected={selected} logs={logs} />}
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
  const tone = result ? verdictTone(result.verdict) : running ? "run" : "idle";
  const initial = useRef(stateKey);
  const animate = useRef(false);
  if (initial.current !== stateKey) animate.current = true;
  const [emphasis, setEmphasis] = useState(false);
  useEffect(() => {
    if (animate.current && tone === "ng") setEmphasis(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stateKey]);
  const parts = result ? verdictParts(result.verdict) : null;
  const reason = result ? displayReason(result.reason) : "";
  // 本件有帧没有位置基准：不论整件 NG 还是不可判都挂徽标（NG 优先于基准无效时 PLC 码看不出定位失败）
  const reference = result && part && result.cycleId === part.cycleId ? partReferenceBadge(part) : null;
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
          {reference && <div className="foot"><span aria-label={`位置基准：${reference}`}>{reference}</span></div>}
        </> : running ? <>
          <div className="big"><span className="pulse" /></div>
          <div className="t">检测中…</div>
          <div className="d">SN <span className="sn">{part?.sn || "—"}</span> · 帧 {part ? `${part.received} / ${part.n}` : "—"}</div>
        </> : <>
          <div className="big">—</div>
          <div className="t">暂无检测结果</div>
          <div className="d">{phase === "FAULT" ? "设备故障，复位后继续接收工件" : "等待工件"}</div>
        </>}
      </Fragment>
    </section>
  );
}

function Progress({ snapshot, result, onDiagnostics }: { snapshot: Snapshot | null; result: ResultView | null; onDiagnostics: () => void }) {
  const part = snapshot?.part;
  return (
    <Card className="prod-progress" role="region" aria-label="本件进度">
      <div className="panel-head">
        <h3 className="panel-title">本件进度</h3>
        <span className="spacer" />
        <Button size="sm" onClick={onDiagnostics}>采集详情{snapshot?.strayFrames ? ` · 游离帧 ${snapshot.strayFrames}` : ""}</Button>
      </div>
      <div className="prod-meters">
        <span>帧</span>
        <ProgressBar label="帧进度" value={part?.received ?? 0} max={part?.n || 1} />
        <b>{part ? `${part.received} / ${part.n}` : "—"}</b>
        <span>测量点</span>
        <ProgressBar label="测量点进度" value={part?.filled ?? 0} max={part?.total || 1} />
        <b>{part ? `${part.filled} / ${part.total}` : "—"}</b>
        <span>收尾耗时</span>
        <span />
        <b>{result?.drainMs != null ? `${result.drainMs} ms` : "—"}</b>
      </div>
    </Card>
  );
}

function segmentStatus(s: ResultView["segments"][number] | undefined, errored: boolean): [string, "ok" | "warn" | "ng" | "fault" | "none"] {
  if (!s) return errored ? ["不可判", "fault"] : ["—", "none"];
  if (s.verdict === "OK") return ["OK", "ok"];
  if (s.verdict === "OK_WITH_EXCURSION") return [`局部超差 ${Math.max(s.excursionLen, s.wExcursionLen ?? 0).toFixed(1)}`, "warn"];
  if (s.verdict === "ERR_INSPECT") return [s.referenceInvalid ? "基准无效" : "未测成", "fault"];
  return [verdictParts(s.verdict).detail, "ng"];
}

function SegmentPanel({ layout, measured, result }: { layout: Recipe | null; measured: Measured[]; result: ResultView | null }) {
  const ranges = useMemo(() => {
    if (!layout) return [];
    const r = layout.segments.map(() => ({ min: Infinity, max: -Infinity, wmin: Infinity, wmax: -Infinity }));
    measured.forEach((m) =>
      m.idx.forEach((j, i) => {
        // 位置只取有基准的有胶站；胶宽另取无基准、超出检测范围里测到的
        const st = m.st[i];
        if (st !== ST.ok && st !== ST.noRef && st !== ST.outOfRange) return;
        const g = r[layout.points.seg[j]];
        if (!g) return;
        const d = m.d[i];
        if (st === ST.ok && d != null) {
          g.min = Math.min(g.min, d);
          g.max = Math.max(g.max, d);
        }
        const w = m.w?.[i];
        if (w != null) {
          g.wmin = Math.min(g.wmin, w);
          g.wmax = Math.max(g.wmax, w);
        }
      }),
    );
    return r;
  }, [layout, measured]);

  const errored = result?.verdict === "ERR_INSPECT";
  const width = layout?.segments.some((g) => g.width);
  return (
    <Card className="prod-segments" role="region" aria-label="检测段结果">
      <div className="panel-head">
        <h3 className="panel-title">检测段结果</h3>
        <span className="spacer" />
        <span className="muted prod-segments-unit">{width ? "偏移 d · 胶宽 mm" : "偏移 d 最小–最大 mm"}</span>
      </div>
      {layout?.segments.length ? <ul className="prod-seg-list">
        {layout.segments.map((g, i) => {
          const [label, tone] = segmentStatus(result?.segments[i], errored);
          const rg = ranges[i];
          const d = rg && rg.min <= rg.max ? `${rg.min.toFixed(2)}–${rg.max.toFixed(2)}` : "—";
          const w = rg && rg.wmin <= rg.wmax ? ` · ${rg.wmin.toFixed(2)}–${rg.wmax.toFixed(2)}` : "";
          return (
            <li key={`${g.name}:${i}`} className={`prod-seg s-${tone}`}>
              <span className="nm" title={g.name}>{g.name}</span>
              {tone === "none" ? <span className="muted">{label}</span> : <StatusBadge tone={tone}>{label}</StatusBadge>}
              <span className="rng mono">{d}{w}</span>
            </li>
          );
        })}
      </ul> : <EmptyState title="等待配方" />}
    </Card>
  );
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