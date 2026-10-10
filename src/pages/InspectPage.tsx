import { useEffect, useMemo, useRef, useState } from "react";
import { RotateCcw } from "lucide-react";
import RuntimeFrame, { usePublishedOverview } from "../features/workspace/RuntimeFrame";
import { WorkpieceOverview } from "../features/workspace/OverviewPage";
import Modal from "../features/plc/components/Modal";
import { displayReason, triggerModeLabel, verdictLabel } from "../features/history";
import { matchesPart } from "../features/cycle/identity";
import {
  computeVis,
  currentFrame,
  cycleApi,
  CycleStepper,
  shotCameras,
  ShotStrip,
  SignalLamps,
  ShotTiles,
  SimControls,
  UnrolledCurve,
  useCycle,
  useLayout,
  useRecipes,
  type LogLine,
  type Measured,
  type Recipe,
  type ResultView,
  type Snapshot,
  type Verdict,
} from "../features/cycle";

function verdictTone(v: Verdict) {
  return v.startsWith("OK") ? "v-ok" : v === "ERR_INSPECT" ? "v-err" : "v-ng";
}

function clock(ts: number) {
  const d = new Date(ts);
  return `${d.toLocaleTimeString("zh-CN", { hour12: false })}.${String(d.getMilliseconds()).padStart(3, "0")}`;
}

