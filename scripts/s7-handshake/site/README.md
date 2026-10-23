# 现场 PLC 程序草稿（S7-1214C · 握手协议 5）

> **协议 6 提示：** 本草稿和 `gluesight_site.scl` 仍是握手协议 5（达到计划数就停发、按“开始拍摄”边沿逐个出脉冲、结束时计数不等判 7006）。PC 升到协议 6 后不能直接联调，需要先按文末“协议 6 需要的改动点”修改；本仓库本轮不改 `gluesight_site.scl`，由 PLC 工程师修改。

[`gluesight_site.scl`](gluesight_site.scl) 用来替换现场工程（涂胶检测20261022，TIA Portal V17）里的 `test_vision [FB3]` / `VISION_2d [FB1]` 视觉流程。**尚未在 TIA Portal 编译，也未在真实 CPU 上运行。** 协议依据：[PLC 通讯协议](../../../docs/integration/plc-communication-protocol.md) 第 2–8 节。

## 为什么重写

现场程序按 step1–step5 自拼流程：step1 清空 `partSn` 后直接置 `partStart`，没有先写 `requestSeq`、产品代码与计划；`test_vision` 固定发 4 个 1 ms 脉冲，不看 `armed` / `acceptedSeq`，也不写 `deviceTriggers`。协议 5 的布防、计数、结果校验、ACK、释放与显式复位都不在其中，改不如重写。现场的 `plc_send_vision [DB4]`、`vision_send_plc [DB2]` 布局已与协议 5 一致，**保留不动**。

## 结构

| 块 | 作用 |
| --- | --- |
| `GlueSightHandshake`（FB） | 协议 5 状态机，只读写 `"plc_send_vision"`（DB4）与 `"vision_send_plc"`（DB2）。相对仓库参考 FB：补齐完整发脉冲条件；布防后每个扫描从累计计数刷新 `deviceTriggers`，提前撤 armed 时末计数在 ACK 前已写入；运动中 PC 置回 visionReady 判 7005；请求前检查 SN 非空、产品代码 / 版本 / 次数大于 0；事务序号由调用方断电保持传入；复位时可选清零序号；等 PC 再就绪的 60 状态不计时 |
| `GlueSightStation`（FB） | 现场适配：机器人信号 → 请求与触发；相机硬触发输出（同一处门控与计数）；结果 → 机器人结果位；复位操作；示教用途在新件提交时锁存。以多重背景调用握手层 |

状态码：10 空闲、20 等布防、30 运动触发、40 等结果、50 等 PC 释放、60 等 PC 再就绪、90 故障、91/92 复位中。

## 导入步骤

1. 先**备份工程**（项目 → 归档）。
2. 项目树 → 外部源文件 → 添加新的外部文件 → 选 `gluesight_site.scl`（UTF-8 带 BOM、CRLF，与 TIA 导出的源文件格式相同）。
3. 右键该源文件 →“从源生成块”，生成 `GlueSightHandshake`、`GlueSightStation` 两个 FB。
4. 在 `Main [OB1]` 中**禁用**原来调用 `test_vision` / `VISION_2d` 的程序段（先不要删），确认原程序不再驱动相机触发输出和 `plc_send_vision`。
5. 在 OB1 新建程序段调用 `GlueSightStation`，生成背景 DB（如 `GlueSightStation_DB`），按下方 TODO 接线。
6. 编译。报错截图发回来修改。
7. 打开背景 DB，确认 `LastSeq` 的“保持”列已勾选，其余变量不保持（见 TODO-13）。

OB1 调用示例（地址以现场确认为准）：

```
"GlueSightStation_DB"(
    Startup            := "FirstScan",                                   // TODO-0，必须接
    NewPart            := "ROBOT_SEND_PLC".ROBOT_OUTPUT.新工件到位,       // TODO-1
    ShotRequest        := "ROBOT_SEND_PLC".ROBOT_OUTPUT.开始拍摄,         // TODO-2
    MotionComplete     := "ROBOT_SEND_PLC".ROBOT_OUTPUT.全部拍摄完成,     // TODO-3
    ResetButton        := "HMI".视觉复位,                                // TODO-4
    ReconcileConfirmed := "HMI".已处置未结件,                            // TODO-4
    SafetyFault        := "急停" OR "机器人报警",                         // TODO-5
    TeachMode          := FALSE,                                        // TODO-6
    PartSn             := "HMI".工件SN,                                  // TODO-7
    ProductCode        := 1,                                            // TODO-7
    PlanVersion        := 41,                                           // TODO-7
    DeviceShots        := 4,                                            // TODO-7
    PulseWidth         := T#10ms,                                       // TODO-8
    CameraTrigger      => "相机触发",                                    // TODO-9
    MotionPermit       => "PLC_SEND_ROBOT".视觉允许运动,                  // TODO-10
    ReadyForPart       => "PLC_SEND_ROBOT".视觉可接件);                   // TODO-10
// 结果位：TODO-11
"PLC_SEND_ROBOT".合格 := "GlueSightStation_DB".ResultBits[0];
...
```

