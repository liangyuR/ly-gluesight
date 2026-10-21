import { useState } from "react";
import { ChevronDown, CircleCheck, Crosshair, TriangleAlert } from "lucide-react";
import { Button, Checkbox, Dialog, Select, TextField } from "qing-steel";
import type { FixtureSpec } from "../cycle/types";
import type { FixtureCheckItem, FixtureRotationEvidence, RotationConfirmation } from "./types";
import { Badge, NumberField } from "./components";
import { fixtureError } from "./teach";

/** 开转角时的扫描范围 ±°（与验证的转角证据一致） */
export const ROTATION_PROBE_DEG = 3;

interface Props {
  fixture: FixtureSpec | null;
  /** 实时检查（画框、改框）或试测报告的检查项；报告优先 */
  checks: FixtureCheckItem[] | null;
  checksSource: "trial" | "live" | null;
  /** 模板来源过期：定位框是在别的示教图上保存的 */
  staleSource: string | null;
  /** 刚画或刚改的框没通过外圈、走廊检查（或数值不合法），没写进草稿 */
  rejected: string | null;
  /** 最近一次同修订验证里本视图的转角证据 */
  evidence: FixtureRotationEvidence | null;
  disabled: boolean;
  editing: boolean;
  drawing: boolean;
  reviewed: boolean;
  /** 已保存的定位框可以做定位试测 */
  canTrial: boolean;
  trialRunning: boolean;
  onChange: (next: FixtureSpec | null) => void;
  onDraw: () => void;
  onReviewed: (checked: boolean) => void;
  onTrial: () => void;
  onRotate: (confirmation: RotationConfirmation) => Promise<boolean>;
}

const bases: { value: RotationConfirmation["basis"]; label: string }[] = [
  { value: "twoTemplates", label: "同一视图两个不同模板估出的转角一致" },
  { value: "lowerDeviation", label: "验证帧上开转角的 |Δd′| 明显低于只找平移" },
  { value: "physical", label: "多件采集时实物确认转过" },
];

function CheckRow({ item }: { item: FixtureCheckItem }) {
  const bad = !item.passed;
  return <li className={"wp-fixture-check" + (bad ? item.level === "block" ? " block" : " warn" : "")}>
    {bad ? <TriangleAlert size={14} aria-hidden="true"/> : <CircleCheck size={14} aria-hidden="true"/>}
    <span>{item.detail}</span>
  </li>;
}

