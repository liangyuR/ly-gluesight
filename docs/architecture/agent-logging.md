# 面向 Agent 分析的运行日志

运行日志按本文实现，事件编码以 [编码表](agent-logging-codes.md) 为准。

日志写入 `~/gs_logs/runtime/<环境>/<资料实例>/`，与项目资料分开；PLC 日志库位于同一目录。绑定、迁移及隔离规则见 [运行日志与视觉证据存储](log-storage-research.md)。

## 1. 目标与依据

运行日志的首要读者是排查现场问题的 Agent：拿到一个诊断包，不再向现场追问截图，就能还原“发生了什么、按什么顺序、双方寄存器是什么值、谁触发的”。

2026-10-24 京航联调的日志是这套设计的依据。一份 1.8 万行的日志里：

- 13380 行是逐帧的“游离帧”，4857 行是每拍重复的同一条“故障”，有效事件不到 30 条；
- PLC 寄存器值完全不在日志里，PLC 停在哪个状态只能靠故障文案差异和游离帧时间倒推，最后仍要现场截 TIA；
- 一次 PLC 侧可见的“视觉故障=1”没有任何原因记录；
- 复位等操作员动作只能从 PLC 写入痕迹倒推；
- 运行日志用毫秒时间戳、PLC 日志 CSV 用本地时间字符串，对照前要换算；看不出现场运行的是哪个构建。

## 2. 设计原则

1. **证据优先于结论**：状态迁移记下迁移前后的值和双方寄存器，而不仅是最终的故障文字。
2. **一件事一行**：持续状态只在出现、变化、结束时记录；高频重复聚合成计数摘要，计数不能丢。
3. **机器读 `code` 与 `data`，人读 `ev` 与 `msg`**：文案可以改，`code` 和 `data` 的键是对 Agent 的契约，改动要走兼容规则。
4. **凡是 PLC 看得见的，日志里都有原因**：每次写出故障输出、撤销视觉就绪，都对应一条带来源的事件。
5. **不静默丢日志**：写失败、聚合、节流都要留下计数；宁可同步写慢一点，也不在分析时制造“这段时间什么都没发生”的假象。

## 3. 行格式（日志 schema 2）

在现有 JSONL 上增量扩展，旧行照常可读。

```json
{"ts":1792837102059,"level":"warn","t":"2026-10-24T18:18:22.059+08:00","code":"s7.fault_output","ev":"S7 故障输出","msg":"写出故障输出（reset_operator）","data":{"cause":"reset_operator","reason":null,"phase":"AwaitAck","ok":true,"error":null}}
```

| 字段 | 说明 |
|---|---|
| `ts`、`level` | 不变，且必须仍是前两个键：`applog::head()` 靠 `{"ts":…,"level":"…` 开头做快速筛选与分段裁剪 |
| `t` | 新增。带 UTC 偏移的本地时间（RFC 3339，毫秒），与 PLC 日志、现场叙述直接对照 |
| `code` | 新增。稳定事件编码，见第 4 节；旧行没有 |
| `ev`、`msg` | 不变。中文事件名与说明，面向人，可随文案调整 |
| `sessionId`、`projectId` | 可选关联字段。运行日志初始化后带启动会话标识，打开项目后带项目编号；旧行缺少时照常读取 |
| 关联字段 | 现有 `cycleId / sn / requestSeq / recipeId / shotId / k / camera / view / seq / stage`，新增 `captureRound`（采集轮次）、`source`（触发来源，见 5.3）；旧日志里的 `teachId`（已停用的示教握手编号）照常读取 |
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
| `part` | 工件生命周期：开始、布防、结束、结论 |
| `frame` | 帧到达与归属：游离、迟到、截止后、配组、丢帧 |
| `measure` | 测量与判定 |
| `camera` | 相机连接、配置下发、取图 |
| `fault` | 节拍进入 / 维持 / 解除故障 |
| `recipe`、`capture`、`calib` | 配方发布与迁移、采集轮次、标定 |
| `storage`、`audit` | 录制、清理、持久追溯 |

### 4.2 代码组织

- 新增 `src-tauri/src/applog/codes.rs`：每个事件一条常量 `Event { code, ev, level }`，调用方写 `log_event(app, &codes::S7_FAULT_OUTPUT, msg, fields)`，不再手写 `ev` 字符串和级别。
- `cycle::log / log_with / trace` 继续保留，写出的行 `code` 为空。不做全量迁移：只有编码表里的事件带 `code`，没有 `code` 的行按 `ev` 与 `msg` 理解。
- `docs/architecture/agent-logging-codes.md` 为编码表（code、ev、级别、触发时机、`data` 键）。由测试保证与 `codes.rs` 一致（见第 9 节）。

### 4.3 拆分的同名事件

