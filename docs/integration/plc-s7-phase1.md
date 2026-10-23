# S7 握手契约与现场接入

线上协议版本保持 **6**。DB4 / DB2 布局、字段名、类型、偏移和握手顺序不变，完整契约见 [PLC 通讯协议](plc-communication-protocol.md)。

## 高电平连续采集

PLC 在相机已布防、事务身份有效、运动采集使能为高期间向 Line0 输出高电平；结束前、故障时或 PC 撤销 armed 时立即拉低。相机自身按软件设定帧率连续曝光，默认 10 fps。PLC 不产生逐帧脉冲，也不计曝光次数。

`deviceTriggers`（DB4.DBW32，Int）保留，PLC 始终写 0。PC 不用该字段做丢帧告警、收尾判断或下一件帧号基线。三台相机按各自 SDK 帧号减基线配组；相机间帧数差异、帧号缺口和传输丢帧用于诊断，不声称曝光同步。

`shotCount` / `deviceShots` 仍是 PC 导出计划的 expectedTotal，生产核对计划身份。整轮示教采集使用估算值，不再精确核对收图数；现场参考 FB 要求估算值大于 0。

## 收尾与隔离

PLC 拉低采集使能并结束运动后置 partEnd。PC 等待完整 drain 窗口，处理该窗口内已接受帧，测量或写盘完成后发布结果；不因 PLC 计数或已收帧数提前完成。结果确认、双方释放、追溯完成前不能开始下一件。下一件基线取本会话已经观察到的最高 SDK 帧号。

Software 单帧触发只用于标定和调试，不参与 PLC 整轮示教与生产。旧拍照点配方从未部署，不保留运行时兼容或迁移。

## 参考程序与验证

- 通用 FB：[phase1_plc.scl](../../scripts/s7-handshake/phase1_plc.scl)。`TriggerPermit` 是已经包含 MotionActive 的高电平门控输出。
- 现场分层 FB：[现场接入说明](../../scripts/s7-handshake/site/README.md)。`MotionPermit` 用于机器人运动放行，`CameraTrigger` 用于 Line0。
- 测试 PLC：[s7_test_plc.py](../../scripts/s7-handshake/s7_test_plc.py)，协议保持 6，可向预留字段写不同值验证 PC 不依赖它。

SCL 须在 TIA Portal 编译，并在真实 CPU 上验证信号顺序与故障拉低。软件测试不替代真实触发接线、相机节点和帧率验证。

## 4. 启动、重连与故障恢复

PC 启动、重新建立 S7 连接或发现配置变化后进入复位要求态。PC 持久化的 pending 不会因一次连接成功消失。PLC 参考 FB 首次调用或 `Startup=1` 也进入 `State=90`，默认停止新触发，保留 DB 中现有请求与 pending 标记。

恢复顺序：确认实物工件和动作已被处理 → 查看并记录双方 pending、SN、序号、已收结果 → 操作员设置 `ReconcileConfirmed` → 将本地 `ResetRequest` 先置低，再发新的上升沿。`ReconcileConfirmed` 不应永久接常量 TRUE。复位不是自动重试原件：PC 明确复位会结束未结事务，需先决定这件是隔离、复检还是已有有效结果。

参考 FB 收到合格复位请求后释放 PLC 的 start/end/ACK，将 `faultReset` 保持低至少 1 s，再置高并保持至少 500 ms，直到看到 **ready 的低状态再到高状态**、`pcProtocolVersion=6`，且 armed/busy/done/visionFault 全低。这样可覆盖初始 Fault/ResetRequired，以及无在途时从 Idle 发起明确复位；不会把先前残留的 ready 高电平认作复位成功。若现场扫描错过 ready 低相或 PC 未处理请求，复位超时停在故障态，必须重做新低→高请求，不能直接放行。

如果 PC 还有在途请求，应先让该请求进入明确故障并完成实物处置；不能在运转中清 PLC 输入来强行切下一件。中性基线期间保持 requestSeq、ackSeq 不变。两个方向的心跳都要求观察**变化**；恒定 1 与恒定 0 都不是活性证据。参考 PLC 在 PC 心跳 3 s 不变时停新触发并锁故障；PC 对 PLC 心跳同样使用 3 s 变化超时。

**空闲时（没有在途事务）能恢复的不判故障，也不需要复位：**

