import { useEffect, useState } from "react";
import { plcApi } from "../api";
import { dataTypeLabels, s7CpuPresets, snTypeForCpu } from "../meta";
import type { PlcConfig, SnDataType } from "../types";
import Modal from "./Modal";

interface S7Phase1DialogProps {
  config: PlcConfig;
  dirty: boolean;
  blockedReason: string;
  onApply: (config: PlcConfig, cpu: string) => void;
  onClose: () => void;
}

const cpuChoices = ["s7200smart", "s71200", "s71500"] as const;
const smartDb = 1;

export default function S7Phase1Dialog({ config, dirty, blockedReason, onApply, onClose }: S7Phase1DialogProps) {
  const [dbText, setDbText] = useState(() => config.points.map(point => /^DB(\d+)\./i.exec(point.address)?.[1])
    .find(value => value && Number(value) >= 1 && Number(value) <= 65535) ?? String(smartDb));
  const [cpu, setCpu] = useState<string>("s7200smart");
  const [snType, setSnType] = useState<SnDataType>(snTypeForCpu("s7200smart"));
  const [preview, setPreview] = useState<{ db: number; snType: SnDataType; config: PlcConfig } | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [retry, setRetry] = useState(0);
  const smart = cpu === "s7200smart";
  const db = smart ? smartDb : Number(dbText);
  const invalid = !smart && (!/^\d+$/.test(dbText) || !Number.isInteger(db) || db < 1 || db > 65535);
  const replaces = config.points.length > 0 || dirty;
  const ready = !invalid && preview?.db === db && preview.snType === snType && !loading && !error;

  useEffect(() => {
    let alive = true;
    setPreview(null);
    setError("");
    setLoading(!invalid);
    if (invalid) return;
    const timer = setTimeout(() => {
      plcApi.s7Phase1Template(db, snType).then(template => {
        if (alive) setPreview({ db, snType, config: template });
      }).catch(reason => { if (alive) setError(String(reason)); })
        .finally(() => { if (alive) setLoading(false); });
    }, 200);
    return () => { alive = false; clearTimeout(timer); };
  }, [db, snType, invalid, retry]);

  const apply = () => {
    if (blockedReason || !ready || !preview || (replaces && !confirmed)) return;
    const preset = s7CpuPresets.find(item => item.key === cpu)!;
    const next = structuredClone(config);
    next.connection = {
      ...next.connection,
      protocol: "s7",
      port: preview.config.connection.port,
      s7: { ...preview.config.connection.s7, rack: preset.rack, slot: preset.slot,
        localTsap: preset.localTsap, remoteTsap: preset.remoteTsap, pduSize: smart ? 240 : 480 },
    };
    next.points = structuredClone(preview.config.points);
    next.heartbeat = structuredClone(preview.config.heartbeat);
    onApply(next, cpu);
  };

  return <Modal title="载入一期 S7 点表" width={800} onClose={onClose} footer={<>
    <button className="btn" onClick={onClose}>取消</button>
    <button className="btn primary" disabled={!!blockedReason || !ready || (replaces && !confirmed)} onClick={apply}>应用到草稿</button>
  </>}>
    {blockedReason && <p className="notice error" role="status">{blockedReason}</p>}
    <fieldset className="form-grid two" disabled={!!blockedReason} style={{ border: 0, padding: 0, margin: 0 }}>
      <label className="field"><span>一期 CPU 预设</span><select className="input" value={cpu} onChange={event => {
        setCpu(event.target.value); setSnType(snTypeForCpu(event.target.value)); setConfirmed(false);
      }}>
        {cpuChoices.map(key => <option key={key} value={key}>{s7CpuPresets.find(item => item.key === key)!.label}</option>)}
      </select></label>
      <label className="field"><span>DB 号</span><input className="input mono" type="number" min={1} max={65535} step={1} value={smart ? smartDb : dbText}
        disabled={smart} aria-invalid={invalid} onChange={event => { setDbText(event.target.value); setConfirmed(false); }} /></label>
      <label className="field"><span>工件 SN 字符串格式</span><select className="input" value={snType} onChange={event => { setSnType(event.target.value as SnDataType); setConfirmed(false); }}>
        <option value="smartString">200 SMART 字符串（1 字节实际长度）</option>
        <option value="string">S7 STRING（声明长度 + 实际长度）</option>
      </select></label>
    </fieldset>
    {smart
      ? <p className="muted hint">S7-200 SMART 的 V 区固定按 DB1 访问（VB100 即 DB1.DBB100），本地 / 远端 TSAP 0x0102 / 0x0201，PDU 240。SN 默认按 200 SMART 原生字符串编码，最多 49 个字符；PLC 若按 STRING[50] 自行写两个头字节，改选 S7 STRING。</p>
      : <p className="muted hint">S7-1200 / 1500 采用机架 0 / 插槽 1。请在 PLC 中启用 PUT/GET，并关闭所选 DB 的优化访问；按点表建立数据块，SN 声明为 String[50]。</p>}
    <p className="muted hint">应用将替换地址表、心跳和 S7 连接预设。IP / 主机名、超时、轮询、重连、日志和自动连接设置保留；之后仍需保存配置。</p>
    {invalid && <p className="c-ng" role="alert">DB 号需为 1–65535 的整数</p>}
    {loading && <p role="status">正在生成点表预览…</p>}
    {error && <p className="c-ng" role="alert">{error} <button className="btn" onClick={() => setRetry(value => value + 1)}>重试</button></p>}
    {ready && preview && <>
      <p className="hint">DB{db} · {preview.config.points.length} 个点位 · {s7CpuPresets.find(item => item.key === cpu)?.label} · SN {dataTypeLabels[snType]}</p>
      <div className="table-wrap"><table className="table"><thead><tr><th>名称 / 标签</th><th>地址</th><th>类型</th><th>权限</th></tr></thead><tbody>
        {preview.config.points.map(point => <tr key={point.id}><td>{point.name}<div className="cell-sub">{point.tags.join(" · ")}</div></td><td className="mono">{point.address}</td><td>{dataTypeLabels[point.dataType]}</td><td>{point.access === "read" ? "PLC→PC 只读" : "PC→PLC 读写"}</td></tr>)}
      </tbody></table></div>
    </>}
    {replaces && <label className="check" style={{ marginTop: 16 }}><input type="checkbox" checked={confirmed} disabled={!!blockedReason} onChange={event => setConfirmed(event.target.checked)} />
      <span>确认替换当前 {config.points.length} 个点位和心跳{dirty ? "（当前草稿尚未保存）" : ""}，并应用 S7 连接预设</span>
    </label>}
  </Modal>;
}
