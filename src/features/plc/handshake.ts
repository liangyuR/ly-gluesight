import type { PlcHandshakeState } from "./types";

type Phase = PlcHandshakeState["phase"];

export const handshakePhaseLabels: Record<Phase, string> = {
  resetRequired: "待复位", idle: "空闲", validating: "校验请求", acquiring: "采集中",
  draining: "等待帧处理结束", awaitAck: "等待结果确认", releasing: "等待握手释放", fault: "故障",
};

/** 协议 7 §4 时序、§5 迁移与 §6 复位规则推导出的“现在在等谁”。 */
const waitingTexts: Record<Phase, string> = {
  resetRequired: "PC 启动或重连后需复位：在在线检测页点“软件复位”（PC 置 pcState=99，等 PLC 回 plcState=0 后 PC 置 10），或由 PLC 置 plcState=99 发起",
  idle: "PLC：先写 requestSeq、productCode（型号）与 sn，最后置 plcState=10",
  validating: "PC：核对请求并布防相机，写 seqEcho=requestSeq，再置 pcState=20（也可写 90/95 后直接置 30 拒收）",
  acquiring: "PLC：给 MotionPermit，运动期间保持 Line0 高电平，相机按设定帧率采集；运动结束先拉低 Line0，再置 plcState=20",
  draining: "PC：收尾收帧、等待测量完成，写 resultCode、faultCode 与 snEcho，最后置 pcState=30",
  awaitAck: "PLC：核对 seqEcho=requestSeq、snEcho=sn 与结果组合，锁存结果后置 plcState=30",
  releasing: "PC 置 pcState=10，等待 PLC 清本件并回 plcState=0",
  fault: "按 PLC 原因码处置现场后复位：在线检测页“软件复位”，或由 PLC 置 plcState=99 发起",
};

export function handshakeWaiting({ phase, message }: Pick<PlcHandshakeState, "phase" | "message">) {
  if (phase === "idle" && message?.startsWith("S7 空闲：")) return message;
  if (phase === "fault") return `${waitingTexts.fault}。原因：${message || "未提供"}`;
  return waitingTexts[phase];
}

/** 协议 7 §1 心跳失活阈值 */
export const heartbeatStaleMs = 3000;

/** 协议 7 §3 双方状态字与 PLC 原因码 */
export const plcStateNames: Record<number, string> = {
  0: "空闲", 10: "请求检测", 20: "运动结束", 30: "已确认结果", 90: "故障", 99: "复位请求",
};
export const pcStateNames: Record<number, string> = {
  0: "未就绪", 10: "就绪", 20: "已布防", 30: "结果有效", 90: "故障", 99: "复位请求",
};
export const plcReasonNames: Record<number, string> = {
  0: "无", 4: "现场互锁不满足", 5: "协议或状态非法", 6: "PC 心跳失活", 7: "运动超时", 8: "等待 PC 结果超时",
  9: "现场执行故障", 10: "等待 PC 回就绪超时", 11: "等待 PC 布防超时", 12: "序号耗尽", 13: "复位条件不满足，拒绝软件复位",
  14: "在途中 PC 故障或未就绪",
};
export const pcModeNames: Record<number, string> = { 0: "正常", 1: "维护" };

export const codeName = (names: Record<number, string>, value: number) => names[value] ?? `未知（${value}）`;

/** 协议 7 §7 结果码与异常码 */
const resultCodeNames: Record<number, string> = {
  0: "无结论（维护模式）", 1: "OK", 2: "OK_WITH_EXCURSION", 11: "NG_POSITION", 12: "NG_ABSOLUTE", 13: "NG_GAP", 14: "NG_WIDTH", 90: "ERR_INSPECT",
};
const faultCodeNames: Record<number, string> = {
  // 91 / 94 / 96：段配方的含义在前；旧拍照点配方仍按旧含义产生这三个码，括号里保留，避免现场按新含义误判。
  91: "整件无图（旧配方：缺帧）", 92: "定位失败", 93: "无效测量点", 94: "预期总数与计划不符（旧配方：拍照点数不符）", 95: "配方不可用", 96: "保留未用（旧配方：多余帧）",
  97: "运动结束超时", 98: "设备丢失", 99: "处理异常", 100: "计划版本、设备绑定或计划内容与 PC 不一致",
};

export function resultCodeLabel(resultCode: number, faultCode: number | null) {
  const result = `${resultCode} ${resultCodeNames[resultCode] ?? "未知结果码"}`;
  if (!faultCode) return result;
  return `${result} · ${faultCode} ${faultCodeNames[faultCode] ?? "未知异常码"}`;
}