## 待现场确认（TODO）

| 编号 | 问题 | 草稿当前做法 |
| --- | --- | --- |
| TODO-0 | 首扫描标志用哪个：系统存储器“FirstScan”还是 OB100 置位的位？ | 输入 `Startup`，**必须接**：上电后进入 90（7000），等显式复位 |
| TODO-1 | “新工件到位”的来源与时机。机器人 / 线体是否会等 `ReadyForPart=1` 再给？ | 上升沿提交请求；视觉未就绪时的上升沿不提交，置 `NewPartIgnored` 提示，机器人需重新给 |
| TODO-2 | **相机触发怎么接**：机器人每到一个拍照点给一个“开始拍摄”脉冲、由 PLC 输出到相机（草稿按此写）？还是机器人直接接相机、PLC 只计数？或 PLC 自己按时间发？ | 每个上升沿在 `TriggerPermit=1` 时发一个脉冲并计数 |
| TODO-2 | 机器人触发信号的脉宽与最短间隔。短于 PLC 扫描周期时 OB1 边沿检测会漏，需改用硬件中断 OB40 或高速计数 | 按 OB1 边沿检测 |
| TODO-3 | 机器人“本件运动结束”信号是哪个（`全部拍摄完成`？）。**PC 提前终止时也要给**；新件开始前必须已复位为 0 | 30 状态内锁存，延后一个扫描交给握手层；新件提交时仍为 1 → 7004 |
| TODO-4 | 复位按钮、“已处置未结件”确认、序号清零放在触摸屏哪里 | 三个输入；确认不可常接 TRUE |
| TODO-5 | 哪些外部故障要停视觉触发（急停、安全门、机器人报警…） | 任一为 1 → 握手层 7009，需复位 |
| TODO-6 | 现场是否走示教整圈采集？如果走，示教 / 生产用途由谁切换 | 新件提交时锁存 `TeachMode`；事务进行中切换 → 7013 停线。示教时不输出质量结论，只给 `CaptureComplete` / `CaptureFailed` |
| TODO-7 | SN、产品代码、计划版本、触发次数从哪里来（触摸屏、MES、机器人、配方表）。计划版本与触发次数以 PC 配方总览“PLC 对接参数”为准 | 4 个输入；PLC 只查 SN 非空、代码 / 版本 / 次数大于 0（→ 7004）。SN 全空格或含不可打印字符由 PC 拒收，表现为 7002 |
| TODO-8 | 相机硬触发要求的脉宽（相机 Line0 规格） | 默认 10 ms，实际按扫描周期取整 |
| TODO-9 | 相机触发输出的 Q 点地址；原 `test_vision` 用的是哪个点 | 输出 `CameraTrigger` |
| TODO-10 | 给机器人的“视觉已布防，允许运动”“视觉可接件”用哪两个点；**PC 提前终止时 `MotionPermit` 掉电平，机器人是停止还是走完本件路径** | 两个输出。机器人走完路径时，剩余拍照请求只计入 `IgnoredShotRequests`、不发脉冲；`ReadyForPart` 等到机器人给出运动结束才恢复。机器人选择停止且不给运动结束时，需在 10 状态按一次复位 |
| TODO-11 | 结果位按交互表 Q1152.0–Q1153.7 输出是否沿用；是否还要“结论有效”“视觉故障”两个位给机器人 | `ResultBits[0..15]` 对应 Q1152.0…Q1153.7，另有 `ResultValid`、`VisionAlarm` |
| TODO-12 | 示教整圈采集的收尾窗口。PC 收尾窗口 + 保存耗时超过 10 s 时要加大 `ResultTimeout` | 默认 10 s |
| TODO-13 | `LastSeq` 断电保持是否生效 | 声明在现场层（单背景）的 `VAR RETAIN`，以 IN_OUT 传给握手层 |
| TODO-14 | PC 生产检测设置里的 `motion_ms`。`MotionTimeout` 须大于 `motion_ms + drain_ms + proc_ms`，PC 的运动超时结果 90/97 才能先于 PLC 的 7003 到达 | `MotionTimeout` 默认 40 s（按 PC 默认 30 s + 1 s + 3 s 留余量） |

## 结果位对照（`ResultBits`）

