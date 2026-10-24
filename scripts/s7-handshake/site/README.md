# GlueSight 现场高电平采集程序

握手协议保持 **6**，DB4 / DB2 字段、偏移和类型不变。`deviceTriggers` 在 DB4.DBW32 保留，始终写 0。

先导入 `teaching_v1.scl`，将非优化数据块 `teach_pc` 分配为 **DB6**、`teach_plc` 分配为 **DB12**，再导入 `gluesight_site.scl`。DB6 / DB12 各为 10 字节，五个连续 Int 对应版本、状态、轮次编号（或回显）、原因、保留；偏移为 0 / 2 / 4 / 6 / 8。不得让 TIA 自动编号覆盖生产 DB2 / DB4。

`gluesight_site.scl` 包含 `GlueSightHandshake` 与 `GlueSightStation`。现场已有 DB 须分别命名为 `plc_send_vision` 和 `vision_send_plc`，关闭优化块访问；完整点表见 [PLC 通讯协议](../../../docs/integration/plc-communication-protocol.md)。

## 现场接入

- `MotionActive` 接机器人相机采集使能电平。机器人等待 `MotionPermit` 后开始运动，在需要采图的连续路径期间保持使能高，结束前拉低，再置 `MotionComplete`。
- `CameraTrigger` 接相机 Line0。生产时要求本件已布防；示教时要求已接受当前轮次且 PC 仍保持请求。身份有效、运动使能高且无故障时为高；运动结束、PC 撤销请求或 armed、心跳超时、外部或本地故障立即拉低。
- 删除了逐拍照点 `ShotRequest`、`PulseWidth`、累计脉冲计数及其诊断。相机自身按软件设置的帧率采集，默认 10 fps；PLC 不产生脉冲、不计曝光帧数。
- `MotionPermit` 与采集使能分开，不能用 `CameraTrigger` 作为机器人开始运动的前置条件。
- `DeviceShots` 填 PC 导出计划的 expectedTotal，`PlanVersion`、`ProductCode` 与导出计划一致。这些参数仅用于生产，示教不读取它们。
- `LastSeq` 和 `LastTeachRoundId` 放入断电保持区。启动、重连或故障后必须经操作员处置未结件并显式复位。
- `TeachMode` 是现场示教许可；`RobotReady` 接机器人可执行新轮次，`RobotStopped` 接机器人已停止的真实反馈，不能直接用运动命令取反代替。示教由 PC 准备相机后写 DB6 请求发起，不使用 `NewPart`、`partStart`、`partEnd` 或生产结果码。
- `CaptureComplete` / `CaptureFailed` 是示教终态锁存，下一次示教受理时清除；`ResultValid` / `ResultBits` 只表示生产结论。两者不可混用。`TeachState` / `TeachReason` 可接 HMI；`VisionAlarm` 包含示教故障。

## 示教 V1 接入

完整状态、原因码与操作时序见 [独立示教采集时序](../../../docs/integration/teaching-capture-sequence.md)。`GlueSightStation` 已调用 `GlueSightTeaching`，与生产共用 `MotionPermit` / `CameraTrigger`，不能在 OB 中另行绕过互锁驱动同一输出。生产请求与示教请求同扫描到达时生产优先，示教拒绝原因 3。示教受理至释放期间禁止新生产请求。

PLC 发布 DB12 时先写版本、轮次回显、原因与保留，最后写状态。读取 DB6 连续两扫描完整快照一致才消费状态；空闲状态忽略暂存的轮次和原因，同一已消费状态、同一编号保留已消费原因，直到新状态提交，避免 PC 先写原因再写状态时误报。活动编号改变仍按协议故障处理。PC 状态撤销或编号不匹配即撤销门控，不等待稳定快照。运动完成先撤销输出并锁存，再发布状态 20。

首次启动固定进入状态 50，不续跑残留请求。使用现有生产显式复位：先确认现场停止并处置未结轮次，PC 清 DB6 状态至 0，基础握手复位完成后，示教在心跳正常、`RobotStopped`、无外部故障及稳定空闲请求时恢复 0。`LastTeachRoundId` 只升不降，PLC 不重新执行已消费的编号；生产的 `SeqRestart` 不清示教编号。编号耗尽时需双方协调维护，不能仅清 PLC 保持区。

基础复位与生产接件就绪分开：状态 91 保持中性输入 1 秒后拉高 `faultReset`；状态 92 至少保持高 500 ms，并等 PC 保持 `visionFault=1` 确认处理复位，然后拉低 `faultReset` 进入 93。PC 必须先看到请求高、再等请求低，完成复位审计和事务记录落盘后，最后清 `visionFault`。状态 93 验证 PC 心跳正常、协议 6、`armed/busy/done/visionFault` 全低后才发布 `ResetComplete`。这不依赖 `visionReady`，因此没有生产配方或有未结示教也能复位；生产新请求仍严格要求 `visionReady`。91 / 92 / 93 均禁止运动及接件并纳入复位超时。若 PC 漏读请求高，不能清故障，PLC 超时等待人工处理。

默认示教运动、PC 排空保存结果、释放超时分别为 120 / 15 / 10 秒，由 `GlueSightTeaching` 输入配置。取消或失败立即停采，并等待机器人已停止才确认；停止反馈超时也进入故障。心跳沿用 DB2 / DB4，丢失后不自动恢复，必须显式复位。`phase1_plc.scl` 仍是生产握手独立参考，示教现场使用本目录整站程序。

## 验收

本 SCL 尚未在 TIA Portal 或真实 CPU 编译运行，不能视为现场验证通过。

导入后核对 DB2 / DB4 / DB6 / DB12 偏移、保持区和机器人停止反馈；验证示教正常完成、受理前取消、采集中取消、重复编号、保存失败、心跳丢失、三类超时、重启复位与生产互斥。验证布防前低电平、运动高电平、结束先低再 partEnd、故障同扫描拉低、完整收尾后才发布结果，以及 ACK/释放完成前不接受下一件。核对相机高电平多帧、低电平停止新曝光、10 fps 回读与实际帧率、多相机序号配组。帧号配组不保证曝光时刻同步。
