import { useEffect, useRef, useState } from "react";
import { storageApi, type StoragePreferences } from "../../../lib/storage";
import { errorText } from "../../../lib/errors";

interface Inputs {
  speed: number;
  exposure: number;
  mmPerPx: number;
  fps: number;
  spacing: number;
  fov: number;
}
type Key = keyof Inputs;
export interface FeasibilitySource { value: number; from: string }
export type FeasibilitySources = Partial<Record<Key, FeasibilitySource | null>>;

const fields: [Key, string, number][] = [
  ["speed", "速度（mm/s）", 50],
  ["exposure", "曝光（µs）", 10],
  ["mmPerPx", "像素当量（mm/px）", 0.005],
  ["fps", "帧率（fps）", 0.1],
  ["spacing", "最小拍照点间距（mm）", 5],
  ["fov", "视野沿运动方向（mm）", 1],
];
const manualHint: Record<Key, string> = {
  speed: "手填：机器人经过拍照点时的线速度",
  exposure: "手填：相机曝光时间",
  mmPerPx: "可手填用于本页估算，不作为标定验证结果",
  fps: "可手填估算，相机连接后自动带入",
  spacing: "手填：相邻拍照点沿路径的最小距离",
  fov: "可手填；有图像尺寸和像素当量后自动计算",
};
const format = (key: Key, value: number) => key === "mmPerPx" ? value.toFixed(4) : String(Math.round(value * 10) / 10);

