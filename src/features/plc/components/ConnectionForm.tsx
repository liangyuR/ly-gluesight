import { useEffect, useRef, useState } from "react";
import { protocols, s7CpuPresets } from "../meta";
import type { ConnectionConfig, McOptions, ModbusOptions, ProtocolKind, S7ConnectionType, S7Options } from "../types";

interface ConnectionFormProps {
  value: ConnectionConfig;
  initialCpuPreset?: string;
  onChange: (value: ConnectionConfig) => void;
}

const hex = (n: number | null) => (n === null || !Number.isFinite(n) ? "" : `0x${n.toString(16).toUpperCase().padStart(4, "0")}`);
const numeric = (text: string) => text.trim() ? Number(text) : NaN;
const numberValue = (n: number) => Number.isFinite(n) ? n : "";
const integerIn = (n: number, min: number, max = Number.MAX_SAFE_INTEGER) => Number.isSafeInteger(n) && n >= min && n <= max;

function parseHex(text: string, optional: boolean): number | null {
  const t = text.trim();
  if (!t && optional) return null;
  if (!/^(?:0x)?[\da-f]+$/i.test(t)) return NaN;
  const n = Number(`0x${t.replace(/^0x/i, "")}`);
  return integerIn(n, 0, 0xffff) ? n : NaN;
}