只拆分了带编码的事件：`故障`（S7、追溯、布防都记成同一事件）改为 `fault.enter`，用 `source` 区分（5.3）；`PLC {category}`（ev 动态拼接）改为 `plc.engine.warn` / `plc.engine.error`，`data.category`；相机断线重试改为 `camera.reconnect_failed`。其余同名事件（`相机`、`站点模式`、`布防阶段`、`校验失败` 等）没有 `code`，按 `ev` 与 `msg` 理解。

## 5. 补齐关键证据

### 5.1 S7 握手与故障输出

| code | 触发 | data |
|---|---|---|
| `s7.fault_output` | PC 撤下输出（故障时置 pcState=90、faultCode=101，待复位时置 pcState=0），或因无法确认而断开通讯 | `cause`（触发路径，含 `reset_operator`、`reset_timeout`）、`reason`、`phase` |
| `s7.reset.request` | 收到复位：操作员软件复位，或 PLC 置复位请求（plcState=99） | `source: operator / plc`、收到时的 `phase` |
| `s7.reset.done` | 复位完成、视觉就绪 | 无 |
| `s7.ack` | 收到结果确认 | `requestSeq`、`resultCode`、`faultCode` |
| `s7.reconnect.forced` | 无法确认撤下 pcState，主动断开重连 | `error` |

规则：`s7.fault_output` 是 PLC 可见故障的唯一出口事件，所有撤下输出的路径都经过它，不依赖调用方是否另写 `fault.enter`。

### 5.2 操作员命令

改变设备或握手状态的 IPC 命令写 `op.*`，记参数摘要与结果（`ok` / `err` + 错误原文），命令入口处统一包装：`op.reset`、`op.capture.import`、`op.camera`、`op.plc`、`op.site`（按对象合并，用 `data.action` 区分动作）、`op.maintenance`、`op.recipe.delete`。手动写点只记点位与结果，不记写入值。

被拒绝的操作也要记（例如复位被拒、闸门超时），这类信息现场最难口述。

### 5.3 故障事件与来源

- `fault.enter`：节拍进入故障，或故障原因变化。`source` 取 `s7 / audit / arm / device / storage / operator`，`data` 带 `reason`、`s7Phase`、在途 `cycleId`。
- `fault.repeat`：同一来源、同一原因重复上报时不再逐条写，按聚合规则（第 6 节）定期输出 `{count, firstTs, lastTs}`。
- `fault.cleared`：复位完成、故障解除，带持续时长。

`enter_fault` 按 `(source, reason)` 去重；不同来源各自保留原因，互不覆盖。

### 5.4 启动与配置摘要

- `app.start`：`data` 带 `version`、`gitSha`、`buildTime`、`utcOffset`、`dataRoot`、`logSchema`、`lyflowSeries`。`gitSha` / `buildTime` 由 `build.rs` 输出 `cargo:rustc-env`，CI 打包（`windows-package.yml`）与本地构建都要能取到，取不到写 `unknown`。
- `app.config`：PlcHost、CycleHost 初始化之后单独写一条（`applog::init` 早于它们，lib.rs 初始化顺序不变）：PLC 协议 / 地址 / 机架 / 插槽，相机 `[id, serial, triggerSource, triggerActivation, frameRate]`，检测设置摘要。配置保存（`op.*.save_config`）后再写一次。
- 关于页（`commands.rs` 的 `app_info`）同步显示 `gitSha`，现场截图即可确认构建。

## 6. 降噪：聚合与节流

新增 `applog::Throttle`，按 `(code, key)` 聚合：

- 窗口内首次出现立即写原事件；
- 之后只计数，窗口结束（默认 10 s）或状态结束时写一条 `<code>.summary`：`{count, firstTs, lastTs, 样本范围}`，例如游离帧按相机给出帧计数区间；
- 计数在进程内存里；退出前不补写未刷新的摘要，进程退出时最多丢一个窗口的计数。

| 来源 | 位置 | key | 摘要内容 |
|---|---|---|---|
| 游离帧 | `cycle.rs` `on_frame` | 相机 | 帧数、帧计数最小 / 最大、当时节拍阶段 |
| 迟到帧、截止后帧、帧归属失败 | `cycle.rs` 同区段 | 相机 + 原因 | 帧数、帧计数范围 |
| 丢帧（通道满） | `cycle.rs` 丢帧累计 | — | 新增丢弃数 |
| 相机断线重连失败 | `camera.rs` 监督线程（约 2 s 一次） | 相机 | 失败次数、最后错误 |
| ly_plc 引擎告警 | `plc.rs` EventSink | category | 次数、最后一条 |
| 故障重复 | `enter_fault` | source + reason | 见 5.3 |
| 报警 | `cycle.rs` `alarm` | 报警文案 | 去重不再随每件开工清空 |

“游离帧 ≥ 3/分钟”报警保留，但改由聚合器的计数触发，不再与逐帧日志绑定。

逐帧的 debug 事件（帧到达、测量完成）保留：它们与工件一一对应、总量受件数约束，是复盘单件测量的依据。`data` 去掉与 `msg` 重复的内容。

