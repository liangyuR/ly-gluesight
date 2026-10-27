import { useEffect, useState } from "react";
import { Button, Checkbox, Select } from "qing-steel";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Notice, NumberField, Panel } from "./components";
import type { PathSegment } from "../plan/types";
import type { WorkspaceView } from "./types";

const statusLabels = { draft: "草稿待确认", confirmed: "已确认", stale: "需复核" };

interface BoundaryDraft {
  cuts: number[];
  baseCuts: string;
  segmentSignature: string;
  captureSignature: string;
  segmentOrder: string;
}

function readDraft(key: string): BoundaryDraft | null {
  try {
    const value = JSON.parse(sessionStorage.getItem(key) ?? "null") as BoundaryDraft | null;
    return value && Array.isArray(value.cuts) && value.cuts.every(v => v === null || typeof v === "number")
      && [value.baseCuts, value.segmentSignature, value.captureSignature, value.segmentOrder].every(v => typeof v === "string")
      ? { ...value, cuts: value.cuts.map(v => v ?? NaN) } : null;
  } catch { return null; }
}

export default function CaptureReviewPanel({ roundId, groupCount, currentSeq, currentFrameReady, segments, disabled, inCalibration, onNavigate }: {
  roundId: string; groupCount: number; currentSeq: number; segments: PathSegment[];
  currentFrameReady: boolean; disabled: boolean; inCalibration: boolean; onNavigate: (seq: number) => void;
}) {
  const { data, act } = useWorkspace();
  const id = data!.workspace.doc.id, revision = data!.workspace.revision;
  const review = data!.workspace.captureReview?.rounds[roundId];
  const template = data!.workspace.captureReview?.template;
  const storageKey = `capture-review:${id}:${roundId}`;
  const [draft, setDraft] = useState<BoundaryDraft | null>(() => readDraft(storageKey));
  const [acceptStaleDraft, setAcceptStaleDraft] = useState(false);
  const [storageFailed, setStorageFailed] = useState(false);
  const [checked, setChecked] = useState(false);
  const [pending, setPending] = useState(false);
  const [mode, setMode] = useState<"proportional" | "original">("proportional");
  const [start, setStart] = useState(currentSeq), [end, setEnd] = useState(currentSeq);
  const [reason, setReason] = useState("图像模糊或反光");
  const savedCuts = review?.cuts ?? [];
  const cuts = draft?.cuts ?? savedCuts;
  const boundaryCount = Math.max(0, segments.length - 1);
  const invalid = !segments.length || groupCount < segments.length || cuts.length !== boundaryCount || cuts.some((cut, i) =>
    !Number.isInteger(cut) || cut < 1 || cut >= groupCount || (i > 0 && cut <= cuts[i - 1]));
  const edited = draft !== null;
  const locked = disabled || pending;
  const excluded = review?.excludedGroups ?? [];
  const currentExcluded = excluded.find(group => group.seq === currentSeq);
  const rangeValid = Number.isInteger(start) && Number.isInteger(end) && start >= 1 && end >= start && end <= groupCount;
  const signature = JSON.stringify(segments.map(segment => [segment.index, segment.kind]));
  const baseline = {
    baseCuts: JSON.stringify(savedCuts), segmentSignature: JSON.stringify(review?.segmentSignature ?? null),
    captureSignature: JSON.stringify(review?.captureSignature ?? null), segmentOrder: signature,
  };
  const baselineKey = JSON.stringify(baseline);
  const staleDraft = !!draft && Object.entries(baseline).some(([key, value]) => draft[key as keyof typeof baseline] !== value);
  useEffect(() => { setAcceptStaleDraft(false); }, [baselineKey]);
  useEffect(() => { setChecked(false); }, [roundId, signature, review?.status, JSON.stringify(savedCuts), edited]);
  useEffect(() => {
    if (!storageFailed || !edited) return;
    const guard = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ""; };
    window.addEventListener("beforeunload", guard);
    return () => window.removeEventListener("beforeunload", guard);
  }, [storageFailed, edited]);
  const remember = (next: BoundaryDraft | null) => {
    setDraft(next); setChecked(false); setAcceptStaleDraft(false);
    try {
      if (next === null) sessionStorage.removeItem(storageKey);
      else sessionStorage.setItem(storageKey, JSON.stringify(next));
      setStorageFailed(false);
    } catch { setStorageFailed(true); }
  };
  const setCut = (index: number, value: number) => {
    const next = Array.from({ length: boundaryCount }, (_, i) => cuts[i] ?? NaN);
    next[index] = value; remember({ ...(draft ?? baseline), cuts: next });
  };
  const run = async (request: () => Promise<WorkspaceView>, message: string, clearDraft = false) => {
    if (locked) return;
    setPending(true);
    try {
      const next = await act(request, message);
      if (next && clearDraft) remember(null);
      if (next) setChecked(false);
    } finally { setPending(false); }
  };
  return <Panel title="轮次整理" help="按原始帧组序号标记边界，所有相机共享分段；生产仍按未分段处理。"
    actions={<Badge tone={!edited && review?.status === "confirmed" ? "ok" : "warn"}>{edited ? "边界未保存" : review ? statusLabels[review.status] : "未分段"}</Badge>}>
    <p className="wp-review-counts">原始 {groupCount} 组 · 屏蔽 {excluded.length} 组 · 未屏蔽 {groupCount - excluded.length} 组</p>
    {currentExcluded && <Notice title={`当前帧组 ${currentSeq} 已屏蔽`} tone="warn">{currentExcluded.reason}。原图仍可查看，原始序号和帧组总数不变。</Notice>}
    {review?.status === "stale" && <Notice title="段序列或采集配置已变化，需要重新复核" tone="warn">核对各段范围，保存草稿后重新确认。</Notice>}
    {edited && <Notice title="有尚未保存的边界" tone="warn">{storageFailed ? "暂存失败，请保存后再离开本页。" : "已在当前窗口暂存，切换轮次或页面后可继续编辑；保存草稿后才会写入配方。"}</Notice>}
    {staleDraft && <Notice title="暂存边界基于旧版本，请核对后重新保存" tone="warn"><Checkbox label="已核对当前段序列及已保存边界，采用此暂存边界" checked={acceptStaleDraft} disabled={locked} onCheckedChange={setAcceptStaleDraft}/></Notice>}
    {inCalibration && <p className="c-warn">修改边界或屏蔽范围会将本轮移出标定，并使已有标定结果失效；整理后需重新加入标定。</p>}
    <div className="wp-review-template wp-actions">
      <span className="muted">{template ? `模板 v${template.version} · ${template.roundId} · ${template.groupCount} 组` : "尚未设置分段模板"}</span>
      <Select label="模板套用方式" value={mode} disabled={locked || edited || !template} onValueChange={value => setMode(value as typeof mode)} options={[
        { value: "proportional", label: "按总帧数比例" }, { value: "original", label: "保留模板帧号" },
      ]}/>
      <Button disabled={locked || edited || !template} onClick={() => void run(() => workspaceApi.captureReviewApplyTemplate(id, revision, roundId, mode), "模板已套用为草稿，请复核边界", true)}>套用模板</Button>
      <Button disabled={locked || edited || review?.status !== "confirmed"} onClick={() => void run(() => workspaceApi.captureReviewSetTemplate(id, revision, roundId), "已设为分段模板")}>设为模板</Button>
    </div>
    <p className="muted hint">套用仅生成待确认草稿。总帧数相同也可能存在边界偏移，请查看边界两侧图像后确认。</p>
    <div className="wp-review-segments" role="group" aria-label="分段边界">
      {segments.map((segment, index) => {
        const first = index === 0 ? 1 : cuts[index - 1] + 1;
        const last = index === segments.length - 1 ? groupCount : cuts[index];
        const cut = cuts[index];
        return <div className="wp-review-segment" key={segment.index}>
          <strong>第 {segment.index} 段 · {segment.kind === "straight" ? "直线" : "弯道"}</strong>
          <span className="muted">{Number.isInteger(first) ? first : "—"}–{Number.isInteger(last) ? last : "—"} 组</span>
          {index < boundaryCount && <>
            <NumberField label={`第 ${segment.index} 段结束帧组`} value={cut ?? null} min={1} max={groupCount - 1} disabled={locked} onChange={value => setCut(index, value)}/>
            <Button size="sm" disabled={locked || !currentFrameReady || currentSeq >= groupCount} onClick={() => setCut(index, currentSeq)}>以当前帧结束第 {segment.index} 段</Button>
            <Button size="sm" disabled={!Number.isInteger(cut) || cut < 1 || cut >= groupCount} onClick={() => onNavigate(cut)}>查看第 {segment.index} 段末帧</Button>
            <Button size="sm" disabled={!Number.isInteger(cut) || cut < 1 || cut >= groupCount} onClick={() => onNavigate(cut + 1)}>查看第 {segment.index + 1} 段首帧</Button>
          </>}
        </div>;
      })}
    </div>
    {invalid && <p className="c-warn" role="status">请填写 {boundaryCount} 个严格递增的整数边界，每段至少保留 1 个原始帧组，结束边界须小于 {groupCount}。</p>}
    <div className="wp-actions">
      <Button disabled={locked || invalid || (staleDraft && !acceptStaleDraft) || (!edited && review?.status !== "stale" && !!review)} onClick={() => void run(() => workspaceApi.captureReviewSave(id, revision, roundId, cuts), "分段草稿已保存", true)}>保存分段草稿</Button>
      {edited && <Button disabled={locked} onClick={() => remember(null)}>放弃未保存边界</Button>}
      <Checkbox label="已查看各段边界两侧图像并确认范围" checked={checked} disabled={locked || edited || invalid || !review || review.status === "stale" || review.status === "confirmed"} onCheckedChange={setChecked}/>
      <Button disabled={locked || edited || invalid || !checked || review?.status !== "draft"} onClick={() => void run(() => workspaceApi.captureReviewConfirm(id, revision, roundId), "分段边界已确认")}>确认分段</Button>
    </div>
    <details className="wp-capture-more" open>
      <summary>异常帧组屏蔽与恢复</summary>
      <p className="muted hint">屏蔽整组所有相机图像，用于排除图像标定测量；保留原始序号、时间位置和帧数统计。轨迹异常、中途停止的轮次应整体移出标定。</p>
      <div className="wp-actions wp-review-mask">
        <NumberField label="屏蔽范围起始帧组" value={start} min={1} max={groupCount} disabled={locked} onChange={setStart}/>
        <NumberField label="屏蔽范围结束帧组" value={end} min={1} max={groupCount} disabled={locked} onChange={setEnd}/>
        <Button disabled={locked} onClick={() => { setStart(currentSeq); setEnd(currentSeq); }}>选中当前帧组</Button>
        <Select label="屏蔽原因" value={reason} disabled={locked} onValueChange={setReason} options={["图像模糊或反光", "遮挡或污染", "图像不可用于标定"].map(value => ({ value, label: value }))}/>
        <Button disabled={locked || !rangeValid} onClick={() => void run(() => workspaceApi.captureReviewExclude(id, revision, roundId, start, end, true, reason), "所选帧组已屏蔽，原始序号和帧数保留")}>屏蔽所选范围</Button>
        <Button disabled={locked || !rangeValid || !excluded.some(group => group.seq >= start && group.seq <= end)} onClick={() => void run(() => workspaceApi.captureReviewExclude(id, revision, roundId, start, end, false, ""), "所选帧组已恢复")}>恢复所选范围</Button>
      </div>
      {!rangeValid && <p className="c-warn" role="status">范围须为 1–{groupCount} 之间的整数，起始不得大于结束。</p>}
      {!!excluded.length && <details className="wp-capture-more"><summary>查看已屏蔽帧组（{excluded.length}）</summary><div className="wp-review-excluded">{excluded.map(group => <Button key={group.seq} size="sm" onClick={() => { onNavigate(group.seq); setStart(group.seq); setEnd(group.seq); }}>帧组 {group.seq} · {group.reason}</Button>)}</div></details>}
    </details>
  </Panel>;
}
