# 面向 Agent 分析的运行日志改造方案

> 状态（2026-10-24）：P1 已实施（分支 `codex/agent-logging`，基于合入 PR #174 后的 main），P2～P4 实施中；原 P5 存量迁移经用户决定不做。编码表见 [agent-logging-codes.md](agent-logging-codes.md)。
>
> P1 与设计的差异：节拍侧按编码写日志的 `cycle::log_event` 推迟到 P2，随第一个调用方一起加入，P1 不留无调用方的代码；`buildTime` 是构建脚本最近一次运行的时间（源码、分支或 HEAD 变化时重跑），`gitSha` 的 `-dirty` 标记按构建时的已跟踪文件状态判断。

## 1. 目标与依据

运行日志的首要读者是排查现场问题的 Agent：拿到一个诊断包，不再向现场追问截图，就能还原“发生了什么、按什么顺序、双方寄存器是什么值、谁触发的”。

2026-10-24 京航联调是这次改造的依据。一份 1.8 万行的日志里：

- 13380 行是逐帧的“游离帧”，4857 行是每拍重复的同一条“故障”，有效事件不到 30 条；
- 示教握手的 PLC 寄存器值完全不在日志里，PLC 停在“已接受、回显 0”还是一直空闲，只能靠故障文案差异和游离帧时间倒推，最后仍要现场截 TIA；
- 一次 PLC 侧可见的“视觉故障=1”没有任何原因记录；
- 复位、开始 / 取消采集等操作员动作只能从 PLC 写入痕迹倒推；
- 运行日志用毫秒时间戳、PLC 日志 CSV 用本地时间字符串，对照前要换算；看不出现场运行的是哪个构建。

现状盘点见附录 A。

## 2. 设计原则

1. **证据优先于结论**：状态迁移记下迁移前后的值和双方寄存器，而不仅是最终的故障文字。
2. **一件事一行**：持续状态只在出现、变化、结束时记录；高频重复聚合成计数摘要，计数不能丢。
3. **机器读 `code` 与 `data`，人读 `ev` 与 `msg`**：文案可以改，`code` 和 `data` 的键是对 Agent 的契约，改动要走兼容规则。
4. **凡是 PLC 看得见的，日志里都有原因**：每次写出故障输出、撤销视觉就绪，都对应一条带来源的事件。
5. **不静默丢日志**：写失败、聚合、节流都要留下计数；宁可同步写慢一点，也不在分析时制造“这段时间什么都没发生”的假象。

## 3. 行格式（日志 schema 2）

在现有 JSONL 上增量扩展，旧行照常可读。

```json
{"ts":1792837092059,"level":"error","t":"2026-10-24T18:18:12.059+08:00","code":"teach.timeout.cancel_ack","ev":"示教超时","msg":"PLC 未确认取消，握手编号 8","teachId":8,"captureRound":"capture-1792837071896-0","data":{"pc":[1,40,8,2,0],"plc":[1,10,0,0,0],"waitedMs":10012}}
```

| 字段 | 说明 |
|---|---|
| `ts`、`level` | 不变，且必须仍是前两个键：`applog::head()` 靠 `{"ts":…,"level":"…` 开头做快速筛选与分段裁剪 |
| `t` | 新增。带 UTC 偏移的本地时间（RFC 3339，毫秒），与 PLC 日志、现场叙述直接对照 |
| `code` | 新增。稳定事件编码，见第 4 节；旧行没有 |
| `ev`、`msg` | 不变。中文事件名与说明，面向人，可随文案调整 |
| 关联字段 | 现有 `cycleId / sn / requestSeq / recipeId / shotId / k / camera / view / seq / stage`，新增 `teachId`（示教握手编号）、`captureRound`（整圈采集轮次）、`source`（触发来源，见 5.4） |
| `data` | 结构化数值。每个 `code` 的键在编码表里固定 |

兼容规则：

- 新字段一律 `Option` + `#[serde(default)]`，否则旧行解析失败会被查询、时间线、诊断包、中断补记录静默跳过。
- `t`、`code` 作为 `Record` 顶层字段，紧跟 `level` 之后；`cycle/interrupted.rs` 测试里的 `Record` 字面量随之补字段。
- 序列化保持紧凑无空格：`CycleScan` 用 `"stage":"partStart"` 等子串粗筛。
- 启动事件的 `data.logSchema = 2`；诊断包清单同步写出。
- `data` 键只增不改；要改含义时换新 `code`，旧 `code` 在编码表中标“停用”保留一个版本。

## 4. 事件编码