export default function InspectPage() {
  const { snapshot, logs, measured } = useCycle();
  const recipes = useRecipes();
  const [view, setView] = useState<"part" | "frame">("part");
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
  const layout = useLayout(layoutId, part ? part.recipeHash : summary?.hash);
  const phase = snapshot?.phase ?? "IDLE";
  const settled = phase === "REPORT" || phase === "RELEASE" || phase === "IDLE" || phase === "FAULT";
  const candidateResult = settled ? (snapshot?.result ?? null) : null;
  const result = candidateResult && (!part || (candidateResult.cycleId === part.cycleId && candidateResult.recipeId === part.recipeId)) ? candidateResult : null;
  const partResult = result && part && result.cycleId === part.cycleId ? result : null;
  const ownLayout = !!layout && !!part && layout.id === part.recipeId && layout.hash === part.recipeHash;
  const shown = useMemo(() => (ownLayout ? measured.filter(m => matchesPart(m, part)) : []), [ownLayout, measured, part]);
  const overview=usePublishedOverview(layout);
  const selectionScope = `${part?.cycleId ?? "idle"}:${part?.bundleHash ?? ""}:${part?.recipeHash ?? layout?.hash}:${layout?.id}:${layout?.shots.length}`;
  const operationScope = `${part?.cycleId ?? "idle"}:${snapshot?.since}:${snapshot?.fault ?? ""}`;
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
  const selectFrame = (k: number) => {
    if (!Number.isInteger(k) || k < 0 || k > lastShot) return;
    setFrameSelection({ scope: selectionScope, k });
    setPointSelection(null);
  };
  const selectPoint = (j: number | null) => {
    if (j === null) { setPointSelection(null); return; }
    if (!layout || !Number.isInteger(j) || j < 0 || j >= layout.points.k.length) return;
    setPointSelection({ scope: selectionScope, j });
    const k = layout.points.k[j];
    if (k >= 0 && k <= lastShot) setFrameSelection({ scope: selectionScope, k });
  };
  const trigger = snapshot?.triggerMode ?? layout?.triggerMode;
  const cams = layout ? shotCameras(layout.shots) : [];
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
      if (mounted.current && request === switchRequest.current) setSwitchError(String(e));
    } finally {
      activeAction.current = null;
      if (mounted.current) setAction(null);
    }
  };
  const resetFault = async () => {
    if (activeAction.current || phase !== "FAULT") return;
    const scope = operationScope;
    activeAction.current = "reset"; setAction("reset"); setResetError("");
    try { await cycleApi.reset(); }
    catch (error) {
      if (mounted.current && currentOperation.current.phase === "FAULT" && currentOperation.current.scope === scope) setResetError(String(error));
    } finally {
      activeAction.current = null;
      if (mounted.current) setAction(null);
    }
  };

  return (
    <div className="fly">
      <div className="fly-bar">
        <span className="fly-chip">{triggerModeLabel(trigger)}</span>
        {snapshot?.productSource === "manual" ? (
          <span className="fly-chip">
            配方
            <select
              id="manual-recipe"
              aria-label="当前检测配方"
              className="input"
              value={snapshot.activeRecipeId ?? ""}
              onChange={(e) => requestSwitch(e.target.value)}
              disabled={!canSwitch || action !== null}
            >
              {!snapshot.activeRecipeId && <option value="">请选择</option>}
              {recipes.map((r) => (
                <option key={r.id} value={r.id}>
                  {r.id} · {r.name}
                </option>
              ))}
            </select>
          </span>
        ) : (
          <span className="fly-chip" title="型号由 PLC 下发的产品代码匹配">
            {layout ? `${layout.id} · v${layout.version}` : "等待 PLC 下发型号"}
            {layout && <span className="muted mono">#{layout.hash.slice(0, 6)}</span>}
          </span>
        )}
        <span className="fly-chip">
          SN <b className="mono">{part?.sn ?? result?.sn ?? "—"}</b>
        </span>
        <span className="spacer" />
        <CycleStepper snapshot={snapshot} />
      </div>

      <div className="fly-bar">
        <SignalLamps />
        <span className="spacer" />
        <SimControls compact />
        {phase === "FAULT" && (
          <button className="btn" onClick={resetFault} disabled={action !== null} aria-busy={action === "reset"}>
            <RotateCcw size={15} />
            {action === "reset" ? "复位中…" : "复位故障"}
          </button>
        )}
      </div>
      {resetError && <div className="notice error" role="alert">{resetError}</div>}
      {!!snapshot?.measurementWorkers.timedOut && <div className="notice error" role="alert">
        测量线程超时未返回：{snapshot.measurementWorkers.timedOut} / {snapshot.measurementWorkers.capacity}，当前可用容量 {snapshot.measurementWorkers.availableCapacity}。
        {snapshot.measurementWorkers.timedOut>=snapshot.measurementWorkers.capacity&&"全部测量容量被占用，当前不可布防。"}
      </div>}

      {snapshot?.alarms.map((a) => (
        <div key={a} className="alarm-bar">
          {a}
        </div>
      ))}

      <div className="fly-body">
        <div className="fly-main">
          <div className={`insp-top${cams.length > 1 ? " grid" : ""}`}>
            <div className="panel">
              <div className="panel-head">
                <h3 className="panel-title">主视图</h3>
                <div className="legend">
                  <span><i style={{ background: "var(--ok)" }} />合格</span>
                  <span><i style={{ background: "var(--warn)" }} />超公差（允许内）</span>
                  <span><i style={{ background: "var(--ng)" }} />NG / 断胶</span>
                  <span><i style={{ background: "var(--err)" }} />未测成</span>
                  <span><i style={{ background: "var(--text-disabled)" }} />待测</span>
                </div>
                <span className="spacer" />
                <div className="segmented">
                  <button className={view === "part" ? "active" : ""} aria-pressed={view === "part"} onClick={() => setView("part")}>整件</button>
                  <button className={view === "frame" ? "active" : ""} aria-pressed={view === "frame"} onClick={() => setView("frame")}>逐拍照点</button>
                </div>
              </div>
              {layout ? view==="frame" ? <ShotTiles layout={layout} vis={vis} current={cur} selected={selected} onSelect={selectFrame} selectedPoint={selectedPoint} /> : <WorkpieceOverview layout={layout} overview={overview} selected={selected} onSelect={selectFrame} vis={vis}/> : <div className="empty">等待配方</div>}
            </div>
            <div className={`cam-tiles n${cams.length}`}>
              <RuntimeFrame part={ownLayout ? part : null} layout={layout} k={selected} measured={shown} vis={vis}/>
            </div>
          </div>

          <div className="panel">
            <div className="panel-head">
              <h3 className="panel-title">拍照点</h3>
              <span className="muted">拍照点按本件设备内触发顺序匹配；设备原始计数单独显示</span>
              <span className="spacer" />
              <span className="muted mono">
                Chunk 触发 {part?.triggers ?? 0} · 收到 {part?.received ?? 0}
              </span>
            </div>
            {layout && <ShotStrip layout={layout} part={part} vis={vis} selected={selected} onSelect={selectFrame}/>}
          </div>

          <div className="panel">
            <div className="panel-head">
              <h3 className="panel-title">展开曲线</h3>
              <span className="muted">
                横向偏移 d（mm，相对示教中线），横轴按拍照点分段、段内为沿中线的弧长；绿色带为公差，红虚线为绝对限，段与段之间不连
              </span>
            </div>
            {layout && <UnrolledCurve key={`${selectionScope}:d`} layout={layout} measured={shown} vis={vis} selected={selectedPoint} onSelect={selectPoint} />}
          </div>
        </div>

        <div className="fly-side">
          <VerdictCard phase={phase} result={result} sn={part?.sn} />
          <Progress snapshot={snapshot} result={partResult} />
          <SegmentPanel layout={layout} measured={shown} result={partResult} />
          <div className="panel">
            <h3 className="panel-title" style={{ marginBottom: 0 }}>事件</h3>
            <EventLog logs={logs} />
          </div>
        </div>
      </div>

      {pendingRecipe && (
        <Modal
          title="切换配方"
          onClose={closeSwitch}
          footer={
            <>
              {(switchError || switchProblem) && <span className="form-error" role="alert">{switchError || switchProblem}</span>}
              <button className="btn" disabled={action === "switch"} onClick={closeSwitch}>取消</button>
              <button className="btn primary" disabled={action !== null || !!switchProblem} aria-busy={action === "switch"} onClick={confirmSwitch}>{action === "switch" ? "切换中…" : "确认切换"}</button>
            </>
          }
        >
          <p>
            切换到 <b>{pendingRecipe}</b>，从下一个工件开始生效。请确认现场上料的型号与之一致。
          </p>
        </Modal>
      )}
    </div>
  );
}

