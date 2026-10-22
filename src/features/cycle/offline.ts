import type { OfflineFault, Scenario, SiteSourceKind, SiteStatus } from "./types";

export const scenarioOptions: [Scenario, string][] = [
  ["normal", "正常件"],
  ["excursion", "局部超差（允许）"],
  ["gap", "断胶（超过允许长度）"],
  ["lostFrame", "传输丢帧 / 缺帧"],
  ["locateFail", "定位失败（胶正常）"],
  ["locateFailGap", "定位失败 + 断胶"],
  ["countMismatch", "计数偏差（只告警）"],
  ["motionTimeout", "运动超时（不发 partEnd）"],
  ["ackMismatch", "ACK 序号不匹配"],
  // E1 后端落地后启用：cameraLost 单台相机缺帧、pulseJitter 脉冲数波动、cornerSlowdown 弯道减速、
  // notInspected 本件未检、earlyEnd 提前 partEnd。后端 Scenario 目前没有这些变体，选中会反序列化失败，
  // 所以下拉与样本集白名单都不放它们（Scenario 联合类型里保留）。
  ["random", "随机（连续运行用）"],
];

/** 只改变虚拟 PLC / 机器人握手与脉冲串、不改图像内容的工况；与后端 Scenario::handshake() 白名单一致（E1 同步）。 */
const handshakeScenarios = new Set<Scenario>([
  "normal", "lostFrame", "countMismatch", "motionTimeout", "ackMismatch",
]);

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
