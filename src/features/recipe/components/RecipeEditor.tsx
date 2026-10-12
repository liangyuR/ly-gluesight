import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Save } from "lucide-react";
import { recipeApi } from "../../cycle/api";
import ShotTiles from "../../cycle/components/ShotTiles";
import { segmentLength, shotTaught } from "../../cycle/vis";
import type { DetectParams, JudgeParams, Polarity, Recipe, RecipeDoc, ShotLimits } from "../../cycle/types";

interface Props {
  initial: RecipeDoc;
  originalId: string | null;
  onDraftChange?: (doc: RecipeDoc) => void;
  saveCandidate: (doc: RecipeDoc) => Promise<boolean>;
}

const paramFields: [keyof JudgeParams, string][] = [
  ["nominal", "名义"],
  ["tolUpper", "上公差"],
  ["tolLower", "下公差"],
  ["absMin", "绝对下限"],
  ["absMax", "绝对上限"],
  ["maxExcursionLen", "允许超差长度"],
];

/** 新启用位置、胶宽判定时的默认值（与 recipe.rs 的 default_limits 一致：名义胶宽 4 mm）。 */
export const defaultPosition: JudgeParams = { nominal: 0, tolUpper: 2, tolLower: 2, absMin: -5, absMax: 5, maxExcursionLen: 5 };
export const defaultWidth: JudgeParams = { nominal: 4, tolUpper: 1.5, tolLower: 1.5, absMin: 1, absMax: 8, maxExcursionLen: 5 };

const polarityLabel: Record<Polarity, string> = { dark: "暗胶条（比背景暗）", light: "亮胶条（比背景亮）" };
const parse = (value: string) => (value === "" ? NaN : Number(value));

function Num({ label, value, onChange, step = 0.1, hint, aria }: { label: string; value: number; onChange: (v: number) => void; step?: number; hint?: string; aria?: string }) {
  return (
    <label className="field" title={hint}>
      <span>{label}</span>
      <input className="input mono" aria-label={aria} type="number" step={step} value={Number.isFinite(value) ? value : ""} onChange={(e) => onChange(parse(e.target.value))} />
    </label>
  );
}

function Section({ title, children, extra }: { title: string; children: ReactNode; extra?: ReactNode }) {
  return (
    <section className="rcp-section">
      <div className="panel-head">
        <h4 className="sub-title">{title}</h4>
        <span className="spacer" />
        {extra}
      </div>
      {children}
    </section>
  );
}

/** 检测参数：搜索半宽、极性、胶宽范围。prefix 区分配方默认与拍照点单独设的。 */
function DetectEditor({ prefix, value, onChange }: { prefix: string; value: DetectParams; onChange: (v: DetectParams) => void }) {
  const [lo, hi] = value.widthRange;
  return (
    <div className="form-grid">
      <Num label="搜索半宽（mm）" aria={`${prefix}搜索半宽（mm）`} value={value.searchMm} hint="沿中线法向找胶的半宽，要盖住机器人与工件带来的偏差" onChange={(searchMm) => onChange({ ...value, searchMm })} />
      <label className="field">
        <span>极性</span>
        <select className="input" aria-label={`${prefix}极性`} value={value.polarity} onChange={(e) => onChange({ ...value, polarity: e.target.value as Polarity })}>
          {(Object.keys(polarityLabel) as Polarity[]).map((p) => (
            <option key={p} value={p}>{polarityLabel[p]}</option>
          ))}
        </select>
      </label>
      <Num label="胶宽下限（mm）" aria={`${prefix}胶宽下限（mm）`} value={lo} hint="比它窄的不当作胶" onChange={(v) => onChange({ ...value, widthRange: [v, hi] })} />
      <Num label="胶宽上限（mm）" aria={`${prefix}胶宽上限（mm）`} value={hi} hint="比它宽的不当作胶，要小于搜索宽度" onChange={(v) => onChange({ ...value, widthRange: [lo, v] })} />
    </div>
  );
}

