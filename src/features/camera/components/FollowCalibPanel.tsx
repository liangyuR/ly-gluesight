import { invoke, isTauri } from "@tauri-apps/api/core";
import { useEffect, useRef, useState, type MouseEvent } from "react";
import { Crosshair, MoveUpRight, Ruler, ScanLine } from "lucide-react";
import { cameraApi } from "../api";
import type { StationView } from "../../workspace/StationCapture";
import type { CameraConfig, FollowCalib, Frame } from "../types";

type Tool = "nozzle" | "direction" | "scale";
type Polarity = "dark" | "light" | "any";

interface ProbePoint {
  l: number;
  offset: number | null;
  width: number | null;
  st: number;
  px: [number, number];
}

const rad = (d: number) => (d * Math.PI) / 180;
const deg = (r: number) => (r * 180) / Math.PI;

/** 与后端 FollowCalib::dir_to_img 一致：先镜像，再按 angleDeg 顺时针转。 */
function dirToImg(c: FollowCalib, [x, y]: [number, number]): [number, number] {
  const xm = c.mirror ? -x : x;
  const [s, co] = [Math.sin(rad(c.angleDeg)), Math.cos(rad(c.angleDeg))];
  return [xm * co - y * s, xm * s + y * co];
}

/** 三目演示的默认标定：胶嘴在图像下方正中，三台相机方位差 120°。 */
function defaultCalib(cam: number, size: [number, number]): FollowCalib {
  return { nozzle: [size[0] / 2, size[1] * 0.88], angleDeg: 120 * cam, mirror: false, mmPerPx: 0.05, maskPx: 60, imageSize: size };
}

interface Props {
  cam: number;
  config: CameraConfig;
  frame: Frame | undefined;
  sample?:StationView|null;
  onSaved: (c: CameraConfig) => void;
}

