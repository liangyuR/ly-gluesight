# 运行日志事件编码表

运行日志每行的 `code` 是给 Agent 的稳定契约，规则见 [日志改造方案](agent-logging.md) 第 3、4 节。本表与 `src-tauri/src/applog/codes.rs` 逐条一致（编码、事件名、级别），由单元测试核对；新增事件先在这里登记。`data` 的键只增不改，改含义时换新编码。

没有 `code` 的行来自未改造的调用点（不做全量迁移），按 `ev` 与 `msg` 理解。

| code | ev | 级别 | 触发时机 | data |
|---|---|---|---|---|
| `app.start` | 启动 | info | 进程启动、日志初始化完成 | `version`、`gitSha`（`-dirty` 表示构建时有未提交改动，取不到为 `unknown`）、`buildTime`、`utcOffset`、`dataRoot`、`logSchema`、`lyflowSeries` |
| `app.config` | 配置摘要 | info | PLC 与相机组初始化完成后；保存 PLC 配置、相机配置、检测设置后 | `offline`、`plc`（连接配置：协议、地址、端口、机架、插槽等）、`cameras`（相机组配置）、`settings`（检测设置） |
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
