import { phaseOccupied } from "../cycle/phase";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type { CapturePlcStatus } from "../workspace/types";
import type {
  ConnectionConfig,
  DataType,
  HistorySample,
  LogLabels,
  LogPage,
  LogQuery,
  PlcConfig,
  PlcHandshakeState,
  PlcOperationState,
  PlcStatus,
  PointValue,
} from "./types";

const previewConfig: PlcConfig = {
  connection: {
    protocol: "s7",
    host: "192.168.2.10",
    port: 102,
    timeoutMs: 1000,
    pollIntervalMs: 200,
    reconnectIntervalMs: 3000,
    s7: { rack: 0, slot: 1, connectionType: "pg", localTsap: 0x0102, remoteTsap: 0x0201, pduSize: 240 },
  },
  points: [],
  heartbeat: { pointId: null, intervalMs: 1000 },
  logRetentionDays: 30,
  autoConnect: false,
};

const previewStatus: PlcStatus = {
  state: "disconnected",
  message: "浏览器预览模式，PLC 功能需在桌面端运行",
  since: Date.now(),
  lastPoll: null,
  cycleMs: null,
  pollCount: 0,
  errorCount: 0,
  dials: 0,
  outageDials: 0,
  lastDialError: null,
  lastError: null,
};

function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: () => T): Promise<T> {
  if (!isTauri() && ["plc_save_config","plc_connect","plc_disconnect","plc_write_point"].includes(cmd))
    return Promise.reject(new Error("PLC 操作需要 GlueSight · 胶路智检 桌面后端"));
  if (!isTauri()) return Promise.resolve(fallback());
  return invoke<T>(cmd, args);
}

export const plcApi = {
  getConfig: () => call<PlcConfig>("plc_get_config", undefined, () => structuredClone(previewConfig)),
  saveConfig: (config: PlcConfig) => call<void>("plc_save_config", { config }, () => undefined),
  connect: () => call<void>("plc_connect", undefined, () => undefined),
  disconnect: () => call<void>("plc_disconnect", undefined, () => undefined),
  getStatus: () => call<PlcStatus>("plc_get_status", undefined, () => previewStatus),
  getValues: () => call<Record<string, PointValue>>("plc_get_values", undefined, () => ({})),
  writePoint: (id: string, value: unknown) => call<void>("plc_write_point", { id, value }, () => undefined),
  queryLogs: (query: LogQuery) => call<LogPage>("plc_query_logs", { query }, () => ({ total: 0, items: [] })),
  exportLogs: (query: LogQuery, labels: LogLabels) =>
    call<string>("plc_export_logs", { query, labels }, () => { throw new Error("导出 PLC 日志需要桌面后端"); }),
  reveal: (path: string) => call<void>("reveal_path", { path }, () => undefined),
  pointHistory: (pointId: string, start: number, end: number) =>
    call<HistorySample[]>("plc_point_history", { pointId, start, end }, () => []),
  checkAddress: (connection: ConnectionConfig, address: string, dataType: DataType) =>
    call<string>("plc_check_address", { connection, address, dataType }, () => "预览模式不校验"),
  s7Phase1Template: (plcDb: number, pcDb: number) =>
    call<PlcConfig>("plc_s7_phase1_template", { plcDb, pcDb }, () => { throw new Error("重置 S7 通讯配置需要桌面后端"); }),
  getOperationState: () => call<PlcOperationState | null>("cycle_snapshot", undefined, () => null),
  getTeachingStatus: () => call<CapturePlcStatus | null>("recipe_capture_plc_status", undefined, () => null),
};

export function useTeachingStatus(link: PlcStatus | null) {
  const [result, setResult] = useState<{ status: CapturePlcStatus | null; error: string; connection: string }>({ status: null, error: "", connection: "" });
  const connection = link?.state === "connected" ? `${link.since}:${link.dials}` : "";
  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    setResult({ status: null, error: "", connection });
    if (!connection) return;
    const poll = async () => {
      try {
        const status = await plcApi.getTeachingStatus();
        if (alive) setResult({ status, error: status ? "" : "示教状态不可用", connection });
      } catch (error) {
        if (alive) setResult({ status: null, error: `示教状态读取失败：${String(error)}`, connection });
      } finally {
        if (alive) timer = setTimeout(() => void poll(), 500);
      }
    };
    void poll();
    return () => { alive = false; clearTimeout(timer); };
  }, [connection]);
  return connection && result.connection === connection ? result : { status: null, error: "" };
}

export function usePlcOperationState() {
  const [operation, setOperation] = useState<{ blockedReason: string; handshake: PlcHandshakeState | null }>({
    blockedReason: "正在核对生产状态，暂不能更改 PLC 配置或手动写入",
    handshake: null,
  });
  useEffect(() => {
    let alive = true;
    let receivedEvent = false;
    const update = (state: PlcOperationState | null) => {
      if (!alive) return;
      setOperation({
        blockedReason: !state ? "生产状态不可用，已锁定 PLC 配置和手动写入"
          : (state.plcLocked ?? phaseOccupied(state.phase))
            ? "生产或结果交付尚未结束，PLC 配置和手动写入已锁定" : "",
        handshake: state?.plcHandshake ?? null,
      });
    };
    const off = subscribe<PlcOperationState>("cycle://snapshot", state => {
      receivedEvent = true;
      update(state);
    }, () => { receivedEvent = true; update(null); });
    plcApi.getOperationState().then(state => { if (!receivedEvent) update(state); })
      .catch(() => { if (!receivedEvent) update(null); });
    return () => { alive = false; off(); };
  }, []);
  return operation;
}

export function subscribe<T>(event: string, cb: (payload: T) => void, onError?: (error: unknown) => void): () => void {
  if (!isTauri()) return () => {};
  let unlisten: UnlistenFn | undefined;
  let disposed = false;
  listen<T>(event, (e) => cb(e.payload)).then((fn) => {
    if (disposed) fn();
    else unlisten = fn;
  }).catch(error => { if (!disposed) onError?.(error); });
  return () => {
    disposed = true;
    unlisten?.();
  };
}

export function usePlcStatus() {
  const [status, setStatus] = useState<PlcStatus | null>(null);
  useEffect(() => {
    plcApi.getStatus().then(setStatus).catch(() => setStatus(null));
    return subscribe<PlcStatus>("plc://status", setStatus);
  }, []);
  return status;
}

export function usePlcValues() {
  const [values, setValues] = useState<Record<string, PointValue>>({});
  useEffect(() => {
    plcApi.getValues().then(setValues).catch(() => setValues({}));
    return subscribe<Record<string, PointValue>>("plc://values", (changed) =>
      setValues((prev) => (Object.keys(changed).length === 0 ? {} : { ...prev, ...changed })),
    );
  }, []);
  return values;
}
