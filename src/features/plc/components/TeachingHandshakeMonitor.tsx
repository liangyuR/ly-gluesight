import { Card, Chip, Heading, StatusBadge, Text, type StatusBadgeTone } from "qing-steel";
import type { CapturePlcStatus, TeachingRegisters } from "../../workspace/types";
import { teachingPhaseLabels, teachingReasonLabel, teachingStateLabel, teachingWaiting } from "../teaching";

interface TeachingHandshakeMonitorProps {
  status: CapturePlcStatus | null;
  connected: boolean;
  error: string;
}

const phaseTones: Partial<Record<CapturePlcStatus["phase"], StatusBadgeTone>> = {
  unavailable: "warn", resetRequired: "warn", fault: "ng", idle: "neutral",
};

function Registers({ side, value }: { side: "pc" | "plc"; value: TeachingRegisters | null }) {
  const label = side === "pc" ? "PC" : "PLC";
  return <dl className="plc-kv" aria-label={`${label} 示教寄存器`}>
    <div><dt>{label} 状态</dt><dd>{value ? <StatusBadge tone={value.state >= (side === "pc" ? 30 : 40) ? "warn" : "info"}>{teachingStateLabel(side, value.state)}</StatusBadge> : "—"}</dd></div>
    <div><dt>轮次</dt><dd className="mono">{side === "pc" ? "teachRoundId" : "teachRoundIdEcho"} {value?.roundId ?? "—"}</dd></div>
    <div><dt>原因码</dt><dd>{value ? teachingReasonLabel(side, value.reason) : "—"}</dd></div>
    <div><dt>协议版本</dt><dd className="mono">{value?.version ?? "—"}</dd></div>
  </dl>;
}

export default function TeachingHandshakeMonitor({ status, connected, error }: TeachingHandshakeMonitorProps) {
  const fresh = connected && !error && !!status?.monitor?.fresh;
  const pc = fresh ? status?.monitor?.pc ?? null : null;
  const plc = fresh ? status?.monitor?.plc ?? null : null;
  const unconfirmed = !!status && status.phase === "idle" && !fresh;
  const phase = !connected ? "未连接" : error ? "读取失败" : unconfirmed ? "状态待确认" : status ? teachingPhaseLabels[status.phase] ?? "未知阶段" : "正在读取";
  const waiting = !connected ? "连接 PLC 后读取示教握手状态"
    : error ? "等待示教状态读取恢复"
    : unconfirmed ? "PC：等待当前连接的有效示教快照"
    : status ? teachingWaiting(status) : "PC：正在读取示教接口状态";
  return <Card role="region" aria-label="示教握手监视" title={<Heading level={3}>示教握手监视 · 只读</Heading>} actions={<Chip>示教协议 V1 · DB6 / DB12</Chip>}>
    <div className="plc-handshake">
      <dl className="plc-kv">
        <div><dt>阶段</dt><dd><StatusBadge tone={error ? "ng" : !connected ? "off" : unconfirmed ? "warn" : status ? phaseTones[status.phase] ?? "info" : "neutral"}>{phase}</StatusBadge></dd></div>
        <div><dt>现在在等</dt><dd>{waiting}</dd></div>
      </dl>
      <div className="plc-handshake-fields">
        <Registers side="pc" value={pc} />
        <Registers side="plc" value={plc} />
      </div>
      {error ? <Text as="p" role="alert">{error}</Text>
        : connected && status?.message && <Text as="p" variant="label" muted>后端说明：{status.message}</Text>}
      {connected && !error && !fresh && <Text as="p" variant="label" muted>尚未取得当前连接的有效快照，双方寄存器值暂不显示。</Text>}
      {fresh && !pc && <Text as="p" variant="label" muted>PC 状态尚未发布确认，PC 寄存器值暂不显示。</Text>}
    </div>
  </Card>;
}
