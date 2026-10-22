import { useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { ArrowUpRight, ChevronDown } from "lucide-react";
import { Button, Checkbox, SegmentedControl, Select, TextField } from "qing-steel";
import type { DetectParams } from "../cycle/types";
import type { CameraTeachDraft, PathSegment, SegmentChecks } from "../plan/types";
import { KindChecksEditor } from "../plan/KindChecksEditor";
import { segmentKindLabel } from "../plan/vis";
import { Badge, NumberField } from "./components";
import { returnState } from "./returnNavigation";

interface Props {
  /** 这台相机的走廊草稿（走廊在画布上编辑，这里只改像素当量与检测参数） */
  teach: CameraTeachDraft;
  /** 配方默认检测参数：本相机没有自定义时用它 */
  defaults: DetectParams;
  /** 配方的段；检测项与允差按段配置 */
  segments: PathSegment[];
  /** 走廊编辑被锁（正在处理、候选待生效） */
  disabled: boolean;
  /** 检测项的编辑只在保存中或候选待生效时锁，不跟走廊草稿的未保存状态联动 */
  checksDisabled: boolean;
  onChange: (next: CameraTeachDraft) => void;
  onSegmentChecks: (segment: number, checks: SegmentChecks) => void;
}

/** 旧拍照点模型：位置名义值不为 0 时的量具确认件编号（定稿 §3）。段配方不用 gaugeRef，保留给旧配方的校验页面。 */
export function GaugeRef({ value, disabled, onChange }: { value?: string; disabled?: boolean; onChange: (value: string) => void }) {
  const missing = !value?.trim();
  return <div className="wp-gauge-ref"><TextField label="量具确认件编号" value={value ?? ""} maxLength={64} disabled={disabled} onChange={e => onChange(e.target.value)}/>
    {missing && <p className="wp-settings-hint c-warn" role="alert">名义值不为 0：必须填写用量具确认过的确认件编号</p>}</div>;
}

const polarity = { dark: "暗胶", light: "亮胶" };

export function TeachingSettings({ teach, defaults, segments, disabled, checksDisabled, onChange, onSegmentChecks }: Props) {
  const location = useLocation();
  const [segment, setSegment] = useState(segments[0]?.index ?? 1);
  const detect = teach.detect ?? defaults;
  const scaled = teach.mmPerPx != null && Number.isFinite(teach.mmPerPx) && teach.mmPerPx > 0;
  const setDetect = (value: DetectParams | null) => onChange({ ...teach, detect: value });
  const current = segments.find(item => item.index === segment) ?? segments[0];
  return <>
    <details className="panel wp-settings-card" open={!scaled}>
      <summary><span className="wp-settings-heading"><span className="wp-settings-title">像素当量</span><Badge tone={scaled ? "ok" : "warn"}>{scaled ? "已设置" : "未设置"}</Badge><ChevronDown size={16} aria-hidden="true"/></span>
        <span className="wp-settings-summary">{scaled ? `${Number(teach.mmPerPx!.toPrecision(6))} mm/px` : "尚未设置像素当量"}</span>
      </summary>
      <div className="wp-settings-body">
        <div className="wp-scale-field"><NumberField label="像素当量" value={teach.mmPerPx} unit="mm/px" min={.0001} max={10} step={.0005} onChange={v => onChange({ ...teach, mmPerPx: Number.isFinite(v) ? v : null })} disabled={disabled}/>
          <Link className="wp-calibration-link" to="/camera/calibration" state={returnState(location, "走廊示教")}>飞拍标定<ArrowUpRight size={13} aria-hidden="true"/></Link>
        </div>
        <p className="wp-settings-hint">每台相机各设一个当量，由飞拍标定得出；走廊上的测量长度、搜索带宽度都按它换算。</p>
      </div>
    </details>
    <details className="panel wp-settings-card">
      <summary><span className="wp-settings-heading"><span className="wp-settings-title">检测参数</span><Badge tone="neutral">{teach.detect ? "本相机自定义" : "配方默认"}</Badge><ChevronDown size={16} aria-hidden="true"/></span>
        <span className="wp-settings-summary">{polarity[detect.polarity]} · {detect.widthRange[0]}–{detect.widthRange[1]} mm</span>
      </summary>
      <div className="wp-settings-body">
        <Checkbox label="本相机自定义检测参数" checked={!!teach.detect} disabled={disabled} onCheckedChange={checked => setDetect(checked ? structuredClone(defaults) : null)}/>
        <div className="wp-form-grid"><NumberField label="搜索半宽" value={detect.searchMm} unit="mm" min={.1} step={.5} onChange={v => setDetect({ ...detect, searchMm: v })} disabled={disabled || !teach.detect}/>
          <Select label="极性" value={detect.polarity} disabled={disabled || !teach.detect} options={[{ value: "dark", label: "暗胶" }, { value: "light", label: "亮胶" }]} onValueChange={value => setDetect({ ...detect, polarity: value as DetectParams["polarity"] })}/>
          <NumberField label="胶宽下限" value={detect.widthRange[0]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [v, detect.widthRange[1]] })} disabled={disabled || !teach.detect}/>
          <NumberField label="胶宽上限" value={detect.widthRange[1]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [detect.widthRange[0], v] })} disabled={disabled || !teach.detect}/>
        </div>
        {teach.detect && <Button size="sm" disabled={disabled} onClick={() => setDetect(null)}>恢复配方默认检测参数</Button>}
      </div>
    </details>
    <details className="panel wp-settings-card">
      <summary><span className="wp-settings-heading"><span className="wp-settings-title">检测项与允差</span><Badge tone="neutral">按段</Badge><ChevronDown size={16} aria-hidden="true"/></span>
        <span className="wp-settings-summary">{current ? `段 ${current.index} · ${segmentKindLabel[current.kind]}` : "尚无段"}</span>
      </summary>
      <div className="wp-settings-body">
        {current ? <>
          {segments.length > 1 && <SegmentedControl size="sm" label="选择段" value={String(current.index)} options={segments.map(item => ({ value: String(item.index), label: `段 ${item.index} · ${segmentKindLabel[item.kind]}` }))} onValueChange={value => setSegment(Number(value))}/>}
          <KindChecksEditor value={current.checks} kind={current.kind} disabled={checksDisabled} onChange={checks => onSegmentChecks(current.index, checks)}/>
          <p className="wp-settings-hint">溢胶、气泡、拉丝：本轮只检溢胶（每帧最大胶宽超限判 NG）；气泡与拉丝没有算法，结果中标“未检项”。</p>
        </> : <p className="wp-settings-hint">配方还没有段，请先在“段列表”里添加。</p>}
      </div>
    </details>
  </>;
}
