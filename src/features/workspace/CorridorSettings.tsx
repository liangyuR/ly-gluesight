import { useEffect, useState, type ReactNode } from "react";
import { Link, useLocation } from "react-router-dom";
import { ArrowUpRight } from "lucide-react";
import { Accordion, Button, SegmentedControl, Select, Switch } from "qing-steel";
import { DetectEditor } from "../recipe";
import { recipeApi } from "../cycle/api";
import type { DetectParams } from "../cycle/types";
import type { CameraInspect, EdgeParams, EdgePolarity, ExpertParams, ExpertSpec, InspectWindow, OffsetMeasure, PathSegment } from "../plan/types";
import { KindChecksEditor } from "../plan/KindChecksEditor";
import { segmentKindLabel } from "../plan/vis";
import { Badge, NumberField } from "./components";
import { returnState } from "./returnNavigation";
import { applyToKind, DEFAULT_EDGE, DEFAULT_HEADING, segmentIssues } from "./segmentEdit";
import { MAX_WINDOW_END_MM, windowError } from "./teach";
import { segmentCameras } from "./workbenchDraft";

export type CameraParams = Pick<CameraInspect, "mmPerPx" | "detect" | "window" | "nozzle" | "expert">;
type Tone = "ok" | "info" | "warn" | "neutral";

const polarity = { dark: "暗胶", light: "亮胶" };
const noChanges = new Set<string>();
const scaleOf = (mmPerPx: number | null | undefined) => mmPerPx != null && Number.isFinite(mmPerPx) && mmPerPx > 0 ? Number(mmPerPx.toPrecision(6)) : null;

/** 右栏卡片：标题行带状态徽标与展开箭头，折叠时多一行摘要，看得出能点开。 */
function SettingsCard({ title, badge, tone = "neutral", summary, open, children }: { title: string; badge?: ReactNode; tone?: Tone; summary?: ReactNode; open?: boolean; children: ReactNode }) {
  return <Accordion className="wp-settings-card" collapsible forceMount ignoreFieldsetDisabled defaultValue={open ? "settings" : undefined} items={[{
    value: "settings",
    title: <span><span className="wp-settings-heading"><span className="wp-settings-title">{title}</span>{badge != null && <Badge tone={tone}>{badge}</Badge>}</span>{summary != null && <span className="wp-settings-summary">{summary}</span>}</span>,
    content: <div className="wp-settings-body">{children}</div>,
  }]} />;
}

const Scope = ({ children }: { children: ReactNode }) => <span className="wp-scope">{children}</span>;

/**
 * 相机视图“测量”（D-SB-45）：像素当量、检测参数与配方站距。检测参数取配方默认时，这里改的就是配方默认值，
 * 同时作用于 `sharing` 里的其他相机；切到本相机自定义时从配方默认复制一份。
 * 站距先经 `spacingError` 检查（已保存的走廊要够三站），不通过时只提示、不写配方。
 */
