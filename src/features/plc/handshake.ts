import type { PlcHandshakeState } from "./types";

type Phase = PlcHandshakeState["phase"];

export const handshakePhaseLabels: Record<Phase, string> = {
  notReady: "通讯未就绪", idle: "空闲", validating: "接件登记", acquiring: "采集中", draining: "收尾并写结果",
};

const waitingTexts: Record<Phase, string> = {
  notReady: "通讯未就绪：检查 PLC 连接、协议版本（须为 9）与 PLC 心跳，恢复后自动就绪",
  idle: "PLC：工件到达时写 productCode、sn，partSeq 加 1，最后置 inspecting=1",
  validating: "PC：读到新件后登记本件（核对配方、定帧计数基线），不回写 PLC",
  acquiring: "PLC：按机械臂 CaptureActive 门控 Line0；收到 PartDone 把 inspecting 置 0",
  draining: "PC：收尾收帧、判定，写 resultCode、faultCode、snEcho，最后写 resultSeq",
};

export function handshakeWaiting({ phase, message }: Pick<PlcHandshakeState, "phase" | "message">) {
  if (phase === "idle" && message?.startsWith("S7 空闲：")) return message;
  if (phase === "notReady" && message) return `${waitingTexts.notReady}。原因：${message}`;
  return waitingTexts[phase];
}

export const protocolVersion = 9;
export const heartbeatStaleMs = 3000;

export const pcModeNames: Record<number, string> = { 0: "正常", 1: "维护" };

export const codeName = (names: Record<number, string>, value: number) => names[value] ?? `未知（${value}）`;

const resultCodeNames: Record<number, string> = {
  0: "无结论（维护模式）", 1: "OK", 2: "OK_WITH_EXCURSION", 11: "NG_POSITION", 12: "NG_ABSOLUTE", 13: "NG_GAP", 14: "NG_WIDTH", 90: "ERR_INSPECT",
};
const faultCodeNames: Record<number, string> = {
  // 91 / 94 / 96：段配方的含义在前；旧拍照点配方仍按旧含义产生这三个码，括号里保留，避免现场按新含义误判。
  91: "整件无图（旧配方：缺帧）", 92: "定位失败", 93: "无效测量点", 94: "预期总数与计划不符（旧配方：拍照点数不符）", 95: "配方不可用", 96: "保留未用（旧配方：多余帧）",
  97: "等工件结束超时", 98: "设备丢失", 99: "处理异常", 100: "配方无法编出拍照计划",
};

export function resultCodeLabel(resultCode: number, faultCode: number | null) {
  const result = `${resultCode} ${resultCodeNames[resultCode] ?? "未知结果码"}`;
  if (!faultCode) return result;
  return `${result} · ${faultCode} ${faultCodeNames[faultCode] ?? "未知异常码"}`;
}
