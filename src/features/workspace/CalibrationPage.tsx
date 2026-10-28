import { useState } from "react";
import { Link } from "react-router-dom";
import { ArrowRight } from "lucide-react";
import { Button, Table, type TableColumn } from "qing-steel";
import { CameraSegmentOverview } from "../plan/CameraSegmentOverview";
import type { PieceView, SegmentCalibView } from "../plan/types";
import { expectedText, segmentKindLabel } from "../plan/vis";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Notice, NumberField, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";
import type { CalibrationSyncResult, HistoryPick } from "./types";
import { CalibrationRoundsField } from "./DocParams";
import { CalibrationFrames } from "./CalibrationFrames";
import { SkipSuggest } from "./SkipSuggest";
import AlignReview from "./AlignReview";
import HistoryPicker from "./HistoryPicker";
import { PieceMask } from "./PieceMask";
import { pieceLabel } from "./pieces";
import "./corridor.css";
import "./capture.css";

const roleText = { teaching: "示教件", calibration: "标定件", standard: "标准件" } as const;

function reviewBadge(piece: PieceView) {
  if (piece.review === "confirmed") return <Badge tone="ok">{piece.confirmedBy === "aligned" ? "边界自动确认" : "边界已确认"}</Badge>;
  if (piece.role === "teaching") return <Badge tone="warn">边界待在分段确认</Badge>;
  return piece.review === "none" ? <Badge tone="neutral">未对齐</Badge> : <Badge tone="warn">{piece.review === "stale" ? "边界需复核" : "边界待复核"}</Badge>;
}

/**
 * 多轮标定（段配方，路由 /recipe/calibration）：示教件之外另选标定件凑足件数（RB-3），选件后软件导入、按示教件边界对齐，可信即加入标定，
 * 否则在本页逐边界复核后加入；帧组屏蔽、弃用、逐帧查看都在本页。每件取帧组数，均值四舍五入作预期帧数 N_exp，
 * 记录 min / max / mean；容差只给建议，操作员确认后才写入。标准件另用于胶偏参考 x_ref。
 */