### 4.1 命名

`<域>.<事件>[.<细分>]`，全小写 ASCII，下划线分词。域：

| 域 | 范围 |
|---|---|
| `app` | 启动、配置摘要、panic、日志自身（写失败、聚合摘要） |
| `alarm` | 节拍界面报警（去重后的写出与汇总） |
| `op` | 操作员命令（界面或 IPC 调用） |
| `plc` | PLC 链路：连接、断开、ly_plc 引擎告警 |
| `s7` | S7 生产握手会话 |
| `teach` | 示教握手（DB6 / DB12） |
| `part` | 工件生命周期：开始、布防、结束、结论 |
| `frame` | 帧到达与归属：游离、迟到、截止后、配组、丢帧 |
| `measure` | 测量与判定 |
| `camera` | 相机连接、配置下发、取图 |
| `fault` | 节拍进入 / 维持 / 解除故障 |
| `recipe`、`capture`、`calib` | 配方发布与迁移、整圈采集、标定 |
| `storage`、`audit` | 录制、清理、持久追溯 |

### 4.2 代码组织

- 新增 `src-tauri/src/applog/codes.rs`：每个事件一条常量 `Event { code, ev, level }`，调用方写 `log_event(app, &codes::TEACH_PLC_CHANGED, msg, fields)`，不再手写 `ev` 字符串和级别。
- 现有 `cycle::log / log_with / trace` 继续保留，写出的行 `code` 为空。不做全量迁移：只有 P2～P4 新增或改动的事件带 `code`，没有 `code` 的行按 `ev` 与 `msg` 理解。
- `docs/architecture/agent-logging-codes.md` 为编码表（code、ev、级别、触发时机、`data` 键）。由测试保证与 `codes.rs` 一致（见第 10 节）。

### 4.3 需要拆分的现有事件

只拆分 P2～P4 本身要改动的事件：`故障`（P2 改为 `fault.enter`）、`PLC {category}` 与 `相机` 断线重试（P3 聚合时改编码）；其余同名事件保持原样。

| 现状 | 问题 | 拆分 |
|---|---|---|
| `相机`（5 处） | 同名不同义 | `camera.connected`、`camera.reconnect_failed`、`camera.released`、`camera.not_ready`、`camera.imaging_applied` |
| `站点模式`（6 处） | 同名不同义 | `op.site.enter_offline`、`op.site.leave_offline`、`site.offline_auto_reset` 等 |
| `PLC {category}` | ev 动态拼接 | `plc.engine.warn` / `plc.engine.error`，`data.category` |
| `故障` | 示教、S7、追溯、布防都记成同一事件 | `fault.enter`，`source` 区分（5.4） |
| `布防阶段`（refused） | 整段 JSON 被序列化进 `msg` | `part.arm.refused`，trace 放 `data`，补 `faultCode` |
| `校验失败` | fault code 未记录 | 并入 `part.arm.refused` |

## 5. 补齐关键证据

### 5.1 示教握手（当前 `teaching.rs` 不写任何运行日志）

| code | 触发 | data |
|---|---|---|
| `teach.request` | `prepare_request` 分配新编号 | `teachId`、`captureRound`、`highWater` |
| `teach.pc_published` | 每次 `publish` 成功（含复位、取消、结果上报） | `pc: [版本, 状态, 编号, 原因, 保留]`、`pcState` 前后值 |
| `teach.plc_changed` | PLC 稳定快照（连续两次读数一致）与上一次采用的值不同 | `plc: [五值]`、`prev: [五值]`、`phase` 前后值 |
| `teach.accepted` / `ended` / `rejected` / `released` | `observe_reply` 对应分支 | `teachId`、拒绝时 `reason` |
| `teach.cancel` / `teach.finish` | 本地取消、结果上报 | `reason`、`success` |
| `teach.awaiting_echo` | PLC 已置 10 但回显为 0（#174 的容错） | 首次出现记一次，回显对上或超时时记 `waitedMs` |
| `teach.timeout.accept` / `teach.timeout.cancel_ack` / `teach.timeout.result` / `teach.timeout.reset` | 各类超时 | 当时的 `pc`、`plc` 五值，`waitedMs` |
| `teach.violation` | 寄存器不合规 | `field`、`plc` 五值（#174 的 `TeachingViolation`） |
| `teach.reset.begin` / `teach.reset.done` | 复位开始、`initialize_after_reset` 成功 | 复位前未结轮次、完成时 PLC 五值 |
| `teach.link_lost` / `teach.points_invalid` | 链路中断、点表校验失败 | 只在进入该状态时记一次 |