export function MeasureCard({ camera, params, defaults, spacing, sharing, disabled, spacingError, onParams, onDefaults, onSpacing }: {
  camera: string; params: CameraParams; defaults: DetectParams; spacing: number; sharing: string[]; disabled: boolean;
  spacingError: (value: number) => string; onParams: (next: CameraParams) => void; onDefaults: (next: DetectParams) => void; onSpacing: (value: number) => void;
}) {
  const location = useLocation();
  const [spacingDraft, setSpacingDraft] = useState(spacing);
  const spacingIssue = spacingError(spacingDraft);
  const changeSpacing = (value: number) => {
    setSpacingDraft(value);
    if (!spacingError(value)) onSpacing(value);
  };
  const custom = params.detect != null;
  const detect = params.detect ?? defaults;
  const scale = scaleOf(params.mmPerPx);
  return <SettingsCard title="测量" badge={scale ? "当量已设" : "缺当量"} tone={scale ? "ok" : "warn"} open
    summary={`${scale ? `${scale} mm/px` : "缺像素当量"} · ${custom ? "本相机自定义" : "配方默认"} · ${polarity[detect.polarity]} ${detect.widthRange[0]}–${detect.widthRange[1]} mm`}>
    <fieldset className="wp-plain-fieldset wp-settings-fields" disabled={disabled}>
      <div className="wp-scale-field"><NumberField label="像素当量" value={params.mmPerPx} unit="mm/px" min={.0001} max={10} step={.0005} onChange={v => onParams({ ...params, mmPerPx: Number.isFinite(v) ? v : null })} disabled={disabled}/>
        <Link className="wp-calibration-link" to="/camera/calibration" state={returnState(location, "走廊示教")}>标定<ArrowUpRight size={13} aria-hidden="true"/></Link>
      </div>
      <SegmentedControl size="sm" label="检测参数来源" value={custom ? "camera" : "recipe"} disabled={disabled}
        options={[{ value: "recipe", label: "配方默认" }, { value: "camera", label: "本相机自定义" }]}
        onValueChange={value => onParams({ ...params, detect: value === "camera" ? structuredClone(defaults) : null })}/>
      <DetectEditor value={detect} changed={noChanges} disabled={disabled} onChange={custom ? next => onParams({ ...params, detect: next }) : onDefaults}/>
      <p className="wp-settings-hint">{custom ? `只用于 ${camera}；切回配方默认会丢掉这里的值。` : `改的是配方默认值${sharing.length ? `，${sharing.join("、")} 也在用` : ""}。`}</p>
      <NumberField label="站距" unit="mm" value={spacingDraft} min={.1} max={10} step={.5} onChange={changeSpacing} disabled={disabled}/>
      {spacingIssue && <p className="c-warn" role="alert">{spacingIssue}{spacingDraft === spacing ? "" : "，未保存"}</p>}
      <p className="wp-settings-hint"><Scope>配方共用</Scope> 沿走廊每隔多少 mm 量一站；当量换算走廊长度与搜索带宽度。改动只影响此后的测量。</p>
    </fieldset>
  </SettingsCard>;
}

let expertSpecs: Promise<Record<string, ExpertSpec>> | null = null;
function useExpertSpecs() {
  const [specs, setSpecs] = useState<Record<string, ExpertSpec>>({});
  useEffect(() => {
    let alive = true;
    void (expertSpecs ??= recipeApi.expertSpecs()).then(value => alive && setSpecs(value), () => { expertSpecs = null; });
    return () => { alive = false; };
  }, []);
  return specs;
}

const without = (params: ExpertParams, name: string) => Object.fromEntries(Object.entries(params).filter(([key]) => key !== name));

/**
 * 相机视图“专家参数”（LF-5）：测量图模板里开放的算子门限，按 lyFlow 原始单位填。改“本相机”只作用于这台相机，
 * 改“配方默认”作用于所有没有覆盖这一项的相机；越界或空值只提示，不写候选配方。
 */
export function ExpertCard({ camera, own, defaults, disabled, onCamera, onDefaults }: {
  camera: string; own: ExpertParams; defaults: ExpertParams; disabled: boolean;
  onCamera: (next: ExpertParams) => void; onDefaults: (next: ExpertParams) => void;
}) {
  const specs = useExpertSpecs();
  const [scope, setScope] = useState<"camera" | "recipe">("camera");
  const [error, setError] = useState("");
  const target = scope === "camera" ? own : defaults;
  const write = scope === "camera" ? onCamera : onDefaults;
  const changed = Object.keys({ ...defaults, ...own }).length;
  const groups = Object.entries(specs).reduce<Record<string, [string, ExpertSpec][]>>((out, entry) => ({ ...out, [entry[1].group]: [...(out[entry[1].group] ?? []), entry] }), {});
  const set = (name: string, spec: ExpertSpec, value: number) => {
    if (!Number.isFinite(value) || value < spec.min || value > spec.max) { setError(`${spec.label}需在 ${spec.min}–${spec.max} 之间，未保存`); return; }
    setError("");
    write({ ...target, [name]: value });
  };
  return <SettingsCard title="专家参数" badge={changed ? `${changed} 项已改` : "默认"} tone={changed ? "info" : "neutral"}
    summary={changed ? `本相机 ${Object.keys(own).length} 项 · 配方默认 ${Object.keys(defaults).length} 项` : "门限均为模板默认值"}>
    <fieldset className="wp-plain-fieldset wp-settings-fields" disabled={disabled}>
      <SegmentedControl size="sm" label="修改范围" value={scope} disabled={disabled}
        options={[{ value: "camera", label: `仅 ${camera}` }, { value: "recipe", label: "配方默认" }]} onValueChange={value => setScope(value as "camera" | "recipe")}/>
      {Object.entries(groups).map(([group, items]) => <div key={group} className="wp-settings-group" role="group" aria-label={group}>
        <span className="wp-settings-label">{group}</span>
        {items.map(([name, spec]) => {
          const value = own[name] ?? defaults[name] ?? spec.default;
          const source = name in own ? camera : name in defaults ? "配方默认" : "";
          return <div key={name} className="wp-expert-row" title={spec.doc}>
            <NumberField label={spec.label} unit={spec.unit} value={target[name] ?? value} min={spec.min} max={spec.max} step={spec.step} disabled={disabled} onChange={v => set(name, spec, v)}/>
            {source && <Scope>{source}</Scope>}
            {name in target && <Button variant="text" size="sm" disabled={disabled} onClick={() => write(without(target, name))}>恢复</Button>}
          </div>;
        })}
      </div>)}
      {error && <p className="c-warn" role="alert">{error}</p>}
      <p className="wp-settings-hint">作用于 lyFlow 算子或 GlueSight 判定（px 等原始单位）；悬停看说明。改动后请重新试测，发布前跑样本验证。</p>
    </fieldset>
  </SettingsCard>;
}

