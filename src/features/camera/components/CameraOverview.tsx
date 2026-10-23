import { useState, type MouseEvent, type ReactNode } from "react";
import { Activity, ChevronRight, Clock, Lock, Plus, RefreshCw, Unplug } from "lucide-react";
import { Alert, Button, Card, IconButton, StatusBadge } from "qing-steel";
import { usePreviewCanvas } from "../api";
import type { CameraConfig, CameraStatus, Frame } from "../types";
import { ago, cameraColor, cameraHealth, clock, count, sourceText, StateBadge, triggerSummary, useNow } from "./health";

interface Props {
  configs: CameraConfig[];
  statuses: CameraStatus[];
  lastFrame: Record<number, Frame>;
  /** 只读（检测中、离线、浏览器）时的原因；为空可编辑 */
  lockedReason: string;
  canReconnect: boolean;
  loading: boolean;
  adding: boolean;
  error: string;
  onOpen: (i: number) => void;
  onRefresh: () => void;
  onAdd?: () => void;
  onReconnect: (i: number) => Promise<string>;
  footer?: ReactNode;
}

export default function CameraOverview({ configs, statuses, lastFrame, lockedReason, canReconnect, loading, adding, error, onOpen, onRefresh, onAdd, onReconnect, footer }: Props) {
  const now = useNow(1000);
  const status = (i: number) => statuses.find(s => s.cam === i) ?? null;
  const health = configs.map((c, i) => cameraHealth(c, status(i)));
  const tally = (label: string) => health.filter(h => h.label === label).length;
  const connected = tally("已连接"), unbound = tally("未绑定"), down = health.filter(h => h.tone === "fault").length, released = tally("已释放给 MVS");
  return <>
    <div className="cg-toolbar">
      <div className="cg-summary">
        <strong>相机组</strong><span className="mono">{configs.length} / 8</span>
        <StatusBadge tone="ok">{connected} 台已连接</StatusBadge>
        {down > 0 && <StatusBadge tone="fault">{down} 台未连接</StatusBadge>}
        {released > 0 && <StatusBadge tone="warn">{released} 台已释放给 MVS</StatusBadge>}
        {unbound > 0 && <StatusBadge tone="warn">{unbound} 台未绑定</StatusBadge>}
      </div>
      <span className="spacer" />
      <span className="muted cg-small">状态每秒刷新 · {clock(now)}</span>
      <IconButton onClick={onRefresh} disabled={loading} aria-label="刷新相机配置" title="刷新相机配置"><RefreshCw size={16} /></IconButton>
      {onAdd && (lockedReason
        ? <StatusBadge tone="off">{lockedReason}</StatusBadge>
        : <Button leadingIcon={<Plus size={15} />} onClick={onAdd} disabled={adding || loading || configs.length >= 8}>{adding ? "添加中…" : "添加相机"}</Button>)}
    </div>
    {error && <Alert tone="ng">{error}</Alert>}
    <div className="cg-grid">
      {configs.map((c, i) => <CameraCard key={c.id} index={i} config={c} status={status(i)} frame={lastFrame[i]} now={now} locked={!!lockedReason} canReconnect={canReconnect} onOpen={() => onOpen(i)} onReconnect={() => onReconnect(i)} />)}
    </div>
    <Alert>相机按各自设定帧率采集，帧序号配组不代表曝光同步。整轮帧数差异与缺帧请查看采集结果。</Alert>
    {footer}
  </>;
}

const cardClickIgnored = "button, a, input, select, label, .cg-card-sub, .mono, .cg-stat b";

