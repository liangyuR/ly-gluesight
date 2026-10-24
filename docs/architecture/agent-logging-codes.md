# 运行日志事件编码表

运行日志每行的 `code` 是给 Agent 的稳定契约，规则见 [日志改造方案](agent-logging.md) 第 3、4 节。本表与 `src-tauri/src/applog/codes.rs` 逐条一致（编码、事件名、级别），由单元测试核对；新增事件先在这里登记。`data` 的键只增不改，改含义时换新编码。

`.summary` 结尾的是聚合摘要（方案第 6 节，`applog::throttle`）：同一编码同一 key（相机、原因等）在窗口内只照写首次，其余计数；窗口到期或状态结束时写一条摘要，`count` 为首条之后又出现的次数（不含照写的那条），`firstTs` / `lastTs` 为计入的首末时间，`…Min` / `…Max` 为计入样本的范围，其余键取最近一次的值。持续出现时每个窗口只出一条摘要；安静一个窗口后再出现重新照写。

没有 `code` 的行来自未改造的调用点（不做全量迁移），按 `ev` 与 `msg` 理解。

| code | ev | 级别 | 触发时机 | data |
|---|---|---|---|---|
| `app.start` | 启动 | info | 进程启动、日志初始化完成 | `version`、`gitSha`（`-dirty` 表示构建时有未提交改动，取不到为 `unknown`）、`buildTime`、`utcOffset`、`dataRoot`、`logSchema`、`lyflowSeries` |
| `app.config` | 配置摘要 | info | PLC 与相机组初始化完成后；保存 PLC 配置、相机配置、检测设置后 | `project`（当前项目 `id`、`name`）、`offline`、`plc`（连接配置：协议、地址、端口、机架、插槽等）、`cameras`（相机组配置）、`settings`（检测设置） |
| `app.panic` | 程序异常 | error | 任一线程 panic | 无；线程名与 panic 信息在 `msg` |
| `app.log_write_failed` | 日志写入失败 | warn | 日志写不进去后首次恢复写入 | `count`（未写入的行数）、`firstTs`、`lastTs` |
| `teach.request` | 示教请求 | info | 开始整圈采集时分配新的握手编号 | `teachId`、`captureRound`、`highWater` |
| `teach.pc_published` | 示教 PC 发布 | info | PC 成功写出 DB6 五个寄存器（请求、结果、取消、复位） | `pc`：[版本, 状态, 编号, 原因, 保留] |
| `teach.plc_changed` | 示教 PLC 变化 | info | PLC 稳定快照（连续两次读数一致）与上次记录不同 | `plc`、`prev`：DB12 [版本, 状态, 编号, 原因, 保留]，`prev` 首次为 null |
| `teach.accepted` | 示教已接受 | info | PLC 接受本轮（状态 10 且回显本轮编号） | 无 |
| `teach.ended` | 示教采集结束 | info | PLC 报采集结束（状态 20） | 无 |
| `teach.rejected` | 示教被拒绝 | warn | PLC 拒绝本轮（状态 40） | `reason`：PLC 原因码 |
| `teach.released` | 示教已释放 | info | 双方回空闲，本轮结束 | 无 |
| `teach.cancel` | 示教取消 | warn | PC 取消本轮（DB6 状态 40） | `reason`：PC 原因码（1 操作员取消，2 等待接受超时） |
| `teach.finish` | 示教结果上报 | info | PC 上报本轮完成或失败（DB6 状态 20 / 30） | `success`、`reason`：PC 原因码 |
| `teach.awaiting_echo` | 等待握手编号回显 | warn | 等待接受期间首次读到 PLC 已置 10 但回显为 0 | `plc`：DB12 五值 |
| `teach.timeout` | 示教超时 | warn | 等待接受、采集结束、结果确认或释放、复位空闲超时 | `stage`：accept / result / confirm / reset，`waitedMs`，`pc`、`plc` 五值 |
| `teach.violation` | 示教寄存器不合规 | error | PLC 稳定快照不合协议 | `field`：违规字段，`plc`：DB12 五值 |
| `teach.reset.begin` | 示教复位开始 | info | 收到复位，PC 写空闲并等待 PLC 回空闲 | `pending`：复位前未结轮次编号，无则 null |
| `teach.reset.done` | 示教复位完成 | info | PLC 合规空闲，示教接口就绪 | `plc`：DB12 五值，`highWater` |
| `teach.fault` | 示教故障 | error | 示教握手进入故障或故障原因变化（同一原因不重复记） | `reason`，`pc`、`plc` 五值（可能为 null） |
| `s7.fault_output` | S7 故障输出 | warn | PC 写出故障输出（视觉故障=1、就绪=0、布防=0），或因无法确认而断开通讯 | `cause`：new_connection / reset_operator / reset_plc_edge / fault / observation_failed，`reason`，`phase`，`ok`，`error` |
| `s7.reset.request` | S7 复位请求 | info | 收到复位：操作员命令或 PLC faultReset 上升沿 | `source`：operator / plc_edge，`phase`：收到时的会话阶段 |
| `s7.reset.done` | S7 复位完成 | info | 复位完成、视觉就绪 | 无 |
| `s7.ack` | S7 结果确认 | info | PLC 确认本件结果 | `requestSeq`、`resultCode`、`faultCode` |
| `s7.reconnect.forced` | S7 主动断开 | error | 无法确认撤销布防或置故障，PC 断开通讯以停止心跳 | `error` |
| `fault.enter` | 故障 | error | 节拍进入故障或故障原因变化 | `reason`、`s7Phase`、`teachPhase`；来源在 `source` |
| `fault.repeat` | 故障重复 | warn | 同一来源同一原因的故障被再次上报，在原因变化或解除时汇总 | `count`、`firstTs`、`lastTs`、`reason` |
| `fault.cleared` | 故障解除 | info | S7 复位完成，节拍离开故障 | `reason`：解除前的原因，`durationMs` |
| `op.reset` | 操作：复位 | info | 操作员点击复位 | `ok`、`error`、`phase` |
| `op.capture.start` | 操作：开始整圈采集 | info | 整圈采集开始命令 | `recipeId`、`cameraIds`、`ok`、`error` |
| `op.capture.stop` | 操作：取消整圈采集 | info | 整圈采集取消命令 | `ok`、`error` |
| `op.camera` | 操作：相机 | info | 相机释放、重连、保存配置、增删、调试曝光、软触发 | `action`：release / reconnect / save_config / add / remove / set_imaging / soft_trigger，`cam`、`ok`、`error` |
| `op.plc` | 操作：PLC | info | PLC 连接、断开、保存配置、手动写点 | `action`：connect / disconnect / save_config / write_point，`point`（写点时，只记点位不记值），`ok`、`error` |
| `op.site` | 操作：站点模式 | info | 进入或退出离线 | `action`：enter_offline / leave_offline，`ok`、`error` |
| `op.recipe.delete` | 操作：删除配方 | info | 删除生产配方或不可用配方 | `id`、`ok`、`error` |
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
