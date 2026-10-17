import { useEffect, useImperativeHandle, useRef, useState, type ReactNode, type Ref } from "react";
import { AlertTriangle, Check, ChevronRight, Copy, Database, Info, Link2Off, Lock, Minus, Radio, RefreshCw, SlidersHorizontal } from "lucide-react";
import { cameraApi } from "../api";
import type { CameraConfig, CameraStatus, DeviceSummary } from "../types";
import { errorText } from "../../../lib/errors";
import { clock, sourceText } from "./health";

export interface CameraConfigHandle {
  save: () => Promise<boolean>;
  discard: () => void;
}

interface Props {
  ref?: Ref<CameraConfigHandle>;
  cam: number;
  initial: CameraConfig;
  status: CameraStatus | null;
  /** 相机组当前配置与状态，“应用到全部相机”用 */
  group: { configs: CameraConfig[]; statuses: CameraStatus[] };
  locked?: boolean;
  saveBlocked?: string;
  onSaved?: (c: CameraConfig) => void;
  onGroupSaved?: () => void;
  onSavingChange?: (saving:boolean) => void;
  onDirtyChange?: (dirty:boolean) => void;
  /** 右侧出图验证等 */
  side?: ReactNode;
  /** 保存栏左侧（移除相机） */
  leading?: ReactNode;
  footer?: ReactNode;
}

type Wiring = Pick<CameraConfig, "triggerSource" | "triggerActivation" | "debouncerUs" | "triggerDelayUs" | "strobe" | "chunk">;
const wiring = (c: CameraConfig): Wiring => ({ triggerSource: c.triggerSource, triggerActivation: c.triggerActivation, debouncerUs: c.debouncerUs, triggerDelayUs: c.triggerDelayUs, strobe: c.strobe, chunk: c.chunk });

interface BatchItem { id: string; name: string; tone: "ok" | "warn" | "ng" | "off"; text: string; list?: string[] }

/** 相机未接受的参数按节点名对到表单字段，方便就地标注；完整列表仍在右侧。 */
const nodes: [keyof CameraConfig, RegExp][] = [
  ["triggerDelayUs", /TriggerDelay/],
  ["debouncerUs", /Debouncer/],
  ["triggerActivation", /TriggerActivation/],
  ["triggerSource", /TriggerSource|TriggerMode/],
  ["strobe", /Line(Selector|Mode|Source|Inverter)|Strobe/],
  ["chunk", /Chunk/],
];

/** 调试下发过的曝光与增益：不写配方，离开页面或重连后也保留显示（用户要求不清理）。 */
const sentImaging = new Map<string, { exposureUs: number; gainDb: number; at: number }>();

