import { useEffect, useRef, useState } from "react";
import { Alert, NumberField, Select, TextField } from "qing-steel";
import { s7CpuPresets, s7Protocol } from "../meta";
import type { ConnectionConfig, S7ConnectionType, S7Options } from "../types";

interface ConnectionFormProps {
  value: ConnectionConfig;
  initialCpuPreset?: string;
  onChange: (value: ConnectionConfig) => void;
}

const hex = (n: number | null) => (n === null || !Number.isFinite(n) ? "" : `0x${n.toString(16).toUpperCase().padStart(4, "0")}`);
const numeric = (value: number | "") => value === "" ? NaN : value;
const numberValue = (n: number) => Number.isFinite(n) ? n : "";
const integerIn = (n: number, min: number, max = Number.MAX_SAFE_INTEGER) => Number.isSafeInteger(n) && n >= min && n <= max;

function parseHex(text: string, optional: boolean): number | null {
  const t = text.trim();
  if (!t && optional) return null;
  if (!/^(?:0x)?[\da-f]+$/i.test(t)) return NaN;
  const n = Number(`0x${t.replace(/^0x/i, "")}`);
  return integerIn(n, 0, 0xffff) ? n : NaN;
}

function HexInput({ label, value, optional = false, placeholder, onChange }: {
  label: string; value: number | null; optional?: boolean; placeholder?: string; onChange: (n: number | null) => void;
}) {
  const [text, setText] = useState(() => hex(value));
  const emitted = useRef(value);
  useEffect(() => {
    if (!Object.is(value, emitted.current)) { setText(hex(value)); emitted.current = value; }
  }, [value]);
  const parsed = parseHex(text, optional);
  const invalid = parsed !== null && !Number.isFinite(parsed);
  const change = (next: string) => {
    setText(next);
    const n = parseHex(next, optional);
    emitted.current = n;
    onChange(n);
  };
  return <TextField label={label} value={text} placeholder={placeholder} error={invalid ? `请输入 0x0000–0xFFFF 的完整十六进制值${optional ? "，或留空自动" : ""}` : undefined} onChange={e => change(e.target.value)} onBlur={e => {
    const n = parseHex(e.target.value, optional);
    if (n === null || Number.isFinite(n)) { emitted.current = n; onChange(n); setText(hex(n)); }
  }} />;
}

export function connectionError(c: ConnectionConfig) {
  if (!c.host.trim()) return "请填写 PLC 地址";
  if (!integerIn(c.port, 1, 65535)) return "端口需为 1–65535 的整数";
  if (!integerIn(c.timeoutMs, 100)) return "通讯超时需为不小于 100 ms 的整数";
  if (!integerIn(c.pollIntervalMs, 20)) return "轮询周期需为不小于 20 ms 的整数";
  if (!integerIn(c.reconnectIntervalMs, 0)) return "重连间隔需为非负整数";
  if (!integerIn(c.s7.rack, 0, 7) || !integerIn(c.s7.slot, 0, 31)) return "S7 机架需为 0–7、插槽需为 0–31 的整数";
  if (!integerIn(c.s7.pduSize, 240, 960)) return "S7 PDU 长度需为 240–960 的整数";
  if ([c.s7.localTsap, c.s7.remoteTsap].some(n => n !== null && !integerIn(n, 0, 0xffff))) return "S7 TSAP 需为完整的 16 位十六进制值，留空自动";
  return "";
}

export default function ConnectionForm({ value: c, initialCpuPreset, onChange }: ConnectionFormProps) {
  const [cpuChoice, setCpuChoice] = useState<string | null>(initialCpuPreset ?? null);
  const set = (patch: Partial<ConnectionConfig>) => onChange({ ...c, ...patch });
  const setS7 = (patch: Partial<S7Options>) => set({ s7: { ...c.s7, ...patch } });
  const num = numeric;

  const matchesCpu = (p: (typeof s7CpuPresets)[number]) => p.rack === c.s7.rack && p.slot === c.s7.slot && p.localTsap === c.s7.localTsap && p.remoteTsap === c.s7.remoteTsap;
  const cpu = s7CpuPresets.find(p => p.key === cpuChoice && matchesCpu(p)) ?? s7CpuPresets.find(matchesCpu);

  const error = connectionError(c);
  return (
    <div className="plc-connection-form">
      <section className="plc-config-card" aria-labelledby="plc-network-title">
        <h4 id="plc-network-title">网络连接</h4>
        <div className="plc-config-fields">
          <TextField label="协议" readOnly value={s7Protocol.label} />
          <TextField label="IP / 主机名" value={c.host} onChange={(e) => set({ host: e.target.value })} />
          <NumberField label="端口" min={1} max={65535} step={1} value={numberValue(c.port)} aria-invalid={!integerIn(c.port, 1, 65535)} onValueChange={(v) => set({ port: num(v) })} />
        </div>
      </section>
      <section className="plc-config-card" aria-labelledby="plc-s7-title">
        <h4 id="plc-s7-title">S7 参数</h4>
        <div className="plc-config-fields">
          <Select label="CPU 型号" value={cpu?.key ?? "custom"}
            options={[...s7CpuPresets.map((p) => ({ value: p.key, label: p.label })), { value: "custom", label: "自定义", disabled: true }]}
            onValueChange={(key) => {
              const p = s7CpuPresets.find((x) => x.key === key);
              if (p) { setCpuChoice(p.key); setS7({ rack: p.rack, slot: p.slot, localTsap: p.localTsap, remoteTsap: p.remoteTsap }); }
            }} />
          <NumberField label="机架 (Rack)" min={0} max={7} value={numberValue(c.s7.rack)} onValueChange={(v) => setS7({ rack: num(v) })} />
          <NumberField label="插槽 (Slot)" min={0} max={31} value={numberValue(c.s7.slot)} onValueChange={(v) => setS7({ slot: num(v) })} />
          <Select label="连接类型" value={c.s7.connectionType} onValueChange={(v) => setS7({ connectionType: v as S7ConnectionType })}
            options={[{ value: "pg", label: "PG" }, { value: "op", label: "OP" }, { value: "basic", label: "S7 Basic" }]} />
          <Select label="PDU 长度" value={String(c.s7.pduSize)} onValueChange={(v) => setS7({ pduSize: Number(v) })}
            options={[240, 480, 960].map((n) => ({ value: String(n), label: String(n) }))} />
          <HexInput label="本地 TSAP（留空自动）" value={c.s7.localTsap} optional placeholder="0x0100" onChange={localTsap => setS7({ localTsap })} />
          <HexInput label="远端 TSAP（留空按机架/插槽）" value={c.s7.remoteTsap} optional placeholder="自动" onChange={remoteTsap => setS7({ remoteTsap })} />
        </div>
      </section>
      <section className="plc-config-card" aria-labelledby="plc-timing-title">
        <h4 id="plc-timing-title">通讯时序</h4>
        <div className="plc-config-fields">
          <NumberField label="通讯超时 (ms)" min={100} value={numberValue(c.timeoutMs)} onValueChange={(v) => set({ timeoutMs: num(v) })} />
          <NumberField label="轮询周期 (ms)" min={20} value={numberValue(c.pollIntervalMs)} onValueChange={(v) => set({ pollIntervalMs: num(v) })} />
          <NumberField label="重连间隔 (ms)" min={0} value={numberValue(c.reconnectIntervalMs)} onValueChange={(v) => set({ reconnectIntervalMs: num(v) })} />
        </div>
      </section>
      {error && <Alert tone="ng">{error}</Alert>}
    </div>
  );
}
