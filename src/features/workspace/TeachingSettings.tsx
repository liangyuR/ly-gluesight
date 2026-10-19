import { useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { ArrowUpRight, ChevronDown } from "lucide-react";
import { Button, Checkbox, Select } from "qing-steel";
import type { DetectParams, JudgeParams, ShotLimits } from "../cycle/types";
import type { CalibrationState, ShotTeach } from "./types";
import { Badge, NumberField } from "./components";
import { returnState } from "./returnNavigation";

interface Props {
  teach: ShotTeach;
  savedMmPerPx: number | null;
  defaults: DetectParams;
  defaultLimits: ShotLimits;
  calibration: CalibrationState;
  disabled: boolean;
  editing: boolean;
  onChange: (next: ShotTeach) => void;
  onRecord: (reference: number, measured: number, reuse: boolean) => Promise<boolean>;
}

const calibrationLabels = { unverified: "未验证", stale: "需重新验证", verified: "已验证", failed: "验证未通过", sampleSet: "样本集声明" };
const polarity = { dark: "暗胶", light: "亮胶" };

export function TeachingSettings({ teach, savedMmPerPx, defaults, defaultLimits, calibration, disabled, editing, onChange, onRecord }: Props) {
  const location = useLocation();
  const [reference, setReference] = useState(10);
  const [measured, setMeasured] = useState(NaN);
  const [reuse, setReuse] = useState(false);
  const scaleChanged = teach.mmPerPx === null || savedMmPerPx === null ? teach.mmPerPx !== savedMmPerPx : Math.fround(teach.mmPerPx) !== Math.fround(savedMmPerPx);
  const status: CalibrationState = scaleChanged ? { state: "stale" } : calibration;
  const validMeasurement = Number.isFinite(reference) && reference > 0 && Number.isFinite(measured) && measured > 0;
  const recordedError = status.state === "verified" || status.state === "failed" ? status.errorMm : null;
  const error = validMeasurement ? measured - reference : recordedError;
  const detect = teach.detect ?? defaults;
  const limits = teach.limits ?? defaultLimits;
  const setDetect = (value: DetectParams | undefined) => {
    const next = { ...teach };
    if (value) next.detect = value; else delete next.detect;
    onChange(next);
  };
  const setLimits = (value: ShotLimits | undefined) => {
    const next = { ...teach };
    if (value) next.limits = value; else delete next.limits;
    onChange(next);
  };
  const record = async () => {
    if (disabled || editing || !validMeasurement) return;
    if (await onRecord(reference, measured, reuse)) { setMeasured(NaN); setReuse(false); }
  };
  const limitGroup = (key: "position" | "width", title: string) => {
    const value = teach.limits?.[key];
    return <div key={key} className="wp-limit-group"><Checkbox label={title + "判定"} checked={!!value} disabled={disabled} onCheckedChange={checked => teach.limits && setLimits({ ...teach.limits, [key]: checked ? structuredClone(defaultLimits[key] ?? { nominal: 0, tolUpper: .1, tolLower: .1, absMin: -1, absMax: 1, maxExcursionLen: 1 }) : null })}/>
      {value && <div className="wp-form-grid">{([["nominal", "名义值"], ["tolUpper", "上公差"], ["tolLower", "下公差"], ["absMin", "绝对下限"], ["absMax", "绝对上限"], ["maxExcursionLen", "允许超差长度"]] as [keyof JudgeParams, string][]).map(([field, label]) => <NumberField key={field} label={title + label} value={value[field]} unit="mm" step={.1} disabled={disabled} onChange={v => teach.limits && setLimits({ ...teach.limits, [key]: { ...value, [field]: v } })}/>)}</div>}
    </div>;
  };
  return <>
    <details className="panel wp-settings-card" open={status.state === "stale" || status.state === "failed"}>
      <summary><span className="wp-settings-heading"><span className="wp-settings-title">标定</span><Badge tone={status.state === "verified" ? "ok" : status.state === "failed" ? "ng" : status.state === "sampleSet" ? "info" : "warn"}>{calibrationLabels[status.state]}</Badge><ChevronDown size={16} aria-hidden="true"/></span>
        <span className="wp-settings-summary">{teach.mmPerPx != null && Number.isFinite(teach.mmPerPx) && teach.mmPerPx > 0 ? `${Number(teach.mmPerPx.toPrecision(6))} mm/px${status.state === "unverified" ? " · 手填，未经标定验证" : ""}` : "尚未设置像素当量"}{recordedError != null && ` · 误差 ${recordedError.toFixed(3)} mm`}</span>
      </summary>
      <div className="wp-settings-body">
        <div className="wp-scale-field"><NumberField label="像素当量" value={teach.mmPerPx} unit="mm/px" min={.0001} max={10} step={.0005} onChange={v => onChange({ ...teach, mmPerPx: Number.isFinite(v) ? v : null })} disabled={disabled}/>
          <Link className="wp-calibration-link" to="/camera/calibration" state={returnState(location, "单帧示教")}>飞拍标定<ArrowUpRight size={13} aria-hidden="true"/></Link>
        </div>
        {scaleChanged && <p className="wp-settings-hint c-warn" role="status">当量已修改，先运行试测保存，再重新验证。</p>}
        {calibration.state === "sampleSet" ? <p className="wp-settings-hint">样本集 {calibration.setId} 声明：{calibration.mmPerPx} mm/px。{scaleChanged ? "保存后需人工验证。" : "此来源不等同于人工验证。"}</p> : <>
          <div className="wp-form-grid"><NumberField label="参考尺寸" value={reference} unit="mm" min={.01} step={.01} onChange={setReference} disabled={disabled}/><NumberField label="实测尺寸" value={measured} unit="mm" min={.01} step={.01} onChange={setMeasured} disabled={disabled}/></div>
          <div className="wp-calibration-error" role="status"><div><span>{validMeasurement ? "待记录误差" : recordedError != null ? "已记录误差" : "验证误差"}</span><strong>{error != null ? `${error > 0 ? "+" : ""}${error.toFixed(3)} mm` : "—"}</strong></div><span>目标 ±0.1 mm</span></div>
          <div className="wp-calibration-reuse"><Checkbox label="复用到其他适用拍照点" checked={reuse} disabled={disabled || editing} onCheckedChange={setReuse}/>
            <details><summary>适用条件</summary><p>仅限同设备、同图且成像几何适用的其他拍照点；勾选表示已人工确认。</p></details>
          </div>
          <Button variant="filled" disabled={disabled || editing || !validMeasurement} onClick={() => void record()}>记录验证</Button>
          {!disabled && !scaleChanged && (editing || !validMeasurement) && <p className="wp-settings-hint">{editing ? "先运行试测保存示教修改。" : "填写参考与实测尺寸后可记录。"}</p>}
        </>}
      </div>
    </details>
    <details className="panel wp-settings-card">
      <summary><span className="wp-settings-heading"><span className="wp-settings-title">检测参数</span><Badge tone="neutral">{teach.detect ? "本图自定义" : "配方默认"}</Badge><ChevronDown size={16} aria-hidden="true"/></span>
        <span className="wp-settings-summary">{polarity[detect.polarity]} · {detect.widthRange[0]}–{detect.widthRange[1]} mm</span>
      </summary>
      <div className="wp-settings-body">
        <Checkbox label="本图自定义检测参数" checked={!!teach.detect} disabled={disabled} onCheckedChange={checked => setDetect(checked ? structuredClone(defaults) : undefined)}/>
        <div className="wp-form-grid"><NumberField label="搜索半宽" value={detect.searchMm} unit="mm" min={.1} step={.5} onChange={v => setDetect({ ...detect, searchMm: v })} disabled={disabled || !teach.detect}/>
          <Select label="极性" value={detect.polarity} disabled={disabled || !teach.detect} options={[{ value: "dark", label: "暗胶" }, { value: "light", label: "亮胶" }]} onValueChange={value => setDetect({ ...detect, polarity: value as DetectParams["polarity"] })}/>
          <NumberField label="胶宽下限" value={detect.widthRange[0]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [v, detect.widthRange[1]] })} disabled={disabled || !teach.detect}/>
          <NumberField label="胶宽上限" value={detect.widthRange[1]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [detect.widthRange[0], v] })} disabled={disabled || !teach.detect}/>
        </div>
        {teach.detect && <Button size="sm" disabled={disabled} onClick={() => setDetect(undefined)}>恢复配方默认检测参数</Button>}
      </div>
    </details>
    <details className="panel wp-settings-card">
      <summary><span className="wp-settings-heading"><span className="wp-settings-title">判定限值</span><Badge tone="neutral">{teach.limits ? "本图自定义" : "配方默认"}</Badge><ChevronDown size={16} aria-hidden="true"/></span>
        <span className="wp-settings-summary">{[limits.position && "位置", limits.width && "胶宽", "缺胶"].filter(Boolean).join(" · ")}</span>
      </summary>
      <div className="wp-settings-body">
        <Checkbox label="本图自定义判定限值" checked={!!teach.limits} disabled={disabled} onCheckedChange={checked => setLimits(checked ? structuredClone(defaultLimits) : undefined)}/>
        {teach.limits ? <>{limitGroup("position", "位置")}{limitGroup("width", "胶宽")}<div className="wp-form-grid"><NumberField label="允许连续缺胶长度" value={limits.maxGapLen} unit="mm" min={0} step={.1} disabled={disabled} onChange={v => setLimits({ ...limits, maxGapLen: v })}/><NumberField label="最低有胶比例" value={limits.minPresent} min={0} max={1} step={.05} disabled={disabled} onChange={v => setLimits({ ...limits, minPresent: v })}/></div><Button size="sm" disabled={disabled} onClick={() => setLimits(undefined)}>恢复配方默认判定限值</Button></> : <div className="wp-default-limits"><span>允许连续缺胶<strong>{limits.maxGapLen} mm</strong></span><span>最低有胶比例<strong>{Math.round(limits.minPresent * 100)}%</strong></span></div>}
      </div>
    </details>
  </>;
}
