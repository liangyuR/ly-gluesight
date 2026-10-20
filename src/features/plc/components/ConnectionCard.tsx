import { ChevronUp, Pencil } from "lucide-react";
import { Alert, Button, Card, Checkbox, Heading, NumberField, Select, Stack, Text } from "qing-steel";
import ConnectionForm from "./ConnectionForm";
import { cpuPresetLabel } from "../meta";
import type { PlcConfig } from "../types";

const fieldsetStyle = { border: 0, padding: 0, margin: 0, minWidth: 0 };
const hex = (n: number | null) => n === null ? "自动" : Number.isFinite(n) ? `0x${n.toString(16).toUpperCase().padStart(4, "0")}` : "无效";
const num = (n: number) => Number.isFinite(n) ? String(n) : "未填";

type SummaryItem = { key: string; text: string };

function summaryLines(config: PlcConfig): SummaryItem[][] {
  const c = config.connection;
  const heartbeatPoint = config.points.find(p => p.id === config.heartbeat.pointId);
  return [[
    { key: "cpu", text: cpuPresetLabel(c) },
    { key: "host", text: `${c.host || "未填地址"}:${num(c.port)}` },
    { key: "rackSlot", text: `Rack ${num(c.s7.rack)} / Slot ${num(c.s7.slot)}` },
    { key: "type", text: c.s7.connectionType === "basic" ? "S7 Basic" : c.s7.connectionType.toUpperCase() },
    { key: "tsap", text: `TSAP ${hex(c.s7.localTsap)}/${hex(c.s7.remoteTsap)}` },
    { key: "pdu", text: `PDU ${num(c.s7.pduSize)}` },
  ], [
    { key: "poll", text: `轮询 ${num(c.pollIntervalMs)} ms` },
    { key: "timeout", text: `超时 ${num(c.timeoutMs)} ms` },
    { key: "reconnect", text: `重连 ${num(c.reconnectIntervalMs)} ms` },
    { key: "heartbeat", text: !config.heartbeat.pointId ? "心跳 不启用"
      : `心跳 ${heartbeatPoint ? `${heartbeatPoint.name} (${heartbeatPoint.address})` : config.heartbeat.pointId} / ${num(config.heartbeat.intervalMs)} ms` },
    { key: "log", text: config.logRetentionDays === 0 ? "日志保留 永久" : `日志保留 ${num(config.logRetentionDays)} 天` },
    { key: "auto", text: `启动自动连接 ${config.autoConnect ? "开" : "关"}` },
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
  const savedText = new Map(saved ? summaryLines(saved).flat().map(item => [item.key, item.text]) : []);
  const writablePoints = draft.points.filter(p => p.access === "readWrite");
  return <Card role="region" aria-label="连接配置" title={<Heading level={3}>连接配置</Heading>}
    actions={open
      ? <Button variant="tonal" leadingIcon={<ChevronUp size={16} />} onClick={() => onOpenChange(false)}>收起</Button>
      : <Button variant="tonal" leadingIcon={<Pencil size={16} />} disabled={!!editBlockedReason} title={editBlockedReason || undefined}
        onClick={() => onOpenChange(true)}>编辑</Button>}>
    <Stack direction="column" align="stretch" gap={14} wrap={false}>
      <div className="plc-conn-summary">
        {summaryLines(draft).map((line, i) => <p key={i}>{line.map(item => {
          const before = savedText.get(item.key);
          const changed = before !== undefined && before !== item.text;
          return <span key={item.key} className={changed ? "plc-diff" : undefined} title={changed ? `已保存：${before}` : undefined}>{item.text}</span>;
        })}</p>)}
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
