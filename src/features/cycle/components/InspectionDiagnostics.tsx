import { Alert, Drawer, SegmentedControl, Table, type TableColumn } from "qing-steel";
import { verdictLabel } from "../../history";
import { expectedText, groupCellState, groupCellText, type GroupCellState } from "../../plan/vis";
import type { FrameView, LogLine, PartView, Readiness, Recipe, Snapshot, Verdict } from "../types";
import { CycleStepper, SignalLamps } from "./CycleStatus";
import ReadinessChecklist from "./ReadinessChecklist";
import ContextHelp from "./ContextHelp";

export type DiagnosticGroup = "readiness" | "capture" | "plc" | "events";
const groups: [DiagnosticGroup, string][] = [["readiness", "开工检查"], ["capture", "采集与测量"], ["plc", "PLC 信号"], ["events", "事件"]];
const groupOptions = groups.map(([value, label]) => ({ value, label }));

type ViewResult = NonNullable<FrameView["viewResults"]>[number];
const viewColumns: TableColumn<ViewResult>[] = [
  { key: "view", header: "检测图", render: view => view.view },
  { key: "verdict", header: "结果", render: view => verdictLabel[view.verdict] },
  { key: "ms", header: "耗时", render: view => view.ms == null ? "—" : `${view.ms} ms` },
  { key: "error", header: "异常明细", render: view => view.error ?? "—" },
];

/** 段配方：选中帧组里每台相机一行 */
type BurstCell = { camera: string; state: GroupCellState; verdict: Verdict | null; frameCounter: number | null };
const burstColumns: TableColumn<BurstCell>[] = [
  { key: "camera", header: "相机", render: cell => cell.camera },
  { key: "state", header: "帧状态", render: cell => groupCellText[cell.state] },
  { key: "verdict", header: "单帧结论", render: cell => cell.verdict ? verdictLabel[cell.verdict] : "—" },
  { key: "counter", header: "设备帧计数", render: cell => cell.frameCounter ?? "—" },
];

