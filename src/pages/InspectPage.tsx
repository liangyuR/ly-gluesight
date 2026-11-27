import { useEffect, useRef, useState } from "react";
import { Alert, Button, Card, Chip, Dialog, EmptyState, Progress as ProgressBar, SegmentedControl, Select } from "qing-steel";
import "./inspect.css";
import { BurstRuntimeFrame } from "../features/workspace/RuntimeFrame";
import { CameraSegmentOverview } from "../features/plan/CameraSegmentOverview";
import { expectedText } from "../features/plan/vis";
import type { PartViewBurst, PathSegment } from "../features/plan/types";
import InspectionReadiness from "../features/cycle/components/InspectionReadiness";
import InspectionDiagnostics, { type DiagnosticGroup } from "../features/cycle/components/InspectionDiagnostics";
import ContextHelp from "../features/cycle/components/ContextHelp";
import { MaintenanceToggle } from "../features/cycle/components/MaintenanceMode";
import PartResult from "../features/cycle/components/PartResult";
import PreviousEvidence, { usePreviousEvidence } from "../features/cycle/components/PreviousEvidence";
import { errorText } from "../lib/errors";
import {
  cycleApi,
  FrameGroupStrip,
  RegionResultPanel,
  useCycle,
  useLayout,
  useReadiness,
  useRecipes,
  type ResultView,
  type Snapshot,
} from "../features/cycle";
import { TitlebarContent } from "../layout/TitlebarContent";

const phaseLabels: Record<string, string> = { IDLE: "空闲", VALIDATE: "接件", ACQUIRE: "采集", DRAIN: "收尾", JUDGE: "判定", FAULT: "故障" };

const viewOptions = [{ value: "image", label: "原图" }, { value: "part", label: "整件布置" }];
const noSegments: PathSegment[] = [];

