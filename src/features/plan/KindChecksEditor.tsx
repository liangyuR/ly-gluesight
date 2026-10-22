import type { ReactNode } from "react";
import { FormField, NumberField, Switch } from "qing-steel";
import type { KindDefault, OffsetCheck, SegmentChecks, SegmentKind } from "./types";
import { segmentKindLabel } from "./vis";
import "./plan.css";

type Value = KindDefault | SegmentChecks;

/** 新开胶偏检查时的限值：与后端 KindDefaults::default 的直线段一致（±2 mm）。 */
const defaultOffset: OffsetCheck = { enabled: true, maxMeanShiftMm: 2, maxLocalDevMm: 2 };
/** 新开胶宽均匀性检查时的极差上限（mm）；后端默认不检（null）。 */
const defaultMaxRangeMm = 1;

const isKindDefault = (value: Value): value is KindDefault => "checks" in value;

function Num({ label, value, unit, step = 0.1, min, max, disabled, onChange }: {
  label: string; value: number; unit?: string; step?: number; min?: number; max?: number; disabled?: boolean; onChange: (value: number) => void;
}) {
  return <FormField label={label} className="plan-field">
    <NumberField aria-label={label} unit={unit} step={step} min={min} max={max} disabled={disabled} value={Number.isFinite(value) ? value : ""} onValueChange={v => onChange(v === "" ? NaN : v)} />
  </FormField>;
}

function Check({ title, enabled, disabled, onToggle, children }: { title: string; enabled: boolean; disabled?: boolean; onToggle: (on: boolean) => void; children: ReactNode }) {
  return <section className={"plan-check" + (enabled ? "" : " off")} aria-label={title}>
    <Switch label={`检${title}`} checked={enabled} disabled={disabled} onCheckedChange={onToggle} />
    <div className="plan-check-fields">{children}</div>
  </section>;
}

export interface KindChecksEditorProps<T extends Value> {
  /** 配方默认参数（KindDefault，带容差）或某一段的检测项（SegmentChecks） */
  value: T;
  kind: SegmentKind;
  onChange: (next: T) => void;
  disabled?: boolean;
}

/**
 * 一种段类型（或一段）的检测项与允差：胶宽、断胶、溢胶每帧独立判，两类段都有；
 * 胶宽均匀性与胶偏只在直线段。弯道段不显示这两项，且发出的值 offset 恒为 null（规格 §4 弯道不检胶偏）。
 * 给 KindDefault 时另有帧数容差（默认 1）。只编辑数值，不做校验：越界由后端校验报出。
 */
export function KindChecksEditor<T extends Value>({ value, kind, onChange, disabled }: KindChecksEditorProps<T>) {
  const checks: SegmentChecks = isKindDefault(value) ? value.checks : value as SegmentChecks;
  const straight = kind === "straight";
  const emit = (next: SegmentChecks) => {
    const normalized = straight ? next : { ...next, offset: null };
    onChange((isKindDefault(value) ? { ...value, checks: normalized } : normalized) as T);
  };
  const { width, break: gap, overflow, offset } = checks;
  const setWidth = (patch: Partial<SegmentChecks["width"]>) => emit({ ...checks, width: { ...width, ...patch } });
  const setGap = (patch: Partial<SegmentChecks["break"]>) => emit({ ...checks, break: { ...gap, ...patch } });
  const setOverflow = (patch: Partial<SegmentChecks["overflow"]>) => emit({ ...checks, overflow: { ...overflow, ...patch } });
  const setOffset = (patch: Partial<OffsetCheck>) => emit({ ...checks, offset: { ...(offset ?? defaultOffset), ...patch } });
  return <div className="plan-checks" role="group" aria-label={`${segmentKindLabel[kind]}段检测项`}>
    {isKindDefault(value) && <div className="plan-check-fields">
      <Num label="帧数容差" unit="帧" step={1} min={0} max={1000} disabled={disabled} value={value.tolerance}
        onChange={tolerance => onChange({ ...value, tolerance } as T)} />
    </div>}
    <Check title="胶宽" enabled={width.enabled} disabled={disabled} onToggle={enabled => setWidth({ enabled })}>
      <Num label="胶宽名义值" unit="mm" disabled={disabled || !width.enabled} value={width.nominalMm} onChange={nominalMm => setWidth({ nominalMm })} />
      <Num label="胶宽上公差" unit="mm" disabled={disabled || !width.enabled} value={width.tolUpper} onChange={tolUpper => setWidth({ tolUpper })} />
      <Num label="胶宽下公差" unit="mm" disabled={disabled || !width.enabled} value={width.tolLower} onChange={tolLower => setWidth({ tolLower })} />
      <Num label="帧内允许超差长度" unit="mm" disabled={disabled || !width.enabled} value={width.maxExcursionMm} onChange={maxExcursionMm => setWidth({ maxExcursionMm })} />
      {straight && <>
        <Switch label="检胶宽均匀性" checked={width.maxRangeMm != null} disabled={disabled || !width.enabled}
          onCheckedChange={on => setWidth({ maxRangeMm: on ? defaultMaxRangeMm : null })} />
        {width.maxRangeMm != null && <Num label="胶宽极差上限" unit="mm" disabled={disabled || !width.enabled} value={width.maxRangeMm} onChange={maxRangeMm => setWidth({ maxRangeMm })} />}
      </>}
    </Check>
    <Check title="断胶" enabled={gap.enabled} disabled={disabled} onToggle={enabled => setGap({ enabled })}>
      <Num label="允许连续缺胶" unit="mm" min={0} disabled={disabled || !gap.enabled} value={gap.maxGapMm} onChange={maxGapMm => setGap({ maxGapMm })} />
      <Num label="最低有胶比例" step={0.05} min={0} max={1} disabled={disabled || !gap.enabled} value={gap.minPresent} onChange={minPresent => setGap({ minPresent })} />
    </Check>
    <Check title="溢胶" enabled={overflow.enabled} disabled={disabled} onToggle={enabled => setOverflow({ enabled })}>
      <Num label="溢胶胶宽上限" unit="mm" disabled={disabled || !overflow.enabled} value={overflow.maxWidthMm} onChange={maxWidthMm => setOverflow({ maxWidthMm })} />
    </Check>
    {straight
      ? <Check title="胶偏" enabled={!!offset?.enabled} disabled={disabled} onToggle={enabled => setOffset({ enabled })}>
        <Num label="整段偏移上限" unit="mm" disabled={disabled || !offset?.enabled} value={(offset ?? defaultOffset).maxMeanShiftMm} onChange={maxMeanShiftMm => setOffset({ maxMeanShiftMm })} />
        <Num label="局部跑偏上限" unit="mm" disabled={disabled || !offset?.enabled} value={(offset ?? defaultOffset).maxLocalDevMm} onChange={maxLocalDevMm => setOffset({ maxLocalDevMm })} />
      </Check>
      : <p className="plan-check-note muted">弯道段不判胶偏、不判胶宽均匀性。</p>}
  </div>;
}