export default function InspectionDiagnostics({ group, onGroup, onClose, snapshot, readiness, readinessError, layout, part, selected, selectedSeq = null, logs, offlineSample = false }: {
  offlineSample?: boolean;
  group: DiagnosticGroup; onGroup: (group: DiagnosticGroup) => void; onClose: () => void;
  snapshot: Snapshot | null; readiness: Readiness | null; readinessError?: string | null;
  layout: Recipe | null; part: PartView | null; selected: number; logs: LogLine[];
  /** 段配方在线件选中的帧组 seq；part.burst 有值时“采集与测量”按帧组显示，不看 selected */
  selectedSeq?: number | null;
}) {
  const burst = part?.burst ?? null;
  const burstGroup = burst && selectedSeq != null ? burst.groups.find(g => g.seq === selectedSeq) ?? null : null;
  const burstCells: BurstCell[] = burst ? burst.cameras.map((camera, c) => ({
    camera, state: burstGroup ? groupCellState(burstGroup, c) : "waiting", verdict: burstGroup?.cells[c]?.verdict ?? null, frameCounter: burstGroup?.cells[c]?.frameCounter ?? null,
  })) : [];
  const frame = part?.frames[selected];
  const alarms = [...new Set(snapshot?.alarms ?? [])];
  return <Drawer open title="检测诊断" className="inspection-drawer" onOpenChange={open => { if (!open) onClose(); }}>
    <SegmentedControl size="sm" label="诊断分组" value={group} onValueChange={key => onGroup(key as DiagnosticGroup)} options={groupOptions} />
    <section className="diagnostic-content" aria-label={groups.find(([key]) => key === group)?.[1]}>
      {group === "readiness" && <>
        {offlineSample && <Alert tone="info">{readiness?.candidateDebug ? `下方检查候选 ${readiness.candidateDebug.recipeId} · 修订 ${readiness.candidateDebug.revision}；通过不代表生产发布。` : "下方为已发布配方的开工报告；候选离线试运行不要求生产发布，启动时另行检查。"}</Alert>}
        {readinessError && <Alert tone="warn">开工条件更新失败，旧报告仅供参考：{readinessError}</Alert>}
        {readiness ? <ReadinessChecklist readiness={readiness} /> : <p className="muted">尚未取得开工报告</p>}
        {snapshot?.fault && <Alert tone="fault">{snapshot.fault}</Alert>}
        <h3>原始告警</h3>
        {alarms.length ? <ul className="diagnostic-alarms">{alarms.map(alarm => <li key={alarm}>{alarm}</li>)}</ul> : <p className="muted">暂无告警</p>}
        <dl className="diagnostic-kv"><dt>配方修订</dt><dd>{layout?.revisionId ?? "—"}</dd><dt>工件周期</dt><dd>{part?.cycleId ?? "—"}</dd></dl>
        <CycleStepper snapshot={snapshot} />
      </>}
      {group === "capture" && burst && <>
        <h3>帧组 {selectedSeq ?? "—"}</h3>
        <dl className="diagnostic-kv">
          <dt>帧组 / 预期</dt><dd>{burst.groupCount} / {expectedText(burst.expectedTotal, burst.toleranceTotal)}</dd>
          <dt>各相机帧数</dt><dd>{burst.cameras.map((camera, c) => `${camera} ${burst.cameraFrames[c] ?? 0}`).join(" · ")}</dd>
          <dt>触发计数（参考）</dt><dd>{burst.deviceTriggers ?? "—"}</dd>
          <dt>已测 / 待测</dt><dd>{burst.measuredFrames} / {burst.pendingMeasure}</dd>
          <dt>测量线程</dt><dd>{snapshot ? `${snapshot.measurementWorkers.running} / ${snapshot.measurementWorkers.capacity}` : "—"}</dd><dt>超时线程</dt><dd>{snapshot?.measurementWorkers.timedOut ?? 0}</dd><dt>可用容量</dt><dd>{snapshot?.measurementWorkers.availableCapacity ?? "—"}</dd><dt>游离帧</dt><dd>{snapshot?.strayFrames ?? 0}</dd>
        </dl>
        <div className="panel-head"><h3>本帧组各相机</h3><ContextHelp label="帧组判定说明">同一触发序号的三台相机各出一帧，每帧单独判；某台相机缺帧时本组按其余相机判，只标检测不完整，不改 OK/NG。</ContextHelp></div>
        {burstGroup ? <Table caption="本帧组各相机" rows={burstCells} columns={burstColumns} rowKey={cell => cell.camera} /> : <p className="muted">本帧组尚未到达</p>}
      </>}
      {group === "capture" && !burst && <>
        <h3>拍照点 {layout?.shots[selected]?.poseId ?? "—"} · k{selected + 1}</h3>
        <dl className="diagnostic-kv">
          <dt>本件设备内顺序</dt><dd>{frame?.ordinal ?? "—"}</dd><dt>设备帧计数</dt><dd>{frame?.frameCounter ?? "—"}</dd><dt>设备触发计数</dt><dd>{frame?.triggerCounter ?? "—"}</dd>
          <dt>本件触发 / 收到</dt><dd>{part ? `${part.triggers} / ${part.received}` : "—"}</dd><dt>处理队列</dt><dd>{snapshot?.part?.queue ?? 0}</dd>
          <dt>测量线程</dt><dd>{snapshot ? `${snapshot.measurementWorkers.running} / ${snapshot.measurementWorkers.capacity}` : "—"}</dd><dt>超时线程</dt><dd>{snapshot?.measurementWorkers.timedOut ?? 0}</dd><dt>可用容量</dt><dd>{snapshot?.measurementWorkers.availableCapacity ?? "—"}</dd><dt>游离帧</dt><dd>{snapshot?.strayFrames ?? 0}</dd>
        </dl>
        <div className="panel-head"><h3>独立检测图结果</h3><ContextHelp label="多检测图判定说明">本点任意选中图 OK 则本点 OK；其余图的 NG 和检测异常仍保留。采集异常独立处理，本点结果不代表整件判定。</ContextHelp></div>
        {frame?.viewResults?.length ? <Table caption="独立检测图结果" rows={frame.viewResults} columns={viewColumns} rowKey={view => String(view.view)} /> : <p className="muted">暂无独立检测图结果</p>}
        {frame?.error && <Alert tone="fault">{frame.error}</Alert>}
      </>}
      {group === "plc" && <><h3>握手信号</h3><SignalLamps /><CycleStepper snapshot={snapshot} /><p className="muted">信号来自当前 PLC 点位；未绑定标签不代表信号为低。</p></>}
      {group === "events" && <div className="event-log">{logs.length ? logs.slice(-200).reverse().map((line, i) => <div key={`${line.ts}-${i}`} className={`l-${line.level}`}><time className="tm">{new Date(line.ts).toLocaleTimeString("zh-CN", { hour12: false })}.{String(line.ts % 1000).padStart(3, "0")}</time><span className="ev">{line.ev}</span>{line.msg}</div>) : <p className="muted">暂无事件</p>}</div>}
    </section>
  </Drawer>;
}
