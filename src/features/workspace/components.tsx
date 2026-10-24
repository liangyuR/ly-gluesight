import type { BrushGuide, ImagePoint } from "./teachingPath";
import { createContext, useContext, useEffect, useId, useRef, useState, type PointerEvent, type ReactNode } from "react";
import { Link, useLocation } from "react-router-dom";
import { Camera, Minus, Plus, RotateCcw } from "lucide-react";
import { Alert, Button, Card, Chip, EmptyState, FormField, IconButton, StatusBadge, TextField } from "qing-steel";
import { desktopAvailable } from "../../lib/desktop";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import type { FrameView, PointVis, Recipe } from "../cycle/types";
import { framePose, movedBox, poseReasonText, poseTransform, viewSpec, viewStations, type Pt } from "../cycle/pose";
import { shotSegment, visColor } from "../cycle/vis";
import type { GrayImage, GrayPixels } from "./types";
import { errorText } from "../../lib/errors";
import { PlanSteps } from "./PlanSteps";
import { searchBand } from "./searchBand";

export function Badge({ children, tone = "info" }: { children: ReactNode; tone?: "info" | "ok" | "ng" | "warn" | "neutral" }) {
  if (tone === "neutral") return <span className="qs-badge"><span className="dot" aria-hidden="true" />{children}</span>;
  return <StatusBadge tone={tone}>{children}</StatusBadge>;
}
const PanelDepth = createContext(0);
export function Panel({ title, detail, actions, children, className = "" }: { title: string; detail?: string; actions?: ReactNode; children: ReactNode; className?: string }) {
  const depth = useContext(PanelDepth);
  return <Card level={depth ? "mid" : "low"} className={"panel wp-panel " + className} title={<><h2>{title}</h2>{detail && <p className="muted">{detail}</p>}</>} actions={actions}>
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
  return <FormField label={label} className="wp-field"><TextField className="num" type="number" value={value != null && Number.isFinite(value) ? value : ""} min={min} max={max} step={step} disabled={disabled} trailing={unit && <span className="unit">{unit}</span>} onChange={e => onChange(e.target.value===""?NaN:Number(e.target.value))} /></FormField>;
}
export function WorkspaceEmpty() {
  const { error } = useWorkspace();
  return <div className="wp-page">{error && <Notice title="无法加载候选配置" tone="warn">{error}</Notice>}<div className="wp-empty"><EmptyState icon={<Camera size={32} aria-hidden="true" />} title={desktopAvailable() ? "尚未选择配方" : "当前为浏览器查看模式"} description={desktopAvailable() ? "在配方库选择或新建配方，再开始规划段与示教走廊。" : "请在桌面软件中连接实际数据。"} action={desktopAvailable() && <Link className="btn primary" to="/recipe">打开配方库</Link>} /></div></div>;
}
export function WorkspaceBar() {
  const { list, drafts, selectedId, select, data, doc, dirty, frameDirty, busy, previewError, error, saveDoc, saveState, retrySave, setError } = useWorkspace();
  const { pathname } = useLocation();
  useEffect(() => () => setError(""), [pathname, setError]);
  const choices = [...list.map(r => ({ id:r.id, name:r.name })), ...drafts.filter(w => !list.some(r => r.id === w.doc.id)).map(w => ({ id:w.doc.id, name:w.doc.name }))];
  const publish = data?.workspace.publish;
  // 配方选择、步骤导航、保存状态合为一行；窄屏时步骤条自动换到第二行
  return <><div className="wp-workbench-bar"><label className="wp-recipe-picker"><select className="input" aria-label="当前配方" title="当前配方" value={selectedId ?? ""} disabled={busy || dirty || frameDirty} onChange={e => void select(e.target.value)}><option value="" disabled>选择配方</option>{choices.map(r => <option key={r.id} value={r.id}>{r.id} · {r.name}</option>)}</select></label>{data && <Badge tone={data.productionVersion ? "ok" : "warn"}>{data.productionVersion ? "已发布 v" + data.productionVersion : "未发布"}</Badge>}<Steps />{data && publish?.state === "waiting" && <Badge tone="warn">v{data.workspace.pending?.doc.version} 待工件结束后生效</Badge>}<span className="wp-save-state" title={data ? `候选 v${doc?.version} · 修订 ${data.workspace.revision}` : undefined}><Badge tone={saveState === "failed" ? "warn" : "neutral"}>{saveState === "saving" ? "保存中" : saveState === "failed" ? "保存失败" : saveState === "pending" ? "等待保存" : "草稿已保存"}</Badge></span>{saveState === "failed" && <Button size="sm" onClick={retrySave}>重试保存</Button>}<Link className="btn" to="/recipe">配方库</Link></div>
    {dirty && <Notice title="候选配置尚未保存" tone="warn"><span>保存后再执行示教、验证和发布。</span><Button size="sm" variant="outlined" disabled={busy || !!previewError} onClick={() => void saveDoc()}>保存候选配置</Button></Notice>}
    {previewError && <Notice title="候选参数无效" tone="warn">{previewError}</Notice>}
    {error && <Notice title="操作未完成" tone="warn">{error}</Notice>}
    {publish?.state === "waiting" && <Notice title="待生效，候选已冻结">等待当前工件结束后切换；生效失败可在验证与发布页重试同一版本。</Notice>}
    {publish?.state === "failed" && <Notice title="生效失败，可重试同一版本" tone="warn">{publish.message}</Notice>}
    {publish?.state === "uncertain" && <Notice title="生效状态不确定，请先核实发布状态" tone="ng">{publish.message}</Notice>}
    {publish?.state === "active" && publish.warning && <Notice title={`v${publish.version} 已生效，但候选版本号未能更新`} tone="warn">{publish.warning}</Notice>}
  </>;
}
export function Steps() {
  const { data } = useWorkspace();
  return data?.plan ? <PlanSteps /> : null;
}

