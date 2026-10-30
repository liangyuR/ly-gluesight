import type { BrushGuide, ImagePoint } from "./teachingPath";
import { createContext, useContext, useEffect, useId, useRef, useState, type PointerEvent, type ReactNode } from "react";
import { Link, useLocation } from "react-router-dom";
import { Camera, Minus, Plus, RotateCcw } from "lucide-react";
import { Alert, Button, Card, Chip, EmptyState, IconButton, NumberField as SteelNumberField, Select, StatusBadge } from "qing-steel";
import { desktopAvailable } from "../../lib/desktop";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import type { GrayImage, GrayPixels } from "./types";
import { errorText } from "../../lib/errors";
import { PlanSteps } from "./PlanSteps";
import { searchBand } from "./searchBand";
import { fanRadii, guideRadii, HEADING_MARK, rayEnd, wedgePath, type HeadingGuide, type HeadingPart } from "./headingOverlay";
import ContextHelp from "../cycle/components/ContextHelp";

export function Badge({ children, tone = "info" }: { children: ReactNode; tone?: "info" | "ok" | "ng" | "warn" | "neutral" }) {
  return <StatusBadge tone={tone}>{children}</StatusBadge>;
}
const PanelDepth = createContext(0);
export function Panel({ title, titleHelp, help, detail, actions, children, className = "" }: { title: string; titleHelp?: ReactNode; help?: ReactNode; detail?: string; actions?: ReactNode; children: ReactNode; className?: string }) {
  const depth = useContext(PanelDepth);
  const headingHelp = titleHelp ?? (help ? <ContextHelp label={`${title}说明`}>{help}</ContextHelp> : null);
  return <Card level={depth ? "mid" : "low"} className={"panel wp-panel " + className} title={<>{headingHelp ? <div style={{ display: "flex", alignItems: "center" }}><h2>{title}</h2>{headingHelp}</div> : <h2>{title}</h2>}{detail && <p className="muted">{detail}</p>}</>} actions={actions}>
    <PanelDepth.Provider value={depth + 1}>{children}</PanelDepth.Provider>
  </Card>;
}
export function Notice({ title, children, tone = "info" }: { title: string; children?: ReactNode; tone?: "info" | "ok" | "warn" | "ng" }) {
  return <Alert tone={tone} title={title}>{children}</Alert>;
}
export function KV({ label, children, title, className }: { label: string; children: ReactNode; title?: string; className?: string }) {
  return <div className="wp-kv" title={title}><span>{label}</span><strong className={className}>{children}</strong></div>;
}
export function NumberField({ label, value, unit, onChange, min, max, step = 1, disabled }: { label: string; value: number | null; unit?: string; onChange: (value: number) => void; min?: number; max?: number; step?: number; disabled?: boolean }) {
  return <SteelNumberField label={label} className="num" value={value != null && Number.isFinite(value) ? value : ""} min={min} max={max} step={step} disabled={disabled} unit={unit} clamp={false} onValueChange={value => onChange(value === "" ? NaN : value)} />;
}
export function WorkspaceEmpty() {
  const { error } = useWorkspace();
  return <div className="wp-page">{error && <Notice title="无法加载候选配置" tone="warn">{error}</Notice>}<div className="wp-empty"><EmptyState icon={<Camera size={32} aria-hidden="true" />} title={desktopAvailable() ? "尚未选择配方" : "当前为浏览器查看模式"} description={desktopAvailable() ? "在配方库选择或新建配方，再开始规划段与示教走廊。" : "请在桌面软件中连接实际数据。"} action={desktopAvailable() && <Button asChild><Link to="/recipe">打开配方库</Link></Button>} /></div></div>;
}
export function WorkspaceBar({ showNavigation = true }: { showNavigation?: boolean } = {}) {
  const { list, drafts, selectedId, select, data, doc, dirty, frameDirty, navigationDirty, busy, previewError, error, saveState, retrySave, setError } = useWorkspace();
  const { pathname } = useLocation();
  useEffect(() => () => setError(""), [pathname, setError]);
  const choices = [...list.map(r => ({ id:r.id, name:r.name })), ...drafts.filter(w => !list.some(r => r.id === w.doc.id)).map(w => ({ id:w.doc.id, name:w.doc.name }))];
  const publish = data?.workspace.publish;
  return <>{showNavigation && <><div className="wp-workbench-bar"><div className="wp-recipe-picker"><Select aria-label="当前配方" title="当前配方" value={selectedId ?? ""} disabled={busy || dirty || frameDirty || navigationDirty} onValueChange={value => void select(value)} placeholder="选择配方" options={choices.map(r => ({ value: r.id, label: `${r.id} · ${r.name}` }))} /></div>{data && <Badge tone={data.productionVersion ? "ok" : "warn"}>{data.productionVersion ? "已发布 v" + data.productionVersion : "未发布"}</Badge>}{data && publish?.state === "waiting" && <Badge tone="warn">v{data.workspace.pending?.doc.version} 待工件结束后生效</Badge>}<span className="wp-save-state" title={data ? `候选 v${doc?.version} · 修订 ${data.workspace.revision}` : undefined}><Badge tone={saveState === "failed" ? "warn" : "neutral"}>{saveState === "saving" ? "保存中" : saveState === "failed" ? "保存失败" : saveState === "pending" ? "等待保存" : "草稿已保存"}</Badge></span>{saveState === "failed" && <Button size="sm" onClick={retrySave}>重试保存</Button>}<Button asChild variant="outlined"><Link to="/recipe">配方库</Link></Button></div><Steps /></>}
    {!showNavigation && saveState === "failed" && <Notice title="候选保存失败" tone="warn"><Button size="sm" onClick={retrySave}>重试保存</Button></Notice>}
    {navigationDirty && <Notice title="工件导航图尚未保存" tone="warn">修改已保留，请在地图中保存或撤销；标注可到<Link to="/recipe/segments">分段页</Link>继续编辑。保存后再发布或切换配方。</Notice>}
    {previewError && <Notice title="候选参数无效" tone="warn">{previewError}</Notice>}
    {error && <Notice title="操作未完成" tone="warn">{error}</Notice>}
    {publish?.state === "waiting" && <Notice title="待生效，候选已冻结">等待当前工件结束后切换；生效失败可在发布页重试同一版本。</Notice>}
    {publish?.state === "failed" && <Notice title="生效失败，可重试同一版本" tone="warn">{publish.message}</Notice>}
    {publish?.state === "uncertain" && <Notice title="生效状态不确定，请先核实发布状态" tone="ng">{publish.message}</Notice>}
    {publish?.state === "active" && publish.warning && <Notice title={`v${publish.version} 已生效，仍有事项需处理`} tone="warn">{publish.warning}</Notice>}
  </>;
}
export function Steps() {
  const { data } = useWorkspace();
  return data ? <PlanSteps /> : null;
}