- PC 输出区（visionReady/armed/busy/done/visionFault/pcProtocolVersion）偏离空闲状态，典型是 PLC 重启或 DB 重新初始化后读回 0：PC 记审计 `idleOutputsRestored`，先清其他输出、最后置 visionReady，告警提示后继续。60 s 内第 3 次被改写才判故障（有写入方在持续覆盖 PC 区）。
- PLC 侧握手位 partEnd / resultAck 在空闲时仍为 1：PC 不写、不判故障，只显示"等待 PLC 清除"；清除前 PLC 若发起新件，按开始校验判握手故障（分不清残留位属于哪一件）。
- 检测设备（相机等）未就绪：PC 撤下 visionReady 暂停接件，设备恢复后自动重新置位。已经在 ready=1 时到来的 partStart 照常接下，该件按设备丢失判 ERR 98。
- PLC 心跳中断、S7 重连仍进入复位要求态，按上面的恢复顺序处理。

**帧号与丢帧：** 每台相机按 SDK 帧号减本会话基线配组，不按到达顺序。单台缺帧标记检测不完整；三台相机整件一帧都没有测成时返回 ERR 91。某台相机与另外每一台帧数都相差超过 1 时告警，不改变 OK/NG。下一件基线仅取观察到的最高 SDK 帧号。deviceTriggers 不参与告警或收尾；partEnd 后等待完整 drain 窗口。帧数超出标定值及容差只记帧数异常，当前不产生 ERR 96。PC 在空闲时发起复位会先置 visionFault=1，参考 FB 随之进入 7002，PLC 侧也要复位。

SCL 的故障分支不清请求 payload、start/end/ACK、锁存结果或 pending；只有明确处置后的复位、或成功完成结果释放才结束 pending。配置 TIA 保持性时，明确保存 PLC 请求区、事务序号及 FB pending/结果信息；还要验证 CPU 冷启动、下载和断电造成数据丢失时仍停在复位态，由双方日志和实物决定处置，不能推断旧件成功。

