import { Alert, Button, EmptyState, StatusBadge, Table, type TableColumn } from "qing-steel";
import type { Recipe } from "../../cycle/types";
import { Panel } from "../../workspace/components";
import type { RecordImages } from "../../workspace/types";
import type { PartDetail } from "../types";
import { referenceBadge } from "../../cycle/vis";

const deliveryLabel = { notRequired: "无需交付", pending: "等待提交", submitted: "已提交，等待 PLC 确认", acknowledged: "PLC 已确认", failed: "交付失败" };
const recordingLabel = { pending: "正在落盘", off: "录制关闭", notRetained: "本件按保留策略不存图", complete: "完整落盘", incomplete: "原图不完整", failed: "录制失败" };
const statusLabel = { waiting: "未到达", measuring: "未完成", done: "测量完成", error: "测量出错", missing: "未收到计划帧" };

type Shot = PartDetail["shots"][number];

export default function PartTrace({ detail, layout, raw, selected, selectedView, onSelect }: {
  detail: PartDetail;
  layout: Recipe | null;
  raw: RecordImages | null;
  selected: number;
  selectedView: number;
  onSelect: (k: number, view: number) => void;
}) {
  const { summary, recording } = detail;
  const sampleImages = raw?.imageSource === "offlineSample";
  const reference = (shot: Shot) => referenceBadge(detail.frames[shot.k]?.referenceFailed, (detail.frames[shot.k]?.referenceFailed?.length ?? 0) > 1);
  const columns: TableColumn<Shot>[] = [
    {
      key: "shot",
      header: "拍照点 / Pose",
      render: shot => <>
        <Button size="sm" variant={selected === shot.k ? "filled" : "tonal"} aria-label={`追溯拍照点 ${shot.shotId}`} aria-pressed={selected === shot.k} onClick={() => onSelect(shot.k, shot.view)}>k{shot.k + 1} · {shot.shotId}</Button>
        <div className="muted">{layout?.shots[shot.k]?.poseId ?? "—"}</div>
      </>,
    },
    { key: "camera", header: "设备 / 检测视角", render: shot => `${shot.camera} / 视角 ${shot.view}` },
    { key: "session", header: "会话 / 序号", render: shot => `${shot.session ?? "—"} / ${shot.ordinal ?? "—"}` },
    { key: "counter", header: "帧 / 触发计数", render: shot => `${shot.frameCounter ?? "—"} / ${shot.triggerCounter ?? "—"}` },
    { key: "score", header: "分数 / 耗时", render: shot => `${shot.score?.toFixed(3) ?? "—"} / ${shot.ms ?? "—"} ms` },
    {
      key: "files",
      header: "原图",
      render: shot => {
        const files = sampleImages ? raw.frames.filter(frame => frame.k === shot.k) : shot.rawFiles;
        return files.length ? files.map(file => {
          const frame = raw?.frames.find(frame => frame.k === shot.k && frame.view === file.view);
          return <Button key={file.view} size="sm" disabled={!frame?.available}
            title={frame?.error ?? file.file} aria-label={`查看 k${shot.k + 1} 视角 ${file.view}`} aria-pressed={selected === shot.k && selectedView === file.view}
            variant={selected === shot.k && selectedView === file.view ? "filled" : "tonal"}
            onClick={() => onSelect(shot.k, file.view)}>视角 {file.view}{frame?.available ? "" : " · 不可用"}</Button>;
        }) : <span className="muted">{sampleImages ? "样本不可用" : "未保留"}</span>;
      },
    },
    {
      key: "status",
      header: "状态与原因",
      render: shot => <>
        <StatusBadge tone={shot.status === "done" ? "ok" : "ng"}>{statusLabel[shot.status]}</StatusBadge>
        {reference(shot) && <StatusBadge tone="fault">{reference(shot)}</StatusBadge>}
        {shot.error && <div>{shot.error}</div>}
      </>,
    },
  ];
  return <Panel title="逐拍照点追溯" className="frames-panel" detail="检测结论、PLC 交付和原图保留分别记录">
    <div className="kv2">
      <span>工件身份</span><b className="mono">{summary.cycleId ?? "未分配"}</b>
      <span>发布包</span><b className="mono">{summary.bundleId ?? "未使用图像发布包"}</b>
      <span>PLC 交付</span><b>{deliveryLabel[summary.delivery.state]}{summary.delivery.message && ` · ${summary.delivery.message}`}</b>
      <span>原图来源</span><b>{sampleImages ? "离线样本原图" : "采集录制原图"}</b>
      <span>原图保留</span><b>{sampleImages ? raw?.complete ? "受管样本完整可用" : "受管样本不完整或不可用" : <>{recordingLabel[recording.state]}{recording.available ? " · 已核验落盘" : ""}</>}</b>
    </div>
    {!!recording.errors.length && <Alert tone="warn">{recording.errors.join("；")}</Alert>}
    <Table<Shot>
      caption="逐拍照点追溯"
      rows={detail.shots}
      columns={columns}
      rowKey={shot => String(shot.k)}
      emptyState={<EmptyState title="本件没有可关联的拍照计划" />}
    />
  </Panel>;
}