function VerdictCard({ phase, result, sn }: { phase: string; result: ResultView | null; sn?: number }) {
  if (!result) {
    const idle = phase === "IDLE" || phase === "FAULT";
    return (
      <div className="panel fly-verdict v-run">
        <div className="vl"><span>判定结果</span><span className="mono">PLC —</span></div>
        <strong>{idle ? "等待工件" : "检测中…"}</strong>
        <div className="vr">{idle ? "" : `SN ${sn ?? "—"}`}</div>
      </div>
    );
  }
  return (
    <div className={`panel fly-verdict ${verdictTone(result.verdict)}`}>
      <div className="vl">
        <span>判定结果 · SN {result.sn}</span>
        <span className="mono">PLC {result.plcCode}{result.faultCode ? ` / ${result.faultCode}` : ""}</span>
      </div>
      <strong>{verdictLabel[result.verdict]}</strong>
      <div className="vr">{displayReason(result.reason)}</div>
    </div>
  );
}

function Progress({ snapshot, result }: { snapshot: Snapshot | null; result: ResultView | null }) {
  const part = snapshot?.part;
  const pct = part && part.total ? (part.filled / part.total) * 100 : 0;
  return (
    <div className="panel">
      <h3 className="panel-title" style={{ marginBottom: 0 }}>本件进度</h3>
      <div className="kv2">
        <span>帧</span>
        <b>{part ? `${part.received} / ${part.n}` : "—"}</b>
        <span>测量点</span>
        <b>{part ? `${part.filled} / ${part.total}` : "—"}</b>
        <div className="bar"><i style={{ width: `${pct}%` }} /></div>
        <span>处理队列</span>
        <b>{part?.queue ?? 0}</b>
        <span>测量线程</span>
        <b>{snapshot ? `${snapshot.measurementWorkers.running} / ${snapshot.measurementWorkers.capacity}` : "—"}</b>
        <span>收尾耗时（partEnd→done）</span>
        <b>{result?.drainMs != null ? `${result.drainMs} ms` : "—"}</b>
        <span>游离帧</span>
        <b className={snapshot?.strayFrames ? "c-warn" : ""}>{snapshot?.strayFrames ?? 0}</b>
      </div>
      <Stats snapshot={snapshot} />
    </div>
  );
}

