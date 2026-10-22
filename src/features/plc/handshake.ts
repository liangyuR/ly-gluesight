import type { PlcHandshakeState } from "./types";

type Phase = PlcHandshakeState["phase"];

export const handshakePhaseLabels: Record<Phase, string> = {
  resetRequired: "待复位", idle: "空闲", validating: "校验请求", acquiring: "采集中",
  draining: "等待帧处理结束", awaitAck: "等待结果确认", releasing: "等待握手释放", fault: "故障",
};

/** 协议 §5 阶段表与 §8 复位流程推导出的“现在在等谁”。 */
const waitingTexts: Record<Phase, string> = {
  resetRequired: "PLC：先把 partStart/partEnd/resultAck 置 0 并保持 faultReset 低 ≥1 s，再发 faultReset 上升沿；也可在在线检测页点“复位故障”",
  idle: "PLC：写 requestSeq、partSn、productCode、shotCount 与 deviceShots（预期总帧数 ΣN_exp）、planVersion，最后置 partStart",
  validating: "PC：核对请求、撤 visionReady、写 acceptedSeq，置 busy，最后置 armed（PLC 此时禁止触发）",
  acquiring: "PLC：相机使能为高时连续发脉冲（deviceTriggers 只作参考，不要求等于预期总帧数），运动结束、脉冲停发后写最终计数并置 partEnd",
  draining: "PC：收尾收帧、等待测量完成，写结果码/异常码/resultSn/resultSeq，最后置 done",
  awaitAck: "PLC：核对 resultSeq=requestSeq、resultSn=partSn 与结果码，写 ackSeq，再置 resultAck",
  releasing: "PLC：看到 done=busy=0 后清 partStart/partEnd/resultAck；PLC 清完后 PC 落盘并置 visionReady",
  fault: "PLC：停止触发并处置实物；PC 不会自行恢复，需 faultReset 上升沿或在线检测页复位",
};

export function handshakeWaiting({ phase, message }: Pick<PlcHandshakeState, "phase" | "message">) {
  if (phase === "idle" && message?.startsWith("S7 空闲：")) return message;
  if (phase === "fault") return `${waitingTexts.fault}。原因：${message || "未提供"}`;
  return waitingTexts[phase];
}

export const plcToPcBits = ["partStart", "partEnd", "resultAck", "faultReset", "plcHeartbeat"] as const;
export const pcToPlcBits = ["visionReady", "armed", "busy", "done", "visionFault", "pcHeartbeat"] as const;
export const heartbeatTags = new Set<string>(["plcHeartbeat", "pcHeartbeat"]);
/** 协议 §1 心跳失活阈值 */
export const heartbeatStaleMs = 3000;

/** 协议 §6 结果码与异常码 */
const resultCodeNames: Record<number, string> = {
  1: "OK", 2: "OK_WITH_EXCURSION", 11: "NG_POSITION", 12: "NG_ABSOLUTE", 13: "NG_GAP", 14: "NG_WIDTH", 90: "ERR_INSPECT",
};
const faultCodeNames: Record<number, string> = {
  // 91 / 94 / 96：协议 6 段配方的含义在前；旧拍照点配方仍按旧含义产生这三个码，括号里保留，避免现场按新含义误判。
  91: "整件无图（旧配方：缺帧）", 92: "定位失败", 93: "无效测量点", 94: "预期总数与计划不符（旧配方：拍照点数不符）", 95: "配方不可用", 96: "保留未用（旧配方：多余帧）",
  97: "运动结束超时", 98: "设备丢失", 99: "处理异常", 100: "计划版本、设备绑定或计划内容与 PC 不一致",
};

export function resultCodeLabel(resultCode: number, faultCode: number | null) {
  const result = `${resultCode} ${resultCodeNames[resultCode] ?? "未知结果码"}`;
  if (!faultCode) return result;
  return `${result} · ${faultCode} ${faultCodeNames[faultCode] ?? "未知异常码"}`;
}
