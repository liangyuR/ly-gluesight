import { useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { ArrowUpRight, ChevronDown } from "lucide-react";
import { Button, Checkbox, SegmentedControl, Select } from "qing-steel";
import type { DetectParams } from "../cycle/types";
import type { CameraInspect, InspectWindow, PathSegment, SegmentChecks } from "../plan/types";
import { KindChecksEditor } from "../plan/KindChecksEditor";
import { segmentKindLabel } from "../plan/vis";
import { Badge, NumberField } from "./components";
import { returnState } from "./returnNavigation";
import { MAX_WINDOW_END_MM, windowError } from "./teach";

export type CameraParams = Pick<CameraInspect, "mmPerPx" | "detect" | "window" | "nozzle">;

const polarity = { dark: "暗胶", light: "亮胶" };

/** 相机参数（D-SB-45）：像素当量与检测参数，这台相机检测的各段走廊都按它们测量；headingSegments 是这台相机检测、开启了逐帧定向的段，derivedNozzle 是未标定时由走廊推出的出胶点。 */
export function CameraParamsSettings({ params, defaults, disabled, headingSegments = [], derivedNozzle = null, onChange }: { params: CameraParams; defaults: DetectParams; disabled: boolean; headingSegments?: number[]; derivedNozzle?: [number, number] | null; onChange: (next: CameraParams) => void }) {
  const location = useLocation();
  const detect = params.detect ?? defaults;
  const scaled = params.mmPerPx != null && Number.isFinite(params.mmPerPx) && params.mmPerPx > 0;
  const setDetect = (value: DetectParams | null) => onChange({ ...params, detect: value });
  return <>
    <details className="panel wp-settings-card" open={!scaled}>
      <summary><span className="wp-settings-heading"><span className="wp-settings-title">像素当量</span><Badge tone={scaled ? "ok" : "warn"}>{scaled ? "已设置" : "未设置"}</Badge><ChevronDown size={16} aria-hidden="true"/></span>
        <span className="wp-settings-summary">{scaled ? `${Number(params.mmPerPx!.toPrecision(6))} mm/px` : "尚未设置像素当量"}</span>
      </summary>
      <div className="wp-settings-body">
        <div className="wp-scale-field"><NumberField label="像素当量" value={params.mmPerPx} unit="mm/px" min={.0001} max={10} step={.0005} onChange={v => onChange({ ...params, mmPerPx: Number.isFinite(v) ? v : null })} disabled={disabled}/>
          <Link className="wp-calibration-link" to="/camera/calibration" state={returnState(location, "走廊示教")}>标定<ArrowUpRight size={13} aria-hidden="true"/></Link>
        </div>
        <p className="wp-settings-hint">每台相机一个当量，默认取设备标定；走廊上的测量长度、搜索带宽度都按它换算。</p>
      </div>
    </details>
    <details className="panel wp-settings-card">
      <summary><span className="wp-settings-heading"><span className="wp-settings-title">检测参数</span><Badge tone="neutral">{params.detect ? "本相机自定义" : "配方默认"}</Badge><ChevronDown size={16} aria-hidden="true"/></span>
        <span className="wp-settings-summary">{polarity[detect.polarity]} · {detect.widthRange[0]}–{detect.widthRange[1]} mm</span>
      </summary>
      <div className="wp-settings-body">
        <Checkbox label="本相机自定义检测参数" checked={!!params.detect} disabled={disabled} onCheckedChange={checked => setDetect(checked ? structuredClone(defaults) : null)}/>
        <div className="wp-form-grid"><NumberField label="搜索半宽" value={detect.searchMm} unit="mm" min={.1} step={.5} onChange={v => setDetect({ ...detect, searchMm: v })} disabled={disabled || !params.detect}/>
          <Select label="极性" value={detect.polarity} disabled={disabled || !params.detect} options={[{ value: "dark", label: "暗胶" }, { value: "light", label: "亮胶" }]} onValueChange={value => setDetect({ ...detect, polarity: value as DetectParams["polarity"] })}/>
          <NumberField label="胶宽下限" value={detect.widthRange[0]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [v, detect.widthRange[1]] })} disabled={disabled || !params.detect}/>
          <NumberField label="胶宽上限" value={detect.widthRange[1]} unit="mm" min={.1} step={.1} onChange={v => setDetect({ ...detect, widthRange: [detect.widthRange[0], v] })} disabled={disabled || !params.detect}/>
        </div>
        {params.detect && <Button size="sm" disabled={disabled} onClick={() => setDetect(null)}>恢复配方默认检测参数</Button>}
      </div>
    </details>
    <WindowSettings value={params.window ?? null} headingSegments={headingSegments} disabled={disabled} onChange={window => onChange({ ...params, window })} />
    <NozzleSettings value={params.nozzle ?? null} derived={derivedNozzle} headingSegments={headingSegments} disabled={disabled} onClear={() => onChange({ ...params, nozzle: null })} />
  </>;
}

