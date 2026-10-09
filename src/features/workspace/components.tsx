import { useEffect, useMemo, useRef, useState, type PointerEvent, type ReactNode } from "react";
import { Link, NavLink } from "react-router-dom";
import { AlertTriangle, Camera, ChevronRight, Info, Minus, Plus, RotateCcw } from "lucide-react";
import { desktopAvailable } from "../../lib/desktop";
import type { Recipe } from "../cycle/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import type { FrameParams, GrayImage, Teaching } from "./types";

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
  return <div className="wp-page">{error && <Notice title="无法加载候选配置" tone="warn">{error}</Notice>}<div className="wp-empty"><Camera size={32} /><h2>{desktopAvailable() ? "尚未选择配方" : "当前为浏览器查看模式"}</h2><p>{desktopAvailable() ? "在配方库选择或新建配方，再开始配置胶路与示教。" : "请在桌面软件中连接实际数据；交互设计可在“操作流程预览”中查看。"}</p><Link className="btn primary" to={desktopAvailable() ? "/recipe" : "/workflow/guide"}>{desktopAvailable() ? "打开配方库" : "查看交互原型"}</Link></div></div>;
}
export function WorkspaceBar() {
  const { list, drafts, selectedId, select, data, doc, dirty, frameDirty, busy, previewError, error, notice, saveDoc } = useWorkspace();
  const choices = [...list.map(r => ({ id:r.id, name:r.name })), ...drafts.filter(w => !list.some(r => r.id === w.doc.id)).map(w => ({ id:w.doc.id, name:w.doc.name }))];
  return <><div className="wp-workbench-bar"><label className="wp-recipe-picker">当前配方<select className="input" value={selectedId ?? ""} disabled={busy} onChange={e => void select(e.target.value)}><option value="" disabled>选择配方</option>{choices.map(r => <option key={r.id} value={r.id}>{r.id} · {r.name}</option>)}</select></label>{data && <><Badge>候选 v{doc?.version}</Badge><Badge tone="neutral">生产 {data.productionVersion ? "v" + data.productionVersion : "未发布"}</Badge><span className="muted">修订 {data.workspace.revision}</span>{data.workspace.pending && <Badge tone="warn">v{data.workspace.pending.doc.version} 待工件结束后生效</Badge>}</>}<span className="spacer" /><Link className="btn" to="/recipe">配方库</Link></div>
    {dirty && <Notice title="候选配置尚未保存" tone="warn"><span>保存后再执行示教、验证和发布。</span><button className="btn small primary" disabled={busy || !!previewError} onClick={() => void saveDoc()}>保存候选配置</button></Notice>}
    {frameDirty && <Notice title="本帧参数有未保存的修改" tone="warn">请在单帧示教中保存参数草稿或重新试测，然后再验证候选。</Notice>}
    {previewError && <Notice title="候选参数无效" tone="warn">{previewError}</Notice>}
    {error && <Notice title="操作未完成" tone="warn">{error}</Notice>}
    {notice && <Notice title={notice} tone="ok" />}
    {data?.workspace.publishError && <Notice title="待生效版本发布失败" tone="warn">{data.workspace.publishError}</Notice>}
  </>;
}
export function Steps() {
  return <nav className="wp-steps" aria-label="配方配置步骤">{[["/recipe/geometry","胶路与拍照规划"],["/recipe/teach","单帧示教"],["/recipe/overview","工件总览"],["/recipe/validation","验证与发布"]].map(([path,label],i) => <NavLink key={path} to={path} className={({isActive}) => isActive ? "active" : ""}><span>{i + 1}</span>{label}{i < 3 && <ChevronRight size={14} />}</NavLink>)}</nav>;
}

export function useGrayImage(id: string | null, imageId: string | null, historyId?: number | null, k = 0) {
  const [image, setImage] = useState<GrayImage | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    let alive = true;
    setImage(null); setError(""); setLoading(false);
    if (!imageId && !historyId) return;
    setLoading(true);
    const request = historyId ? workspaceApi.recordImage(historyId, k) : workspaceApi.image(id!, imageId!);
    request.then(image => alive && setImage(image)).catch(e => alive && setError(String(e))).finally(() => alive && setLoading(false));
    return () => { alive = false; };
  }, [id, imageId, historyId, k]);
  return { image, error, loading };
}