/** 示教页右栏“定位特征”：框与门限、检查清单、来源过期提示、喷嘴确认、定位试测入口与开转角的高级确认。 */
export function FixtureSettings({ fixture, checks, checksSource, staleSource, rejected, evidence, disabled, editing, drawing, reviewed, canTrial, trialRunning, onChange, onDraw, onReviewed, onTrial, onRotate }: Props) {
  const [rotating, setRotating] = useState(false);
  const [basis, setBasis] = useState<RotationConfirmation["basis"] | "">("");
  const [note, setNote] = useState("");
  const [confirming, setConfirming] = useState(false);
  const blocked = checks?.filter(c => c.level === "block" && !c.passed) ?? [];
  const warned = checks?.filter(c => c.level === "warn" && !c.passed) ?? [];
  const invalid = fixture ? fixtureError(fixture) : "";
  const set = (patch: Partial<FixtureSpec>) => fixture && onChange({ ...fixture, ...patch });
  const setRect = (index: number, value: number) => fixture && onChange({ ...fixture, rect: fixture.rect.map((v, i) => i === index ? value : v) as FixtureSpec["rect"] });
  const tone = !fixture ? "neutral" : rejected || staleSource || blocked.length || invalid ? "warn" : checksSource === "trial" ? "ok" : "info";
  const status = !fixture ? "未使用" : rejected ? "框未采用" : staleSource ? "需确认框" : invalid || blocked.length ? "检查未通过" : checksSource === "trial" ? "试测已验收" : "待试测";
  const rotated = (fixture?.angleRangeDeg ?? 0) > 0;
  const closeRotation = () => { setRotating(false); setBasis(""); setNote(""); };
  const confirmRotation = async () => {
    if (!basis || !note.trim() || confirming) return;
    setConfirming(true);
    try { if (await onRotate({ basis, note: note.trim() })) closeRotation(); } finally { setConfirming(false); }
  };
  return <details className="panel wp-settings-card wp-fixture-card" open={!!fixture || drawing || undefined}>
    <summary><span className="wp-settings-heading"><span className="wp-settings-title">定位特征</span><Badge tone={tone}>{status}</Badge><ChevronDown size={16} aria-hidden="true"/></span>
      <span className="wp-settings-summary">{fixture ? `框 ${fixture.rect.join(", ")} · 搜索 ±${fixture.searchRadiusPx} px · ${rotated ? `转角 ±${fixture.angleRangeDeg}°` : "只找平移"}` : "胶路相对工件特征判胶偏；不用时按静态示教线判"}</span>
    </summary>
    <div className="wp-settings-body" role="group" aria-label="定位特征设置">
      {!fixture ? <>
        <p className="wp-settings-hint">在冻结示教图上框一块与胶同面、刚性、不被喷嘴挡住的孔或角点，避开胶检测走廊与高光。</p>
        <Button variant={drawing ? "filled" : "tonal"} leadingIcon={<Crosshair size={15}/>} disabled={disabled} onClick={onDraw}>{drawing ? "在原图上拖出定位框" : "在原图上画定位框"}</Button>
      </> : <>
        {staleSource && <p className="wp-settings-hint c-warn" role="alert">{staleSource}</p>}
        <div className="wp-form-grid">
          {(["x", "y", "宽", "高"] as const).map((label, i) => <NumberField key={label} label={`定位框 ${label}`} value={fixture.rect[i]} unit="px" step={1} disabled={disabled} onChange={v => setRect(i, v)}/>)}
          <NumberField label="搜索半径" value={fixture.searchRadiusPx} unit="px" min={3} max={200} step={5} disabled={disabled} onChange={v => set({ searchRadiusPx: v })}/>
          <NumberField label="最低分数" value={fixture.minScore} min={.05} max={1} step={.05} disabled={disabled} onChange={v => set({ minScore: v })}/>
          <NumberField label="次峰比上限" value={fixture.maxPeakRatio} min={.05} max={1} step={.05} disabled={disabled} onChange={v => set({ maxPeakRatio: v })}/>
        </div>
        {rejected ? <p className="wp-settings-hint c-warn" role="alert">这个框没有写进示教：{rejected}</p> : invalid && <p className="wp-settings-hint c-warn" role="alert">{invalid}</p>}
        <div className="wp-fixture-readonly">
          <span>转角<strong>{rotated ? `±${fixture.angleRangeDeg}°（已开启，看不出角度按定位失败）` : "只找平移"}</strong></span>
          <span>示教余量 m<strong>{fixture.endMarginMm ?? 0} mm</strong></span>
          <span>两端搬移上限<strong>{fixture.maxEndShiftMm != null ? `${fixture.maxEndShiftMm} mm` : "临时默认"}</strong></span>
          <small className="muted">示教余量与两端搬移上限只能在验证页按验证数据的建议设置。</small>
        </div>
        {checks && <ul className="wp-fixture-checks" aria-label={checksSource === "trial" ? "示教图检查（试测）" : "定位框检查"}>
          {[...blocked, ...warned, ...checks.filter(c => c.passed)].map(item => <CheckRow key={item.id} item={item}/>)}
        </ul>}
        {checksSource !== "trial" && !invalid && <p className="wp-settings-hint">保存参数并试测后，示教图上的定位验收才算数。</p>}
        <Checkbox label="已确认定位框内没有喷嘴、胶枪等随相机不动的结构" checked={reviewed} disabled={disabled || editing} onCheckedChange={onReviewed}/>
        <div className="wp-actions">
          <Button size="sm" variant="tonal" disabled={disabled || !canTrial || trialRunning} title={canTrial ? "在样本库里同一拍照点、同一幅图的原图上试定位，只作诊断" : "先保存参数并试测"} onClick={onTrial}>{trialRunning ? "定位试测中…" : "定位试测"}</Button>
          <Button size="sm" variant="text" disabled={disabled} onClick={onDraw}>{drawing ? "拖出新框" : "重画定位框"}</Button>
          {rotated ? <Button size="sm" variant="text" disabled={disabled} onClick={() => set({ angleRangeDeg: 0 })}>关闭转角</Button>
            : <Button size="sm" variant="text" disabled={disabled || editing} title={editing ? "先试测保存当前修改" : undefined} onClick={() => setRotating(true)}>高级：开启转角…</Button>}
          <Button size="sm" variant="text" disabled={disabled} onClick={() => onChange(null)}>不用特征定位</Button>
        </div>
      </>}
    </div>
    {rotating && fixture && <Dialog open title="开启转角（高级）" closeDisabled={confirming} onOpenChange={open => { if (!open) closeRotation(); }} footer={<>
      <Button disabled={confirming} onClick={closeRotation}>取消</Button>
      <Button variant="filled" disabled={!basis || !note.trim() || confirming} onClick={() => void confirmRotation()}>确认开启 ±{ROTATION_PROBE_DEG}°</Button>
    </>}>
      <p>v1 默认只找平移。只有验证数据表明转动在最远站造成的误差超过位置公差的 1/5、全部验证帧（含亮度 ±40）都看得出角度，并且有独立于单个模板的证据时，才开转角。开启后看不出角度的帧按定位失败处理（ERR 92）。</p>
      {evidence ? <ul className="wp-fixture-evidence" aria-label="转角证据">
        <li>±3° 离线定位的 L·|θ| P95：<b>{evidence.leverThetaP95Mm != null ? `${evidence.leverThetaP95Mm.toFixed(3)} mm` : "—"}</b>（门限 公差/5 = {evidence.thresholdMm.toFixed(3)} mm，L = {evidence.leverMm.toFixed(1)} mm）{evidence.exceeds ? "，超过" : "，没超过"}</li>
        <li>看得出角度的帧：<b>{evidence.observableFrames} / {evidence.frames}</b>{evidence.allObservable ? "，全部" : "，不是全部"}</li>
        <li>前两个条件{evidence.autoConditions ? "都满足" : "没有都满足"}；第三个条件（独立证据）要在下面确认。</li>
      </ul> : <p className="c-warn">还没有当前修订的验证统计：先在验证页跑一次独立正常样本，再决定是否开转角。</p>}
      <Select label="独立证据" value={basis} options={[{ value: "", label: "请选择" }, ...bases]} disabled={confirming} onValueChange={value => setBasis(value as RotationConfirmation["basis"] | "")}/>
      <TextField label="说明（记进应用日志）" value={note} maxLength={200} disabled={confirming} onChange={e => setNote(e.target.value)}/>
    </Dialog>}
  </details>;
}
