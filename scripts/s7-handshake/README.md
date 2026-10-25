# S7 握手测试 PLC

这个独立测试服务通过真实 TCP 套接字提供 TPKT / COTP / S7comm 的连接协商、ReadVar 和 WriteVar。它用于测试现有 `ly-plc` S7 客户端和生产通讯协议 7 的握手，不启动桌面、不访问实际 PLC。

仅依赖 Python 3.11+ 标准库。服务固定绑定 `127.0.0.1`，由系统分配随机端口，不接受固定端口或外部监听地址。它不是 Siemens CPU 模拟器，不模拟 PLC 扫描周期、双快照、超时、实际 PUT/GET 权限、优化 DB 或硬件时序。

```powershell
python -m unittest discover -s scripts/s7-handshake/tests -v
python -u scripts/s7-handshake/s7_test_plc.py --port 0
```

启动后，stdout 第一行是 JSON：

```json
{"event":"ready","protocol":"s7","host":"127.0.0.1","port":49152,"pid":1234,"control":"stdio-json","plc_db":4,"pc_db":2,"db_size":1024}
```

`49152` 是示例；客户端必须使用实际返回端口。Rust 集成测试可用 `Command` 启动该文件，将 stdin/stdout 设为管道，读取 ready 后配置 `ly-plc` 的 S7 host/port。控制命令每行一个 JSON，响应为 `{"id":原id,"ok":true,"result":...}` 或 `{"id":原id,"ok":false,"error":"..."}`。stdout 不包含日志；协议事件从 `trace` 读取。EOF 或 `shutdown` 会退出服务。调用方应在测试结束时关闭 stdin、等待并回收子进程。

默认按协议 7 的 S7-1200 布局：DB4（PLC 写、PC 读）、DB2（PC 写、PLC 读），各 68 字节，PDU 240。可选 `--plc-db 321 --pc-db 322` 改两个 DB 号（范围 1–65535，两者不能相同，也不能占用示教保留的 DB6 / DB12）；`--db-size` 为两个 DB 各自的大小，默认 1024，范围 68–65536；`--pdu-size` 默认 240，范围 240–960。S7 实际协商取客户端请求与服务端配置的较小值。参数不接受前缀缩写。

```powershell
python -u scripts/s7-handshake/s7_test_plc.py --port 0 --plc-db 4 --pc-db 2
```

## 默认点表

字段、取值与规则见[生产通讯协议 7](../../docs/integration/plc-communication-protocol.md)；标签名与 `src-tauri/src/inspection.rs` 一致。大端字节序，Int 为有符号 16 位，DInt 为有符号 32 位，SN 为 S7 STRING[50]（52 字节：声明长度 50、实际长度、字符、补 0）。

| 偏移 | DB4（PLC → PC） | DB2（PC → PLC） |
| --- | --- | --- |
| 0 | plcVersion | pcVersion |
| 2 | plcHeartbeat | pcHeartbeat |
| 4 | plcState | pcState |
| 6 | plcReason | pcMode |
| 8 | requestSeq（DInt） | seqEcho（DInt） |
| 12 | productCode | resultCode |
| 14 | 保留（不配置点位，保持 0） | faultCode |
| 16 | sn | snEcho |

初始值：`plcVersion` 为 7，`sn` / `snEcho` 为空串（头为 `32 00`），其余为 0（`plcState` 从 0 开始）。`plcHeartbeat` 每 500 ms 加 1，到 32767 后回 0。PC 字段由被测客户端写入。SN 写入时须为 1–50 个 ASCII 可打印字符且去首尾空白后不为空，原样保存，不做 trim 或大小写变换。

## 示教 V1 数据区

服务额外创建 DB6（PC 写）和 DB12（PLC 写），各恰好 10 字节，不受 `--db-size` 影响。五个 Int 在偏移 0 / 2 / 4 / 6 / 8：

| DB | 字段顺序 |
| --- | --- |
| DB6 | teachPcVersion / teachPcState / teachRoundId / teachPcReason / teachPcReserved |
| DB12 | teachPlcVersion / teachPlcState / teachRoundIdEcho / teachPlcReason / teachPlcReserved |

DB12 初始版本 1、状态 50、原因 5，要求复位；其余为 0。服务不自动执行示教状态机，调用方通过 `plc_values` 或原始 DB 写入模拟状态和异常，也可向 DB6 / DB12 定向注入读写失败。协议细节见 [示教时序](../../docs/integration/teaching-capture-sequence.md)，现场 SCL 见 [整站接入](site/README.md)。

```json
{"id":40,"op":"plc_values","values":{"teachPlcVersion":1,"teachRoundIdEcho":7,"teachPlcReason":0,"teachPlcState":10}}
{"id":41,"op":"read_db","db":6,"byte":0,"size":10}
{"id":42,"op":"fault","function":"write","kind":"reject","db":6,"byte":2,"count":1}
```

## 控制命令

`read_db` 返回 `hex` 和字节数组 `data`。`write_db` 仅允许 `hex` / `data` 二选一。`db` 按真实 DB 号寻址，省略时使用 PLC 区 DB（默认 4）；访问 PC 区须显式传 `"db":2`。

```json
{"id":1,"op":"read_db","db":4,"byte":0,"size":68}
{"id":2,"op":"write_db","byte":8,"hex":"00000011"}
{"id":3,"op":"set_bit","byte":0,"bit":1,"value":true}
{"id":4,"op":"create_db","db":200,"size":1024}
{"id":5,"op":"status"}
{"id":6,"op":"trace","clear":true}
```

