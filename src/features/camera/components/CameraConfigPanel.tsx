import { useEffect, useImperativeHandle, useRef, useState, type ReactNode, type Ref } from "react";
import { AlertTriangle, Copy, Lock, RefreshCw, SlidersHorizontal } from "lucide-react";
import { Accordion, Alert, Button, Card, Checkbox, FormField, Heading, IconButton, NumberField, Select, Stack, StatusBadge, Surface, TextField } from "qing-steel";
import { cameraApi } from "../api";
import type { BandwidthEstimate, CameraConfig, CameraStatus, DeviceSummary } from "../types";
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

type Wiring = Pick<CameraConfig, "triggerSource" | "triggerActivation" | "frameRate" | "debouncerUs" | "triggerDelayUs" | "strobe" | "chunk">;
const wiring = (c: CameraConfig): Wiring => ({ triggerSource: c.triggerSource, triggerActivation: c.triggerActivation, frameRate: c.frameRate, debouncerUs: c.debouncerUs, triggerDelayUs: c.triggerDelayUs, strobe: c.strobe, chunk: c.chunk });

interface BatchItem { id: string; name: string; tone: "ok" | "warn" | "ng" | "off"; text: string; list?: string[] }

/** 相机未接受的参数按节点名对到表单字段，方便就地标注；完整列表仍在右侧。 */
const nodes: [keyof CameraConfig, RegExp][] = [
  ["triggerDelayUs", /TriggerDelay/],
  ["debouncerUs", /Debouncer/],
  ["triggerActivation", /TriggerActivation/],
  ["frameRate", /FrameRate/],
  ["triggerSource", /TriggerSource|TriggerMode/],
  ["strobe", /Line(Selector|Mode|Source|Inverter)|Strobe/],
  ["chunk", /Chunk/],
];

const isEstimate = (b: unknown): b is BandwidthEstimate =>
  typeof b === "object" && b !== null && typeof (b as BandwidthEstimate).mbpsTotal === "number" && Array.isArray((b as BandwidthEstimate).perCamera);

