import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Checkbox, EmptyState, Select, StatusBadge, Table, type TableColumn } from "qing-steel";
import { Panel } from "../../workspace/components";
import type { FrameRecord, FrameState, PartDetailBurst } from "../../plan/types";
import { expectedText, productionSkippedRegion, regionKindLabel } from "../../plan/vis";
import { verdictLabel, verdictTone } from "../meta";
import { focusInHistory } from "../focus";
import type { PartDetail } from "../types";
import "../../cycle/components/burst.css";
import MixSummary from "../../plan/MixSummary";
import { MeasurementEvidenceSummary, measurementIssue } from "../../plan/measurementEvidence";

const deliveryLabel = { notRequired: "无需交付", pending: "等待提交", submitted: "已回写 PLC", acknowledged: "PLC 已确认", failed: "交付失败" };
const recordingLabel = { pending: "正在落盘", off: "录制关闭", notRetained: "本件按保留策略不存图", complete: "完整落盘", incomplete: "原图不完整", failed: "录制失败" };

const frameStateText: Record<FrameState, string> = { measured: "已测", missing: "缺帧", error: "测量失败", unmeasured: "未测", outOfCorridor: "出走廊", notInspected: "本段不检测" };
const mm = (v: number | null | undefined) => v == null ? "—" : v.toFixed(2);
type TraceFrame = Pick<FrameRecord, "camera" | "seq">;
const frameKey = ({ camera, seq }: TraceFrame) => `${camera}:${seq}`;

function findFrameIndex(rows: readonly FrameRecord[], { camera, seq }: TraceFrame) {
  let low = 0;
  let high = rows.length;
  while (low < high) {
    const mid = low + Math.floor((high - low) / 2);
    const frame = rows[mid];
    if (frame.camera < camera || (frame.camera === camera && frame.seq < seq)) low = mid + 1;
    else high = mid;
  }
  const frame = rows[low];
  return frame?.camera === camera && frame.seq === seq ? low : -1;
}

function hasFrameIssue({ status, metrics }: FrameRecord) {
  return (status !== "measured" && status !== "notInspected") || !!measurementIssue(metrics?.measurementEvidence)
    || !!metrics?.mixEvidence?.directions.length || !!metrics?.mixEvidence?.incomplete;
}

/**
 * 逐帧追溯：每台相机每个帧组一行（缺帧也有），写帧号、触发计数、所在区域、状态、胶宽与胶中心、原图。
 * 原图按 (相机, seq) 选，没保存的帧（缺帧、录制队满丢弃、未录制）按钮禁用。
 */
type TraceProps = {
  detail: PartDetail;
  burst: PartDetailBurst;
  selected: TraceFrame | null;
  onSelect: (camera: number, seq: number, revealImage?: boolean) => void;
  focusFrame?: TraceFrame;
};

export function BurstPartTrace(props: TraceProps) {
  const select = useRef(props.onSelect);
  select.current = props.onSelect;
  const onSelect = useCallback((camera: number, seq: number, revealImage?: boolean) => select.current(camera, seq, revealImage), []);
  return <TraceTable {...props} onSelect={onSelect} />;
}