| 下标 | 交互表地址 | 条件 |
| --- | --- | --- |
| 0 | Q1152.0 | resultCode=1 合格 |
| 1 | Q1152.1 | resultCode=2 局部超差但符合条件 |
| 2–5 | Q1152.2–Q1152.5 | resultCode=11–14（位置偏差、绝对限值、断胶、胶宽） |
| 6–15 | Q1152.6–Q1153.7 | resultCode=90 且 faultCode=91–100（缺帧 … 版本 / 绑定 / 计划不一致） |

请用 Bool 逐位赋值，不要把 16 个位拼成 Word 写 QW1152：西门子 Word 高字节在前，`QW1152` 的第 0 位是 Q1153.0。结论在 ACK 时写出，保持到下一件提交。

## 诊断码

| 码 | 来源 | 含义 |
| --- | --- | --- |
| 7000 | 握手层 | 启动或未知状态，需复位 |
| 7001 | 握手层 | PC 心跳 3 s 未变化 |
| 7002 | 握手层 | PC 置了 visionFault（含 PC 拒收 SN 等请求内容） |
| 7003 | 握手层 | 阶段超时（布防 / 运动 / 结果 / 释放 / 复位；等 PC 再就绪不计时） |
| 7004 | 握手层 | 新件参数不合法（SN 空、代码 / 版本 / 次数 ≤ 0、运动结束信号未清、PC 协议版本不是 5） |
| 7005 | 握手层 | 布防或结果的身份 / 状态与本件不符 |
| 7006 | 握手层 | 触发计数不符（超过计划、回退、运动结束时不等于计划） |
| 7007 | 握手层 | 事务序号到上限，需带 `SeqRestart` 复位 |
| 7008 | 握手层 | 在途请求内容被改动 |
| 7009 | 握手层 | 外部故障（`SafetyFault` 或现场层故障，具体看 `StationDiagnostic`） |
| 7010 | 现场层 | 未布防就收到拍照请求（机器人没等 `MotionPermit`） |
| 7011 | 现场层 | 上一个触发脉冲未结束又来请求（会丢帧） |
| 7012 | 现场层 | 拍照请求多于计划次数 |
| 7013 | 现场层 | 事务进行中切换了示教 / 生产用途 |

## 验收（导入后先离线核对，再联调）

1. 两个 DB 偏移：`plc_send_vision` 最后一项 `partSn` 偏移 38.0、DB 长 90；`vision_send_plc` 的 `resultSn` 偏移 20.0、DB 长 72。16 位字段均为 Int。
2. PC 连上后在 PLC 页看到 `plcHeartbeat` 每 500 ms 翻转，`protocolVersion=5`。
3. 上电后状态为 90（7000）。触摸屏先给“已处置”确认，再按复位：状态 91 → 92 → 10，PC 侧复位成功、`visionReady=1`。
4. 走一件：`NewPart` → 20 → PC 布防 → 30、`MotionPermit=1` → 机器人逐点请求，`deviceTriggers` 逐个增加 → 运动结束 → `partEnd=1` → PC 给结果 → ACK → 释放 → 回到 10。核对机器人收到的结果位。
5. 断电重启后再走一件，`requestSeq` 接着上次递增而不是从 1 开始。
6. 异常注入，每种都要能按第 3 步复位恢复：未布防时给拍照请求（7010）；多给一个拍照点（7012）；运动中拔网线（7001）；事务中切换示教开关（7013）。
7. PC 提前终止（如 PC 页停止相机）：PLC 收到 90/9x 并正常 ACK、释放，不进故障；机器人走完路径期间的拍照请求计入 `IgnoredShotRequests`、不发脉冲；机器人给出运动结束后 `ReadyForPart` 恢复。

## 协议 6 需要的改动点

协议 6（相机使能期间的固定频率脉冲串、三台相机共用一个触发、`shotCount`/`deviceShots` 为预期总数 ΣN_exp、`deviceTriggers` 只作参考）与本草稿的“逐个拍照点发脉冲”模型冲突。以下行号对应当前 `gluesight_site.scl`，参考实现见仓库的 [`phase1_plc.scl`](../phase1_plc.scl)（版本 6，已删除两处计数 7006）；协议依据见 [PLC 通讯协议](../../../docs/integration/plc-communication-protocol.md) 第 2、4.2、5、13 节。**以下只是改动点清单，草稿本身未改动，也未在 TIA Portal 编译或联调。**