export default function InspectPage() {
  const { snapshot, snapshotError, retrySnapshot } = useCycle();
  const recipes = useRecipes();
  const [view, setView] = useState<"image" | "part">("image");
  const [diagnostics, setDiagnostics] = useState<DiagnosticGroup | null>(null);
  const [seqSelection, setSeqSelection] = useState<{ scope: string; seq: number; camera?: number } | null>(null);
  const { lastEvidence, setLastEvidence } = usePreviousEvidence();
  const [pendingRecipe, setPendingRecipe] = useState<string | null>(null);
  const [switchError, setSwitchError] = useState("");
  const [action, setAction] = useState<"switch" | null>(null);
  const activeAction = useRef<"switch" | null>(null);
  const mounted = useRef(false);
  const switchRequest = useRef(0);

  const part = snapshot?.part ?? null;
  const layoutId = part?.recipeId ?? snapshot?.activeRecipeId ?? recipes[0]?.id;
  const summary = recipes.find((r) => r.id === layoutId);
  const recipeRevision = part?.recipeRevision ?? summary?.revisionId;
  const layout = useLayout(layoutId, recipeRevision);
  const phase = snapshot?.phase ?? "IDLE";
  const settled = phase === "IDLE" || phase === "FAULT";
  const candidateResult = settled ? (snapshot?.result ?? null) : null;
  const result = candidateResult && (!part || (candidateResult.cycleId === part.cycleId && candidateResult.recipeId === part.recipeId)) ? candidateResult : null;
  const resultScope = result ? `${result.cycleId}:${result.ts}` : "current";
  const partResult = result && part && result.cycleId === part.cycleId ? result : null;
  const burst = part?.burst ?? null;
  const burstJudgement = result?.burst ?? null;
  const burstSegments = layout && (!recipeRevision || layout.revisionId === recipeRevision) ? layout.pathSegments : noSegments;
  const burstScope = `${part?.cycleId ?? "idle"}:${part?.bundleId ?? ""}:${part?.recipeRevision ?? ""}`;
  const latestSeq = burst?.groups.reduce<number | null>((latest, group) => latest == null || group.seq > latest ? group.seq : latest, null) ?? null;
  const burstManual = seqSelection?.scope === burstScope && !!burst?.groups.some(group => group.seq === seqSelection.seq);
  const burstSeq = burstManual ? seqSelection!.seq : latestSeq;
  const { readiness, status: readinessStatus, error: readinessError } = useReadiness(`${phase}:${snapshot?.fault ?? ""}:${snapshot?.activeRecipeId ?? ""}:${snapshot?.productSource ?? ""}:${snapshot?.alarms.length ?? 0}`);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; switchRequest.current++; };
  }, []);

  const resumeFollowing = () => setSeqSelection(null);
  const selectGroup = (seq: number, camera?: number) => {
    if (!burst?.groups.some(group => group.seq === seq)) return false;
    setSeqSelection({ scope: burstScope, seq, camera });
    setView("image");
    return true;
  };
  const locateEvidence = (seq: number, camera?: number) => {
    if (!selectGroup(seq, camera) || !partResult || !part) return;
    setLastEvidence({ cycleId: part.cycleId, sn: part.sn, seq, camera, cameraName: camera == null ? undefined : part.burst.cameras[camera] });
  };
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

  const busy = !!snapshot && phase !== "IDLE" && phase !== "FAULT";
  return (
    <div className="prod">
      <TitlebarContent slot="details"><div className="inspect-title-details" role="group" aria-label="当前检测信息">
        {snapshot?.productSource === "manual" ? (
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
            {layout ? `${layout.id} · v${layout.version}` : "等待 PLC 下发型号"}
          </Chip>
        )}
        <Chip className="prod-sn" title={part?.sn || result?.sn || undefined}>
          SN <b className="sn">{part?.sn || result?.sn || "—"}</b>
        </Chip>
      </div></TitlebarContent>
      <TitlebarContent slot="actions"><div className="inspect-title-actions" role="group" aria-label="在线检测操作">
        <span className={`qs-badge prod-phase${phase === "FAULT" ? " fault" : busy ? " run" : ""}`}><span className="dot" />{snapshot ? phaseLabels[phase] : snapshotError ? "状态读取失败" : "状态确认中"}</span>
        {snapshot && <MaintenanceToggle maintenance={snapshot.maintenance} compact />}
        <Button size="sm" variant="text" onClick={() => setDiagnostics("readiness")}>诊断</Button>
      </div></TitlebarContent>
      {snapshotError && <Alert tone="warn" title="在线检测状态读取失败"
        action={<Button size="sm" onClick={retrySnapshot}>重新读取</Button>}>{snapshotError}</Alert>}
      {!!snapshot?.measurementWorkers.timedOut && <Alert tone="ng">
        测量线程超时未返回：{snapshot.measurementWorkers.timedOut} / {snapshot.measurementWorkers.capacity}，当前可用容量 {snapshot.measurementWorkers.availableCapacity}。
        {snapshot.measurementWorkers.timedOut>=snapshot.measurementWorkers.capacity&&"全部测量容量被占用，新到的工件无法检测。"}
      </Alert>}
      {snapshot?.recordingAlarm && <Alert tone="warn">
        原图录制失败：连续 {snapshot.recordingAlarm.consecutive} 件原图未完整保存（最近 <span className="mono">{snapshot.recordingAlarm.cycleId}</span>：{snapshot.recordingAlarm.reason}）。检测与判定照常；请检查录制盘空间与录制目录写权限，下一件完整保存后自动消除。
      </Alert>}
      <InspectionReadiness snapshot={snapshot} readiness={readiness} status={readinessStatus} error={readinessError} onDiagnostics={() => setDiagnostics("readiness")} />

      <div className="prod-body">
        <div className="prod-main">
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
              : <BurstRuntimeFrame part={part} seq={burstSeq} regions={partResult?.burst?.regions} focusedCamera={burstManual ? seqSelection?.camera : undefined} />}
          </Card>

          <Card>
            <div className="panel-head">
              <h3 className="panel-title">帧组</h3>
              <ContextHelp label="帧组说明">同一触发序号（相机 SDK 帧号）的三台相机各出一帧，配成一个帧组；每格是一台相机的一帧，颜色只表示本帧：绿色为合格，黄色为允许范围内的局部超差，红色为 NG，紫色为测量失败，蓝色为测量中；灰色为缺帧、未测或走出走廊，只算检测不完整，不影响 OK/NG。本件判完后上方色带是各段结论，胶宽不均、整段胶偏等整段性问题看色带。</ContextHelp>
              <span className="spacer" />
              <span className="muted mono">{burst ? `收到 ${burst.groupCount} 帧组 / 预期 ${expectedText(burst.expectedTotal)}` : "—"}</span>
            </div>
            {burst ? <FrameGroupStrip groups={burst.groups} cameras={burst.cameras} regions={partResult?.burst?.regions} selected={burstSeq} onSelect={selectGroup} /> : <EmptyState title="等待工件" description="新工件开始后按帧组显示三台相机的帧。" />}
            {burst && !partResult?.burst && <p className="muted" role="note">当前为临时测量状态；本件判定完成后标出生产跳过帧，跳过帧不参与判定。</p>}
          </Card>
        </div>

        <aside className="prod-side" aria-label="整件结果">
          <PartResult key={resultScope} phase={phase} result={result} part={part}
            segments={burstSegments} onLocate={locateEvidence} />
          {lastEvidence && lastEvidence.cycleId !== part?.cycleId && <PreviousEvidence
            key={`${lastEvidence.cycleId}:${lastEvidence.seq}:${lastEvidence.camera ?? ""}`}
            evidence={lastEvidence} onDismiss={() => setLastEvidence(null)} />}
          <Stats snapshot={snapshot} />
          <Progress snapshot={snapshot} result={partResult} onDiagnostics={() => setDiagnostics("capture")} />
          <Card className="prod-segments" role="region" aria-label="检测区域结果">
            <div className="panel-head">
              <h3 className="panel-title">检测区域结果</h3>
              <ContextHelp label="区域结果说明">整件按区域（直线、弯道、未分段）分别判；每个区域写帧数、胶宽范围、胶偏或未判原因。未分段区域只跑每帧独立检测、不判胶偏。缺帧只标检测不完整，不改 OK/NG。</ContextHelp>
            </div>
            <RegionResultPanel key={resultScope} judgement={burstJudgement} segments={burstSegments} cameras={burst?.cameras ?? summary?.cameras}
              expectedTotal={burst ? burst.expectedTotal : undefined} onSelectGroup={burst ? locateEvidence : undefined} />
          </Card>
        </aside>
      </div>

      {diagnostics && <InspectionDiagnostics group={diagnostics} onGroup={setDiagnostics} onClose={() => setDiagnostics(null)} snapshot={snapshot} readiness={readiness} readinessError={readinessError} layout={layout} part={part} regions={partResult?.burst?.regions} selectedSeq={burstSeq} />}
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

/** 在途件的进度：帧组数 / 预期总数；预期待标定时只写已收到的帧组数。 */
function burstProgressText(burst: PartViewBurst) {
  return burst.expectedTotal == null ? String(burst.groupCount) : `${burst.groupCount} / ${expectedText(burst.expectedTotal)}`;
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
  return (
    <section className="qs-stats prod-stats" aria-label="今日统计">
      <div className="qs-stat wide"><small>今日</small><b>{s.total}</b></div>
      <div className="qs-stat wide"><small>良率</small><b>{s.total ? <>{((s.ok / s.total) * 100).toFixed(1)}<small>%</small></> : "--"}</b></div>
      <div className="qs-stat ok"><small>OK</small><b>{s.ok}</b></div>
      <div className="qs-stat ng"><small>NG</small><b>{s.ng}</b></div>
      <div className="qs-stat fault"><small>ERR</small><b>{s.err}</b></div>
    </section>
  );
}
