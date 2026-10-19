import { useState, type MouseEvent, type ReactNode } from "react";
import { Activity, ChevronRight, Clock, Lock, Plus, RefreshCw, Unplug } from "lucide-react";
import { Alert, Button, Card, Heading, IconButton, StatusBadge, Surface, Table, type TableColumn } from "qing-steel";
import { usePreviewCanvas } from "../api";
import { SYNC_WINDOW_MS, type SyncMember, type SyncRow, type SyncState } from "../sync";
import type { CameraConfig, CameraStatus, Frame } from "../types";
import { ago, cameraColor, cameraHealth, clock, count, sourceText, StateBadge, triggerSummary, useNow } from "./health";

interface Props {
  configs: CameraConfig[];
  statuses: CameraStatus[];
  lastFrame: Record<number, Frame>;
  members: SyncMember[];
  sync: SyncState;
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

export default function CameraOverview({ configs, statuses, lastFrame, members, sync, lockedReason, canReconnect, loading, adding, error, onOpen, onRefresh, onAdd, onReconnect, footer }: Props) {
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
        {sync.last?.ok === false && <StatusBadge tone="warn">触发不同步</StatusBadge>}
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
      {configs.map((c, i) => <CameraCard key={c.id} index={i} config={c} status={status(i)} frame={lastFrame[i]} now={now} lagging={sync.last?.rows.some(r => r.cam === i && (r.result === "lag" || r.result === "jump")) ?? false} locked={!!lockedReason} canReconnect={canReconnect} onOpen={() => onOpen(i)} onReconnect={() => onReconnect(i)} />)}
    </div>
    {configs.length > 1 && <SyncPanel configs={configs} members={members} sync={sync} onOpen={onOpen} />}
    {footer}
  </>;
}

const cardClickIgnored = "button, a, input, select, label, .cg-card-sub, .mono, .cg-stat b";

function CameraCard({ index, config, status, frame, now, lagging, locked, canReconnect, onOpen, onReconnect }: { index: number; config: CameraConfig; status: CameraStatus | null; frame: Frame | undefined; now: number; lagging: boolean; locked: boolean; canReconnect: boolean; onOpen: () => void; onReconnect: () => Promise<string> }) {
  const health = cameraHealth(config, status);
  const scope = JSON.stringify([config.id, config.source, config.serial, config.viewCount]);
  const { img, canvas } = usePreviewCanvas(index, frame?.frameCounter, 500, scope, 1);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const reconnect = async () => {
    setBusy(true); setError("");
    try { await onReconnect(); } catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  const head = <div className="cg-card-head"><strong>{config.name}</strong><span className="mono">{config.id}</span>{lagging && health.tone === "ok" ? <StatusBadge tone="warn">触发落后</StatusBadge> : <StateBadge {...health} />}</div>;
  const openFromCard = (event: MouseEvent<HTMLElement>) => { if (!window.getSelection()?.toString() && !(event.target as Element).closest(cardClickIgnored)) onOpen(); };
  const detail = <Button variant="text" size="sm" leadingIcon={locked ? <Lock size={12} /> : undefined} trailingIcon={<ChevronRight size={14} />} onClick={onOpen}>{locked ? "只读查看" : health.label === "未绑定" ? "进入详情选择设备" : "参数详情"}</Button>;
  if (health.label === "未绑定") return <Card role="article" className="cg-card clickable" style={cameraColor(index)} aria-label={`${config.name} ${config.id}`} onClick={openFromCard}>
    {head}
    <div className="cg-card-sub">{sourceText[config.source]} · 尚未绑定物理设备</div>
    <Alert tone="warn" action={detail}>未绑定设备</Alert>
    <div className="cg-card-foot">不参与同步核对</div>
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
      <div className={"cg-stat" + (lagging ? " warn" : "")}><span>触发计数</span><b>{count(frame?.triggerCounter)}</b></div>
      <div className={"cg-stat" + (fault && (status?.lostPackets ?? 0) > 0 ? " fault" : "")}><span>丢包</span><b>{count(status?.lostPackets)}</b></div>
      <div className={"cg-stat" + (fault ? " fault" : lagging ? " warn" : "")}><span>最近出图</span><b className="text">{age == null ? "—" : ago(age)}</b></div>
    </div>
    {fault ? <>
      <Alert tone="fault" action={<>{canReconnect && <Button size="sm" leadingIcon={<RefreshCw size={13} />} onClick={() => void reconnect()} disabled={busy}>{busy ? "连接中…" : "立即重新连接"}</Button>}{detail}</>}>依次检查：网线与交换机端口灯 → 供电 / PoE → IP 是否冲突 → 详情里“重新枚举”确认设备重新出现。</Alert>
      {error && <Alert tone="ng">{error}</Alert>}
    </> : <div className="cg-card-foot"><span>{triggerSummary(config)}</span>{status?.ready && <><span>·</span><span className="mono">{status.fps.toFixed(1)} fps</span></>}{lagging ? <StatusBadge tone="warn">最近一次触发未同步</StatusBadge> : null}{detail}</div>}
  </Card>;
}

const resultView: Record<SyncRow["result"], { tone: "ok" | "warn" | "off"; text: string }> = {
  sync: { tone: "ok", text: "同步" },
  lag: { tone: "warn", text: "落后" },
  jump: { tone: "warn", text: "计数跳变" },
  pending: { tone: "off", text: "" },
  excluded: { tone: "off", text: "" },
};

function SyncPanel({ configs, members, sync, onOpen }: { configs: CameraConfig[]; members: SyncMember[]; sync: SyncState; onOpen: (i: number) => void }) {
  const participants = members.filter(m => !m.excluded);
  const down = members.filter(m => m.excluded === "未连接").map(m => configs[m.cam]?.id);
  const paused = participants.length < 2;
  const last = sync.last;
  const bad = last?.rows.filter(r => r.result === "lag" || r.result === "jump") ?? [];
  const badge = paused
    ? <StatusBadge tone={down.length ? "fault" : "off"}>已暂停：{down.length ? `${down.join("、")} 未连接，` : ""}参与核对的相机不足 2 台</StatusBadge>
    : !last || last.ok === null ? <StatusBadge tone="off">等待下一次触发</StatusBadge>
    : last.ok ? <StatusBadge tone="ok">最近一次触发：全部同步</StatusBadge>
    : <StatusBadge tone="warn">最近一次触发：{bad.map(r => configs[r.cam]?.id).join("、")} {bad.some(r => r.result === "lag") ? "落后" : "计数跳变"}</StatusBadge>;
  const row = (i: number) => {
    const member = members.find(m => m.cam === i);
    const r = last?.rows.find(x => x.cam === i);
    if (member?.excluded) return { before: null, after: null, delta: null, result: "excluded" as const, reason: member.excluded + "，已排除" };
    return r ?? { before: null, after: null, delta: null, result: "pending" as const, reason: "等待触发" };
  };
  const first = bad[0];
  const columns: TableColumn<number>[] = [
    { key: "camera", header: "相机", render: i => {
      const r = row(i);
      return <span className="cg-who"><span className={"dot" + (r.result === "excluded" ? " off" : "")} style={r.result === "excluded" ? undefined : { background: `var(--cam-${(i % 8) + 1})` }} />{configs[i].name}<span className="mono">{configs[i].id}</span></span>;
    } },
    { key: "before", header: "触发前", align: "right", render: i => count(row(i).before) },
    { key: "after", header: "触发后", align: "right", render: i => count(row(i).after) },
    { key: "delta", header: "变化", align: "right", render: i => { const delta = row(i).delta; return delta == null ? "—" : `+${delta}`; } },
    { key: "result", header: "结果", render: i => {
      const r = row(i), view = resultView[r.result], misses = sync.misses[i] ?? 0;
      return <StatusBadge tone={view.tone}>{view.text || r.reason}{(r.result === "lag" || r.result === "jump") && ` · ${r.reason}`}{misses > 0 && r.result !== "excluded" && ` · 已累计 ${misses} 次`}</StatusBadge>;
    } },
  ];
  return <Card role="region" aria-label="同步触发核对">
    <div className="cg-sync-head">
      <Heading level={3}>同步触发核对</Heading>
      {badge}
      <span className="cg-sync-meta"><span>自打开本页：触发 <b>{sync.triggers}</b> 次</span><span>不同步 <b>{sync.mismatches}</b> 次</span>{last && <span>最近一次 <b>{clock(last.at, true)}</b></span>}</span>
      <span className="spacer" />
      <span className="muted cg-small">按各相机 Chunk 触发计数前后值比对，窗口 {SYNC_WINDOW_MS} ms</span>
    </div>
    <div className="cg-sync-grid">
      <Table<number> caption="同步触发核对结果" rows={configs.map((_, i) => i)} columns={columns} rowKey={i => configs[i].id} />
      {first ? <Surface level="mid" className="cg-hint">
        <strong className="warn">{configs[first.cam]?.id} {first.result === "lag" ? "没有收到这次触发" : "计数跳变，期间有触发没有出图"}，按顺序排查</strong>
        <ol>
          <li><b>触发线</b>：Line0 是否接到与其他相机同一路机器人位置比较输出，接头是否松动。</li>
          <li><b>输入滤波</b>：滤波时间过长会把较窄的触发脉冲滤掉，与其他相机对比后适当调小。</li>
          <li><b>网线 / 丢包</b>：丢包持续上升时检查网线与交换机端口。</li>
          <li><b>触发源</b>：确认触发源、触发沿与其他相机一致，可在详情里“应用到全部相机”统一。</li>
        </ol>
        <div className="cg-row"><Button size="sm" trailingIcon={<ChevronRight size={13} />} onClick={() => onOpen(first.cam)}>查看 {configs[first.cam]?.id} 触发接线</Button><span className="muted cg-small">核对在下一次触发时自动继续</span></div>
      </Surface> : <Surface level="mid" className="cg-hint">
        <strong>怎么看</strong>
        <ol>
          <li>PLC / 机器人每发一次触发，参与核对的相机<b>触发计数都应加 1</b>；某台 +0 即落后。</li>
          <li>触发计数来自相机 Chunk 数据；<b>未绑定、未连接、软触发或关闭 Chunk 的相机不参与核对</b>，表中写明原因。</li>
          <li>核对只看同步性，不判断图像内容；画面是否正确在配方工作台的整圈采集里确认。</li>
        </ol>
      </Surface>}
    </div>
  </Card>;
}