export function useGrayImage(id: string | null, imageId: string | null) {
  const key=JSON.stringify([id,imageId]);
  const [state, setState] = useState<{key:string;image:GrayImage|null;error:string;loading:boolean}>({key,image:null,error:"",loading:false});
  useEffect(() => {
    let alive = true;
    setState({key,image:null,error:"",loading:!!id&&!!imageId});
    if (!id || !imageId) return;
    workspaceApi.image(id, imageId).then(image => alive && setState({key,image,error:"",loading:false})).catch(e => alive && setState({key,image:null,error:errorText(e),loading:false}));
    return () => { alive = false; };
  }, [id, imageId]);
  return state.key===key?state:{image:null,error:"",loading:!!id&&!!imageId};
}

/** 叠加在原图上的内容（这张图的像素坐标）。 */
export interface ImageOverlay {
  /** 走廊中线（草稿或已保存的） */
  path: [number, number][];
  searchHalfPx?: number;
  /** 测量横线（多轮标定逐帧查看：各站两侧边缘） */
  segments?: { from: [number, number]; to: [number, number]; color: string }[];
  /** 选中的中线点 */
  selected?: number | null;
  /** 逐帧定向叠加（走廊示教页，开启定向的段） */
  heading?: HeadingGuide | null;
}

function HeadingLayer({ guide, size, screenPx }: { guide: HeadingGuide; size: [number, number]; screenPx: (px: number) => number }) {
  const c = guide.nozzle;
  const radii = guideRadii(guide, size);
  const fan = fanRadii(guide, size);
  const [nx, ny] = c;
  const arm = screenPx(12);
  const base = guide.fallback ?? (guide.range ? (guide.range[0] + guide.range[1]) / 2 : 0);
  const dim = (part: HeadingPart) => guide.focus && guide.focus !== part ? .15 : 1;
  const mark = (part: HeadingPart, r: number, deg: number) => {
    const [x, y] = rayEnd(c, r, deg);
    return <g aria-hidden="true" opacity={dim(part)}>
      <circle cx={x} cy={y} r={screenPx(8)} fill="var(--surface-lowest)" stroke="var(--on-surface)" strokeWidth={1} vectorEffect="non-scaling-stroke" />
      <text x={x} y={y} textAnchor="middle" dominantBaseline="central" fontSize={screenPx(11)} fill="var(--on-surface)">{HEADING_MARK[part]}</text>
    </g>;
  };
  return <g aria-label="逐帧定向叠加" pointerEvents="none" fill="none" vectorEffect="non-scaling-stroke">
    {guide.zone && guide.zone[0] > 0 && <path aria-label="跳过段" opacity={dim("skip")} d={wedgePath(c, [0, guide.zone[0]], [0, 360])} fill="var(--on-surface-variant)" fillOpacity={.18} stroke="var(--on-surface-variant)" strokeOpacity={.5} strokeWidth={1} vectorEffect="non-scaling-stroke" />}
    {guide.zone && <path aria-label="测量区间环带" opacity={dim("zone")} d={wedgePath(c, guide.zone, [0, 360])} fill="var(--primary)" fillOpacity={.06} fillRule="evenodd" stroke="var(--primary)" strokeOpacity={.6} strokeWidth={1} strokeDasharray="6 4" vectorEffect="non-scaling-stroke" />}
    {guide.sector && <path aria-label="搜索扇区" opacity={dim("sector")} d={wedgePath(c, fan, guide.sector)} fill="var(--primary)" fillOpacity={.1} stroke="var(--primary)" strokeWidth={1} strokeDasharray="3 4" vectorEffect="non-scaling-stroke" />}
    {guide.range && <path aria-label="本段方向范围" opacity={dim("range")} d={wedgePath(c, fan, guide.range)} fill="var(--warn)" fillOpacity={.22} stroke="var(--warn)" strokeWidth={1.5} vectorEffect="non-scaling-stroke" />}
    {guide.fallback != null && (() => { const [x, y] = rayEnd(c, radii[1], guide.fallback); return <line aria-label="回退方向" opacity={dim("fallback")} x1={nx} y1={ny} x2={x} y2={y} stroke="var(--ok)" strokeWidth={2} strokeDasharray="8 4" vectorEffect="non-scaling-stroke" />; })()}
    {guide.measured != null && (() => { const [x, y] = rayEnd(c, radii[1], guide.measured); return <line aria-label="实测方向" opacity={dim("reading")} x1={nx} y1={ny} x2={x} y2={y} stroke="var(--primary)" strokeWidth={2} vectorEffect="non-scaling-stroke" />; })()}
    {guide.zone && guide.zone[0] > 0 && mark("skip", guide.zone[0] / 2, base + 180)}
    {guide.zone && mark("zone", (guide.zone[0] + guide.zone[1]) / 2, base + 180)}
    {guide.sector && mark("sector", fan[1], guide.sector[0])}
    {guide.range && mark("range", fan[1], guide.range[1])}
    {guide.fallback != null && mark("fallback", radii[1], guide.fallback)}
    {guide.measured != null && mark("reading", radii[1] * .8, guide.measured)}
    <g aria-label={guide.calibrated ? "出胶点（已标定）" : "出胶点（走廊推算）"} stroke={guide.calibrated ? "var(--ok)" : "var(--warn)"} strokeWidth={2}>
      <line x1={nx - arm} y1={ny} x2={nx + arm} y2={ny} vectorEffect="non-scaling-stroke" />
      <line x1={nx} y1={ny - arm} x2={nx} y2={ny + arm} vectorEffect="non-scaling-stroke" />
      <circle cx={nx} cy={ny} r={screenPx(6)} vectorEffect="non-scaling-stroke" />
      <text x={nx} y={ny} dx={screenPx(14)} dy={screenPx(-10)} fontSize={screenPx(13)} fill={guide.calibrated ? "var(--ok)" : "var(--warn)"} stroke="var(--surface-lowest)" strokeWidth={3} paintOrder="stroke">{HEADING_MARK.nozzle} {guide.calibrated ? "出胶点" : "出胶点（推算）"}</text>
    </g>
  </g>;
}
/** 灰度像素直接画进同一块 canvas：换帧只重画，不产生新的图片资源。 */
function GrayCanvas({ image, pixels }: { image: GrayImage; pixels: GrayPixels }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const rgba = useRef<ImageData | null>(null);
  useEffect(() => {
    const element = canvas.current;
    const context = element?.getContext("2d");
    if (!element || !context) return;
    const { data, width, height } = pixels;
    if (element.width !== width || element.height !== height) { element.width = width; element.height = height; }
    if (rgba.current?.width !== width || rgba.current.height !== height) rgba.current = new ImageData(width, height);
    const out = new Uint32Array(rgba.current.data.buffer);
    for (let i = 0; i < data.length; i++) out[i] = 0xff000000 | data[i] * 0x010101;
    context.putImageData(rgba.current, 0, 0);
  }, [pixels]);
  return <foreignObject width={image.width} height={image.height} pointerEvents="none"><canvas ref={canvas} className="wp-gray-canvas" /></foreignObject>;
}

