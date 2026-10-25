# GlueSight 现场参考程序（生产通讯协议 7）

`gluesight_site.scl` 按生产通讯协议 7 实现 PLC 侧状态机；点表、状态与原因码、迁移表、规则、结果码和时间参数均见[协议 7 规格](../../../docs/integration/plc-communication-protocol.md)，本文只写导入与接线。协议 7 与协议 6 不兼容，PC 与 PLC 必须成套切换。

## 导入

1. 先导入 `teaching_v1.scl`，将非优化数据块 `teach_pc` 分配为 **DB6**、`teach_plc` 分配为 **DB12**（不变）。
2. 再导入 `gluesight_site.scl`。它包含：
   - `plc_send_vision`（DB4，PLC 写）与 `vision_send_plc`（DB2，PC 写）：各 68 字节，非优化块访问。协议 6 的同名 DB 布局不同，须删除后按本文件重新生成，并分配回 DB4 / DB2；生成后核对偏移量列与规格点表一致。
   - `GlueSightProduction`：协议 7 状态机，一个 `CASE` 按 `plcState` 分支，只写 DB4、只读 DB2。
   - `GlueSightStation`：现场适配，调用生产与示教，合并 `MotionPermit` 与 `Line0`。
3. 不得让 TIA 自动编号覆盖 DB2 / DB4 / DB6 / DB12。

## 现场接线（TODO 项需现场确认）

| 编号 | 参数 | 约定 |
| --- | --- | --- |
| TODO-0 | `Startup` | 首个扫描周期标志。启动后 `plcState` 为 99（PLC 发起复位），PC 回 10 后进入 0 |
| TODO-1 | `PartPresent` | 工件到位、机器人等待放行的电平；机器人收到 `MotionPermit` 或 `SkipMotion` 并完成本件后拉低，PLC 随之撤销两者 |
| TODO-2 | `MotionActive` | 采集路径期间保持一段连续高电平；由高变低视为运动结束（`PartPresent` 拉低也视为结束） |
| TODO-3 | `InterlockOk` | 急停、安全门等互锁满足；运动中失去互锁进入 90（4） |
| TODO-4 | `ExecutionFault` | 机器人报警等现场执行故障；plcState 10 时进入 90（9），并阻止软件复位 |
| TODO-5 | `OperatorReset` / `ResetPermit` / `SeqRestart` | 现场复位按钮（90 → 99）；允许 PC 软件复位（不允许时 90（13））；序号耗尽 90（12）后与 PC 协同回绕时置位，复位完成即从 1 重新开始 |
| TODO-6 | `ProductCode` / `PartSn` | 型号 1–32767 与工件 SN。不合规时不提交请求，按未检放行并置 `PartDataInvalid` |
| TODO-7 | `TeachMode` / `RobotReady` / `RobotStopped` / `MotionComplete` | 仅示教 V1 使用，含义同 `teaching_v1.scl` |
| TODO-8 | `Line0` | 三台相机 Line0。生产仅在规格第六节 Line0 条件全部满足时为高；示教门控同 V1 |
| TODO-9 | `MotionPermit` / `SkipMotion` | PC 不在线、示教占用、故障或复位中来件都直接给 `MotionPermit`、按未检放行，不等待 PC；`SkipMotion` 是 PC 拒收后“本件不运动”的放行信号，机器人如何处理需现场确认 |
| TODO-10 | `ResultOk` / `ResultNg` / `InspectError` / `Unchecked` / `Maintenance` / `PartDataInvalid` / `DispositionDone` | 每件恰有一个结论位，保持到下一件提交或放行；`DispositionDone` 为单扫描脉冲。去向（HMI、输出位或 MES）需现场确认 |

`LastSeq`、`LastTeachRoundId` 在 `GlueSightStation` 的断电保持区。超时默认值取规格第九节：布防 10 s、运动 30 s、结果 10 s、回就绪 10 s，PC 心跳 3 s 不变判失活。

## 实现约定

- 规格未写明、由本程序补充的处理：运动超时对任何已放行的运动计时；10 / 20 / 30 期间到位的下一件等待本件结束后再处理；请求在途、未放行时机器人撤走工件按 90（9）；30 状态下 PC 回 0 / 90 按 90（14）。
- 与示教互斥：`teach_plc.teachPlcState` 非 0 时不提交生产请求、来件按未检放行；`plcState` 非 0 或生产件未离开时示教拒绝受理。示教停在 40 / 50 期间生产件全部按未检放行，需 PC 清示教请求或完成复位后恢复。
- 示教 V1 的接入与时序不变，见[独立示教采集时序](../../../docs/integration/teaching-capture-sequence.md)。

## 验收

本 SCL 尚未在 TIA Portal 或真实 CPU 编译运行，不能视为现场验证通过。导入后按规格第五节逐行验证迁移（正常一件、拒收、PC 不在线、各超时、心跳失活、PC 故障、两种复位、序号耗尽），并核对维护模式、SN 回显不符、Line0 门控时序与示教互斥。
