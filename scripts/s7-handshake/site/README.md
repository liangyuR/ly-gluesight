# GlueSight 现场高电平采集程序

握手协议保持 **6**，DB4 / DB2 字段、偏移和类型不变。`deviceTriggers` 在 DB4.DBW32 保留，始终写 0。

`gluesight_site.scl` 包含 `GlueSightHandshake` 与 `GlueSightStation`。现场已有 DB 须分别命名为 `plc_send_vision` 和 `vision_send_plc`，关闭优化块访问；完整点表见 [PLC 通讯协议](../../../docs/integration/plc-communication-protocol.md)。

## 现场接入

- `MotionActive` 接机器人相机采集使能电平。机器人等待 `MotionPermit` 后开始运动，在需要采图的连续路径期间保持使能高，结束前拉低，再置 `MotionComplete`。
- `CameraTrigger` 接相机 Line0。只有本件已布防、身份有效、运动使能高且无故障时为高；运动结束、PC 撤销 armed、心跳超时、外部或本地故障立即拉低。
- 删除了逐拍照点 `ShotRequest`、`PulseWidth`、累计脉冲计数及其诊断。相机自身按软件设置的帧率采集，默认 10 fps；PLC 不产生脉冲、不计曝光帧数。
- `MotionPermit` 与采集使能分开，不能用 `CameraTrigger` 作为机器人开始运动的前置条件。
- `DeviceShots` 填 PC 导出计划的 expectedTotal，`PlanVersion`、`ProductCode` 与导出计划一致。示教可填正的估算帧数，不参与采集帧数核对。
- `LastSeq` 放入断电保持区。启动、重连或故障后必须经操作员处置未结件并显式复位。
- 外部锁定示教/生产用途。示教结果 1 只代表采集完成，不代表合格放行。

## 验收

本 SCL 尚未在 TIA Portal 或真实 CPU 编译运行，不能视为现场验证通过。

导入后核对 DB 偏移；验证布防前低电平、运动高电平、结束先低再 partEnd、故障同扫描拉低、完整收尾后才发布结果，以及 ACK/释放完成前不接受下一件。核对相机高电平多帧、低电平停止新曝光、10 fps 回读与实际帧率、多相机序号配组。帧号配组不保证曝光时刻同步。
