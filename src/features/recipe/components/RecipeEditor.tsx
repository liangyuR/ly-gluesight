import { useEffect, useId, useState, type ReactNode } from "react";
import { FormField, NumberField, Select, Switch, TextField } from "qing-steel";
import type { DetectParams, JudgeParams, Polarity, RecipeDoc, ShotLimits } from "../../cycle/types";

interface Props {
  initial: RecipeDoc;
  /** 已发布的生产配方；给出时标出与它不同的字段 */
  production?: RecipeDoc | null;
  onDraftChange?: (doc: RecipeDoc) => void;
  disabled?: boolean;
}

const paramFields: [keyof JudgeParams, string, string][] = [
  ["nominal", "名义", ""],
  ["tolUpper", "上公差", "高于名义的允许偏差"],
  ["tolLower", "下公差", "低于名义的允许偏差"],
  ["absMin", "绝对下限", "低于它直接 NG"],
  ["absMax", "绝对上限", "高于它直接 NG"],
  ["maxExcursionLen", "允许超差长度", "连续超出公差带超过这个长度判 NG"],
];

/** 新启用位置、胶宽判定时的默认值（与 recipe.rs 的 default_limits 一致：名义胶宽 4 mm）。 */
export const defaultPosition: JudgeParams = { nominal: 0, tolUpper: 2, tolLower: 2, absMin: -5, absMax: 5, maxExcursionLen: 5 };
export const defaultWidth: JudgeParams = { nominal: 4, tolUpper: 1.5, tolLower: 1.5, absMin: 1, absMax: 8, maxExcursionLen: 5 };

const polarityLabel: Record<Polarity, string> = { dark: "暗胶条（比背景暗）", light: "亮胶条（比背景亮）" };
const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
const fixed = (v: number) => Number.isFinite(v) ? String(Math.round(v * 100) / 100) : "—";

/** 与生产配方逐项比较的字段，changedFields 与页面上的标记共用这份清单。 */
const compared: [string, (d: RecipeDoc) => unknown][] = [
  ["name", d => d.name], ["productCode", d => d.productCode], ["triggerMode", d => d.triggerMode], ["spacing", d => d.spacing], ["filterWindow", d => d.filterWindow],
  ["searchMm", d => d.detect.searchMm], ["polarity", d => d.detect.polarity], ["widthRange", d => d.detect.widthRange],
  ["position", d => d.limits.position], ["width", d => d.limits.width], ["maxGapLen", d => d.limits.maxGapLen], ["minPresent", d => d.limits.minPresent],
];
export function changedFields(doc: RecipeDoc, production: RecipeDoc | null | undefined) {
  if (!production) return new Set<string>();
  return new Set(compared.filter(([, get]) => !same(get(doc), get(production))).map(([key]) => key));
}

function Field({ id, label, changed, hint, wide, children }: { id?: string; label: string; changed?: boolean; hint?: string; wide?: boolean; children: ReactNode }) {
  return (
    <FormField id={id} className={"rcp-field" + (wide ? " wide" : "") + (changed ? " changed" : "")} label={<span title={hint}>{label}{changed && <i className="rcp-changed" title="与生产配方不同" />}</span>}>
      {children}
    </FormField>
  );
}

function Num({ id, value, onChange, unit, step = 0.1, aria }: { id?: string; value: number; onChange: (v: number) => void; unit?: string; step?: number; aria: string }) {
  return <NumberField id={id} aria-label={aria} unit={unit} step={step} value={Number.isFinite(value) ? value : ""} onValueChange={(v) => onChange(v === "" ? NaN : v)} />;
}

function Section({ title, children, extra }: { title: string; children: ReactNode; extra?: ReactNode }) {
  return (
    <section className="rcp-section">
      <h4 className="sub-title">{title}{extra && <small>{extra}</small>}</h4>
      {children}
    </section>
  );
}

