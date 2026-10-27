import { useEffect, useId, useState, type ReactNode } from "react";
import { FormField, NumberField, Select, TextField } from "qing-steel";
import type { DetectParams, Polarity, RecipeDoc } from "../../cycle/types";
import { KindChecksEditor } from "../../plan/KindChecksEditor";
import type { KindDefault, SegmentKind } from "../../plan/types";

interface Props {
  initial: RecipeDoc;
  /** 已发布的生产配方；给出时标出与它不同的字段 */
  production?: RecipeDoc | null;
  onDraftChange?: (doc: RecipeDoc) => void;
  disabled?: boolean;
}

/** 后端 RecipeDoc.calibration_rounds 的默认值（可配范围 10..=100）。 */
const DEFAULT_CALIBRATION_ROUNDS = 10;
const DEFAULT_BOUNDARY_GUARD = 2;
const segmentKinds: [SegmentKind, string][] = [["straight", "直线"], ["corner", "弯道"]];
const polarityLabel: Record<Polarity, string> = { dark: "暗胶条（比背景暗）", light: "亮胶条（比背景亮）" };
const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/** 与生产配方逐项比较的字段，changedFields 与页面上的标记共用这份清单。 */
const compared: [string, (d: RecipeDoc) => unknown][] = [
  ["name", d => d.name], ["productCode", d => d.productCode], ["triggerMode", d => d.triggerMode], ["spacing", d => d.spacing], ["filterWindow", d => d.filterWindow],
  ["searchMm", d => d.detect.searchMm], ["polarity", d => d.detect.polarity], ["widthRange", d => d.detect.widthRange],
  ["position", d => d.limits.position], ["width", d => d.limits.width], ["maxGapLen", d => d.limits.maxGapLen], ["minPresent", d => d.limits.minPresent],
  ["segmentDefaults", d => d.segmentDefaults], ["calibrationRounds", d => d.calibrationRounds], ["boundaryGuard", d => d.boundaryGuard],
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

export function DetectEditor({ value, changed, disabled, onChange }: { value: DetectParams; changed: Set<string>; disabled?: boolean; onChange: (v: DetectParams) => void }) {
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
        <summary><h4 className="sub-title">配方信息{identityChanged && <i className="rcp-changed" title="与生产配方不同" />}</h4><span className="muted">{doc.id} · {doc.name} · 型号 {doc.productCode}</span></summary>
        <div className="rcp-grid">
          <Field label="配方编号"><TextField className="mono" aria-label="配方编号" value={doc.id} readOnly disabled /></Field>
          <Field label="名称" changed={changed.has("name")}><TextField aria-label="名称" value={doc.name} onChange={(e) => set("name", e.target.value)} /></Field>
          <Field label="型号（PLC 下发）" changed={changed.has("productCode")}><Num aria="型号（PLC 下发）" step={1} value={doc.productCode} onChange={(v) => set("productCode", v)} /></Field>
        </div>
      </details>

      <Section title="采样">
        <div className="rcp-grid">
          <Field label="站距" changed={changed.has("spacing")} hint="沿示教中线每隔这个距离量一站"><Num aria="站距（mm）" unit="mm" value={doc.spacing} onChange={(v) => set("spacing", v)} /></Field>
          <Field label="中值滤波窗口" changed={changed.has("filterWindow")} hint="1–31 的奇数"><Num aria="中值滤波窗口（点，奇数）" unit="点" step={2} value={doc.filterWindow} onChange={(v) => set("filterWindow", v)} /></Field>
        </div>
      </Section>

      <Section title="检测" extra="相机没单独设置检测参数时使用；改检测参数后需重新试测">
        <DetectEditor value={doc.detect} changed={changed} disabled={disabled} onChange={(v) => set("detect", v)} />
      </Section>

      <Section title="标定" extra="同一工件程序连续采集多轮，取各轮帧数均值作预期帧数">
        <div className="rcp-grid">
          <Field label="标定轮数" changed={changed.has("calibrationRounds")} hint="至少 10 轮，最多 100 轮">
            <Num aria="标定轮数" unit="轮" step={1} value={doc.calibrationRounds ?? DEFAULT_CALIBRATION_ROUNDS} onChange={(v) => set("calibrationRounds", v)} />
          </Field>
          <Field label="边界过渡帧数" changed={changed.has("boundaryGuard")} hint="按标定比例推算的段边界两侧各留几帧，只判每帧胶宽、断胶、溢胶，不判胶偏；0–5 帧">
            <Num aria="边界过渡帧数" unit="帧" step={1} value={doc.boundaryGuard ?? DEFAULT_BOUNDARY_GUARD} onChange={(v) => set("boundaryGuard", v)} />
          </Field>
        </div>
      </Section>
      {doc.segmentDefaults && <Section title="按段类型的默认检测项" extra="新建段时取用；每帧独立检测，弯道段不判胶偏，帧数容差默认 1">
        {segmentKinds.map(([kind, label]) => <section key={kind} className="rcp-kind" aria-label={`${label}段默认检测项`}>
          <h5 className="sub-title">{label}段{changed.has("segmentDefaults") && !same(doc.segmentDefaults?.[kind], production?.segmentDefaults?.[kind]) && <i className="rcp-changed" title="与生产配方不同" />}</h5>
          <KindChecksEditor value={doc.segmentDefaults![kind]} kind={kind} disabled={disabled} onChange={(next: KindDefault) => set("segmentDefaults", { ...doc.segmentDefaults!, [kind]: next })} />
        </section>)}
      </Section>}
    </div>
  );
}
