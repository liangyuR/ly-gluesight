import { useEffect, useState, type ReactNode } from "react";
import { Card, EmptyState, Heading, StatusBadge, Text, Tooltip, type StatusBadgeTone } from "qing-steel";
import { codeName, handshakePhaseLabels, handshakeWaiting, heartbeatStaleMs, pcModeNames, protocolVersion, resultCodeLabel } from "../handshake";
import { formatTs } from "../time";
import type { PlcHandshakeState, PlcValue, PointValue } from "../types";

/** 按标签读取已保存点表中对应点位的实时值；undefined 表示点表没有该标签。 */
export type TagValue = (tag: string) => PointValue | null | undefined;

interface HandshakeMonitorProps {
  handshake: PlcHandshakeState | null;
  connected: boolean;
  tagValue: TagValue;
}

type LampTone = "off" | "ok" | "pc" | "fault" | "unbound";

const phaseTones: Partial<Record<PlcHandshakeState["phase"], StatusBadgeTone>> = { notReady: "fault", idle: "neutral" };
const waitingSummary: Record<PlcHandshakeState["phase"], string> = {
  notReady: "恢复后自动就绪", idle: "等待工件到达", validating: "PC 接件登记",
  acquiring: "采集中，等待工件结束", draining: "PC 收尾并写结果",
};

function useNow(intervalMs: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(timer);
  }, [intervalMs]);
  return now;
}

export function OfflineNote() {
  return <Text as="p" variant="label" muted>连接后显示信号</Text>;
}

function Lamp({ tag, label, tone, state, value, extra, details }: {
  tag: string; label: string; tone: LampTone; state: string; value: PointValue | null | undefined; extra?: string; details?: ReactNode;
}) {
  const raw = value && !value.error && value.value !== null ? typeof value.value === "boolean" ? Number(value.value) : String(value.value) : "—";
  const badgeTone: StatusBadgeTone = tone === "pc" ? "info" : tone === "unbound" ? "neutral" : tone;
  return <Tooltip content={<div className="plc-signal-details">
    <span className="mono">{tag}</span>
    <span>状态：{state}</span>
    <span>当前值：{raw}</span>
    {value && Number.isFinite(value.ts) && value.ts > 0 && <span>最近更新：{formatTs(value.ts)}</span>}
    {value?.error && <span>读取错误：{value.error}</span>}
    {value === undefined && <span>已保存点表未配置此信号</span>}
    {details}
  </div>}>
    <StatusBadge tone={badgeTone} className="plc-monitor-meta" role="img" aria-label={`${label}：${state}`} tabIndex={0}>
      {label} · {state}{extra && <Text mono> {extra}</Text>}
    </StatusBadge>
  </Tooltip>;
}

function heartbeatLamp(value: PointValue | null | undefined, now: number): { tone: LampTone; state: string; extra?: string } {
  if (value === undefined) return { tone: "unbound", state: "未配置" };
  if (value?.error) return { tone: "fault", state: "读取失败" };
  if (!value || value.value === null || !Number.isFinite(value.ts) || value.ts <= 0) return { tone: "off", state: "等待数据" };
  if (typeof value.value !== "number" && typeof value.value !== "boolean") return { tone: "fault", state: "值无效" };
  const age = Math.max(0, now - value.ts);
  const stale = age > heartbeatStaleMs;
  return { tone: stale ? "fault" : "ok", state: stale ? "失活" : "正常", extra: `${(age / 1000).toFixed(1)}s` };
}

const read = (tagValue: TagValue, connected: boolean, tag: string) => {
  const v = tagValue(tag);
  return connected && v && !v.error && v.value !== null ? v.value : null;
};
const show = (value: unknown) => value === null || value === undefined || value === "" ? "—" : String(value);
const num = (value: PlcValue | null) => typeof value === "number" ? value : null;

export function CommonMonitor({ connected, tagValue }: Pick<HandshakeMonitorProps, "connected" | "tagValue">) {
  const now = useNow(500);
  const protocol = (tag: string) => show(read(tagValue, connected, tag));
  const heartbeatDetail = <span>失活阈值：{heartbeatStaleMs / 1000} s 不变化</span>;
  const lamps = [
    { tag: "plcHeartbeat", label: "PLC 心跳", ...heartbeatLamp(tagValue("plcHeartbeat"), now), details: heartbeatDetail },
    { tag: "pcHeartbeat", label: "PC 心跳", ...heartbeatLamp(tagValue("pcHeartbeat"), now), details: heartbeatDetail },
  ];
  return <Card role="region" aria-label="通用监视" title={<Heading level={3}>通用监视</Heading>}
    actions={<Tooltip content={`生产协议版本：PLC ${protocol("plcVersion")} · PC ${protocol("pcVersion")}；示教使用独立扩展协议 V1`}>
      <Text variant="label" muted className="plc-monitor-meta" tabIndex={0}>生产协议 {protocol("plcVersion")} · 示教 V1</Text>
    </Tooltip>}>
    {connected
      ? <div className="lamps">{lamps.map(lamp => <Lamp key={lamp.tag} {...lamp} value={tagValue(lamp.tag)} />)}</div>
      : <OfflineNote />}
  </Card>;
}