function LimitsEditor({ value, changed, onChange }: { value: ShotLimits; changed: Set<string>; onChange: (v: ShotLimits) => void }) {
  const row = (kind: "position" | "width", name: string, fallback: JudgeParams) => {
    const p = value[kind];
    return (
      <tr key={kind} className={p ? "" : "off"}>
        <th scope="row"><span className="rcp-rowhead">
          <Switch label={name} aria-label={`${name}判定`} checked={!!p} onCheckedChange={(on) => onChange({ ...value, [kind]: on ? { ...fallback } : null })} />
          {changed.has(kind) && <i className="rcp-changed" title="与生产配方不同" />}
        </span></th>
        {p ? <>
          {paramFields.map(([k, field]) => (
            <td key={k}><Num aria={`${name} · ${field}`} step={0.05} value={p[k]} onChange={(v) => onChange({ ...value, [kind]: { ...p, [k]: v } })} /></td>
          ))}
          <td className="rcp-band">{fixed(p.nominal - p.tolLower)} ~ {fixed(p.nominal + p.tolUpper)}</td>
        </> : <td colSpan={paramFields.length + 1} className="muted">不判{name}</td>}
      </tr>
    );
  };
  return (
    <div className="table-wrap">
      <table className="table rcp-limits">
        <thead>
          <tr>
            <th />
            {paramFields.map(([k, label, hint]) => <th key={k} title={hint || undefined}>{label}</th>)}
            <th title="名义减下公差 ~ 名义加上公差">合格区间</th>
          </tr>
        </thead>
        <tbody>
          {row("position", "位置", defaultPosition)}
          {value.position && value.position.nominal !== 0 && <tr>
            <th scope="row" title="位置名义值不为 0 时必填：用量具确认过的确认件编号">量具确认件</th>
            <td colSpan={paramFields.length + 1}>
              <TextField aria-label="量具确认件编号" value={value.gaugeRef ?? ""} maxLength={64} onChange={(e) => onChange({ ...value, gaugeRef: e.target.value })} />
              {!value.gaugeRef?.trim() && <span className="c-warn" role="alert">名义值不为 0：必须填写量具确认件编号，否则候选保存不了</span>}
            </td>
          </tr>}
          {row("width", "胶宽", defaultWidth)}
          <tr>
            <th scope="row" title="同一拍照点内连续缺胶超过允许断胶长度判断胶；累计缺胶超过它且有胶站占比低于最低有胶比例时也判断胶"><span className="rcp-rowhead">连续性{(changed.has("maxGapLen") || changed.has("minPresent")) && <i className="rcp-changed" title="与生产配方不同" />}</span></th>
            <td colSpan={paramFields.length + 1}>
              <span className="rcp-inline">允许断胶<Num aria="允许断胶长度（mm）" unit="mm" value={value.maxGapLen} onChange={(maxGapLen) => onChange({ ...value, maxGapLen })} /></span>
              <span className="rcp-inline">最低有胶<Num aria="最低有胶比例（%）" unit="%" step={1} value={Math.round(value.minPresent * 1000) / 10} onChange={(v) => onChange({ ...value, minPresent: v / 100 })} /></span>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  );
}

