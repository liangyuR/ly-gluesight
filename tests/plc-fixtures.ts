import type { PlcConfig, PlcPoint } from "../src/features/plc/types";

export const plcPoint: PlcPoint = { id: "speed", name: "速度", address: "DB1.DBD0", dataType: "f32", wordOrder: null,
  access: "readWrite", edge: "none", poll: true, logChanges: true, tags: [], description: "" };
export function plcConfig(): PlcConfig {
  return { connection: { protocol: "s7", host: "192.168.1.10", port: 102, timeoutMs: 1000, pollIntervalMs: 200,
    reconnectIntervalMs: 3000,
    s7: { rack: 0, slot: 1, connectionType: "pg", localTsap: null, remoteTsap: null, pduSize: 480 } },
    points: [structuredClone(plcPoint)], heartbeat: { pointId: "speed", intervalMs: 1000 }, logRetentionDays: 30, autoConnect: false };
}
