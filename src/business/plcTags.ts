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

export const teachingTagPresets: TagPreset[] = [
  { value: "teachPcVersion", label: "示教 PC 协议版本（PC→PLC）" },
  { value: "teachPcState", label: "示教 PC 状态（PC→PLC）" },
  { value: "teachRoundId", label: "示教握手编号（PC→PLC）" },
  { value: "teachPcReason", label: "示教 PC 原因码（PC→PLC）" },
  { value: "teachPcReserved", label: "示教 PC 保留字段（PC→PLC）" },
  { value: "teachPlcVersion", label: "示教 PLC 协议版本（PLC→PC）" },
  { value: "teachPlcState", label: "示教 PLC 状态（PLC→PC）" },
  { value: "teachRoundIdEcho", label: "示教握手编号回显（PLC→PC）" },
  { value: "teachPlcReason", label: "示教 PLC 原因码（PLC→PC）" },
  { value: "teachPlcReserved", label: "示教 PLC 保留字段（PLC→PC）" },
  { value: "teachPlcSnapshot", label: "示教 PLC 完整快照（PLC→PC）" },
];

export const teachingHandshakeTags = new Set(teachingTagPresets.map(preset => preset.value));
export const commonPlcTags = new Set(["plcHeartbeat", "pcHeartbeat", "plcVersion", "pcVersion"]);