关联：`recipe_capture_start` 起示教轮次后，`applog` 维护“当前示教上下文”（与现有在途工件 `ACTIVE` 同法），后续行自动带 `teachId`、`captureRound`，轮次释放或复位时清除。

`teach.plc_changed` 只在稳定值变化时写，正常一轮约 5～8 行，不会每拍刷屏；这正是这次现场缺失的那份证据。

### 5.2 S7 握手与故障输出

| code | 触发 | data |
|---|---|---|
| `s7.fault_output` | `stop_outputs` 每次写出 `fault_plan`（视觉故障=1、就绪=0、布防=0） | `cause`（触发路径）、`reason`、`phase` 前后值 |
| `s7.reset.request` | 收到复位：操作员命令或 PLC `faultReset` 上升沿 | `source: operator / plc_edge`、当时 `phase` |
| `s7.reset.from_idle` | `plc_session.rs` 空闲时收到复位转入 ResetRequired | 同上；这是 10-24 18:18:22 “无记录故障”的最可能路径 |
| `s7.reset.done` | 现有“S7 复位完成” | 补原未结事务、`lastRequestSeq` |
| `s7.intake.paused` / `s7.intake.resumed` | 撤销 / 恢复视觉就绪（设备未就绪、审计未就绪等） | `why` |
| `s7.ack` | 收到结果确认 | `ackSeq`、`result` |
| `s7.reconnect.forced` | 故障确认失败后主动断开重连 | `error` |
| `s7.start.capture` | 示教采集事务的开始请求（当前在 `start_part` 提前返回、无日志） | `request` |

规则：`s7.fault_output` 是 PLC 可见故障的唯一出口事件，所有写 `fault_plan` 的路径都经过它，不依赖调用方是否另写 `fault.enter`。

### 5.3 操作员命令

所有改变设备或握手状态的 IPC 命令写 `op.*`，记参数摘要与结果（`ok` / `err` + 错误原文），命令入口处统一包装：

`op.reset`、`op.capture.start`、`op.capture.stop`、`op.camera.release`、`op.camera.reconnect`、`op.camera.save_config`、`op.camera.add`、`op.camera.remove`、`op.camera.set_imaging`、`op.camera.soft_trigger`、`op.plc.connect`、`op.plc.disconnect`、`op.plc.save_config`、`op.plc.write_point`、`op.site.enter_offline`、`op.site.leave_offline`、`op.offline.run` / `capture` / `stop`、`op.recipe.publish`、`op.recipe.delete`、`op.calib.apply`。

被拒绝的操作也要记（例如复位被拒、闸门超时），这类信息现场最难口述。

### 5.4 故障事件与来源

- `fault.enter`：节拍进入故障，或故障原因变化。`source` 取 `teach / s7 / audit / arm / device / storage / operator`，`data` 带 `reason`、`s7Phase`、`teachPhase`、在途 `cycleId`。
- `fault.repeat`：同一来源、同一原因重复上报时不再逐条写，按聚合规则（第 6 节）定期输出 `{count, firstTs, lastTs}`。
- `fault.cleared`：复位完成、故障解除，带持续时长。

`enter_fault` 改为按 `(source, reason)` 去重；同时修掉两个重复根源（实施时单独提交，附回归测试）：

1. `cycle/capture.rs` 的 `TeachEvent::Fault(reason) if was_busy` 分支无守卫：示教事务未结且 PLC 断线时，`teaching.poll` 每次都返回同一 `Fault`，每拍两次 `poll_teaching` 各写一行。
2. 示教原因经 `s7.fault()` 覆盖 S7 会话的 `message`，下一拍 S7 自己的错误与之不同又判为“变化”，两条原因交替写出。

### 5.5 启动与配置摘要

- `app.start`：`data` 带 `version`、`gitSha`、`buildTime`、`utcOffset`、`dataRoot`、`logSchema`、`lyflowSeries`。`gitSha` / `buildTime` 由 `build.rs` 输出 `cargo:rustc-env`，CI 打包（`windows-package.yml`）与本地构建都要能取到，取不到写 `unknown`。
- `app.config`：PlcHost、CycleHost 初始化之后单独写一条（`applog::init` 早于它们，lib.rs 初始化顺序不变）：PLC 协议 / 地址 / 机架 / 插槽，相机 `[id, serial, triggerSource, triggerActivation, frameRate]`，检测设置摘要。配置保存（`op.*.save_config`）后再写一次。
- 关于页（`commands.rs` 的 `app_info`）同步显示 `gitSha`，现场截图即可确认构建。

