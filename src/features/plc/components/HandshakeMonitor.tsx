import { useEffect, useState } from "react";
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

const phaseTones: Partial<Record<PlcHandshakeState["phase"], StatusBadgeTone>> = { fault: "fault", resetRequired: "warn", idle: "neutral" };
const waitingSummary: Record<PlcHandshakeState["phase"], string> = {
  resetRequired: "完成双方故障复位", idle: "等待 PLC 提交工件", validating: "PC 校验并布防",
  acquiring: "等待 PLC 通知运动结束", draining: "PC 收尾并生成结果", awaitAck: "等待 PLC 确认结果",
  releasing: "等待双方释放握手", fault: "处置现场后复位",
};

function useNow(intervalMs: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(timer);
  }, [intervalMs]);
  return now;
}

const signalLabels: Record<string, [string, string, string]> = {
  partStart: ["工件开始", "请求有效", "未请求"],
  partEnd: ["运动结束", "已结束", "未结束"],
  resultAck: ["结果确认", "已确认", "未确认"],
  visionReady: ["视觉就绪", "已就绪", "未就绪"],
  armed: ["采集布防", "已布防", "未布防"],
  busy: ["检测中", "进行中", "空闲"],
  done: ["结果有效", "有效", "无结果"],
};

function Bits({ label, tags, tagValue, connected }: { label: string; tags: readonly string[]; tagValue: TagValue; connected: boolean }) {
  return <div className="plc-bit-group" role="group" aria-label={label}>
    <Text variant="label" muted>{label}</Text>
    <div className="plc-common-signals">
      {tags.map(tag => {
        const value = tagValue(tag);
        const [name, on, off] = signalLabels[tag];
        const available = connected && value && !value.error && value.value !== null;
        const valid = available && (typeof value.value === "boolean" || value.value === 0 || value.value === 1);
        const active = valid && (value.value === true || value.value === 1);
        const text = !connected ? "未连接" : value === undefined ? "未配置" : value?.error ? "读取失败"
          : !available ? "等待数据" : !valid ? "值无效" : active ? on : off;
        const tone = connected && (value?.error || available && !valid) ? "fault" : active ? "info" : "neutral";
        return <Tooltip key={tag} content={<div className="plc-signal-details">
          <span className="mono">{tag}</span>
          <span>当前值：{available ? typeof value.value === "boolean" ? Number(value.value) : String(value.value) : "—"}</span>
          {connected && value && Number.isFinite(value.ts) && value.ts > 0 && <span>最近更新：{formatTs(value.ts)}</span>}
          {connected && value?.error && <span>读取错误：{value.error}</span>}
        </div>}>
          <div className="plc-common-signal" tabIndex={0}>
            <span className="plc-common-signal-name">{name}</span>
            <StatusBadge tone={tone} className="plc-flat-state">{text}</StatusBadge>
          </div>
        </Tooltip>;
      })}
    </div>
  </div>;
}

const commonSignals = [
  { tag: "plcHeartbeat", label: "PLC 心跳", heartbeat: true },
  { tag: "pcHeartbeat", label: "PC 心跳", heartbeat: true },
  { tag: "visionFault", label: "视觉故障", heartbeat: false },
  { tag: "faultReset", label: "故障复位", heartbeat: false },
];

function commonSignalState(tag: string, value: PointValue | null | undefined, connected: boolean, now: number): { tone: StatusBadgeTone; text: string; timing?: string } {
  if (!connected) return { tone: "neutral", text: "未连接" };
  if (value === undefined) return { tone: "neutral", text: "未配置" };
  if (value?.error) return { tone: "fault", text: "读取失败" };
  if (!value || value.value === null) return { tone: "neutral", text: "等待数据" };
  if (typeof value.value !== "boolean" && value.value !== 0 && value.value !== 1) return { tone: "fault", text: "值无效" };
  if (tag === "plcHeartbeat" || tag === "pcHeartbeat") {
    if (!Number.isFinite(value.ts) || value.ts <= 0) return { tone: "neutral", text: "等待数据" };
    const age = Math.max(0, now - value.ts);
    const stale = age > heartbeatStaleMs;
    return { tone: stale ? "fault" : "ok", text: stale ? "失活" : "正常", timing: `${(age / 1000).toFixed(1)} s ${stale ? "未翻转" : "前翻转"}` };
  }
  const active = value.value === true || value.value === 1;
  return tag === "visionFault"
    ? { tone: active ? "fault" : "neutral", text: active ? "故障" : "无故障" }
    : { tone: active ? "info" : "neutral", text: active ? "信号有效" : "未触发" };
}