| # | 位置 | 现在 | 协议 6 要求 |
| --- | --- | --- | --- |
| 1 | `GlueSightHandshake`：:102 `protocolVersion := INT#5`；:239、:402、:430 的 `pcProtocolVersion` 判断；文件头注释 :1、:5、:7 | 版本 5 | 全部改为 6（新件 7004 条件、复位成功判断、`ReadyForPart` 都用到）；协议 5 的 PC 与程序不能混用 |
| 2 | `GlueSightHandshake` 触发计数区 :172–:184 | 已发数超过 `FrozenDeviceShots` 置 `CountError`（7006） | 删除“超过计划数”判定，连续脉冲串下已发数允许超过 `shotCount`；`deviceTriggers` 是 Int，差值超过 32767 时饱和为 32767，不得回绕成负数；仅保留 `IssuedPulseCount < CounterBase`（累计计数回退）判 7006 |
| 3 | `GlueSightHandshake` 结束 :302–:309 | `MotionComplete` 时 `CurrentTriggers <> FrozenDeviceShots` 判 7006 | 删除计数相等判断：运动结束、脉冲串停发后，先写最终 `deviceTriggers`，再置 `partEnd`、转状态 40；不要求等于 `shotCount` |
| 4 | `GlueSightHandshake` :441 `TriggerPermit` | `MotionPermit AND (CurrentTriggers < FrozenDeviceShots)`，达到计划数就停发 | 去掉 `CurrentTriggers < FrozenDeviceShots`；停发条件只剩 `MotionPermit`（含相机使能为高由现场层门控）；`PlannedShots`、`TriggersIssued` 输出（:444–:445）只作显示 |
| 5 | `GlueSightStation` 输入 `ShotRequest`（:455）、边沿检测（:522–:523）、相机硬触发区（:591–:618） | 机器人每个拍照点给一个上升沿，PLC 发一个脉冲；上一个脉冲未结束又来请求判 7011；未布防就请求判 7010；多于计划次数判 7012 | 改为机器人“相机使能”电平输入（整条胶路只拉高一次，弯道不拉低；起头稳定后拉高、收尾前拉低）；使能为高且 `TriggerPermit=1` 时，PLC 以固定频率（按视野选定，约 10–25 Hz，相机上限 66 fps）输出脉冲串，同时触发三台相机，每个实际输出脉冲使 `IssuedPulseCount` 加一；脉冲频率与 `PulseWidth` 须匹配（周期大于脉宽）；删除 7012（不再有计划拍照点数）；7010 改为“未布防就使能为高”；7011 改为“脉冲周期小于脉宽”类的配置诊断，或删除 |
| 6 | `GlueSightStation` 输入 `DeviceShots`（:465）与 TODO-7 | “须等于机器人拍照点数” | 填 PC 计划导出的 `expectedTotal`（ΣN_exp），`shotCount` 与 `deviceShots` 仍写同一值；`PlanVersion` 同取导出值；重新标定后两者都会变，须同步 |
| 7 | `GlueSightStation` 输入 `MotionComplete`（:456）与 TODO-3 | 机器人本件运动及全部拍照点结束 | 含义改为“相机使能已拉低、脉冲串已停发”（PC 提前终止或机器人提前终止时也要给）；其后才写最终计数并置 `partEnd` |
| 8 | 机器人“本件未检”DO（新增） | 草稿没有 | 机器人等不到 `MotionPermit` / 视觉可接件，超时后照常涂胶并置“本件未检”；PLC 对该件不提交 `partStart`（`NewPart` 在视觉未就绪时已不提交，可复用 `NewPartIgnored`），或提前终止；该件按未检处置（隔离、复检）。PC 看不到这个 DO，PC 没有该件记录 |
| 9 | 结果分发（`ResultBits`） | 按 `resultCode` 直接输出 | 提前 `partEnd` 时 PC 仍可能给出 `resultCode=1` 并带 `short_part` 标记（标记不在 DB 里）；已置“本件未检”的件不得仅凭 `resultCode=1` 放行 |
| 10 | 诊断码表与 TODO-2、TODO-3、TODO-7、TODO-10 | 按协议 5 描述 | 7004 的“PC 协议版本不是 5”改为 6；7006 只保留“累计计数回退”；7010–7012 按第 5 项调整；TODO 条目的描述同步 |
| 11 | 验收清单第 2、4、6 项 | `protocolVersion=5`；`deviceTriggers` 逐个增加；“多给一个拍照点”判 7012 | 改为 `protocolVersion=6`；使能期间 `deviceTriggers` 随脉冲串增加，结束时不要求等于 `shotCount`；删除 7012 异常注入；增加“相机使能拉低后无新脉冲”“使能为高但未布防时无脉冲”两项 |

改完后需重新按本文“导入步骤”和“验收”走一遍；仓库不替 PLC 工程师改该文件，以免在没有 TIA 编译和实机验证的情况下留下看似可用的程序。
