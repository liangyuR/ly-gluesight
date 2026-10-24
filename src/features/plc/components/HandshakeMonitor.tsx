import { useEffect, useState, type ReactNode } from "react";
import { Card, EmptyState, Heading, StatusBadge, Text, Tooltip, type StatusBadgeTone } from "qing-steel";
import { handshakePhaseLabels, handshakeWaiting, heartbeatStaleMs, pcToPlcBits, plcToPcBits, resultCodeLabel } from "../handshake";
import { formatTs } from "../time";
import type { PlcHandshakeState, PointValue } from "../types";

/** 按标签读取已保存点表中对应点位的实时值；undefined 表示点表没有该标签。 */
export type TagValue = (tag: string) => PointValue | null | undefined;

interface HandshakeMonitorProps {
  handshake: PlcHandshakeState | null;
  connected: boolean;
  tagValue: TagValue;
}

type LampTone = "off" | "ok" | "pc" | "fault" | "unbound";

const phaseTones: Partial<Record<PlcHandshakeState["phase"], StatusBadgeTone>> = { fault: "fault", resetRequired: "warn", idle: "neutral" };
const waitingSummary: Record<PlcHandshakeState["phase"], string> = {
  resetRequired: "完成双方故障复位", idle: "等待 PLC 提交工件", validating: "PC 校验并布防",
  acquiring: "等待 PLC 置运动结束", draining: "PC 收尾并生成结果", awaitAck: "等待 PLC 确认结果",
  releasing: "等待双方释放握手", fault: "处置现场后复位",
};

const signalNames: Record<string, string> = {
  partStart: "工件开始", partEnd: "运动结束", resultAck: "结果确认",
  visionReady: "视觉就绪", armed: "采集布防", busy: "检测中", done: "结果有效",
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
  return <Text as="p" variant="label" muted>PLC 未连接，信号不显示</Text>;
}

function Lamp({ tag, label, tone, state, value, extra, details }: {
  tag: string; label: string; tone: LampTone; state: string; value: PointValue | null | undefined; extra?: string; details?: ReactNode;
}) {
  const raw = value && !value.error && value.value !== null ? typeof value.value === "boolean" ? Number(value.value) : String(value.value) : "—";
  const cls = tone === "off" ? "lamp" : tone === "ok" ? "lamp on" : tone === "pc" ? "lamp pc on" : `lamp ${tone}`;
  return <Tooltip content={<div className="plc-signal-details">
    <span className="mono">{tag}</span>
    <span>状态：{state}</span>
    <span>当前值：{raw}</span>
    {value && Number.isFinite(value.ts) && value.ts > 0 && <span>最近更新：{formatTs(value.ts)}</span>}
    {value?.error && <span>读取错误：{value.error}</span>}
    {value === undefined && <span>已保存点表未配置此信号</span>}
    {details}
  </div>}>
    <span className={cls} role="img" aria-label={`${label}：${state}`} tabIndex={0}>
      {label}{extra && <span className="lamp-extra">{extra}</span>}
    </span>
  </Tooltip>;
}

function bitLamp(value: PointValue | null | undefined, activeTone: LampTone): { tone: LampTone; state: string } {
  if (value === undefined) return { tone: "unbound", state: "未配置" };
  if (value?.error) return { tone: "fault", state: "读取失败" };
  if (!value || value.value === null) return { tone: "off", state: "等待数据" };
  if (typeof value.value !== "boolean" && value.value !== 0 && value.value !== 1) return { tone: "fault", state: "值无效" };
  return value.value === true || value.value === 1 ? { tone: activeTone, state: "有效" } : { tone: "off", state: "无效" };
}

function heartbeatLamp(value: PointValue | null | undefined, now: number): { tone: LampTone; state: string; extra?: string } {
  const bit = bitLamp(value, "ok");
  if (bit.state !== "有效" && bit.state !== "无效") return bit;
  if (!value || !Number.isFinite(value.ts) || value.ts <= 0) return { tone: "off", state: "等待数据" };
  const age = Math.max(0, now - value.ts);
  const stale = age > heartbeatStaleMs;
  return { tone: stale ? "fault" : "ok", state: stale ? "失活" : "正常", extra: `${(age / 1000).toFixed(1)}s` };
}

const read = (tagValue: TagValue, connected: boolean, tag: string) => {
  const v = tagValue(tag);
  return connected && v && !v.error && v.value !== null ? v.value : null;
};
const show = (value: unknown) => value === null || value === undefined || value === "" ? "—" : String(value);

