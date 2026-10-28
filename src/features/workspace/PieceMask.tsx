import { useState } from "react";
import { Button, Select } from "qing-steel";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { NumberField } from "./components";
import "./capture.css";

const reasons = ["图像模糊或反光", "遮挡或污染", "图像不可用于标定"];

/**
 * 一件的异常帧组屏蔽与恢复（多轮标定页）：屏蔽整组所有相机图像，只排除图像标定测量；原始序号、时间位置和帧数统计保留。
 * 已加入标定的件改了屏蔽范围后要重新同步标定件。
 */
export function PieceMask({ roundId, groupCount, disabled, inCalibration }: { roundId: string; groupCount: number; disabled: boolean; inCalibration: boolean }) {
  const { data, act } = useWorkspace();
  const [start, setStart] = useState(1), [end, setEnd] = useState(1);
  const [reason, setReason] = useState(reasons[0]);
  if (!data) return null;
  const id = data.workspace.doc.id, revision = data.workspace.revision;
  const excluded = data.workspace.captureReview?.rounds[roundId]?.excludedGroups ?? [];
  const rangeValid = Number.isInteger(start) && Number.isInteger(end) && start >= 1 && end >= start && end <= groupCount;
  const exclude = (on: boolean) => void act(() => workspaceApi.captureReviewExclude(id, revision, roundId, start, end, on, on ? reason : ""),
    on ? "所选帧组已屏蔽，原始序号和帧数保留" : "所选帧组已恢复");
  return <div className="wp-stack">
    <p className="wp-review-counts">原始 {groupCount} 组 · 屏蔽 {excluded.length} 组 · 未屏蔽 {groupCount - excluded.length} 组</p>
    <p className="muted hint">屏蔽整组所有相机图像，只排除图像标定测量；原始序号、时间位置和帧数统计保留。轨迹异常、中途停止的件应整体弃用。{inCalibration && "改了屏蔽范围后点“同步标定件”重新加入标定。"}</p>
    <div className="wp-actions wp-review-mask">
      <NumberField label="屏蔽范围起始帧组" value={start} min={1} max={groupCount} disabled={disabled} onChange={setStart}/>
      <NumberField label="屏蔽范围结束帧组" value={end} min={1} max={groupCount} disabled={disabled} onChange={setEnd}/>
      <Select label="屏蔽原因" value={reason} disabled={disabled} onValueChange={setReason} options={reasons.map(value => ({ value, label: value }))}/>
      <Button disabled={disabled || !rangeValid} onClick={() => exclude(true)}>屏蔽所选范围</Button>
      <Button disabled={disabled || !rangeValid || !excluded.some(group => group.seq >= start && group.seq <= end)} onClick={() => exclude(false)}>恢复所选范围</Button>
    </div>
    {!rangeValid && <p className="c-warn" role="status">范围须为 1–{groupCount} 之间的整数，起始不得大于结束。</p>}
    {excluded.length > 0 && <div className="wp-review-excluded" aria-label="已屏蔽帧组">{excluded.map(group => <Button key={group.seq} size="sm" onClick={() => { setStart(group.seq); setEnd(group.seq); }}>帧组 {group.seq} · {group.reason}</Button>)}</div>}
  </div>;
}
