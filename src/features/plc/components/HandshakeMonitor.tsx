import { useEffect, useState } from "react";
import { Card, Chip, EmptyState, Heading, StatusBadge, Text, type StatusBadgeTone } from "qing-steel";
import { handshakePhaseLabels, handshakeWaiting, heartbeatStaleMs, heartbeatTags, pcToPlcBits, plcToPcBits, resultCodeLabel } from "../handshake";
import type { PlcHandshakeState, PointValue } from "../types";

/** 按标签读取已保存点表中对应点位的实时值；undefined 表示点表没有该标签。 */
export type TagValue = (tag: string) => PointValue | null | undefined;

interface HandshakeMonitorProps {
  handshake: PlcHandshakeState | null;
  connected: boolean;
  tagValue: TagValue;
}

const phaseTones: Partial<Record<PlcHandshakeState["phase"], StatusBadgeTone>> = { fault: "ng", resetRequired: "warn", idle: "neutral" };

function useNow(intervalMs: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(timer);
  }, [intervalMs]);
  return now;
}

function bitBadge(value: PointValue | null | undefined, connected: boolean): { tone: StatusBadgeTone; text: string } {
  if (value === undefined) return { tone: "neutral", text: "未配置" };
  if (!connected || !value) return { tone: "off", text: "—" };
  if (value.error || value.value === null) return { tone: "ng", text: "!" };
  return value.value === true || value.value === 1 ? { tone: "ok", text: "●1" } : { tone: "off", text: "○0" };
}

/** 后端只在值或错误变化时更新 ts（取该次轮询时刻），所以 ts 即最后一次翻转时间。 */
function Heartbeat({ tag, value, connected }: { tag: string; value: PointValue | null | undefined; connected: boolean }) {
  const now = useNow(500);
  const badge = bitBadge(value, connected);
  if (badge.text !== "●1" && badge.text !== "○0" || !value) return <span className="plc-bit"><span className="mono">{tag}</span><StatusBadge tone={badge.tone}>{badge.text}</StatusBadge></span>;
  const age = Math.max(0, now - value.ts);
  const stale = age > heartbeatStaleMs;
  return <span className="plc-bit">
    <span className="mono">{tag}</span>
    <StatusBadge tone={stale ? "warn" : badge.tone}>{badge.text}</StatusBadge>
    <Text variant="label" muted={!stale} className={stale ? "plc-stale" : undefined}>
      {stale ? `失活 >${heartbeatStaleMs / 1000} s` : `${(age / 1000).toFixed(1)} s 前翻转`}
    </Text>
  </span>;
}

function Bits({ label, tags, tagValue, connected }: { label: string; tags: readonly string[]; tagValue: TagValue; connected: boolean }) {
  return <div className="plc-bits" role="group" aria-label={label}>
    <span className="plc-bits-label">{label}</span>
    <div className="plc-bits-grid">
      {tags.map(tag => heartbeatTags.has(tag)
        ? <Heartbeat key={tag} tag={tag} value={tagValue(tag)} connected={connected} />
        : <span key={tag} className="plc-bit"><span className="mono">{tag}</span>
          <StatusBadge tone={bitBadge(tagValue(tag), connected).tone}>{bitBadge(tagValue(tag), connected).text}</StatusBadge></span>)}
    </div>
  </div>;
}

export default function HandshakeMonitor({ handshake, connected, tagValue }: HandshakeMonitorProps) {
  const read = (tag: string) => {
    const v = tagValue(tag);
    return connected && v && !v.error && v.value !== null ? v.value : null;
  };
  const show = (value: unknown) => value === null || value === undefined || value === "" ? "—" : String(value);
  const protocol = read("protocolVersion");
  const title = <Heading level={3}>握手监视 · 只读</Heading>;
  if (!handshake) return <Card role="region" aria-label="握手监视" title={title}>
    <EmptyState title="没有 S7 握手会话" description="连接 PLC 并载入一期 S7 点表后，这里显示双方握手位和当前阶段" />
  </Card>;

  const waiting = handshakeWaiting(handshake);
  const done = read("done");
  const resultCode = read("resultCode");
  const faultCode = read("faultCode");
  return <Card role="region" aria-label="握手监视" title={title} actions={<Chip>协议 {show(protocol)}</Chip>}>
    <div className="plc-handshake">
      <dl className="plc-kv">
        <div><dt>阶段</dt><dd><StatusBadge tone={phaseTones[handshake.phase] ?? "info"}>
          {handshakePhaseLabels[handshake.phase]} <span className="mono">({handshake.phase})</span>
        </StatusBadge></dd></div>
        <div><dt>现在在等</dt><dd>{waiting}</dd></div>
      </dl>
      <Bits label="PLC → PC" tags={plcToPcBits} tagValue={tagValue} connected={connected} />
      <Bits label="PC → PLC" tags={pcToPlcBits} tagValue={tagValue} connected={connected} />
      <div className="plc-handshake-fields">
        <dl className="plc-kv">
          <div><dt>序号</dt><dd className="mono">
            requestSeq {show(handshake.requestSeq ?? read("requestSeq"))} · acceptedSeq {show(read("acceptedSeq"))} · resultSeq {show(handshake.resultSeq ?? read("resultSeq"))} · ackSeq {show(read("ackSeq"))}
          </dd></div>
          <div><dt>结果</dt><dd><span className="mono">resultCode {show(resultCode)} · faultCode {show(faultCode)}</span>
            {(done === true || done === 1) && typeof resultCode === "number" && <> <Chip>{resultCodeLabel(resultCode, typeof faultCode === "number" ? faultCode : null)}</Chip></>}
          </dd></div>
        </dl>
        <dl className="plc-kv">
          <div><dt>SN</dt><dd>partSn <span className="sn">{show(read("partSn"))}</span> · resultSn <span className="sn">{show(read("resultSn"))}</span></dd></div>
          <div><dt>脉冲计数（参考）</dt><dd className="mono">
            deviceTriggers {show(read("deviceTriggers"))}（累计脉冲，参考） · deviceShots {show(read("deviceShots"))}（预期总帧数 ΣN_exp）
          </dd></div>
        </dl>
      </div>
      {handshake.message && !waiting.includes(handshake.message) && <Text as="p" variant="label" muted>后端说明：{handshake.message}</Text>}
    </div>
  </Card>;
}