export function CommonMonitor({ connected, tagValue }: Pick<HandshakeMonitorProps, "connected" | "tagValue">) {
  const now = useNow(500);
  const protocol = (tag: string) => show(read(tagValue, connected, tag));
  const heartbeatDetail = <span>失活阈值：{heartbeatStaleMs / 1000} s 无翻转</span>;
  const lamps = [
    { tag: "plcHeartbeat", label: "PLC 心跳", ...heartbeatLamp(tagValue("plcHeartbeat"), now), details: heartbeatDetail },
    { tag: "pcHeartbeat", label: "PC 心跳", ...heartbeatLamp(tagValue("pcHeartbeat"), now), details: heartbeatDetail },
    { tag: "visionFault", label: "视觉故障", ...bitLamp(tagValue("visionFault"), "fault") },
    { tag: "faultReset", label: "故障复位", ...bitLamp(tagValue("faultReset"), "pc") },
  ];
  return <Card role="region" aria-label="通用监视" title={<Heading level={3}>通用监视</Heading>}
    actions={<Tooltip content={`生产协议版本：PLC ${protocol("protocolVersion")} · PC ${protocol("pcProtocolVersion")}；示教使用独立扩展协议 V1`}>
      <Text variant="label" muted className="plc-monitor-meta" tabIndex={0}>生产协议 {protocol("protocolVersion")} · 示教 V1</Text>
    </Tooltip>}>
    {connected
      ? <div className="lamps">{lamps.map(lamp => <Lamp key={lamp.tag} {...lamp} value={tagValue(lamp.tag)} />)}</div>
      : <OfflineNote />}
  </Card>;
}

export default function HandshakeMonitor({ handshake, connected, tagValue }: HandshakeMonitorProps) {
  const title = <Heading level={3}>握手监视</Heading>;
  if (!handshake) return <Card role="region" aria-label="握手监视" title={title}>
    <EmptyState title="没有 S7 握手会话" description="使用「重置 S7 通讯配置」生成标准点表，保存配置并连接 PLC 后，这里显示双方握手位和当前阶段" />
  </Card>;

  const r = (tag: string) => read(tagValue, connected, tag);
  const retained = !connected && (handshake.phase === "fault" || handshake.phase === "resetRequired");
  const done = r("done");
  const resultCode = r("resultCode");
  const faultCode = r("faultCode");
  const resultValid = done === true || done === 1;
  const resultText = done === null ? "—" : !resultValid ? "等待结果"
    : resultCode === 1 ? "OK" : resultCode === 2 ? "合格（需关注）"
    : typeof resultCode === "number" && [11, 12, 13, 14].includes(resultCode) ? "NG"
    : resultCode === 90 ? "检测异常" : resultCode === null ? "—" : "未知结果";
  const requestSeq = connected ? handshake.requestSeq ?? r("requestSeq") : null;
  const fields = [
    { label: "事务", value: show(requestSeq), detail: <>
      <span>请求序号 requestSeq：{show(requestSeq)}</span>
      <span>已接受序号 acceptedSeq：{show(r("acceptedSeq"))}</span>
      <span>结果序号 resultSeq：{show(connected ? handshake.resultSeq ?? r("resultSeq") : null)}</span>
      <span>确认序号 ackSeq：{show(r("ackSeq"))}</span>
    </> },
    { label: "SN", value: show(r("partSn")), detail: <>
      <span>工件 SN partSn：{show(r("partSn"))}</span><span>结果 SN resultSn：{show(r("resultSn"))}</span>
    </> },
    { label: "结果", value: resultText, detail: <>
      <span>结果码 resultCode：{show(resultCode)}</span><span>异常码 faultCode：{show(faultCode)}</span>
      {resultValid && typeof resultCode === "number" && <span>{resultCodeLabel(resultCode, typeof faultCode === "number" ? faultCode : null)}</span>}
    </> },
    { label: "预期帧数", value: show(r("deviceShots")), detail: <>
      <span>设备预期帧数 deviceShots：{show(r("deviceShots"))}</span>
      <span>计划总帧数 shotCount：{show(r("shotCount"))}</span>
      <span>采集预留字段 deviceTriggers：{show(r("deviceTriggers"))}（协议 6 固定为 0）</span>
    </> },
  ];
  const lamp = (tag: string, activeTone: LampTone) =>
    <Lamp key={tag} tag={tag} label={signalNames[tag]} {...bitLamp(tagValue(tag), activeTone)} value={tagValue(tag)} />;

  const actions = connected || retained ? <div className="plc-monitor-head">
    <Tooltip content={handshake.message || `后端阶段：${handshake.phase}`}>
      <StatusBadge tone={retained ? "fault" : phaseTones[handshake.phase] ?? "info"} tabIndex={0}>
        {retained ? "重连后需复位" : handshakePhaseLabels[handshake.phase]}
      </StatusBadge>
    </Tooltip>
    {connected && <Tooltip content={handshakeWaiting(handshake)}>
      <Text variant="label" muted className="plc-monitor-meta" tabIndex={0}>{waitingSummary[handshake.phase]}</Text>
    </Tooltip>}
  </div> : undefined;

  return <Card role="region" aria-label="握手监视" title={title} actions={actions}>
    {connected ? <div className="plc-handshake">
      <div className="lamps" role="group" aria-label="握手信号">
        {plcToPcBits.map(tag => lamp(tag, "ok"))}
        <span className="lamp-sep" aria-hidden="true">|</span>
        {pcToPlcBits.map(tag => lamp(tag, "pc"))}
      </div>
      <dl className="plc-values">
        {fields.map(field => <Tooltip key={field.label} content={<div className="plc-signal-details">{field.detail}</div>}>
          <div tabIndex={0}><dt>{field.label}</dt><dd>{field.value}</dd></div>
        </Tooltip>)}
      </dl>
      {handshake.message && <Text as="p" variant="label" muted className="plc-handshake-message">{handshake.message}</Text>}
    </div> : <OfflineNote />}
  </Card>;
}
