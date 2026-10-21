import type { OfflineFault, Scenario, SiteSourceKind, SiteStatus } from "./types";

export const scenarioOptions: [Scenario, string][] = [
  ["normal", "正常件"],
  ["excursion", "局部超差（允许）"],
  ["gap", "断胶（超过允许长度）"],
  ["lostFrame", "传输丢帧 / 缺帧"],
  ["locateFail", "定位失败（胶正常）"],
  ["locateFailGap", "定位失败 + 断胶"],
  ["countMismatch", "设备触发数与计划不符"],
  ["motionTimeout", "运动超时（不发 partEnd）"],
  ["ackMismatch", "ACK 序号不匹配"],
  ["random", "随机（连续运行用）"],
];

const handshakeScenarios = new Set<Scenario>(["normal", "lostFrame", "countMismatch", "motionTimeout", "ackMismatch"]);

/** 样本集的图像是真实原图，只能模拟握手类工况；合成图可模拟全部工况。 */
export function scenariosFor(source: SiteSourceKind | null): [Scenario, string][] {
  return source === "synthetic" ? scenarioOptions : scenarioOptions.filter(([v]) => handshakeScenarios.has(v));
}

export const faultOptions: [OfflineFault, string][] = [
  ["disconnect", "断开当前 S7 连接"],
  ["heartbeatStop", "停止 PLC 心跳"],
  ["heartbeatResume", "恢复 PLC 心跳"],
  ["rejectWrite", "拒绝下一次写"],
  ["rejectRead", "拒绝下一次读"],
  ["delay", "下一次请求超时"],
  ["clear", "清除待执行故障"],
];

export const sourceLabel: Record<SiteSourceKind, string> = { sampleSet: "样本集", synthetic: "合成图" };
export const measurementLabel: Record<SiteStatus["measurement"], string> = { lyFlow: "lyFlow", simulated: "模拟测量" };

export function siteModeText(status: Pick<SiteStatus, "mode" | "source" | "sampleSet">) {
  if (status.mode !== "offline") return "真机";
  if (status.source === "sampleSet") return `离线 · 样本集${status.sampleSet ? " " + status.sampleSet.name : ""}`;
  return "离线 · 合成图";
}
