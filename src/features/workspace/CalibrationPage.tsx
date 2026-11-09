import { useState } from "react";
import { Link } from "react-router-dom";
import { ArrowRight } from "lucide-react";
import { Button, Table, type TableColumn } from "qing-steel";
import { CameraSegmentOverview } from "../plan/CameraSegmentOverview";
import type { SegmentCalibView } from "../plan/types";
import { expectedText, segmentKindLabel } from "../plan/vis";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Notice, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";
import { CalibrationFrames } from "./CalibrationFrames";
import { SkipSuggest } from "./SkipSuggest";
import { PassSkipSuggest } from "./PassSkipSuggest";
import { HistoryTrials } from "./HistoryTrials";
import { PieceMask } from "./PieceMask";
import { pieceLabel } from "./pieces";
import "./corridor.css";
import "./capture.css";
import { useRecipeHref } from "./recipeRoutes";
import { OperationStatus, useOperation } from "./operation";

/**
 * 试测（段配方，路由 /recipe/<编号>/calibration）：示教件是唯一的基准件，兼作标准件。各段 N_exp 在示教件分段确认后由后端写入；
 * 本页测量示教件得出胶偏参考 x_ref（单段配方的直线段），屏蔽示教件的异常帧组，并用历史件试测。
 * 走廊、当量或检测参数改了之后测量失效，需重新测量。
 */
export default function CalibrationPage() {
  const href = useRecipeHref();
  const operation = useOperation();
  const { data, doc, busy, dirty, frameDirty, segmentDirty, act } = useWorkspace();
  const [masking, setMasking] = useState(false);
  const plan = data?.plan ?? null;
  const calibration = plan?.calibration ?? null;
  if (!data || !doc || !plan) return <WorkspaceEmpty />;

  const piece = plan.pieces[0] ?? null;
  const locked = busy || dirty || frameDirty || segmentDirty || !!data.workspace.pending;
  const lockedReason = data.workspace.pending ? "候选版本正在等待生效" : busy ? "正在处理，请稍候" : dirty || frameDirty || segmentDirty ? "有未保存的改动，保存后再测量" : "";
  const taught = plan.cameraTeaching.length > 0 && plan.cameraTeaching.every(item => item.ready);
  const segmented = !!calibration && calibration.segments.length > 0 && calibration.segments.every(segment => segment.nExp != null);
  const measureReason = !piece ? "还没有示教件" : !segmented ? "示教件分段未确认：先在分段步骤确认" : !taught ? "走廊示教未完成：各相机要有像素当量、检测的每一段都要有走廊" : lockedReason;
  const expectedTotal = calibration?.expectedTotal ?? plan.expectedTotal;
  const measured = !!calibration?.measured;
  const stale = !measured && calibration?.roundId != null;
  const measure = () => void act(() => operation.run(taskId => workspaceApi.measureTeaching(doc.id, data.workspace.revision, taskId)), "示教件已按当前走廊测量");
  const columns: TableColumn<SegmentCalibView>[] = [
    { key: "segment", header: "段", render: row => `段 ${row.index} · ${segmentKindLabel[row.kind]}` },
    { key: "nexp", header: "示教帧数", align: "right", render: row => row.nExp ?? <span className="c-warn">示教件分段未确认</span> },
    { key: "xref", header: "胶偏参考 x_ref", render: row => row.kind === "corner" ? "不检胶偏" : row.xRef ? `${row.xRef.xRef.toFixed(3)} mm（离散 ${row.xRef.spread.toFixed(3)}）` : <span className="muted">{row.xRefPending ?? "—"}</span> },
  ];
  const hinted = calibration?.segments.filter(segment => segment.hints.length) ?? [];
  return <div className="wp-page"><WorkspaceBar />
    <Panel title="示教件基准" help="示教件是唯一的基准件，兼作标准件：各段示教帧数取示教件已确认分段的帧组数；测量示教件得出胶偏参考">
      <CameraSegmentOverview cameras={plan.cameras} segments={plan.segments} />
      <div className="wp-calib-need">
        <Badge tone={expectedTotal != null ? "ok" : "warn"}>{expectedTotal != null ? `示教总帧数 ${expectedText(expectedTotal)}` : "示教总帧数待示教件分段"}</Badge>
        {piece ? <span className="muted">示教件 {pieceLabel(piece)}</span>
          : <span className="muted">还没有示教件：<Link className="wp-text-link" to={href("/recipe/capture")}>先选示教件<ArrowRight size={12} aria-hidden="true" /></Link></span>}
        <Badge tone={measured ? "ok" : stale ? "warn" : "neutral"}>{measured ? "已按当前走廊测量" : stale ? "走廊或检测参数改过，需重新测量" : "还没测量"}</Badge>
        <Button variant={measured ? "tonal" : "filled"} disabled={!!measureReason} title={measureReason || "按当前走廊测量示教件的每一帧，得出胶偏参考"} onClick={measure}>测量示教件</Button>
        {measureReason && <span className="muted" role="note">{measureReason}</span>}
        <Link className="wp-text-link" to={href("/recipe/teach")}>分段与示教</Link>
      </div>
      {calibration && calibration.segments.length > 0 && <Table className="wp-calib-table" caption="逐段基准" columns={columns} rows={calibration.segments} rowKey={row => String(row.index)} />}
      <OperationStatus operation={operation} label="测量示教件" />
      {hinted.length > 0 && <Notice title="定向提示" tone="warn">
        <ul className="wp-calib-hints" aria-label="按段的定向提示">{hinted.flatMap(segment => segment.hints.map((hint, i) => <li key={`${segment.index}-${i}`}>段 {segment.index} · {segmentKindLabel[segment.kind]}：{hint}</li>))}</ul>
      </Notice>}
    </Panel>
    {piece && <Panel title="示教件帧组屏蔽" help="屏蔽示教件里模糊、反光或被遮挡的帧组，只排除图像测量；屏蔽后示教件的测量失效，需重新测量"
      actions={<Button size="sm" aria-expanded={masking} onClick={() => setMasking(value => !value)}>{masking ? "收起" : "帧组屏蔽"}</Button>}>
      <p className="muted">{piece.groupCount} 帧组{piece.excludedGroups > 0 && ` · 屏蔽 ${piece.excludedGroups} 组`}</p>
      {masking && <PieceMask roundId={piece.roundId} groupCount={piece.groupCount} disabled={locked} measured={piece.measured} />}
    </Panel>}
    {calibration?.roundId != null && <SkipSuggest locked={locked} />}
    {calibration?.roundId != null && <PassSkipSuggest locked={locked} />}
    {calibration?.roundId != null && <CalibrationFrames data={data} plan={plan} />}
    <HistoryTrials recipeId={doc.id} />
  </div>;
}