function DetectEditor({ value, changed, disabled, onChange }: { value: DetectParams; changed: Set<string>; disabled?: boolean; onChange: (v: DetectParams) => void }) {
  const [lo, hi] = value.widthRange;
  const loId = useId(), hiId = useId();
  return (
    <div className="rcp-grid">
      <Field label="搜索半宽" changed={changed.has("searchMm")} hint="沿中线法向找胶的半宽，要盖住机器人与工件带来的偏差"><Num aria="搜索半宽（mm）" unit="mm" value={value.searchMm} onChange={(searchMm) => onChange({ ...value, searchMm })} /></Field>
      <Field label="极性" changed={changed.has("polarity")}>
        <Select aria-label="极性" disabled={disabled} value={value.polarity} options={(Object.keys(polarityLabel) as Polarity[]).map((p) => ({ value: p, label: polarityLabel[p] }))} onValueChange={(polarity) => onChange({ ...value, polarity: polarity as Polarity })} />
      </Field>
      <Field id={loId} label="胶宽范围" wide changed={changed.has("widthRange")} hint="范围外的不当作胶；上限要小于搜索宽度（搜索半宽的两倍）">
        <span className="rcp-range"><Num id={loId} aria="胶宽下限（mm）" value={lo} onChange={(v) => onChange({ ...value, widthRange: [v, hi] })} /><small>–</small><Num id={hiId} aria="胶宽上限（mm）" unit="mm" value={hi} onChange={(v) => onChange({ ...value, widthRange: [lo, v] })} /></span>
      </Field>
    </div>
  );
}

/** 配方默认检测与判定参数；修改即时交给 onDraftChange，由工作台自动保存为候选配置。 */
export default function RecipeEditor({ initial, production, onDraftChange, disabled }: Props) {
  const [doc, setDoc] = useState<RecipeDoc>(initial);
  useEffect(() => { onDraftChange?.(doc); }, [doc, onDraftChange]);
  const set = <K extends keyof RecipeDoc>(k: K, v: RecipeDoc[K]) => setDoc((previous) => ({ ...previous, [k]: v }));
  const changed = changedFields(doc, production);
  const identityChanged = changed.has("name") || changed.has("productCode");

  return (
    <div className="rcp-editor">
      <details className="rcp-section rcp-identity" open={identityChanged || undefined}>
        <summary><h4 className="sub-title">配方信息{identityChanged && <i className="rcp-changed" title="与生产配方不同" />}</h4><span className="muted">{doc.id} · {doc.name} · 产品代码 {doc.productCode}</span></summary>
        <div className="rcp-grid">
          <Field label="配方编号"><TextField className="mono" aria-label="配方编号" value={doc.id} readOnly disabled /></Field>
          <Field label="名称" changed={changed.has("name")}><TextField aria-label="名称" value={doc.name} onChange={(e) => set("name", e.target.value)} /></Field>
          <Field label="产品代码（PLC 下发）" changed={changed.has("productCode")}><Num aria="产品代码（PLC 下发）" step={1} value={doc.productCode} onChange={(v) => set("productCode", v)} /></Field>
        </div>
      </details>

      <Section title="采样">
        <div className="rcp-grid">
          <Field label="触发方式" wide changed={changed.has("triggerMode")}>
            <Select aria-label="触发方式" disabled={disabled} value={doc.triggerMode} options={[{ value: "fly", label: "飞拍（位置比较触发）" }, { value: "stop", label: "停稳拍" }]} onValueChange={(mode) => set("triggerMode", mode as RecipeDoc["triggerMode"])} />
          </Field>
          <Field label="站距" changed={changed.has("spacing")} hint="沿示教中线每隔这个距离量一站"><Num aria="站距（mm）" unit="mm" value={doc.spacing} onChange={(v) => set("spacing", v)} /></Field>
          <Field label="中值滤波窗口" changed={changed.has("filterWindow")} hint="1–31 的奇数"><Num aria="中值滤波窗口（点，奇数）" unit="点" step={2} value={doc.filterWindow} onChange={(v) => set("filterWindow", v)} /></Field>
        </div>
      </Section>

      <Section title="检测" extra="拍照点没单独设置时使用；改检测参数后需重新试测">
        <DetectEditor value={doc.detect} changed={changed} disabled={disabled} onChange={(v) => set("detect", v)} />
      </Section>

      <Section title="判定" extra="单位 mm；连续超出公差带超过允许超差长度判 NG，超出绝对限直接 NG">
        <LimitsEditor value={doc.limits} changed={changed} onChange={(v) => set("limits", v)} />
      </Section>
    </div>
  );
}
