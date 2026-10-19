export type ProtocolKind = "s7";
export type DataType = "bool" | "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "f32" | "f64" | "string" | "smartString";
export type SnDataType = Extract<DataType, "string" | "smartString">;
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
  poll: boolean;
  logChanges: boolean;
  tags: string[];
  description: string;
}

export interface S7Options {
  rack: number;
  slot: number;
  connectionType: S7ConnectionType;
  localTsap: number | null;
  remoteTsap: number | null;
  pduSize: number;
}

export interface ConnectionConfig {
  protocol: ProtocolKind;
  host: string;
  port: number;
  timeoutMs: number;
  pollIntervalMs: number;
  reconnectIntervalMs: number;
  s7: S7Options;
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
  dials: number;
  outageDials: number;
  lastDialError: PlcError | null;
  lastError: PlcError | null;
}

export type PlcErrorKind = "offline" | "link" | "timeout" | "rejected" | "invalid" | "decode" | "mismatch";

export interface PlcError {
  kind: PlcErrorKind;
  message: string;
}

export type PlcValue = boolean | number | string;

export interface PointValue {
  value: PlcValue | null;
  error: string | null;
  errorKind: PlcErrorKind | null;
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
  /** 记录时的关联上下文，如 requestSeq=12 */
  context: string | null;
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
  shotCount: number;
  deviceIds: string[];
  deviceShots: number;
  shots: { shotId: string; poseId: string; cameraIds: string[] }[];
}
