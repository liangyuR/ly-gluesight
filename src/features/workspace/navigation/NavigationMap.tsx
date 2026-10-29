import { Accordion, Button, Checkbox, Select, NumberField, FileInput } from "qing-steel";
import { useEffect, useId, useRef, useState, type PointerEvent, type WheelEvent } from "react";
import { frameFraction, hitFrame, mapIssues, pathSlice, pointOnPath, projectPath, spatialRanges } from "./geometry";
import { defaultPresentation, type FrameRange, type MapPoint, type NavigationMapData, type NavigationPresentation, type NavigationSegment } from "./types";
import "./navigation.css";

export interface NavigationMapProps {
  map: NavigationMapData | null;
  imageUrl: string | null;
  segments: NavigationSegment[];
  ranges: FrameRange[] | null;
  seq: number;
  currentSegment?: number;
  frameStatus: "confirmed" | "draft" | "unavailable";
  editable?: boolean;
  disabled?: boolean;
  disabledReason?: string;
  error?: string | null;
  initialDraft?: NavigationMapData | null;
  onDraftChange?: (map: NavigationMapData | null) => void;
  onSave?: (map: NavigationMapData, confirm: boolean) => Promise<boolean>;
  onImport?: (file: File) => Promise<void>;
  onRemove?: () => Promise<void>;
  onNavigate: (segment: number, seq: number) => void;
  onDirty?: (dirty: boolean) => void;
}

type EditStep = "route" | "range" | "segments";
type Mode = "navigate" | "draw" | "vertices" | "start" | "end" | "boundary" | "pan";
type Drag = { kind: "vertex"; index: number; before: NavigationMapData } | { kind: "pan"; x: number; y: number; origin: MapPoint };
const stringify = (map: NavigationMapData | null) => JSON.stringify(map);
const svgPoints = (points: MapPoint[]) => points.map(point => point.join(",")).join(" ");