const TraceTable = memo(function TraceTable({ detail, burst, selected, onSelect, focusFrame }: TraceProps) {
  const { summary, recording } = detail;
  const [onlyAbnormal, setOnlyAbnormal] = useState(false);
  const [camera, setCamera] = useState("all");
  const [page, setPage] = useState(0), [pageSize, setPageSize] = useState(50);
  const verdicts = useMemo(() => new Map(burst.frameVerdicts.map(frame => [frameKey(frame), frame.verdict])), [burst.frameVerdicts]);
  const verdictFor = useCallback((frame: FrameRecord) => frame.status === "measured" ? verdicts.get(frameKey(frame)) : undefined, [verdicts]);
  const abnormalFrames = useMemo(() => burst.frames.filter(frame => {
    const verdict = verdictFor(frame);
    return (verdict !== undefined && verdict !== "OK") || hasFrameIssue(frame);
  }), [burst.frames, verdictFor]);
  const rows = useMemo(() => {
    const frames = onlyAbnormal ? abnormalFrames : burst.frames;
    return camera === "all" ? frames : frames.filter(frame => frame.camera === Number(camera));
  }, [abnormalFrames, burst.frames, onlyAbnormal, camera]);
  const pages = Math.max(1, Math.ceil(rows.length / pageSize));
  const shownPage = Math.min(page, pages - 1);
  const traceTools = useRef<HTMLDivElement>(null);
  const focusedFrame = useRef<TraceFrame | null>(null);
  useEffect(() => {
    const index = selected ? findFrameIndex(rows, selected) : -1;
    setPage(index < 0 ? 0 : Math.floor(index / pageSize));
  }, [selected?.camera, selected?.seq, rows, pageSize]);
  useLayoutEffect(() => {
    const tools = traceTools.current;
    if (!tools || !focusFrame || focusedFrame.current === focusFrame) return;
    const index = findFrameIndex(rows, focusFrame);
    if (index >= 0) {
      const targetPage = Math.floor(index / pageSize);
      if (shownPage !== targetPage) { setPage(targetPage); return; }
    }
    focusedFrame.current = focusFrame;
    const key = frameKey(focusFrame);
    const panel = tools.closest(".burst-trace");
    const image = panel?.querySelector<HTMLButtonElement>(`[data-trace-image="${key}"]`);
    const row = panel?.querySelector<HTMLButtonElement>(`[data-trace-frame="${key}"]`);
    focusInHistory(image ?? row ?? tools);
  }, [focusFrame, rows, shownPage, pageSize]);
  const columns: TableColumn<FrameRecord>[] = [
    {
      key: "frame",
      header: "相机 / 帧组",
      render: frame => <Button size="sm" variant={selected?.camera === frame.camera && selected.seq === frame.seq ? "filled" : "tonal"} aria-label={`追溯 ${burst.cameras[frame.camera] ?? `相机 ${frame.camera + 1}`} 帧组 ${frame.seq}`}
        data-trace-frame={frameKey(frame)} aria-pressed={selected?.camera === frame.camera && selected.seq === frame.seq} onClick={() => onSelect(frame.camera, frame.seq)}>{burst.cameras[frame.camera] ?? `相机 ${frame.camera + 1}`} · #{frame.seq}</Button>,
    },
    { key: "counter", header: "帧号 / 触发计数", render: frame => `${frame.frameCounter ?? "—"} / ${frame.triggerCounter ?? "—"}` },
    { key: "region", header: "区域", render: frame => frame.region != null && burst.judgement.regions[frame.region] ? regionKindLabel(burst.judgement.regions[frame.region].region.kind) : "—" },
    { key: "status", header: "状态 / 单帧结论", render: frame => {
      const skipped = !!productionSkippedRegion(burst.judgement.regions, frame.seq);
      const verdict = verdictFor(frame);
      const stateTone = frame.status === "error" ? "fault" : frame.status === "measured" || frame.status === "notInspected" ? "neutral" : "warn";
      return <>
        {skipped
          ? <><StatusBadge tone="neutral">生产跳过</StatusBadge><div className="muted">仅供诊断：{frameStateText[frame.status]}</div></>
          : verdict ? <StatusBadge tone={verdictTone(verdict)}>{verdictLabel[verdict]}</StatusBadge>
            : <StatusBadge tone={stateTone}>{frame.status === "measured" ? "已测 · 未判定" : frameStateText[frame.status]}</StatusBadge>}
        {frame.metrics?.error && <div>{frame.metrics.error}</div>}
        {frame.metrics?.mixEvidence && <MixSummary evidence={frame.metrics.mixEvidence} diagnostic={skipped}/>}
        {frame.metrics?.measurementEvidence && <div><MeasurementEvidenceSummary evidence={frame.metrics.measurementEvidence} /></div>}
      </>;
    } },
    { key: "width", header: "胶宽 min / max / mean", render: frame => frame.metrics && frame.status === "measured" ? `${mm(frame.metrics.widthMin)} / ${mm(frame.metrics.widthMax)} / ${mm(frame.metrics.widthMean)}` : "—" },
    { key: "x", header: "胶中心 x（mm）", align: "right", render: frame => frame.status === "measured" ? mm(frame.metrics?.xMm) : "—" },
    {
      key: "raw",
      header: "原图",
      render: frame => frame.rawFile
        ? <Button size="sm" aria-label={`查看 ${burst.cameras[frame.camera] ?? `相机 ${frame.camera + 1}`} 帧组 ${frame.seq} 原图`} title={frame.rawFile} variant={selected?.camera === frame.camera && selected.seq === frame.seq ? "filled" : "tonal"} data-trace-image={frameKey(frame)} onClick={() => onSelect(frame.camera, frame.seq, true)}>查看</Button>
        : <span className="muted">{frame.status === "missing" ? "缺帧" : "未保留"}</span>,
    },
  ];
  return <Panel title="逐帧追溯" className="frames-panel burst-trace" help="每台相机每个帧组一行。选择一帧可查看该相机的原始记录与已保留的原图。">
    <div className="kv2">
      <span>工件身份</span><b className="mono">{summary.cycleId ?? "未分配"}</b>
      <span>发布包</span><b className="mono">{summary.bundleId ?? "未使用图像发布包"}</b>
      <span>PLC 交付</span><b>{deliveryLabel[summary.delivery.state]}{summary.delivery.message && ` · ${summary.delivery.message}`}</b>
      <span>帧组 / 预期</span><b>{burst.judgement.groupCount} / {expectedText(burst.expectedTotal)}</b>
      <span>各相机帧数</span><b>{burst.cameras.map((name, c) => `${name} ${burst.cameraFrames[c] ?? 0}`).join(" · ")}</b>
      <span>触发计数（参考）</span><b>{burst.deviceTriggers ?? "—"}</b>
      <span>原图保留</span><b>{recordingLabel[recording.state]}{recording.available ? " · 已核验落盘" : ""}</b>
    </div>
    {!!recording.errors.length && <Alert tone="warn">{recording.errors.join("；")}</Alert>}
    <div className="burst-trace-tools" ref={traceTools} tabIndex={-1} role="group" aria-label="逐帧追溯筛选">
      <Select aria-label="相机筛选" value={camera} onValueChange={setCamera} options={[{ value: "all", label: "全部相机" }, ...burst.cameras.map((name, c) => ({ value: String(c), label: name }))]} />
      <Checkbox label={`只看异常帧（${abnormalFrames.length}）`} checked={onlyAbnormal} onCheckedChange={setOnlyAbnormal} />
      <span className="muted">{rows.length} / {burst.frames.length} 帧</span>
      <Select aria-label="每页帧数" value={String(pageSize)} onValueChange={value => setPageSize(Number(value))} options={[{ value: "50", label: "50 帧 / 页" }, { value: "100", label: "100 帧 / 页" }]} />
      <Button size="sm" disabled={shownPage === 0} onClick={() => setPage(shownPage - 1)}>上一页帧记录</Button>
      <span aria-live="polite">第 {shownPage + 1} / {pages} 页</span>
      <Button size="sm" disabled={shownPage >= pages - 1} onClick={() => setPage(shownPage + 1)}>下一页帧记录</Button>
    </div>
    <Table<FrameRecord>
      caption="逐帧追溯"
      rows={rows.slice(shownPage * pageSize, (shownPage + 1) * pageSize)}
      columns={columns}
      rowKey={frameKey}
      emptyState={<EmptyState title={onlyAbnormal ? "没有异常帧" : "本件没有帧记录"} />}
    />
  </Panel>;
}, (previous, next) => previous.detail === next.detail && previous.burst === next.burst && previous.selected?.camera === next.selected?.camera && previous.selected?.seq === next.selected?.seq && previous.onSelect === next.onSelect && previous.focusFrame === next.focusFrame);