标准 DB 的保持性按整块设置，因此 PC 输出也可能保留旧值；启动时仍需完成双方复位。具体保持性规则见 [Siemens 标准块与优化块访问说明](https://docs.tia.siemens.cloud/r/en-us/v20/programming-basics/blocks-in-the-user-program/blocks-with-optimized-access/basics-of-block-access)。


## 6. 计划导出 JSON、版本与实际字段

进入配方后在总览页的“PLC 对接参数”面板读取已发布生产配方的计划导出（`PlanExport`，可复制或另存 JSON）：开始请求要填的 `productCode`、`planVersion`，以及 `shotCount` 与 `deviceShots` 都要填的预期总数 `expectedTotal`（ΣN_exp），另有三台相机、段序列、各段 N_exp 与容差、整件容差 `toleranceTotal`。配方库卡片另有一行摘要；发布确认框列出这次发布 PLC 要同步的变化；读取操作不写 PLC。将这些参数与现场机器人/PLC 路径版本一起评审并配置。计划的三台相机必须是三个互不相同的独立设备，共用一个触发；未标定（没有 ΣN_exp）的配方不能发布生产，也不能生成可核对的计划。

以下是算法示例，不是现场配方。三段（直线、弯道、直线）预期帧数 20、8、22，容差各 1，因此 `expectedTotal=50`、`toleranceTotal=3`；各段 `expectedFrames` 在分段算法就绪前可以为 `null`，此时只有 `expectedTotal` 来自整件多轮采集：

```json
{
  "protocolVersion": 6,
  "recipeId": "DEMO",
  "productCode": 7,
  "planVersion": 41,
  "cameras": ["cam1", "cam2", "cam3"],
  "segments": [
    {"index": 1, "kind": "straight", "lengthMm": 120.0, "expectedFrames": 20, "tolerance": 1, "inspectRange": {"aMm": 8.0, "bMm": 120.0}},
    {"index": 2, "kind": "corner", "lengthMm": 40.0, "expectedFrames": 8, "tolerance": 1, "inspectRange": null},
    {"index": 3, "kind": "straight", "lengthMm": 130.0, "expectedFrames": 22, "tolerance": 1, "inspectRange": {"aMm": 0.0, "bMm": 118.0}}
  ],
  "expectedTotal": 50,
  "toleranceTotal": 3
}
```

旧点表里的 `planHash` / `acceptedPlanHash` 点名不再自动改名，校验时直接拒绝并提示新点名，需要用 S7 模板重新生成点表。

生产计划不计算或比较内容哈希。`planVersion` 由配方库在保存/发布时逐字段比较后分配：产品代码、段序列（序号、类型）、各段 N_exp、整件 ΣN_exp 或相机绑定任一项变了才换新号，新号比本库已使用的号都大（删掉重建、换产品代码也不复用旧号）；只改限值、容差、胶偏参考、检测区间或段长时沿用原号，PLC 不用跟着改。**重新标定使 N_exp 或 ΣN_exp 变化时会分配新的 `planVersion`，PLC 须同步新的 `shotCount`/`deviceShots`**，PC 在界面提示；不同步则拒绝布防（生产请求数量不符 90/94，版本不符 90/100）。复制配方文件造成计划版本重复时，复制件不加载。导出的段序列、相机与各段 N_exp 直接供现场评审，PLC 使用计划版本和 ΣN_exp 核对。`planReserved`（DB4.DBD16）与 `acceptedPlanReserved`（DB2.DBD16）为零值保留位，非零值拒绝布防/在途处理。旧现场程序若仍要求摘要相等，或仍以“已发数达到 shotCount 就停发 / 结束时数量必须相等”为逻辑，需要同步更新后再联调（见 [现场接入说明](../../scripts/s7-handshake/site/README.md)）。

对接参数卡的 `productCode` 取自生产配方，写入 DB4.DBW8。`planVersion` 在 PLC 中为 DInt，PC 分配的版本超过 2147483647 时拒绝导出；ΣN_exp 必须不超过 32767（PLC Int）。一期按帧在图像里检测，计划不含工件坐标；机器人程序与物理输出映射不在 JSON 内，需保存在现场的对应表。更换相机或改变设备配置时必须暂停接件，核对三台相机绑定，重新导出并人工评审相机与现场触发输出的对应关系，完成复位后再生产。


## 8. 历史软件证据与验证边界

2026-10-11 单设备协议 2（SN 为 UDInt，已被协议 3 取代）合并主分支后验证：Rust 默认回归 417 项通过、44 项默认忽略；`cargo test --offline --lib plc_session -- --include-ignored --test-threads=1 --nocapture` 的 48 项全部通过，包含真实 `ly_plc::PlcEngine` 与 Python peer 的回环 TCP、单设备正常/少触发/多触发、提前 ERR 的 ACK 实际计数、释放、重连和恢复。Python S7 回环测试 23 项通过；前端 43 个文件、963 项通过，类型检查及生产构建通过。Astra High 独立源码复核通过，28 个字段地址/类型/方向、HTML 正文与生成物同步检查通过。两份原生进程恢复脚本仅做语法及字段检查，未重新执行其完整 UI/进程场景。三张时序图按检查流程保持旧版，待协议确认后统一更新。

当前日志保存在 `outputs/plc-single-device-integrated-rust.log`、`outputs/plc-single-device-integrated-session.log`、`outputs/plc-single-device-integrated-frontend.log`，回环场景证据位于 `src-tauri/target/s7-session-tests/`。这些都是软件证据，不能证明双方模式互锁、上述 SCL 缺口或现场硬件已经完成验收。

以下测试数量是先前集成阶段的历史记录，不能作为协议 2、3、4、5 或 6 的通过记录。协议 4（交互表 V1.0）、协议 5（交互表 V1.1）的软件验证记录分别在对应的 [交互表适配 PR](../decisions/plc-table.md) 描述中，协议 6 的软件验证记录在分段连拍的 PR 描述中：Rust 库回归、`plc_session` 线协议专项（协议 5 的 SN 为 S7 STRING[50]）、Python 测试 PLC 与前端回归。

[Python S7 测试 peer](../../scripts/s7-handshake/README.md) 使用真正的回环 TCP、TPKT/COTP/S7 ReadVar/WriteVar，支持默认点表（协议 6，布局同协议 5：PLC 区 DB4、PC 区 DB2，PDU 默认 240）、可改的两个 DB 号（`--plc-db` / `--pc-db`，须不同）、故障注入和 PLC 请求/ACK 控制。运行：

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

SCL 当前只完成源码审查和字段/写入方向静态核对，**没有 TIA 编译、PLCSIM、真实 CPU、实际 I/O、三相机共用触发的硬件、现场长稳或实物准确率验收证据**。Python 服务也不模拟 CPU 扫描、优化块权限或真实触发电气特性。