const headingSummary = (params: CameraParams, derived: [number, number] | null) =>
  `${params.window ? `窗口 ${params.window.startMm}–${params.window.startMm + params.window.lengthMm} mm` : "窗口未设置"} · ${params.nozzle ? "出胶点已标定" : derived ? "出胶点按走廊推算" : "出胶点未标定"}`;

/** 相机视图“逐帧定向”（D-SB-47）：这台相机各段共用的检测窗口与出胶点；有段开启定向却缺窗口时展开。 */
export function CameraHeadingCard({ camera, params, headingSegments, derivedNozzle, disabled, onChange }: {
  camera: string; params: CameraParams; headingSegments: number[]; derivedNozzle: [number, number] | null; disabled: boolean; onChange: (next: CameraParams) => void;
}) {
  const needed = headingSegments.length > 0;
  const missing = needed && !params.window;
  return <SettingsCard title="逐帧定向" badge={missing ? "窗口需设置" : needed ? `段 ${headingSegments.join("、")} 开启` : "未使用"} tone={missing ? "warn" : "neutral"} open={missing}
    summary={headingSummary(params, derivedNozzle)}>
    <HeadingShared camera={camera} params={params} derivedNozzle={derivedNozzle} needed={needed} disabled={disabled} onChange={onChange}/>
    <p className="wp-settings-hint">开关与方向容差按段设置：在左侧选段。{needed ? "" : "没有段开启逐帧定向时可不填。"}</p>
  </SettingsCard>;
}

function HeadingShared({ camera, params, derivedNozzle, needed, disabled, onChange }: {
  camera: string; params: CameraParams; derivedNozzle: [number, number] | null; needed: boolean; disabled: boolean; onChange: (next: CameraParams) => void;
}) {
  return <>
    <WindowFields camera={camera} value={params.window ?? null} needed={needed} disabled={disabled} onChange={window => onChange({ ...params, window })}/>
    <NozzleStatus camera={camera} value={params.nozzle ?? null} derived={derivedNozzle} needed={needed} disabled={disabled} onClear={() => onChange({ ...params, nozzle: null })}/>
  </>;
}