export default function FollowCalibPanel({ cam, config, sample, onSaved }: Props) {
  const img=sample?{fullWidth:sample.image.width,fullHeight:sample.image.height}:null;
  const svg = useRef<SVGSVGElement>(null);
  const size: [number, number] = img ? [img.fullWidth, img.fullHeight] : (config.follow?.imageSize ?? [1280, 1024]);
  const [calib, setCalib] = useState<FollowCalib>(config.follow ?? defaultCalib(cam, size));
  const [tool, setTool] = useState<Tool>("nozzle");
  const [travelDeg, setTravelDeg] = useState(0);
  const [scalePts, setScalePts] = useState<[number, number][]>([]);
  const [scaleMm, setScaleMm] = useState(10);
  const [probe, setProbe] = useState({ polarity: "dark" as Polarity, beadWidth: 2, searchMm: 4, nearMm: 3, farMm: 18 });
  const [points, setPoints] = useState<ProbePoint[]>([]);
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);
  const probeSerial=useRef(0);
  const probeResultKey=useRef<string|null>(null);
  const savingRef=useRef(false),probingRef=useRef(false),saveSerial=useRef(0);
  const [saving,setSaving]=useState(false);
  const [probing,setProbing]=useState(false);
  const configKey=JSON.stringify(config);
  const scope=JSON.stringify([cam,configKey,sample?.metadata.id,size]);
  const current=useRef({scope,alive:true});current.current.scope=scope;
  const probeKey=JSON.stringify([scope,calib,travelDeg,probe]);
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;probeSerial.current++;saveSerial.current++;};},[]);
  useEffect(()=>{
    setCalib(config.follow??defaultCalib(cam,size));setTravelDeg(0);setScalePts([]);setTool("nozzle");
  },[cam,configKey]);
  useEffect(()=>{
    probeSerial.current++;saveSerial.current++;probeResultKey.current=null;
    probingRef.current=false;savingRef.current=false;setProbing(false);setSaving(false);
    setScalePts([]);setPoints([]);setNotice(null);
  },[scope]);
  useEffect(()=>{
    if(probeResultKey.current===probeKey)return;
    probeResultKey.current=null;probeSerial.current++;probingRef.current=false;setProbing(false);setPoints([]);setNotice(null);
  },[probeKey]);

  useEffect(()=>{
    if(scalePts.length!==2||!Number.isFinite(scaleMm)||scaleMm<=0)return;
    const px=Math.hypot(scalePts[1][0]-scalePts[0][0],scalePts[1][1]-scalePts[0][1]);
    if(px>3){const mmPerPx=Number((scaleMm/px).toFixed(5));setCalib(c=>({...c,mmPerPx}));}
  },[scalePts,scaleMm]);

  useEffect(() => {
    if(!img)return;
    setCalib(c=>{
      if(c.imageSize[0]===img.fullWidth&&c.imageSize[1]===img.fullHeight)return c;
      const imageSize:[number,number]=[img.fullWidth,img.fullHeight];
      return !config.follow&&JSON.stringify(c)===JSON.stringify(defaultCalib(cam,c.imageSize))?defaultCalib(cam,imageSize):{...c,imageSize};
    });
  }, [img?.fullWidth,img?.fullHeight,cam,configKey]);

  // 标定时机器人沿 travelDeg 方向走，胶条在胶嘴身后，即工件坐标里的 −travel 方向
  const behind: [number, number] = [-Math.cos(rad(travelDeg)), -Math.sin(rad(travelDeg))];
  const beadImg = dirToImg(calib, behind);
  const beadDeg = deg(Math.atan2(beadImg[1], beadImg[0]));

  const toImage = (e: MouseEvent<SVGSVGElement>): [number, number] | null => {
    const el = svg.current;
    const m = el?.getScreenCTM();
    if (!el || !m) return null;
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(m.inverse());
    return [p.x, p.y];
  };

  const click = (e: MouseEvent<SVGSVGElement>) => {
    const p = toImage(e);
    if (!p || !sample || savingRef.current || probingRef.current || !p.every(Number.isFinite)||p[0]<0||p[1]<0||p[0]>=size[0]||p[1]>=size[1]) return;
    if (tool === "nozzle") setCalib({ ...calib, nozzle: [Math.min(size[0]-1,Math.round(p[0])), Math.min(size[1]-1,Math.round(p[1]))] });
    else if (tool === "direction") {
      if(Math.hypot(p[0]-calib.nozzle[0],p[1]-calib.nozzle[1])<=3)return;
      setCalib({ ...calib, angleDeg: angleFor(deg(Math.atan2(p[1] - calib.nozzle[1], p[0] - calib.nozzle[0]))) });
    } else {
      const next = scalePts.length >= 2 ? [p] : [...scalePts, p];
      setScalePts(next);
    }
  };

  /** 胶条在图像里的方向 theta（度）→ 图像方位 angleDeg，按标定时的走向换算。 */
  const angleFor = (thetaDeg: number) => {
    const m: [number, number] = [calib.mirror ? -behind[0] : behind[0], behind[1]];
    return Number(deg(rad(thetaDeg) - Math.atan2(m[1], m[0])).toFixed(2));
  };

  const runProbe = async (auto = false) => {
    if (!isTauri()||!sample||probingRef.current||savingRef.current||calibError||probeError) return;
    setNotice(null);
    const serial=++probeSerial.current;probingRef.current=true;setProbing(true);
    const valid=()=>current.current.alive&&current.current.scope===scope&&serial===probeSerial.current;
    try {
      const r = await invoke<{ points: ProbePoint[]; directionDeg: number }>("teach_follow_probe", {
        request: { cam, imageId:sample?.metadata.id, calib, directionDeg: auto ? null : beadDeg, ...probe },
      });
      if(!valid())return;
      const nextCalib=auto?{...calib,angleDeg:angleFor(r.directionDeg)}:calib;
      probeResultKey.current=JSON.stringify([scope,nextCalib,travelDeg,probe]);
      setPoints(r.points);
      if (auto) setCalib(nextCalib);
      const ok = r.points.filter((p) => p.st === 0);
      const w = ok.map((p) => p.width ?? 0);
      const o = ok.map((p) => p.offset ?? 0);
      const mean = (a: number[]) => a.reduce((x, y) => x + y, 0) / Math.max(a.length, 1);
      const sd = (a: number[]) => Math.sqrt(mean(a.map((v) => (v - mean(a)) ** 2)));
      setNotice({
        ok: r.points.length > 0 && ok.length >= r.points.length * 0.8,
        text: `测到 ${ok.length}/${r.points.length} 点 · 胶宽 ${mean(w).toFixed(2)} ± ${sd(w).toFixed(2)} mm · 偏移 ${mean(o).toFixed(2)} ± ${sd(o).toFixed(2)} mm`,
      });
    } catch (e) {
      if(valid())setNotice({ ok: false, text: String(e) });
    } finally {if(valid()){probingRef.current=false;setProbing(false);}}
  };

  const save = async () => {
    if(!sample||calibError||probingRef.current||savingRef.current)return;
    savingRef.current=true;setSaving(true);setNotice(null);
    const serial=++saveSerial.current;
    const valid=()=>current.current.alive&&current.current.scope===scope&&serial===saveSerial.current;
    try {
      const next = { ...config, follow: calib };
      await cameraApi.saveConfig(cam, next);
      if(!valid())return;
      onSaved(next);
      setNotice({ ok: true, text: "随动标定已保存" });
    } catch (e) {
      if(valid())setNotice({ ok: false, text: String(e) });
    } finally {if(valid()){savingRef.current=false;setSaving(false);}}
  };

  const num = (label: string, value: number, onChange: (v: number) => void, step = 1) => (
    <label className="field">
      <span>{label}</span>
      <input className="input mono" type="number" step={step} value={Number.isFinite(value)?value:""} onChange={(e) => onChange(e.target.value===""?NaN:Number(e.target.value))} />
    </label>
  );
  const arrowLen = Math.min(size[0], size[1]) * 0.35;
  const [nx, ny] = calib.nozzle;
  // 空输入保留为无效参数；绘图使用有限占位值，避免 SVG 接收 NaN 或 Infinity。
  const draw = (value: number) => Number.isFinite(value) ? value : 0;
  const drawNx = draw(nx), drawNy = draw(ny);
  const scaleError=scalePts.length===2?(!Number.isFinite(scaleMm)||scaleMm<=0?"比例尺实际距离必须大于 0":Math.hypot(scalePts[1][0]-scalePts[0][0],scalePts[1][1]-scalePts[0][1])<=3?"比例尺两点距离需大于 3 px":""):"";
  const calibError=scaleError||(!Number.isFinite(calib.mmPerPx)||calib.mmPerPx<=0?"像素当量必须大于 0":
    calib.mmPerPx<=.001||calib.mmPerPx>=5?"像素当量需在 0.001–5 mm/px 之间":
    size[0]<=16||size[1]<=16?"冻结图像尺寸需大于 16 px":
    ![nx,ny,calib.maskPx,calib.angleDeg].every(Number.isFinite)?"标定参数必须是有限数值":
    nx<0||nx>=size[0]||ny<0||ny>=size[1]?"胶嘴位置必须位于冻结图像内":
    calib.maskPx<0?"遮挡半径不得小于 0":"");
  const probeError=![probe.beadWidth,probe.searchMm,probe.nearMm,probe.farMm,travelDeg].every(Number.isFinite)?"试测参数必须是有限数值":
    probe.beadWidth<=0||probe.searchMm<=0?"名义胶宽和搜索半宽必须大于 0":
    probe.nearMm<0||probe.farMm<=probe.nearMm?"试测窗口远端必须大于近端，近端不得小于 0":"";

  return (
    <div className="panel">
      <fieldset disabled={saving||probing} style={{border:0,padding:0,margin:0,minWidth:0}}>
      <div className="panel-head">
        <h3 className="panel-title">随动标定</h3>
        <span className="muted">相机相对胶嘴：胶嘴位置、图像方位、像素当量（属于相机工位，换型不重标）</span>
        <span className="spacer" />
        <button className="btn" onClick={() => {setScalePts([]);setCalib(defaultCalib(cam, size));}} title="三目演示：胶嘴在下方正中，相机方位 120° 均布">
          默认三目
        </button>
        <button className="btn primary" onClick={save} disabled={!sample||probing||saving||!!calibError}>
          保存标定
        </button>
      </div>
      <div className="segmented">
        <button className={tool === "nozzle" ? "active" : ""} onClick={() => setTool("nozzle")}>
          <Crosshair size={14} /> 点胶嘴
        </button>
        <button className={tool === "direction" ? "active" : ""} onClick={() => setTool("direction")}>
          <MoveUpRight size={14} /> 点胶条方向
        </button>
        <button className={tool === "scale" ? "active" : ""} onClick={() => setTool("scale")}>
          <Ruler size={14} /> 量比例
        </button>
      </div>
      <p className="muted hint">
        {tool === "nozzle" && "在图上点胶嘴中心。"}
        {tool === "direction" && "机器人沿下面填的方向涂一段直线胶，在图上点胶条远离胶嘴的方向上任一点。"}
        {tool === "scale" && "在图上点两个相距已知长度的点（如胶嘴外径两侧、标尺刻度），填入实际距离。"}
      </p>
      <div className="calib-view">
        {sample&&<img src={sample.image.url} alt="冻结的随动标定原图" style={{width:"100%",display:"block"}}/>}
        {!img && <span className="muted">先在“冻结标定样本”取新样本，再在冻结图像上标定。</span>}
        <svg ref={svg} viewBox={`0 0 ${size[0]} ${size[1]}`} onClick={click} aria-label="随动标定图像，点击设置胶嘴、方向或比例尺">
          <circle cx={drawNx} cy={drawNy} r={Math.max(0, draw(calib.maskPx))} fill="var(--accent-overlay)" stroke="var(--accent-text)" strokeDasharray="6 5" vectorEffect="non-scaling-stroke" />
          <line x1={drawNx - 12} x2={drawNx + 12} y1={drawNy} y2={drawNy} stroke="var(--accent-text)" vectorEffect="non-scaling-stroke" />
          <line x1={drawNx} x2={drawNx} y1={drawNy - 12} y2={drawNy + 12} stroke="var(--accent-text)" vectorEffect="non-scaling-stroke" />
          <line
            x1={drawNx}
            y1={drawNy}
            x2={draw(drawNx + beadImg[0] * arrowLen)}
            y2={draw(drawNy + beadImg[1] * arrowLen)}
            stroke="var(--accent-text)"
            strokeWidth={2}
            strokeDasharray="10 6"
            vectorEffect="non-scaling-stroke"
          />
          {[probe.nearMm, probe.farMm].map((l,i) => (
            <circle key={i} cx={drawNx} cy={drawNy} r={Math.max(0, draw(l / calib.mmPerPx))} fill="none" stroke="var(--accent-overlay)" vectorEffect="non-scaling-stroke" />
          ))}
          {scalePts.map(([x, y], i) => (
            <circle key={i} cx={x} cy={y} r={5} fill="var(--text)" vectorEffect="non-scaling-stroke" />
          ))}
          {scalePts.length === 2 && <line x1={scalePts[0][0]} y1={scalePts[0][1]} x2={scalePts[1][0]} y2={scalePts[1][1]} stroke="var(--text)" vectorEffect="non-scaling-stroke" />}
          {points.map((p) => (
            <circle key={p.l} cx={p.px[0]} cy={p.px[1]} r={3} fill={p.st === 0 ? "var(--ok)" : "var(--ng)"} vectorEffect="non-scaling-stroke" />
          ))}
        </svg>
      </div>
      <div className="calib-row four">
        {num("胶嘴 x（px）", nx, (v) => setCalib({ ...calib, nozzle: [v, ny] }))}
        {num("胶嘴 y（px）", ny, (v) => setCalib({ ...calib, nozzle: [nx, v] }))}
        {num("遮挡半径（px）", calib.maskPx, (v) => setCalib({ ...calib, maskPx: v }))}
        {num("像素当量（mm/px）", calib.mmPerPx, (v) => setCalib({ ...calib, mmPerPx: v }), 0.001)}
        {num("图像方位（°）", calib.angleDeg, (v) => setCalib({ ...calib, angleDeg: v }), 0.5)}
        {num("标定时走向（°）", travelDeg, setTravelDeg, 5)}
        {num("量比例：实际距离（mm）", scaleMm, setScaleMm, 0.5)}
        <label className="check">
          <input type="checkbox" checked={calib.mirror} onChange={(e) => setCalib({ ...calib, mirror: e.target.checked })} />
          图像相对工件坐标镜像
        </label>
      </div>
      <div className="panel-head">
        <h4 className="sub-title">试测</h4>
        <span className="muted">假设胶嘴身后是一条沿黄色虚线的直胶条，逐 0.5 mm 跑卡尺</span>
        <span className="spacer" />
        <button className="btn" onClick={() => runProbe(true)} disabled={!img||probing||!!calibError||!!probeError} title="在冻结样本上转一圈找胶条方向，按标定时走向换算图像方位">
          自动找方向
        </button>
        <button className="btn" onClick={() => runProbe()} disabled={!img||probing||!!calibError||!!probeError}>
          <ScanLine size={15} />
          在当前帧试测
        </button>
      </div>
      <div className="calib-row four">
        <label className="field">
          <span>胶条极性</span>
          <select className="input" value={probe.polarity} onChange={(e) => setProbe({ ...probe, polarity: e.target.value as Polarity })}>
            <option value="dark">比背景暗</option>
            <option value="light">比背景亮</option>
            <option value="any">不限</option>
          </select>
        </label>
        {num("名义胶宽（mm）", probe.beadWidth, (v) => setProbe({ ...probe, beadWidth: v }), 0.1)}
        {num("搜索半宽（mm）", probe.searchMm, (v) => setProbe({ ...probe, searchMm: v }), 0.5)}
        {num("窗口近端（mm）", probe.nearMm, (v) => setProbe({ ...probe, nearMm: v }), 0.5)}
        {num("窗口远端（mm）", probe.farMm, (v) => setProbe({ ...probe, farMm: v }), 0.5)}
      </div>
      {(calibError||probeError)&&<div className="notice error" role="alert">{calibError||probeError}</div>}
      {notice && <div className={`notice ${notice.ok ? "ok" : "error"}`}>{notice.text}</div>}
      </fieldset>
    </div>
  );
}
