import { useEffect, useRef, useState } from "react";
import { FolderOpen, RefreshCw } from "lucide-react";
import { cameraApi } from "../api";
import type { CameraConfig, CameraSource, CameraStatus, DeviceSummary, FollowCalib, RecordEntry } from "../types";

interface Props {
  cam: number;
  initial: CameraConfig;
  /** 随动标定由标定面板维护，这里保存时带上它的最新值 */
  follow: FollowCalib | null;
  status: CameraStatus | null;
  onSaved?: (c: CameraConfig) => void;
  onSavingChange?: (saving:boolean) => void;
}

const sources: [CameraSource, string][] = [
  ["mvs", "海康 MVS"],
  ["sim", "模拟相机"],
  ["replay", "回放目录"],
];

export default function CameraConfigPanel({ cam, initial, follow, status, onSaved, onSavingChange }: Props) {
  const [config, setConfig] = useState<CameraConfig>(initial);
  const [devices, setDevices] = useState<DeviceSummary[]>([]);
  const [records, setRecords] = useState<RecordEntry[]>([]);
  const [deviceError, setDeviceError] = useState("");
  const [recordError, setRecordError] = useState("");
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [choosingDirectory, setChoosingDirectory] = useState(false);
  const [directoryError, setDirectoryError] = useState("");
  const directoryPending = useRef(false);
  const mounted=useRef(true),deviceSerial=useRef(0),recordSerial=useRef(0),pending=useRef(false);
  const current=useRef({cam,source:config.source});current.current={cam,source:config.source};
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;deviceSerial.current++;recordSerial.current++;};},[]);
  useEffect(()=>()=>onSavingChange?.(false),[onSavingChange]);

  const refreshDevices = () => {
    const serial=++deviceSerial.current;
    setDeviceError("");
    cameraApi
      .listDevices()
      .then(r=>{if(mounted.current&&serial===deviceSerial.current&&current.current.source==="mvs")setDevices(r);})
      .catch(e=>{if(mounted.current&&serial===deviceSerial.current&&current.current.source==="mvs")setDeviceError(String(e));});
  };
  const refreshRecords=()=>{
    const serial=++recordSerial.current;
    setRecordError("");
    cameraApi.records().then(r=>{if(mounted.current&&serial===recordSerial.current&&current.current.source==="replay")setRecords(r.items);})
      .catch(e=>{if(mounted.current&&serial===recordSerial.current&&current.current.source==="replay")setRecordError(String(e));});
  };

  useEffect(() => {
    if (config.source === "mvs") refreshDevices();
    if (config.source === "replay") refreshRecords();
    return()=>{deviceSerial.current++;recordSerial.current++;};
  }, [config.source]);

  // 序列号留空的相机连上后后台会固定序列号：页面每次重新取配置都跟上；用户在表单里改过还没保存就不动
  const shownSerial = useRef(initial.serial);
  useEffect(() => {
    const prev = shownSerial.current;
    shownSerial.current = initial.serial;
    setConfig((c) => (c.serial === prev ? { ...c, serial: initial.serial } : c));
  }, [initial]);

  const set = <K extends keyof CameraConfig>(key: K, value: CameraConfig[K]) => {setNotice(null);setConfig(c=>({ ...c, [key]: value }));};
  const chooseDirectory = async () => {
    if (directoryPending.current || pending.current) return;
    directoryPending.current = true;
    setChoosingDirectory(true);
    setDirectoryError("");
    try {
      const directory = await cameraApi.pickReplayDir(config.replayDir);
      if (mounted.current && current.current.cam === cam && current.current.source === "replay" && directory !== null) set("replayDir", directory);
    } catch (e) {
      if (mounted.current && current.current.cam === cam && current.current.source === "replay") setDirectoryError(String(e));
    } finally {
      directoryPending.current = false;
      if (mounted.current) setChoosingDirectory(false);
    }
  };
  const numberLabels={triggerDelayUs:"触发延时（µs）",debouncerUs:"输入滤波（µs）",exposureUs:"曝光时间（µs）",gainDb:"增益（dB）",fps:"帧率（fps）",replayChannel:"通道"};
  const num = (key: "triggerDelayUs" | "debouncerUs" | "exposureUs" | "gainDb" | "fps" | "replayChannel", step = 1) => (
    <input id={`cam-${key}`} aria-label={numberLabels[key]} className="input mono" type="number" step={step} value={Number.isFinite(config[key])?config[key]:""} onChange={(e) => set(key, e.target.value===""?NaN:Number(e.target.value))} />
  );
  const mvs = config.source === "mvs";
  const replay = config.source === "replay";
  const triggered = config.acquisition === "triggered";
  const invalid=!config.name.trim()?"相机名称不能为空":
    !(config.exposureUs>=1&&config.exposureUs<=1_000_000)?"曝光时间需在 1–1000000 µs 之间":
    !Number.isFinite(config.gainDb)?"增益需为有限数":
    ![config.triggerDelayUs,config.debouncerUs].every(v=>Number.isFinite(v)&&v>=0)?"触发延时与输入滤波需为非负有限数":
    !triggered&&!(config.fps>=1&&config.fps<=500)?"连续采集帧率需在 1–500 fps 之间":
    replay&&!config.replayDir.trim()?"回放相机需要填写图片目录":
    replay&&(!Number.isInteger(config.replayChannel)||config.replayChannel<0)?"回放通道需为非负整数":"";

  const save = async () => {
    if(pending.current||directoryPending.current||invalid)return;
    pending.current=true;
    setSaving(true);
    onSavingChange?.(true);
    setNotice(null);
    try {
      const next = { ...config, follow };
      const warnings = await cameraApi.saveConfig(cam, next);
      if(!mounted.current||current.current.cam!==cam)return;
      setNotice({ ok: warnings.length === 0, text: warnings.length ? `已应用，${warnings.length} 项参数相机未接受` : "已保存并应用" });
      shownSerial.current = next.serial;
      onSaved?.(next);
    } catch (e) {
      if(mounted.current&&current.current.cam===cam)setNotice({ ok: false, text: String(e) });
    } finally {
      pending.current=false;if(mounted.current&&current.current.cam===cam){setSaving(false);onSavingChange?.(false);}
    }
  };

  return (
    <div className="panel cam-config">
      <div className="panel-toolbar">
        <h3 className="panel-title">相机参数</h3>
        <button className="btn primary" onClick={save} disabled={saving||choosingDirectory||!!invalid}>
          {saving ? "写入中…" : "保存并应用"}
        </button>
      </div>
      <fieldset disabled={saving||choosingDirectory} style={{border:0,padding:0,margin:0,minWidth:0}}><div className="cfg-grid">
        <span>名称</span>
        <input id="cam-name" aria-label="名称" className="input" value={config.name} onChange={(e) => set("name", e.target.value)} />
        <span>图像源</span>
        <div className="segmented">
          {sources.map(([v, label]) => (
            <button key={v} className={config.source === v ? "active" : ""} onClick={() => set("source", v)}>
              {label}
            </button>
          ))}
        </div>
        <span>采集方式</span>
        <div className="segmented">
          <button className={triggered ? "active" : ""} onClick={() => set("acquisition", "triggered")}>触发（飞拍）</button>
          <button className={!triggered ? "active" : ""} onClick={() => set("acquisition", "freeRun")}>连续（随动）</button>
        </div>
        {!triggered && (
          <>
            <span>帧率（fps）</span>
            {num("fps")}
            <span className="hint-cell">布防期间按此帧率出帧；胶嘴速度 ÷ 帧率 就是相邻两帧间胶嘴走过的距离</span>
          </>
        )}
        {replay && (
          <>
            <span>图片目录</span>
            <div className="row">
              <input id="cam-replayDir" aria-label="图片目录" className="input mono grow" value={config.replayDir} placeholder="D:\现场图\Glue1" onChange={(e) => set("replayDir", e.target.value)} />
              <button className="btn cam-directory-btn" onClick={chooseDirectory}><FolderOpen size={16} />{choosingDirectory ? "选择中…" : "选择目录"}</button>
            </div>
            {directoryError && <span className="hint-cell c-ng" role="alert">{directoryError}</span>}
            {recordError&&<span className="hint-cell c-ng">{recordError} <button className="btn small" onClick={refreshRecords}>重新读取录制</button></span>}
            {records.length > 0 && (
              <>
                <span>帧录制</span>
                <select aria-label="帧录制" className="input" value="" onChange={(e) => e.target.value && set("replayDir", e.target.value)}>
                  <option value="">从录制目录里选…</option>
                  {records.map((r) => (
                    <option key={r.path} value={r.path}>
                      {r.name} · {r.frames} 帧
                    </option>
                  ))}
                </select>
              </>
            )}
            <span>通道</span>
            {num("replayChannel")}
            <span className="hint-cell">认 cam{"{通道}"}_{"{序号}"}.pgm（帧录制）与 Frame{"{序号}"}_{"{通道}"}.jpg（海康演示图）；0 取目录里的第一个通道</span>
          </>
        )}
        {mvs && (
          <>
            <span>相机</span>
            <div className="row">
              <select id="cam-serial" aria-label="相机设备" className="input grow" value={config.serial} onChange={(e) => set("serial", e.target.value)}>
                <option value="">第一台可用相机</option>
                {config.serial && !devices.some((d) => d.serial === config.serial) && <option value={config.serial}>{config.serial}（未发现）</option>}
                {devices.map((d) => (
                  <option key={d.serial} value={d.serial}>
                    {d.model} · {d.serial} · {d.ip ?? d.transport}
                  </option>
                ))}
              </select>
              <button className="icon-btn" onClick={refreshDevices} title="重新枚举">
                <RefreshCw size={16} />
              </button>
            </div>
            {deviceError && <span className="hint-cell c-ng">{deviceError}</span>}
            {triggered && (
              <>
                <span>触发源</span>
                <select id="cam-trigger" aria-label="触发源" className="input" value={config.triggerSource} onChange={(e) => set("triggerSource", e.target.value as CameraConfig["triggerSource"])}>
                  <option value="Line0">Line0（机器人位置比较输出）</option>
                  <option value="Software">Software（台架调试、模拟节拍）</option>
                </select>
                <span>触发沿</span>
                <select
                  id="cam-activation"
                  aria-label="触发沿"
                  className="input"
                  value={config.triggerActivation}
                  onChange={(e) => set("triggerActivation", e.target.value as CameraConfig["triggerActivation"])}
                  disabled={config.triggerSource !== "Line0"}
                >
                  <option value="RisingEdge">上升沿</option>
                  <option value="FallingEdge">下降沿</option>
                </select>
                <span>触发延时（µs）</span>
                {num("triggerDelayUs")}
                <span>输入滤波（µs）</span>
                {num("debouncerUs")}
                <span className="hint-cell">滤掉 Line0 毛刺，避免多帧</span>
              </>
            )}
            <span>曝光时间（µs）</span>
            {num("exposureUs")}
            <span className="hint-cell">运动中取图需要微秒级曝光配合频闪</span>
            <span>增益（dB）</span>
            {num("gainDb", 0.5)}
            <span>Line1 输出</span>
            <label className="check">
              <input type="checkbox" checked={config.strobe} onChange={(e) => set("strobe", e.target.checked)} />
              曝光信号（ExposureStartActive）驱动频闪
            </label>
            <span>Chunk 数据</span>
            <label className="check">
              <input type="checkbox" checked={config.chunk} onChange={(e) => set("chunk", e.target.checked)} />
              帧计数、Line0 触发计数、时间戳
            </label>
          </>
        )}
      </div></fieldset>
      {invalid&&<div className="notice error" role="alert">{invalid}</div>}
      {mvs && (
        <p className="muted hint">
          固定写入：ExposureAuto/GainAuto=Off、PixelFormat=Mono8；{triggered ? "TriggerMode=On" : "TriggerMode=Off + AcquisitionFrameRate"}；GigE 相机自动设置最佳包长。
          {status?.sdkVersion && ` SDK ${status.sdkVersion}`}
        </p>
      )}
      {config.source === "sim" && (
        <p className="muted hint">
          {triggered ? "模拟相机收到触发后约 180 ms 交付一帧，用于没有硬件时跑通节拍。" : "模拟随动相机按胶嘴此刻的位置合成画面，需要先做随动标定（可用下方的默认三目标定）。"}
        </p>
      )}
      {notice && <div className={`notice ${notice.ok ? "ok" : "error"}`}>{notice.text}</div>}
      {status?.warnings.length ? (
        <ul className="warn-list">
          {status.warnings.map((w) => (
            <li key={w}>{w}</li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