/** 检测窗口：起点、长度都空为未设置；不合法时只提示，不写入配方。 */
function WindowFields({ camera, value, needed, disabled, onChange }: { camera: string; value: InspectWindow | null; needed: boolean; disabled: boolean; onChange: (next: InspectWindow | null) => void }) {
  const [start, setStart] = useState(value?.startMm ?? NaN);
  const [length, setLength] = useState(value?.lengthMm ?? NaN);
  const empty = Number.isNaN(start) && Number.isNaN(length);
  const error = empty ? "" : windowError({ startMm: start, lengthMm: length });
  const update = (nextStart: number, nextLength: number) => {
    setStart(nextStart); setLength(nextLength);
    if (Number.isNaN(nextStart) && Number.isNaN(nextLength)) { if (value) onChange(null); return; }
    const next = { startMm: nextStart, lengthMm: nextLength };
    if (!windowError(next)) onChange(next);
  };
  return <div className="wp-settings-group" role="group" aria-label="检测窗口">
    <span className="wp-settings-label">检测窗口<Scope>{camera} 各段共用</Scope>{needed && !value && <Badge tone="warn">需设置</Badge>}</span>
    <div className="wp-form-grid"><NumberField label="窗口起点" value={start} unit="mm" min={0} max={MAX_WINDOW_END_MM} step={.5} onChange={v => update(v, length)} disabled={disabled}/>
      <NumberField label="窗口长度" value={length} unit="mm" min={0} max={MAX_WINDOW_END_MM} step={.5} onChange={v => update(start, v)} disabled={disabled}/>
    </div>
    {error && <p className="c-warn" role="alert">{error}，未保存</p>}
    <p className="wp-settings-hint">每帧只测距出胶点 起点 到 起点 + 长度 的一段（图上环带）。起点避开胶嘴遮挡，长度不小于每帧胶条后移量（20 fps 约 20 mm）；两项都清空即未设置。</p>
  </div>;
}

/** 出胶点：在开启定向的段的示教帧上用“标出胶点”点选，这里只看状态和清除标定。 */
function NozzleStatus({ camera, value, derived, needed, disabled, onClear }: { camera: string; value: [number, number] | null; derived: [number, number] | null; needed: boolean; disabled: boolean; onClear: () => void }) {
  const [badge, tone]: [string, Tone] = value ? ["已标定", "ok"] : derived ? ["按走廊推算", "info"] : ["未标定", needed ? "warn" : "neutral"];
  const point = value ?? derived;
  return <div className="wp-settings-group" role="group" aria-label="出胶点">
    <span className="wp-settings-label">出胶点<Scope>{camera} 各段共用</Scope><Badge tone={tone}>{badge}</Badge></span>
    {point && <span className="wp-nozzle-point"><span className="plan-mono">({Math.round(point[0])}, {Math.round(point[1])}) px</span>
      {value && <Button variant="text" size="sm" disabled={disabled} onClick={onClear}>清除标定</Button>}</span>}
    <p className="wp-settings-hint">在开启定向的段的示教帧上选“标出胶点”，点胶嘴出胶处。未标定时取开启定向的各段走廊起点的平均，相差超过 1 mm 报“出胶点不一致”。</p>
  </div>;
}

/** 段参数的本地草稿：有空值或越界时只留在卡片里提示，不写候选配方（自动保存会被后端校验拒绝）。 */
function useSegmentDraft(segments: PathSegment[], index: number, onChange: (segments: PathSegment[]) => void) {
  const saved = segments.find(item => item.index === index);
  const [draft, setDraft] = useState<PathSegment | null>(null);
  const current = draft?.index === index ? draft : saved;
  const issues = draft?.index === index ? segmentIssues([draft]) : [];
  const patch = (change: Partial<PathSegment>) => {
    if (!current) return;
    const next = { ...current, ...change };
    if (segmentIssues([next]).length) { setDraft(next); return; }
    setDraft(null);
    onChange(segments.map(item => item.index === index ? next : item));
  };
  return { saved, current, issues, patch, dirty: !!draft };
}

/**
 * 段视图“逐帧定向”（D-SB-47）：开关与方向容差属于本段；检测窗口与出胶点是检测相机各段共用的，
 * 放在一起对照示教帧上的叠加调整。`sharedLocked` 非空时共用参数只读并说明原因。
 */
