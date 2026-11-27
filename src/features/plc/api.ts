import { useCycleControl, type CycleControlState } from "../cycle/control";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { createSharedSnapshot } from "../../lib/desktopSnapshot";
import type {
  ConnectionConfig,
  DataType,
  HistorySample,
  LogLabels,
  LogGroupKey,
  LogGroupPage,
  LogGroupQuery,
  LogPage,
  LogQuery,
  PlcConfig,
  PlcHandshakeState,
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
  queryLogGroups: (query: LogGroupQuery) =>
    call<LogGroupPage>("plc_query_log_groups", { query }, () => ({ total: 0, items: [] })),
  queryGroupLogs: (group: LogGroupKey, query: LogQuery) =>
    call<LogPage>("plc_query_group_logs", { group, query }, () => ({ total: 0, items: [] })),
  exportLogs: (query: LogQuery, labels: LogLabels, group?: LogGroupKey) =>
    call<string>("plc_export_logs", { query, labels, group }, () => { throw new Error("导出 PLC 日志需要桌面后端"); }),
  reveal: (path: string) => call<void>("reveal_path", { path }, () => undefined),
  pointHistory: (pointId: string, start: number, end: number, group?: LogGroupKey) =>
    call<HistorySample[]>("plc_point_history", { pointId, start, end, group }, () => []),
  checkAddress: (connection: ConnectionConfig, address: string, dataType: DataType) =>
    call<string>("plc_check_address", { connection, address, dataType }, () => "预览模式不校验"),
  s7Phase1Template: (plcDb: number, pcDb: number) =>
    call<PlcConfig>("plc_s7_phase1_template", { plcDb, pcDb }, () => { throw new Error("重置 S7 通讯配置需要桌面后端"); }),
};

interface PlcOperationView {
  blockedReason: string;
  handshake: PlcHandshakeState | null;
}

const pendingOperation: PlcOperationView = {
  blockedReason: "正在核对生产状态，暂不能更改 PLC 配置或手动写入",
  handshake: null,
};

function selectOperation(state: CycleControlState | null): PlcOperationView {
  return {
    blockedReason: !state ? "生产状态不可用，已锁定 PLC 配置和手动写入"
      : state.plcLocked !== false ? "生产或结果交付尚未结束，PLC 配置和手动写入已锁定" : "",
    handshake: state?.plcHandshake ?? null,
  };
}

function sameOperation(current: PlcOperationView, next: PlcOperationView) {
  const a = current.handshake, b = next.handshake;
  return current.blockedReason === next.blockedReason && (a === b || (
    a !== null && b !== null && a.phase === b.phase && a.requestSeq === b.requestSeq &&
    a.message === b.message && a.connection === b.connection
  ));
}

export function usePlcOperationState() {
  return useCycleControl(selectOperation, pendingOperation, sameOperation);
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

const subscribePlcStatus = createSharedSnapshot<PlcStatus>("plc://status", () => plcApi.getStatus(), { readOnListenFailure: true });

export function usePlcStatus() {
  const [status, setStatus] = useState<PlcStatus | null>(null);
  useEffect(() => subscribePlcStatus(setStatus), []);
  return status;
}

export function usePlcValues() {
  const [values, setValues] = useState<Record<string, PointValue>>({});
  useEffect(() => {
    let active = true, acceptEvents = true, resetCount = 0;
    let unlisten: UnlistenFn | undefined;
    let latest: Record<string, PointValue> = {};

    const start = async () => {
      if (isTauri()) {
        try {
          const stop = await listen<Record<string, PointValue>>("plc://values", ({ payload }) => {
            if (!active || !acceptEvents) return;
            if (Object.keys(payload).length === 0) {
              resetCount++;
              latest = {};
            } else {
              latest = { ...latest, ...payload };
            }
            setValues(latest);
          });
          if (!active) { stop(); return; }
          unlisten = stop;
        } catch {
          acceptEvents = false;
        }
      }
      if (!active) return;
      const resetCountAtRead = resetCount;
      try {
        const initial = await plcApi.getValues();
        if (active && resetCount === resetCountAtRead) {
          latest = { ...initial, ...latest };
          setValues(latest);
        }
      } catch {
        // 已收到的点位事件仍有效。
      }
    };
    void start();
    return () => { active = false; unlisten?.(); };
  }, []);
  return values;
}
