import { useEffect, useState } from "react";

interface Inputs {
  speed: number;
  exposure: number;
  mmPerPx: number;
  fps: number;
  spacing: number;
  fov: number;
}

const fields: [keyof Inputs, string, number][] = [
  ["speed", "速度（mm/s）", 50],
  ["exposure", "曝光（µs）", 10],
  ["mmPerPx", "像素当量（mm/px）", 0.005],
  ["fps", "帧率（fps）", 0.1],
  ["spacing", "最小拍照点间距（mm）", 5],
  ["fov", "视野沿运动方向（mm）", 1],
];

/** 按方案文档 2.1 / 2.3 核算：模糊 ≤ 0.5 px，相邻触发间隔 ≥ 单帧时间，相邻视野留出同框余量。 */
export default function FeasibilityCalc({ exposure, fps }: { exposure?: number; fps?: number | null }) {
  const [v, setV] = useState<Inputs>({ speed: 300, exposure: 60, mmPerPx: 0.04, fps: 5.6, spacing: 165, fov: 216 });

  useEffect(() => {
    if(exposure!==undefined)setV(prev=>({...prev,exposure}));
  }, [exposure]);
  useEffect(()=>{if(fps!=null)setV(prev=>({...prev,fps}));},[fps]);

  const blur = v.speed * v.exposure * 1e-6;
  const blurPx = v.mmPerPx ? blur / v.mmPerPx : 0;
  const maxExposure = v.speed ? ((0.5 * v.mmPerPx) / v.speed) * 1e6 : 0;
  const frameMs = v.fps ? 1000 / v.fps : 0;
  const triggerMs = v.speed ? (v.spacing / v.speed) * 1000 : 0;
  const overlap = v.fov - v.spacing;
  const vBlur = v.exposure ? (0.5 * v.mmPerPx) / (v.exposure * 1e-6) : 0;
  const vFps = v.spacing * v.fps;
  const vMax = Math.min(vBlur, vFps);
  const valid=Object.values(v).every(value=>Number.isFinite(value)&&value>0)&&[blur,blurPx,maxExposure,frameMs,triggerMs,overlap,vBlur,vFps,vMax].every(Number.isFinite);

  const rows: [string, string, boolean][] = [
    ["运动模糊", `${blur.toFixed(3)} mm = ${blurPx.toFixed(2)} px（上限 0.5 px，曝光 ≤ ${maxExposure.toFixed(0)} µs）`, blurPx <= 0.5],
    ["相邻触发间隔", `${triggerMs.toFixed(0)} ms（单帧时间 ${frameMs.toFixed(0)} ms）`, triggerMs >= frameMs],
    ["相邻视野重叠", `${overlap.toFixed(0)} mm（需 ≥ 20 mm 同框余量）`, overlap >= 20],
    ["该配置允许的最高速度", `${vMax.toFixed(0)} mm/s（模糊限 ${vBlur.toFixed(0)} · 帧率限 ${vFps.toFixed(0)}）`, v.speed <= vMax],
  ];

  return (
    <div className="panel">
      <div className="panel-head">
        <h3 className="panel-title">飞拍可行性</h3>
        <span className="muted">按位置触发时视野重叠由拍照点间距决定，速度只受曝光模糊和帧时间限制</span>
      </div>
      <div className="calc-in">
        {fields.map(([key, label, step]) => (
          <label key={key} className="field">
            <span>{label}</span>
            <input id={`calc-${key}`} className="input mono" type="number" min={0} step={step} value={Number.isFinite(v[key])?v[key]:""} onChange={(e) => setV(previous=>({ ...previous, [key]: e.target.value===""?NaN:Number(e.target.value) }))} />
          </label>
        ))}
      </div>
      <div className="calc-out">
        {!valid&&<p className="c-ng" role="alert">速度、曝光、像素当量、帧率、拍照点间距和视野必须大于 0 且为有限数；数值过大时请降低数量级。</p>}
        <p className={valid&&rows.every(row=>row[2])?"c-ok":"c-ng"} role="status">{!valid?"请修正参数后计算":rows.every(row=>row[2])?"当前输入满足飞拍条件":"当前输入存在未满足项"}</p>
        {rows.map(([label, value, ok], i) => (
          <div key={label} className={i === rows.length - 1 ? "big" : ""}>
            <span>{label}</span>
            <b className="mono">{valid?value:"参数无效"}</b>
            <span className={ok&&valid ? "c-ok" : "c-ng"}>{ok&&valid ? "满足" : "不满足"}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