/** 判定限值：位置、胶宽可分别停用（null 即不判），加允许断胶长度。 */
function LimitsEditor({ prefix, value, onChange }: { prefix: string; value: ShotLimits; onChange: (v: ShotLimits) => void }) {
  const row = (kind: "position" | "width", name: string, fallback: JudgeParams) => {
    const p = value[kind];
    const label = `${prefix}${name}`;
    return (
      <tr key={kind}>
        <td>
          {name}
          {p ? (
            <button type="button" className="btn small" aria-label={`停用${label}判定`} onClick={() => onChange({ ...value, [kind]: null })}>停用{name}判定</button>
          ) : null}
        </td>
        {p ? (
          paramFields.map(([k, field]) => (
            <td key={k}>
              <input aria-label={`${label} · ${field}`} className="input mono" type="number" step={0.05} value={Number.isFinite(p[k]) ? p[k] : ""} onChange={(e) => onChange({ ...value, [kind]: { ...p, [k]: parse(e.target.value) } })} />
            </td>
          ))
        ) : (
          <td colSpan={paramFields.length}>
            <span className="muted">不判{name}</span>{" "}
            <button type="button" className="btn small" aria-label={`启用${label}判定`} onClick={() => onChange({ ...value, [kind]: { ...fallback } })}>
              启用{name}判定
            </button>
          </td>
        )}
      </tr>
    );
  };
  return (
    <>
      <div className="table-wrap">
        <table className="table rcp-limits">
          <thead>
            <tr>
              <th />
              {paramFields.map(([k, l]) => (
                <th key={k}>{l}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {row("position", "位置", defaultPosition)}
            {row("width", "胶宽", defaultWidth)}
          </tbody>
        </table>
      </div>
      <div className="form-grid">
        <Num label="允许断胶长度（mm）" aria={`${prefix}允许断胶长度（mm）`} value={value.maxGapLen} hint="同一拍照点内连续缺胶超过这个长度判断胶；整段一站胶都没有时总是判断胶" onChange={(maxGapLen) => onChange({ ...value, maxGapLen })} />
        <Num label="最低有胶比例（%）" aria={`${prefix}最低有胶比例（%）`} step={1} value={Math.round(value.minPresent * 1000) / 10} hint="零散断胶：同一拍照点内累计缺胶超过允许断胶长度、且有胶站占比低于这个值时判断胶" onChange={(v) => onChange({ ...value, minPresent: v / 100 })} />
      </div>
    </>
  );
}

export default function RecipeEditor({ initial, originalId, onDraftChange, saveCandidate }: Props) {
  const [doc, setDoc] = useState<RecipeDoc>(initial);
  const [preview, setPreview] = useState<Recipe | null>(null);
  const [previewDoc, setPreviewDoc] = useState<RecipeDoc | null>(null);
  const [previewError, setPreviewError] = useState("");
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const mounted = useRef(true), pendingSave = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { onDraftChange?.(doc); }, [doc, onDraftChange]);

  useEffect(() => {
    let alive = true;
    setPreviewError("");
    const t = setTimeout(() => {
      recipeApi
        .preview(doc)
        .then((r) => {
          if (!alive) return;
          setPreview(r);
          setPreviewDoc(doc);
          setPreviewError("");
        })
        .catch((e) => { if (alive) { setPreview(null); setPreviewDoc(null); setPreviewError(String(e)); } });
    }, 350);
    return () => { alive = false; clearTimeout(t); };
  }, [doc]);

  const set = <K extends keyof RecipeDoc>(k: K, v: RecipeDoc[K]) => { setNotice(null); setDoc((previous) => ({ ...previous, [k]: v })); };

  const save = async () => {
    if (pendingSave.current || previewDoc !== doc || previewError) return;
    pendingSave.current = true; setSaving(true); setNotice(null);
    try {
      const saved = await saveCandidate(doc);
      if (mounted.current) setNotice(saved ? { ok: true, text: "候选配置已保存，生产版本保持不变" } : { ok: false, text: "候选配置未保存，请修正错误后重试" });
    } catch (e) {
      if (mounted.current) setNotice({ ok: false, text: String(e) });
    } finally { pendingSave.current = false; if (mounted.current) setSaving(false); }
  };

  const currentPreview = previewDoc === doc ? preview : null;
  const summary = useMemo(() => {
    if (!currentPreview) return "";
    const len = currentPreview.segments.reduce((a, g) => a + segmentLength(g, currentPreview.spacing), 0);
    const measured = currentPreview.shots.filter((s) => !s.skip);
    return `${currentPreview.shots.length} 个拍照点 · 已示教 ${measured.filter(shotTaught).length}/${measured.length} · 中线共 ${len.toFixed(1)} mm · ${currentPreview.points.x.length} 个测量点`;
  }, [currentPreview]);
  const none = useMemo(() => (currentPreview ? new Array(currentPreview.points.x.length).fill("none") : []), [currentPreview]);

  return (
    <div className="rcp-editor">
      <div className="rcp-form">
        <div className="panel-toolbar">
          <h3 className="panel-title">
            {originalId ? `编辑 ${originalId}` : "新配方"} · 飞拍
            {originalId && <span className="muted mono"> v{doc.version}</span>}
          </h3>
          <button className="btn primary" onClick={() => void save()} disabled={saving || !!previewError || previewDoc !== doc}>
            <Save size={15} />
            {saving ? "保存中…" : "保存候选配置"}
          </button>
        </div>
        {notice && <div className={`notice ${notice.ok ? "ok" : "error"}`}>{notice.text}</div>}
        <fieldset disabled={saving} style={{ border: 0, padding: 0, margin: 0, minWidth: 0, display: "flex", flexDirection: "column", gap: 14 }}>
        <Section title="基本">
          <div className="form-grid">
            <label className="field">
              <span>配方编号</span>
              <input className="input mono" value={doc.id} readOnly disabled />
            </label>
            <label className="field">
              <span>名称</span>
              <input className="input" value={doc.name} onChange={(e) => set("name", e.target.value)} />
            </label>
            <Num label="产品代码（PLC 下发）" value={doc.productCode} step={1} onChange={(v) => set("productCode", v)} />
            <label className="field">
              <span>触发方式</span>
              <select className="input" value={doc.triggerMode} onChange={(e) => set("triggerMode", e.target.value as RecipeDoc["triggerMode"])}>
                <option value="fly">飞拍（位置比较触发）</option>
                <option value="stop">停稳拍</option>
              </select>
            </label>
            <Num label="站距（mm）" value={doc.spacing} hint="沿示教中线每隔这个距离量一站" onChange={(v) => set("spacing", v)} />
            <Num label="中值滤波窗口（点，奇数）" value={doc.filterWindow} step={2} onChange={(v) => set("filterWindow", v)} />
          </div>
        </Section>

        <Section title="默认检测参数">
          <DetectEditor prefix="" value={doc.detect} onChange={(v) => set("detect", v)} />
          <p className="muted hint">沿示教中线逐站沿法向找胶。拍照点没单独设检测参数时用这里的值；改检测参数后要重新试测。</p>
        </Section>

        <Section title="默认判定限值（mm）">
          <LimitsEditor prefix="" value={doc.limits} onChange={(v) => set("limits", v)} />
          <p className="muted hint">
            位置：胶条中线相对示教中线的横向偏移；胶宽：沿法向量出的宽度。连续超出公差带超过“允许超差长度”判 NG，超出绝对限直接 NG；同一拍照点内连续缺胶超过“允许断胶长度”判断胶。每个拍照点自成一段，段与段之间不连。
          </p>
        </Section>

        <Section title="飞拍拍照点">
          <p className="muted">{doc.shots.length} 个拍照点由整圈实际采集生成；选图和不检测设置在单帧示教中调整。</p>
        </Section>
        </fieldset>
      </div>

      <div className="rcp-preview">
        <div className="panel-head">
          <h4 className="sub-title">预览</h4>
          <span className="muted">{previewError ? "" : currentPreview ? summary : "正在更新预览…"}</span>
        </div>
        {previewError && <div className="notice error">{previewError}</div>}
        {currentPreview && <ShotTiles layout={currentPreview} vis={none} className="rcp-tiles" idleLabel="已示教" />}
        {currentPreview && (
          <div className="rcp-segs">
            {currentPreview.segments.map((g) => (
              <span key={g.name}>
                {g.name} <b className="mono">{segmentLength(g, currentPreview.spacing).toFixed(1)}</b>
              </span>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
