import type { PlcConfig, PlcPoint } from "../src/features/plc/types";

export const plcPoint: PlcPoint = { id: "speed", name: "速度", address: "HR100", dataType: "f32", wordOrder: null,
  access: "readWrite", edge: "none", logChanges: true, tags: [], description: "" };
export function plcConfig(): PlcConfig {
  return { connection: { protocol: "simulator", host: "192.168.1.10", port: 0, timeoutMs: 1000, pollIntervalMs: 200,
    reconnectIntervalMs: 3000, modbus: { unitId: 1 },
    s7: { rack: 0, slot: 1, connectionType: "pg", localTsap: null, remoteTsap: null, pduSize: 480 },
    mc: { networkNo: 0, pcNo: 255, moduleIo: 1023, moduleStation: 0, xyOctal: false } },
    points: [structuredClone(plcPoint)], heartbeat: { pointId: "speed", intervalMs: 1000 }, logRetentionDays: 30, autoConnect: false };
}
