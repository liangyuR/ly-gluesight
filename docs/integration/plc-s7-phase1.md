# 一期 S7 飞拍握手契约与 PLC 参考程序

面向电气与视觉对接的完整协议见 [PLC 通讯协议](plc-communication-protocol.md)。一期架构、剩余工作包和验收路线见 [软件架构图](../architecture/gluesight-software-architecture.html)，文档入口见 [docs 索引](../README.md)。

本页按 2026-10-22 协议 5（《涂胶检测通信交互表20261022_V1.1》）复核，与协议 4 及交互表的差异见 [通讯协议第 12 节](plc-communication-protocol.md#12-协议-5-与协议-4交互表-v11-差异)，取舍与待确认问题见 [交互表适配决策](../decisions/plc-table.md)。生产与示教整圈采集共用以下点表，共享区没有用途字段；示教成功也回传 `1/0`，只表示采集完成。现场须以外部模式互锁隔离生产请求和良品放行；具体流程、超时与当前模式切换缺口见 [通讯协议 4.1 节](plc-communication-protocol.md#41-示教整圈采集)。

本协议由 GlueSight 项目定义，通过 `ly-plc` 的 S7 ISO-on-TCP 客户端读写 PLC 共享区。交互表 V1.1 定为 **S7-1200，IP 192.168.2.10，端口 102，两个标准 DB（PLC 区 DB4、PC 区 DB2）**；PC 默认以机架 0 / 插槽 1（TSAP 自动）、请求 PDU 480 接入，实际 PDU 以 CPU 协商为准，CPU 须允许 PUT/GET 访问。**具体 CPU 订货号、固件及实际触发输出尚未确认。**

[PLC SCL 参考源文件](../../scripts/s7-handshake/phase1_plc.scl)提供可审查的 DB4/DB2 布局与 PLC 侧流程，**尚未导入 TIA Portal、尚未编译、尚未在真实 Siemens CPU 或机台运行**。Python peer 的线协议测试不等于 SCL 编译或现场验收。

## 1. 支持边界与责任

- PLC/机器人维护运动路径、拍照位置、输出脉冲、脉冲宽度及唯一设备与触发输出的对应关系；GlueSight 的 UI、S7 网络轮询和 Windows 线程均不生成位置同步的硬触发。
- 本次握手固定服务一台 SDK 三目设备，仅提供唯一设备的计划触发数 deviceShots 和实际触发数 deviceTriggers。示教开始前 PC 不手填数量，由 PLC 的开始请求提供 1–64 次并由 PC 锁存，拍照点依据有效收图生成。配方每个拍照点绑定该设备与 Pose；软件已接入按设备会话与触发计数归属拍照点的路由，模拟与回放的软件闭环已完成，真实设备仍须联调验收。现场三目设备按一个 Device、一次触发一个含三视角的设备帧建模；全文称该连接为“设备通道 1”，不定义其他设备通道。一个拍照点选择一个或多个视角均不增加触发数，图 1/2/3 均属于该设备帧。当前海康路径已支持明确配置横向、纵向三等分或三个矩形区域的单张拼接图拆分；未配置布局拒绝启用，布局/尺寸不符产生图像错误。现场 SDK 原图交付、视角顺序、计数和触发线路仍待验收，不能把软件路径视为真机验证。
- `TriggerPermit` 只允许现场触发程序开始/继续发新脉冲；SCL 不直接写物理输出。`IssuedPulseCount`（单个 UDInt） 由现场可靠计数源提供，每个实际输出脉冲加一，不能使用普通扫描对高速脉冲的布尔采样代替。PLC 发出脉冲不等于相机成功曝光，后者仍需相机帧/触发计数和完整性验证。
- PLC 单独写 DB4（字节 0–89），PC 单独写 DB2（字节 0–71）。PLC 代码只读 DB2，PC 点表将 DB4 设为只读。两个写入方不得写对方的 DB，也不得用整块初始化（含 `FILL_BLK`）/块复制覆盖对方 DB。
- 一件只有一个在途事务。普通质量 NG 可以按有效结果确认；通讯、身份或状态不确定进入故障，保留 pending，停止新触发及下一件。现场运动的停机策略由 PLC/机器人程序负责。

**单设备协议版本 5：** 点表共 28 个字段，PLC 区默认 DB4、PC 区默认 DB2；16 位字段为 Int（只用 0–32767），序号、计划版本与保留字段为 DInt；`partSn` 在 DB4.DBB38、`resultSn` 在 DB2.DBB20，均为 S7 `STRING[50]`（SN 1–50 字符）；`resultCode`/`faultCode` 为 DB2.DBW8/DBW10；`protocolVersion`/`pcProtocolVersion` 由 4 升为 5。旧多通道点表和计划、旧协议 2/3/4 点表（DB100 或 DB1、SN 为 UDInt、`STRING[24]` 或 200 SMART 字符串、序号为 UDInt）、旧 PLC 程序（`protocolVersion` 不为 5）直接拒绝，须按协议 5 模板重建点表并重新导出，不做兼容读取。

## 2. DB4 / DB2 点表（交互表 V1.1）

共享区是两个**关闭优化块访问的标准 DB**：PLC 区 **DB4**（PLC 写、PC 读，至少 90 字节，0–89）与 PC 区 **DB2**（PC 写、PLC 读，至少 72 字节，0–71），两个 DB 都从字节 0 起紧凑排列。DB 号可改（PC 模板默认 4 / 2，两者必须不同）。多字节值为 S7 大端 ABCD；Int 为 16 位有符号、本协议只使用 0–32767，DInt 为 32 位有符号、只使用 0–2147483647，PC 读到负数按读错误进入视觉故障。布尔按指定 bit 访问。工件 SN 为 `String[50]`，规则见下表后的说明。

| 所有者 | 地址 | 类型 | 标签及含义 |
| --- | --- | --- | --- |
| PLC | DB4.DBX0.0 | Bool | `partStart`：请求有效，从提交 payload 起保持到结果释放 |
| PLC | DB4.DBX0.1 | Bool | `partEnd`：运动和本件全部计划触发已结束，保持到释放 |
| PLC | DB4.DBX0.2 | Bool | `resultAck`：匹配结果已接收，保持到 PC 清 done/busy |
| PLC | DB4.DBX0.3 | Bool | `faultReset`：明确复位请求，必须有新低→高边沿 |
| PLC | DB4.DBX0.4 | Bool | `plcHeartbeat`：名义每 500 ms 翻转 |
| PLC | DB4.DBW2 | Int | `protocolVersion`：固定 5；2、3、4 及其他值拒绝 |
| PLC | DB4.DBD4 | DInt | `requestSeq`：本件非零递增事务序号，最大 2147483647 |
| PLC | DB4.DBW8 | Int | `productCode`：生产配方产品代码 1–32767，手动选配方也须匹配；示教候选由 PC 选择 |
| PLC | DB4.DBW10 | Int | `shotCount`：协议总拍照数 1–32767；示教采集接受 PLC 提供的 1–64 |
| PLC | DB4.DBD12 | DInt | `planVersion`：生产使用配方库分配的导出版本，不等于配方修订；示教直接记录 PLC 非零版本，未与生产计划比对 |
| PLC | DB4.DBD16 | DInt | `planReserved`：保留位，固定写零，无业务身份用途；非零拒绝 |
| PLC | DB4.DBW20 | Int | `deviceShots`：唯一设备计划触发数，等于 shotCount |
| PLC | DB4.DBD28 | DInt | `ackSeq`：确认时回显本件 resultSeq |
| PLC | DB4.DBW32 | Int | `deviceTriggers`：唯一设备本件已发脉冲数；布防前为 0，结束时等于计划数 |
| PLC | DB4.DBB38.50 | String[50] | `partSn`：工件 SN 字符串，占 DBB38–89（声明长度、实际长度、字符），不替代事务序号 |
| PLC | DB4 保留：DBX0.5–0.7、DBB1、DBB22–27、DBB34–37 | 保留 | 保持 0，不另作跨方向写入字段 |
| PC | DB2.DBX0.0 | Bool | `visionReady`：可以接收下一件；不能仅凭连接成功启动 |
| PC | DB2.DBX0.1 | Bool | `armed`：本件布防完成；结合 acceptedSeq 核对 |
| PC | DB2.DBX0.2 | Bool | `busy`：事务处理中，直到有效 ACK 后清除 |
| PC | DB2.DBX0.3 | Bool | `done`：结果提交标志；结果 payload 写好后最后置位 |
| PC | DB2.DBX0.4 | Bool | `visionFault`：协议/设备故障，需处置并复位 |
| PC | DB2.DBX0.5 | Bool | `pcHeartbeat`：PC 通讯心跳，建议 500 ms |
| PC | DB2.DBW2 | Int | `pcProtocolVersion`：成功复位后为 5 |
| PC | DB2.DBD4 | DInt | `resultSeq`：结果对应的 requestSeq |
| PC | DB2.DBW8 | Int | `resultCode`：生产整件结果或示教采集完成码，不能单凭该值确定用途 |
| PC | DB2.DBW10 | Int | `faultCode`：检测异常码；正常质量结果为 0 |
| PC | DB2.DBD12 | DInt | `acceptedSeq`：已接受并布防的 requestSeq |
| PC | DB2.DBD16 | DInt | `acceptedPlanReserved`：保留位，布防时 PC 写零，核对接受状态时应为零 |
| PC | DB2.DBB20.50 | String[50] | `resultSn`：结果对应的 partSn，占 DBB20–71，实际长度与字符须相等 |
| PC | DB2 保留：DBX0.6–0.7、DBB1 | 保留 | PC 写 0，PLC 不写 |

**工件 SN（字符串）：** `partSn` 与 `resultSn` 均为 S7 `STRING[50]`，各占 52 字节：首字节声明最大长度，固定 50（`0x32`）；第二字节实际长度 0–50；其后为字符，未用字节 PC 写入时补 0，空串头为 `32 00`。SN 为 1–50 个字符，每个字符为 ASCII 可打印字符 0x20–0x7E，去掉首尾空白后不能为空；只用 S7 STRING，不再支持 200 SMART 字符串。PLC 用字符串赋值或 `S_MOVE` 写 `partSn`，不要按字节拼头字节；比较两段 SN 直接用字符串 `==`。PC 每个轮询周期读取两段 SN，声明长度不为 50 或实际长度大于 50 时该点位读错误，直接进入视觉故障，与是否有 `partStart` 无关；`resultSn` 头字节异常时 PC 在显式复位中写回空串，`partSn` 头字节异常须由 PLC 写回空串后再复位。SN 原样保存与回显，不做 trim 或大小写变换；PLC 侧要求 `resultSn` 与 `partSn` 的实际长度及字符相等。头字节正常但内容不合规的 `partSn` 与请求 payload 不合法（`requestSeq=0`、`planReserved≠0`、`deviceShots≠shotCount`、协议版本不符）同样处理：PC 不布防、不生成结果、不写 `resultCode`，进入视觉故障（`visionFault=1`、`visionReady=0`），PLC 须释放 `partStart` 并执行故障复位（`faultReset` 上升沿）后重新开始。字节示例见 [通讯协议第 10 节](plc-communication-protocol.md#10-四点飞拍示例)。离线产线（仿真）分配形如 `OFFLINE-0001234567` 的 SN（`OFFLINE-` 加 10 位数字），同一离线运行内唯一。

SCL 使用符号 DB 名 `GlueSightPlcArea`（PLC 区）与 `GlueSightPcArea`（PC 区），适用于 S7-1200/1500：导入后在 TIA 的块属性中**明确指定编号**（交互表为 DB4 / DB2），关闭优化访问，核对编辑器偏移量列与上表逐行一致；源文件的符号名称本身不会保证自动分配到指定编号。使用其他 DB 编号时，同步改 PC 模板中的两个 DB 号和测试 peer 的 `--plc-db` / `--pc-db`。

## 3. 正常一件时序

1. **等就绪。** 已完成本连接的显式复位，观察到 PC 心跳变化；`visionReady=1` 且 `visionFault/armed/busy/done=0`，PLC `partStart/partEnd/resultAck=0`。不能将重连前残留的 ready 高电平直接当作新会话就绪。
2. **先 payload，后 start。** PLC 在新工件上递增非零 `requestSeq`，写 SN、产品代码、计划版本、总数、设备计划触发数，清实际触发数和旧 ACK，再最后置 `partStart=1`。上述请求字段在整件期间冻结，包含等待结果和等待 ACK 释放阶段。
3. **等布防承诺。** 必须同时满足 `armed=1`、`busy=1`、`done=partEnd=visionReady=visionFault=0`、`acceptedSeq=requestSeq`、`planReserved=acceptedPlanReserved=0`，且心跳活跃、现场用途和设备互锁满足，才允许现场触发。不以任意单一高电平代替这组条件；当前参考 FB 尚缺部分检查，见第 7 节。
4. **运动与硬触发。** 现场程序按已核对的拍照点次序向唯一设备发脉冲。SCL 在布防确认时冻结单路累计脉冲计数基线，以增量形成本件触发数；计数回退、超计划、设备身份错误均停止新触发。`IssuedPulseCount` 在一件内不得重置/回绕。
5. **先最终计数，后结束。** 运动完成后，deviceTriggers 必须等于 deviceShots，再置 `partEnd=1`；`partStart` 继续保持。PC 核对计数并等待图像及测量完成。
6. **先验证并接收结果，后 ACK。** `done=1` 时核对 `resultSeq=requestSeq`、`resultSn` 与 `partSn` 的实际长度及字符相等，检查结果码及异常码并在 PLC 侧锁存；随后写 `ackSeq=resultSeq`，最后置 `resultAck=1`。有效 NG/检测异常表示已收到了该件结论，ACK 不表示良品放行。
7. **等待双方释放。** PLC 保持 request payload、start/end/ACK，直到 PC `done=0` 且 `busy=0`。再清 start/end/ACK，等 PC 再次 ready，才结束 pending 并允许下一件。

拒收和提前终止也必须完成结果握手：等待布防时允许 PC 直接提交 `ERR_INSPECT=90`；已经布防后，若 `armed` 撤销但 busy 和已接受身份仍正确，PLC 立即停止新触发，限时等待结果。没有经过本地设备计数核对并置 `partEnd` 时，只接受 `90` 与 `91–100` 的已知异常码组合，不能把提前到达的 OK/NG 当有效结论。结果必须满足 seq/SN、armed 低、busy 高和 ready 低，再锁存并 ACK。`visionFault`、心跳失活或身份改变仍直接进入保留 pending 的故障态。

`requestSeq` 与 SN 用途不同：同一 SN 复检也要新事务序号。序号为 DInt，最大 `2147483647`，不能溢出成负数，也不得直接回绕开始生产；到达上限前先处置未结件，在中性基线下由 PLC 工程师把序号清零，再明确复位双方会话。

## 4. 启动、重连与故障恢复

PC 启动、重新建立 S7 连接或发现配置变化后进入复位要求态。PC 持久化的 pending 不会因一次连接成功消失。PLC 参考 FB 首次调用或 `Startup=1` 也进入 `State=90`，默认停止新触发，保留 DB 中现有请求与 pending 标记。

恢复顺序：确认实物工件和动作已被处理 → 查看并记录双方 pending、SN、序号、已收结果 → 操作员设置 `ReconcileConfirmed` → 将本地 `ResetRequest` 先置低，再发新的上升沿。`ReconcileConfirmed` 不应永久接常量 TRUE。复位不是自动重试原件：PC 明确复位会结束未结事务，需先决定这件是隔离、复检还是已有有效结果。

参考 FB 收到合格复位请求后释放 PLC 的 start/end/ACK，将 `faultReset` 保持低至少 1 s，再置高并保持至少 500 ms，直到看到 **ready 的低状态再到高状态**、`pcProtocolVersion=5`，且 armed/busy/done/visionFault 全低。这样可覆盖初始 Fault/ResetRequired，以及无在途时从 Idle 发起明确复位；不会把先前残留的 ready 高电平认作复位成功。若现场扫描错过 ready 低相或 PC 未处理请求，复位超时停在故障态，必须重做新低→高请求，不能直接放行。

如果 PC 还有在途请求，应先让该请求进入明确故障并完成实物处置；不能在运转中清 PLC 输入来强行切下一件。中性基线期间保持 requestSeq、ackSeq 不变。两个方向的心跳都要求观察**变化**；恒定 1 与恒定 0 都不是活性证据。参考 PLC 在 PC 心跳 3 s 不变时停新触发并锁故障；PC 对 PLC 心跳同样使用 3 s 变化超时。

**空闲时（没有在途事务）能恢复的不判故障，也不需要复位：**

- PC 输出区（visionReady/armed/busy/done/visionFault/pcProtocolVersion）偏离空闲状态，典型是 PLC 重启或 DB 重新初始化后读回 0：PC 记审计 `idleOutputsRestored`，先清其他输出、最后置 visionReady，告警提示后继续。60 s 内第 3 次被改写才判故障（有写入方在持续覆盖 PC 区）。
- PLC 侧握手位 partEnd / resultAck 在空闲时仍为 1：PC 不写、不判故障，只显示"等待 PLC 清除"；清除前 PLC 若发起新件，按开始校验判握手故障（分不清残留位属于哪一件）。
- 检测设备（相机等）未就绪：PC 撤下 visionReady 暂停接件，设备恢复后自动重新置位。已经在 ready=1 时到来的 partStart 照常接下，该件按设备丢失判 ERR 98。
- PLC 心跳中断、S7 重连仍进入复位要求态，按上面的恢复顺序处理。

**触发计数错位：** 每件的 ERR 91（缺帧）/ 96（多帧）原因都写明相机、本件应收的计数范围和实际收到的计数，并区分跳号（相机计了数但帧在传输中丢了）、末尾未到、不大于本件基线（被当作上一件的迟到帧）。一件在 partEnd 前就判 ERR（运动超时等）时，PC 在 PLC 确认结果那一刻读取唯一设备的已发触发数推下一件基线，不再按计划数假定。相机漏了一个脉冲时，按计数分不清"漏脉冲"和"上一件末帧迟到 / 相机晚一帧送出"，为了不把帧绑错拍照点，PC 不自动改基线：之后每件都以明确原因判 ERR 91，执行故障复位后重新对齐。PC 在空闲时发起复位会先置 visionFault=1，参考 FB 随之进入 7002，PLC 侧也要复位。

SCL 的故障分支不清请求 payload、start/end/ACK、锁存结果或 pending；只有明确处置后的复位、或成功完成结果释放才结束 pending。配置 TIA 保持性时，明确保存 PLC 请求区、事务序号及 FB pending/结果信息；还要验证 CPU 冷启动、下载和断电造成数据丢失时仍停在复位态，由双方日志和实物决定处置，不能推断旧件成功。

标准 DB 的保持性按整块设置，因此 PC 输出也可能保留旧值；启动时仍需完成双方复位。具体保持性规则见 [Siemens 标准块与优化块访问说明](https://docs.tia.siemens.cloud/r/en-us/v20/programming-basics/blocks-in-the-user-program/blocks-with-optimized-access/basics-of-block-access)。

## 5. 结果与诊断

以下质量语义仅适用于生产事务。示教完整采集返回 `1/0`，可报告的接收失败、中止或超时返回 `90/94`，不形成产品质量结论、不计入生产 OK/NG；布防、计数边界或通信故障可能只置 visionFault。两种用途的有效结果仍须正常 ACK 和释放；没有有效 done 的故障按复位流程处置。DB 中没有可用于区别用途的模式字段。

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
| 92 | 定位失败：位置基准无效、胶偏判不了；另有 NG 时报 NG |
| 93 | 无效测量点 |
| 94 | 拍照计划/数量不匹配 |
| 95 | 配方不可用 |
| 96 | 多余帧 |
| 97 | 运动结束超时 |
| 98 | 设备丢失 |
| 99 | 布防或处理超时、测量执行失败等处理异常，结合 PC 原因查看 |
| 100 | 计划版本、设备绑定或计划内容与 PC 不一致 |

协议身份不一致、回写失败和心跳失活可能只体现为 `visionFault` 与应用错误原因，不保证一定生成可提交的 `done/resultCode=90`；没有有效 done 时不得读取旧结果当新结论。未知结果/异常组合按异常处理，不作为 OK。

SCL `LocalDiagnostic` 是 PLC FB 本地诊断，**不写 DBW78**，也不是 PC faultCode：7000 启动/未知状态需复位；7001 PC 心跳超时；7002 PC 协议故障；7003 阶段超时；7004 新件计划/前置条件错误；7005 布防或结果身份/状态不符；7006 触发计数不符；7007 序号耗尽；7008 在途 payload 被修改；7009 外部设备互锁故障。

## 6. 计划 JSON、版本与实际字段

进入配方后在总览页的“PLC 对接参数”面板读取已发布生产配方开始请求要填的 `productCode`、`planVersion`、`shotCount`（`deviceShots` 填同一值），以及拍照顺序与 Pose 对照表。配方库卡片另有一行摘要；发布确认框列出这次发布 PLC 要同步的变化；读取操作不写 PLC。将这些参数与现场机器人/PLC 路径版本一起评审并配置。所有拍照点都绑定唯一 SDK 设备；多设备配置无法生成计划。

以下是算法示例，不是现场配方。两点均属于唯一设备，因此 deviceShots=2：

```json
{
  "protocolVersion": 5,
  "recipeId": "DEMO",
  "planVersion": 1,
  "shotCount": 2,
  "deviceId": "cam1",
  "deviceShots": 2,
  "shots": [
    {"shotId": "P1", "poseId": "P1", "cameraId": "cam1"},
    {"shotId": "P2", "poseId": "P2", "cameraId": "cam1"}
  ]
}
```

旧点表里的 `planHash` / `acceptedPlanHash` 点名不再自动改名，校验时直接拒绝并提示新点名，需要用 S7 模板重新生成点表。

生产拍照计划不计算或比较内容哈希。`planVersion` 由配方库在保存/发布时逐字段比较后分配：产品代码或拍照点的顺序、编号、Pose、相机任一项变了才换新号，新号比本库已使用的号都大（删掉重建、换产品代码也不复用旧号）；只改限值、检测参数、示教中线、视角选择或"不检"时沿用原号，PLC 不用跟着改。发布页会提示本次发布是否改变拍照计划；改变时，生效后 PLC 侧须同步新的 `planVersion` 与设备计数，否则拒绝布防（故障 100）。复制配方文件造成计划版本重复时，复制件不加载。导出的 shotId、poseId、cameraId 与设备绑定直接供现场评审，PLC 使用计划版本、总数和设备计划触发数核对。`planReserved`（DB4.DBD16）与 `acceptedPlanReserved`（DB2.DBD16）为零值保留位，非零值拒绝布防/在途处理。旧现场程序若仍要求摘要相等，需要同步更新后再联调。

对接参数卡的 `productCode` 取自生产配方，写入 DB4.DBW8。`planVersion` 在 PLC 中为 DInt，PC 分配的版本超过 2147483647 时拒绝导出。一期按拍照点在图像里检测，计划不含工件坐标。`poseId` 是现场机器人 / PLC 程序里的 Pose 标识，同一 Pose 可被多个拍照点引用，因此不要求唯一；机器人程序与物理输出映射不在 JSON 内，需保存在现场的对应表。修改产品代码或拍照点的顺序、编号、Pose、相机绑定后，由配方库分配新版本并重新导出、评审和验证。更换设备或改变设备配置时必须暂停接件，核对所有拍照点绑定，重新导出并人工评审唯一设备与现场触发输出的对应关系，完成复位后再生产。

## 7. SCL 接入限制与状态语义

**参考程序尚需补齐的接入保护：** `AcceptedMatches` 只核对序号，`TriggerPermit` 未完整检查 ready、done 和两处零值保留字段。现场输出逻辑必须在 TriggerPermit 之外同时检查第 3 节全部条件，或补齐 FB 后再接入。发现 armed 撤下时，状态 30 直接转状态 40，未刷新最后已发脉冲计数；PC 却在 ACK 时以 DB 计数维护后续帧归属。因此还须补齐停发后末脉冲计数刷新并验证；未补齐时，提前终止后隔离并复位对齐。参考 FB 也没有示教/生产模式确认，不能把采集的 `1/0` 接到良品输出。以上是当前实现缺口，不是已验收功能。

现场逐步调试与验收清单已于 2026-10-12 按测试范围选择删除。上述协议保护、参考程序缺口与下述状态语义保留；软件回环不证明真实 CPU、I/O 或 SDK 行为。

FB 状态：10 等新件；20 等布防；30 允许运动触发；40 等结果；50 等 PC 清 done/busy；60 等再次 ready；90 故障/启动等待人工处置；91 中性低基线；92 保持 reset 并等待新就绪。`CycleComplete/ResetComplete` 为单扫描事件，`ResultLatched` 和锁存结果保持至下一件，不要直接用一扫描事件控制持续动作。

## 8. 历史软件证据与验证边界

2026-10-11 单设备协议 2（SN 为 UDInt，已被协议 3 取代）合并主分支后验证：Rust 默认回归 417 项通过、44 项默认忽略；`cargo test --offline --lib plc_session -- --include-ignored --test-threads=1 --nocapture` 的 48 项全部通过，包含真实 `ly_plc::PlcEngine` 与 Python peer 的回环 TCP、单设备正常/少触发/多触发、提前 ERR 的 ACK 实际计数、释放、重连和恢复。Python S7 回环测试 23 项通过；前端 43 个文件、963 项通过，类型检查及生产构建通过。Astra High 独立源码复核通过，28 个字段地址/类型/方向、HTML 正文与生成物同步检查通过。两份原生进程恢复脚本仅做语法及字段检查，未重新执行其完整 UI/进程场景。三张时序图按检查流程保持旧版，待协议确认后统一更新。

当前日志保存在 `outputs/plc-single-device-integrated-rust.log`、`outputs/plc-single-device-integrated-session.log`、`outputs/plc-single-device-integrated-frontend.log`，回环场景证据位于 `src-tauri/target/s7-session-tests/`。这些都是软件证据，不能证明双方模式互锁、上述 SCL 缺口或现场硬件已经完成验收。

以下测试数量是先前集成阶段的历史记录，不能作为协议 2、3、4 或 5 的通过记录。协议 4（交互表 V1.0）、协议 5（交互表 V1.1）的软件验证记录分别在对应的 [交互表适配 PR](../decisions/plc-table.md) 描述中：Rust 库回归、`plc_session` 线协议专项（协议 5 的 SN 为 S7 STRING[50]）、Python 测试 PLC 与前端回归。

[Python S7 测试 peer](../../scripts/s7-handshake/README.md) 使用真正的回环 TCP、TPKT/COTP/S7 ReadVar/WriteVar，支持默认点表（协议 5：PLC 区 DB4、PC 区 DB2，PDU 默认 240）、可改的两个 DB 号（`--plc-db` / `--pc-db`，须不同）、故障注入和 PLC 请求/ACK 控制。运行：

```powershell
python -m unittest discover -s scripts/s7-handshake/tests -v
python -u scripts/s7-handshake/s7_test_plc.py --port 0 --plc-db 4 --pc-db 2
```

2026-10-09，该 Python 测试命令在主机环境 21 项通过，含固定黄金报文、bit 邻位保护、多 item 填充、分包/粘包、拒绝、延迟、执行前后断线与重连、心跳冻结和进程控制。Windows 沙箱若拒绝 localhost 连接（WinError 10013），需要在获准主机环境运行同一测试，不能跳过 TCP 后记为通过。

P0 集成分支上，Rust 默认回归 339 项通过、35 项默认忽略；会话专项（`plc_session` 过滤，含 29 项线协议场景）41 项全部通过（57.76 秒）。会话专项在真实 `ly_plc::PlcEngine` 和回环 S7 服务之间运行，覆盖同 SN 不同序号、计划拒收、早/迟 ACK、写拒绝、执行后断线、释放失败、重启、腐坏日志、审计失败、慢落盘、停止心跳、校验后重连禁止布防，以及空闲时 DB 重新初始化自动恢复、反复被改写判故障、PLC 残留握手位等待、设备未就绪撤下 ready。复现命令在 `src-tauri` 下运行：

```powershell
cargo test --offline --lib
cargo test --offline --lib plc_session -- --include-ignored --test-threads=1 --nocapture
```

专项日志在 `src-tauri/target/s7-session-tests/verification.log`；各案例目录保留线报文、最终状态、事务及复位审计和 DB 快照。前端 PLC 的 7 个文件、127 项回归通过，应用/测试类型检查和生产构建通过。命令与分层验证边界见 [测试记录](../testing/TESTING.md#s7-一期握手)。

SCL 当前只完成源码审查和字段/写入方向静态核对，**没有 TIA 编译、PLCSIM、真实 CPU、实际 I/O、单设备三目硬件、现场长稳或实物准确率验收证据**。Python 服务也不模拟 CPU 扫描、优化块权限或真实触发电气特性。