## 6. 降噪：聚合与节流

新增 `applog::Throttle`，按 `(code, key)` 聚合：

- 窗口内首次出现立即写原事件；
- 之后只计数，窗口结束（默认 10 s）或状态结束时写一条 `<code>.summary`：`{count, firstTs, lastTs, 样本范围}`，例如游离帧按相机给出帧计数区间；
- 计数在进程内存里，退出前（含 panic 钩子）尽力写出未刷新的摘要。

| 来源 | 位置 | key | 摘要内容 |
|---|---|---|---|
| 游离帧 | `cycle.rs` `on_frame` | 相机 | 帧数、帧计数最小 / 最大、当时节拍阶段 |
| 迟到帧、截止后帧、帧归属失败 | `cycle.rs` 同区段 | 相机 + 原因 | 帧数、帧计数范围 |
| 丢帧（通道满） | `cycle.rs` 丢帧累计 | — | 新增丢弃数 |
| 相机断线重连失败 | `camera.rs` 监督线程（约 2 s 一次） | 相机 | 失败次数、最后错误 |
| ly_plc 引擎告警 | `plc.rs` EventSink | category | 次数、最后一条 |
| 故障重复 | `enter_fault` | source + reason | 见 5.4 |
| 报警 | `cycle.rs` `alarm` | 报警文案 | 去重不再随每件开工清空 |

“游离帧 ≥ 3/分钟”报警保留，但改由聚合器的计数触发，不再与逐帧日志绑定。

逐帧的 debug 事件（帧到达、测量完成）保留：它们与工件一一对应、总量受件数约束，是复盘单件测量的依据。`data` 去掉与 `msg` 重复的内容。

P3 实施说明：聚合器为 `applog::throttle::Throttle`，时间由调用方传入，节拍每拍 `flush`。持续出现时每个窗口只出一条摘要，安静一个窗口后再出现才重新照写原事件；摘要的 `count` 不含照写的那条。与上表的差异：相机重连失败窗口 60 s（重连约 2 s 一次，10 s 窗口降噪有限），重连成功或相机移出相机组时立即出摘要；报警去重窗口 10 分钟，编码归新域 `alarm`；游离帧在下一件开工时立即出摘要；“游离帧 ≥ 3/分钟”报警仍由 1 分钟滑动计数触发，与日志写不写无关；帧归属失败按是否判本件 ERR 分为 `frame.unrouted` 与 `frame.route_fault`。退出前（含 panic 钩子）写出未刷新摘要尚未实现：进程退出时最多丢一个窗口的计数。

## 7. 写入方式

10-24 的现场样本中约 95% 是可聚合的重复行，降噪后写入量大幅下降，维持同步追加写，保留“崩溃不丢尾部”：

- panic 钩子写日志改用 `try_lock`，拿不到锁就写 stderr，避免 panic 发生在写日志途中时自死锁；
- 写失败（磁盘满、文件打不开）计数，恢复后补写 `app.log_write_failed {count, firstTs, lastTs}`；
- 若以后实测同步写阻塞节拍，再改后台线程，队列满时必须记 `app.log_dropped`，不得静默丢弃。

## 8. 诊断包

现状：已包含运行日志（按时间精确筛选）、`logs/plc.jsonl`（ts 倒序、无本地时间）、`config/`、`state/`（含示教握手状态）、握手审计、检测记录与原图。

调整：

1. `manifest.json`：增加 `build {version, gitSha, buildTime}`、`utcOffset`、`logSchema`、`configSummary`（同 `app.config`）、`files[{path, bytes, sha256}]`。
2. `logs/plc.jsonl` 改为时间正序，并为每行补 `t`，与运行日志同口径。
3. 包根目录写 `README.md`（固定模板）：包结构、时间口径、编码表版本与链接、推荐分析顺序（先看 `fault.enter` 与 `s7.fault_output`，再按 `teachId` / `cycleId` 串事件）。
4. 握手审计当前文件按时间段筛选后写出，不再整份拷贝。
5. 诊断导出对话框的说明文案同步更新。

## 9. 实施分期

