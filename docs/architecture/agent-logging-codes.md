# 运行日志事件编码表

运行日志每行的 `code` 是给 Agent 的稳定契约，规则见 [日志改造方案](agent-logging.md) 第 3、4 节。本表与 `src-tauri/src/applog/codes.rs` 逐条一致（编码、事件名、级别），由单元测试核对；新增事件先在这里登记。`data` 的键只增不改，改含义时换新编码。

没有 `code` 的行是尚未迁移的旧调用点（方案 P5 逐步迁完），按 `ev` 与 `msg` 理解。

| code | ev | 级别 | 触发时机 | data |
|---|---|---|---|---|
| `app.start` | 启动 | info | 进程启动、日志初始化完成 | `version`、`gitSha`（`-dirty` 表示构建时有未提交改动，取不到为 `unknown`）、`buildTime`、`utcOffset`、`dataRoot`、`logSchema`、`lyflowSeries` |
| `app.config` | 配置摘要 | info | PLC 与相机组初始化完成后；保存 PLC 配置、相机配置、检测设置后 | `offline`、`plc`（连接配置：协议、地址、端口、机架、插槽等）、`cameras`（相机组配置）、`settings`（检测设置） |
| `app.panic` | 程序异常 | error | 任一线程 panic | 无；线程名与 panic 信息在 `msg` |
| `app.log_write_failed` | 日志写入失败 | warn | 日志写不进去后首次恢复写入 | `count`（未写入的行数）、`firstTs`、`lastTs` |
