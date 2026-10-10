# 一期 S7 飞拍握手契约与 PLC 参考程序

一期架构、剩余工作包和验收路线见 [软件架构图](../architecture/gluesight-software-architecture.html)，文档入口见 [docs 索引](../README.md)。

本协议由 GlueSight 项目定义，通过现有 `ly-plc` 的 S7 ISO-on-TCP 客户端读写 PLC DB。**CPU 型号、固件、IP、机架/插槽、TSAP、TIA Portal 版本及实际触发输出尚未确定。** DB100 是建议的默认共享区；现场可选择其他 DB，但双方配置必须一致。

[PLC SCL 参考源文件](../../scripts/s7-handshake/phase1_plc.scl)提供可审查的 DB 布局与 PLC 侧流程，**尚未导入 TIA Portal、尚未编译、尚未在真实 Siemens CPU 或机台运行**。Python peer 的线协议测试不等于 SCL 编译或现场验收。

## 1. 支持边界与责任

- PLC/机器人维护运动路径、拍照位置、输出脉冲、脉冲宽度及相机槽映射；GlueSight 的 UI、S7 网络轮询和 Windows 线程均不生成位置同步的硬触发。
- 本次握手提供三个相机槽的计划计数和实际触发计数。配方的每个拍照点各自绑定相机与 Pose，计划导出按拍照点的相机统计各槽计划数；软件按帧归属到拍照点（P0 步 3）完成前，多相机配方可以导出计划但不能布防。**三个协议槽不表示三相机飞拍归属、分点资源和现场联调已完成**。2026-10-10 确认现场“三相机”是一台三目设备（SDK 中一个 Device、一次触发出三幅图，按拍照点选一幅）；单台设备、单路触发时只用槽 1，计划计数与触发数都记在槽 1。槽的最终含义待触发线路确认后修订，见 [P0 计划 1.2 节](../architecture/p0-plan.md)。
- `TriggerPermit` 只允许现场触发程序开始/继续发新脉冲；SCL 不直接写物理输出。`IssuedPulseCount[1..3]` 由现场可靠计数源提供，每个实际输出脉冲加一，不能使用普通扫描对高速脉冲的布尔采样代替。PLC 发出脉冲不等于相机成功曝光，后者仍需相机帧/触发计数和完整性验证。
- PLC 单独拥有 DB 字节 0–63；PC 单独拥有字节 64–87。PLC 代码只读 PC 区，PC 点表将 PLC 区设为只读。两个写入方不得共用同一字节，也不得用整 DB 的初始化/块复制覆盖对方区域。
- 一件只有一个在途事务。普通质量 NG 可以按有效结果确认；通讯、身份或状态不确定进入故障，保留 pending，停止新触发及下一件。现场运动的停机策略由 PLC/机器人程序负责。

## 2. DB100 点表

使用**关闭优化块访问的标准 DB**，至少 88 字节。多字节值为 S7 大端 ABCD；PC `u16/u32` 对应 PLC `UInt/UDInt`，不能用有符号 `Int/DInt` 接收大于其上限的 hash/序号。布尔按指定 bit 访问。

