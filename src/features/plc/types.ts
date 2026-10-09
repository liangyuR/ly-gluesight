export type ProtocolKind = "modbusTcp" | "s7" | "mc" | "simulator";
export type DataType = "bool" | "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "f32" | "f64";
export type WordOrder = "ABCD" | "CDAB" | "BADC" | "DCBA";
export type Access = "read" | "readWrite";
export type EdgeMode = "none" | "rising" | "falling" | "both";
export type S7ConnectionType = "pg" | "op" | "basic";

export interface PlcPoint {
  id: string;
  name: string;
  address: string;
  dataType: DataType;
  wordOrder: WordOrder | null;
  access: Access;
  edge: EdgeMode;
  logChanges: boolean;
  tags: string[];
  description: string;
}

export interface ModbusOptions {
  unitId: number;
}

export interface S7Options {
  rack: number;
  slot: number;
  connectionType: S7ConnectionType;
  localTsap: number | null;
  remoteTsap: number | null;
  pduSize: number;
}

export interface McOptions {
  networkNo: number;
  pcNo: number;
  moduleIo: number;
  moduleStation: number;
  xyOctal: boolean;
}

export interface ConnectionConfig {
  protocol: ProtocolKind;
  host: string;
  port: number;
  timeoutMs: number;
  pollIntervalMs: number;
  reconnectIntervalMs: number;
  modbus: ModbusOptions;
  s7: S7Options;
  mc: McOptions;
}

export interface HeartbeatConfig {
  pointId: string | null;
  intervalMs: number;
}

export interface PlcConfig {
  connection: ConnectionConfig;
  points: PlcPoint[];
  heartbeat: HeartbeatConfig;
  logRetentionDays: number;
  autoConnect: boolean;
}

export type LinkState = "disconnected" | "connecting" | "connected" | "error";

export interface PlcStatus {
  state: LinkState;
  message: string;
  since: number;
  lastPoll: number | null;
  cycleMs: number | null;
  pollCount: number;
  errorCount: number;
}

export type PlcValue = boolean | number;

export interface PointValue {
  value: PlcValue | null;
  error: string | null;
  ts: number;
}

export type LogLevel = "info" | "warn" | "error";
export type LogCategory = "connection" | "value" | "write" | "edge" | "error" | "config";

export interface LogEntry {
  id: number;
  ts: number;
  level: LogLevel;
  category: LogCategory;
  pointId: string | null;
  pointName: string | null;
  message: string;
  oldValue: string | null;
  newValue: string | null;
}

export interface LogQuery {
  start?: number | null;
  end?: number | null;
  levels?: LogLevel[];
  categories?: LogCategory[];
  pointId?: string | null;
  keyword?: string | null;
  limit?: number;
  offset?: number;
}

export interface LogPage {
  total: number;
  items: LogEntry[];
}

export interface HistorySample {
  ts: number;
  value: string | null;
}

export interface EdgeEvent {
  pointId: string;
  pointName: string;
  tags: string[];
  rising: boolean;
  value: PlcValue;
  ts: number;
}

export interface TagPreset {
  value: string;
  label: string;
}

export interface PlcOperationState {
  phase: string;
  plcLocked?: boolean;
  plcHandshake?: PlcHandshakeState | null;
  activeRecipeId?: string | null;
}

export interface PlcHandshakeState {
  phase: "resetRequired" | "idle" | "validating" | "acquiring" | "draining" | "awaitAck" | "releasing" | "fault";
  requestSeq: number | null;
  resultSeq: number | null;
  message: string | null;
}

export interface PlcRecipeChoice { id: string; name: string; version: number }

export interface PlcRecipePlan {
  protocolVersion: number;
  recipeId: string;
  planVersion: number;
  planHash: number;
  shotCount: number;
  cameraSlots: [string, string, string];
  cameraShots: [number, number, number];
  /** poseId：现场机器人 / PLC 程序里的 Pose 标识，同一 Pose 可触发几台相机 */
  shots: { shotId: string; poseId: string; cameraId: string; center: [number, number] }[];
}