实现：聚合器为 `applog::throttle::Throttle`，时间由调用方传入，节拍每拍 `flush`。持续出现时每个窗口只出一条摘要，安静一个窗口后再出现才重新照写原事件；摘要的 `count` 不含照写的那条。与上表的差异：相机重连失败窗口 60 s（重连约 2 s 一次，10 s 窗口降噪有限），重连成功或相机移出相机组时立即出摘要；报警去重窗口 10 分钟，编码归新域 `alarm`；游离帧在下一件开工时立即出摘要；“游离帧 ≥ 3/分钟”报警仍由 1 分钟滑动计数触发，与日志写不写无关；帧归属失败按是否判本件 ERR 分为 `frame.unrouted` 与 `frame.route_fault`。

## 7. 写入方式

10-24 的现场样本中约 95% 是可聚合的重复行，降噪后写入量大幅下降，维持同步追加写，保留“崩溃不丢尾部”：

- panic 钩子写日志改用 `try_lock`，拿不到锁就写 stderr，避免 panic 发生在写日志途中时自死锁；
- 写失败（磁盘满、文件打不开）计数，恢复后补写 `app.log_write_failed {count, firstTs, lastTs}`；
- 若以后实测同步写阻塞节拍，再改后台线程，队列满时必须记 `app.log_dropped`，不得静默丢弃。

## 8. 诊断包

诊断包包含运行日志（按时间精确筛选）、`logs/plc.jsonl`、`config/`、`state/`、握手审计、检测记录与原图。面向 Agent 的约定：

1. `manifest.json` 带 `build {version, gitSha, buildTime}`、`utcOffset`、`logSchema`、`configSummary`（同 `app.config`）、`files[{path, bytes, sha256}]`。
2. `logs/plc.jsonl` 按时间正序，每行带 `t`，与运行日志同口径。
3. 包根目录写 `README.md`（固定模板）：包结构、时间口径、编码表版本与链接、推荐分析顺序（先看 `fault.enter` 与 `s7.fault_output`，再按 `cycleId` 串事件）。
4. 握手审计当前文件按时间段筛选后写出，不整份拷贝。

实现细节：

- `manifest.json` 的 `schema` 升为 2；`configSummary` 与 `app.config` 共用 `commands::config_summary`；`files` 覆盖除 `manifest.json` 本身外的全部条目（含 `README.md`），按写入顺序排列。
- `logs/plc.jsonl` 按 `(ts, id)` 升序；超过 20 万条时仍保留最新的部分。`t` 位于每行首个字段，其余字段与 PLC 日志库一致。
- 握手审计当前文件按每行 `ts` 筛选 `[from, to]`；无法解析 `ts` 的行不打包并记入 `notes`；时间段内无行时不写该文件。已滚动分段仍按滚动时刻整段选取。
- README 模板在 `diagnostics.rs` 的 `README` 常量中，schema 版本取自 `applog::SCHEMA`。

## 9. 测试

遵守项目单元测试准入规则，只写能说清所防回归的测试：

| 测试 | 防止的回归 |
|---|---|
| 编码表一致性：`codes.rs` 的每个 code 唯一，且与 `agent-logging-codes.md` 表格一一对应 | code 是对 Agent 的契约，改名或漏登记会让分析脚本悄悄失效 |
| 旧格式行（无 `t` / `code`）可被查询、时间线和诊断包读出 | 新字段非 Option 时旧行被静默丢弃 |
| 聚合：窗口内 N 次游离帧 → 1 条原事件 + 1 条摘要，`count = N - 1`、帧计数范围正确 | 降噪丢计数，或退回逐帧刷屏 |
| 同一 `(source, reason)` 的故障连续上报 → 只有 1 条 `fault.enter`，摘要计数正确；原因变化时再记一条 | 复现 10-24 的每拍重复 |
| S7：收到复位撤下输出时写出 `s7.fault_output` 且 `cause` 指明复位来源 | PLC 可见故障无记录（10-24 18:18:22） |

为便于断言，`applog` 提供测试用的内存 sink（只在 `cfg(test)` 下替换文件 sink），测试断言 `code` 与 `data`，不断言 `msg` 文案。

端到端核对：跑一次 `pnpm test:ui:production`，检查生成的日志里有 `app.start`、`app.config`、完整的 `part.*` 链，且无逐帧 warn 刷屏；离线模式注入一次故障与复位，检查 `fault.enter` / `s7.fault_output` / `s7.reset.*` 顺序。

## 10. 风险与待定

- **日志体积**：`app.config` 每次保存一行，增量可忽略；`t` 每行约 30 字节，降噪后总量仍远低于改造前。
- **编码表维护成本**：新增事件必须先登记再使用，由一致性测试约束。
- **待定**：聚合窗口默认 10 s 是否合适，等现场数据。
