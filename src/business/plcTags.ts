import type { TagPreset } from "../features/plc";

export const inspectionTagPresets: TagPreset[] = [
  { value: "plcVersion", label: "PLC 协议版本（PLC→PC）" },
  { value: "plcHeartbeat", label: "PLC 心跳（PLC→PC）" },
  { value: "inspecting", label: "在检（PLC→PC）" },
  { value: "partSeq", label: "件序号（PLC→PC）" },
  { value: "productCode", label: "型号（PLC→PC）" },
  { value: "sn", label: "工件 SN（PLC→PC）" },
  { value: "pcVersion", label: "上位机协议版本（PC→PLC）" },
  { value: "pcHeartbeat", label: "上位机心跳（PC→PLC）" },
  { value: "pcMode", label: "上位机模式（PC→PLC）" },
  { value: "resultSeq", label: "结果件序号（PC→PLC）" },
  { value: "resultCode", label: "结果码（PC→PLC）" },
  { value: "faultCode", label: "异常码（PC→PLC）" },
  { value: "snEcho", label: "结果 SN（PC→PLC）" },
];

export const inspectionHandshakeTags = new Set(inspectionTagPresets.map(preset => preset.value));

export const commonPlcTags = new Set(["plcHeartbeat", "pcHeartbeat", "plcVersion", "pcVersion"]);

export const plcTagHelp: Record<string, string> = {
  plcVersion: "PLC 实现的生产协议版本，应为 9；不是 9 时通讯未就绪。",
  plcHeartbeat: "PLC 周期变化的计数；3 s 不变化时通讯未就绪，在检的件记中断，恢复后自动就绪。",
  inspecting: "在检：工件到达置 1，工件结束置 0。PC 读到 1 且件序号变化即登记本件，回 0 后收尾并写结果。",
  partSeq: "件序号，每到一件加 1，PLC 在置在检前写入；0 表示还没有件。PC 写结果时把它写到结果件序号。",
  productCode: "本件型号，PLC 在置在检前写入；PC 按型号选择配方。",
  sn: "本件工件 SN，PLC 在置在检前写入；PC 记入检测记录，并在写结果时写到结果 SN。",
  pcVersion: "PC 实现的生产协议版本，固定写 9。",
  pcHeartbeat: "PC 周期变化的计数；PLC 发现 3 s 不变化按 PC 心跳失活处理，不取结果，本件记未检。",
  pcMode: "0 正常；1 维护：结果码与异常码都写 0，PLC 按未检件放行。切换从下一件生效，仅供 HMI 显示。",
  resultSeq: "结果对应的件序号，PC 写完结果码、异常码与结果 SN 后最后写入；PLC 看到它等于本件件序号才取结果，0 表示没有结果。",
  resultCode: "本件结论：1 OK、2 OK_WITH_EXCURSION、11–14 各类 NG、90 检测错误；维护模式为 0。",
  faultCode: "结果码为 90 时的原因，例如 91 整件无图、95 配方不可用；其余情况为 0。",
  snEcho: "结果对应的工件 SN，供 HMI 或 MES 核对。",
};