/** 出胶点（D-SB-47）：在开启定向的段的示教帧上用“标出胶点”点选；这里只看状态和清除标定。 */
function NozzleSettings({ value, derived, headingSegments, disabled, onClear }: { value: [number, number] | null; derived: [number, number] | null; headingSegments: number[]; disabled: boolean; onClear: () => void }) {
  const [badge, tone]: [string, "ok" | "info" | "warn" | "neutral"] = value ? ["已标定", "ok"] : derived ? ["按走廊推算", "info"] : headingSegments.length ? ["未标定", "warn"] : ["未标定", "neutral"];
  const point = value ?? derived;
  return <details className="panel wp-settings-card">
    <summary><span className="wp-settings-heading"><span className="wp-settings-title">出胶点</span><Badge tone={tone}>{badge}</Badge><ChevronDown size={16} aria-hidden="true"/></span>
      <span className="wp-settings-summary">{point ? `(${Math.round(point[0])}, ${Math.round(point[1])}) px` : headingSegments.length ? "到开启定向的段的示教帧上标定" : "只用于开启逐帧定向的段"}</span>
    </summary>
    <div className="wp-settings-body">
      {value && <Button size="sm" disabled={disabled} onClick={onClear}>清除标定，改按走廊起点推算</Button>}
      <p className="wp-settings-hint">逐帧定向以出胶点为圆心找胶条方向、量检测窗口。在开启逐帧定向的段的示教帧上选“标出胶点”，点胶嘴出胶处；标定后各段走廊从哪头画起都可以，示教方向取背离出胶点的方向。未标定时取这台相机开启定向的各段走廊起点的平均，起点相差超过 1 mm 会报“出胶点不一致”。出胶点须在示教帧内。</p>
    </div>
  </details>;
}

/** 逐帧定向的检测窗口（D-SB-47）：起点、长度都空为未设置；不合法时只提示，不写入配方。 */
function WindowSettings({ value, headingSegments, disabled, onChange }: { value: InspectWindow | null; headingSegments: number[]; disabled: boolean; onChange: (next: InspectWindow | null) => void }) {
  const [start, setStart] = useState(value?.startMm ?? NaN);
  const [length, setLength] = useState(value?.lengthMm ?? NaN);
  const empty = Number.isNaN(start) && Number.isNaN(length);
  const error = empty ? "" : windowError({ startMm: start, lengthMm: length });
  const needed = headingSegments.length > 0;
  const update = (nextStart: number, nextLength: number) => {
    setStart(nextStart); setLength(nextLength);
    if (Number.isNaN(nextStart) && Number.isNaN(nextLength)) { if (value) onChange(null); return; }
    const next = { startMm: nextStart, lengthMm: nextLength };
    if (!windowError(next)) onChange(next);
  };
  const [badge, tone]: [string, "ok" | "warn" | "neutral"] = value ? ["已设置", "ok"] : needed ? ["需设置", "warn"] : ["未设置", "neutral"];
  return <details className="panel wp-settings-card" open={needed && !value}>
    <summary><span className="wp-settings-heading"><span className="wp-settings-title">检测窗口</span><Badge tone={tone}>{badge}</Badge><ChevronDown size={16} aria-hidden="true"/></span>
      <span className="wp-settings-summary">{value ? `距出胶点 ${value.startMm}–${value.startMm + value.lengthMm} mm` : needed ? `段 ${headingSegments.join("、")} 开启了逐帧定向` : "只用于开启逐帧定向的段"}</span>
    </summary>
    <div className="wp-settings-body">
      <div className="wp-form-grid"><NumberField label="窗口起点" value={start} unit="mm" min={0} max={MAX_WINDOW_END_MM} step={.5} onChange={v => update(v, length)} disabled={disabled}/>
        <NumberField label="窗口长度" value={length} unit="mm" min={0} max={MAX_WINDOW_END_MM} step={.5} onChange={v => update(start, v)} disabled={disabled}/>
      </div>
      {error && <p className="c-warn" role="alert">{error}，未保存</p>}
      <p className="wp-settings-hint">逐帧定向的段每帧只测沿胶路距出胶点 起点 到 起点 + 长度 的一段：起点避开胶嘴遮挡与刚挤出的胶堆，长度建议不小于每帧胶条后移量（20 fps 下 NEW-14 估计约 20 mm），否则相邻帧之间有一截胶测不到。{needed ? "这台相机检测的开启逐帧定向的段需要设置。" : "没有开启逐帧定向的段时可不填。"}两项都清空即为未设置。</p>
    </div>
  </details>;
}

/** 按段的检测项与允差；只给一段时不显示段选择。 */
export function SegmentChecksSettings({ segments, disabled, onChange }: { segments: PathSegment[]; disabled: boolean; onChange: (segment: number, checks: SegmentChecks) => void }) {
  const [segment, setSegment] = useState(segments[0]?.index ?? 1);
  const current = segments.find(item => item.index === segment) ?? segments[0];
  return <details className="panel wp-settings-card">
    <summary><span className="wp-settings-heading"><span className="wp-settings-title">检测项与允差</span><Badge tone="neutral">按段</Badge><ChevronDown size={16} aria-hidden="true"/></span>
      <span className="wp-settings-summary">{current ? `段 ${current.index} · ${segmentKindLabel[current.kind]}` : "尚无段"}</span>
    </summary>
    <div className="wp-settings-body">
      {current ? <>
        {segments.length > 1 && <SegmentedControl size="sm" label="选择段" value={String(current.index)} options={segments.map(item => ({ value: String(item.index), label: `段 ${item.index} · ${segmentKindLabel[item.kind]}` }))} onValueChange={value => setSegment(Number(value))}/>}
        <KindChecksEditor value={current.checks} kind={current.kind} disabled={disabled} onChange={checks => onChange(current.index, checks)}/>
        <p className="wp-settings-hint">溢胶、气泡、拉丝：本轮只检溢胶（每帧最大胶宽超限判 NG）；气泡与拉丝没有算法，结果中标“未检项”。</p>
      </> : <p className="wp-settings-hint">配方还没有段，请先在“段列表”里添加。</p>}
    </div>
  </details>;
}