export default function CalibrationPage() {
  const { data, doc, cameras, busy, dirty, frameDirty, act, setError } = useWorkspace();
  // 键入但还没确认的容差，按段序号缓存；只在“配方 + 标定结果”不变时有效（见 resultKey）
  const [typed, setTyped] = useState<{ key: string; values: Record<number, number> }>({ key: "", values: {} });
  const [picking, setPicking] = useState(false);
  const [synced, setSynced] = useState<CalibrationSyncResult["pieces"] | null>(null);
  const [open, setOpen] = useState<{ roundId: string; panel: "review" | "mask" } | null>(null);
  const plan = data?.plan ?? null;
  const calibration = plan?.calibration ?? null;
  // 换了配方，或标定结果（整件统计、各段类型 / 帧数统计 / 建议容差）变了，旧的键入值不再对应这一段：不沿用，输入框回到建议值
  const resultKey = JSON.stringify([doc?.id ?? null, calibration?.part ?? null, calibration?.segments.map(segment => [segment.index, segment.kind, segment.outcome]) ?? null]);
  const tolerances = typed.key === resultKey ? typed.values : {};
  if (!data || !doc || !plan) return <WorkspaceEmpty />;

  const minRounds = calibration?.minRounds ?? plan.calibrationRounds;
  const pieces = plan.pieces;
  const counted = pieces.filter(piece => piece.inCalibration && piece.role !== "standard");
  const standards = pieces.filter(piece => piece.role === "standard");
  const short = Math.max(0, minRounds - counted.length);
  const locked = busy || dirty || frameDirty || !!data.workspace.pending;
  const teaching = pieces.find(piece => piece.role === "teaching");
  const expectedTotal = calibration?.expectedTotal ?? plan.expectedTotal;
  const toleranceTotal = calibration?.toleranceTotal ?? plan.toleranceTotal;
  const multi = plan.segments.length > 1;
  const needsReference = plan.segments.some(segment => segment.kind === "straight" && segment.checks.offset?.enabled);
  const applyReason = short > 0 ? `还差 ${short} 件：至少 ${minRounds} 件才能标定` : locked ? "正在处理或候选待生效，请稍候" : "";
  const nameOf = (id: string) => cameras.find(camera => camera.id === id)?.name ?? id;
  const run = (request: () => ReturnType<typeof workspaceApi.calibrationApply>, message: string) => act(request, message);
  /** 选件、同步：结果按件列出已加入与待复核 */
  const syncWith = async (request: () => Promise<CalibrationSyncResult>) => {
    let result: CalibrationSyncResult["pieces"] = [];
    const next = await act(async () => {
      const response = await request();
      result = response.pieces;
      return response.view;
    }, () => `已加入标定 ${result.filter(item => item.added).length} 件，${result.filter(item => !item.added).length} 件未加入`);
    if (next) setSynced(result);
    return !!next;
  };
  const pick = async (picks: HistoryPick[]) => {
    const done = await syncWith(() => workspaceApi.calibrationPick(doc.id, data.workspace.revision, picks));
    if (done) setPicking(false);
    return done;
  };
  const sync = () => void syncWith(() => workspaceApi.calibrationSync(doc.id, data.workspace.revision));
  const remove = (piece: PieceView) => void run(() => workspaceApi.calibrationRemoveRound(doc.id, data.workspace.revision, piece.roundId), `已弃用 ${pieceLabel(piece)}，请重新应用标定`)
    .then(next => { if (next) { setSynced(null); setOpen(null); } });
  const toggle = (roundId: string, panel: "review" | "mask") => setOpen(current => current?.roundId === roundId && current.panel === panel ? null : { roundId, panel });
  const labelOf = (roundId: string) => { const piece = pieces.find(item => item.roundId === roundId); return piece ? pieceLabel(piece) : "未列出的件"; };
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
    { key: "range", header: "各件 min – max（均值）", align: "right", render: row => row.outcome ? `${row.outcome.calib.min} – ${row.outcome.calib.max}（${row.outcome.calib.mean.toFixed(1)}）` : "—" },
    { key: "tolerance", header: "容差", render: row => {
      const value = tolerances[row.index] ?? row.outcome?.suggestedTolerance ?? row.tolerance;
      return <span className="wp-calib-tol"><NumberField label={`段 ${row.index} 帧数容差`} value={value} min={0} step={1} unit="帧" disabled={locked} onChange={v => setTyped(current => ({ key: resultKey, values: { ...(current.key === resultKey ? current.values : {}), [row.index]: v } }))}/>
        <Button size="sm" aria-label={`确认段 ${row.index} 容差`} disabled={locked} onClick={() => confirm(row)}>确认容差</Button>
        <span className="muted">当前 ±{row.tolerance}{row.outcome ? `，建议 ±${row.outcome.suggestedTolerance}` : ""}</span></span>;
    } },
    { key: "xref", header: "胶偏参考 x_ref", render: row => row.kind === "corner" ? "不检胶偏" : row.xRef ? `${row.xRef.xRef.toFixed(3)} mm（离散 ${row.xRef.spread.toFixed(3)}）` : <span className="muted">{row.xRefPending ?? "—"}</span> },
  ];
  const hinted = calibration?.segments.filter(segment => segment.hints?.length) ?? [];
  return <div className="wp-page"><WorkspaceBar />
    <Panel title="多轮标定" help="示教件与另选的标定件各取帧组数，均值作预期帧数；容差由操作员确认">
      <CameraSegmentOverview cameras={plan.cameras} segments={plan.segments} />
      <div className="wp-actions" style={{ marginTop: 12 }}>
        <Badge tone={expectedTotal != null ? "ok" : "warn"}>{expectedTotal != null ? `预期总数 ${expectedText(expectedTotal, toleranceTotal)}` : "预期总数待标定"}</Badge>
        <Link className="wp-text-link" to="/recipe/segments">段列表</Link>
      </div>
    </Panel>
    {multi && <Notice title="多段计划按边界逐段标定">每个标定件（不含标准件）的边界都确认后，应用标定按边界得出各段预期帧数与建议容差，ΣN_exp 取各段之和；有标定件边界未确认时各段保持“待标定”，ΣN_exp 取各件整件帧组数的均值。生产仍按整件核对帧数、不分段判定。</Notice>}
    <Panel title="标定件" help="示教件在走廊示教完成后同步加入标定；其余标定件从历史件另选，选件后软件按示教件边界对齐，可信即加入标定，否则在这里逐边界复核">
      <div className="wp-calib-need">
        <Badge tone={short > 0 ? "warn" : "ok"}>标定需要 {minRounds} 件（含示教件，不含标准件），已加入 {counted.length} 件</Badge>
        <span className="muted">标准件 {standards.length} 件</span>
        <CalibrationRoundsField />
        <Button variant="filled" data-variant="filled" disabled={!!applyReason} title={applyReason || "按已加入的标定件计算预期帧数与建议容差"} onClick={apply}>应用标定</Button>
        {applyReason && <span className="muted" role="note">{applyReason}</span>}
      </div>
      <div className="wp-actions">
        <Button disabled={locked || !teaching} aria-expanded={picking} onClick={() => setPicking(value => !value)}>{picking ? "收起选件" : "选标定件"}</Button>
        <Button disabled={locked || !teaching} title="未确认边界的件重新对齐，已确认的测量并加入标定" onClick={sync}>同步标定件</Button>
        {!teaching && <span className="muted">还没有示教件：<Link className="wp-text-link" to="/recipe/capture">先选示教件<ArrowRight size={12} aria-hidden="true" /></Link></span>}
      </div>
      {needsReference && !standards.length && <p className="muted hint">有直线段要判胶偏，但还没有标准件：胶偏参考 x_ref 无法算出。</p>}
      {synced && synced.length > 0 && <Notice title="标定件同步结果" tone={synced.every(item => item.added) ? "ok" : "warn"}>
        <ul aria-label="标定件同步结果">{synced.map(item => <li key={item.roundId}>{labelOf(item.roundId)}：{item.added ? "已加入标定" : `未加入${item.issue ? `（${item.issue}）` : ""}`}</li>)}</ul>
      </Notice>}
      {pieces.length > 0 ? <ul className="wp-piece-list" aria-label="标定件">{pieces.map(piece => {
        const label = pieceLabel(piece);
        const opened = open?.roundId === piece.roundId ? open.panel : null;
        return <li key={piece.roundId} className={"wp-piece" + (opened ? " selected" : "")} aria-label={label}>
          <div className="wp-piece-head">
            <strong>{label}</strong>
            <Badge tone={piece.role === "teaching" ? "info" : "neutral"}>{roleText[piece.role]}</Badge>
            {reviewBadge(piece)}
            <Badge tone={piece.inCalibration ? "ok" : "neutral"}>{piece.inCalibration ? "已加入标定" : "未加入"}</Badge>
            <span className="muted">{piece.groupCount} 帧组{piece.excludedGroups > 0 && ` · 屏蔽 ${piece.excludedGroups} 组`}</span>
            {piece.role === "teaching" && !piece.inCalibration && <span className="muted">走廊示教完成后点“同步标定件”加入</span>}
            <div className="wp-actions">
              {piece.role !== "teaching" && <Button size="sm" aria-label={`复核边界 ${label}`} aria-expanded={opened === "review"} disabled={locked} onClick={() => toggle(piece.roundId, "review")}>{piece.review === "confirmed" ? "重新复核" : "复核边界"}</Button>}
              <Button size="sm" aria-label={`帧组屏蔽 ${label}`} aria-expanded={opened === "mask"} onClick={() => toggle(piece.roundId, "mask")}>帧组屏蔽</Button>
              {piece.role !== "teaching" && <Button size="sm" aria-label={`弃用 ${label}`} disabled={locked} onClick={() => remove(piece)}>弃用</Button>}
            </div>
          </div>
          {opened === "review" && <AlignReview plan={plan} piece={piece} locked={locked ? "正在处理或候选待生效，请稍候" : ""} onSynced={result => { setSynced(result); setOpen(null); }} />}
          {opened === "mask" && <PieceMask roundId={piece.roundId} groupCount={piece.groupCount} disabled={locked} inCalibration={piece.inCalibration} />}
        </li>;
      })}</ul> : <p className="wp-capture-empty">还没有标定件。先选示教件，再点“选标定件”从历史件里另选。</p>}
    </Panel>
    {picking && <HistoryPicker recipeId={doc.id} mode="multi" title="选标定件" disabled={locked} nameOf={nameOf} onError={setError}
      confirmLabel={count => count ? `加入选中的 ${count} 件` : "加入选中的历史件"} onConfirm={pick} />}
    <Panel title="标定结果" help="预期帧数写入后，容差仍是原值，需逐段确认">
      {calibration?.part ? <>
        <div className="wp-calib-part" role="group" aria-label="整件标定">
          <Badge tone="ok">整件预期 {calibration.part.nExp} 帧组</Badge>
          <span className="wp-capture-chip"><small>各件 min – max（均值）</small><strong>{calibration.part.calib.min} – {calibration.part.calib.max}（{calibration.part.calib.mean.toFixed(1)}）</strong></span>
          <span className="wp-capture-chip"><small>件数</small><strong>{calibration.part.calib.rounds}</strong></span>
          <span className="wp-capture-chip"><small>建议容差</small><strong>±{calibration.part.suggestedTolerance}</strong></span>
        </div>
        {toleranceTotal < calibration.part.suggestedTolerance && <p className="muted hint">当前 Σ容差 ±{toleranceTotal} 小于整件建议容差 ±{calibration.part.suggestedTolerance}：运行时帧组数可能越界告警（只告警，不影响结论）。</p>}
        <Table className="wp-calib-table" caption="逐段标定结果" columns={columns} rows={calibration.segments} rowKey={row => String(row.index)} />
        {hinted.length > 0 && <Notice title="标定提示" tone="warn">
          <ul className="wp-calib-hints" aria-label="按段的标定提示">{hinted.flatMap(segment => segment.hints.map((hint, i) => <li key={`${segment.index}-${i}`}>段 {segment.index} · {segmentKindLabel[segment.kind]}：{hint}</li>))}</ul>
        </Notice>}
      </> : <p className="wp-capture-empty">还没有标定结果。加入足够的标定件后点“应用标定”。</p>}
    </Panel>
    {counted.length > 0 && <SkipSuggest locked={locked} />}
    {(calibration?.rounds.length ?? 0) > 0 && <CalibrationFrames data={data} plan={plan} />}
  </div>;
}