export function SegmentHeadingCard({ segments, index, camera, params, derivedNozzle, disabled, sharedLocked, onSegments, onParams }: {
  segments: PathSegment[]; index: number; camera: string; params: CameraParams; derivedNozzle: [number, number] | null; disabled: boolean; sharedLocked: string;
  onSegments: (segments: PathSegment[]) => void; onParams: (next: CameraParams) => void;
}) {
  const { saved, current, issues, patch } = useSegmentDraft(segments, index, onSegments);
  if (!saved || !current) return null;
  const heading = current.heading;
  const absoluteX = current.checks.offset?.enabled && (current.offsetRef?.measure ?? "absoluteX") === "absoluteX";
  return <SettingsCard title="逐帧定向" badge={saved.heading ? `开 ±${saved.heading.toleranceDeg}°` : "关"} tone={saved.heading ? "info" : "neutral"} open
    summary={saved.heading ? headingSummary(params, derivedNozzle) : "关闭：按示教走廊测量"}>
    <Switch label="逐帧定向" aria-label={`段 ${index} 逐帧定向`} checked={!!heading} disabled={disabled} onCheckedChange={on => patch({ heading: on ? { ...DEFAULT_HEADING } : null })}/>
    {heading ? <>
      <NumberField label="方向容差" unit="°" value={heading.toleranceDeg} min={0} max={45} step={1} disabled={disabled} onChange={v => patch({ heading: { toleranceDeg: v } })}/>
      {issues.length > 0 && <p className="c-warn" role="alert">{issues.join("；")}，未保存</p>}
      <p className="wp-settings-hint">方向范围 = 示教方向 ± 方向容差（0–45°），即示教帧上的黄色实线扇形，保存后更新；虚线扇形为搜索扇区，由相邻段的方向范围推出。</p>
      {absoluteX && <p className="c-warn">定向测量时胶路由图像拟合，绝对 x 恒为 0：本段胶偏请在“检测项”里改用“胶条到边缘距离”。</p>}
      {sharedLocked && <p className="wp-settings-hint">{sharedLocked}</p>}
      <HeadingShared camera={camera} params={params} derivedNozzle={derivedNozzle} needed disabled={disabled || !!sharedLocked} onChange={onParams}/>
    </> : <p className="wp-settings-hint">开启后每帧按图像找胶条方向，只测检测窗口内的一段，不再依赖按帧号选走廊。</p>}
  </SettingsCard>;
}

function checksSummary(segment: PathSegment) {
  const { width, break: gap, overflow, offset } = segment.checks;
  const parts = [
    width.enabled && `胶宽 ${width.nominalMm}${width.tolUpper === width.tolLower ? `±${width.tolUpper}` : ` +${width.tolUpper}/−${width.tolLower}`}`,
    gap.enabled && "断胶",
    overflow.enabled && `溢胶 ≤${overflow.maxWidthMm}`,
    offset?.enabled && `胶偏 ${segment.offsetRef?.measure === "toEdge" ? "到边缘" : "绝对 x"}`,
  ].filter((part): part is string => !!part);
  return { count: parts.length, text: parts.length ? parts.join(" · ") : "全部关闭" };
}

/** 段视图“检测项”：胶宽、断胶、溢胶、胶偏与胶偏参考，可把检测项套用到其余同类段。 */
export function SegmentChecksCard({ segments, index, cameras, disabled, onChange }: { segments: PathSegment[]; index: number; cameras: string[]; disabled: boolean; onChange: (segments: PathSegment[]) => void }) {
  const { saved, current, issues, patch, dirty } = useSegmentDraft(segments, index, onChange);
  if (!saved || !current) return null;
  const summary = checksSummary(saved);
  const at = segments.findIndex(item => item.index === index);
  const sameKind = segments.filter(item => item.kind === current.kind).length - 1;
  return <SettingsCard title="检测项" badge={`${summary.count} 项开`} summary={summary.text}>
    <KindChecksEditor value={current.checks} kind={current.kind} disabled={disabled} onChange={checks => patch({ checks })}/>
    {current.kind === "straight" && current.checks.offset?.enabled && <OffsetReference segment={current} cameras={cameras} disabled={disabled} onPatch={patch}/>}
    {issues.length > 0 && <p className="c-warn" role="alert">{issues.join("；")}，未保存</p>}
    {sameKind > 0 && <Button size="sm" disabled={disabled || dirty} onClick={() => onChange(applyToKind(segments, at))}
      title={`其余 ${sameKind} 个${segmentKindLabel[current.kind]}段的检测项改成和段 ${index} 一样（不改逐帧定向、胶偏参考与段列表里的参数）`}>
      检测项套用到其余 {sameKind} 个{segmentKindLabel[current.kind]}段
    </Button>}
    <p className="wp-settings-hint">溢胶、气泡、拉丝：本轮只检溢胶（每帧最大胶宽超限判 NG）；气泡与拉丝没有算法，结果中标“未检项”。</p>
  </SettingsCard>;
}

