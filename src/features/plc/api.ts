import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type {
  ConnectionConfig,
  DataType,
  HistorySample,
  LogPage,
  LogQuery,
  PlcConfig,
  PlcStatus,
  PointValue,
} from "./types";

const previewConfig: PlcConfig = {
  connection: {
    protocol: "simulator",
    host: "192.168.0.10",
    port: 502,
    timeoutMs: 1000,
    pollIntervalMs: 200,
    reconnectIntervalMs: 3000,
    modbus: { unitId: 1 },
    s7: { rack: 0, slot: 1, connectionType: "pg", localTsap: null, remoteTsap: null, pduSize: 480 },
    mc: { networkNo: 0, pcNo: 255, moduleIo: 0x03ff, moduleStation: 0, xyOctal: false },
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
  pointHistory: (pointId: string, start: number, end: number) =>
    call<HistorySample[]>("plc_point_history", { pointId, start, end }, () => []),
  checkAddress: (connection: ConnectionConfig, address: string, dataType: DataType) =>
    call<string>("plc_check_address", { connection, address, dataType }, () => "预览模式不校验"),
};

export function subscribe<T>(event: string, cb: (payload: T) => void): () => void {
  if (!isTauri()) return () => {};
  let unlisten: UnlistenFn | undefined;
  let disposed = false;
  listen<T>(event, (e) => cb(e.payload)).then((fn) => {
    if (disposed) fn();
    else unlisten = fn;
  });
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
