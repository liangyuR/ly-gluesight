import { useEffect, useImperativeHandle, useRef, useState, type ReactNode, type Ref } from "react";
import { RefreshCw } from "lucide-react";
import { cameraApi } from "../api";
import type { CameraConfig, CameraSource, CameraStatus, DeviceSummary } from "../types";

export interface CameraConfigHandle {
  save: () => Promise<boolean>;
  discard: () => void;
}

interface Props {
  ref?: Ref<CameraConfigHandle>;
  cam: number;
  initial: CameraConfig;
  status: CameraStatus | null;
  locked?: boolean;
  onSaved?: (c: CameraConfig) => void;
  onSavingChange?: (saving:boolean) => void;
  onDirtyChange?: (dirty:boolean) => void;
}

const sourceText: Record<CameraSource, string> = { mvs: "海康 MVS", sim: "合成图（离线）", sampleSet: "样本集（离线）" };
const normalize = (c: CameraConfig): CameraConfig => ({ ...c, acquisition: "triggered" });

function Group({ title, open = true, children }: { title: string; open?: boolean; children: ReactNode }) {
  return <details className="cfg-group" open={open}><summary>{title}</summary><div className="cfg-grid">{children}</div></details>;
}

export default function CameraConfigPanel({ ref, cam, initial, status, locked, onSaved, onSavingChange, onDirtyChange }: Props) {
  const [baseline, setBaseline] = useState<CameraConfig>(initial);
  const [config, setConfig] = useState<CameraConfig>(() => normalize(initial));
  const [devices, setDevices] = useState<DeviceSummary[]>([]);
  const [deviceError, setDeviceError] = useState("");
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const mounted=useRef(true),deviceSerial=useRef(0),pending=useRef(false);
  const current=useRef({cam,source:config.source});current.current={cam,source:config.source};
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;deviceSerial.current++;};},[]);
  useEffect(()=>()=>onSavingChange?.(false),[onSavingChange]);

  const refreshDevices = () => {
    const serial=++deviceSerial.current;
    setDeviceError("");
    cameraApi
      .listDevices()
      .then(r=>{if(mounted.current&&serial===deviceSerial.current&&current.current.source==="mvs")setDevices(r);})
      .catch(e=>{if(mounted.current&&serial===deviceSerial.current&&current.current.source==="mvs")setDeviceError(String(e));});
  };

  useEffect(() => {
    if (config.source === "mvs") refreshDevices();
    return()=>{deviceSerial.current++;};
  }, [config.source]);

  // 序列号留空的相机连上后后台会固定序列号：页面每次重新取配置都跟上；用户在表单里改过还没保存就不动
  const shownSerial = useRef(initial.serial);
  useEffect(() => {
    const prev = shownSerial.current;
    shownSerial.current = initial.serial;
    setBaseline(initial);
    setConfig((c) => (c.serial === prev ? { ...c, serial: initial.serial } : c));
  }, [initial]);

  const dirty = JSON.stringify(config) !== JSON.stringify(baseline);
  useEffect(()=>{onDirtyChange?.(dirty);},[dirty,onDirtyChange]);
  useEffect(()=>()=>onDirtyChange?.(false),[onDirtyChange]);

  const set = <K extends keyof CameraConfig>(key: K, value: CameraConfig[K]) => {setNotice(null);setConfig(c=>({ ...c, [key]: value }));};
  const numberLabels={triggerDelayUs:"触发延时（µs）",debouncerUs:"输入滤波（µs）",exposureUs:"曝光时间（µs）",gainDb:"增益（dB）"};
  const num = (key: keyof typeof numberLabels, step = 1) => (
    <input id={`cam-${key}`} aria-label={numberLabels[key]} className="input mono" type="number" step={step} value={Number.isFinite(config[key])?config[key]:""} onChange={(e) => set(key, e.target.value===""?NaN:Number(e.target.value))} />
  );
  const mvs = config.source === "mvs";
  const line0 = config.triggerSource === "Line0";
  const invalid=!config.name.trim()?"相机名称不能为空":
    ![1,3].includes(config.viewCount)?"设备视角数只能为 1 或 3":
    mvs&&!config.serial?"请选择要绑定的相机设备":
    mvs&&config.counterAfterOpen!=null&&(!Number.isSafeInteger(config.counterAfterOpen)||config.counterAfterOpen<0)?"开流前触发计数需为非负安全整数，未知时请留空":
    !(config.exposureUs>=1&&config.exposureUs<=1_000_000)?"曝光时间需在 1–1000000 µs 之间":
    !Number.isFinite(config.gainDb)?"增益需为有限数":
    ![config.triggerDelayUs,config.debouncerUs].every(v=>Number.isFinite(v)&&v>=0)?"触发延时与输入滤波需为非负有限数":"";

  const save = async () => {
    if(pending.current||invalid)return false;
    pending.current=true;
    setSaving(true);
    onSavingChange?.(true);
    setNotice(null);
    try {
      const next = { ...config };
      const warnings = await cameraApi.saveConfig(cam, next);
      if(!mounted.current||current.current.cam!==cam)return true;
      setNotice({ ok: warnings.length === 0, text: warnings.length ? `已应用，${warnings.length} 项参数相机未接受` : "已保存并应用" });
      shownSerial.current = next.serial;
      setBaseline(next);
      onSaved?.(next);
      return true;
    } catch (e) {
      if(mounted.current&&current.current.cam===cam)setNotice({ ok: false, text: String(e) });
      return false;
    } finally {
      pending.current=false;if(mounted.current&&current.current.cam===cam){setSaving(false);onSavingChange?.(false);}
    }
  };
  const discard = () => {setNotice(null);setConfig(normalize(baseline));};
  const saveRef=useRef(save);saveRef.current=save;
  const discardRef=useRef(discard);discardRef.current=discard;
  useImperativeHandle(ref,()=>({save:()=>saveRef.current(),discard:()=>discardRef.current()}),[]);

  const warnings = status?.warnings ?? [];
  return (
    <section className="panel cam-config" aria-label="相机参数">
      <fieldset disabled={saving||locked} className="cam-config-fields">
        <Group title="基本">
          <span>名称</span>
          <input id="cam-name" aria-label="名称" className="input" value={config.name} onChange={(e) => set("name", e.target.value)} />
          <span>相机编号</span>
          <span className="mono">{config.id}</span>
          <span>图像源</span>
          <span>{sourceText[config.source]}</span>
          <span>设备视角</span>
          <select aria-label="设备视角" className="input" value={config.viewCount} onChange={e => set("viewCount", Number(e.target.value))}>
            <option value={1}>单视角</option>
            <option value={3}>三目（一次触发三幅图）</option>
          </select>
          {config.viewCount === 3 && mvs && <><span>SDK 拼接布局</span><select className="input" aria-label="SDK 拼接布局" value={config.compositeLayout?.kind ?? ""} onChange={e => set("compositeLayout", e.target.value ? {kind:e.target.value as "horizontal" | "vertical"} : null)}><option value="">请选择实际交付布局</option><option value="horizontal">横向三等分</option><option value="vertical">纵向三等分</option>{config.compositeLayout?.kind === "rects" && <option value="rects">已配置矩形区域</option>}</select></>}
          {config.viewCount === 3 && <span className="hint-cell">每个拍照点可多选图 1／2／3 分别示教，三幅图共用一次设备触发。</span>}
          {config.source === "sampleSet" && <span className="hint-cell">图像来自离线模式所选样本集，相机与视角按样本集生成。</span>}
          {config.source === "sim" && <span className="hint-cell">合成图相机收到触发后约 180 ms 交付一帧，用于没有硬件时跑通节拍。</span>}
          {initial.acquisition === "freeRun" && <span className="hint-cell c-warn">连续采集已停用，保存后改为触发采集。</span>}
        </Group>
        {mvs && <>
          <Group title="物理设备">
            <span>相机设备</span>
            <div className="row">
              <select id="cam-serial" aria-label="相机设备" className="input grow" value={config.serial} onChange={(e) => set("serial", e.target.value)}>
                <option value="" disabled>请选择设备</option>
                {config.serial && !devices.some((d) => d.serial === config.serial) && <option value={config.serial}>{config.serial}（未发现）</option>}
                {devices.map((d) => <option key={d.serial} value={d.serial}>{d.model} · {d.serial} · {d.ip ?? d.transport}</option>)}
              </select>
              <button className="icon-btn" onClick={refreshDevices} title="重新枚举设备" aria-label="重新枚举设备"><RefreshCw size={16} /></button>
            </div>
            {deviceError && <span className="hint-cell c-ng">{deviceError}</span>}
            {config.serial && !devices.some((d) => d.serial === config.serial) && !deviceError && <span className="hint-cell c-warn">已绑定的设备当前未发现，检查网线与供电后重新枚举。</span>}
          </Group>
          <Group title="触发接线">
            <span>触发源</span>
            <select id="cam-trigger" aria-label="触发源" className="input" value={config.triggerSource} onChange={(e) => set("triggerSource", e.target.value as CameraConfig["triggerSource"])}>
              <option value="Line0">Line0（机器人位置比较输出）</option>
              <option value="Software">Software（台架调试、模拟节拍）</option>
            </select>
            {line0 && <>
              <span>触发沿</span>
              <select id="cam-activation" aria-label="触发沿" className="input" value={config.triggerActivation} onChange={(e) => set("triggerActivation", e.target.value as CameraConfig["triggerActivation"])}>
                <option value="RisingEdge">上升沿</option>
                <option value="FallingEdge">下降沿</option>
              </select>
              <span>输入滤波（µs）</span>
              {num("debouncerUs")}
              <span className="hint-cell">滤掉 Line0 毛刺，避免多帧</span>
            </>}
            <span>触发延时（µs）</span>
            {num("triggerDelayUs")}
            <span>Line1 输出</span>
            <label className="check"><input type="checkbox" checked={config.strobe} onChange={(e) => set("strobe", e.target.checked)} />曝光信号驱动频闪</label>
            <span>Chunk 数据</span>
            <label className="check"><input type="checkbox" checked={config.chunk} onChange={(e) => set("chunk", e.target.checked)} />帧计数、触发计数、时间戳</label>
          </Group>
          <Group title="成像">
            <span>曝光时间（µs）</span>
            {num("exposureUs")}
            <span className="hint-cell">运动中取图需要微秒级曝光配合频闪</span>
            <span>增益（dB）</span>
            {num("gainDb", 0.5)}
          </Group>
          <Group title="高级" open={false}>
            <span>开流前触发计数</span>
            <input aria-label="开流前触发计数" className="input mono" type="number" min={0} max={Number.MAX_SAFE_INTEGER} step={1} placeholder="未知" value={config.counterAfterOpen??""} onChange={e=>set("counterAfterOpen",e.target.value===""?null:Number(e.target.value))}/>
            <span className="hint-cell">仅填写台架确认的开流前触发计数；未知时留空，不能假定为 0。</span>
            <span className="hint-cell">固定写入：ExposureAuto/GainAuto=Off、PixelFormat=Mono8、TriggerMode=On；GigE 相机自动设置最佳包长。{status?.sdkVersion && ` SDK ${status.sdkVersion}`}</span>
          </Group>
        </>}
      </fieldset>
      {!locked && <div className="cam-save-bar">
        {invalid ? <span className="c-ng" role="alert">{invalid}</span> : dirty ? <span className="c-warn">● 有未保存修改</span> : notice ? <span className={notice.ok ? "c-ok" : "c-ng"}>{notice.text}</span> : <span className="muted">参数与相机一致</span>}
        <span className="spacer" />
        <button className="btn" onClick={discard} disabled={saving||!dirty}>放弃</button>
        <button className="btn primary" onClick={()=>void save()} disabled={saving||!dirty||!!invalid}>{saving ? "写入中…" : "保存并应用"}</button>
      </div>}
      {warnings.length > 0 && <details className="cam-warnings"><summary>相机未接受 {warnings.length} 项参数</summary><ul>{warnings.map((w) => <li key={w}>{w}</li>)}</ul></details>}
    </section>
  );
}
