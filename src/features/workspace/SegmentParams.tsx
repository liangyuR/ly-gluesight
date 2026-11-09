import { useState, type ReactNode } from "react";
import { Button, NumberField, SegmentedControl, Surface } from "qing-steel";
import type { PathSegment, SegmentKind } from "../plan/types";
import { Notice } from "./components";
import { MAX_SKIP_FRAMES, patchSegment, segmentIssues } from "./segmentEdit";
import { setKind, toggleCamera, valid } from "./workbenchDraft";
import type { SegmentDraft } from "./useSegmentDraft";

type Field = "lengthMm" | "skipHead" | "skipTail";

/**
 * 分段与示教页右栏的当前段参数：类型、检测相机改分段草稿（随分段自动保存）；长度与段首 / 段尾跳过写进候选配置（随配置自动保存）。
 * 段结构（段数、类型）有未保存的改动时段参数只读：段位置还对不上已保存的段列表。两边的保存互相等待，不同时提交。
 */
export default function SegmentParams({ seg, pos, label, segments, cameras, disabled, onSegments, children }: {
  seg: SegmentDraft; pos: number; label: string; segments: PathSegment[]; cameras: string[]; disabled: boolean;
  onSegments: (segments: PathSegment[]) => void; children?: ReactNode;
}) {
  const draft = seg.draft;
  const at = seg.restructured ? -1 : pos;
  const segment = at >= 0 ? segments[at] : undefined;
  const [local, setLocal] = useState<{ index: number | undefined; values: Partial<Pick<PathSegment, Field>> }>({ index: segment?.index, values: {} });
  const values = local.index === segment?.index ? local.values : {};
  if (!draft) return null;
  const kind = draft.skips[pos] ? "skip" : draft.kinds[pos];
  const chosen = draft.cameras[pos] ?? [];
  const draftLocked = seg.locked || disabled;
  const paramsLocked = disabled ? "当前不能修改" : !segment ? "段结构的改动还没保存：保存后再改段参数" : "";
  const shown = segment ? { ...segment, ...values } as PathSegment : undefined;
  const issues = shown ? segmentIssues([shown]) : [];
  const edit = (field: Field, value: number | null) => {
    if (!segment) return;
    const next = { ...values, [field]: value } as Partial<Pick<PathSegment, Field>>;
    const patched = { ...segment, ...next } as PathSegment;
    if (segmentIssues([patched]).length) {
      setLocal({ index: segment.index, values: next });
    } else {
      setLocal({ index: segment.index, values: {} });
      onSegments(patchSegment(segments, at, next));
    }
  };
  const changeKind = (value: string) => {
    const next = setKind(draft, pos, value as SegmentKind | "skip");
    if (valid(next)) seg.change(next);
  };
  const allSkipOther = draft.skips.every((skip, i) => i === pos || skip);

  return <Surface role="region" className="wp-seg-corridor-note sp-card" aria-label={`${label} 段参数`}>
    <div className="wp-seg-head"><h3>{label} · 段参数</h3>{children}</div>
    <SegmentedControl size="sm" label="类型" value={kind} disabled={!!draftLocked} options={[
      { value: "straight", label: "直线" }, { value: "corner", label: "弯道" },
      { value: "skip", label: "不检测", disabled: allSkipOther },
    ]} onValueChange={changeKind} />
    <div className="swb-cams" role="group" aria-label={`${label} 检测相机`}>
      <span className="muted">检测相机</span>
      {cameras.map(id => {
        const on = kind !== "skip" && chosen.includes(id);
        return <Button variant="text" size="sm" key={id} type="button" className={`swb-cam${on ? " on" : ""}`} aria-pressed={on} disabled={!!draftLocked || kind === "skip"}
          onClick={() => seg.change(toggleCamera(draft, pos, id, cameras))}>{id}</Button>;
      })}
    </div>
    {seg.locked && <p className="muted">{seg.locked}</p>}
    <div className="sp-fields" title={paramsLocked || undefined}>
      <NumberField label="长度" unit="mm" aria-label={`${label} 长度`} step={1} min={0} clamp={false} disabled={!!paramsLocked}
        value={shown?.lengthMm ?? ""} onValueChange={value => edit("lengthMm", value === "" ? null : value)} />
      <NumberField label="段首跳过" unit="帧" aria-label={`${label} 段首跳过帧数`} step={1} min={0} max={MAX_SKIP_FRAMES} clamp={false} disabled={!!paramsLocked}
        value={shown && Number.isFinite(shown.skipHead) ? shown.skipHead : ""} onValueChange={value => edit("skipHead", value === "" ? 0 : value)} />
      <NumberField label="段尾跳过" unit="帧" aria-label={`${label} 段尾跳过帧数`} step={1} min={0} max={MAX_SKIP_FRAMES} clamp={false} disabled={!!paramsLocked}
        value={shown && Number.isFinite(shown.skipTail) ? shown.skipTail : ""} onValueChange={value => edit("skipTail", value === "" ? 0 : value)} />
    </div>
    {issues.length > 0 && <p className="c-warn" role="alert">{issues.join("；")}（修正后自动保存）</p>}
    {paramsLocked && segment && <p className="muted">{paramsLocked}</p>}
    <p className="muted">跳过的帧不判定，结果里列为“未检区域”。{segment?.expectedFrames != null ? `示教 ${segment.expectedFrames} 帧。` : ""}</p>
    {segment?.inspectRange && <Notice title="原毫米检测区间已停用" tone="warn">
      检 {segment.inspectRange.aMm}–{segment.inspectRange.bMm} mm 从未在判定中排除帧，现在不再使用，请改用上面的段首 / 段尾跳过帧数。
      <Button size="sm" disabled={!!paramsLocked} onClick={() => onSegments(patchSegment(segments, at, { inspectRange: null }))}>清除</Button>
    </Notice>}
  </Surface>;
}