export function useGrayImage(id: string | null, imageId: string | null, historyId?: number | null, k = 0, view?: number) {
  const key=JSON.stringify([id,imageId,historyId,k,view]);
  const [state, setState] = useState<{key:string;image:GrayImage|null;error:string;loading:boolean}>({key,image:null,error:"",loading:false});
  useEffect(() => {
    let alive = true;
    setState({key,image:null,error:"",loading:!!imageId||!!historyId});
    if (!imageId && !historyId) return;
    const request = historyId ? workspaceApi.recordImage(historyId, k, view) : workspaceApi.image(id!, imageId!);
    request.then(image => alive && setState({key,image,error:"",loading:false})).catch(e => alive && setState({key,image:null,error:errorText(e),loading:false}));
    return () => { alive = false; };
  }, [id, imageId, historyId, k, view]);
  return state.key===key?state:{image:null,error:"",loading:!!imageId||!!historyId};
}

/** 叠加在原图上的内容，全部是这张图的像素坐标（示教中线就在这张图里点出；特征定位的拍照点按记录的位姿把中线与站点搬到当帧）。 */
export interface ImageOverlay {
  /** 示教中线（草稿或已保存的）；按位姿叠图时为搬移后的 N′ = T(N) */
  path: [number, number][];
  /** 各站位置（来自配方布局，中线保存后才有）；按位姿叠图时为 T(N_i) */
  stations?: [number, number][];
  searchHalfPx?: number;
  /** 各站颜色；不给时统一用强调色 */
  stationColors?: string[];
  /** labelOutward：标签沿横线方向放到 to 端外侧（横线即法向，避免压在胶路上） */
  segments?: { from: [number, number]; to: [number, number]; color: string; label?: string; labelOutward?: boolean }[];
  /** 中线改了还没保存：各站是旧的，淡显 */
  stale?: boolean;
  /** 选中的中线点 */
  selected?: number | null;
  /** 特征定位：示教框、带 4 px 外圈的模板框、搜索范围（框能移到的范围）与杠杆臂 L（模板中心 → 最远站）；blocked 时框标红 */
  fixture?: FixtureOverlay | null;
}
/** 四角多边形（像素中心坐标），见 cycle/pose.ts 的 movedBox */
export type BoxPolygon = Pt[];
export interface FixtureOverlay {
  rect: [number, number, number, number];
  padded: [number, number, number, number];
  /** 搜索范围：只有示教页给 */
  search?: [number, number, number, number];
  lever?: [[number, number], [number, number]] | null;
  blocked?: boolean;
  /** 按位姿搬移后的内框与外圈；有它时示教位置的框淡显 */
  moved?: { inner: BoxPolygon; padded: BoxPolygon } | null;
  /** 位姿没被采用的原因：框留在示教位置、用故障色虚线 */
  failed?: string | null;
  /** 示教图自定位被拒时的候选落点（故障色虚线），只在示教页画 */
  candidate?: BoxPolygon | null;
  /** 复测（重跑）的位姿与原记录不同时，复测定位框（虚线） */
  replay?: BoxPolygon | null;
}
const identityPoint = (p: Pt): Pt => p;
/** 定位模板外圈（与 recipe.rs 的 FIXTURE_TEMPLATE_PAD_PX 一致） */
const FIXTURE_PAD_PX = 4;
/**
 * 配方布局里拍照点 k 的示教中线与各站（图像像素），给了 vis 时各站按测量状态着色。
 * 给了 frame 且投影视图有特征定位时按该帧记录的位姿叠图：位姿被采用 → 中线、站点、内框与外圈都画在 T(·)；
 * 有位姿但没被采用 → 中线与站点留在示教位置、框标“定位未采用”；没有位姿（未测、测量出错）→ 只画示教中线。
 * 没有特征定位的拍照点与原来完全相同。
 */
