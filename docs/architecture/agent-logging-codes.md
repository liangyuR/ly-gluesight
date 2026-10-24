# 运行日志事件编码表

运行日志每行的 `code` 是给 Agent 的稳定契约，规则见 [日志改造方案](agent-logging.md) 第 3、4 节。本表与 `src-tauri/src/applog/codes.rs` 逐条一致（编码、事件名、级别），由单元测试核对；新增事件先在这里登记。`data` 的键只增不改，改含义时换新编码。

`.summary` 结尾的是聚合摘要（方案第 6 节，`applog::throttle`）：同一编码同一 key（相机、原因等）在窗口内只照写首次，其余计数；窗口到期或状态结束时写一条摘要，`count` 为首条之后又出现的次数（不含照写的那条），`firstTs` / `lastTs` 为计入的首末时间，`…Min` / `…Max` 为计入样本的范围，其余键取最近一次的值。持续出现时每个窗口只出一条摘要；安静一个窗口后再出现重新照写。

没有 `code` 的行是尚未迁移的旧调用点（方案 P5 逐步迁完），按 `ev` 与 `msg` 理解。

| code | ev | 级别 | 触发时机 | data |
|---|---|---|---|---|
| `app.start` | 启动 | info | 进程启动、日志初始化完成 | `version`、`gitSha`（`-dirty` 表示构建时有未提交改动，取不到为 `unknown`）、`buildTime`、`utcOffset`、`dataRoot`、`logSchema`、`lyflowSeries` |
| `app.config` | 配置摘要 | info | PLC 与相机组初始化完成后；保存 PLC 配置、相机配置、检测设置后 | `offline`、`plc`（连接配置：协议、地址、端口、机架、插槽等）、`cameras`（相机组配置）、`settings`（检测设置） |
| `app.panic` | 程序异常 | error | 任一线程 panic | 无；线程名与 panic 信息在 `msg` |
| `app.log_write_failed` | 日志写入失败 | warn | 日志写不进去后首次恢复写入 | `count`（未写入的行数）、`firstTs`、`lastTs` |
| `alarm.raised` | 报警 | error | 节拍界面新增一条报警；同一报警文案 10 分钟内只照写首次，之后（每件开工、复位后重新出现）计入摘要 | `alarm`（报警文案） |
| `alarm.raised.summary` | 报警汇总 | error | `alarm.raised` 的聚合摘要：10 分钟窗口到期且窗口内有计数 | `alarm`、`count`、`firstTs`、`lastTs` |
| `frame.stray` | 游离帧 | warn | 节拍不在采集 / 收尾阶段时收到触发帧；按相机聚合，窗口内首次照写 | `frameCounter`、`phase`（当时节拍阶段）；相机在顶层 `camera` |
| `frame.stray.summary` | 游离帧汇总 | warn | `frame.stray` 的聚合摘要：10 s 窗口到期，或下一件开工 | `count`、`firstTs`、`lastTs`、`frameCounterMin`、`frameCounterMax`、`phase`（最近一帧时） |
| `frame.late` | 迟到帧 | warn | 段配方收到不属于本件的帧（上一件迟到或布防前那次打开）；按相机聚合 | `frameCounter` |
| `frame.late.summary` | 迟到帧汇总 | warn | `frame.late` 的聚合摘要：10 s 窗口到期 | `count`、`firstTs`、`lastTs`、`frameCounterMin`、`frameCounterMax` |
| `frame.after_deadline` | 截止后帧 | warn | 本件已到收尾截止后仍收到帧，丢弃；按相机聚合 | `frameCounter` |
| `frame.after_deadline.summary` | 截止后帧汇总 | warn | `frame.after_deadline` 的聚合摘要：10 s 窗口到期 | `count`、`firstTs`、`lastTs`、`frameCounterMin`、`frameCounterMax` |
| `frame.unrouted` | 帧归属 | warn | 帧归不到本件（旧配方的布防前帧、不在本件计划里的相机），不判本件故障；按相机 + 原因聚合 | `reason`（`stale` / `unknown_camera`）、`frameCounter` |
| `frame.unrouted.summary` | 帧归属汇总 | warn | `frame.unrouted` 的聚合摘要：10 s 窗口到期 | `reason`、`count`、`firstTs`、`lastTs`、`frameCounterMin`、`frameCounterMax` |
| `frame.route_fault` | 帧归属故障 | error | 帧归属让本件判 ERR（重复、超计划、相机重开、计数来源不可信）；按相机 + 原因聚合 | `reason`（`duplicate` / `extra` / `session_changed` / `untrusted_counter` / `fault`）、`frameCounter`、`faultCode`（旧配方） |
| `frame.route_fault.summary` | 帧归属故障汇总 | error | `frame.route_fault` 的聚合摘要：10 s 窗口到期 | `reason`、`count`、`firstTs`、`lastTs`、`frameCounterMin`、`frameCounterMax` |
| `frame.dropped` | 丢帧 | warn | 帧通道满、相机回调丢弃了帧（节拍发现累计丢弃数增加）；窗口内首次照写 | `dropped`（这次新增的丢弃数）、`total`（进程内累计） |
| `frame.dropped.summary` | 丢帧汇总 | warn | `frame.dropped` 的聚合摘要：10 s 窗口到期 | `count`（发现增加的次数）、`firstTs`、`lastTs`、`dropped`（计入的新增丢弃合计）、`total` |
| `camera.reconnect_failed` | 相机重连失败 | warn | 监督线程重开海康相机失败（约每 2 s 一次，含掉线后的重连）；按相机聚合，窗口 60 s | `error`、`lost`（本次是否因掉线而重连）；相机在顶层 `camera` |
| `camera.reconnect_failed.summary` | 相机重连失败汇总 | warn | `camera.reconnect_failed` 的聚合摘要：60 s 窗口到期，或重连成功、相机移出相机组 | `count`、`firstTs`、`lastTs`、`error`（最后一次）、`lost`（最后一次） |
| `plc.engine.warn` | PLC 引擎告警 | warn | ly_plc 引擎写出 warn 级日志；按 category 聚合 | `category`、`message` |
| `plc.engine.warn.summary` | PLC 引擎告警汇总 | warn | `plc.engine.warn` 的聚合摘要：10 s 窗口到期 | `category`、`message`（最后一条）、`count`、`firstTs`、`lastTs` |
| `plc.engine.error` | PLC 引擎错误 | error | ly_plc 引擎写出 error 级日志；按 category 聚合 | `category`、`message` |
| `plc.engine.error.summary` | PLC 引擎错误汇总 | error | `plc.engine.error` 的聚合摘要：10 s 窗口到期 | `category`、`message`（最后一条）、`count`、`firstTs`、`lastTs` |
