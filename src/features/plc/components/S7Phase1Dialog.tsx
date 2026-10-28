import { useEffect, useState } from "react";
import { plcApi } from "../api";
import { dataTypeLabels, s7CpuPresets } from "../meta";
import type { PlcConfig, PlcPoint } from "../types";
import { Alert, Button, Checkbox, Grid, NumberField, Select, Table, type TableColumn } from "qing-steel";
import Modal from "./Modal";

interface S7Phase1DialogProps {
  config: PlcConfig;
  dirty: boolean;
  blockedReason: string;
  onApply: (config: PlcConfig, cpu: string) => void;
  onClose: () => void;
}

const cpuChoices = ["s71200", "s71500"] as const;
const previewColumns: TableColumn<PlcPoint>[] = [
  { key: "name", header: "名称 / 标签", render: point => <>{point.name}<div className="cell-sub">{point.tags.join(" · ")}</div></> },
  { key: "address", header: "地址", render: point => <span className="mono">{point.address}</span> },
  { key: "dataType", header: "类型", render: point => dataTypeLabels[point.dataType] },
  { key: "access", header: "权限", render: point => point.access === "read" ? "PLC→PC 只读" : "PC→PLC 读写" },
];
/** 生产通讯协议 8：PLC 写 DB4，PC 写 DB2。 */
const defaultDb = { plc: 4, pc: 2 };

const dbOf = (config: PlcConfig, tag: string) => {
  const address = config.points.find(point => point.tags.includes(tag))?.address ?? "";
  const value = Number(/^DB(\d+)\./i.exec(address)?.[1]);
  return Number.isInteger(value) && value >= 1 && value <= 65535 ? value : null;
};

/** 沿用当前点表里两个区的 DB 号；缺失或两区共用一个 DB（协议 4 的旧点表）时取协议默认值。 */
function initialDbs(config: PlcConfig) {
  const plc = dbOf(config, "plcState");
  const pc = dbOf(config, "pcState");
  return plc && pc && plc !== pc ? { plc: String(plc), pc: String(pc) } : { plc: String(defaultDb.plc), pc: String(defaultDb.pc) };
}

const validDb = (text: string) => /^\d+$/.test(text) && Number(text) >= 1 && Number(text) <= 65535;

export default function S7Phase1Dialog({ config, dirty, blockedReason, onApply, onClose }: S7Phase1DialogProps) {
  const [dbText, setDbText] = useState(() => initialDbs(config));
  const [cpu, setCpu] = useState<string>("s71200");
  const [preview, setPreview] = useState<{ plcDb: number; pcDb: number; config: PlcConfig } | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [retry, setRetry] = useState(0);
  const plcDb = Number(dbText.plc);
  const pcDb = Number(dbText.pc);
  const invalid = !validDb(dbText.plc) || !validDb(dbText.pc) ? "DB 号需为 1–65535 的整数"
    : plcDb === pcDb ? "PLC 区与 PC 区必须使用两个不同的 DB" : "";
  const replaces = config.points.length > 0 || dirty;
  const ready = !invalid && preview?.plcDb === plcDb && preview.pcDb === pcDb && !loading && !error;

  useEffect(() => {
    let alive = true;
    setPreview(null);
    setError("");
    setLoading(!invalid);
    if (invalid) return;
    const timer = setTimeout(() => {
      plcApi.s7Phase1Template(plcDb, pcDb).then(template => {
        if (alive) setPreview({ plcDb, pcDb, config: template });
      }).catch(reason => { if (alive) setError(String(reason)); })
        .finally(() => { if (alive) setLoading(false); });
    }, 200);
    return () => { alive = false; clearTimeout(timer); };
  }, [plcDb, pcDb, invalid, retry]);

  const apply = () => {
    if (blockedReason || !ready || !preview || (replaces && !confirmed)) return;
    const preset = s7CpuPresets.find(item => item.key === cpu)!;
    const next = structuredClone(config);
    next.connection = {
      ...next.connection,
      protocol: "s7",
      port: preview.config.connection.port,
      s7: { ...preview.config.connection.s7, rack: preset.rack, slot: preset.slot,
        localTsap: preset.localTsap, remoteTsap: preset.remoteTsap },
    };
    next.points = structuredClone(preview.config.points);
    next.heartbeat = structuredClone(preview.config.heartbeat);
    onApply(next, cpu);
  };

  const setDb = (key: "plc" | "pc") => (value: number | "") => {
    setDbText(previous => ({ ...previous, [key]: value === "" ? "" : String(value) }));
    setConfirmed(false);
  };

  return <Modal title="重置 S7 通讯配置" width={800} onClose={onClose} footer={<>
    <Button onClick={onClose}>取消</Button>
    <Button variant="filled" disabled={!!blockedReason || !ready || (replaces && !confirmed)} onClick={apply}>应用到草稿</Button>
  </>}>
    {blockedReason && <Alert tone="warn">{blockedReason}</Alert>}
    <fieldset disabled={!!blockedReason} style={{ border: 0, padding: 0, margin: 0 }}>
      <Grid minWidth={220} gap={14}>
        <Select label="CPU 预设" value={cpu} onValueChange={value => { setCpu(value); setConfirmed(false); }}
          options={cpuChoices.map(key => ({ value: key, label: s7CpuPresets.find(item => item.key === key)!.label }))} />
        <NumberField label="PLC 区 DB（PLC 写）" min={1} max={65535} step={1} value={dbText.plc === "" ? "" : Number(dbText.plc)}
          aria-invalid={!!invalid} onValueChange={setDb("plc")} />
        <NumberField label="PC 区 DB（PC 写）" min={1} max={65535} step={1} value={dbText.pc === "" ? "" : Number(dbText.pc)}
          aria-invalid={!!invalid} onValueChange={setDb("pc")} />
      </Grid>
    </fieldset>
    <p className="muted hint">生产通讯协议 8：默认 PLC 写 DB4、PC 写 DB2，各 68 字节，双方以状态字握手；S7-1200 / 1500 采用机架 0 / 插槽 1。请在 PLC 中启用 PUT/GET，并关闭所用 DB 的优化访问；按点表建立数据块，sn 与 snEcho 声明为 String[50]。</p>
    <p className="muted hint">将生产点表、心跳及 S7 连接预设恢复为标准配置。保留 IP / 主机名、超时、轮询、重连、日志和自动连接设置；应用后需保存配置。</p>
    {invalid && <Alert tone="ng">{invalid}</Alert>}
    {loading && <p role="status">正在生成点表预览…</p>}
    {error && <Alert tone="ng" action={<Button size="sm" onClick={() => setRetry(value => value + 1)}>重试</Button>}>{error}</Alert>}
    {ready && preview && <>
      <p className="hint">PLC 区 DB{plcDb} · PC 区 DB{pcDb} · {preview.config.points.length} 个点位 · {s7CpuPresets.find(item => item.key === cpu)?.label}</p>
      <Table<PlcPoint> caption="点表预览" rows={preview.config.points} columns={previewColumns} rowKey={point => point.id} />
    </>}
    {replaces && <div style={{ marginTop: 16 }}><Checkbox label={`确认重置当前 ${config.points.length} 个点位、心跳和 S7 连接预设${dirty ? "（当前草稿尚未保存）" : ""}`} checked={confirmed} disabled={!!blockedReason} onCheckedChange={setConfirmed} /></div>}
  </Modal>;
}