function SegmentPanel({ layout, measured, result }: { layout: Recipe | null; measured: Measured[]; result: ResultView | null }) {
  const ranges = useMemo(() => {
    if (!layout) return [];
    const r = layout.segments.map(() => ({ min: Infinity, max: -Infinity, wmin: Infinity, wmax: -Infinity }));
    measured.forEach((m) =>
      m.idx.forEach((j, i) => {
        if (m.st[i] !== 0) return;
        const g = r[layout.points.seg[j]];
        if (!g) return;
        g.min = Math.min(g.min, m.d[i]);
        g.max = Math.max(g.max, m.d[i]);
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
    <div className="panel">
      <div className="panel-head">
        <h3 className="panel-title">逐拍照点结果</h3>
        <span className="spacer" />
        <span className="muted" style={{ fontSize: 11 }}>{width ? "偏移 d · 胶宽" : "偏移 d 最小–最大"}</span>
      </div>
      <div className="seg-list">
        {layout?.segments.map((g, i) => {
          const s = result?.segments[i];
          const tone = s ? (s.verdict === "OK" ? "c-ok" : s.verdict === "OK_WITH_EXCURSION" ? "c-warn" : "c-ng") : errored ? "c-err" : "c-mut";
          const rg = ranges[i];
          const d = rg && rg.min <= rg.max ? `${rg.min.toFixed(2)}–${rg.max.toFixed(2)}` : "—";
          const w = rg && rg.wmin <= rg.wmax ? ` · ${rg.wmin.toFixed(2)}–${rg.wmax.toFixed(2)}` : "";
          return (
            <div key={g.name}>
              <span className={`dot ${tone}`} style={{ background: "currentColor" }} />
              <span>{g.name}</span>
              <span className="rng">{d}{w}</span>
              <span className={`ss ${tone}`}>
                {s ? (s.verdict === "OK_WITH_EXCURSION" ? `局部超差 ${Math.max(s.excursionLen, s.wExcursionLen ?? 0).toFixed(1)}` : s.verdict) : errored ? "不可判" : "—"}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function EventLog({ logs }: { logs: LogLine[] }) {
  return (
    <div className="event-log">
      {logs
        .slice(-80)
        .reverse()
        .map((l, i) => (
          <div key={`${l.ts}-${i}`} className={`l-${l.level}`}>
            <span className="tm">{clock(l.ts)}</span>
            <span className="ev">{l.ev}</span>
            {l.msg}
          </div>
        ))}
    </div>
  );
}

function Stats({ snapshot }: { snapshot: Snapshot | null }) {
  const s = snapshot?.stats ?? { total: 0, ok: 0, ng: 0, err: 0 };
  const items: [string, string | number, string][] = [
    ["今日", s.total, ""],
    ["OK", s.ok, "c-ok"],
    ["NG", s.ng, "c-ng"],
    ["ERR", s.err, "c-err"],
    ["良率", s.total ? `${((s.ok / s.total) * 100).toFixed(1)}%` : "--", ""],
  ];
  return (
    <div className="stats5">
      {items.map(([label, value, tone]) => (
        <div key={label}>
          <span>{label}</span>
          <b className={tone}>{value}</b>
        </div>
      ))}
    </div>
  );
}