/** 在原图上编辑中线：点空白处在末尾加点，拖动点调整位置，点一下选中。 */
export interface PathEditing {
  add: (p: [number, number]) => void;
  move: (index: number, p: [number, number]) => void;
  select: (index: number | null) => void;
}

/**
 * controlsAt="footer"（仅走廊示教页）：缩放与显示开关下沉到底部信息行，左侧放 summary，
 * 开关文案为“中线 / 搜索带 / 点柄”，操作提示由页面自己给；默认 "header" 的外观与行为不变。
 */
export function GrayViewer({ image, loading = false, error = "", label, caption, tools, subtools, overlay, onEdit, editDisabled = false, region, onRegion, brush, onBrush, onPoint, extractionFailure, emptyTitle, emptyHint, controlsAt = "header", summary }: {
  image: GrayImage | null; loading?: boolean; error?: string; label: string; caption?: ReactNode; tools?: ReactNode; subtools?: ReactNode; overlay?: ImageOverlay; onEdit?: PathEditing; editDisabled?: boolean; region?: [number, number, number, number] | null; onRegion?: (region: [number, number, number, number]) => void;
  brush?: BrushGuide | null; onBrush?: (points: ImagePoint[]) => void; onPoint?: (p: [number, number]) => void; extractionFailure?: { point: ImagePoint; message: string } | null; emptyTitle?: string; emptyHint?: string; controlsAt?: "header" | "footer"; summary?: ReactNode;
}) {
  const [viewport, setViewport] = useState({ zoom: 1, x: 0, y: 0 });
  const { zoom, x, y } = viewport;
  const svg = useRef<SVGSVGElement>(null);
  const [fit, setFit] = useState(1);
  useEffect(() => {
    const element = svg.current;
    if (!element || !image || typeof ResizeObserver === "undefined") return;
    const measure = () => {
      const scale = Math.min(element.clientWidth / image.width, element.clientHeight / image.height);
      if (scale > 0 && Number.isFinite(scale)) setFit(scale);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [image?.url, image?.width, image?.height]);
  const pan = useRef<{ start: DOMPoint; x: number; y: number; inverse: DOMMatrix } | null>(null);
  const [panning, setPanning] = useState(false);
  const reset = () => { setViewport({ zoom: 1, x: 0, y: 0 }); };
  const zoomBy = (factor: number, anchor?: DOMPoint) => {
    if (!image || drag.current || regionStart.current || brushStroke.current || pan.current) return;
    setViewport(previous => {
      const next = Math.min(4, Math.max(.5, previous.zoom * factor));
      const ratio = next / previous.zoom;
      const ax = (anchor?.x ?? image.width / 2) - image.width / 2;
      const ay = (anchor?.y ?? image.height / 2) - image.height / 2;
      return { zoom: next, x: ax - (ax - previous.x) * ratio, y: ay - (ay - previous.y) * ratio };
    });
  };
  const brushStroke = useRef<{ points: ImagePoint[]; inverse: DOMMatrix; pointerId: number } | null>(null);
  const regionStart = useRef<[number, number] | null>(null);
  const [shown, setShown] = useState(true);
  const [bandShown, setBandShown] = useState(true);
  const [headingShown, setHeadingShown] = useState(true);
  const bandClip = useId();
  const [allHandles, setAllHandles] = useState(false);
  const group = useRef<SVGGElement>(null);
  const drag = useRef<{ index: number; inverse: DOMMatrix } | null>(null);
  const stopGesture = () => { drag.current = null; regionStart.current = null; brushStroke.current = null; pan.current = null; setPanning(false); };
  useEffect(() => { reset(); stopGesture(); setAllHandles(false); }, [image?.url, image?.width, image?.height]);
  useEffect(() => {
    const element = svg.current;
    if (!element || !image) return;
    const wheel = (event: WheelEvent) => {
      if (!event.ctrlKey) return;
      event.preventDefault();
      const inverse = element.getScreenCTM()?.inverse();
      if (!inverse || !event.deltaY) return;
      const anchor = new DOMPoint(event.clientX, event.clientY).matrixTransform(inverse);
      const pixels = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? element.clientHeight : 1);
      zoomBy(Math.exp(-Math.max(-200, Math.min(200, pixels)) * .002), anchor);
    };
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  }, [image?.url, image?.width, image?.height]);
  const point = (e: PointerEvent<SVGSVGElement>, inverse?: DOMMatrix): [number, number] | null => {
    const transform = inverse ?? group.current?.getScreenCTM()?.inverse();
    if (!transform || !image) return null;
    const p = new DOMPoint(e.clientX,e.clientY).matrixTransform(transform);
    // 取 0.1 px：后端按 f32 存，取回的值与草稿一致
    const round = (v: number, max: number) => Math.round(Math.min(max, Math.max(0, v)) * 10) / 10;
    return [round(p.x, image.width), round(p.y, image.height)];
  };
  const brushPoint = (e: PointerEvent<SVGSVGElement>, inverse: DOMMatrix): ImagePoint | null => {
    const p = point(e, inverse);
    return p && image ? [Math.min(p[0], image.width - 1), Math.min(p[1], image.height - 1)] : null;
  };
  const screenPx = (px: number) => px / (fit * zoom);
  const handle = screenPx(7);
  const editing = !!onEdit && shown && !editDisabled;
  const path = overlay?.path ?? [];
  const band = searchBand(path, overlay?.searchHalfPx);
  const hint = error || emptyHint;
  const footer = controlsAt === "footer";
  const controls = <div className="wp-zoom"><IconButton size="sm" aria-label="缩小原图" disabled={!image} onClick={() => zoomBy(1/1.25)}><Minus size={15} /></IconButton><span>{Math.round(zoom * 100)}%</span><IconButton size="sm" aria-label="放大原图" disabled={!image} onClick={() => zoomBy(1.25)}><Plus size={15} /></IconButton><IconButton size="sm" aria-label="适应窗口" disabled={!image} onClick={reset}><RotateCcw size={15} /></IconButton>{overlay && <Chip selected={shown} onClick={() => setShown(v => !v)}>{footer ? "中线" : "中线叠加"}</Chip>}{band && <Chip selected={bandShown} onClick={() => setBandShown(v => !v)}>搜索带</Chip>}{overlay?.heading && <Chip selected={headingShown} onClick={() => setHeadingShown(v => !v)}>定向</Chip>}{onEdit && overlay && <Chip selected={allHandles} disabled={!shown || editDisabled} title={footer ? "显示全部点柄" : undefined} onClick={() => setAllHandles(v => !v)}>{footer ? "点柄" : "编辑点"}</Chip>}</div>;
  return <div className={"wp-viewport" + (footer ? " wp-viewport-footer" : "")}>{footer ? tools && <div className="wp-image-tools">{tools}</div> : <div className="wp-image-tools">{tools ?? <span>{caption ?? label}</span>}{controls}</div>}{subtools}
    {!image ? <div className="wp-image-empty"><Camera size={32} /><strong>{loading ? "正在读取原图…" : error ? "原图不可用" : emptyTitle ?? "暂无图像"}</strong>{hint && <span>{hint}</span>}</div> :
      <svg ref={svg} role="img" tabIndex={0} style={{ aspectRatio: `${image.width} / ${image.height}` }} className={"wp-gray-image" + (editing ? " editable" : "") + (panning ? " panning" : "")} viewBox={"0 0 " + image.width + " " + image.height} aria-label={label} aria-description="Ctrl 加滚轮缩放，中键拖动平移；聚焦后加减键缩放，0 适应窗口" onKeyDown={e => {
        if (e.altKey || e.metaKey || drag.current || regionStart.current || brushStroke.current || pan.current) return;
        if (["+", "=", "-", "0"].includes(e.key)) {
          e.preventDefault();
          if (e.key === "0") reset(); else zoomBy(e.key === "-" ? 1/1.25 : 1.25);
        }
      }} onAuxClick={e => { if (e.button === 1) e.preventDefault(); }} onPointerDown={e => {
        if (pan.current || drag.current || regionStart.current || brushStroke.current) return;
        if (e.button === 1) {
          const inverse = e.currentTarget.getScreenCTM()?.inverse();
          if (!inverse) return;
          e.preventDefault();
          e.currentTarget.focus({ preventScroll: true });
          pan.current = { start: new DOMPoint(e.clientX, e.clientY).matrixTransform(inverse), x, y, inverse };
          setPanning(true);
          e.currentTarget.setPointerCapture?.(e.pointerId);
          return;
        }
        if (e.button !== 0) return;
        e.currentTarget.focus({ preventScroll: true });
        if (onBrush && !editDisabled) {
          const inverse = group.current?.getScreenCTM()?.inverse();
          const p = inverse ? brushPoint(e, inverse) : null;
          if (p && inverse) { brushStroke.current = { points: [p], inverse, pointerId: e.pointerId }; onBrush([p]); e.currentTarget.setPointerCapture?.(e.pointerId); }
          return;
        }
        if (onRegion) { const p = point(e); if (p) { regionStart.current = p; onRegion([p[0], p[1], 0, 0]); e.currentTarget.setPointerCapture?.(e.pointerId); } return; }
        if (onPoint) { if (!editDisabled) { const p = point(e); if (p) onPoint(p); } return; }
        if (!editing || !onEdit) return;
        const inverse = group.current?.getScreenCTM()?.inverse();
        const p = inverse ? point(e, inverse) : null;
        if (!p || !inverse) return;
        const vertex = (e.target as Element).closest?.("[data-vertex]")?.getAttribute("data-vertex");
        // 整次拖动使用按下时的图像坐标变换，提示文字改变页面布局也不影响
        const index = vertex != null ? Number(vertex) : path.length;
        if (vertex == null) onEdit.add(p);
        onEdit.select(index);
        drag.current = { index, inverse };
        e.currentTarget.setPointerCapture?.(e.pointerId);
      }} onPointerMove={e => {
        if (pan.current) {
          const gesture = pan.current;
          const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(gesture.inverse);
          setViewport(previous => ({ ...previous, x: gesture.x + p.x - gesture.start.x, y: gesture.y + p.y - gesture.start.y }));
          return;
        }
        if (brushStroke.current && onBrush && !editDisabled) {
          const stroke = brushStroke.current;
          if (e.pointerId !== stroke.pointerId) return;
          const p = brushPoint(e, stroke.inverse), last = stroke.points[stroke.points.length - 1];
          if (p && stroke.points.length < 4096 && Math.hypot(p[0] - last[0], p[1] - last[1]) >= 2) { stroke.points.push(p); onBrush([...stroke.points]); }
          return;
        }
        if (onRegion && regionStart.current) { const p = point(e), a = regionStart.current; if (p) onRegion([Math.min(a[0],p[0]), Math.min(a[1],p[1]), Math.abs(p[0]-a[0]), Math.abs(p[1]-a[1])]); return; }
        const current = drag.current;
        if (!current || !onEdit || editDisabled) return;
        const p = point(e, current.inverse);
        if (p) onEdit.move(current.index, p);
      }} onPointerUp={stopGesture} onPointerCancel={stopGesture} onLostPointerCapture={stopGesture}>
        <g ref={group} transform={"translate(" + (image.width/2 + x) + " " + (image.height/2 + y) + ") scale(" + zoom + ") translate(" + -image.width/2 + " " + -image.height/2 + ")"}>{image.pixels ? <GrayCanvas image={image} pixels={image.pixels} /> : <image href={image.url} width={image.width} height={image.height} />}
          {band && bandShown && <g aria-label="搜索带边界" pointerEvents="none" clipPath={`url(#${bandClip})`} fill="none" stroke="var(--primary)" opacity={.45}>
            <defs><clipPath id={bandClip}><rect width={image.width} height={image.height} /></clipPath></defs>
            {[band.left, band.right].map((edge, i) => <polyline key={i} points={edge.map(p => p.join(",")).join(" ")} strokeWidth={1} strokeDasharray="5 5" vectorEffect="non-scaling-stroke" />)}
            {band.bars.map(i => <line key={i} x1={band.left[i][0]} y1={band.left[i][1]} x2={band.right[i][0]} y2={band.right[i][1]} strokeWidth={1} strokeDasharray="2 6" vectorEffect="non-scaling-stroke" />)}
          </g>}
          {brush && brush.points.length > 0 && <g aria-label="刷选胶路区域" pointerEvents="none">
            <polyline points={brush.points.map(p => p.join(",")).join(" ")} fill="none" stroke="var(--primary)" strokeOpacity={.22} strokeWidth={brush.radiusPx * 2} strokeLinecap="round" strokeLinejoin="round" />
            <polyline points={brush.points.map(p => p.join(",")).join(" ")} fill="none" stroke="var(--primary)" strokeWidth={1} strokeDasharray="4 4" vectorEffect="non-scaling-stroke" />
            {[brush.points[0], brush.points[brush.points.length - 1]].map(([px, py], i) => <g key={i}><circle cx={px} cy={py} r={handle} fill={i === 0 ? "var(--ok)" : "var(--primary)"} /><text x={px} y={py} dx={screenPx(10)} dy={screenPx(-8)} fontSize={screenPx(13)} fill="var(--primary)" stroke="var(--surface-lowest)" strokeWidth={3} paintOrder="stroke" vectorEffect="non-scaling-stroke">{i === 0 ? "起点" : "终点"}</text></g>)}
          </g>}
          {extractionFailure && <g aria-label="中线提取失败位置" pointerEvents="none" transform={`translate(${extractionFailure.point[0]} ${extractionFailure.point[1]})`}>
            <title>{extractionFailure.message}</title>
            <circle r={screenPx(13)} fill="var(--surface-lowest)" fillOpacity={.8} stroke="var(--ng)" strokeWidth={3} vectorEffect="non-scaling-stroke" />
            <text textAnchor="middle" dy={screenPx(5.5)} fontSize={screenPx(16)} fontWeight="bold" fill="var(--ng)">!</text>
          </g>}
          {overlay?.heading && headingShown && <HeadingLayer guide={overlay.heading} size={[image.width, image.height]} screenPx={screenPx} />}
          {region && <rect x={region[0]} y={region[1]} width={region[2]} height={region[3]} fill="var(--primary)" fillOpacity={.12} stroke="var(--primary)" strokeWidth={2} vectorEffect="non-scaling-stroke" />}
          {overlay && shown && <g aria-label="中线叠加">
            {path.length >= 2 && <polyline points={path.map(p => p.join(",")).join(" ")} fill="none" stroke="var(--primary)" strokeWidth={2} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />}
            {onEdit && path.map(([x, y], i) => (allHandles || path.length <= 24 || i === 0 || i === path.length - 1 || overlay.selected === i) && <circle key={"v" + i} data-vertex={i} aria-label={"中线点 " + (i + 1)} cx={x} cy={y} r={handle} fill={overlay.selected === i ? "var(--primary)" : "var(--surface-lowest)"} fillOpacity={.85} stroke={i === 0 ? "var(--ok)" : "var(--primary)"} strokeWidth={2} vectorEffect="non-scaling-stroke" />)}
            {overlay.segments && <g aria-label="测量横线" pointerEvents="none">
              {overlay.segments.map((segment, i) => <line key={i} x1={segment.from[0]} y1={segment.from[1]} x2={segment.to[0]} y2={segment.to[1]} stroke={segment.color} strokeWidth={2} vectorEffect="non-scaling-stroke" />)}
            </g>}
          </g>}
        </g></svg>}
    {footer ? <div className="wp-image-footer">{summary}{controls}</div> : image && <div className="wp-image-footer"><span>{image.width} × {image.height} px · Ctrl＋滚轮缩放 · 中键拖动 · + / − / 0</span><span>{onBrush ? "按住左键沿胶路刷过；每一笔重画选区，中键拖动视图" : onRegion ? "拖动框选一段连续胶路区域" : onEdit ? (path.length > 24 && !allHandles ? "密集中线仅显示端点；打开“编辑点”可调整全部点" : "点击图像依次加中线点（从胶嘴一侧往外，绿圈为起点），拖动点可调整") : "原始灰度图像"}</span></div>}
  </div>;
}