function useBandwidth(hz: number) {
  const [estimate, setEstimate] = useState<BandwidthEstimate | null>(null);
  useEffect(() => {
    let alive = true;
    const load = () => cameraApi.bandwidth(hz).then(b => { if (alive) setEstimate(isEstimate(b) ? b : null); }).catch(() => { if (alive) setEstimate(null); });
    load();
    const timer = setInterval(load, 2000);
    return () => { alive = false; clearInterval(timer); };
  }, [hz]);
  return estimate;
}

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
  const label = (text: string, key?: keyof CameraConfig) => <span>{text}{key && fieldWarnings(key).length > 0 && <AlertTriangle size={13} aria-label="相机未接受" />}</span>;
  const note = (hint?: ReactNode, key?: keyof CameraConfig) => {
    const items = key ? fieldWarnings(key) : [];
    return hint || items.length ? <>{hint}{items.map(w => <span key={w} className="cg-rejected">相机未接受：{w}。相机保持写入前的值。</span>)}</> : undefined;
  };
  const numberLabels={triggerDelayUs:"触发延时（µs）",debouncerUs:"输入滤波（µs）"};
  const num = (key: keyof typeof numberLabels, text: string, hint?: ReactNode) => (
    <NumberField id={`cam-${key}`} label={label(text, key)} aria-label={numberLabels[key]} unit="µs" step={1} value={Number.isFinite(config[key])?config[key]:""} onValueChange={v => set(key, v===""?NaN:v)} description={note(hint, key)} />
  );
  const mvs = config.source === "mvs";
  const line0 = config.triggerSource === "Line0";
  const invalid=!Number.isFinite(config.frameRate)||config.frameRate<=0?"帧率必须为正数":
    !config.name.trim()?"相机名称不能为空":
    ![1,3].includes(config.viewCount)?"设备视角数只能为 1 或 3":
    mvs&&!config.serial?"请选择要绑定的相机设备":
    mvs&&config.counterAfterOpen!=null&&(!Number.isSafeInteger(config.counterAfterOpen)||config.counterAfterOpen<0)?"开流前触发计数需为非负安全整数，未知时请留空":
    ![config.triggerDelayUs,config.debouncerUs].every(v=>Number.isFinite(v)&&v>=0)?"触发延时与输入滤波需为非负有限数":"";

  const save = async () => {
    if(pending.current||locked||invalid||saveBlocked)return false;
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
    if (batching || dirty || locked || saveBlocked) return;
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
  const rates = group.configs.filter(c=>c.source==="mvs"&&c.triggerSource==="Line0").map(c=>c.frameRate);
  const bandwidthHz = rates.length ? Math.max(...rates) : 10;
  const bandwidth = useBandwidth(bandwidthHz);
  const unestimated = bandwidth ? group.configs.filter((_, i) => bandwidth.perCamera[i] == null).map(c => c.name) : [];

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

  const batchOk = batch?.items.every(i => i.tone === "ok" || i.tone === "off");
  const saveState = invalid ? <StatusBadge tone="ng" role="alert">{invalid}</StatusBadge>
    : saveBlocked && dirty ? <StatusBadge tone="warn">{saveBlocked}</StatusBadge>
    : dirty ? <Stack gap={8}><StatusBadge tone="warn">有未保存修改</StatusBadge><span className="muted cg-small">离开前会提示保存或放弃</span></Stack>
    : notice ? <StatusBadge tone={notice.tone}>{notice.text}</StatusBadge>
    : <span className="muted">参数与相机一致</span>;

  return <>
    <div className="cg-detail">
      <div className="cg-col">
        <fieldset disabled={saving||locked} className="cg-fields">
          <Card role="region" aria-label="基础信息" title={<Heading level={3}>基础信息</Heading>} actions={!mvs && <span className="cg-note"><Lock size={12} /> 由离线来源生成，只读</span>}>
            <Stack direction="column" align="stretch" gap={12}>
              <TextField id="cam-name" label="名称" value={config.name} onChange={(e) => set("name", e.target.value)} disabled={!mvs} />
              <TextField label="相机编号" readOnly value={config.id} description="配方用它引用相机，建相机时分配，不可改" />
              <TextField label="图像源" readOnly value={sourceText[config.source]} />
              <TextField label="设备视角" readOnly value="单视角" />
              {config.source === "sampleSet" && <span className="cg-small muted">图像来自离线模式所选样本集，相机与视角按样本集生成，不可在此更改。</span>}
              {config.source === "sim" && <span className="cg-small muted">合成图相机收到触发后约 180 ms 交付一帧，用于没有硬件时跑通节拍。</span>}
            </Stack>
          </Card>

          {mvs ? <Card role="region" aria-label="绑定设备" title={<Heading level={3}>绑定设备</Heading>} actions={enumerated && <span className="cg-note">枚举到 {devices.length} 台 · {clock(enumerated)}</span>}>
            <Stack direction="column" align="stretch" gap={12}>
              <FormField id="cam-serial" label="相机设备" error={deviceError || undefined} description="只列出当前枚举到的设备，不自动推荐；换相机时在此改选，保存并应用后生效。">
                <Stack wrap={false} className="cg-serial">
                  <Select id="cam-serial" aria-label="相机设备" value={config.serial} placeholder="请选择设备" onValueChange={v => set("serial", v)}
                    options={[
                      ...(config.serial && !devices.some((d) => d.serial === config.serial) ? [{ value: config.serial, label: `${config.serial}（未发现）` }] : []),
                      ...devices.map((d) => ({ value: d.serial, label: `${d.model} · ${d.serial} · ${d.ip ?? d.transport}` })),
                    ]} />
                  <IconButton onClick={refreshDevices} title="重新枚举设备" aria-label="重新枚举设备"><RefreshCw size={16} /></IconButton>
                </Stack>
              </FormField>
              {config.serial && enumerated && !devices.some((d) => d.serial === config.serial) && !deviceError && <Alert tone="warn">已绑定的设备当前未发现，检查网线与供电后重新枚举。</Alert>}
              {device && <Surface level="mid" className="cg-devinfo">
                <span>型号</span><span>{device.model}</span><span>序列号</span><span className="mono">{device.serial}</span>
                <span>IP</span><span className="mono">{device.ip ?? "—"}</span><span>传输</span><span>{device.transport}</span>
                {device.userName && <><span>用户名</span><span>{device.userName}</span></>}{status?.sdkVersion && <><span>SDK</span><span className="mono">{status.sdkVersion}</span></>}
              </Surface>}
            </Stack>
          </Card> : <Card role="region" aria-label="绑定设备" title={<Heading level={3}>绑定设备</Heading>}>
            <Alert title="不适用：离线图像源没有物理设备">离线期间不枚举、不绑定海康设备；原相机组的绑定关系已保留，退出离线后恢复。</Alert>
          </Card>}

          {mvs && <Card role="region" aria-label="链路带宽" title={<Heading level={3}>链路带宽</Heading>} actions={<span className="cg-note">按组内最高设定帧率 {bandwidthHz} fps 估算</span>}>
            {bandwidth ? <Stack direction="column" align="stretch" gap={8}>
              <span>相机组 {group.configs.length} 台同时出图合计 <b>{Math.round(bandwidth.mbpsTotal)} Mbps</b>，单个千兆口按 80% 计 {Math.round(bandwidth.budgetMbps)} Mbps。</span>
              {unestimated.length > 0 && <span className="muted cg-small">{unestimated.join("、")} 还没出过图，分辨率未知，未计入合计。</span>}
              {bandwidth.overBudget && <Alert tone="warn" title="带宽超预算">各相机连续出图，合计超过单个千兆口的 80%，可能丢帧；降低设定帧率、缩小图像区域或分口接入。</Alert>}
            </Stack> : <span className="muted">带宽估算不可用。</span>}
          </Card>}

          {mvs ? <Card role="region" aria-label="触发接线" title={<Heading level={3}>触发接线</Heading>} actions={<>{warnings.length > 0 && <StatusBadge tone="warn">{warnings.length} 项相机未接受</StatusBadge>}<span className="cg-note">保存后由设备确认，未接受项列在右侧“参数生效情况”</span></>}>
            <Stack direction="column" align="stretch" gap={12}>
              <Select id="cam-trigger" label={label("触发源", "triggerSource")} aria-label="触发源" value={config.triggerSource} onValueChange={v => { set("triggerSource", v as CameraConfig["triggerSource"]); if(v==="Line0")set("triggerActivation","LevelHigh"); }}
                options={[{ value: "Line0", label: "Line0（高电平连续采集）" }, { value: "Software", label: "Software（标定、调试单帧）" }]} description={note(undefined, "triggerSource")} />
              {line0 && <>
                <TextField label="触发方式" readOnly value="高电平（LevelHigh）" description="Line0 为高电平时按相机帧率连续采集，低电平停止新曝光。" />
                <NumberField label={label("设定帧率", "frameRate")} aria-label="设定帧率" unit="fps" min={0.01} step={1} value={Number.isFinite(config.frameRate)?config.frameRate:""} onValueChange={v=>set("frameRate",v===""?NaN:v)} description={note("参与同轮采集的相机需设置相同帧率。", "frameRate")} />
                {num("debouncerUs", "输入滤波", "过滤 Line0 电平毛刺。")}
                {num("triggerDelayUs", "触发延时")}
              </>}
              <Stack direction="column" align="stretch" gap={4}>
                <span className="cg-group-label">{label("Line1 输出", "strobe")}</span>
                <Checkbox label="曝光信号驱动频闪" checked={config.strobe} onCheckedChange={v => set("strobe", v)} />
                {note(undefined, "strobe") && <small className="muted">{note(undefined, "strobe")}</small>}
              </Stack>
              <Stack direction="column" align="stretch" gap={4}>
                <span className="cg-group-label">{label("Chunk 数据", "chunk")}</span>
                <Checkbox label="帧计数、触发计数、时间戳" checked={config.chunk} onCheckedChange={v => set("chunk", v)} />
                <small className="muted">帧序号用于采集诊断，不代表各相机曝光同步。{note(undefined, "chunk")}</small>
              </Stack>
              <Surface level="mid" className="cg-batch" aria-label="应用到全部相机">
                <div className="cg-row">
                  <p>把以上触发源、帧率、高电平、滤波、延时、频闪、Chunk 复制到其他已绑定的海康相机并保存应用。</p>
                  <Button size="sm" leadingIcon={<Copy size={13} />} onClick={() => void applyAll()} disabled={batching || !!batchBlocked} title={batchBlocked || undefined}>{batching ? "应用中…" : batch ? "再次应用到全部相机" : "应用到全部相机"}</Button>
                </div>
                {batchBlocked && !locked && <span className="muted cg-small">{batchBlocked}</span>}
                {batch && <>
                  <StatusBadge tone={batchOk ? "ok" : "warn"}>{clock(batch.at)} 应用到全部相机：{batch.items.filter(i => i.tone === "ok").length} 台已应用{batch.items.some(i => i.tone === "warn" || i.tone === "ng") && `，${batch.items.filter(i => i.tone === "warn" || i.tone === "ng").length} 台需处理`}</StatusBadge>
                  <ul>{batch.items.map(i => <li key={i.id || i.name}><span className="cg-who">{i.name}<span className="mono">{i.id}</span></span><div><StatusBadge tone={i.tone === "ng" ? "fault" : i.tone}>{i.text}</StatusBadge>{i.list && <ul>{i.list.map(w => <li key={w}>{w}</li>)}</ul>}</div></li>)}</ul>
                </>}
              </Surface>
            </Stack>
          </Card> : <Card role="region" aria-label="触发接线" title={<Heading level={3}>触发接线</Heading>}>
            <Alert title="不适用：离线图像源按节拍自动出图">不接触发线，没有触发沿、滤波、延时、频闪和 Chunk 设置；触发计数即样本序号。</Alert>
          </Card>}

          <Card>
            <Accordion collapsible items={[{
              value: "debug",
              title: <>高级参数（调试）<StatusBadge tone="off">{mvs ? "曝光 / 增益 · 不写入配方" : "离线不适用"}</StatusBadge></>,
              content: mvs ? <Stack direction="column" align="stretch" gap={12}>
                <Alert>曝光和增益<b>归配方管</b>（每个配方、每台相机各一份，在配方工作台“整圈采集”里设置）。这里的值<b>只临时下发到设备用于调试，不写进配方</b>；开始采集或生产时会被配方的值覆盖。设备当前实际值读不回来，下方只显示本次调试下发过的值。</Alert>
                <div className="cg-adv-grid">
                  <NumberField label="曝光时间" aria-label="调试曝光（µs）" unit="µs" min={1} step={1} value={exposure===""?"":Number(exposure)} onValueChange={v => setExposure(v===""?"":String(v))} placeholder="例如 8000" />
                  <NumberField label="增益" aria-label="调试增益（dB）" unit="dB" min={0} step={0.1} value={gain===""?"":Number(gain)} onValueChange={v => setGain(v===""?"":String(v))} placeholder="例如 6.0" />
                </div>
                <div className="cg-adv-sent">
                  <Button size="sm" leadingIcon={<SlidersHorizontal size={13} />} onClick={() => void sendImaging()} disabled={imagingBusy || !!imagingInvalid || !!imagingBlocked} title={imagingBlocked || imagingInvalid || undefined}>{imagingBusy ? "下发中…" : "临时下发到设备"}</Button>
                  {sent ? <span>本次调试已下发：曝光 <b>{sent.exposureUs} µs</b> · 增益 <b>{sent.gainDb} dB</b>（{clock(sent.at).slice(0, 5)}）</span> : <span>{imagingBlocked || imagingInvalid}</span>}
                </div>
                {imagingError && <Alert tone="ng">{imagingError}</Alert>}
                <span className="muted cg-small">固定写入：ExposureAuto / GainAuto = Off、PixelFormat = Mono8、TriggerMode = On；GigE 相机自动设置最佳包长。{status?.sdkVersion && ` SDK ${status.sdkVersion}。`}</span>
              </Stack> : <Alert>样本集与合成图没有曝光和增益可下发。</Alert>,
            }]} />
          </Card>
        </fieldset>
        {footer}
      </div>
      <aside className="cg-side">{side}</aside>
    </div>
    {!locked && <div className="cg-savebar">
      {leading}
      <span className="spacer" />
      {saveState}
      <Button onClick={discard} disabled={saving||!dirty}>放弃</Button>
      <Button variant="filled" onClick={()=>void save()} disabled={saving||!dirty||!!invalid||!!saveBlocked} title={saveBlocked||undefined}>{saving ? "写入中…" : "保存并应用"}</Button>
    </div>}
  </>;
}