function HexInput({ value, optional = false, placeholder, onChange }: {
  value: number | null; optional?: boolean; placeholder?: string; onChange: (n: number | null) => void;
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
  return <>
    <input className="input mono" value={text} placeholder={placeholder} aria-invalid={invalid} onChange={e => change(e.target.value)} onBlur={e => {
      const n = parseHex(e.target.value, optional);
      if (n === null || Number.isFinite(n)) { emitted.current = n; onChange(n); setText(hex(n)); }
    }} />
    {invalid && <span className="c-ng">请输入 0x0000–0xFFFF 的完整十六进制值{optional ? "，或留空自动" : ""}</span>}
  </>;
}

export function connectionError(c: ConnectionConfig) {
  if (c.protocol !== "simulator" && !c.host.trim()) return "请填写 PLC 地址";
  if (c.protocol !== "simulator" && !integerIn(c.port, 1, 65535)) return "端口需为 1–65535 的整数";
  if (!integerIn(c.timeoutMs, 100)) return "通讯超时需为不小于 100 ms 的整数";
  if (!integerIn(c.pollIntervalMs, 20)) return "轮询周期需为不小于 20 ms 的整数";
  if (!integerIn(c.reconnectIntervalMs, 0)) return "重连间隔需为非负整数";
  if (!integerIn(c.modbus.unitId, 0, 255)) return "Modbus 站号需为 0–255 的整数";
  if (!integerIn(c.s7.rack, 0, 7) || !integerIn(c.s7.slot, 0, 31)) return "S7 机架需为 0–7、插槽需为 0–31 的整数";
  if (!integerIn(c.s7.pduSize, 240, 960)) return "S7 PDU 长度需为 240–960 的整数";
  if ([c.s7.localTsap, c.s7.remoteTsap].some(n => n !== null && !integerIn(n, 0, 0xffff))) return "S7 TSAP 需为完整的 16 位十六进制值，留空自动";
  if (![c.mc.networkNo, c.mc.pcNo, c.mc.moduleStation].every(n => integerIn(n, 0, 255))) return "MC 网络号、PC 号和站号需为 0–255 的整数";
  if (!integerIn(c.mc.moduleIo, 0, 0xffff)) return "MC 模块 IO 号需为完整的 16 位十六进制值";
  return "";
}

export default function ConnectionForm({ value: c, initialCpuPreset, onChange }: ConnectionFormProps) {
  const [cpuChoice, setCpuChoice] = useState<string | null>(initialCpuPreset ?? null);
  const set = (patch: Partial<ConnectionConfig>) => onChange({ ...c, ...patch });
  const setModbus = (patch: Partial<ModbusOptions>) => set({ modbus: { ...c.modbus, ...patch } });
  const setS7 = (patch: Partial<S7Options>) => set({ s7: { ...c.s7, ...patch } });
  const setMc = (patch: Partial<McOptions>) => set({ mc: { ...c.mc, ...patch } });
  const isSim = c.protocol === "simulator";
  const num = numeric;

  const changeProtocol = (protocol: ProtocolKind) =>
    set({ protocol, port: protocols[protocol].defaultPort });

  const matchesCpu = (p: (typeof s7CpuPresets)[number]) => p.rack === c.s7.rack && p.slot === c.s7.slot && p.localTsap === c.s7.localTsap && p.remoteTsap === c.s7.remoteTsap;
  const cpu = s7CpuPresets.find(p => p.key === cpuChoice && matchesCpu(p)) ?? s7CpuPresets.find(matchesCpu);

  return (
    <div className="form-grid">
      <label className="field">
        <span>协议</span>
        <select className="input" value={c.protocol} onChange={(e) => changeProtocol(e.target.value as ProtocolKind)}>
          {Object.entries(protocols).map(([k, v]) => (
            <option key={k} value={k}>
              {v.label}
            </option>
          ))}
        </select>
      </label>
      <label className="field">
        <span>IP / 主机名</span>
        <input className="input mono" disabled={isSim} value={c.host} onChange={(e) => set({ host: e.target.value })} />
      </label>
      <label className="field">
        <span>端口</span>
        <input className="input mono" type="number" min={1} max={65535} step={1} disabled={isSim} value={numberValue(c.port)} aria-invalid={!isSim && !integerIn(c.port, 1, 65535)} onChange={(e) => set({ port: num(e.target.value) })} />
      </label>

      {c.protocol === "modbusTcp" && (
        <label className="field">
          <span>站号 (Unit ID)</span>
          <input className="input mono" type="number" min={0} max={255} value={numberValue(c.modbus.unitId)} onChange={(e) => setModbus({ unitId: num(e.target.value) })} />
        </label>
      )}

      {c.protocol === "s7" && (
        <>
          <label className="field">
            <span>CPU 型号</span>
            <select
              className="input"
              value={cpu?.key ?? "custom"}
              onChange={(e) => {
                const p = s7CpuPresets.find((x) => x.key === e.target.value);
                if (p) { setCpuChoice(p.key); setS7({ rack: p.rack, slot: p.slot, localTsap: p.localTsap, remoteTsap: p.remoteTsap }); }
              }}
            >
              {s7CpuPresets.map((p) => (
                <option key={p.key} value={p.key}>
                  {p.label}
                </option>
              ))}
              <option value="custom" disabled>
                自定义
              </option>
            </select>
          </label>
          <label className="field">
            <span>机架 (Rack)</span>
            <input className="input mono" type="number" min={0} max={7} value={numberValue(c.s7.rack)} onChange={(e) => setS7({ rack: num(e.target.value) })} />
          </label>
          <label className="field">
            <span>插槽 (Slot)</span>
            <input className="input mono" type="number" min={0} max={31} value={numberValue(c.s7.slot)} onChange={(e) => setS7({ slot: num(e.target.value) })} />
          </label>
          <label className="field">
            <span>连接类型</span>
            <select className="input" value={c.s7.connectionType} onChange={(e) => setS7({ connectionType: e.target.value as S7ConnectionType })}>
              <option value="pg">PG</option>
              <option value="op">OP</option>
              <option value="basic">S7 Basic</option>
            </select>
          </label>
          <label className="field">
            <span>PDU 长度</span>
            <select className="input" value={c.s7.pduSize} onChange={(e) => setS7({ pduSize: num(e.target.value) })}>
              {[240, 480, 960].map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>本地 TSAP（留空自动）</span>
            <HexInput value={c.s7.localTsap} optional placeholder="0x0100" onChange={localTsap => setS7({ localTsap })} />
          </label>
          <label className="field">
            <span>远端 TSAP（留空按机架/插槽）</span>
            <HexInput value={c.s7.remoteTsap} optional placeholder="自动" onChange={remoteTsap => setS7({ remoteTsap })} />
          </label>
        </>
      )}

      {c.protocol === "mc" && (
        <>
          <label className="field">
            <span>网络号</span>
            <input className="input mono" type="number" min={0} max={255} value={numberValue(c.mc.networkNo)} onChange={(e) => setMc({ networkNo: num(e.target.value) })} />
          </label>
          <label className="field">
            <span>PC 号</span>
            <input className="input mono" type="number" min={0} max={255} value={numberValue(c.mc.pcNo)} onChange={(e) => setMc({ pcNo: num(e.target.value) })} />
          </label>
          <label className="field">
            <span>目标模块 IO 号</span>
            <HexInput value={c.mc.moduleIo} placeholder="0x03FF" onChange={moduleIo => setMc({ moduleIo: moduleIo ?? NaN })} />
          </label>
          <label className="field">
            <span>目标模块站号</span>
            <input className="input mono" type="number" min={0} max={255} value={numberValue(c.mc.moduleStation)} onChange={(e) => setMc({ moduleStation: num(e.target.value) })} />
          </label>
        </>
      )}

      <label className="field">
        <span>通讯超时 (ms)</span>
        <input className="input mono" type="number" min={100} value={numberValue(c.timeoutMs)} onChange={(e) => set({ timeoutMs: num(e.target.value) })} />
      </label>
      <label className="field">
        <span>轮询周期 (ms)</span>
        <input className="input mono" type="number" min={20} value={numberValue(c.pollIntervalMs)} onChange={(e) => set({ pollIntervalMs: num(e.target.value) })} />
      </label>
      <label className="field">
        <span>重连间隔 (ms)</span>
        <input className="input mono" type="number" min={0} value={numberValue(c.reconnectIntervalMs)} onChange={(e) => set({ reconnectIntervalMs: num(e.target.value) })} />
      </label>

      {c.protocol === "mc" && (
        <label className="check field-check">
          <input type="checkbox" checked={c.mc.xyOctal} onChange={(e) => setMc({ xyOctal: e.target.checked })} />
          <span>X/Y 使用八进制编号（iQ-F / FX5）</span>
        </label>
      )}
      {connectionError(c) && <p role="alert" className="c-ng">{connectionError(c)}</p>}
    </div>
  );
}