interface Field { label: string; tag: string; value: string; details?: ReactNode; violation?: boolean }

function Registers({ side, fields }: { side: "PLC" | "PC"; fields: Field[] }) {
  return <div className="plc-values-row" role="group" aria-label={`${side} 生产寄存器`}>
    <span className="plc-values-side">{side}</span>
    <dl className="plc-values">
      {fields.map(field => <Tooltip key={field.label} content={<div className="plc-signal-details">
        <span className="mono">{field.tag}</span>
        {field.details}
      </div>}>
        <div tabIndex={0} className={field.violation ? "violation" : undefined}><dt>{field.label}</dt><dd>{field.value}</dd></div>
      </Tooltip>)}
    </dl>
  </div>;
}

export default function HandshakeMonitor({ handshake, connected, tagValue }: HandshakeMonitorProps) {
  const title = <Heading level={3}>握手监视</Heading>;
  if (!handshake) return <Card role="region" aria-label="握手监视" title={title}>
    <EmptyState title="没有 S7 生产会话" description="使用「重置 S7 通讯配置」生成标准点表，保存配置并连接 PLC 后，这里显示当前件、最近结果和当前阶段" />
  </Card>;

  const r = (tag: string) => read(tagValue, connected, tag);
  const notReady = handshake.phase === "notReady";
  const inspecting = r("inspecting");
  const partSeq = num(r("partSeq"));
  const pcMode = num(r("pcMode"));
  const resultSeq = num(r("resultSeq"));
  const resultCode = num(r("resultCode"));
  const faultCode = num(r("faultCode"));
  const sn = r("sn");
  const snEcho = r("snEcho");
  const hasResult = !!resultSeq;
  const matched = hasResult && resultSeq === partSeq;
  const verdict = resultCode === 0 ? "无结论（维护）"
    : resultCode === 1 ? "OK" : resultCode === 2 ? "合格（需关注）"
    : resultCode !== null && [11, 12, 13, 14].includes(resultCode) ? "NG"
    : resultCode === 90 ? "检测错误" : resultCode === null ? "—" : "未知结果";
  const resultText = !hasResult ? "—" : matched ? verdict : `${verdict}（非本件）`;
  const raw = (value: unknown) => <span>当前值：{show(value)}</span>;
  const version = (tag: string): Field => {
    const value = num(r(tag));
    return { label: "协议版本", tag, value: show(value), details: <span>应为 {protocolVersion}</span>, violation: value !== null && value !== protocolVersion };
  };

  const plcFields: Field[] = [
    version("plcVersion"),
    { label: "心跳", tag: "plcHeartbeat", value: show(r("plcHeartbeat")) },
    { label: "在检", tag: "inspecting", value: inspecting === null ? "—" : String(Number(inspecting)), details: <span>工件到达置 1，工件结束置 0</span> },
    { label: "件序号", tag: "partSeq", value: show(partSeq), details: <span>每到一件加 1；0 表示还没有件</span> },
    { label: "型号", tag: "productCode", value: show(r("productCode")) },
    { label: "SN", tag: "sn", value: show(sn) },
  ];
  const pcFields: Field[] = [
    version("pcVersion"),
    { label: "心跳", tag: "pcHeartbeat", value: show(r("pcHeartbeat")) },
    { label: "模式", tag: "pcMode", value: pcMode === null ? "—" : codeName(pcModeNames, pcMode), details: <>{raw(pcMode)}<span>维护模式下结果码为 0，PLC 按未检放行</span></> },
    { label: "结果件序号", tag: "resultSeq", value: show(resultSeq), details: <span>等于 PLC 件序号时是本件结果；0 表示没有结果</span>, violation: matched && !!inspecting },
    { label: "结果", tag: "resultCode / faultCode", value: resultText, details: <>
      <span>结果码 resultCode：{show(resultCode)}</span><span>异常码 faultCode：{show(faultCode)}</span>
      {hasResult && resultCode !== null && <span>{resultCodeLabel(resultCode, faultCode)}</span>}
    </> },
    { label: "结果 SN", tag: "snEcho", value: show(snEcho), violation: matched && snEcho !== null && sn !== null && snEcho !== sn },
  ];

  const actions = connected || notReady ? <div className="plc-monitor-head">
    <Tooltip content={handshake.message || `后端阶段：${handshake.phase}`}>
      <StatusBadge tone={phaseTones[handshake.phase] ?? "info"} tabIndex={0}>{handshakePhaseLabels[handshake.phase]}</StatusBadge>
    </Tooltip>
    {connected && <Tooltip content={handshakeWaiting(handshake)}>
      <Text variant="label" muted className="plc-monitor-meta" tabIndex={0}>{waitingSummary[handshake.phase]}</Text>
    </Tooltip>}
  </div> : undefined;

  return <Card role="region" aria-label="握手监视" title={title} actions={actions}>
    {connected ? <div className="plc-handshake">
      <Registers side="PLC" fields={plcFields} />
      <Registers side="PC" fields={pcFields} />
      <Text as="p" variant="label" muted className="plc-handshake-message" title={handshake.message || undefined}>{handshake.message || " "}</Text>
    </div> : <OfflineNote />}
  </Card>;
}