export default function NavigationMap(props: NavigationMapProps) {
  const { map, imageUrl, segments, ranges, seq, frameStatus, editable = false, disabled = false, onDraftChange, onDirty } = props;
  const [draft, setDraft] = useState<NavigationMapData | null>(() => props.initialDraft ?? null);
  const [editing, setEditing] = useState(editable && !!props.initialDraft);
  const [source, setSource] = useState(() => stringify(map));
  const [undo, setUndo] = useState<NavigationMapData[]>([]);
  const [mode, setMode] = useState<Mode>(props.initialDraft ? "draw" : "navigate");
  const [step, setStep] = useState<EditStep>("route");
  const [vertex, setVertex] = useState<number | null>(null);
  const [boundary, setBoundary] = useState<number | null>(null);
  const [selectedSegment, setSelectedSegment] = useState<number | undefined>(props.currentSegment);
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState<MapPoint>([0, 0]);
  const [canvasSize, setCanvasSize] = useState({ width: 600, height: 360 });
  const [message, setMessage] = useState("");
  const [working, setWorking] = useState(false);
  const [removing, setRemoving] = useState(false);
  const drag = useRef<Drag | null>(null);
  const svg = useRef<SVGSVGElement>(null);
  const marker = useId().replace(/:/g, "");
  const shown = draft ?? map;
  const presentation = shown?.view ?? defaultPresentation;
  const { rotation, flipX, flipY, background } = presentation;
  const dirty = !!draft && stringify(draft) !== stringify(map);
  const sourceChanged = !!draft && source !== stringify(map);
  const locked = disabled || working;
  const issues = shown ? mapIssues(shown, segments.length) : [];
  const location = shown && ranges ? frameFraction(shown, ranges, seq) : null;
  const navigationReady = !!imageUrl && !!shown?.confirmed && !issues.length && !!ranges && ranges.length === segments.length && frameStatus !== "unavailable";
  const preferred = selectedSegment == null ? location?.segment : segments.findIndex(item => item.index === selectedSegment);
  const position = navigationReady && location && shown ? pointOnPath(shown, location.fraction) : null;
  const width = shown?.image.width ?? 1000, height = shown?.image.height ?? 700;
  const radians = rotation * Math.PI / 180, cos = Math.cos(radians), sin = Math.sin(radians);
  const viewWidth = (Math.abs(width * cos) + Math.abs(height * sin)) / zoom;
  const viewHeight = (Math.abs(width * sin) + Math.abs(height * cos)) / zoom;
  const unrotate = ([x, y]: MapPoint): MapPoint => [(cos * x + sin * y) * (flipX ? -1 : 1), (-sin * x + cos * y) * (flipY ? -1 : 1)];
  const orientation = `translate(${width / 2} ${height / 2}) rotate(${rotation}) scale(${flipX ? -1 : 1} ${flipY ? -1 : 1}) translate(${-width / 2} ${-height / 2})`;
  const upright = `scale(${flipX ? -1 : 1} ${flipY ? -1 : 1}) rotate(${-rotation})`;
  const fit = () => { setZoom(1); setPan([0, 0]); };
  const changeView = (changes: Partial<NavigationPresentation>) => {
    if (!shown || locked || sourceChanged) return;
    const view = { ...presentation, ...changes };
    if (JSON.stringify(view) === JSON.stringify(presentation)) return;
    if (!draft) setSource(stringify(map));
    const next = { ...shown, view };
    setDraft(next); onDraftChange?.(next); setMessage("");
  };
  const resetView = () => { fit(); changeView({ rotation: 0, flipX: false, flipY: false }); };
  const rotate = (value: number) => { if (Number.isFinite(value)) { changeView({ rotation: ((value + 180) % 360 + 360) % 360 - 180 }); fit(); } };
  const viewX = (width - viewWidth) / 2 + pan[0], viewY = (height - viewHeight) / 2 + pan[1];

  useEffect(() => { setZoom(1); setPan([0, 0]); }, [shown?.image.id]);
  useEffect(() => { onDirty?.(dirty); }, [dirty, onDirty]);
  useEffect(() => { setSelectedSegment(props.currentSegment); }, [props.currentSegment, seq]);
  useEffect(() => {
    const canvas = svg.current;
    if (!canvas) return;
    const measure = () => {
      const rect = canvas.getBoundingClientRect();
      if (rect.width && rect.height) setCanvasSize(previous => previous.width === rect.width && previous.height === rect.height ? previous : { width: rect.width, height: rect.height });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [shown?.image.id]);
  useEffect(() => {
    if (draft && !dirty && source !== stringify(map)) {
      setDraft(map ? structuredClone(map) : null);
      setSource(stringify(map));
    }
  }, [map, draft, dirty, source]);

  const update = (next: NavigationMapData, remember = true) => {
    if (locked || !editing) return;
    if (remember && shown) setUndo(previous => [...previous.slice(-49), structuredClone(shown)]);
    const value = { ...next, confirmed: false };
    setDraft(value);
    onDraftChange?.(value);
    setMessage("");
  };
  const begin = () => {
    if (!shown || locked || sourceChanged) return;
    const value = structuredClone(shown);
    if (!draft) setSource(stringify(map));
    setDraft(value); setEditing(true); setUndo([]); setMode("draw"); setStep("route");
    onDraftChange?.(value);
  };
  const end = () => {
    setDraft(null); setEditing(false); setUndo([]); setMode("navigate"); setMessage("");
    onDraftChange?.(null);
  };
  const run = async (action: () => Promise<void>) => {
    setWorking(true); setMessage("");
    try { await action(); } catch (error) { setMessage(error instanceof Error ? error.message : String(error)); }
    finally { setWorking(false); }
  };
  const save = (confirm: boolean) => {
    if (!shown || !props.onSave) return;
    void run(async () => { if (await props.onSave!(shown, confirm)) end(); });
  };
  const pixels = (clientX: number, clientY: number): { point: MapPoint; scale: number } | null => {
    const rect = svg.current?.getBoundingClientRect();
    if (!rect?.width || !rect.height) return null;
    const scale = Math.min(rect.width / viewWidth, rect.height / viewHeight);
    const left = rect.left + (rect.width - viewWidth * scale) / 2;
    const top = rect.top + (rect.height - viewHeight * scale) / 2;
    const point = unrotate([(clientX - left) / scale + viewX - width / 2, (clientY - top) / scale + viewY - height / 2]);
    return { point: [point[0] + width / 2, point[1] + height / 2], scale };
  };
  const normalized = (point: MapPoint): MapPoint => [Math.max(0, Math.min(1, point[0] / width)), Math.max(0, Math.min(1, point[1] / height))];
  const onPointerDown = (event: PointerEvent<SVGSVGElement>) => {
    if (!shown || locked || event.button !== 0 && event.button !== 1) return;
    const at = pixels(event.clientX, event.clientY);
    if (!at) return;
    if (mode === "pan" || event.button === 1) {
      drag.current = { kind: "pan", x: event.clientX, y: event.clientY, origin: pan };
      event.currentTarget.setPointerCapture(event.pointerId);
      event.preventDefault(); return;
    }
    if (!editing || mode === "navigate") {
      if (!navigationReady || !ranges) return;
      const hit = hitFrame(shown, ranges, at.point, 12 / at.scale, preferred);
      if (hit.kind === "ambiguous") { setMessage("此处对应多个胶路位置，请先选择段或点击交叉点旁的胶路。"); return; }
      if (hit.kind === "hit") {
        setSelectedSegment(segments[hit.segment].index);
        props.onNavigate(segments[hit.segment].index, hit.seq);
        setMessage("");
      }
      return;
    }
    if (mode === "draw" || mode === "vertices") {
      const index = shown.points.findIndex(([x, y]) => Math.hypot(x * width - at.point[0], y * height - at.point[1]) * at.scale < 12);
      setVertex(index < 0 ? null : index);
      if (index >= 0) {
        drag.current = { kind: "vertex", index, before: structuredClone(shown) };
        event.currentTarget.setPointerCapture(event.pointerId);
      } else if (mode === "draw" && at.point[0] >= 0 && at.point[0] <= width && at.point[1] >= 0 && at.point[1] <= height) {
        update({ ...shown, points: [...shown.points, normalized(at.point)] });
      }
      return;
    }
    const projections = projectPath(shown, at.point);
    const best = projections[0];
    if (!best || best.distance * at.scale > 12) { setMessage("请点击胶路附近，标记会吸附到折线上。"); return; }
    if (projections.some(other => Math.abs(other.distance - best.distance) < 1e-8 && Math.abs(other.fraction - best.fraction) > 1e-8)) {
      setMessage("此处胶路重叠，请点击交叉点旁的胶路，或用下方进度输入定位。"); return;
    }
    let fraction = best.fraction;
    if (mode === "start") { update({ ...shown, captureStart: fraction }); setMode("end"); }
    if (mode === "end") {
      if (shown.closed && fraction <= shown.captureStart) fraction += 1;
      update({ ...shown, captureEnd: fraction });
    }
    if (mode === "boundary") {
      if (shown.closed && fraction < shown.captureStart) fraction += 1;
      if (fraction <= shown.captureStart || fraction >= shown.captureEnd) { setMessage("分界点必须位于采集起止点之间。"); return; }
      if (boundary != null && boundary < shown.cuts.length) update({ ...shown, cuts: shown.cuts.map((value, i) => i === boundary ? fraction : value) });
      else if (shown.cuts.length < segments.length - 1) update({ ...shown, cuts: [...shown.cuts, fraction].sort((a, b) => a - b) });
      else {
        const nearest = shown.cuts.findIndex(cut => {
          const point = pointOnPath(shown, cut);
          return point && Math.hypot(point[0] - at.point[0], point[1] - at.point[1]) * at.scale <= 12;
        });
        if (nearest >= 0) setBoundary(nearest);
        else setMessage("分界已标完。要调整时，先点一个分界标记，再点新位置。");
      }
    }
  };
  const onPointerMove = (event: PointerEvent<SVGSVGElement>) => {
    const moving = drag.current, at = pixels(event.clientX, event.clientY);
    if (!moving || !at || locked || !shown) return;
    if (moving.kind === "pan") setPan([moving.origin[0] - (event.clientX - moving.x) / at.scale, moving.origin[1] - (event.clientY - moving.y) / at.scale]);
    else update({ ...shown, points: shown.points.map((point, index) => index === moving.index ? normalized(at.point) : point) }, false);
  };
  const onPointerUp = () => {
    if (drag.current?.kind === "vertex" && stringify(drag.current.before) !== stringify(shown)) {
      const before = drag.current.before;
      setUndo(previous => [...previous.slice(-49), before]);
    }
    drag.current = null;
  };
  const wheel = (event: WheelEvent<SVGSVGElement>) => {
    if (!event.ctrlKey) return;
    event.preventDefault();
    setZoom(value => Math.max(1, Math.min(8, value * (event.deltaY > 0 ? 0.85 : 1.15))));
  };
  const removeVertex = () => {
    if (shown && vertex != null) { update({ ...shown, points: shown.points.filter((_, index) => index !== vertex) }); setVertex(null); }
  };
  const reverse = () => {
    if (shown) update({ ...shown, points: shown.closed ? [shown.points[0], ...shown.points.slice(1).reverse()] : [...shown.points].reverse(), captureStart: 0, captureEnd: 1, cuts: [] });
  };
  const canConfirm = !!shown && !issues.length && !locked && !sourceChanged;
  const stroke = 3 / Math.min(canvasSize.width / viewWidth, canvasSize.height / viewHeight);
  const steps: [EditStep, string][] = [["route", "1 画胶路"], ["range", "2 采集范围"], ...(segments.length > 1 ? [["segments", "3 分段"] as [EditStep, string]] : [])];
  const selectStep = (next: EditStep) => {
    setStep(next); setMode(next === "route" ? "draw" : next === "range" ? "start" : "boundary"); setBoundary(null); setVertex(null); setMessage("");
  };
  const nextStep = step === "route" ? "range" : step === "range" && segments.length > 1 ? "segments" : null;

  return <section className="navigation-map" aria-label="工件胶路导航图">
    <div className="navigation-map-heading"><div><strong>工件胶路导航图</strong><small>示意位置 · 按段估计帧组进度</small></div>
      {editable && <div className="navigation-map-actions">
        <FileInput accept="image/png,image/jpeg,image/bmp,image/tiff" aria-label="导入工件图" buttonLabel={map ? "更换工件图" : "导入工件图"} placeholder="" files={[]} disabled={locked || dirty || !props.onImport} onFilesChange={files => {
          const file = files[0];
          if (file && props.onImport) void run(async () => { await props.onImport!(file); end(); fit(); });
        }} />
        {map && !editing && <Button size="sm" variant="outlined" type="button" disabled={locked || !props.onSave} onClick={begin}>编辑地图</Button>}
        {map && props.onRemove && !editing && <Button size="sm" variant="outlined" type="button" disabled={locked} onClick={() => setRemoving(true)}>移除地图</Button>}
      </div>}
    </div>
    {props.disabledReason && <p className="navigation-map-note">{props.disabledReason}</p>}
    {props.error && <p role="alert">{props.error}</p>}
    {sourceChanged && <p role="alert">地图来源已更新，保留了本地未保存内容。请撤销本地修改后重新编辑，避免覆盖新版本。</p>}
    {!shown && <p className="navigation-map-note">可选：导入完整工件图，沿胶路标段后辅助选帧。</p>}
    {shown && <>
      {editing && <div className="navigation-map-workflow">
        <div className="navigation-map-steps" role="group" aria-label="地图编辑步骤">{steps.map(([value, label]) => <Button size="sm" variant={step === value ? "tonal" : "outlined"} type="button" key={value} aria-pressed={step === value} disabled={locked} onClick={() => selectStep(value)}>{label}</Button>)}</div>
        <div className="navigation-map-step-content">
          {step === "route" && <><p>沿涂胶方向点击画线，拖动已有点即可调整。</p><Checkbox label="闭合胶路" checked={shown.closed} disabled={locked} onChange={event => update({ ...shown, closed: event.target.checked, captureStart: 0, captureEnd: 1, cuts: [] })} />{vertex != null && <Button size="sm" variant="outlined" type="button" disabled={locked} onClick={removeVertex}>删除选中顶点</Button>}</>}
          {step === "range" && <><p>沿胶路先点采集起点，再点终点；已有范围正确时可直接保存。</p><Button size="sm" variant={mode === "start" ? "tonal" : "outlined"} type="button" aria-pressed={mode === "start"} disabled={locked} onClick={() => setMode("start")}>标采集起点</Button><Button size="sm" variant={mode === "end" ? "tonal" : "outlined"} type="button" aria-pressed={mode === "end"} disabled={locked} onClick={() => setMode("end")}>标采集终点</Button>{segments.length === 1 && <span>当前仅 1 段，无需标分界。</span>}</>}
          {step === "segments" && <><p>沿胶路依次点出 {segments.length - 1} 个分界，已标 {shown.cuts.length} 个。</p>{shown.cuts.map((_, index) => <Button size="sm" variant={boundary === index ? "tonal" : "outlined"} key={index} type="button" aria-pressed={boundary === index} disabled={locked} onClick={() => { setMode("boundary"); setBoundary(index); }}>调整分界 {index + 1}</Button>)}</>}
          <Button size="sm" variant="outlined" type="button" disabled={locked || !undo.length} onClick={() => { const previous = undo.at(-1)!; setDraft(previous); onDraftChange?.(previous); setUndo(undo.slice(0, -1)); setVertex(null); }}>撤销上一步</Button>
        </div>
      </div>}
      <div className="navigation-map-tools">

        <Button size="sm" variant={mode === "pan" ? "tonal" : "outlined"} type="button" aria-pressed={mode === "pan"} onClick={() => setMode(mode === "pan" ? !editing ? "navigate" : step === "route" ? "draw" : step === "range" ? "start" : "boundary" : "pan")}>平移</Button>
        <Button size="sm" variant="outlined" type="button" aria-label="缩小地图" disabled={zoom <= 1} onClick={() => setZoom(value => Math.max(1, value / 1.5))}>−</Button>
        <span aria-label="地图缩放">{Math.round(zoom * 100)}%</span>
        <Button size="sm" variant="outlined" type="button" aria-label="放大地图" disabled={zoom >= 8} onClick={() => setZoom(value => Math.min(8, value * 1.5))}>＋</Button>
        <Button size="sm" variant="outlined" type="button" onClick={fit}>适合窗口</Button>
        <Accordion collapsible forceMount className="navigation-map-options" items={[{ value: "view", title: "视图设置", content: <><div className="navigation-map-actions">
        <Button size="sm" variant="outlined" type="button" aria-label="向左旋转90度" disabled={locked || sourceChanged} onClick={() => rotate(rotation - 90)}>↶ 90°</Button>
        <Button size="sm" variant="outlined" type="button" aria-label="向右旋转90度" disabled={locked || sourceChanged} onClick={() => rotate(rotation + 90)}>↷ 90°</Button>
        <label className="navigation-map-angle">角度 <NumberField clamp={false} aria-label="工件视图旋转角度" step={1} disabled={locked || sourceChanged} value={rotation} onValueChange={nextValue => rotate(Number(nextValue))} />°</label>
        <Button size="sm" variant={flipX ? "tonal" : "outlined"} type="button" aria-pressed={flipX} disabled={locked || sourceChanged} onClick={() => { changeView({ flipX: !flipX }); fit(); }}>水平翻转</Button>
        <Button size="sm" variant={flipY ? "tonal" : "outlined"} type="button" aria-pressed={flipY} disabled={locked || sourceChanged} onClick={() => { changeView({ flipY: !flipY }); fit(); }}>垂直翻转</Button>
        <Button size="sm" variant="outlined" type="button" disabled={locked || sourceChanged} onClick={resetView}>重置视图</Button>
        <label className="navigation-map-background">底色 <Select options={[{ value: "slate", label: "中灰" }, { value: "light", label: "浅色" }, { value: "checker", label: "淡棋盘格" }]} aria-label="透明图片底色" value={background} disabled={locked || sourceChanged} onChange={event => changeView({ background: event.target.value as NavigationPresentation["background"] })} /></label>
        </div></> }]} />
      </div>
      <svg ref={svg} className={`navigation-map-canvas mode-${mode} background-${background}`} role="img" aria-label="工件图与有向胶路" tabIndex={0}
        viewBox={`${viewX} ${viewY} ${viewWidth} ${viewHeight}`} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp} onWheel={wheel}
        onKeyDown={event => {
          if (!editing || (mode !== "vertices" && mode !== "draw") || vertex == null || locked) return;
          if (event.key === "Delete" || event.key === "Backspace") { event.preventDefault(); removeVertex(); }
          if (event.key.startsWith("Arrow")) {
            event.preventDefault();
            const point = shown.points[vertex];
            if (!point) return;
            const step = event.shiftKey ? 10 : 1;
            const delta = unrotate([event.key === "ArrowLeft" ? -step : event.key === "ArrowRight" ? step : 0, event.key === "ArrowUp" ? -step : event.key === "ArrowDown" ? step : 0]);
            const next: MapPoint = [point[0] * width + delta[0], point[1] * height + delta[1]];
            update({ ...shown, points: shown.points.map((item, i) => i === vertex ? normalized(next) : item) });
          }
        }}>
        <defs><marker id={marker} markerWidth="5" markerHeight="5" refX="3" refY="2.5" orient="auto"><path d="M0 0 L5 2.5 L0 5 Z" fill="currentColor" /></marker></defs>
        <g className="navigation-map-content" transform={orientation}>
        {imageUrl && <image href={imageUrl} x={0} y={0} width={width} height={height} />}
        <polyline points={svgPoints([...shown.points.map(([x, y]): MapPoint => [x * width, y * height]), ...(shown.closed && shown.points.length > 2 ? [[shown.points[0][0] * width, shown.points[0][1] * height] as MapPoint] : [])])} className="navigation-route" strokeWidth={stroke} />
        {!issues.length && spatialRanges(shown).map(([start, end], index) => <polyline key={index} className={`navigation-segment${index === preferred ? " selected" : ""}`} strokeWidth={stroke * (index === preferred ? 1.8 : 1)} points={svgPoints(pathSlice(shown, start, end))} />)}
        {shown.points.slice(0, -1).map(([x, y], index) => {
          if (index % Math.max(1, Math.ceil((shown.points.length - 1) / 8)) !== 0) return null;
          const next = shown.points[index + 1];
          return <line key={`direction-${index}`} x1={(x * 0.55 + next[0] * 0.45) * width} y1={(y * 0.55 + next[1] * 0.45) * height} x2={(x * 0.4 + next[0] * 0.6) * width} y2={(y * 0.4 + next[1] * 0.6) * height} strokeWidth={stroke * 0.6} className="navigation-direction" markerEnd={`url(#${marker})`} />;
        })}
        {shown.points[0] && <g transform={`translate(${shown.points[0][0] * width} ${shown.points[0][1] * height})`}><circle className="navigation-origin" r={stroke * 1.3} /><text transform={upright} x={stroke * 2} y={-stroke * 3} fontSize={stroke * 4}>胶路起点</text></g>}
        {[{ value: shown.captureStart, label: "采集起", key: "start" }, ...shown.cuts.map((value, index) => ({ value, label: `${index + 1}|${index + 2}`, key: `cut${index}` })), { value: shown.captureEnd, label: "采集止", key: "end" }].map(item => {
          const point = pointOnPath(shown, item.value);
          return point && <g key={item.key} transform={`translate(${point[0]} ${point[1]})`}><rect className="navigation-boundary" x={-stroke} y={-stroke} width={stroke * 2} height={stroke * 2} /><text transform={upright} x={stroke * 2} y={stroke * 4} fontSize={stroke * 4}>{item.label}</text></g>;
        })}
        {position && <g transform={`translate(${position[0]} ${position[1]})`}><circle className="navigation-position" r={stroke * 2.2} /><text transform={upright} x={stroke * 3} y={-stroke * 8} fontSize={stroke * 4}>帧组 {seq}</text></g>}
        {editing && step === "route" && shown.points.map(([x, y], index) => <circle key={`vertex-${index}`} cx={x * width} cy={y * height} r={stroke * (vertex === index ? 1.8 : 1.2)} className={`navigation-vertex${vertex === index ? " selected" : ""}`} />)}
        </g>
      </svg>
      {!editing && dirty && <div className="navigation-map-actions navigation-view-save"><span>视图尚未保存</span><Button size="sm" variant="outlined" type="button" disabled={locked} onClick={end}>撤销视图修改</Button><Button size="sm" variant="outlined" type="button" disabled={locked || sourceChanged || !props.onSave} onClick={() => save(shown.confirmed)}>保存视图</Button></div>}
      {!imageUrl && <p className="navigation-map-note">工件图尚未加载或不可用。</p>}
      <div className="navigation-map-segments" role="group" aria-label="地图段选择">{segments.map((segment, index) => <Button size="sm" variant={index === preferred ? "tonal" : "outlined"} key={segment.index} type="button" aria-pressed={index === preferred} onClick={() => setSelectedSegment(segment.index)}>段 {segment.index} · {segment.kind === "corner" ? "弯道" : "直线"}</Button>)}</div>
      <p className="navigation-map-note" role="status">{navigationReady ? `帧组 ${seq} · ${frameStatus === "draft" ? "估计位置／边界未确认" : "帧组估计位置"}` : !imageUrl ? "工件图不可用，暂不定位或跳帧。" : !shown.confirmed || issues.length ? "空间对应待确认，暂不定位或跳帧。" : "缺少可用的示教件帧范围，暂不定位或跳帧。"} 未采集部分为灰色。</p>
      {editing && <div className="navigation-map-editor">
        <Accordion collapsible forceMount className="navigation-map-advanced" items={[{ value: "advanced", title: "精确调整与更多操作", content: <>
        <div className="navigation-map-actions">
          <Button size="sm" variant="outlined" type="button" disabled={locked || shown.points.length < 2} onClick={reverse}>反转涂胶方向</Button>
          <Button size="sm" variant="outlined" type="button" disabled={locked || !shown.closed || vertex == null} onClick={() => { if (vertex != null) { update({ ...shown, points: [...shown.points.slice(vertex), ...shown.points.slice(0, vertex)], captureStart: 0, captureEnd: 1, cuts: [] }); setVertex(0); } }}>选中顶点设为起点</Button>
        </div>
        <div className="navigation-map-fields">
          <label>选中顶点<Select options={[{ value: "", label: "未选择" }, ...shown.points.map((_, index) => ({ value: String(index), label: `顶点 ${index + 1}` }))]} aria-label="选中顶点" value={vertex ?? ""} disabled={locked} onChange={event => { setVertex(event.target.value === "" ? null : Number(event.target.value)); setMode("draw"); setStep("route"); }} /></label>
          {vertex != null && shown.points[vertex] && <>{([0, 1] as const).map(axis => <label key={axis}>顶点 {vertex + 1} {axis === 0 ? "X" : "Y"}（图片 %）<NumberField clamp={false} min={0} max={100} step={0.1} value={Number((shown.points[vertex][axis] * 100).toFixed(4))} disabled={locked} onValueChange={nextValue => update({ ...shown, points: shown.points.map((point, index) => index === vertex ? point.map((value, dim) => dim === axis ? Math.max(0, Math.min(1, Number(nextValue) / 100)) : value) as MapPoint : point) })} /></label>)}</>}
          <Button size="sm" variant="outlined" type="button" disabled={locked} onClick={() => { update({ ...shown, points: [...shown.points, [0.5, 0.5]] }); setVertex(shown.points.length); setMode("draw"); setStep("route"); }}>添加顶点并输入坐标</Button>
          <label>采集起点（路径 %）<NumberField clamp={false} min={0} max={100} step={0.1} value={Number((shown.captureStart * 100).toFixed(4))} disabled={locked} onValueChange={nextValue => update({ ...shown, captureStart: Number(nextValue) / 100 })} /></label>
          <label>采集终点（路径 %）<NumberField clamp={false} min={0} max={shown.closed ? 200 : 100} step={0.1} value={Number((shown.captureEnd * 100).toFixed(4))} disabled={locked} onValueChange={nextValue => update({ ...shown, captureEnd: Number(nextValue) / 100 })} /></label>
          {shown.cuts.length < segments.length - 1 && <Button size="sm" variant="outlined" type="button" disabled={locked} onClick={() => {
            const largest = spatialRanges(shown).sort((a, b) => (b[1] - b[0]) - (a[1] - a[0]))[0];
            if (largest && largest[1] > largest[0]) update({ ...shown, cuts: [...shown.cuts, (largest[0] + largest[1]) / 2].sort((a, b) => a - b) });
          }}>添加分界并输入进度</Button>}
          {shown.cuts.map((cut, index) => <div className="navigation-cut-field" key={index}><label>段 {index + 1} 后分界（路径 %）<NumberField clamp={false} step={0.1} value={Number((cut * 100).toFixed(4))} disabled={locked} onValueChange={nextValue => update({ ...shown, cuts: shown.cuts.map((value, at) => at === index ? Number(nextValue) / 100 : value) })} /></label><Button size="sm" variant={mode === "boundary" && boundary === index ? "tonal" : "outlined"} type="button" disabled={locked} aria-pressed={mode === "boundary" && boundary === index} onClick={() => { setMode("boundary"); setStep("segments"); setBoundary(index); }}>在图上移动分界 {index + 1}</Button><Button size="sm" variant="outlined" type="button" disabled={locked} onClick={() => { update({ ...shown, cuts: shown.cuts.filter((_, at) => at !== index) }); setBoundary(null); }}>删除分界 {index + 1}</Button></div>)}
        </div>
        {shown.closed && <p className="navigation-map-note">闭合路径从起点展开；采集终点超过 100% 表示跨过起点，采集跨度最多一圈。</p>}
        <Button size="sm" variant="outlined" type="button" disabled={locked || sourceChanged || !props.onSave} onClick={() => save(false)}>保存地图草稿</Button>
        </> }]} />
        {!!issues.length && <ul className="navigation-map-issues">{issues.map(issue => <li key={issue}>{issue}</li>)}</ul>}
        <div className="navigation-map-actions">
          <Button size="sm" variant="outlined" type="button" disabled={locked} aria-label="撤销地图修改" onClick={end}>取消修改</Button>
          {nextStep && <Button size="sm" variant="outlined" type="button" disabled={locked || shown.points.length < (shown.closed ? 3 : 2)} onClick={() => selectStep(nextStep)}>下一步</Button>}
          <Button size="sm" variant="filled" type="button" aria-label="确认空间对应并保存" disabled={!canConfirm || !props.onSave} onClick={() => save(true)}>保存地图</Button>
        </div>
      </div>}
    </>}
    {message && <p role="alert">{message}</p>}
    {removing && <div className="navigation-map-remove" role="group" aria-label="确认移除地图"><p>移除候选地图后仍可示教和发布，已冻结的发布资料保留。</p><Button size="sm" variant="outlined" type="button" disabled={working} onClick={() => setRemoving(false)}>取消移除</Button><Button size="sm" variant="outlined" type="button" disabled={locked} onClick={() => void run(async () => { await props.onRemove?.(); setRemoving(false); end(); })}>确认移除地图</Button></div>}
  </section>;
}