function CameraCard({ index, config, status, frame, now, locked, canReconnect, onOpen, onReconnect }: { index: number; config: CameraConfig; status: CameraStatus | null; frame: Frame | undefined; now: number; locked: boolean; canReconnect: boolean; onOpen: () => void; onReconnect: () => Promise<string> }) {
  const health = cameraHealth(config, status);
  const scope = JSON.stringify([config.id, config.source, config.serial, config.viewCount]);
  const { img, canvas } = usePreviewCanvas(index, frame?.frameCounter, 500, scope, 1);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const reconnect = async () => {
    setBusy(true); setError("");
    try { await onReconnect(); } catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  const head = <div className="cg-card-head"><strong>{config.name}</strong><span className="mono">{config.id}</span>{<StateBadge {...health} />}</div>;
  const openFromCard = (event: MouseEvent<HTMLElement>) => { if (!window.getSelection()?.toString() && !(event.target as Element).closest(cardClickIgnored)) onOpen(); };
  const detail = <Button variant="text" size="sm" leadingIcon={locked ? <Lock size={12} /> : undefined} trailingIcon={<ChevronRight size={14} />} onClick={onOpen}>{locked ? "只读查看" : health.label === "未绑定" ? "进入详情选择设备" : "参数详情"}</Button>;
  if (health.label === "未绑定") return <Card role="article" className="cg-card clickable" style={cameraColor(index)} aria-label={`${config.name} ${config.id}`} onClick={openFromCard}>
    {head}
    <div className="cg-card-sub">{sourceText[config.source]} · 尚未绑定物理设备</div>
    <Alert tone="warn" action={detail}>未绑定设备</Alert>
    <div className="cg-card-foot">绑定后可查看采集状态</div>
  </Card>;
  const age = frame ? now - frame.ts : null;
  const fault = health.tone === "fault";
  const device = status?.device;
  return <Card role="article" className="cg-card clickable" style={cameraColor(index)} aria-label={`${config.name} ${config.id}`} onClick={openFromCard}>
    {head}
    <div className="cg-card-sub">{device ? <>{device.model} · <span className="mono">{device.serial}</span>{device.ip && <> · <span className="mono">{device.ip}</span></>} · {device.transport}</> : config.source === "mvs" ? <>序列号 <span className="mono">{config.serial}</span></> : sourceText[config.source]}</div>
    <div className={"cg-thumb" + (fault || status?.released ? " dim" : age != null && age < 1500 ? " fresh" : "")}>
      <canvas aria-label={`${config.name} 最新图像`} role="img" ref={canvas} style={{ display: img ? "block" : "none" }} />
      {!img && !fault && <span className="cg-thumb-empty">{status?.released ? "已释放给 MVS，重新连接后出图" : "等待第一帧"}</span>}
      {img && !fault && age != null && <span className={"cg-tag " + (age < 3000 ? "fresh" : "stale")}>{age < 3000 ? <Activity size={11} /> : <Clock size={11} />}{ago(age)}</span>}
      {img && <span className="cg-tag size">{img.fullWidth}×{img.fullHeight}</span>}
      {img && config.viewCount === 3 && <span className="cg-tag views">三目 · 视角 1/3</span>}
      {fault && <div className="cg-thumb-fault"><strong><Unplug size={14} />{status?.message || "相机未连接"}</strong>{frame && <span>最后一帧 {clock(frame.ts)} · 后台每 2 秒自动重连</span>}</div>}
    </div>
    <div className="cg-stats">
      <div className="cg-stat"><span>帧计数</span><b>{count(frame?.frameCounter)}</b></div>
      <div className="cg-stat"><span>触发计数</span><b>{count(frame?.triggerCounter)}</b></div>
      <div className={"cg-stat" + (fault && (status?.lostPackets ?? 0) > 0 ? " fault" : "")}><span>丢包</span><b>{count(status?.lostPackets)}</b></div>
      <div className={"cg-stat" + (fault ? " fault" : "")}><span>最近出图</span><b className="text">{age == null ? "—" : ago(age)}</b></div>
    </div>
    {fault ? <>
      <Alert tone="fault" action={<>{canReconnect && <Button size="sm" leadingIcon={<RefreshCw size={13} />} onClick={() => void reconnect()} disabled={busy}>{busy ? "连接中…" : "立即重新连接"}</Button>}{detail}</>}>依次检查：网线与交换机端口灯 → 供电 / PoE → IP 是否冲突 → 详情里“重新枚举”确认设备重新出现。</Alert>
      {error && <Alert tone="ng">{error}</Alert>}
    </> : <div className="cg-card-foot"><span>{triggerSummary(config)}</span>{status?.ready && <><span>·</span><span className="mono">实际 {status.fps.toFixed(1)} fps</span></>}{detail}</div>}
  </Card>;
}
