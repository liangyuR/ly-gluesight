import { Card, Heading, StatusBadge, Text, Tooltip, type StatusBadgeTone } from "qing-steel";
import type { CapturePlcStatus, TeachingRegisters } from "../../workspace/types";
import { teachingPhaseLabels, teachingReasonName, teachingStateName, teachingWaiting } from "../teaching";
import { OfflineNote } from "./HandshakeMonitor";

interface TeachingHandshakeMonitorProps {
  status: CapturePlcStatus | null;
  connected: boolean;
  error: string;
}

const phaseTones: Partial<Record<CapturePlcStatus["phase"], StatusBadgeTone>> = {
  unavailable: "warn", resetRequired: "warn", fault: "fault", idle: "neutral",
};
const waitingSummary: Record<CapturePlcStatus["phase"], string> = {
  unavailable: "核对示教点表与连接", resetRequired: "完成示教复位", idle: "等待开始采集",
  requested: "等待 PLC 接受", accepted: "等待 PLC 采集结束", draining: "PC 收尾保存",
  reporting: "等待 PLC 确认结果", cancelling: "等待 PLC 确认取消", releasing: "等待 PLC 释放", fault: "处置现场后复位",
};

function Registers({ side, value }: { side: "pc" | "plc"; value: TeachingRegisters | null }) {
  const label = side === "pc" ? "PC" : "PLC";
  const fields = value ? [
    { label: "状态", value: teachingStateName(side, value.state), raw: `${side === "pc" ? "teachPcState" : "teachPlcState"}：${value.state}` },
    { label: "轮次", value: String(value.roundId), raw: `${side === "pc" ? "teachRoundId" : "teachRoundIdEcho"}：${value.roundId}` },
    { label: "原因", value: teachingReasonName(side, value.reason), raw: `${side === "pc" ? "teachPcReason" : "teachPlcReason"}：${value.reason}` },
  ] : [];
  return <div className="plc-values-row" role="group" aria-label={`${label} 示教寄存器`}>
    <span className="plc-values-side">{label}</span>
    {value ? <dl className="plc-values">
      {fields.map(field => <Tooltip key={field.label} content={<div className="plc-signal-details">
        <span className="mono">{field.raw}</span>
        <span>协议版本：{value.version}</span>
      </div>}>
        <div tabIndex={0}><dt>{field.label}</dt><dd>{field.value}</dd></div>
      </Tooltip>)}
    </dl> : <Text variant="label" muted>—</Text>}
  </div>;
}

export default function TeachingHandshakeMonitor({ status, connected, error }: TeachingHandshakeMonitorProps) {
  const fresh = connected && !error && !!status?.monitor?.fresh;
  const pc = fresh ? status?.monitor?.pc ?? null : null;
  const plc = fresh ? status?.monitor?.plc ?? null : null;
  const unconfirmed = !!status && status.phase === "idle" && !fresh;
  const phase = error ? "读取失败" : unconfirmed ? "状态待确认" : status ? teachingPhaseLabels[status.phase] ?? "未知阶段" : "正在读取";
  const tone: StatusBadgeTone = error ? "fault" : unconfirmed ? "warn" : status ? phaseTones[status.phase] ?? "info" : "neutral";
  const waiting = error ? "等待示教状态读取恢复"
    : unconfirmed ? "PC：等待当前连接的有效示教快照"
    : status ? teachingWaiting(status) : "PC：正在读取示教接口状态";
  const summary = error ? "等待读取恢复" : unconfirmed ? "等待有效快照" : status ? waitingSummary[status.phase] ?? "核对示教接口" : "正在读取";
  const actions = connected ? <div className="plc-monitor-head">
    <StatusBadge tone={tone}>{phase}</StatusBadge>
    <Tooltip content={waiting}><Text variant="label" muted className="plc-monitor-meta" tabIndex={0}>{summary}</Text></Tooltip>
  </div> : undefined;
  return <Card role="region" aria-label="示教握手监视" title={<Heading level={3}>示教握手监视</Heading>} actions={actions}>
    {!connected ? <OfflineNote />
      : error ? <Text as="p" role="alert">{error}</Text>
      : <div className="plc-handshake">
        {fresh ? <>
          <Registers side="pc" value={pc} />
          <Registers side="plc" value={plc} />
          {!pc && <Text as="p" variant="label" muted>PC 状态尚未发布确认，PC 寄存器值暂不显示。</Text>}
        </> : <Text as="p" variant="label" muted>尚未取得当前连接的有效快照，双方寄存器值暂不显示。</Text>}
        {status?.message && <Text as="p" variant="label" muted>后端说明：{status.message}</Text>}
      </div>}
  </Card>;
}