| 所有者 | 地址 | 类型 | 标签及含义 |
| --- | --- | --- | --- |
| PLC | DB100.DBX0.0 | Bool | `partStart`：请求有效，从提交 payload 起保持到结果释放 |
| PLC | DB100.DBX0.1 | Bool | `partEnd`：运动和本件全部计划触发已结束，保持到释放 |
| PLC | DB100.DBX0.2 | Bool | `resultAck`：匹配结果已接收，保持到 PC 清 done/busy |
| PLC | DB100.DBX0.3 | Bool | `faultReset`：明确复位请求，必须有新低→高边沿 |
| PLC | DB100.DBX0.4 | Bool | `plcHeartbeat`：名义每 500 ms 翻转 |
| PLC | DB100.DBW2 | UInt | `protocolVersion`：固定 1 |
| PLC | DB100.DBD4 | UDInt | `requestSeq`：本件非零递增事务序号 |
| PLC | DB100.DBD8 | UDInt | `partSn`：工件 SN，不替代事务序号 |
| PLC | DB100.DBW12 | UInt | `productCode`：选择已发布配方的产品代码 |
| PLC | DB100.DBW14 | UInt | `shotCount`：本件总拍照数，1–65535 |
| PLC | DB100.DBD16 | UDInt | `planVersion`：导出的配方计划版本 |
| PLC | DB100.DBD20 | UDInt | `planHash`：导出的 32 位计划摘要 |
| PLC | DB100.DBW24 / DBW26 / DBW28 | UInt ×3 | `camera1Shots / camera2Shots / camera3Shots`：各槽计划数，总和等于 shotCount |
| PLC | DB100.DBD32 | UDInt | `ackSeq`：确认时回显本件 resultSeq |
| PLC | DB100.DBW36 / DBW38 / DBW40 | UInt ×3 | `camera1Triggers / camera2Triggers / camera3Triggers`：本件已发脉冲数；布防前为 0，结束时等于计划数 |
| PLC | DBB0 保留位、DBB1、DBW30、DBB42–63 | 保留 | 不另作跨方向写入字段 |
| PC | DB100.DBX64.0 | Bool | `visionReady`：可以接收下一件；不能仅凭连接成功启动 |
| PC | DB100.DBX64.1 | Bool | `armed`：本件布防完成；结合 acceptedSeq/hash 核对 |
| PC | DB100.DBX64.2 | Bool | `busy`：事务处理中，直到有效 ACK 后清除 |
| PC | DB100.DBX64.3 | Bool | `done`：结果提交标志；结果 payload 写好后最后置位 |
| PC | DB100.DBX64.4 | Bool | `visionFault`：协议/设备故障，需处置并复位 |
| PC | DB100.DBX64.5 | Bool | `pcHeartbeat`：PC 通讯心跳，建议 500 ms |
| PC | DB100.DBW66 | UInt | `pcProtocolVersion`：成功复位后为 1 |
| PC | DB100.DBD68 | UDInt | `resultSeq`：结果对应的 requestSeq |
| PC | DB100.DBD72 | UDInt | `resultSn`：结果对应的 partSn |
| PC | DB100.DBW76 | UInt | `resultCode`：整件结果码 |
| PC | DB100.DBW78 | UInt | `faultCode`：检测异常码；正常质量结果为 0 |
| PC | DB100.DBD80 | UDInt | `acceptedSeq`：已接受并布防的 requestSeq |
| PC | DB100.DBD84 | UDInt | `acceptedPlanHash`：已接受的 planHash |
| PC | DBB64 保留位、DBB65 | 保留 | PLC 不写 |

SCL 使用符号 DB 名 `GlueSightHandshake`。导入后在 TIA 的块属性中**明确指定编号 100**，关闭优化访问，查看每个字段的实际偏移；源文件的符号名称本身不会保证自动分配到 DB100。使用其他 DB 编号时，同步改 PC 模板和测试 peer 的 `--db`。

## 3. 正常一件时序

1. **等就绪。** 已完成本连接的显式复位，观察到 PC 心跳变化；`visionReady=1` 且 `visionFault/armed/busy/done=0`，PLC `partStart/partEnd/resultAck=0`。不能将重连前残留的 ready 高电平直接当作新会话就绪。
2. **先 payload，后 start。** PLC 在新工件上递增非零 `requestSeq`，写 SN、产品代码、计划版本/hash、总数、各槽计划数，清三个触发数和旧 ACK，再最后置 `partStart=1`。上述请求字段在整件期间冻结，包含等待结果和等待 ACK 释放阶段。
3. **等布防承诺。** 必须同时满足 `armed=1`、`busy=1`、`visionReady=0`、`acceptedSeq=requestSeq`、`acceptedPlanHash=planHash`，才开启现场 `TriggerPermit`。不以任意单一高电平代替这组条件。
4. **运动与硬触发。** 现场程序按已核对的拍照点次序向对应相机发脉冲。SCL 在布防确认时冻结三路累计脉冲计数基线，以增量形成本件触发数；计数回退、超计划、错误槽位均停止新触发。`IssuedPulseCount` 在一件内不得重置/回绕。
5. **先最终计数，后结束。** 运动完成后，各槽实际触发数必须分别等于计划数，再置 `partEnd=1`；`partStart` 继续保持。PC 核对计数并等待图像及测量完成。
6. **先验证并接收结果，后 ACK。** `done=1` 时核对 `resultSeq=requestSeq`、`resultSn=partSn`，检查结果码及异常码并在 PLC 侧锁存；随后写 `ackSeq=resultSeq`，最后置 `resultAck=1`。有效 NG/检测异常表示已收到了该件结论，ACK 不表示良品放行。
7. **等待双方释放。** PLC 保持 request payload、start/end/ACK，直到 PC `done=0` 且 `busy=0`。再清 start/end/ACK，等 PC 再次 ready，才结束 pending 并允许下一件。

