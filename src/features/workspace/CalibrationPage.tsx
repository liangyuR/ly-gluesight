import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { ArrowRight } from "lucide-react";
import { Button, Table, type TableColumn } from "qing-steel";
import { errorText } from "../../lib/errors";
import { CameraSegmentOverview } from "../plan/CameraSegmentOverview";
import type { CalibrationRoundView, SegmentCalibView } from "../plan/types";
import { expectedText, segmentKindLabel } from "../plan/vis";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Notice, NumberField, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { CaptureSummary } from "./types";
import { CalibrationRoundsField } from "./DocParams";
import { CalibrationFrames } from "./CalibrationFrames";
import "./corridor.css";

/**
 * 多轮标定（段配方，路由 /recipe/calibration）：同一工件程序连续采集多轮，每轮取帧组数，均值四舍五入作预期帧数 N_exp，
 * 记录 min / max / mean；容差只给建议，操作员确认后才写入。标准件轮次另用于胶偏参考 x_ref。
 * 多段计划：每个标定轮次都在采集页确认了手动分段时逐段标定（ΣN_exp 取各段之和）；否则各段保持“待标定”，ΣN_exp 取整件均值，凭它即可发布。
 */
export default function CalibrationPage() {
  const { data, doc, busy, dirty, frameDirty, act, setError } = useWorkspace();
  const [available, setAvailable] = useState<CaptureSummary[]>([]);
  // 键入但还没确认的容差，按段序号缓存；只在“配方 + 标定结果”不变时有效（见 resultKey）
  const [typed, setTyped] = useState<{ key: string; values: Record<number, number> }>({ key: "", values: {} });
  const plan = data?.plan ?? null;
  const calibration = plan?.calibration ?? null;
  // 换了配方，或标定结果（整件统计、各段类型 / 帧数统计 / 建议容差）变了，旧的键入值不再对应这一段：不沿用，输入框回到建议值
  const resultKey = JSON.stringify([doc?.id ?? null, calibration?.part ?? null, calibration?.segments.map(segment => [segment.index, segment.kind, segment.outcome]) ?? null]);
  const tolerances = typed.key === resultKey ? typed.values : {};
  const addedKey = (calibration?.rounds ?? []).map(round => round.roundId).join(",");
  useEffect(() => {
    if (!doc) return;
    let alive = true;
    void workspaceApi.captureList(doc.id).then(list => { if (alive) setAvailable(list.filter(round => round.state === "complete")); }).catch(e => { if (alive) setError(errorText(e)); });
    return () => { alive = false; };
  }, [doc?.id, addedKey]);
  if (!data || !doc || !plan) return <WorkspaceEmpty />;

  const minRounds = calibration?.minRounds ?? plan.calibrationRounds;
  const added = calibration?.rounds ?? [];
  const normal = added.filter(round => !round.standard);
  const standard = added.filter(round => round.standard);
  const short = Math.max(0, minRounds - normal.length);
  const locked = busy || dirty || frameDirty || !!data.workspace.pending;
  const candidates = available.filter(round => !added.some(item => item.roundId === round.roundId));
  const expectedTotal = calibration?.expectedTotal ?? plan.expectedTotal;
  const toleranceTotal = calibration?.toleranceTotal ?? plan.toleranceTotal;
  const multi = plan.segments.length > 1;
  const needsReference = plan.segments.some(segment => segment.kind === "straight" && segment.checks.offset?.enabled);
  const applyReason = short > 0 ? `还差 ${short} 轮：至少 ${minRounds} 轮才能标定` : locked ? "正在处理或候选待生效，请稍候" : "";
  const run = (request: () => ReturnType<typeof workspaceApi.calibrationApply>, message: string) => act(request, message);
  const add = (roundId: string, isStandard: boolean) => void run(() => workspaceApi.calibrationAddRound(doc.id, data.workspace.revision, roundId, isStandard), isStandard ? "已加入标定（标准件轮次）" : "已加入多轮标定");
  const remove = (roundId: string) => void run(() => workspaceApi.calibrationRemoveRound(doc.id, data.workspace.revision, roundId), "已移出标定，请重新应用标定");
  // 重新应用标定：之前键入但没确认的容差不再对应新的统计，一并丢弃，输入框回到新的建议值
  const apply = () => void run(() => workspaceApi.calibrationApply(doc.id, data.workspace.revision), "标定已应用：预期帧数已写入，容差仍待确认").then(next => { if (next) setTyped({ key: "", values: {} }); });
  const confirm = (segment: SegmentCalibView) => {
    const value = tolerances[segment.index] ?? segment.outcome?.suggestedTolerance ?? segment.tolerance;
    if (!Number.isInteger(value) || value < 0) { setError("帧数容差需为不小于 0 的整数"); return; }
    void run(() => workspaceApi.confirmTolerance(doc.id, data.workspace.revision, segment.index, value), `段 ${segment.index} 的帧数容差已确认为 ±${value}`);
  };
  const columns: TableColumn<SegmentCalibView>[] = [
    { key: "segment", header: "段", render: row => `段 ${row.index} · ${segmentKindLabel[row.kind]}` },
    { key: "nexp", header: "预期帧数 N_exp", align: "right", render: row => row.outcome ? row.outcome.nExp : <span className="c-warn">待标定{row.pending ? `（${row.pending}）` : ""}</span> },
    { key: "range", header: "各轮 min – max（均值）", align: "right", render: row => row.outcome ? `${row.outcome.calib.min} – ${row.outcome.calib.max}（${row.outcome.calib.mean.toFixed(1)}）` : "—" },
    { key: "tolerance", header: "容差", render: row => {
      const value = tolerances[row.index] ?? row.outcome?.suggestedTolerance ?? row.tolerance;
      return <span className="wp-calib-tol"><NumberField label={`段 ${row.index} 帧数容差`} value={value} min={0} step={1} unit="帧" disabled={locked} onChange={v => setTyped(current => ({ key: resultKey, values: { ...(current.key === resultKey ? current.values : {}), [row.index]: v } }))}/>
        <Button size="sm" aria-label={`确认段 ${row.index} 容差`} disabled={locked} onClick={() => confirm(row)}>确认容差</Button>
        <span className="muted">当前 ±{row.tolerance}{row.outcome ? `，建议 ±${row.outcome.suggestedTolerance}` : ""}</span></span>;
    } },
    { key: "xref", header: "胶偏参考 x_ref", render: row => row.kind === "corner" ? "不检胶偏" : row.xRef ? `${row.xRef.xRef.toFixed(3)} mm（离散 ${row.xRef.spread.toFixed(3)}）` : <span className="muted">{row.xRefPending ?? "—"}</span> },
  ];
  const hinted = calibration?.segments.filter(segment => segment.hints?.length) ?? [];
  const roundMeta = (round: CalibrationRoundView) => `${round.groupCount} 个帧组 · 各相机 ${round.cameraFrames.join(" / ")} 帧 · 不完整帧组 ${round.incompleteGroups} · 丢帧告警 ${round.lossAlarms}`;
  return <div className="wp-page"><WorkspaceBar />
    <Panel title="多轮标定" detail="同一工件程序连续采集多轮，取帧组数均值作预期帧数；容差由操作员确认">
      <CameraSegmentOverview cameras={plan.cameras} segments={plan.segments} />
      <div className="wp-actions" style={{ marginTop: 12 }}>
        <Badge tone={expectedTotal != null ? "ok" : "warn"}>{expectedTotal != null ? `预期总数 ${expectedText(expectedTotal, toleranceTotal)}` : "预期总数待标定"}</Badge>
        <Link className="wp-text-link" to="/recipe/segments">段列表</Link>
      </div>
    </Panel>
    {multi && <Notice title="多段计划按手动分段逐段标定">每个标定轮次（不含标准件）都在采集轮次页确认分段后，应用标定按边界得出各段预期帧数与建议容差，ΣN_exp 取各段之和；有轮次未确认分段时各段保持“待标定”，ΣN_exp 取各轮整件帧组数的均值。生产仍按整件核对帧数、不分段判定。</Notice>}
    <Panel title="标定轮次" detail={`同一工件程序连续采集，至少 ${minRounds} 轮；标准件轮次另用于胶偏参考，不计入轮数`}>
      <div className="wp-calib-need">
        <Badge tone={short > 0 ? "warn" : "ok"}>{short > 0 ? `已加入 ${normal.length} / ${minRounds} 轮，还差 ${short} 轮` : `已加入 ${normal.length} 轮，轮数足够`}</Badge>
        <span className="muted">标准件轮次 {standard.length} 轮</span>
        <CalibrationRoundsField />
        <Button variant="filled" data-variant="filled" disabled={!!applyReason} title={applyReason || "按已加入的轮次计算预期帧数与建议容差"} onClick={apply}>应用标定</Button>
        {applyReason && <span className="muted" role="note">{applyReason}</span>}
      </div>
      {needsReference && !standard.length && <p className="muted hint">有直线段要判胶偏，但还没有标准件轮次：胶偏参考 x_ref 无法算出。</p>}
      {added.length > 0 && <ul className="wp-calib-rounds" aria-label="已加入的轮次">{added.map(round => <li key={round.roundId} className="wp-calib-round">
        <span className="mono">{round.roundId}</span><Badge tone={round.standard ? "info" : "neutral"}>{round.standard ? "标准件" : "标定"}</Badge><span className="wp-calib-meta">{roundMeta(round)}</span>
        <div className="wp-actions"><Button size="sm" aria-label={`移出轮次 ${round.roundId}`} disabled={locked} onClick={() => remove(round.roundId)}>移出</Button></div>
      </li>)}</ul>}
      <h3 className="wp-calib-sub">可加入的采集轮次</h3>
      {candidates.length ? <ul className="wp-calib-rounds" aria-label="可加入的采集轮次">{candidates.map(round => <li key={round.roundId} className="wp-calib-round">
        <span className="mono">{round.roundId}</span><span className="wp-calib-meta">{new Date(round.createdAt).toLocaleString("zh-CN")} · 已接收 {round.receivedCount} 帧组</span>
        <div className="wp-actions"><Button size="sm" aria-label={`加入标定 ${round.roundId}`} disabled={locked} onClick={() => add(round.roundId, false)}>加入标定</Button><Button size="sm" aria-label={`设为标准件 ${round.roundId}`} disabled={locked} onClick={() => add(round.roundId, true)}>设为标准件轮次</Button></div>
      </li>)}</ul> : <p className="wp-capture-empty">没有可加入的完整采集轮次。到“采集轮次”导入多轮后回来。<Link className="wp-text-link" to="/recipe/capture">前往采集轮次<ArrowRight size={12} aria-hidden="true"/></Link></p>}
    </Panel>
    <Panel title="标定结果" detail="预期帧数写入后，容差仍是原值，需逐段确认">
      {calibration?.part ? <>
        <div className="wp-calib-part" role="group" aria-label="整件标定">
          <Badge tone="ok">整件预期 {calibration.part.nExp} 帧组</Badge>
          <span className="wp-capture-chip"><small>各轮 min – max（均值）</small><strong>{calibration.part.calib.min} – {calibration.part.calib.max}（{calibration.part.calib.mean.toFixed(1)}）</strong></span>
          <span className="wp-capture-chip"><small>轮数</small><strong>{calibration.part.calib.rounds}</strong></span>
          <span className="wp-capture-chip"><small>建议容差</small><strong>±{calibration.part.suggestedTolerance}</strong></span>
        </div>
        {toleranceTotal < calibration.part.suggestedTolerance && <p className="muted hint">当前 Σ容差 ±{toleranceTotal} 小于整件建议容差 ±{calibration.part.suggestedTolerance}：运行时帧组数可能越界告警（只告警，不影响结论）。</p>}
        <Table className="wp-calib-table" caption="逐段标定结果" columns={columns} rows={calibration.segments} rowKey={row => String(row.index)} />
        {hinted.length > 0 && <Notice title="标定提示" tone="warn">
          <ul className="wp-calib-hints" aria-label="按段的标定提示">{hinted.flatMap(segment => segment.hints.map((hint, i) => <li key={`${segment.index}-${i}`}>段 {segment.index} · {segmentKindLabel[segment.kind]}：{hint}</li>))}</ul>
        </Notice>}
      </> : <p className="wp-capture-empty">还没有标定结果。加入足够的轮次后点“应用标定”。</p>}
    </Panel>
    {added.length > 0 && <CalibrationFrames data={data} plan={plan} />}
  </div>;
}
