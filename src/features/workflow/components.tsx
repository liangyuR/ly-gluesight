import { useEffect, useId, useRef, useState, type ReactNode, type PointerEvent } from "react";
import { AlertTriangle, Camera, Check, ChevronRight, Info, LoaderCircle, Minus, Plus, RotateCcw, X } from "lucide-react";
import { useWorkflow } from "./context";
import { initialPositions, type RecipeConfiguration, type TeachFrame, type Verdict, type View, type WorkflowState } from "./model";
import { screens } from "./catalog";

export function Badge({ children, tone = "info" }: { children: ReactNode; tone?: "info" | "ok" | "ng" | "warn" | "neutral" }) {
  return <span className={"wf-badge wf-" + tone}>{children}</span>;
}
export function VerdictBadge({ verdict }: { verdict: Verdict }) {
  return <Badge tone={verdict === "OK" ? "ok" : verdict === "NG" ? "ng" : "warn"}>{verdict === "ERR" ? "ERR · 测量异常" : verdict}</Badge>;
}
export function Panel({ title, detail, actions, children, className = "" }: { title: string; detail?: string; actions?: ReactNode; children: ReactNode; className?: string }) {
  return <section className={"panel wf-panel " + className}><div className="wf-panel-head"><div><h2>{title}</h2>{detail && <p className="wf-caption">{detail}</p>}</div>{actions}</div>{children}</section>;
}
export function Notice({ title, children, tone = "info" }: { title: string; children?: ReactNode; tone?: "info" | "ok" | "warn" | "ng" }) {
  const Icon = tone === "ok" ? Check : tone === "warn" || tone === "ng" ? AlertTriangle : Info;
  return <div className={"wf-notice wf-" + tone}><Icon size={17} aria-hidden="true" /><div><strong>{title}</strong>{children && <p>{children}</p>}</div></div>;
}
export function KV({ label, children }: { label: string; children: ReactNode }) {
  return <div className="wf-kv"><span>{label}</span><strong>{children}</strong></div>;
}
export function NumberField({ label, value, unit, onChange, min = 0, max = 10000, step = 1, disabled, help }: { label: string; value: number; unit?: string; onChange: (value: number) => void; min?: number; max?: number; step?: number; disabled?: boolean; help?: string }) {
  return <label className="wf-field"><span>{label}</span><div className="wf-input-unit"><input className="input" aria-label={label} type="number" value={value} min={min} max={max} step={step} disabled={disabled} aria-invalid={value < min || value > max} onChange={e => { const n = Number(e.target.value); if (Number.isFinite(n)) onChange(n); }} onBlur={() => { if (value < min || value > max) onChange(Math.min(max, Math.max(min, value))); }} />{unit && <span>{unit}</span>}</div>{help && <small>{help}</small>}</label>;
}
export function SelectField({ label, value, options, onChange, disabled }: { label: string; value: string; options: string[]; onChange: (value: string) => void; disabled?: boolean }) {
  return <label className="wf-field"><span>{label}</span><select className="input" value={value} onChange={e => onChange(e.target.value)} disabled={disabled}>{options.map(o => <option key={o}>{o}</option>)}</select></label>;
}
export function Steps({ active }: { active: View }) {
  const { go } = useWorkflow();
  const views: View[] = ["geometry", "teach", "overview", "validation"];
  return <nav className="wf-steps" aria-label="配方配置步骤">{views.map((id, i) => <button key={id} className={id === active ? "active" : ""} onClick={() => go(id)} aria-current={id === active ? "step" : undefined}><span>{i + 1}</span>{screens.find(s => s.id === id)?.label}{i < 3 && <ChevronRight size={14} />}</button>)}</nav>;
}
export function FrameList({ frames, selected, onSelect, horizontal, defect, missing, disabled, labels }: { frames: TeachFrame[]; selected: number; onSelect: (id: number) => void; horizontal?: boolean; defect?: number; missing?: number; disabled?: boolean; labels?: string[] }) {
  const paths = ["M8 32V22Q8 8 22 8H92", "M8 12H92", "M8 8H78Q92 8 92 22V32", "M92 8V20Q92 30 78 30H8", "M8 26H92", "M8 8V20Q8 30 22 30H92"];
  return <div className={"wf-frame-list " + (horizontal ? "horizontal" : "")} aria-label="帧选择">{frames.map(f => {
    const status = labels?.[f.id - 1] ?? (missing === f.id ? "原图缺失" : defect === f.id ? "断胶" : f.saved ? "已保存" : f.imageId === null ? "待取样" : f.trial ? (f.trial.pass ? "试匹配通过" : "定位失败") : "待试匹配");
    return <button key={f.id} className={"wf-frame-item " + (selected === f.id ? "selected " : "") + (missing === f.id ? "is-warn" : defect === f.id ? "is-ng" : "")} aria-pressed={selected === f.id} aria-label={"选择帧 k" + f.id} disabled={disabled} onClick={() => onSelect(f.id)}>
      <span className="wf-frame-mini"><svg viewBox="0 0 100 36" aria-hidden="true"><path d={paths[f.id - 1]} fill="none" stroke="currentColor" strokeWidth="3" /></svg></span>
      <span className="wf-frame-name">k{f.id}<small>相机 1</small></span><span className="wf-frame-status">{status}</span>
    </button>;
  })}</div>;
}
const beadPaths = [
  "M 86 366 L 86 152 Q 86 84 155 84 L 684 84",
  "M 34 204 L 686 204",
  "M 36 84 L 552 84 Q 628 84 628 160 L 628 379",
  "M 626 36 L 626 284 Q 626 352 558 352 L 34 352",
  "M 34 238 L 686 238",
  "M 86 36 L 86 286 Q 86 352 155 352 L 684 352",
];
export function FrameCanvas({ id, imageId, overlay = false, failed = false, defect = false, missing = false, label = "冻结样本" }: { id: number; imageId: number | null; overlay?: boolean; failed?: boolean; defect?: boolean; missing?: boolean; label?: string }) {
  const uid = useId().replace(/:/g, "");
  const [zoom, setZoom] = useState(1);
  const [showOverlay, setShowOverlay] = useState(true);
  const path = beadPaths[(id - 1) % 6];
  const available = imageId !== null && !missing;
  useEffect(() => { setZoom(1); }, [id, imageId]);
  return <div className="wf-viewport">
    <div className="wf-image-tools"><span>{label} · k{id}</span><div><button className="icon-btn" aria-label="缩小图像" disabled={!available} onClick={() => setZoom(z => Math.max(0.75, z - 0.25))}><Minus size={15} /></button><span className="wf-caption">{Math.round(zoom * 100)}%</span><button className="icon-btn" aria-label="放大图像" disabled={!available} onClick={() => setZoom(z => Math.min(3, z + 0.25))}><Plus size={15} /></button><button className="icon-btn" aria-label="适应窗口" disabled={!available} onClick={() => setZoom(1)}><RotateCcw size={14} /></button>{overlay && available && <button className={"wf-layer-btn " + (showOverlay ? "active" : "")} onClick={() => setShowOverlay(v => !v)} aria-pressed={showOverlay}>测量叠加</button>}</div></div>
    {imageId === null || missing ? <div className="wf-image-empty"><Camera size={35} /><strong>{missing ? "该帧原图不可用" : "尚未冻结本帧样本"}</strong><span>{missing ? "测量数据仍可回放，原图复测不可用" : "取一帧后，图像和本帧参数将绑定"}</span></div> :
      <svg className="wf-camera-svg" viewBox="0 0 720 430" role="img" aria-label={"帧 k" + id + " 的" + label + (defect && id === 3 ? "，含断胶标记" : "")}>
        <defs><pattern id={"grid" + uid} width="24" height="24" patternUnits="userSpaceOnUse"><path d="M24 0H0V24" fill="none" stroke="#222" strokeWidth=".6" /></pattern><linearGradient id={"metal" + uid} x2="1" y2="1"><stop stopColor="#242424" /><stop offset=".5" stopColor="#383838" /><stop offset="1" stopColor="#181818" /></linearGradient><clipPath id={"clip" + uid}><rect width="720" height="430" /></clipPath></defs>
        <rect width="720" height="430" fill="#101010" /><rect width="720" height="430" fill={"url(#grid" + uid + ")"} />
        <g clipPath={"url(#clip" + uid + ")"}><g transform={"translate(360 215) scale(" + zoom + ") translate(-360 -215)"}>
          <path d={path} transform={"translate(0 " + (id > 3 ? -35 : 35) + ")"} fill="none" stroke={"url(#metal" + uid + ")"} strokeWidth="102" />
          <path d={path} transform={"translate(0 " + (id > 3 ? -32 : 32) + ")"} fill="none" stroke="#666" strokeWidth="2" />
          <path d={path} fill="none" stroke="#0e0e0e" strokeWidth="21" strokeLinecap="round" />
          <path d={path} fill="none" stroke={failed ? "#767676" : "#c7c7c7"} strokeWidth="11" strokeLinecap="round" />
          <path d={path} fill="none" stroke={failed ? "#858585" : "#e3e3e3"} strokeWidth="3" />
          {defect && id === 3 && <rect x="440" y="70" width="38" height="29" fill="#101010" />}
          {overlay && showOverlay && <><rect x="24" y="38" width="672" height="354" rx="4" fill="none" stroke={failed ? "var(--warn)" : "var(--accent-text)"} strokeDasharray="7 5" /><path d={path} fill="none" stroke={failed ? "var(--warn)" : "var(--accent-text)"} strokeWidth="2" strokeDasharray={failed ? "8 8" : undefined} /><path d={path} transform="translate(0 10)" fill="none" stroke="var(--accent)" strokeWidth="1" />{defect && id === 3 && <><rect x="426" y="48" width="66" height="69" fill="var(--ng-soft)" fillOpacity=".45" stroke="var(--ng)" strokeWidth="2" /><text x="421" y="139" fill="var(--ng)" fontSize="15">断胶 6.2 mm</text></>}</>}
        </g></g>
        <rect x="552" y="396" width="154" height="24" rx="3" fill="#0a0a0a" /><text x="562" y="413" fill="#a3a3a3" fontSize="12">示意图 · 样本 {String(imageId).padStart(6, "0")}</text>
      </svg>}
    <div className="wf-image-footer"><span>{missing ? "原图不可用" : imageId === null ? "图像未就绪" : "image#" + String(imageId).padStart(6, "0")}</span><span>相机 1 · k{id} · 图像与参数绑定</span></div>
  </div>;
}