拒收和提前终止也必须完成结果握手：等待布防时允许 PC 直接提交 `ERR_INSPECT=90`；已经布防后，若 `armed` 撤销但 busy 和已接受身份仍正确，PLC 立即停止新触发，限时等待结果。没有经过本地三路计数核对并置 `partEnd` 时，只接受 `90` 与 `91–100` 的已知异常码组合，不能把提前到达的 OK/NG 当有效结论。结果必须满足 seq/SN、armed 低、busy 高和 ready 低，再锁存并 ACK。`visionFault`、心跳失活或身份改变仍直接进入保留 pending 的故障态。

`requestSeq` 与 SN 用途不同：同一 SN 复检也要新事务序号。最大序号 `4294967295` 后不得直接回绕开始生产；先处置未结件，在中性基线下由 PLC 工程师重置序号，再明确复位双方会话。

## 4. 启动、重连与故障恢复

PC 启动、重新建立 S7 连接或发现配置变化后进入复位要求态。PC 持久化的 pending 不会因一次连接成功消失。PLC 参考 FB 首次调用或 `Startup=1` 也进入 `State=90`，默认停止新触发，保留 DB 中现有请求与 pending 标记。

恢复顺序：确认实物工件和动作已被处理 → 查看并记录双方 pending、SN、序号、已收结果 → 操作员设置 `ReconcileConfirmed` → 将本地 `ResetRequest` 先置低，再发新的上升沿。`ReconcileConfirmed` 不应永久接常量 TRUE。复位不是自动重试原件：PC 明确复位会结束未结事务，需先决定这件是隔离、复检还是已有有效结果。

参考 FB 收到合格复位请求后释放 PLC 的 start/end/ACK，将 `faultReset` 保持低至少 1 s，再置高并保持至少 500 ms，直到看到 **ready 的低状态再到高状态**、`pcProtocolVersion=1`，且 armed/busy/done/visionFault 全低。这样可覆盖初始 Fault/ResetRequired，以及无在途时从 Idle 发起明确复位；不会把先前残留的 ready 高电平认作复位成功。若现场扫描错过 ready 低相或 PC 未处理请求，复位超时停在故障态，必须重做新低→高请求，不能直接放行。

如果 PC 还有在途请求，应先让该请求进入明确故障并完成实物处置；不能在运转中清 PLC 输入来强行切下一件。中性基线期间保持 requestSeq、ackSeq 不变。两个方向的心跳都要求观察**变化**；恒定 1 与恒定 0 都不是活性证据。参考 PLC 在 PC 心跳 3 s 不变时停新触发并锁故障；PC 对 PLC 心跳同样使用 3 s 变化超时。

SCL 的故障分支不清请求 payload、start/end/ACK、锁存结果或 pending；只有明确处置后的复位、或成功完成结果释放才结束 pending。配置 TIA 保持性时，明确保存 PLC 请求区、事务序号及 FB pending/结果信息；还要验证 CPU 冷启动、下载和断电造成数据丢失时仍停在复位态，由双方日志和实物决定处置，不能推断旧件成功。

