import { useEffect, useState, type ReactNode } from "react";
import { Link, useLocation } from "react-router-dom";
import { ArrowUpRight } from "lucide-react";
import { Accordion, Button, SegmentedControl, Select, Switch } from "qing-steel";
import { DetectEditor } from "../recipe";
import { recipeApi } from "../cycle/api";
import type { DetectParams } from "../cycle/types";
import type { CameraInspect, EdgeParams, EdgePolarity, ExpertParams, ExpertSpec, HeadingOverlay, InspectWindow, OffsetMeasure, PathSegment } from "../plan/types";
import { KindChecksEditor } from "../plan/KindChecksEditor";
import { segmentKindLabel } from "../plan/vis";
import { Badge, NumberField } from "./components";
import { HeadingDiagram } from "./HeadingDiagram";
import { FAN_LENGTH_PX, HEADING_MARK, type HeadingPart } from "./headingOverlay";
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

/** 逐帧定向面板里的专家参数组；其余组在“专家参数”卡片。 */
const HEADING_EXPERT_GROUPS = ["定胶路"];

/** 专家参数表单：`groups` 判断哪些组放在这里。改“本相机”只作用于这台相机，改“配方默认”作用于所有没有覆盖这一项的相机；越界或空值只提示，不写候选配方。 */
function ExpertFields({ camera, own, defaults, disabled, groups, hint, onCamera, onDefaults }: {
  camera: string; own: ExpertParams; defaults: ExpertParams; disabled: boolean; groups: (group: string) => boolean; hint: ReactNode;
  onCamera: (next: ExpertParams) => void; onDefaults: (next: ExpertParams) => void;
}) {
  const specs = useExpertSpecs();
  const [scope, setScope] = useState<"camera" | "recipe">("camera");
  const [error, setError] = useState("");
  const target = scope === "camera" ? own : defaults;
  const write = scope === "camera" ? onCamera : onDefaults;
  const grouped = Object.entries(specs).filter(([, spec]) => groups(spec.group)).reduce<Record<string, [string, ExpertSpec][]>>((out, entry) => ({ ...out, [entry[1].group]: [...(out[entry[1].group] ?? []), entry] }), {});
  const set = (name: string, spec: ExpertSpec, value: number) => {
    if (!Number.isFinite(value) || value < spec.min || value > spec.max) { setError(`${spec.label}需在 ${spec.min}–${spec.max} 之间，未保存`); return; }
    setError("");
    write({ ...target, [name]: value });
  };
  return <fieldset className="wp-plain-fieldset wp-settings-fields" disabled={disabled}>
    <SegmentedControl size="sm" label="修改范围" value={scope} disabled={disabled}
      options={[{ value: "camera", label: `仅 ${camera}` }, { value: "recipe", label: "配方默认" }]} onValueChange={value => setScope(value as "camera" | "recipe")}/>
    {Object.entries(grouped).map(([group, items]) => <div key={group} className="wp-settings-group" role="group" aria-label={group}>
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
    <p className="wp-settings-hint">{hint}</p>
  </fieldset>;
}

/**
 * 相机视图“专家参数”（LF-5）：测量图模板里开放的算子门限，按 lyFlow 原始单位填。“定胶路”组在“逐帧定向”面板里。
 */
export function ExpertCard({ camera, own, defaults, disabled, onCamera, onDefaults }: {
  camera: string; own: ExpertParams; defaults: ExpertParams; disabled: boolean;
  onCamera: (next: ExpertParams) => void; onDefaults: (next: ExpertParams) => void;
}) {
  const specs = useExpertSpecs();
  const here = (params: ExpertParams) => Object.keys(params).filter(name => !HEADING_EXPERT_GROUPS.includes(specs[name]?.group ?? "")).length;
  const changed = here({ ...defaults, ...own });
  return <SettingsCard title="专家参数" badge={changed ? `${changed} 项已改` : "默认"} tone={changed ? "info" : "neutral"}
    summary={changed ? `本相机 ${here(own)} 项 · 配方默认 ${here(defaults)} 项` : "门限均为模板默认值"}>
    <ExpertFields camera={camera} own={own} defaults={defaults} disabled={disabled} groups={group => !HEADING_EXPERT_GROUPS.includes(group)} onCamera={onCamera} onDefaults={onDefaults}
      hint="作用于 lyFlow 算子或 GlueSight 判定（px 等原始单位）；悬停看说明。改动后请重新试测，发布前跑样本验证。"/>
  </SettingsCard>;
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

const headingSummary = (params: CameraParams, derived: [number, number] | null) =>
  `${params.window ? `跳过前 ${params.window.startMm} mm，测到 ${params.window.startMm + params.window.lengthMm} mm` : "测量区间未设置"} · ${params.nozzle ? "出胶点已标定" : derived ? "出胶点按走廊推算" : "出胶点未标定"}`;

const deg = (value: number) => `${Math.round(value)}°`;

/** 面板一行：编号与示意图、画布叠加上的同一部件对应，指着这一行时两处都只突出它。 */
function PartRow({ part, title, hint, onFocus, children }: { part: HeadingPart; title: string; hint: ReactNode; onFocus: (part: HeadingPart | null) => void; children?: ReactNode }) {
  return <div className="wp-part-row" role="group" aria-label={`${HEADING_MARK[part]} ${title}`}
    onPointerEnter={() => onFocus(part)} onPointerLeave={() => onFocus(null)} onFocus={() => onFocus(part)} onBlur={() => onFocus(null)}>
    <span className="wp-part-mark" aria-hidden="true">{HEADING_MARK[part]}</span>
    <div className="wp-part-body"><span className="wp-part-title">{title}</span>{children}<p className="wp-settings-hint">{hint}</p></div>
  </div>;
}

/**
 * “逐帧定向”面板（D-SB-47）：出胶点、测量区间、方向与“什么算找到胶”的门限都在这里，配一张编号示意图；
 * 每行编号与示意图、示教帧上的叠加一致。相机视图（`index` 为 null）设这台相机各段共用的部分并列出开启的段；
 * 段视图另设本段开关与允许偏离，只读显示示教方向、搜索扇区与回退方向（`geometry`，按已保存的走廊算）。
 * `sharedLocked` 非空时共用参数只读并说明原因。
 */
export function HeadingCard({ camera, params, derivedNozzle, segments, index, headingSegments, geometry, expertDefaults, disabled, sharedLocked, onFocus, onSegments, onParams, onExpertDefaults }: {
  camera: string; params: CameraParams; derivedNozzle: [number, number] | null; segments: PathSegment[]; index: number | null; headingSegments: PathSegment[];
  geometry: HeadingOverlay | null; expertDefaults: ExpertParams; disabled: boolean; sharedLocked: string;
  onFocus: (part: HeadingPart | null) => void; onSegments: (segments: PathSegment[]) => void; onParams: (next: CameraParams) => void; onExpertDefaults: (next: ExpertParams) => void;
}) {
  const { saved, current, issues, patch } = useSegmentDraft(segments, index ?? -1, onSegments);
  const [focus, setFocus] = useState<HeadingPart | null>(null);
  const point = (part: HeadingPart | null) => { setFocus(part); onFocus(part); };
  if (index != null && (!saved || !current)) return null;
  const heading = current?.heading ?? null;
  const needed = index != null ? !!heading : headingSegments.length > 0;
  const missing = needed && !params.window;
  const sharedDisabled = disabled || !!sharedLocked;
  const scale = scaleOf(params.mmPerPx);
  const zonePx = params.window && scale ? params.window.lengthMm / scale : null;
  const fanPx = zonePx != null ? Math.min(FAN_LENGTH_PX, zonePx) : FAN_LENGTH_PX;
  const [badge, tone]: [string, Tone] = index != null ? [saved!.heading ? `开 ±${saved!.heading.toleranceDeg}°` : "关", saved!.heading ? "info" : "neutral"]
    : missing ? ["测量区间需设置", "warn"] : needed ? [`段 ${headingSegments.map(item => item.index).join("、")} 开启`, "neutral"] : ["未使用", "neutral"];
  const absoluteX = current?.checks.offset?.enabled && (current.offsetRef?.measure ?? "absoluteX") === "absoluteX";
  return <SettingsCard title="逐帧定向" badge={badge} tone={tone} open={index != null || missing}
    summary={index != null && !saved!.heading ? "关闭：按示教走廊测量" : headingSummary(params, derivedNozzle)}>
    {index != null && <Switch label="本段逐帧定向" aria-label={`段 ${index} 逐帧定向`} checked={!!heading} disabled={disabled} onCheckedChange={on => patch({ heading: on ? { ...DEFAULT_HEADING } : null })}/>}
    {index != null && !heading ? <p className="wp-settings-hint">开启后每帧按图像找胶条从出胶点出去的方向，只测测量区间内的一段，不再依赖按帧号选走廊。</p> : <>
      <HeadingDiagram focus={focus}/>
      <div className="wp-settings-group" role="group" aria-label="出胶点与测量区间">
        <span className="wp-settings-label">出胶点与测量区间<Scope>{camera} 各段共用</Scope>{missing && <Badge tone="warn">需设置</Badge>}</span>
        {sharedLocked && <p className="wp-settings-hint">{sharedLocked}</p>}
        <PartRow part="nozzle" title="出胶点" onFocus={point} hint="胶嘴出胶处，下面的距离和方向都从这里量。在开启定向的段的示教帧上选“标出胶点”点选；未标定时取这些段走廊起点的平均，相差超过 1 mm 报“出胶点不一致”。">
          <NozzleStatus value={params.nozzle ?? null} derived={derivedNozzle} needed={needed} disabled={sharedDisabled} onClear={() => onParams({ ...params, nozzle: null })}/>
        </PartRow>
        <WindowFields value={params.window ?? null} disabled={sharedDisabled} onFocus={point} onChange={window => onParams({ ...params, window })}/>
      </div>
      {index != null && heading ? <div className="wp-settings-group" role="group" aria-label={`段 ${index} 方向`}>
        <span className="wp-settings-label">方向<Scope>段 {index}</Scope></span>
        <PartRow part="range" title="允许偏离示教方向" onFocus={point}
          hint={geometry ? `示教方向 ${deg(geometry.fallback)}，方向范围 ${deg(geometry.range[0])} … ${deg(geometry.range[1])}（图上黄色实线扇形），范围内的方向优先。` : "保存走廊后按示教方向显示方向范围（图上黄色实线扇形），范围内的方向优先。"}>
          <NumberField label="允许偏离" unit="°" value={heading.toleranceDeg} min={0} max={45} step={1} disabled={disabled} onChange={v => patch({ heading: { toleranceDeg: v } })}/>
        </PartRow>
        {issues.length > 0 && <p className="c-warn" role="alert">{issues.join("；")}，未保存</p>}
        <PartRow part="sector" title="搜索扇区" onFocus={point}
          hint={`由本段与相邻段的方向范围推出，不能直接改。只在测量区间里离出胶点最近的 ${Math.round(fanPx)} px${scale ? `（约 ${(fanPx * scale).toFixed(1)} mm）` : ""} 内找胶条往哪个方向出去（图上虚线扇形）。`}>
          <span className="plan-mono">{geometry ? `${deg(geometry.sector[0])} … ${deg(geometry.sector[1])}` : "保存走廊后显示"}</span>
        </PartRow>
        <PartRow part="fallback" title="找不到胶时" onFocus={point} hint="选中方向上找到胶的比例、暗带边缘陡度或胶点数不达标时回退（门限在下方“什么算找到胶”），试测结果里写明原因。">
          <span>按示教方向{geometry ? ` ${deg(geometry.fallback)}` : ""} 测（图上绿色虚线）</span>
        </PartRow>
        <PartRow part="reading" title="方向读数" onFocus={point} hint="取拟合出的胶路首尾连线的方向，不取起跑方向；试测时图上蓝色实线。用于细化分段边界。">
          <span className="muted">每帧试测得出</span>
        </PartRow>
        {absoluteX && <p className="c-warn">定向测量时胶路由图像拟合，绝对 x 恒为 0：本段胶偏请在“检测项”里改用“胶条到边缘距离”。</p>}
      </div> : <div className="wp-settings-group" role="group" aria-label="各段方向">
        <span className="wp-settings-label">方向<Scope>按段设置</Scope></span>
        <p className="wp-settings-hint">{needed ? `已开启：${headingSegments.map(item => `段 ${item.index} ±${item.heading?.toleranceDeg ?? 0}°`).join("、")}。` : "没有段开启逐帧定向，上面的参数可不填。"}开关与允许偏离示教方向的角度在左侧选段后设置。</p>
      </div>}
      <div className="wp-settings-group" role="group" aria-label="什么算找到胶">
        <span className="wp-settings-label">什么算找到胶<Scope>高级</Scope></span>
        <ExpertFields camera={camera} own={params.expert ?? {}} defaults={expertDefaults} disabled={sharedDisabled} groups={group => HEADING_EXPERT_GROUPS.includes(group)}
          onCamera={expert => onParams({ ...params, expert })} onDefaults={onExpertDefaults}
          hint={`选中方向不满足这些门限就转 ${HEADING_MARK.fallback} 回退；悬停看说明。改动后请重新试测。`}/>
      </div>
    </>}
  </SettingsCard>;
}

/** 测量区间：配方存起点与长度，这里按“跳过前多少、测到多少”填；两项都空为未设置，不合法时只提示、不写入配方。 */
function WindowFields({ value, disabled, onFocus, onChange }: { value: InspectWindow | null; disabled: boolean; onFocus: (part: HeadingPart | null) => void; onChange: (next: InspectWindow | null) => void }) {
  const [skip, setSkip] = useState(value?.startMm ?? NaN);
  const [end, setEnd] = useState(value ? value.startMm + value.lengthMm : NaN);
  const of = (s: number, e: number) => ({ startMm: s, lengthMm: Math.round((e - s) * 1000) / 1000 });
  const empty = Number.isNaN(skip) && Number.isNaN(end);
  const error = empty ? "" : windowError(of(skip, end));
  const update = (nextSkip: number, nextEnd: number) => {
    setSkip(nextSkip); setEnd(nextEnd);
    if (Number.isNaN(nextSkip) && Number.isNaN(nextEnd)) { if (value) onChange(null); return; }
    const next = of(nextSkip, nextEnd);
    if (!windowError(next)) onChange(next);
  };
  return <>
    <PartRow part="skip" title="跳过胶嘴附近" onFocus={onFocus} hint="胶嘴会挡住胶条，各方向在出胶点附近也挤在一起分不开，这一段不测；一般取 5 mm 左右。">
      <NumberField label="跳过前" value={skip} unit="mm" min={0} max={MAX_WINDOW_END_MM} step={.5} onChange={v => update(v, end)} disabled={disabled}/>
    </PartRow>
    <PartRow part="zone" title="测量到" onFocus={onFocus} hint="每帧只测距出胶点从跳过处到这里的一段（图上环带）。这一段不要短于每帧胶条后移量（20 fps 约 20 mm），否则相邻帧之间有胶测不到。两项都清空即未设置。">
      <NumberField label="测到距出胶点" value={end} unit="mm" min={0} max={MAX_WINDOW_END_MM} step={.5} onChange={v => update(skip, v)} disabled={disabled}/>
    </PartRow>
    {error && <p className="c-warn" role="alert">{error}，未保存</p>}
  </>;
}

/** 出胶点状态：在开启定向的段的示教帧上用“标出胶点”点选，这里只看状态和清除标定。 */
function NozzleStatus({ value, derived, needed, disabled, onClear }: { value: [number, number] | null; derived: [number, number] | null; needed: boolean; disabled: boolean; onClear: () => void }) {
  const [badge, tone]: [string, Tone] = value ? ["已标定", "ok"] : derived ? ["按走廊推算", "info"] : ["未标定", needed ? "warn" : "neutral"];
  const point = value ?? derived;
  return <span className="wp-nozzle-point"><span><Badge tone={tone}>{badge}</Badge>{point && <span className="plan-mono"> ({Math.round(point[0])}, {Math.round(point[1])}) px</span>}</span>
    {value && <Button variant="text" size="sm" disabled={disabled} onClick={onClear}>清除标定</Button>}</span>;
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
