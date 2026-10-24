import type { CapturePlcStatus, TeachingRegisters } from "../workspace/types";

type TeachingSide = "pc" | "plc";

export const teachingPhaseLabels: Record<CapturePlcStatus["phase"], string> = {
  unavailable: "示教接口未就绪", resetRequired: "示教需要复位", idle: "空闲",
  requested: "等待接受", accepted: "采集中", draining: "尾帧与保存收尾",
  reporting: "等待结果确认", cancelling: "等待取消确认", releasing: "等待释放", fault: "示教握手故障",
};

const waiting: Record<CapturePlcStatus["phase"], string> = {
  unavailable: "PC：核对 DB6 / DB12 的示教点表配置及连接状态",
  resetRequired: "操作员：停止门控并处置现场运动和未结轮次，在在线检测页完成显式复位",
  idle: "操作员：核对现场产品与示教轨迹，在整圈采集页开始一轮采集",
  requested: "PLC：核对示教许可与互锁，回显本轮编号并接受或拒绝请求",
  accepted: "PLC：执行本轮运动，采集区间保持 Line0 高电平；结束后拉低并报告采集结束",
  draining: "PC：等待尾帧窗口结束及原图保存完成，再报告本轮结果",
  reporting: "PLC：锁存 PC 的完成或失败结果，停止门控并处置运动后报告已确认",
  cancelling: "PLC：停止门控、处置运动并确认取消；PC 保持本轮编号等待确认",
  releasing: "PLC：确认本轮 PC 已回空闲，释放资源并回空闲；双方空闲后才能开始下一轮",
  fault: "操作员：停止门控，核对未结轮次与故障原因，处置现场后显式复位",
};

const stateLabels: Record<TeachingSide, Record<number, string>> = {
  pc: { 0: "空闲", 10: "请求", 20: "已完成", 30: "失败", 40: "取消" },
  plc: { 0: "空闲", 10: "已接受", 20: "采集结束", 30: "已确认", 40: "已拒绝", 50: "故障" },
};

const reasonLabels: Record<TeachingSide, Record<number, string>> = {
  pc: {
    0: "无", 1: "操作员取消", 2: "等待接受超时取消", 3: "相机故障或会话变化",
    4: "保存失败", 5: "收尾处理超时", 6: "协议或通讯异常", 7: "等待采集结束超时",
  },
  plc: {
    0: "无", 1: "示教未许可", 2: "机器人未就绪", 3: "生产或其他事务占用", 4: "现场互锁不满足",
    5: "协议或状态非法", 6: "PC 心跳失活", 7: "运动超时", 8: "等待 PC 结果超时",
    9: "现场执行故障", 10: "等待释放超时",
  },
};

export const teachingStateLabel = (side: TeachingSide, value: number) => `${stateLabels[side][value] ?? "未知状态"} (${value})`;
export const teachingReasonLabel = (side: TeachingSide, value: number) => `${value} · ${reasonLabels[side][value] ?? "未知原因码"}`;
export const teachingWaiting = (status: CapturePlcStatus) => waiting[status.phase] ?? "PC：核对示教接口状态";

const registerTags: Record<string, [TeachingSide, keyof TeachingRegisters]> = {
  teachPcVersion: ["pc", "version"], teachPcState: ["pc", "state"], teachRoundId: ["pc", "roundId"],
  teachPcReason: ["pc", "reason"], teachPcReserved: ["pc", "reserved"],
  teachPlcVersion: ["plc", "version"], teachPlcState: ["plc", "state"], teachRoundIdEcho: ["plc", "roundId"],
  teachPlcReason: ["plc", "reason"], teachPlcReserved: ["plc", "reserved"],
};

export function teachingPointValue(tag: string, status: CapturePlcStatus | null, connected: boolean): number | number[] | null {
  const monitor = status?.monitor;
  if (!connected || !monitor?.fresh) return null;
  if (tag === "teachPlcSnapshot") {
    const plc = monitor.plc;
    return plc ? [plc.version, plc.state, plc.roundId, plc.reason, plc.reserved].flatMap(value => [(value >> 8) & 255, value & 255]) : null;
  }
  const register = registerTags[tag];
  return register ? monitor[register[0]]?.[register[1]] ?? null : null;
}