export default function CameraConfigPanel({ ref, cam, initial, status, group, locked, saveBlocked, onSaved, onGroupSaved, onSavingChange, onDirtyChange, side, leading, footer }: Props) {
  const [baseline, setBaseline] = useState<CameraConfig>(initial);
  const [config, setConfig] = useState<CameraConfig>(initial);
  const [devices, setDevices] = useState<DeviceSummary[]>([]);
  const [enumerated, setEnumerated] = useState<number | null>(null);
  const [deviceError, setDeviceError] = useState("");
  const [notice, setNotice] = useState<{ tone: "ok" | "warn" | "ng"; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [batch, setBatch] = useState<{ at: number; items: BatchItem[] } | null>(null);
  const [batching, setBatching] = useState(false);
  const mounted=useRef(true),deviceSerial=useRef(0),pending=useRef(false);
  const current=useRef({cam,source:config.source});current.current={cam,source:config.source};
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;deviceSerial.current++;};},[]);
  useEffect(()=>()=>onSavingChange?.(false),[onSavingChange]);

  const refreshDevices = () => {
    const serial=++deviceSerial.current;
    setDeviceError("");
    cameraApi
      .listDevices()
      .then(r=>{if(mounted.current&&serial===deviceSerial.current&&current.current.source==="mvs"){setDevices(r);setEnumerated(Date.now());}})
      .catch(e=>{if(mounted.current&&serial===deviceSerial.current&&current.current.source==="mvs")setDeviceError(String(e));});
  };

  useEffect(() => {
    if (config.source === "mvs") refreshDevices();
    return()=>{deviceSerial.current++;};
  }, [config.source]);

  useEffect(() => { setBaseline(initial); }, [initial]);

  const dirty = JSON.stringify(config) !== JSON.stringify(baseline);
  useEffect(()=>{onDirtyChange?.(dirty);},[dirty,onDirtyChange]);
  useEffect(()=>()=>onDirtyChange?.(false),[onDirtyChange]);

  const set = <K extends keyof CameraConfig>(key: K, value: CameraConfig[K]) => {setNotice(null);setConfig(c=>({ ...c, [key]: value }));};
  const warnings = status?.warnings ?? [];
  const fieldWarnings = (key: keyof CameraConfig) => { const re = nodes.find(([k]) => k === key)?.[1]; return re ? warnings.filter(w => re.test(w)) : []; };
  const label = (text: string, key?: keyof CameraConfig) => <span className="lbl">{text}{key && fieldWarnings(key).length > 0 && <AlertTriangle size={13} aria-label="相机未接受" />}</span>;
  const rejected = (key: keyof CameraConfig) => fieldWarnings(key).map(w => <span key={w} className="hint warn"><AlertTriangle size={12} /><span>相机未接受：{w}。相机保持写入前的值。</span></span>);
  const numberLabels={triggerDelayUs:"触发延时（µs）",debouncerUs:"输入滤波（µs）"};
  const num = (key: keyof typeof numberLabels) => (
    <span className="cg-unit"><input id={`cam-${key}`} aria-label={numberLabels[key]} className={"input mono"+(fieldWarnings(key).length?" invalid":"")} type="number" step={1} value={Number.isFinite(config[key])?config[key]:""} onChange={(e) => set(key, e.target.value===""?NaN:Number(e.target.value))} /><small>µs</small></span>
  );
  const mvs = config.source === "mvs";
  const line0 = config.triggerSource === "Line0";
  const invalid=!config.name.trim()?"相机名称不能为空":
    ![1,3].includes(config.viewCount)?"设备视角数只能为 1 或 3":
    mvs&&!config.serial?"请选择要绑定的相机设备":
    mvs&&config.counterAfterOpen!=null&&(!Number.isSafeInteger(config.counterAfterOpen)||config.counterAfterOpen<0)?"开流前触发计数需为非负安全整数，未知时请留空":
    ![config.triggerDelayUs,config.debouncerUs].every(v=>Number.isFinite(v)&&v>=0)?"触发延时与输入滤波需为非负有限数":"";

  const save = async () => {
    if(pending.current||invalid||saveBlocked)return false;
    pending.current=true;
    setSaving(true);
    onSavingChange?.(true);
    setNotice(null);
    try {
      const next = { ...config };
      const warnings = await cameraApi.saveConfig(cam, next);
      if(!mounted.current||current.current.cam!==cam)return true;
      setNotice({ tone: warnings.length ? "warn" : "ok", text: warnings.length ? `已应用，${warnings.length} 项参数相机未接受` : `已保存并应用 · ${clock(Date.now())}` });
      setBaseline(next);
      onSaved?.(next);
      return true;
    } catch (e) {
      if(mounted.current&&current.current.cam===cam)setNotice({ tone: "ng", text: String(e) });
      return false;
    } finally {
      pending.current=false;if(mounted.current&&current.current.cam===cam){setSaving(false);onSavingChange?.(false);}
    }
  };
  const discard = () => {setNotice(null);setConfig(baseline);};
  const saveRef=useRef(save);saveRef.current=save;
  const discardRef=useRef(discard);discardRef.current=discard;
  useImperativeHandle(ref,()=>({save:()=>saveRef.current(),discard:()=>discardRef.current()}),[]);

  const others = group.configs.filter(c => c.id !== config.id);
  const applyAll = async () => {
    if (batching || dirty) return;
    setBatching(true); setBatch(null);
    const items: BatchItem[] = [];
    try {
      const fresh = await cameraApi.rigConfig();
      for (const [j, c] of fresh.entries()) {
        if (c.id === config.id) continue;
        const st = group.statuses.find(s => s.id === c.id);
        if (c.source !== "mvs") { items.push({ id: c.id, name: c.name, tone: "off", text: "跳过：不是海康相机" }); continue; }
        if (!c.serial) { items.push({ id: c.id, name: c.name, tone: "off", text: "跳过：未绑定设备" }); continue; }
        if (st?.released) { items.push({ id: c.id, name: c.name, tone: "warn", text: "跳过：已释放给 MVS，重新连接后再应用" }); continue; }
        try {
          const w = await cameraApi.saveConfig(j, { ...c, ...wiring(baseline) });
          items.push(w.length ? { id: c.id, name: c.name, tone: "warn", text: `已应用，${w.length} 项参数相机未接受`, list: w } : { id: c.id, name: c.name, tone: "ok", text: "已应用" });
        } catch (e) { items.push({ id: c.id, name: c.name, tone: "ng", text: "未能应用：" + errorText(e) }); }
      }
    } catch (e) { items.push({ id: "", name: "相机组", tone: "ng", text: "读取相机组失败：" + errorText(e) }); }
    finally {
      if (mounted.current) { setBatch({ at: Date.now(), items }); setBatching(false); }
      onGroupSaved?.();
    }
  };
  const batchBlocked = dirty ? "当前相机有未保存修改，先保存并应用" : (!others.some(c => c.source === "mvs" && c.serial) ? "没有其他已绑定的海康相机" : "");

  const device = devices.find(d => d.serial === config.serial) ?? (status?.device?.serial === config.serial ? status.device : null);
  const sent = sentImaging.get(config.id);
  const [exposure, setExposure] = useState(sent ? String(sent.exposureUs) : "");
  const [gain, setGain] = useState(sent ? String(sent.gainDb) : "");
  const [imagingBusy, setImagingBusy] = useState(false);
  const [imagingError, setImagingError] = useState("");
  const [, setSentAt] = useState(0);
  const exposureUs = Number(exposure), gainDb = Number(gain);
  const imagingInvalid = exposure === "" || gain === "" ? "填写曝光与增益" : !(exposureUs >= 1 && exposureUs <= 1_000_000) ? "曝光需在 1–1000000 µs 之间" : !(Number.isFinite(gainDb) && gainDb >= 0) ? "增益需为不小于 0 的数" : "";
  const imagingBlocked = status?.released ? "已释放给 MVS，重新连接后再下发" : !status?.ready ? "相机未连接" : "";
  const sendImaging = async () => {
    if (imagingBusy || imagingInvalid || imagingBlocked) return;
    setImagingBusy(true); setImagingError("");
    try {
      await cameraApi.setImaging(cam, config.id, { exposureUs, gainDb });
      sentImaging.set(config.id, { exposureUs, gainDb, at: Date.now() });
      if (mounted.current) setSentAt(Date.now());
    } catch (e) { if (mounted.current) setImagingError(errorText(e)); }
    finally { if (mounted.current) setImagingBusy(false); }
  };

  const saveState = invalid ? <span className="cg-save-state ng" role="alert">{invalid}</span>
    : saveBlocked && dirty ? <span className="cg-save-state warn">{saveBlocked}</span>
    : dirty ? <span className="cg-save-state warn">● 有未保存修改<span className="muted cg-small">离开前会提示保存或放弃</span></span>
    : notice ? <span className={"cg-save-state " + notice.tone}>{notice.tone === "ok" ? <Check size={14} /> : <AlertTriangle size={14} />}{notice.text}</span>
    : <span className="cg-save-state muted">参数与相机一致</span>;

  return <>
    <div className="cg-detail">
      <div className="cg-col">
        <fieldset disabled={saving||locked} className="cg-fields">
          <section className="panel" aria-label="基础信息">
            <div className="cg-panel-head"><h2 className="panel-title">基础信息</h2>{!mvs && <span className="cg-note"><Lock size={12} /> 由离线来源生成，只读</span>}</div>
            <div className="cg-form">
              {label("名称")}
              <input id="cam-name" aria-label="名称" className="input" value={config.name} onChange={(e) => set("name", e.target.value)} disabled={!mvs} />
              {label("相机编号")}
              <span className="val"><span className="mono">{config.id}</span><span className="muted cg-small">配方用它引用相机，建相机时分配，不可改</span></span>
              {label("图像源")}
              <span className="val">{sourceText[config.source]}</span>
              {label("设备视角")}
              <select aria-label="设备视角" className="input" value={config.viewCount} onChange={e => set("viewCount", Number(e.target.value))} disabled={!mvs}>
                <option value={1}>单视角</option>
                <option value={3}>三目（一次触发三幅图）</option>
              </select>
              {config.viewCount === 3 && mvs && <>{label("SDK 拼接布局")}<select className="input" aria-label="SDK 拼接布局" value={config.compositeLayout?.kind ?? ""} onChange={e => set("compositeLayout", e.target.value ? {kind:e.target.value as "horizontal" | "vertical"} : null)}><option value="">请选择实际交付布局</option><option value="horizontal">横向三等分</option><option value="vertical">纵向三等分</option>{config.compositeLayout?.kind === "rects" && <option value="rects">已配置矩形区域</option>}</select></>}
              {config.viewCount === 3 && <span className="hint">每个拍照点可多选图 1／2／3 分别示教，三幅图共用一次设备触发。</span>}
              {config.source === "sampleSet" && <span className="hint">图像来自离线模式所选样本集，相机与视角按样本集生成，不可在此更改。</span>}
              {config.source === "sim" && <span className="hint">合成图相机收到触发后约 180 ms 交付一帧，用于没有硬件时跑通节拍。</span>}
            </div>
          </section>

          {mvs ? <section className="panel" aria-label="绑定设备">
            <div className="cg-panel-head"><h2 className="panel-title">绑定设备</h2>{enumerated && <span className="cg-note">枚举到 {devices.length} 台 · {clock(enumerated)}</span>}</div>
            <div className="cg-form">
              {label("相机设备")}
              <div className="row">
                <select id="cam-serial" aria-label="相机设备" className="input" value={config.serial} onChange={(e) => set("serial", e.target.value)}>
                  <option value="" disabled>请选择设备</option>
                  {config.serial && !devices.some((d) => d.serial === config.serial) && <option value={config.serial}>{config.serial}（未发现）</option>}
                  {devices.map((d) => <option key={d.serial} value={d.serial}>{d.model} · {d.serial} · {d.ip ?? d.transport}</option>)}
                </select>
                <button type="button" className="icon-btn" onClick={refreshDevices} title="重新枚举设备" aria-label="重新枚举设备"><RefreshCw size={16} /></button>
              </div>
              <span className="hint">只列出当前枚举到的设备，不自动推荐；换相机时在此改选，保存并应用后生效。</span>
              {deviceError && <span className="hint ng">{deviceError}</span>}
              {config.serial && enumerated && !devices.some((d) => d.serial === config.serial) && !deviceError && <span className="hint warn"><AlertTriangle size={12} /><span>已绑定的设备当前未发现，检查网线与供电后重新枚举。</span></span>}
              {device && <div className="cg-devinfo full">
                <span>型号</span><span>{device.model}</span><span>序列号</span><span className="mono">{device.serial}</span>
                <span>IP</span><span className="mono">{device.ip ?? "—"}</span><span>传输</span><span>{device.transport}</span>
                {device.userName && <><span>用户名</span><span>{device.userName}</span></>}{status?.sdkVersion && <><span>SDK</span><span className="mono">{status.sdkVersion}</span></>}
              </div>}
            </div>
          </section> : <section className="panel" aria-label="绑定设备">
            <h2 className="panel-title">绑定设备</h2>
            <div className="cg-na"><Link2Off size={16} /><div><strong>不适用：离线图像源没有物理设备</strong>离线期间不枚举、不绑定海康设备；原相机组的绑定关系已保留，退出离线后恢复。</div></div>
          </section>}

          {mvs ? <section className="panel" aria-label="触发接线">
            <div className="cg-panel-head"><h2 className="panel-title">触发接线</h2>{warnings.length > 0 && <span className="cg-badge warn"><AlertTriangle size={12} />{warnings.length} 项相机未接受</span>}<span className="cg-note">保存后由设备确认，未接受项列在右侧“参数生效情况”</span></div>
            <div className="cg-form">
              {label("触发源", "triggerSource")}
              <select id="cam-trigger" aria-label="触发源" className="input" value={config.triggerSource} onChange={(e) => set("triggerSource", e.target.value as CameraConfig["triggerSource"])}>
                <option value="Line0">Line0（机器人位置比较输出）</option>
                <option value="Software">Software（台架调试、模拟节拍）</option>
              </select>
              {rejected("triggerSource")}
              {line0 && <>
                {label("触发沿", "triggerActivation")}
                <select id="cam-activation" aria-label="触发沿" className="input" value={config.triggerActivation} onChange={(e) => set("triggerActivation", e.target.value as CameraConfig["triggerActivation"])}>
                  <option value="RisingEdge">上升沿</option>
                  <option value="FallingEdge">下降沿</option>
                </select>
                {rejected("triggerActivation")}
                {label("输入滤波", "debouncerUs")}
                {num("debouncerUs")}
                <span className="hint">滤掉 Line0 毛刺，避免一次触发出多帧；仅 Line0 触发时有效。</span>
                {rejected("debouncerUs")}
              </>}
              {label("触发延时", "triggerDelayUs")}
              {num("triggerDelayUs")}
              {rejected("triggerDelayUs")}
              {label("Line1 输出", "strobe")}
              <label className="check"><input type="checkbox" checked={config.strobe} onChange={(e) => set("strobe", e.target.checked)} />曝光信号驱动频闪</label>
              {rejected("strobe")}
              {label("Chunk 数据", "chunk")}
              <label className="check"><input type="checkbox" checked={config.chunk} onChange={(e) => set("chunk", e.target.checked)} />帧计数、触发计数、时间戳</label>
              <span className="hint">同步触发核对依赖 Chunk 里的触发计数；关闭后这台相机不能核对。</span>
              {rejected("chunk")}
              {label("开流前触发计数")}
              <span className="cg-unit"><input aria-label="开流前触发计数" className="input mono" type="number" min={0} max={Number.MAX_SAFE_INTEGER} step={1} placeholder="未知" value={config.counterAfterOpen??""} onChange={e=>set("counterAfterOpen",e.target.value===""?null:Number(e.target.value))}/><small>次</small></span>
              <span className="hint">仅填写台架确认的开流前触发计数；未知时留空，不能假定为 0。</span>
              <div className="cg-batch full" aria-label="应用到全部相机">
                <div className="cg-row">
                  <p>把以上触发源、触发沿、滤波、延时、频闪、Chunk 复制到其他已绑定的海康相机并保存应用；开流前触发计数、视角数不复制。</p>
                  <button type="button" className="btn small" onClick={() => void applyAll()} disabled={batching || !!batchBlocked} title={batchBlocked || undefined}><Copy size={13} />{batching ? "应用中…" : batch ? "再次应用到全部相机" : "应用到全部相机"}</button>
                </div>
                {batchBlocked && !locked && <span className="muted cg-small">{batchBlocked}</span>}
                {batch && <>
                  <span className={"cg-state " + (batch.items.every(i => i.tone === "ok" || i.tone === "off") ? "ok" : "warn")}>{batch.items.every(i => i.tone === "ok" || i.tone === "off") ? <Check size={13} /> : <AlertTriangle size={13} />}{clock(batch.at)} 应用到全部相机：{batch.items.filter(i => i.tone === "ok").length} 台已应用{batch.items.some(i => i.tone === "warn" || i.tone === "ng") && `，${batch.items.filter(i => i.tone === "warn" || i.tone === "ng").length} 台需处理`}</span>
                  <ul>{batch.items.map(i => <li key={i.id || i.name}><span className="cg-who">{i.name}<span className="mono">{i.id}</span></span><div><span className={"cg-state " + (i.tone === "ng" ? "fault" : i.tone)}>{i.tone === "ok" ? <Check size={13} /> : i.tone === "off" ? <Minus size={13} /> : <AlertTriangle size={13} />}{i.text}</span>{i.list && <ul>{i.list.map(w => <li key={w}>{w}</li>)}</ul>}</div></li>)}</ul>
                </>}
              </div>
            </div>
          </section> : <section className="panel" aria-label="触发接线">
            <h2 className="panel-title">触发接线</h2>
            <div className="cg-na"><Radio size={16} /><div><strong>不适用：离线图像源按节拍自动出图</strong>不接触发线，没有触发沿、滤波、延时、频闪和 Chunk 设置；触发计数即样本序号。</div></div>
          </section>}

          <section className="panel">
            <details className="cg-adv">
              <summary><ChevronRight size={16} />高级参数（调试）<span className="cg-badge off">{mvs ? "曝光 / 增益 · 不写入配方" : "离线不适用"}</span></summary>
              {mvs ? <div className="cg-adv-body">
                <div className="cg-adv-note"><Info size={15} /><span>曝光和增益<b>归配方管</b>（每个配方、每台相机各一份，在配方工作台“整圈采集”里设置）。这里的值<b>只临时下发到设备用于调试，不写进配方</b>；开始采集或生产时会被配方的值覆盖。设备当前实际值读不回来，下方只显示本次调试下发过的值。</span></div>
                <div className="cg-adv-grid">
                  <label>曝光时间<span className="cg-unit"><input aria-label="调试曝光（µs）" className="input mono" type="number" min={1} step={1} value={exposure} onChange={e => setExposure(e.target.value)} placeholder="例如 8000" /><small>µs</small></span></label>
                  <label>增益<span className="cg-unit"><input aria-label="调试增益（dB）" className="input mono" type="number" min={0} step={0.1} value={gain} onChange={e => setGain(e.target.value)} placeholder="例如 6.0" /><small>dB</small></span></label>
                </div>
                <div className="cg-adv-sent">
                  <button type="button" className="btn small" onClick={() => void sendImaging()} disabled={imagingBusy || !!imagingInvalid || !!imagingBlocked} title={imagingBlocked || imagingInvalid || undefined}><SlidersHorizontal size={13} />{imagingBusy ? "下发中…" : "临时下发到设备"}</button>
                  {sent ? <span>本次调试已下发：曝光 <b>{sent.exposureUs} µs</b> · 增益 <b>{sent.gainDb} dB</b>（{clock(sent.at).slice(0, 5)}）</span> : <span>{imagingBlocked || imagingInvalid}</span>}
                </div>
                {imagingError && <span className="cg-err">{imagingError}</span>}
                <span className="muted cg-small">固定写入：ExposureAuto / GainAuto = Off、PixelFormat = Mono8、TriggerMode = On；GigE 相机自动设置最佳包长。{status?.sdkVersion && ` SDK ${status.sdkVersion}。`}</span>
              </div> : <div className="cg-adv-body"><div className="cg-na"><Database size={16} /><div>样本集与合成图没有曝光和增益可下发。</div></div></div>}
            </details>
          </section>
        </fieldset>
        {footer}
      </div>
      <aside className="cg-side">{side}</aside>
    </div>
    {!locked && <div className="cg-savebar">
      {leading}
      <span className="spacer" />
      {saveState}
      <button className="btn" onClick={discard} disabled={saving||!dirty}>放弃</button>
      <button className="btn primary" onClick={()=>void save()} disabled={saving||!dirty||!!invalid||!!saveBlocked} title={saveBlocked||undefined}>{saving ? "写入中…" : "保存并应用"}</button>
    </div>}
  </>;
}