const measureOptions: { value: OffsetMeasure; label: string }[] = [
  { value: "absoluteX", label: "图像横向坐标（绝对 x）" },
  { value: "toEdge", label: "胶条到边缘距离" },
];
const edgePolarityOptions: { value: EdgePolarity; label: string }[] = [
  { value: "brightToDark", label: "亮→暗" },
  { value: "darkToBright", label: "暗→亮" },
];

/** 直线段胶偏参考（D-SB-48）：参考相机只能是本段的检测相机（D-SB-41）；改测量量、相机或找边参数清空 x_ref，图纸胶边距只是参考值不清。 */
function OffsetReference({ segment, cameras, disabled, onPatch }: { segment: PathSegment; cameras: string[]; disabled: boolean; onPatch: (change: Partial<PathSegment>) => void }) {
  const name = `段 ${segment.index}`;
  const offsetRef = segment.offsetRef;
  const measure = offsetRef?.measure ?? "absoluteX";
  const inspecting = segmentCameras(segment, cameras);
  const camera = offsetRef?.camera ?? inspecting[0];
  const edge = offsetRef?.edge ?? DEFAULT_EDGE;
  const setReference = (next: { measure?: OffsetMeasure; camera?: string }) => {
    const target = next.measure ?? measure;
    onPatch({ offsetRef: { measure: target, camera: next.camera ?? camera, xRef: null, spread: null, ...(target === "toEdge" ? { edge: { ...edge } } : {}) } });
  };
  const setEdge = (change: Partial<EdgeParams>) => offsetRef && onPatch({ offsetRef: { ...offsetRef, edge: { ...edge, ...change },
    ...("nominalMm" in change ? {} : { xRef: null, spread: null }) } });
  return <div className="wp-settings-group" role="group" aria-label={`${name} 胶偏参考`}>
    <span className="wp-settings-label">胶偏参考</span>
    <Select label="胶偏测量量" aria-label={`${name} 胶偏测量量`} value={measure} options={measureOptions} disabled={disabled} onValueChange={value => setReference({ measure: value as OffsetMeasure })}/>
    <Select label="参考相机" aria-label={`${name} 参考相机`} value={camera} options={inspecting.map(id => ({ value: id, label: id }))} disabled={disabled} onValueChange={value => setReference({ camera: value })}/>
    {measure === "toEdge" && <>
      <Select label="零件边极性" aria-label={`${name} 零件边极性`} value={edge.polarity} options={edgePolarityOptions} disabled={disabled} onValueChange={value => setEdge({ polarity: value as EdgePolarity })}/>
      <NumberField label="找边搜索长度" unit="mm" value={edge.searchMm} min={0} max={50} step={1} disabled={disabled} onChange={v => setEdge({ searchMm: v })}/>
      <NumberField label="找边对比度下限" value={edge.contrastMin} min={1} max={255} step={1} disabled={disabled} onChange={v => setEdge({ contrastMin: v })}/>
      <NumberField label="图纸胶边距（可空）" unit="mm" value={edge.nominalMm ?? null} min={0} step={.1} disabled={disabled} onChange={v => setEdge({ nominalMm: Number.isNaN(v) ? null : v })}/>
      <p className="wp-settings-hint">在参考相机画面里，沿测站法向从胶边往外找零件边：极性按画面里零件边的明暗跳变选，搜索长度 0–50 mm，对比度下限 1–255。图纸胶边距填了就以它为参考值，否则用标准件得出的 x_ref。</p>
    </>}
    <p className={offsetRef?.xRef == null ? "wp-settings-hint" : "plan-mono"}>{measure === "toEdge" && edge.nominalMm != null
      ? `参考值取图纸胶边距 ${edge.nominalMm} mm`
      : offsetRef?.xRef != null
        ? `x_ref ${offsetRef.xRef} ± ${offsetRef.spread ?? "—"} mm（标准件多轮测量均值）`
        : measure === "toEdge" ? "x_ref 待标定：由标准件多轮采集的胶边距得出；或填图纸胶边距" : "x_ref 待标定：由标准件多轮采集得出，不能手填"}</p>
  </div>;
}
