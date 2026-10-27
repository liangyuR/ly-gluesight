import { useState } from "react";
import { Alert, Button, Checkbox, EmptyState, Select, StatusBadge, Table, type TableColumn } from "qing-steel";
import { Panel } from "../../workspace/components";
import type { FrameRecord, FrameState, PartDetailBurst } from "../../plan/types";
import { expectedText, regionKindLabel } from "../../plan/vis";
import type { PartDetail } from "../types";
import "../../cycle/components/burst.css";

const deliveryLabel = { notRequired: "无需交付", pending: "等待提交", submitted: "已提交，等待 PLC 确认", acknowledged: "PLC 已确认", failed: "交付失败" };
const recordingLabel = { pending: "正在落盘", off: "录制关闭", notRetained: "本件按保留策略不存图", complete: "完整落盘", incomplete: "原图不完整", failed: "录制失败" };

const frameStateText: Record<FrameState, string> = { measured: "已测", missing: "缺帧", error: "测量失败", unmeasured: "未测", outOfCorridor: "出走廊", notInspected: "本段不检测" };
const mm = (v: number | null | undefined) => v == null ? "—" : v.toFixed(2);

/** 一帧没出完整结果：缺帧、测量失败、未测、走出走廊（都只算检测不完整，不影响 OK/NG）；本段不由这台相机检测的帧按设计不测，不算异常。 */
export const frameAbnormal = (frame: FrameRecord) => frame.status !== "measured" && frame.status !== "notInspected";

/**
 * 逐帧追溯：每台相机每个帧组一行（缺帧也有），写帧号、触发计数、所在区域、状态、胶宽与胶中心、原图。
 * 原图按 (相机, seq) 选，没保存的帧（缺帧、录制队满丢弃、未录制）按钮禁用。
 */
export function BurstPartTrace({ detail, burst, selected, onSelect }: {
  detail: PartDetail;
  burst: PartDetailBurst;
  selected: { camera: number; seq: number } | null;
  onSelect: (camera: number, seq: number) => void;
}) {
  const { summary, recording } = detail;
  const [onlyAbnormal, setOnlyAbnormal] = useState(false);
  const [camera, setCamera] = useState("all");
  const rows = burst.frames.filter(frame => (!onlyAbnormal || frameAbnormal(frame)) && (camera === "all" || frame.camera === Number(camera)));
  const abnormal = burst.frames.filter(frameAbnormal).length;
  const columns: TableColumn<FrameRecord>[] = [
    {
      key: "frame",
      header: "相机 / 帧组",
      render: frame => <Button size="sm" variant={selected?.camera === frame.camera && selected.seq === frame.seq ? "filled" : "tonal"} aria-label={`追溯 ${burst.cameras[frame.camera] ?? `相机 ${frame.camera + 1}`} 帧组 ${frame.seq}`}
        aria-pressed={selected?.camera === frame.camera && selected.seq === frame.seq} onClick={() => onSelect(frame.camera, frame.seq)}>{burst.cameras[frame.camera] ?? `相机 ${frame.camera + 1}`} · #{frame.seq}</Button>,
    },
    { key: "counter", header: "帧号 / 触发计数", render: frame => `${frame.frameCounter ?? "—"} / ${frame.triggerCounter ?? "—"}` },
    { key: "region", header: "区域", render: frame => frame.region != null && burst.judgement.regions[frame.region] ? regionKindLabel(burst.judgement.regions[frame.region].region.kind) : "—" },
    { key: "status", header: "状态", render: frame => <>
      <StatusBadge tone={frame.status === "measured" ? "ok" : frame.status === "error" ? "fault" : frame.status === "notInspected" ? "neutral" : "warn"}>{frameStateText[frame.status]}</StatusBadge>
      {frame.metrics?.error && <div>{frame.metrics.error}</div>}
    </> },
    { key: "width", header: "胶宽 min / max / mean", render: frame => frame.metrics && frame.status === "measured" ? `${mm(frame.metrics.widthMin)} / ${mm(frame.metrics.widthMax)} / ${mm(frame.metrics.widthMean)}` : "—" },
    { key: "x", header: "胶中心 x（mm）", align: "right", render: frame => frame.status === "measured" ? mm(frame.metrics?.xMm) : "—" },
    {
      key: "raw",
      header: "原图",
      render: frame => frame.rawFile
        ? <Button size="sm" aria-label={`查看 ${burst.cameras[frame.camera] ?? `相机 ${frame.camera + 1}`} 帧组 ${frame.seq} 原图`} title={frame.rawFile} variant={selected?.camera === frame.camera && selected.seq === frame.seq ? "filled" : "tonal"} onClick={() => onSelect(frame.camera, frame.seq)}>查看</Button>
        : <span className="muted">{frame.status === "missing" ? "缺帧" : "未保留"}</span>,
    },
  ];
  return <Panel title="逐帧追溯" className="frames-panel burst-trace" help="每台相机每个帧组一行；检测结论、PLC 交付和原图保留分别记录">
    <div className="kv2">
      <span>工件身份</span><b className="mono">{summary.cycleId ?? "未分配"}</b>
      <span>发布包</span><b className="mono">{summary.bundleId ?? "未使用图像发布包"}</b>
      <span>PLC 交付</span><b>{deliveryLabel[summary.delivery.state]}{summary.delivery.message && ` · ${summary.delivery.message}`}</b>
      <span>帧组 / 预期</span><b>{burst.judgement.groupCount} / {expectedText(burst.expectedTotal, burst.toleranceTotal)}</b>
      <span>各相机帧数</span><b>{burst.cameras.map((name, c) => `${name} ${burst.cameraFrames[c] ?? 0}`).join(" · ")}</b>
      <span>触发计数（参考）</span><b>{burst.deviceTriggers ?? "—"}</b>
      <span>原图保留</span><b>{recordingLabel[recording.state]}{recording.available ? " · 已核验落盘" : ""}</b>
    </div>
    {!!recording.errors.length && <Alert tone="warn">{recording.errors.join("；")}</Alert>}
    <div className="burst-trace-tools">
      <Select aria-label="相机筛选" value={camera} onValueChange={setCamera} options={[{ value: "all", label: "全部相机" }, ...burst.cameras.map((name, c) => ({ value: String(c), label: name }))]} />
      <Checkbox label={`只看异常帧（${abnormal}）`} checked={onlyAbnormal} onCheckedChange={setOnlyAbnormal} />
      <span className="muted">{rows.length} / {burst.frames.length} 帧</span>
    </div>
    <Table<FrameRecord>
      caption="逐帧追溯"
      rows={rows}
      columns={columns}
      rowKey={frame => `${frame.camera}:${frame.seq}`}
      emptyState={<EmptyState title={onlyAbnormal ? "没有异常帧" : "本件没有帧记录"} />}
    />
  </Panel>;
}