标准 DB 的保持性按整块设置，因此 PC 输出也可能保留旧值；启动时仍需完成双方复位。具体保持性规则见 [Siemens 标准块与优化块访问说明](https://docs.tia.siemens.cloud/r/en-us/v20/programming-basics/blocks-in-the-user-program/blocks-with-optimized-access/basics-of-block-access)。

## 5. 结果与诊断

| resultCode | 含义 |
| --- | --- |
| 1 | OK |
| 2 | OK_WITH_EXCURSION：局部超差仍符合配方允许条件 |
| 11 | NG_POSITION：位置偏差 |
| 12 | NG_ABSOLUTE：绝对限值超差 |
| 13 | NG_GAP：断胶 |
| 14 | NG_WIDTH：胶宽超差 |
| 90 | ERR_INSPECT：检测异常，结合 faultCode 与 PC 原因查看 |

| faultCode | 含义 |
| --- | --- |
| 0 | 无检测异常；质量 NG 不靠 faultCode 表示 |
| 91 | 缺帧 |
| 92 | 定位失败 |
| 93 | 无效测量点 |
| 94 | 拍照计划/数量不匹配 |
| 95 | 配方不可用 |
| 96 | 多余帧 |
| 97 | 运动结束超时 |
| 98 | 设备丢失 |
| 99 | 测量超时或测量执行失败 |
| 100 | 计划版本/hash、槽映射或计划内容与 PC 不一致 |

协议身份不一致、回写失败和心跳失活可能只体现为 `visionFault` 与应用错误原因，不保证一定生成可提交的 `done/resultCode=90`；没有有效 done 时不得读取旧结果当新结论。未知结果/异常组合按异常处理，不作为 OK。

SCL `LocalDiagnostic` 是 PLC FB 本地诊断，**不写 DBW78**，也不是 PC faultCode：7000 启动/未知状态需复位；7001 PC 心跳超时；7002 PC 协议故障；7003 阶段超时；7004 新件计划/前置条件错误；7005 布防或结果身份/状态不符；7006 触发计数不符；7007 序号耗尽；7008 在途 payload 被修改；7009 外部设备互锁故障。

## 6. 计划 JSON、版本与 hash

在 PLC 通讯页使用“配方握手计划 · 只读”，选择已保存的生产配方，复制完整计划 JSON。候选草稿没有在此处发布；复制操作不写 PLC。将 `planVersion/planHash/shotCount/cameraShots` 与现场机器人/PLC 路径版本一起评审并配置。`cameraSlots` 的数组顺序定义协议槽，不能按相机连接顺序随意重排。

以下是算法示例，不是现场配方。两点属于同一台相机，因此计划数为 `[2,0,0]`：

```json
{
  "protocolVersion": 1,
  "recipeId": "DEMO",
  "planVersion": 1,
  "planHash": 581977774,
  "shotCount": 2,
  "cameraSlots": ["cam1", "", ""],
  "cameraShots": [2, 0, 0],
  "shots": [
    {"shotId": "P1", "poseId": "P1", "cameraId": "cam1"},
    {"shotId": "P2", "poseId": "P2", "cameraId": "cam1"}
  ]
}
```

当前 `plc_plan.rs` 对 UTF-8 紧凑 JSON 元组 `[1,recipeId,planVersion,cameraSlots,shots]` 做 32 位 FNV-1a：初始 `2166136261`，每字节执行 `h = ((h XOR byte) × 16777619) mod 2^32`。顺序、Pose、相机绑定和版本均影响 hash；序列化采用 Rust `serde_json` 与 `PlanShot` 字段顺序（`shotId`、`poseId`、`cameraId`）。示例的 hash 由 `plc_plan.rs` 的单元测试 `documented_example_hash` 固定，改 hash 输入时两边同步修改。**PLC 应复制应用导出的数值，不在 PLC 中重新拼 JSON 算 hash**；美化后的 JSON 不是原始 hash 输入。该 32 位值用于版本一致性核对，不是无碰撞证明或认证机制。

`productCode` 在当前计划 JSON 中不提供，需从生产配方另外核对并写 DBW12。一期按拍照点在图像里检测，计划不含工件坐标。`poseId` 是现场机器人 / PLC 程序里的 Pose 标识，同一 Pose 可同时触发几台相机，因此不要求唯一；机器人程序与物理输出映射不在 JSON 内，需保存在现场的对应表。修改产品代码、拍照计划或槽映射后重新导出、评审和验证，不能只保留旧 hash。

## 7. SCL 接入与调试

1. 确认 CPU/固件/TIA 版本及实际 IP、端口、rack/slot、TSAP、DB 编号。标准 S7 服务通常用 TCP 102；本机 Python peer 使用随机端口，两者不要混淆。依据实际 CPU 设置远程 PUT/GET 访问并使用标准非优化 DB；相关配置随固件而异，按 [Siemens GET/PUT 官方说明](https://docs.tia.siemens.cloud/r/simatic_s7_1200_manual_collection_enus_20/communication/s7-communication/get-and-put-read-and-write-from-a-remote-cpu) 核对。
2. 将 SCL 文件作为外部源导入 TIA，生成块，将 `GlueSightHandshake` 指定为 DB100。检查第 2 节每个偏移、88 字节长度、保持性和块号冲突。`GlueSightPhase1` FB 可使用优化实例，通讯 DB 不能优化。标准/优化访问差异见 [Siemens 块访问说明](https://docs.tia.siemens.cloud/r/en-us/v20/programming-basics/blocks-in-the-user-program/blocks-with-optimized-access/basics-of-block-access)。
3. 建立 FB 实例并在循环程序中每扫描调用，按 CPU 启动机制给 `Startup` 首扫描脉冲。接入 NewPart、人工 ResetRequest/ReconcileConfirmed、现场计数与 MotionComplete、外部互锁及计划参数；先调用 FB，再由现场输出逻辑依据 TriggerPermit 放行。参考代码采用 IEC_TIMER 多重实例和 TON，具体 CPU/TIA 的声明及调用须通过编译确认，见 [Siemens IEC 定时器调用](https://docs.tia.siemens.cloud/r/en-us/v20/scl-s7-1200-s7-1500/timer-operations-s7-1200-s7-1500/calling-iec-timers-s7-1200-s7-1500)。
4. 对 SCL 做 TIA 编译，修复版本相关语法/类型差异后再次核对 DB 偏移与跨方向写入。参考阶段超时是联调起点：布防 10 s、运动 30 s、结果 10 s、释放 10 s、复位 10 s；均应为正值，并按现场节拍与 PC 超时预算核准，不能视作现场 SLA。
5. GlueSight 加载一期 S7 模板，确认实际连接参数和 DB，保持自动连接关闭，核对地址表、UInt/UDInt、ABCD 及 pcHeartbeat 绑定；应用保留握手点的写入所有权。准备兼容的 lyFlow DLL、设备驱动、已标定/示教/验证的生产配方，导出实际计划。
6. 首先在无硬触发输出的条件下完成首次低基线和显式复位。分别看两个心跳变化，确认 ready、版本、armed/busy/done/fault 状态，运行一件握手检查 SN/seq/hash 对应及释放顺序。
7. 接通现场触发后验证每槽计数、第一/最后一帧缺失、错误相机、额外脉冲、乱序、触发间隔和结果截止。核对 NG、ERR 以及未知结果不会错误放行。
8. 验证读写拒绝、写入已发生但响应丢失、心跳冻结、网线断开、PC/PLC 重启、错误 ACK、旧 reset 高电平和未结件恢复。确认不能切新件或丢弃 pending，并保存PLC观察表、应用日志、计划和原图证据。

FB 状态：10 等新件；20 等布防；30 允许运动触发；40 等结果；50 等 PC 清 done/busy；60 等再次 ready；90 故障/启动等待人工处置；91 中性低基线；92 保持 reset 并等待新就绪。`CycleComplete/ResetComplete` 为单扫描事件，`ResultLatched` 和锁存结果保持至下一件，不要直接用一扫描事件控制持续动作。

## 8. 可复现的软件证据与未完成项

[Python S7 测试 peer](../../scripts/s7-handshake/README.md) 使用真正的回环 TCP、TPKT/COTP/S7 ReadVar/WriteVar，支持默认点表、非默认 DB、故障注入和 PLC 请求/ACK 控制。运行：

```powershell
python -m unittest discover -s scripts/s7-handshake/tests -v
python -u scripts/s7-handshake/s7_test_plc.py --port 0
```

2026-10-09，该 Python 测试命令在主机环境 21 项通过，含固定黄金报文、bit 邻位保护、多 item 填充、分包/粘包、拒绝、延迟、执行前后断线与重连、心跳冻结和进程控制。Windows 沙箱若拒绝 localhost 连接（WinError 10013），需要在获准主机环境运行同一测试，不能跳过 TCP 后记为通过。

Rust 默认回归 59 项通过、0 失败；默认忽略的 17 项 S7 线协议测试已单独运行，3 项实际 DLL 测试本轮未执行。会话专项在真实 `ly_plc::PlcEngine` 和回环 S7 服务之间运行，**17 项线协议场景和 1 项事务文件恢复全部通过**，耗时 103.46 秒；文件恢复项也包含在默认 59 项中，不重复累计。覆盖同 SN 不同序号、计划拒收、早/迟 ACK、写拒绝、执行后断线、释放失败、重启、腐坏日志、审计失败、慢落盘、停止心跳，以及校验后重连禁止布防。复现命令在 `src-tauri` 下运行：

```powershell
cargo test --offline --lib
cargo test --offline --lib plc_session -- --include-ignored --test-threads=1 --nocapture
```

专项日志在 `src-tauri/target/s7-session-tests/verification.log`；各案例目录保留线报文、最终状态、事务及复位审计和 DB 快照。前端 PLC 的 7 个文件、127 项回归通过，应用/测试类型检查和生产构建通过。命令与分层验证边界见 [测试记录](../testing/TESTING.md#s7-一期握手)。

SCL 当前只完成源码审查和字段/写入方向静态核对，**没有 TIA 编译、PLCSIM、真实 CPU、实际 I/O、三相机硬件、现场长稳或实物准确率验收证据**。Python 服务也不模拟 CPU 扫描、优化块权限或真实触发电气特性。
