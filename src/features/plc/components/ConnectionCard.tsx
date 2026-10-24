import { ChevronUp, Pencil } from "lucide-react";
import { Alert, Button, Card, Checkbox, Heading, NumberField, Select, Stack, Text, Tooltip } from "qing-steel";
import ConnectionForm from "./ConnectionForm";
import { cpuPresetLabel } from "../meta";
import type { PlcConfig } from "../types";

const fieldsetStyle = { border: 0, padding: 0, margin: 0, minWidth: 0 };
const hex = (n: number | null) => n === null ? "自动" : Number.isFinite(n) ? `0x${n.toString(16).toUpperCase().padStart(4, "0")}` : "无效";
const num = (n: number) => Number.isFinite(n) ? String(n) : "未填";

type SummaryItem = { key: string; label: string; value: string; detail?: string };

function summaryLines(config: PlcConfig): SummaryItem[][] {
  const c = config.connection;
  const heartbeatPoint = config.points.find(p => p.id === config.heartbeat.pointId);
  return [[
    { key: "cpu", label: "CPU 型号", value: cpuPresetLabel(c) },
    { key: "host", label: "PLC 地址", value: `${c.host || "未填地址"}:${num(c.port)}` },
    { key: "rackSlot", label: "Rack / Slot", value: `${num(c.s7.rack)} / ${num(c.s7.slot)}` },
    { key: "type", label: "连接类型", value: c.s7.connectionType === "basic" ? "S7 Basic" : c.s7.connectionType.toUpperCase() },
    { key: "tsap", label: "TSAP（本地 / 远端）", value: `${hex(c.s7.localTsap)} / ${hex(c.s7.remoteTsap)}` },
    { key: "pdu", label: "PDU", value: num(c.s7.pduSize) },
  ], [
    { key: "poll", label: "轮询周期", value: `${num(c.pollIntervalMs)} ms` },
    { key: "timeout", label: "通讯超时", value: `${num(c.timeoutMs)} ms` },
    { key: "reconnect", label: "重连间隔", value: `${num(c.reconnectIntervalMs)} ms` },
    { key: "heartbeat", label: "心跳", value: !config.heartbeat.pointId ? "未启用" : heartbeatPoint?.name || config.heartbeat.pointId,
      detail: config.heartbeat.pointId ? `${heartbeatPoint ? `${heartbeatPoint.address} · ` : ""}${num(config.heartbeat.intervalMs)} ms` : undefined },
    { key: "log", label: "日志保留", value: config.logRetentionDays === 0 ? "永久" : `${num(config.logRetentionDays)} 天` },
    { key: "auto", label: "启动自动连接", value: config.autoConnect ? "开启" : "关闭" },
  ]];
}

interface ConnectionCardProps {
  draft: PlcConfig;
  saved: PlcConfig | null;
  onChange: (draft: PlcConfig) => void;
  cpuPreset: { revision: number; key: string };
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 非空时禁止展开编辑，作为按钮 title。 */
  editBlockedReason: string;
  /** 非空时编辑态的表单锁定（生产锁）。 */
  blockedReason: string;
  connected: boolean;
  reconnectSaved: boolean;
}

export default function ConnectionCard({ draft, saved, onChange, cpuPreset, open, onOpenChange, editBlockedReason, blockedReason, connected, reconnectSaved }: ConnectionCardProps) {
  const summaryText = (item: SummaryItem) => [item.value, item.detail].filter(Boolean).join(" · ");
  const savedText = new Map(saved ? summaryLines(saved).flat().map(item => [item.key, summaryText(item)]) : []);
  const writablePoints = draft.points.filter(p => p.access === "readWrite");
  return <Card role="region" aria-label="连接配置" title={<Heading level={3}>连接配置</Heading>}
    actions={open
      ? <Button variant="tonal" leadingIcon={<ChevronUp size={16} />} onClick={() => onOpenChange(false)}>收起</Button>
      : <Button variant="tonal" leadingIcon={<Pencil size={16} />} disabled={!!editBlockedReason} title={editBlockedReason || undefined}
        onClick={() => onOpenChange(true)}>编辑</Button>}>
    <Stack direction="column" align="stretch" gap={14} wrap={false}>
      <div className="plc-conn-summary">
        {summaryLines(draft).map((line, i) => <dl className="plc-summary-grid" key={i}>{line.map(item => {
          const before = savedText.get(item.key);
          const changed = before !== undefined && before !== summaryText(item);
          return <Tooltip key={item.key} content={<div className="plc-signal-details">
            <span>{item.label}：{summaryText(item)}</span>
            {changed && <span>已保存：{before}</span>}
          </div>}>
            <div className={`plc-summary-item${changed ? " plc-summary-item-changed" : ""}`} tabIndex={0}>
              <dt>{item.label}{changed && <span className="plc-summary-changed">已修改</span>}</dt>
              <dd>{item.value}</dd>
            </div>
          </Tooltip>;
        })}</dl>)}
      </div>
      {reconnectSaved && <Text as="p" variant="label" muted>重连使用后端已保存的配置，当前草稿不会保存或应用。</Text>}
      {open && <>
        {connected && <Alert tone="info">已连接时保存，需断开重连后新参数才生效</Alert>}
        <fieldset disabled={!!blockedReason} style={fieldsetStyle}>
          <Stack direction="column" align="stretch" gap={14} wrap={false}>
            <ConnectionForm key={cpuPreset.revision} initialCpuPreset={cpuPreset.key} value={draft.connection} onChange={(connection) => onChange({ ...draft, connection })} />
            <section className="plc-config-card" aria-labelledby="plc-monitor-title">
              <h4 id="plc-monitor-title">心跳与日志</h4>
              <div className="plc-config-fields">
                <Select label="心跳点位" value={draft.heartbeat.pointId ?? ""}
                  onValueChange={(v) => onChange({ ...draft, heartbeat: { ...draft.heartbeat, pointId: v || null } })}
                  options={[{ value: "", label: "不启用" }, ...writablePoints.map((p) => ({ value: p.id, label: `${p.name}（${p.address}）` }))]} />
                <NumberField label="心跳周期 (ms)" min={100} step={1} disabled={!draft.heartbeat.pointId}
                  value={Number.isFinite(draft.heartbeat.intervalMs) ? draft.heartbeat.intervalMs : ""}
                  onValueChange={(v) => onChange({ ...draft, heartbeat: { ...draft.heartbeat, intervalMs: v === "" ? NaN : v } })} />
                <NumberField label="日志保留天数（0 为永久）" min={0} step={1}
                  value={Number.isFinite(draft.logRetentionDays) ? draft.logRetentionDays : ""}
                  onValueChange={(v) => onChange({ ...draft, logRetentionDays: v === "" ? NaN : v })} />
              </div>
            </section>
            <Checkbox label="启动时自动连接" checked={draft.autoConnect} onCheckedChange={(checked) => onChange({ ...draft, autoConnect: checked })} />
          </Stack>
        </fieldset>
      </>}
    </Stack>
  </Card>;
}
