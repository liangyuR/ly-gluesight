import type { TagPreset } from "../features/plc";

export const inspectionTagPresets: TagPreset[] = [
  { value: "plcVersion", label: "PLC 协议版本（PLC→PC）" },
  { value: "plcHeartbeat", label: "PLC 心跳（PLC→PC）" },
  { value: "plcState", label: "PLC 状态（PLC→PC）" },
  { value: "plcReason", label: "PLC 原因码（PLC→PC）" },
  { value: "requestSeq", label: "请求序号（PLC→PC）" },
  { value: "productCode", label: "型号（PLC→PC）" },
  { value: "sn", label: "工件 SN（PLC→PC）" },
  { value: "pcVersion", label: "上位机协议版本（PC→PLC）" },
  { value: "pcHeartbeat", label: "上位机心跳（PC→PLC）" },
  { value: "pcState", label: "上位机状态（PC→PLC）" },
  { value: "pcMode", label: "上位机模式（PC→PLC）" },
  { value: "seqEcho", label: "回显请求序号（PC→PLC）" },
  { value: "resultCode", label: "结果码（PC→PLC）" },
  { value: "faultCode", label: "异常码（PC→PLC）" },
  { value: "snEcho", label: "回显工件 SN（PC→PLC）" },
];

export const inspectionHandshakeTags = new Set(inspectionTagPresets.map(preset => preset.value));

export const commonPlcTags = new Set(["plcHeartbeat", "pcHeartbeat", "plcVersion", "pcVersion"]);

export const plcTagHelp: Record<string, string> = {
  plcVersion: "PLC 实现的生产协议版本，应为 7；与 PC 不一致时 PC 不就绪。",
  plcHeartbeat: "PLC 周期变化的计数；PC 发现 3 s 不变化按 PLC 心跳失活处理。",
  plcState: "PLC 握手状态：0 空闲、10 请求检测、20 运动结束、30 已确认结果、90 故障、99 复位请求。",
  plcReason: "PLC 置故障时给出的原因，例如 6 PC 心跳失活、7 运动超时、14 在途中 PC 故障或未就绪；0 表示无。",
  requestSeq: "每件检测的事务序号，PLC 在置请求检测前写入、逐件递增；PC 回显到回显事务序号供 PLC 核对。",
  productCode: "本件型号，PLC 在置请求检测前写入；PC 按型号选择配方。",
  sn: "本件工件 SN，PLC 在置请求检测前写入；PC 记入检测记录，并在写结果时回显。",
  pcVersion: "PC 实现的生产协议版本，固定写 7。",
  pcHeartbeat: "PC 周期变化的计数；PLC 发现长时间不变化按 PC 心跳失活（原因码 6）处理。",
  pcState: "PC 握手状态：0 未就绪、10 就绪、20 已布防、30 结果有效、90 故障、99 复位请求。",
  pcMode: "0 正常；1 维护：结果码与异常码都写 0，PLC 按未检件放行。切换从下一件生效。",
  seqEcho: "PC 核对请求后写回本件的请求事务序号；PLC 只认与本件序号一致的布防和结果。",
  resultCode: "本件结论，PC 在置结果有效前写入：1 OK、2 OK_WITH_EXCURSION、11–14 各类 NG、90 检测异常；维护模式为 0。",
  faultCode: "结果码为 90 或 PC 故障时的原因，例如 91 整件无图、97 运动结束超时、101 PC 握手故障；其余情况为 0。",
  snEcho: "PC 写结果时回显本件工件 SN；PLC 核对与本件 SN 一致才锁存结果。",
};