| 阶段 | 内容 | 依赖 |
|---|---|---|
| P1 基础 | `Record` 加 `t` / `code`、`codes.rs` 与 `log_event`、编码表文档与一致性测试、`app.start` / `app.config`、`build.rs` 构建信息、panic 钩子 `try_lock`、写失败计数 | 合入 #174 |
| P2 关键证据 | 示教握手事件与上下文（5.1）、`s7.fault_output` 与复位来源（5.2）、操作员命令（5.3）、`fault.enter` 去重与来源及两个重复根源修复（5.4） | P1 |
| P3 降噪 | `Throttle` 与第 6 节各来源 | P1 |
| P4 诊断包 | 第 8 节 | P1（`t`、编码表） |

P2、P3、P4 改动的文件基本不重叠（`teaching.rs` / `plc_session.rs` / IPC 命令，`cycle.rs` 帧处理 / `camera.rs` / `plc.rs`，`diagnostics.rs`），P1 合入后可并行。P2 与 P3 都会碰 `cycle.rs`，分区段修改并先后合入。

## 10. 测试计划

遵守项目单元测试准入规则，只写能说清所防回归的测试：

| 测试 | 防止的回归 |
|---|---|
| 编码表一致性：`codes.rs` 的每个 code 唯一，且与 `agent-logging-codes.md` 表格一一对应 | code 是对 Agent 的契约，改名或漏登记会让分析脚本悄悄失效 |
| 旧格式行（无 `t` / `code`）可被查询、时间线和诊断包读出 | 新字段非 Option 时旧行被静默丢弃 |
| 聚合：窗口内 N 次游离帧 → 1 条原事件 + 1 条摘要，`count = N - 1`、帧计数范围正确 | 降噪丢计数，或退回逐帧刷屏 |
| 同一 `(source, reason)` 的故障连续上报 → 只有 1 条 `fault.enter`，摘要计数正确；原因变化时再记一条 | 复现 10-24 的每拍重复 |
| 示教：PLC 稳定快照变化时恰好写一条 `teach.plc_changed`，`data.plc` 为该快照；不变不写 | 关键证据缺失或刷屏 |
| S7：空闲时收到复位转入 ResetRequired，写出 `s7.fault_output` 且 `cause` 指明复位来源 | PLC 可见故障无记录（10-24 18:18:22） |

为便于断言，`applog` 提供测试用的内存 sink（只在 `cfg(test)` 下替换文件 sink），测试断言 `code` 与 `data`，不断言 `msg` 文案。

验证除上述测试外，跑一次 `pnpm test:ui:production`，检查生成的日志里有 `app.start`、`app.config`、完整的 `part.*` 链，且无逐帧 warn 刷屏；离线模式注入一次故障与复位，检查 `fault.enter` / `s7.fault_output` / `s7.reset.*` 顺序。

## 11. 风险与待定

- **日志体积**：`teach.pc_published`、`teach.plc_changed` 每轮几行，`app.config` 每次保存一行，增量可忽略；`t` 每行约 30 字节，降噪后总量仍远低于现状。
- **编码表维护成本**：新增事件必须先登记再使用，由一致性测试约束。
- **与 PR #174 的关系**：#174 改了 `teaching.rs`（违规诊断、等待回显）与 `cycle.rs`（闸门拆分），P1 开工前先同步。
- **待定**：聚合窗口默认 10 s 是否合适；`op.plc.write_point` 是否记录写入值（可能含现场敏感点位，倾向只记点位与结果）。

## 附录 A：现状盘点摘要

- 写入入口：`applog::write / emit`；`cycle::log / log_with / trace`（trace 记 debug）；`audit.rs` 经 `app_log / app_record` 转写；`plc.rs` 转写 ly_plc 事件。`teaching.rs`、`recipe_capture*`、`plc_session.rs` 不写运行日志。
- 逐帧无限流：游离帧（`cycle.rs` `on_frame`）、迟到帧、截止后帧、帧归属失败；相机断线每约 2 s 一行；丢帧每拍一行。
- 关键数据只在 `msg`：游离帧的相机与帧计数、布防拒绝的整段 trace、校验失败的 fault code、`audit.rs` 中的 k / view / camera / seq、启动事件的版本。
- PLC 可见故障无记录：`plc_session.rs` 空闲收到复位转 ResetRequired 后立即写 `fault_plan`，无事件、不改 `message`。
- 示教 DB12 快照读取用 `read_now`，不进 `plc.db` 的值变化；DB6 写入会进 `plc.db`。
- 诊断包：`logs/plc.jsonl` 为 ts 倒序、无本地时间；清单缺构建信息、时区与文件清单；`state/` 整份拷贝。
- 读取方约束：`applog::head()` 依赖 `{"ts":…,"level":"…` 开头；`CycleScan` 依赖紧凑序列化子串；`Record` 全量解析失败的行被静默跳过。
