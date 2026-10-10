import type { Recipe } from "../../cycle/types";
import { Panel } from "../../workspace/components";
import type { RecordImages } from "../../workspace/types";
import type { PartDetail } from "../types";

const deliveryLabel = { notRequired: "无需交付", pending: "等待提交", submitted: "已提交，等待 PLC 确认", acknowledged: "PLC 已确认", failed: "交付失败" };
const recordingLabel = { pending: "正在落盘", off: "录制关闭", notRetained: "本件按保留策略不存图", complete: "完整落盘", incomplete: "原图不完整", failed: "录制失败" };
const statusLabel = { waiting: "未到达", measuring: "未完成", done: "测量完成", locateFailed: "定位失败", error: "测量出错", missing: "未收到计划帧" };

export default function PartTrace({ detail, layout, raw, selected, selectedView, onSelect }: {
  detail: PartDetail;
  layout: Recipe | null;
  raw: RecordImages | null;
  selected: number;
  selectedView: number;
  onSelect: (k: number, view: number) => void;
}) {
  const { summary, recording } = detail;
  return <Panel title="逐拍照点追溯" className="frames-panel" detail="检测结论、PLC 交付和原图保留分别记录">
    <div className="kv2">
      <span>工件身份</span><b className="mono">{summary.cycleId ?? "未分配"}</b>
      <span>发布包</span><b className="mono">{summary.bundleId ?? "未使用图像发布包"}</b>
      <span>PLC 交付</span><b>{deliveryLabel[summary.delivery.state]}{summary.delivery.message && ` · ${summary.delivery.message}`}</b>
      <span>原图保留</span><b>{recordingLabel[recording.state]}{recording.available ? " · 已核验落盘" : ""}</b>
    </div>
    {!!recording.errors.length && <div className="notice error" role="status">{recording.errors.join("；")}</div>}
    <div className="table-wrap"><table className="table"><thead><tr>
      <th>拍照点 / Pose</th><th>设备 / 检测视角</th><th>会话 / 序号</th><th>帧 / 触发计数</th><th>分数 / 耗时</th><th>原图</th><th>状态与原因</th>
    </tr></thead><tbody>{detail.shots.map(shot => <tr key={shot.k} aria-selected={selected === shot.k}>
      <td><button className="btn small" aria-label={`追溯拍照点 ${shot.shotId}`} aria-pressed={selected === shot.k} onClick={() => onSelect(shot.k, shot.view)}>k{shot.k + 1} · {shot.shotId}</button><div className="muted">{layout?.shots[shot.k]?.poseId ?? "—"}</div></td>
      <td>{shot.camera} / 视角 {shot.view}</td>
      <td>{shot.session ?? "—"} / {shot.ordinal ?? "—"}</td>
      <td>{shot.frameCounter ?? "—"} / {shot.triggerCounter ?? "—"}</td>
      <td>{shot.score?.toFixed(3) ?? "—"} / {shot.ms ?? "—"} ms</td>
      <td>{shot.rawFiles.length ? shot.rawFiles.map(file => {
        const frame = raw?.frames.find(frame => frame.k === shot.k && frame.view === file.view);
        return <button key={file.view} className="btn small" disabled={!frame?.available}
          title={frame?.error ?? file.file} aria-label={`查看 k${shot.k + 1} 视角 ${file.view}`} aria-pressed={selected === shot.k && selectedView === file.view}
          onClick={() => onSelect(shot.k, file.view)}>视角 {file.view}{frame?.available ? "" : " · 不可用"}</button>;
      }) : <span className="muted">未保留</span>}</td>
      <td className={shot.status === "done" ? "c-ok" : "c-err"}>{statusLabel[shot.status]}{shot.error && <div>{shot.error}</div>}</td>
    </tr>)}{!detail.shots.length && <tr><td colSpan={7} className="muted center">本件没有可关联的拍照计划</td></tr>}</tbody></table></div>
  </Panel>;
}
