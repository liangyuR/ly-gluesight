import { useEffect, useRef, useState, type PointerEvent, type ReactNode } from "react";
import { Link, NavLink } from "react-router-dom";
import { AlertTriangle, Camera, ChevronRight, Info, Minus, Plus, RotateCcw } from "lucide-react";
import { desktopAvailable } from "../../lib/desktop";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import type { PointVis, Recipe } from "../cycle/types";
import { shotSegment, visColor } from "../cycle/vis";
import type { GrayImage, Teaching } from "./types";
import { sameTeach, shotTeach } from "./teach";

export function Badge({ children, tone = "info" }: { children: ReactNode; tone?: "info" | "ok" | "ng" | "warn" | "neutral" }) {
  return <span className={"wp-badge " + tone}>{children}</span>;
}
export function Panel({ title, detail, actions, children, className = "" }: { title: string; detail?: string; actions?: ReactNode; children: ReactNode; className?: string }) {
  return <section className={"panel wp-panel " + className}><div className="wp-panel-head"><div><h2>{title}</h2>{detail && <p className="muted">{detail}</p>}</div>{actions}</div>{children}</section>;
}
export function Notice({ title, children, tone = "info" }: { title: string; children?: ReactNode; tone?: "info" | "ok" | "warn" | "ng" }) {
  return <div className={"wp-notice " + tone}>{tone === "info" || tone === "ok" ? <Info size={17} /> : <AlertTriangle size={17} />}<div><strong>{title}</strong>{children && <p>{children}</p>}</div></div>;
}
export function KV({ label, children }: { label: string; children: ReactNode }) {
  return <div className="wp-kv"><span>{label}</span><strong>{children}</strong></div>;
}
export function NumberField({ label, value, unit, onChange, min, max, step = 1, disabled }: { label: string; value: number; unit?: string; onChange: (value: number) => void; min?: number; max?: number; step?: number; disabled?: boolean }) {
  return <label className="field wp-field"><span>{label}</span><div className="wp-input-unit"><input className="input mono" aria-label={label} type="number" value={Number.isFinite(value) ? value : ""} min={min} max={max} step={step} disabled={disabled} onChange={e => onChange(e.target.value===""?NaN:Number(e.target.value))} />{unit && <small>{unit}</small>}</div></label>;
}
export function WorkspaceEmpty() {
  const { error } = useWorkspace();
  return <div className="wp-page">{error && <Notice title="无法加载候选配置" tone="warn">{error}</Notice>}<div className="wp-empty"><Camera size={32} /><h2>{desktopAvailable() ? "尚未选择配方" : "当前为浏览器查看模式"}</h2><p>{desktopAvailable() ? "在配方库选择或新建配方，再开始规划拍照点与示教中线。" : "请在桌面软件中连接实际数据；交互设计可在“操作流程预览”中查看。"}</p><Link className="btn primary" to={desktopAvailable() ? "/recipe" : "/workflow/guide"}>{desktopAvailable() ? "打开配方库" : "查看交互原型"}</Link></div></div>;
}
export function WorkspaceBar() {
  const { list, drafts, selectedId, select, data, doc, dirty, frameDirty, busy, previewError, error, notice, saveDoc } = useWorkspace();
  const choices = [...list.map(r => ({ id:r.id, name:r.name })), ...drafts.filter(w => !list.some(r => r.id === w.doc.id)).map(w => ({ id:w.doc.id, name:w.doc.name }))];
  return <><div className="wp-workbench-bar"><label className="wp-recipe-picker">当前配方<select className="input" value={selectedId ?? ""} disabled={busy} onChange={e => void select(e.target.value)}><option value="" disabled>选择配方</option>{choices.map(r => <option key={r.id} value={r.id}>{r.id} · {r.name}</option>)}</select></label>{data && <><Badge>候选 v{doc?.version}</Badge><Badge tone="neutral">生产 {data.productionVersion ? "v" + data.productionVersion : "未发布"}</Badge><span className="muted">修订 {data.workspace.revision}</span>{data.workspace.pending && <Badge tone="warn">v{data.workspace.pending.doc.version} 待工件结束后生效</Badge>}</>}<span className="spacer" /><Link className="btn" to="/recipe">配方库</Link></div>
    {dirty && <Notice title="候选配置尚未保存" tone="warn"><span>保存后再执行示教、验证和发布。</span><button className="btn small primary" disabled={busy || !!previewError} onClick={() => void saveDoc()}>保存候选配置</button></Notice>}
    {frameDirty && <Notice title="示教中线有未保存的修改" tone="warn">请在单帧示教中保存中线并重新试测，然后再验证候选。</Notice>}
    {previewError && <Notice title="候选参数无效" tone="warn">{previewError}</Notice>}
    {error && <Notice title="操作未完成" tone="warn">{error}</Notice>}
    {notice && <Notice title={notice} tone="ok" />}
    {data?.workspace.publishError && <Notice title="待生效版本发布失败" tone="warn">{data.workspace.publishError}</Notice>}
  </>;
}
export function Steps() {
  return <nav className="wp-steps" aria-label="配方配置步骤">{[["/recipe/geometry","拍照点规划"],["/recipe/teach","单帧示教"],["/recipe/overview","工件总览"],["/recipe/validation","验证与发布"]].map(([path,label],i) => <NavLink key={path} to={path} className={({isActive}) => isActive ? "active" : ""}><span>{i + 1}</span>{label}{i < 3 && <ChevronRight size={14} />}</NavLink>)}</nav>;
}

