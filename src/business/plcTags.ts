import type { TagPreset } from "../features/plc";

export const inspectionTagPresets: TagPreset[] = [
  { value: "partStart", label: "工件开始（PLC→PC）" },
  { value: "partEnd", label: "运动结束、脉冲串停发（旧配方：触发计划发完）（PLC→PC）" },
  { value: "resultAck", label: "结果确认（PLC→PC）" },
  { value: "faultReset", label: "故障复位（PLC→PC）" },
  { value: "plcHeartbeat", label: "PLC 心跳（PLC→PC）" },
  { value: "protocolVersion", label: "协议版本（PLC→PC）" },
  { value: "requestSeq", label: "请求序号（PLC→PC）" },
  { value: "partSn", label: "工件序列号（PLC→PC）" },
  { value: "productCode", label: "产品代码（PLC→PC）" },
  { value: "shotCount", label: "预期总帧数 ΣN_exp（旧配方：触发次数）（PLC→PC）" },
  { value: "planVersion", label: "拍照计划版本（PLC→PC）" },
  { value: "planReserved", label: "保留字段（PLC→PC）" },
  { value: "deviceShots", label: "设备预期总帧数 ΣN_exp（旧配方：设备触发次数）（PLC→PC）" },
  { value: "deviceTriggers", label: "累计脉冲（参考）（PLC→PC）" },
  { value: "ackSeq", label: "确认序号（PLC→PC）" },
  { value: "visionReady", label: "视觉就绪（PC→PLC）" },
  { value: "armed", label: "已布防（PC→PLC）" },
  { value: "busy", label: "检测中（PC→PLC）" },
  { value: "done", label: "结果有效（PC→PLC）" },
  { value: "visionFault", label: "视觉故障（PC→PLC）" },
  { value: "pcHeartbeat", label: "上位机心跳（PC→PLC）" },
  { value: "pcProtocolVersion", label: "上位机协议版本（PC→PLC）" },
  { value: "acceptedSeq", label: "已接受请求序号（PC→PLC）" },
  { value: "acceptedPlanReserved", label: "保留字段（PC→PLC）" },
  { value: "resultSeq", label: "结果序号（PC→PLC）" },
  { value: "resultCode", label: "结果码（PC→PLC）" },
  { value: "faultCode", label: "异常码（PC→PLC）" },
  { value: "resultSn", label: "结果 SN（PC→PLC）" },
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
export const commonPlcTags = new Set(["plcHeartbeat", "pcHeartbeat", "faultReset", "visionFault", "protocolVersion", "pcProtocolVersion"]);