/** 按方案文档 2.1 / 2.3 核算：模糊 ≤ 0.5 px，相邻触发间隔 ≥ 单帧时间，相邻视野留出同框余量。 */
export default function FeasibilityCalc({ sources: given = {}, imageSize }: { sources?: FeasibilitySources; imageSize?: { width: number; height: number } | null }) {
  const [manual, setManual] = useState<Partial<Inputs>>({});
  const [axis, setAxis] = useState<"width" | "height">("width");
  const mm = "mmPerPx" in manual ? manual.mmPerPx! : given.mmPerPx?.value ?? NaN;
  const sources: FeasibilitySources = { ...given, fov: given.fov ?? (imageSize && Number.isFinite(mm) && mm > 0 ? { value: Math.round(imageSize[axis] * mm * 10) / 10, from: `图像${axis === "width" ? "宽" : "高"} ${imageSize[axis]} px × 当量` } : null) };
  const touched = useRef(new Set<Key>());
  const [storageError, setStorageError] = useState("");
  const [loadingPreferences, setLoadingPreferences] = useState(true);
  const [readRevision, setReadRevision] = useState(0);
  const saveVersion = useRef(0);
  const save = (patch: StoragePreferences) => {
    const version = ++saveVersion.current;
    void storageApi.updatePreferences(patch).then(() => {
      if (version === saveVersion.current) setStorageError("");
    }).catch(error => { if (version === saveVersion.current) setStorageError(`飞拍参数未保存：${errorText(error)}`); });
  };
  useEffect(() => {
    let active = true;
    setLoadingPreferences(true);
    void storageApi.preferences().then(saved => {
      if (!active) return;
      setStorageError("");
      setManual(previous => ({ ...previous,
        ...(!touched.current.has("speed") && saved.flyshotSpeedMmS != null ? { speed: saved.flyshotSpeedMmS } : {}),
        ...(!touched.current.has("spacing") && saved.flyshotSpacingMm != null ? { spacing: saved.flyshotSpacingMm } : {}),
      }));
    }).catch(error => { if (active) setStorageError(`飞拍参数读取失败：${errorText(error)}`); })
      .finally(() => { if (active) setLoadingPreferences(false); });
    return () => { active = false; };
  }, [readRevision]);
  const change = (key: Key, value: number) => {
    touched.current.add(key);
    setManual(previous => ({ ...previous, [key]: value }));
    if (key === "speed") save({ flyshotSpeedMmS: Number.isFinite(value) ? value : null });
    if (key === "spacing") save({ flyshotSpacingMm: Number.isFinite(value) ? value : null });
  };
  const pick = (k: Key) => (k in manual ? manual[k] : sources[k]?.value) ?? NaN;
  const v: Inputs = { speed: pick("speed"), exposure: pick("exposure"), mmPerPx: pick("mmPerPx"), fps: pick("fps"), spacing: pick("spacing"), fov: pick("fov") };
  const restore = (key: Key) => {
    touched.current.add(key);
    setManual(prev => { const next = { ...prev }; delete next[key]; return next; });
    if (key === "speed") save({ flyshotSpeedMmS: null });
    if (key === "spacing") save({ flyshotSpacingMm: null });
  };

  const blur = v.speed * v.exposure * 1e-6;
  const blurPx = v.mmPerPx ? blur / v.mmPerPx : 0;
  const maxExposure = v.speed ? ((0.5 * v.mmPerPx) / v.speed) * 1e6 : 0;
  const frameMs = v.fps ? 1000 / v.fps : 0;
  const triggerMs = v.speed ? (v.spacing / v.speed) * 1000 : 0;
  const overlap = v.fov - v.spacing;
  const vBlur = v.exposure ? (0.5 * v.mmPerPx) / (v.exposure * 1e-6) : 0;
  const vFps = v.spacing * v.fps;
  const vMax = Math.min(vBlur, vFps);
  const missing = fields.filter(([k]) => !(Number.isFinite(v[k]) && v[k] > 0)).map(([, label]) => label.replace(/（.*）/, ""));
  const valid = !missing.length && [blur, blurPx, maxExposure, frameMs, triggerMs, overlap, vBlur, vFps, vMax].every(Number.isFinite);

  const rows: [string, string, boolean][] = [
    ["运动模糊", `${blur.toFixed(3)} mm = ${blurPx.toFixed(2)} px（上限 0.5 px，曝光 ≤ ${maxExposure.toFixed(0)} µs）`, blurPx <= 0.5],
    ["相邻触发间隔", `${triggerMs.toFixed(0)} ms（单帧时间 ${frameMs.toFixed(0)} ms）`, triggerMs >= frameMs],
    ["相邻视野重叠", `${overlap.toFixed(0)} mm（需 ≥ 20 mm 同框余量）`, overlap >= 20],
    ["该配置允许的最高速度", `${vMax.toFixed(0)} mm/s（模糊限 ${vBlur.toFixed(0)} · 帧率限 ${vFps.toFixed(0)}）`, v.speed <= vMax],
  ];
  const status = missing.length ? `还缺：${missing.join("、")}` : !valid ? "数值过大，请降低数量级后再核算" : rows.every(row => row[2]) ? "按当前数值满足飞拍条件" : "按当前数值存在未满足项";

  return (
    <div className="panel">
      <div className="panel-head">
        <h3 className="panel-title">飞拍可行性</h3>
        <span className="muted">核算按这组速度、曝光和拍照点间距飞拍时，图像会不会拖影、相机来不来得及、相邻视野是否连续</span>
      </div>
      {loadingPreferences && <p className="muted" aria-live="polite">正在读取已保存的飞拍参数…</p>}
      {storageError && <p className="notice error" role="alert">{storageError} <button type="button" className="btn" onClick={() => { if (!touched.current.has("speed") && !touched.current.has("spacing")) { setReadRevision(v => v + 1); return; } save({
        ...(touched.current.has("speed") ? { flyshotSpeedMmS: Number.isFinite(manual.speed) ? manual.speed! : null } : {}),
        ...(touched.current.has("spacing") ? { flyshotSpacingMm: Number.isFinite(manual.spacing) ? manual.spacing! : null } : {}),
      }); }}>重试保存</button></p>}
      <div className="calc-in">
        {fields.map(([key, label, step]) => {
          const source = sources[key], overridden = !!source && key in manual;
          return (
            <label key={key} className="field">
              <span>{label}</span>
              <input id={`calc-${key}`} className="input mono" type="number" min={0} step={step} placeholder="请填写" value={Number.isFinite(v[key]) ? v[key] : ""} onChange={(e) => change(key, e.target.value === "" ? NaN : Number(e.target.value))} />
              <small className={"calc-src" + (overridden ? " changed" : source ? " linked" : "")}>
                {overridden ? <>已手改，{source.from}为 {format(key, source.value)}<button type="button" className="calc-restore" onClick={() => restore(key)}>恢复</button></> : source ? source.from : manualHint[key]}
              </small>
              {key === "fov" && imageSize && !given.fov && <select className="input calc-axis" aria-label="运动方向" value={axis} onChange={e => setAxis(e.target.value as "width" | "height")}><option value="width">工件沿图像宽度方向运动</option><option value="height">工件沿图像高度方向运动</option></select>}
            </label>
          );
        })}
      </div>
      <div className="calc-out">
        <p className={valid && rows.every(row => row[2]) ? "c-ok" : missing.length ? "muted" : "c-ng"} role="status">{status}</p>
        {rows.map(([label, value, ok], i) => (
          <div key={label} className={i === rows.length - 1 ? "big" : ""}>
            <span>{label}</span>
            <b className="mono">{valid ? value : "—"}</b>
            <span className={!valid ? "muted" : ok ? "c-ok" : "c-ng"}>{!valid ? "待核算" : ok ? "满足" : "不满足"}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