export function OverviewMap({ editable = false, physical = false, defect = false, missing = false, recipe, overview }: { editable?: boolean; physical?: boolean; defect?: boolean; missing?: boolean; recipe?: RecipeConfiguration; overview?: WorkflowState["overview"] }) {
  const { state: s, dispatch } = useWorkflow();
  const g = recipe ?? s.recipe;
  const display = overview ?? s.overview;
  const ref = useRef<SVGSVGElement>(null);
  const drag = useRef<{ id: number; clientX: number; clientY: number; x: number; y: number } | null>(null);
  const positions = physical ? initialPositions : display.positions;
  const select = (id: number) => dispatch({ type: "select-frame", id });
  const move = (id: number, x: number, y: number) => dispatch({ type: "overview", patch: { saved: false, positions: s.overview.positions.map((p, i) => i + 1 === id ? { x: Math.min(0.97, Math.max(0.03, x)), y: Math.min(0.95, Math.max(0.05, y)) } : p) } });
  const down = (e: PointerEvent<SVGGElement>, id: number) => {
    select(id);
    if (!editable) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const p = positions[id - 1]; drag.current = { id, clientX: e.clientX, clientY: e.clientY, ...p };
  };
  const onMove = (e: PointerEvent<SVGGElement>) => {
    if (!drag.current || !ref.current) return;
    const rect = ref.current.getBoundingClientRect();
    const scale = Math.min(rect.width / 800, rect.height / 420);
    if (!Number.isFinite(scale) || scale <= 0) return;
    const d = drag.current;
    move(d.id, d.x + (e.clientX - d.clientX) / (520 * scale), d.y + (e.clientY - d.clientY) / (230 * scale));
  };
  const bw = physical ? g.fovWidth / Math.max(1, g.width) * 520 : 124;
  const bh = physical ? g.fovHeight / Math.max(1, g.height) * 230 : 88;
  return <svg ref={ref} className={"wf-overview-svg " + (editable ? "editable" : "")} viewBox="0 0 800 420" aria-label={physical ? "胶路与六帧物理视野规划" : "工件总览，点击帧框选择原图"}>
    <rect width="800" height="420" fill="#080808" />
    {!physical && display.background ? <image href={display.background} x="116" y="48" width="568" height="282" preserveAspectRatio="xMidYMid meet" opacity=".75" /> : <><rect x="130" y="64" width="540" height="250" rx="36" fill="#171717" stroke="#424242" strokeWidth="2" /><rect x="166" y="100" width="468" height="178" rx="15" fill="#0a0a0a" stroke="#333" /></>}
    <rect x="140" y="74" width="520" height="230" rx={Math.max(0, g.radius) / Math.max(1, g.width) * 520} fill="none" stroke="var(--accent-text)" strokeWidth="4" />
    {defect && <path d="M612 74H634" stroke="var(--ng)" strokeWidth="6" />}
    {positions.map((p, i) => {
      const selected = s.selectedFrame === i + 1;
      const tone = (defect || missing) && i === 2 ? (missing ? "var(--warn)" : "var(--ng)") : selected ? "var(--accent-text)" : "#686868";
      return <g key={i} role="button" tabIndex={0} aria-label={"总览帧 k" + (i + 1)} aria-pressed={selected} onClick={() => select(i + 1)} onPointerDown={e => down(e, i + 1)} onPointerMove={onMove} onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }} onKeyDown={e => {
        if (e.key === "Enter" || e.key === " ") { e.preventDefault(); select(i + 1); }
        if (editable && e.key.startsWith("Arrow")) { e.preventDefault(); move(i + 1, p.x + (e.key === "ArrowRight" ? .01 : e.key === "ArrowLeft" ? -.01 : 0), p.y + (e.key === "ArrowDown" ? .01 : e.key === "ArrowUp" ? -.01 : 0)); }
      }} transform={"translate(" + (140 + p.x * 520) + " " + (74 + p.y * 230) + ")"}>
        <rect x={-bw / 2} y={-bh / 2} width={bw} height={bh} rx="4" fill={selected ? "var(--accent-soft)" : "#111"} fillOpacity={selected ? .56 : .2} stroke={tone} strokeWidth={selected ? 2.5 : 1.2} strokeDasharray={physical ? "5 3" : undefined} />
        <rect x="-22" y="-12" width="44" height="24" rx="4" fill="#080808" stroke={tone} /><text textAnchor="middle" y="5" fill={tone} fontSize="14" fontWeight="600">k{i + 1}</text>
        {editable && <rect x={bw / 2 - 4} y={bh / 2 - 4} width="8" height="8" fill="var(--accent)" />}
      </g>;
    })}
    <path d="M140 352H660M140 346V358M660 346V358" stroke="#616161" /><text x="400" y="375" textAnchor="middle" fill="var(--text-muted)" fontSize="13">{g.width} mm</text>
    <text x="400" y="410" textAnchor="middle" fill="var(--text-muted)" fontSize="12">{physical ? "物理视野 · 包含本帧搜索余量" : editable ? "拖动或用方向键调整显示框 · 测量坐标保持独立" : "点击任意帧框，查看该帧原图"}</text>
  </svg>;
}
export function Curve({ defect = false, invalid = false, recipe }: { defect?: boolean; invalid?: boolean; recipe?: RecipeConfiguration }) {
  const { state } = useWorkflow();
  const g = recipe ?? state.recipe;
  const uid = useId().replace(/:/g, "");
  const length = 2 * (g.width + g.height - 4 * g.radius) + 2 * Math.PI * g.radius;
  const low = Math.min(2, g.target - g.tolerance - .5), high = Math.max(4.5, g.target + g.tolerance + .5);
  const y = (v: number) => 107 - (v - low) / (high - low) * 92;
  const points = Array.from({ length: 65 }, (_, i) => (28 + i * 11.3) + "," + y(3.3 + Math.sin(i * .6) * .12 + Math.sin(i * .17) * .15)).join(" ");
  const gapStart = invalid ? length / 3 : 352.4, gapEnd = invalid ? length / 2 : 358.6;
  const gapX = 28 + gapStart / length * 725, gapWidth = Math.max(3, (gapEnd - gapStart) / length * 725);
  return <div className="wf-curve"><div className="wf-row"><strong>距内边距离 d <span className="wf-caption">/ mm</span></strong><span className="wf-caption">沿胶路弧长 / mm</span></div><svg viewBox="0 0 790 138" role="img" aria-label={invalid ? "测量无效的曲线" : defect ? "含断胶区间的距离曲线" : "距离测量曲线"}>
    <defs><clipPath id={"curve" + uid}><rect x="28" y="15" width="725" height="92" /></clipPath></defs>
    <rect x="28" y={y(g.target + g.tolerance)} width="725" height={y(g.target - g.tolerance) - y(g.target + g.tolerance)} fill="var(--accent-soft)" />
    {[2, 3, 4].map(v => <g key={v}><path d={"M28 " + y(v) + "H753"} stroke="#2a2a2a" strokeDasharray="4 4" /><text x="7" y={y(v) + 4} fill="var(--text-muted)" fontSize="11">{v}</text></g>)}
    <g clipPath={"url(#curve" + uid + ")"}><polyline points={points} fill="none" stroke="var(--accent-text)" strokeWidth="2" />
      {(defect || invalid) && <><rect x={gapX} y="16" width={gapWidth} height="91" fill={invalid ? "var(--warn-soft)" : "var(--ng-soft)"} /><path d={"M" + gapX + " " + y(3.3) + "h" + gapWidth} stroke={invalid ? "var(--warn)" : "var(--ng)"} strokeWidth="2" strokeDasharray="3 2" /></>}
    </g>{defect && <text x={gapX + 8} y="21" fill="var(--ng)" fontSize="11">6.2 mm</text>}<path d="M28 107H753" stroke="#555" />{[0, 1, 2, 3, 4].map(i => <text key={i} x={28 + i * 181} y="126" textAnchor={i === 0 ? "start" : i === 4 ? "end" : "middle"} fill="var(--text-muted)" fontSize="11">{(i * length / 4).toFixed(0)}</text>)}
  </svg><div className="wf-caption">{invalid ? "k3 数据无效，不能判为胶路缺陷" : defect ? "断胶区间 352.4–358.6 mm · 6.2 mm · 关联 k3" : "基准 " + g.target + " mm · 容差 ±" + g.tolerance + " mm"} <span>胶宽 w 单独判定</span></div></div>;
}
export function Dialog({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null);
  const id = useId();
  useEffect(() => {
    const d = ref.current;
    const trigger = document.activeElement as HTMLElement | null;
    if (d && !d.open) d.showModal();
    return () => { d?.close(); if (trigger?.isConnected) trigger.focus(); };
  }, []);
  return <dialog ref={ref} className="wf-dialog" aria-labelledby={id} onKeyDown={e => {
    if (e.key !== "Tab") return;
    const controls = Array.from(e.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled), a[href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex="0"]')).filter(el => el.getClientRects().length > 0);
    const first = controls[0], last = controls.at(-1);
    if (!first || !last) { e.preventDefault(); return; }
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  }} onCancel={e => { e.preventDefault(); onClose(); }} onClick={e => { if (e.target === e.currentTarget) onClose(); }}><div className="wf-dialog-head"><h2 id={id}>{title}</h2><button className="icon-btn" aria-label="关闭弹窗" onClick={onClose}><X size={18} /></button></div>{children}</dialog>;
}
export function BusyLabel({ busy, children }: { busy: boolean; children: ReactNode }) {
  return <>{busy && <LoaderCircle className="wf-spin" size={15} />}{children}</>;
}
export function useTask() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const ref = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (ref.current !== null) clearTimeout(ref.current); ref.current = null; }, []);
  return { busy, error, run: (action: () => void) => {
    if (ref.current !== null) return;
    setBusy(true); setError("");
    ref.current = setTimeout(() => {
      try { action(); } catch (cause) { setError(cause instanceof Error ? cause.message : "预览操作失败，请重试。"); }
      finally { setBusy(false); ref.current = null; }
    }, 450);
  } };
}