export function useGrayImage(id: string | null, imageId: string | null, historyId?: number | null, k = 0) {
  const key=JSON.stringify([id,imageId,historyId,k]);
  const [state, setState] = useState<{key:string;image:GrayImage|null;error:string;loading:boolean}>({key,image:null,error:"",loading:false});
  useEffect(() => {
    let alive = true;
    setState({key,image:null,error:"",loading:!!imageId||!!historyId});
    if (!imageId && !historyId) return;
    const request = historyId ? workspaceApi.recordImage(historyId, k) : workspaceApi.image(id!, imageId!);
    request.then(image => alive && setState({key,image,error:"",loading:false})).catch(e => alive && setState({key,image:null,error:String(e),loading:false}));
    return () => { alive = false; };
  }, [id, imageId, historyId, k]);
  return state.key===key?state:{image:null,error:"",loading:!!imageId||!!historyId};
}

/** 叠加在原图上的内容，全部是这张图的像素坐标（示教中线就在这张图里点出，不做对齐变换）。 */
export interface ImageOverlay {
  /** 示教中线（草稿或已保存的） */
  path: [number, number][];
  /** 各站位置（来自配方布局，中线保存后才有） */
  stations?: [number, number][];
  /** 各站颜色；不给时统一用强调色 */
  stationColors?: string[];
  /** 中线改了还没保存：各站是旧的，淡显 */
  stale?: boolean;
  /** 选中的中线点 */
  selected?: number | null;
}
/** 配方布局里拍照点 k 的示教中线与各站（图像像素），给了 vis 时各站按测量状态着色。 */
export function shotOverlay(layout: Recipe | null | undefined, k: number, vis?: PointVis[]): ImageOverlay | undefined {
  const shot = layout?.shots[k];
  if (!layout || !shot || shot.path.length < 2) return undefined;
  const own = shotSegment(layout, k)?.segment;
  const index = own ? Array.from({ length: own.count }, (_, i) => own.first + i) : [];
  return {
    path: shot.path,
    stations: index.map(j => [layout.points.x[j], layout.points.y[j]] as [number, number]),
    stationColors: vis ? index.map(j => visColor[vis[j] ?? "none"]) : undefined,
  };
}
/** 在原图上编辑中线：点空白处在末尾加点，拖动点调整位置，点一下选中。 */
export interface PathEditing {
  add: (p: [number, number]) => void;
  move: (index: number, p: [number, number]) => void;
  select: (index: number | null) => void;
}