export function GrayViewer({ image, loading = false, error = "", label, params, layout, k = 0, onRect }: {
  image: GrayImage | null; loading?: boolean; error?: string; label: string; params?: FrameParams; layout?: Recipe; k?: number;
  onRect?: (rect: [number, number, number, number]) => void;
}) {
  const [zoom, setZoom] = useState(1);
  const [overlay, setOverlay] = useState(true);
  const group = useRef<SVGGElement>(null);
  const drag = useRef<{ start: [number, number]; inverse: DOMMatrix } | null>(null);
  useEffect(() => { setZoom(1); drag.current = null; }, [image?.url]);
  const points = useMemo(() => {
    if (!layout || !params || !image || !layout.shots[k]) return [];
    if (![params.dx, params.dy, params.deg, params.mmPerPx].every(Number.isFinite) || params.mmPerPx <= 0) return [];
    const [cx,cy] = layout.shots[k].center;
    const sin = Math.sin(params.deg * Math.PI / 180), cos = Math.cos(params.deg * Math.PI / 180);
    const groups:string[][]=[];let group:string[]|null=null;
    layout.points.k.forEach((owner,j)=>{
      if(owner!==k){group=null;return;}
      if(!group){group=[];groups.push(group);}
      group.push(String(image.width / 2 + params.dx + (layout.points.x[j] - cx) / params.mmPerPx * cos - (layout.points.y[j] - cy) / params.mmPerPx * sin) + "," +
        String(image.height / 2 + params.dy + (layout.points.x[j] - cx) / params.mmPerPx * sin + (layout.points.y[j] - cy) / params.mmPerPx * cos));
    });
    return groups.map(group=>group.join(" "));
  }, [layout, params, image, k]);
  const point = (e: PointerEvent<SVGSVGElement>, inverse?: DOMMatrix): [number, number] | null => {
    const transform = inverse ?? group.current?.getScreenCTM()?.inverse();
    if (!transform || !image) return null;
    const p = new DOMPoint(e.clientX,e.clientY).matrixTransform(transform);
    return [Math.round(Math.min(image.width, Math.max(0,p.x))), Math.round(Math.min(image.height, Math.max(0,p.y)))];
  };
  return <div className="wp-viewport"><div className="wp-image-tools"><span>{label}</span><div><button className="icon-btn" aria-label="缩小原图" onClick={() => setZoom(z => Math.max(.5,z-.25))}><Minus size={15} /></button><span>{Math.round(zoom * 100)}%</span><button className="icon-btn" aria-label="放大原图" onClick={() => setZoom(z => Math.min(4,z+.25))}><Plus size={15} /></button><button className="icon-btn" aria-label="适应窗口" onClick={() => setZoom(1)}><RotateCcw size={15} /></button>{params && <button className={"btn small" + (overlay ? " active" : "")} aria-pressed={overlay} onClick={() => setOverlay(v => !v)}>测量叠加</button>}</div></div>
    {!image ? <div className="wp-image-empty"><Camera size={32} /><strong>{loading ? "正在读取原图…" : error ? "原图不可用" : "尚未冻结本帧图像"}</strong><span>{error || "取样后，图像与参数绑定。机器人应停在当前拍照点。"}</span></div> :
      <svg role="img" className={"wp-gray-image" + (onRect ? " editable" : "")} viewBox={"0 0 " + image.width + " " + image.height} aria-label={label} onPointerDown={e => {
        if (!onRect) return;
        const inverse = group.current?.getScreenCTM()?.inverse();
        const start = inverse ? point(e, inverse) : null;
        if (start && inverse) {
          // 参数提示可能在首次移动时改变页面布局，整次拖动使用按下时的图像坐标。
          drag.current = { start, inverse };
          e.currentTarget.setPointerCapture(e.pointerId);
        }
      }} onPointerMove={e => {
        const current = drag.current;
        if (!current || !onRect) return;
        const p = point(e, current.inverse), start = current.start;
        if (!p) return;
        onRect([Math.min(start[0],p[0]),Math.min(start[1],p[1]),Math.abs(p[0]-start[0]),Math.abs(p[1]-start[1])]);
      }} onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }}>
        <g ref={group} transform={"translate(" + image.width/2 + " " + image.height/2 + ") scale(" + zoom + ") translate(" + -image.width/2 + " " + -image.height/2 + ")"}><image href={image.url} width={image.width} height={image.height} />
          {overlay && params && <>{points.map((path,i)=><polyline key={i} points={path} fill="none" stroke="var(--accent-text)" strokeWidth={1.5} vectorEffect="non-scaling-stroke" />)}<rect x={params.rect[0]} y={params.rect[1]} width={params.rect[2]} height={params.rect[3]} fill="var(--accent-overlay)" stroke="var(--accent-text)" strokeDasharray="6 4" vectorEffect="non-scaling-stroke" /></>}
        </g></svg>}
    {image && <div className="wp-image-footer"><span>{image.width} × {image.height} px</span><span>{onRect ? "拖出定位模板；包含内边与特征，避开胶条" : "原始灰度图像"}</span></div>}
  </div>;
}

function FrameThumbnail({ id, frame }: { id: string; frame: Teaching }) {
  const {image} = useGrayImage(id, frame.image?.id ?? null);
  return <span className="wp-frame-thumbnail">{image ? <img src={image.url} alt="" /> : <Camera size={19} />}</span>;
}
export function FrameRail({ id, frames, selected, onSelect, disabled }: { id: string; frames: Teaching[]; selected: number; onSelect: (k: number) => void; disabled?: boolean }) {
  const { frameDrafts } = useWorkspace();
  return <div className="wp-frame-list" aria-label="示教帧选择">{frames.map(frame => {
    const dirty = frameDrafts[frame.k] && JSON.stringify(frameDrafts[frame.k]) !== JSON.stringify(frame.params);
    return <button key={frame.k} className={"wp-frame-item " + (selected === frame.k ? "selected" : "")} aria-label={"选择帧 k" + (frame.k + 1)} aria-pressed={selected === frame.k} onClick={() => onSelect(frame.k)} disabled={disabled}>
      <FrameThumbnail id={id} frame={frame} /><strong>k{frame.k + 1}<small>{frame.image?.camera ?? "未取样"}</small></strong><span>{dirty ? "参数待试测" : frame.saved ? "已保存" : frame.trial ? frame.trial.passed ? "试测通过" : "试测失败" : frame.image ? "待试测" : "待取样"}</span>
    </button>;
  })}</div>;
}