export function CommonMonitor({ connected, tagValue }: Pick<HandshakeMonitorProps, "connected" | "tagValue">) {
  const now = useNow(500);
  const protocol = (tag: string) => {
    const value = tagValue(tag);
    return connected && value && !value.error && value.value !== null ? String(value.value) : "—";
  };
  return <Card role="region" aria-label="通用监视" title={<Heading level={3}>通用监视 <Text variant="label" muted>只读</Text></Heading>}
    actions={<Text variant="label" muted>PLC {connected ? "已连接" : "未连接"}</Text>}>
    <div className="plc-handshake">
      <div className="plc-common-signals">
        {commonSignals.map(signal => {
          const value = tagValue(signal.tag);
          const state = commonSignalState(signal.tag, value, connected, now);
          const raw = connected && value && !value.error && value.value !== null
            ? typeof value.value === "boolean" ? Number(value.value) : String(value.value) : "—";
          const updated = connected && value && Number.isFinite(value.ts) && value.ts > 0 ? formatTs(value.ts) : "—";
          return <Tooltip key={signal.tag} content={<div className="plc-signal-details">
            <span className="mono">{signal.tag}</span>
            <span>当前值：{raw}</span>
            <span>最近更新：{updated}</span>
            {signal.heartbeat && <span>失活阈值：{heartbeatStaleMs / 1000} s 无翻转</span>}
            {connected && value?.error && <span>读取错误：{value.error}</span>}
            {value === undefined && <span>已保存点表未配置此信号</span>}
          </div>}>
            <div className="plc-common-signal" tabIndex={0}>
              <span className="plc-common-signal-name">{signal.label}</span>
              <StatusBadge tone={state.tone} className="plc-flat-state">{state.text}</StatusBadge>
              {state.timing && <span className="plc-common-signal-time">{state.timing}</span>}
            </div>
          </Tooltip>;
        })}
      </div>
      <Tooltip content={`生产协议版本：PLC ${protocol("protocolVersion")} · PC ${protocol("pcProtocolVersion")}；示教使用独立扩展协议 V1`}>
        <span className="plc-common-protocol" tabIndex={0}>生产协议 {protocol("protocolVersion")} · 示教协议 V1</span>
      </Tooltip>
    </div>
  </Card>;
}

export default function HandshakeMonitor({ handshake, connected, tagValue }: HandshakeMonitorProps) {
  const read = (tag: string) => {
    const v = tagValue(tag);
    return connected && v && !v.error && v.value !== null ? v.value : null;
  };
  const show = (value: unknown) => value === null || value === undefined || value === "" ? "—" : String(value);
  const protocol = read("protocolVersion");
  const title = <Heading level={3}>握手监视 <Text variant="label" muted>只读</Text></Heading>;
  if (!handshake) return <Card role="region" aria-label="握手监视" title={title}>
    <EmptyState title="没有 S7 握手会话" description="使用「重置 S7 通讯配置」生成标准点表，保存配置并连接 PLC 后，这里显示双方握手位和当前阶段" />
  </Card>;

  const waiting = handshakeWaiting(handshake);
  const done = read("done");
  const resultCode = read("resultCode");
  const faultCode = read("faultCode");
  const resultValid = done === true || done === 1;
  const resultText = !connected ? "—" : done === null ? "等待数据" : !resultValid ? "等待结果"
    : resultCode === 1 ? "OK" : resultCode === 2 ? "合格（需关注）"
    : typeof resultCode === "number" && [11, 12, 13, 14].includes(resultCode) ? "NG"
    : resultCode === 90 ? "检测异常" : resultCode === null ? "等待数据" : "未知结果";
  const fields = [
    { label: "事务序号", value: show(connected ? handshake.requestSeq ?? read("requestSeq") : null), detail: <>
      <span>请求序号 requestSeq：{show(connected ? handshake.requestSeq ?? read("requestSeq") : null)}</span>
      <span>已接受序号 acceptedSeq：{show(read("acceptedSeq"))}</span>
      <span>结果序号 resultSeq：{show(connected ? handshake.resultSeq ?? read("resultSeq") : null)}</span>
      <span>确认序号 ackSeq：{show(read("ackSeq"))}</span>
    </> },
    { label: "检测结果", value: resultText, detail: <>
      <span>结果码 resultCode：{show(resultCode)}</span><span>异常码 faultCode：{show(faultCode)}</span>
      {resultValid && typeof resultCode === "number" && <span>{resultCodeLabel(resultCode, typeof faultCode === "number" ? faultCode : null)}</span>}
    </> },
    { label: "工件 SN", value: show(read("partSn")), detail: <>
      <span>工件 SN partSn：{show(read("partSn"))}</span><span>结果 SN resultSn：{show(read("resultSn"))}</span>
    </> },
    { label: "预期总帧数", value: show(read("deviceShots")), detail: <>
      <span>设备预期帧数 deviceShots：{show(read("deviceShots"))}</span>
      <span>计划总帧数 shotCount：{show(read("shotCount"))}</span>
      <span>采集预留字段 deviceTriggers：{show(read("deviceTriggers"))}（协议 6 固定为 0）</span>
    </> },
  ];
  return <Card role="region" aria-label="握手监视" title={title} actions={<Text variant="label" muted>生产协议 {show(protocol)}</Text>}>
    <div className="plc-handshake">
      <dl className="plc-handshake-overview">
        <div><dt>当前阶段</dt><dd><Tooltip content={`后端阶段：${handshake.phase}`}>
          <StatusBadge tone={phaseTones[handshake.phase] ?? "info"} className="plc-flat-state" tabIndex={0}>
            {handshakePhaseLabels[handshake.phase]}
          </StatusBadge>
        </Tooltip></dd></div>
        <div><dt>现在在等</dt><dd><Tooltip content={waiting}>
          <span className="plc-handshake-waiting" tabIndex={0}>{waitingSummary[handshake.phase]}</span>
        </Tooltip></dd></div>
      </dl>
      {handshake.message && <Text as="p" variant="label" muted className="plc-handshake-message">{handshake.message}</Text>}
      <Bits label="PLC → PC" tags={plcToPcBits} tagValue={tagValue} connected={connected} />
      <Bits label="PC → PLC" tags={pcToPlcBits} tagValue={tagValue} connected={connected} />
      <dl className="plc-handshake-summary">
        {fields.map(field => <Tooltip key={field.label} content={<div className="plc-signal-details">{field.detail}</div>}>
          <div className="plc-summary-item" tabIndex={0}><dt>{field.label}</dt><dd>{field.value}</dd></div>
        </Tooltip>)}
      </dl>
    </div>
  </Card>;
}