export function GrayViewer({ image, loading = false, error = "", label, overlay, onEdit }: {
  image: GrayImage | null; loading?: boolean; error?: string; label: string; overlay?: ImageOverlay; onEdit?: PathEditing;
}) {
  const [zoom, setZoom] = useState(1);
  const [shown, setShown] = useState(true);
  const group = useRef<SVGGElement>(null);
  const drag = useRef<{ index: number; inverse: DOMMatrix } | null>(null);
  useEffect(() => { setZoom(1); drag.current = null; }, [image?.url]);
  const point = (e: PointerEvent<SVGSVGElement>, inverse?: DOMMatrix): [number, number] | null => {
    const transform = inverse ?? group.current?.getScreenCTM()?.inverse();
    if (!transform || !image) return null;
    const p = new DOMPoint(e.clientX,e.clientY).matrixTransform(transform);
    // 取 0.1 px：后端按 f32 存，取回的值与草稿一致
    const round = (v: number, max: number) => Math.round(Math.min(max, Math.max(0, v)) * 10) / 10;
    return [round(p.x, image.width), round(p.y, image.height)];
  };
  const size = image ? Math.max(image.width, image.height) : 1;
  const handle = size / 120, dot = size / 320;
  const editing = !!onEdit && shown;
  const path = overlay?.path ?? [];
  return <div className="wp-viewport"><div className="wp-image-tools"><span>{label}</span><div><button className="icon-btn" aria-label="缩小原图" onClick={() => setZoom(z => Math.max(.5,z-.25))}><Minus size={15} /></button><span>{Math.round(zoom * 100)}%</span><button className="icon-btn" aria-label="放大原图" onClick={() => setZoom(z => Math.min(4,z+.25))}><Plus size={15} /></button><button className="icon-btn" aria-label="适应窗口" onClick={() => setZoom(1)}><RotateCcw size={15} /></button>{overlay && <button className={"btn small" + (shown ? " active" : "")} aria-pressed={shown} onClick={() => setShown(v => !v)}>中线叠加</button>}</div></div>
    {!image ? <div className="wp-image-empty"><Camera size={32} /><strong>{loading ? "正在读取原图…" : error ? "原图不可用" : "尚未冻结本帧图像"}</strong><span>{error || "取样后在这张图上点出胶路中线。机器人应停在当前拍照点。"}</span></div> :
      <svg role="img" className={"wp-gray-image" + (editing ? " editable" : "")} viewBox={"0 0 " + image.width + " " + image.height} aria-label={label} onPointerDown={e => {
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
        const current = drag.current;
        if (!current || !onEdit) return;
        const p = point(e, current.inverse);
        if (p) onEdit.move(current.index, p);
      }} onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }}>
        <g ref={group} transform={"translate(" + image.width/2 + " " + image.height/2 + ") scale(" + zoom + ") translate(" + -image.width/2 + " " + -image.height/2 + ")"}><image href={image.url} width={image.width} height={image.height} />
          {overlay && shown && <g aria-label="中线叠加">
            {overlay.stations?.map(([x, y], i) => <circle key={"s" + i} cx={x} cy={y} r={dot} fill={overlay.stationColors?.[i] ?? "var(--accent)"} opacity={overlay.stale ? .35 : .9} />)}
            {path.length >= 2 && <polyline points={path.map(p => p.join(",")).join(" ")} fill="none" stroke="var(--accent-text)" strokeWidth={2} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />}
            {onEdit && path.map(([x, y], i) => <circle key={"v" + i} data-vertex={i} aria-label={"中线点 " + (i + 1)} cx={x} cy={y} r={handle} fill={overlay.selected === i ? "var(--accent)" : "var(--bg)"} fillOpacity={.85} stroke={i === 0 ? "var(--ok)" : "var(--accent-text)"} strokeWidth={2} vectorEffect="non-scaling-stroke" />)}
          </g>}
        </g></svg>}
    {image && <div className="wp-image-footer"><span>{image.width} × {image.height} px</span><span>{onEdit ? "点击图像依次加中线点（从胶嘴一侧往外，绿圈为起点），拖动点可调整" : "原始灰度图像"}</span></div>}
  </div>;
}

function FrameThumbnail({ id, frame }: { id: string; frame: Teaching }) {
  const {image} = useGrayImage(id, frame.image?.id ?? null);
  return <span className="wp-frame-thumbnail">{image ? <img src={image.url} alt="" /> : <Camera size={19} />}</span>;
}
export function FrameRail({ id, frames, selected, onSelect, disabled }: { id: string; frames: Teaching[]; selected: number; onSelect: (k: number) => void; disabled?: boolean }) {
  const { frameDrafts, data } = useWorkspace();
  return <div className="wp-frame-list" aria-label="示教帧选择">{frames.map(frame => {
    const shot = data?.workspace.doc.shots[frame.k];
    const dirty = !!frameDrafts[frame.k] && !!shot && !sameTeach(frameDrafts[frame.k], shotTeach(shot));
    const status = shot?.skip ? "不检" : dirty ? "中线待保存" : frame.saved ? "已保存" : frame.trial ? frame.trial.passed ? "试测通过" : "试测未通过" : shot && shot.path.length < 2 ? frame.image ? "待点中线" : "待取样" : frame.image ? "待试测" : "待取样";
    return <button key={frame.k} className={"wp-frame-item " + (selected === frame.k ? "selected" : "")} aria-label={"选择帧 k" + (frame.k + 1)} aria-pressed={selected === frame.k} onClick={() => onSelect(frame.k)} disabled={disabled}>
      <FrameThumbnail id={id} frame={frame} /><strong>k{frame.k + 1}{shot ? " · " + shot.id : ""}<small>{shot ? `${shot.camera} · 视角 ${shot.view}` : frame.image?.camera ?? "未取样"}</small></strong><span>{status}</span>
    </button>;
  })}</div>;
}