`plc_values` 按给定顺序原子写入任意已配置字段，不检查状态迁移；Int 接受 -32768–32767，DInt 接受 -2147483648–2147483647，可写负数或非法组合构造故障场景，任一值不合规则整批拒绝。拆成多条命令可模拟“先写数据、最后写状态”的中间快照。

PLC 侧握手由调用方逐步驱动，每步按协议 7 第五节迁移表检查当前 `plcState` / `pcState`，前置条件不满足时返回错误且不改内存；写入时数据在前、状态最后：

| 命令 | 前置条件 | 结果 |
| --- | --- | --- |
| `plc_request` | plcState 0，pcState 10 | 写 `values`（`productCode`、`sn` 等）；未给 `requestSeq` 时取当前值加 1，已到 2147483647 则进入 90（12）；置 10 |
| `plc_motion_end` | plcState 10，pcState 20，seqEcho = requestSeq | 置 20 |
| `plc_ack` | plcState 10（拒收）或 20，pcState 30 | 核对 seqEcho、snEcho 逐字节相等与结果组合（pcMode 1 不看结果码；拒收只接受 90 + 91–100）；合格置 30，否则 90（5） |
| `plc_release` | plcState 30，pcState 10 | 置 0 |
| `plc_reset` | pcState 99 且 plcState 10/20/30/90，或 plcState 99 且 pcState 10/99 | 置 0；plcState 90 时传 `"allowed":false` 表示现场不允许，保持 90（13） |
| `plc_reset_request` | plcState 90 | 操作员现场复位，置 99 |

服务不计时，布防、运动、结果、回就绪超时及心跳失活等进入 90 的情形用 `plc_values` 写 `plcReason` 与 `plcState` 构造。

```json
{"id":10,"op":"plc_request","values":{"productCode":7,"sn":"SN-12345"}}
{"id":11,"op":"plc_motion_end"}
{"id":12,"op":"plc_ack"}
{"id":13,"op":"plc_release"}
{"id":14,"op":"plc_values","values":{"plcReason":8,"plcState":90}}
{"id":15,"op":"plc_reset","allowed":false}
```

要覆盖其他点表，可用 `configure_handshake` 完整替换字段映射（握手命令仍要求协议 7 的全部标签）。字段类型为 `bool/u8/u16/i16/u32/i32/string`，bool 必须有 `bit`；`string` 固定为 S7 STRING[50]，须从偶数字节开始；头字节不合规时配置会写入空串。`db` 省略时为 PLC 区 DB：

```json
{"id":20,"op":"configure_handshake","fields":{"plcState":{"db":200,"byte":4,"type":"i16"},"requestSeq":{"db":200,"byte":8,"type":"i32"}}}
```

## 故障注入

`function` 为 `read/write/setup/any`；`count` 是要影响的匹配请求数，默认 1。可用 `db/byte/bit` 限定目标；byte 在请求范围内即匹配，bit 仅匹配 bit 请求。匹配请求之后才消耗次数。拒绝返回 S7 item 错误，默认 `0x03`，可设置 `code`；不改写被拒绝的 item。

```json
{"id":30,"op":"fault","function":"write","kind":"reject","db":2,"byte":8,"count":1}
{"id":31,"op":"fault","function":"read","kind":"delay","delay_ms":250,"count":1}
{"id":32,"op":"fault","function":"write","kind":"disconnect","after_apply":false}
{"id":33,"op":"fault","function":"write","kind":"disconnect","after_apply":true}
{"id":34,"op":"fault","kind":"heartbeat_stop"}
{"id":35,"op":"fault","kind":"heartbeat_resume"}
{"id":36,"op":"heartbeat","enabled":false,"interval_ms":500}
{"id":37,"op":"clear_faults"}
{"id":38,"op":"shutdown"}
```

`disconnect` 默认在执行前断线；`after_apply=true` 在执行后、发送响应前断线，用于验证“写入可能已经发生但未收到确认”的恢复。延迟只阻塞该 S7 连接，控制 API 仍可访问。停止心跳保持最后的值，用于验证 3 s 不变判失活。`clear_faults` 只清除待执行的协议故障，不改变心跳状态。

## 验证范围

Python 测试直接通过 socket 发送固定十六进制报文，并逐字节断言 COTP CC、S7 ACK_DATA、PDU reference、DB 字节/bit 数据、以 bit 为单位的长度、multi-item 填充以及 S7 错误码；不调用服务自身的报文编码函数充当客户端。还覆盖协议 7 的 DB4 / DB2 偏移、S7 STRING 头字节与补 0、整批原子拒绝、握手命令的迁移前置条件与结果核对、分包/粘包、读写拒绝、延迟、断线重连、心跳冻结、子进程控制 API 和非默认 DB 号。

Windows 受限环境若在 `socket.connect` 返回 `WinError 10013`，需在获准的主机环境运行同一测试命令；不能跳过 TCP 断言后记录为通过。这些测试不验证 SCL 状态机执行。协议 6 及更早的运行记录见 [S7 测试记录](../../docs/testing/TESTING.md#s7-一期握手)。

现场参考程序见 [site/gluesight_site.scl](site/README.md)，尚未在 TIA Portal 编译。`phase1_plc.scl` 是协议 6 的旧参考，已被它取代，仅作历史保留。接真实 Siemens CPU 的 PUT/GET、机架/插槽、网络和现场节拍验收仍属于后续硬件验证。