export function shotOverlay(layout: Recipe | null | undefined, k: number, vis?: PointVis[], frame?: FrameView | null): ImageOverlay | undefined {
  const shot = layout?.shots[k];
  if (!layout || !shot || shot.path.length < 2) return undefined;
  const spec = viewSpec(shot, shot.view);
  if (spec?.fixture && spec.path.length >= 2) {
    const { rect } = spec.fixture, index = viewStations(layout, k, shot.view);
    const pose = framePose(frame, shot.view), T = poseTransform(rect, pose), move = T ?? identityPoint;
    const [x, y, w, h] = rect, p = FIXTURE_PAD_PX;
    return {
      path: spec.path.map(move),
      stations: index.map(j => move([layout.points.x[j], layout.points.y[j]])),
      stationColors: vis ? index.map(j => visColor[vis[j] ?? "none"]) : undefined,
      fixture: pose ? { rect, padded: [x - p, y - p, w + 2 * p, h + 2 * p], moved: T ? { inner: movedBox(rect, 0, T), padded: movedBox(rect, p, T) } : null, failed: T ? null : poseReasonText(pose.reason) } : null,
    };
  }
  const own = shotSegment(layout, k)?.segment;
  const index = own ? Array.from({ length: own.count }, (_, i) => own.first + i) : [];
  return {
    path: shot.path,
    stations: index.map(j => [layout.points.x[j], layout.points.y[j]] as [number, number]),
    stationColors: vis ? index.map(j => visColor[vis[j] ?? "none"]) : undefined,
  };
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
export function GrayViewer({ image, loading = false, error = "", label, caption, tools, subtools, overlay, onEdit, editDisabled = false, region, onRegion, onRegionDone, brush, onBrush, extractionFailure, emptyTitle, emptyHint, controlsAt = "header", summary }: {
  image: GrayImage | null; loading?: boolean; error?: string; label: string; caption?: ReactNode; tools?: ReactNode; subtools?: ReactNode; overlay?: ImageOverlay; onEdit?: PathEditing; editDisabled?: boolean; region?: [number, number, number, number] | null; onRegion?: (region: [number, number, number, number]) => void;
  /** 框选松手：一次拖动结束（定位框据此取整并做检查） */
  onRegionDone?: () => void;
  brush?: BrushGuide | null; onBrush?: (points: ImagePoint[]) => void; extractionFailure?: { point: ImagePoint; message: string } | null; emptyTitle?: string; emptyHint?: string; controlsAt?: "header" | "footer"; summary?: ReactNode;
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
  const stations = overlay?.stations ?? [];
  const gaps = stations.slice(1).map((p, i) => Math.hypot(p[0] - stations[i][0], p[1] - stations[i][1])).filter(d => d > 0).sort((a, b) => a - b);
  const stationGap = gaps.length ? gaps[gaps.length >> 1] * fit * zoom : Infinity;
  const handle = screenPx(7), dot = screenPx(Math.min(3, Math.max(1, stationGap / 2)));
  const editing = !!onEdit && shown && !editDisabled;
  const path = overlay?.path ?? [];
  const band = searchBand(path, overlay?.searchHalfPx);
  const hint = error || emptyHint;
  const footer = controlsAt === "footer";
  const controls = <div className="wp-zoom"><IconButton size="sm" aria-label="缩小原图" disabled={!image} onClick={() => zoomBy(1/1.25)}><Minus size={15} /></IconButton><span>{Math.round(zoom * 100)}%</span><IconButton size="sm" aria-label="放大原图" disabled={!image} onClick={() => zoomBy(1.25)}><Plus size={15} /></IconButton><IconButton size="sm" aria-label="适应窗口" disabled={!image} onClick={reset}><RotateCcw size={15} /></IconButton>{overlay && <Chip selected={shown} onClick={() => setShown(v => !v)}>{footer ? "中线" : "中线叠加"}</Chip>}{band && <Chip selected={bandShown} onClick={() => setBandShown(v => !v)}>搜索带</Chip>}{onEdit && overlay && <Chip selected={allHandles} disabled={!shown || editDisabled} title={footer ? "显示全部点柄" : undefined} onClick={() => setAllHandles(v => !v)}>{footer ? "点柄" : "编辑点"}</Chip>}</div>;
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
      }} onPointerUp={() => { const drawn = !!regionStart.current; stopGesture(); if (drawn) onRegionDone?.(); }} onPointerCancel={stopGesture} onLostPointerCapture={stopGesture}>
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
          {region && <rect x={region[0]} y={region[1]} width={region[2]} height={region[3]} fill="var(--primary)" fillOpacity={.12} stroke="var(--primary)" strokeWidth={2} vectorEffect="non-scaling-stroke" />}
          {overlay?.fixture && shown && (() => {
            const f = overlay.fixture, color = f.failed ? "var(--fault)" : f.blocked ? "var(--ng)" : "var(--ok)";
            const polygon = (corners: BoxPolygon, props: Record<string, unknown>) => <polygon points={corners.map(p => p.join(",")).join(" ")} fill="none" vectorEffect="non-scaling-stroke" {...props} />;
            const box = (r: [number, number, number, number], props: Record<string, unknown>) => polygon(movedBox(r, 0), props);
            return <g aria-label={f.failed ? "定位未采用：" + f.failed : f.moved ? "定位框（按位姿）" : f.blocked ? "定位框（未通过检查）" : "定位框"} pointerEvents="none">
              {f.search && box(f.search, { stroke: "var(--primary)", strokeWidth: 1, strokeDasharray: "6 4", opacity: .7, "aria-label": "定位搜索范围" })}
              {f.moved ? <>
                {box(f.rect, { stroke: "var(--on-surface-variant)", strokeWidth: 1, strokeDasharray: "2 3", opacity: .6, "aria-label": "定位框示教位置" })}
                {polygon(f.moved.padded, { stroke: color, strokeWidth: 1, strokeDasharray: "3 3", "aria-label": "模板外圈" })}
                {polygon(f.moved.inner, { stroke: color, strokeWidth: 2, "aria-label": "定位框" })}
              </> : <>
                {box(f.padded, { stroke: color, strokeWidth: 1, strokeDasharray: "3 3", "aria-label": "模板外圈" })}
                {box(f.rect, { stroke: color, strokeWidth: 2, strokeDasharray: f.failed ? "6 3" : undefined, "aria-label": "定位框" })}
              </>}
              {f.candidate && polygon(f.candidate, { stroke: "var(--fault)", strokeWidth: 1.5, strokeDasharray: "4 3", "aria-label": "定位候选落点（未采用）" })}
              {f.replay && polygon(f.replay, { stroke: "var(--warn)", strokeWidth: 1.5, strokeDasharray: "5 3", "aria-label": "复测定位框" })}
              {f.lever && <line x1={f.lever[0][0]} y1={f.lever[0][1]} x2={f.lever[1][0]} y2={f.lever[1][1]} stroke="var(--primary)" strokeWidth={1} strokeDasharray="2 4" vectorEffect="non-scaling-stroke" aria-label="杠杆臂 L" />}
            </g>;
          })()}
          {overlay && shown && <g aria-label="中线叠加">
            {stations.map(([x, y], i) => <circle key={"s" + i} cx={x} cy={y} r={dot} fill={overlay.stationColors?.[i] ?? "var(--primary)"} opacity={overlay.stale ? .35 : .9} />)}
            {path.length >= 2 && <polyline points={path.map(p => p.join(",")).join(" ")} fill="none" stroke="var(--primary)" strokeWidth={2} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />}
            {onEdit && path.map(([x, y], i) => (allHandles || path.length <= 24 || i === 0 || i === path.length - 1 || overlay.selected === i) && <circle key={"v" + i} data-vertex={i} aria-label={"中线点 " + (i + 1)} cx={x} cy={y} r={handle} fill={overlay.selected === i ? "var(--primary)" : "var(--surface-lowest)"} fillOpacity={.85} stroke={i === 0 ? "var(--ok)" : "var(--primary)"} strokeWidth={2} vectorEffect="non-scaling-stroke" />)}
            {overlay.segments && <g aria-label="测量横线" pointerEvents="none">
              {overlay.segments.map((segment, i) => <g key={i}>
                <line x1={segment.from[0]} y1={segment.from[1]} x2={segment.to[0]} y2={segment.to[1]} stroke={segment.color} strokeWidth={2} vectorEffect="non-scaling-stroke" />
                {segment.label && (segment.labelOutward ? (() => {
                  const dx = segment.to[0] - segment.from[0], dy = segment.to[1] - segment.from[1], length = Math.hypot(dx, dy) || 1;
                  const ux = dx / length, uy = dy / length, gap = screenPx(6);
                  return <text x={segment.to[0] + ux * gap} y={segment.to[1] + uy * gap} dy={screenPx(4)} textAnchor={ux < -.3 ? "end" : ux > .3 ? "start" : "middle"} fontSize={screenPx(12)} fill={segment.color} stroke="var(--surface-lowest)" strokeWidth={3} paintOrder="stroke" vectorEffect="non-scaling-stroke">{segment.label}</text>;
                })() : <text x={segment.to[0]} y={segment.to[1]} dx={screenPx(4)} dy={screenPx(-4)} fontSize={screenPx(12)} fill={segment.color} stroke="var(--surface-lowest)" strokeWidth={3} paintOrder="stroke" vectorEffect="non-scaling-stroke">{segment.label}</text>)}
              </g>)}
            </g>}
          </g>}
        </g></svg>}
    {footer ? <div className="wp-image-footer">{summary}{controls}</div> : image && <div className="wp-image-footer"><span>{image.width} × {image.height} px · Ctrl＋滚轮缩放 · 中键拖动 · + / − / 0</span><span>{onBrush ? "按住左键沿胶路刷过；每一笔重画选区，中键拖动视图" : onRegion ? "拖动框选一段连续胶路区域" : onEdit ? (path.length > 24 && !allHandles ? "密集中线仅显示端点；打开“编辑点”可调整全部点" : "点击图像依次加中线点（从胶嘴一侧往外，绿圈为起点），拖动点可调整") : "原始灰度图像"}</span></div>}
  </div>;
}
