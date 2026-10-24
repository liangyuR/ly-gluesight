import type {
  ConnectionConfig,
  DataType,
  EdgeMode,
  LinkState,
  LogCategory,
  LogLevel,
  PlcConfig,
  PlcPoint,
  PointValue,
  S7Options,
  WordOrder,
} from "./types";

export const s7Protocol = {
  label: "西门子 S7",
  defaultPort: 102,
  defaultOrder: "ABCD" as WordOrder,
  examples: ["DB1.DBW0", "DB1.DBX2.3", "DB1.8", "DB1.DBB100.50", "M10.0", "MW20", "I0.1", "QD4", "VW100"],
  hint: "字节寻址，Bool 需带位号 (.0-.7)；字符串写成 DB1.DBB100.50（起始字节.最大长度）；V 区等价 DB1（200 SMART）；1200/1500 需启用 PUT/GET 并取消 DB 优化访问",
};

export const s7CpuPresets = [
  { key: "s71200", label: "S7-1200", rack: 0, slot: 1, localTsap: null, remoteTsap: null },
  { key: "s71500", label: "S7-1500", rack: 0, slot: 1, localTsap: null, remoteTsap: null },
  { key: "s7300", label: "S7-300", rack: 0, slot: 2, localTsap: null, remoteTsap: null },
  { key: "s7400", label: "S7-400", rack: 0, slot: 3, localTsap: null, remoteTsap: null },
  { key: "s7200smart", label: "S7-200 SMART", rack: 0, slot: 1, localTsap: 0x0102, remoteTsap: 0x0201 },
] as const;

type CpuPreset = (typeof s7CpuPresets)[number];
export const matchesCpu = (preset: CpuPreset, s7: S7Options) =>
  preset.rack === s7.rack && preset.slot === s7.slot && preset.localTsap === s7.localTsap && preset.remoteTsap === s7.remoteTsap;
/** 连接参数对应的 CPU 预设名；参数相同的预设（如 1200/1500）并列，不匹配任何预设时为“自定义”。 */
export const cpuPresetLabel = (connection: ConnectionConfig) =>
  s7CpuPresets.filter(preset => matchesCpu(preset, connection.s7)).map(preset => preset.label).join(" / ") || "自定义";

export const dataTypeLabels: Record<DataType, string> = {
  bool: "Bool",
  u8: "UInt8 (Byte)",
  i8: "Int8",
  u16: "UInt16 (Word)",
  i16: "Int16 (Int)",
  u32: "UInt32 (DWord)",
  i32: "Int32 (DInt)",
  f32: "Float32 (Real)",
  f64: "Float64 (LReal)",
  string: "String (S7 STRING)",
  smartString: "String (200 SMART)",
  bytes: "Bytes",
};

export function isText(dataType: DataType) {
  return dataType === "string" || dataType === "smartString";
}

export const edgeLabels: Record<EdgeMode, string> = {
  none: "无",
  rising: "上升沿",
  falling: "下降沿",
  both: "双边沿",
};

export const levelLabels: Record<LogLevel, string> = {
  info: "信息",
  warn: "警告",
  error: "错误",
};

export const categoryLabels: Record<LogCategory, string> = {
  connection: "连接",
  value: "值变化",
  write: "写入",
  edge: "边沿",
  error: "读取异常",
  config: "配置",
};

export const linkStateLabels: Record<LinkState, string> = {
  disconnected: "未连接",
  connecting: "连接中",
  connected: "已连接",
  error: "异常",
};

export function isMultiWord(dataType: DataType) {
  return ["u32", "i32", "f32", "f64"].includes(dataType);
}

export function effectiveOrder(point: PlcPoint): WordOrder {
  return point.wordOrder ?? s7Protocol.defaultOrder;
}

export function asS7(config: PlcConfig): PlcConfig {
  if (config.connection.protocol === "s7") return config;
  return { ...config, connection: { ...config.connection, protocol: "s7", port: s7Protocol.defaultPort } };
}

export function formatValue(v: PointValue | undefined) {
  if (!v) return "--";
  if (v.error) return v.error;
  if (v.value === null) return "--";
  if (typeof v.value === "boolean") return v.value ? "1" : "0";
  if (Array.isArray(v.value)) return v.value.map(byte => byte.toString(16).padStart(2, "0")).join(" ").toUpperCase();
  return String(v.value);
}

export function newPoint(): PlcPoint {
  return {
    id: `p_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
    name: "",
    address: "",
    dataType: "u16",
    wordOrder: null,
    access: "read",
    edge: "none",
    poll: true,
    logChanges: true,
    tags: [],
    description: "",
  };
}
