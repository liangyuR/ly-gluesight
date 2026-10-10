# S7 握手测试 PLC

这个独立测试服务通过真实 TCP 套接字提供 TPKT / COTP / S7comm 的连接协商、ReadVar 和 WriteVar。它用于测试现有 `ly-plc` S7 客户端和一期握手，不启动桌面、不访问实际 PLC，也不修改旧 `robot-plc-demo`。

仅依赖 Python 3.11+ 标准库。服务固定绑定 `127.0.0.1`，由系统分配随机端口，不接受固定端口或外部监听地址。它不是 Siemens CPU 模拟器，不模拟 PLC 扫描周期、实际 PUT/GET 权限、优化 DB 或硬件时序。

```powershell
python -m unittest discover -s scripts/s7-handshake/tests -v
python -u scripts/s7-handshake/s7_test_plc.py --port 0
```

启动后，stdout 第一行是 JSON：

```json
{"event":"ready","protocol":"s7","host":"127.0.0.1","port":49152,"pid":1234,"control":"stdio-json","db":100,"db_size":1024}
```

`49152` 是示例；客户端必须使用实际返回端口。Rust 集成测试可用 `Command` 启动该文件，将 stdin/stdout 设为管道，读取 ready 后配置 `ly-plc` 的 S7 host/port。控制命令每行一个 JSON，响应为 `{"id":原id,"ok":true,"result":...}` 或 `{"id":原id,"ok":false,"error":"..."}`。stdout 不包含日志；协议事件从 `trace` 读取。EOF 或 `shutdown` 会退出服务。调用方应在测试结束时关闭 stdin、等待并回收子进程。

可选 `--db 321` 将默认点表置于 DB321；`--db-size` 默认 1024，范围 88–65536；`--pdu-size` 默认 960，范围 240–960。S7 实际协商取客户端请求与服务端配置的较小值。

## 默认一期点表

多字节数值使用大端字节序。PLC `protocolVersion` 初始为 1，其他数据初始为 0；`plcHeartbeat` 每 500 ms 翻转。PC 字段由被测客户端写入。

| 方向 | 地址 | 字段 |
| --- | --- | --- |
| PLC → PC | DB100.DBX0.0 / .1 / .2 / .3 / .4 | partStart / partEnd / resultAck / faultReset / plcHeartbeat |
| PLC → PC | DBW2 | protocolVersion |
| PLC → PC | DBD4 / DBD8 | requestSeq / partSn |
| PLC → PC | DBW12 / DBW14 | productCode / shotCount |
| PLC → PC | DBD16 / DBD20 | planVersion / 保留位（填 0） |
| PLC → PC | DBW24 / DBW26 / DBW28 | camera1Shots / camera2Shots / camera3Shots |
| PLC → PC | DBD32 | ackSeq |
| PLC → PC | DBW36 / DBW38 / DBW40 | camera1Triggers / camera2Triggers / camera3Triggers |
| PC → PLC | DB100.DBX64.0 / .1 / .2 / .3 / .4 / .5 | visionReady / armed / busy / done / visionFault / pcHeartbeat |
| PC → PLC | DBW66 | pcProtocolVersion |
| PC → PLC | DBD68 / DBD72 | resultSeq / resultSn |
| PC → PLC | DBW76 / DBW78 | resultCode / faultCode |
| PC → PLC | DBD80 / DBD84 | acceptedSeq / 保留位（填 0） |

## 控制命令

`read_db` 返回 `hex` 和字节数组 `data`。`write_db` 仅允许 `hex` / `data` 二选一。`db` 省略时使用启动时配置的默认 DB。

```json
{"id":1,"op":"read_db","db":100,"byte":0,"size":88}
{"id":2,"op":"write_db","byte":4,"hex":"00000011"}
{"id":3,"op":"set_bit","byte":0,"bit":1,"value":true}
{"id":4,"op":"create_db","db":200,"size":1024}
{"id":5,"op":"status"}
{"id":6,"op":"trace","clear":true}
```

`plc_request` 原子写入全部 `values` 并默认置 `partStart=true`；`plc_ack` 默认将当前 `resultSeq` 回显到 `ackSeq`，然后置 `resultAck=true`。调用方负责在看到有效 `done` 及匹配结果后发 ACK。`plc_release` 在 PC `done`、`busy` 均为 false 时清除 `partStart/partEnd/resultAck`。服务不自动发新件或自动确认错误结果，测试可通过 `plc_values` 或原始 DB 写入刻意构造旧序号、错误 ACK 和协议违规场景。

```json
{"id":10,"op":"plc_request","values":{"requestSeq":17,"partSn":12345,"productCode":7,"shotCount":4,"planVersion":1,"planReserved":0,"camera1Shots":2,"camera2Shots":1,"camera3Shots":1}}
{"id":11,"op":"plc_values","values":{"partEnd":true,"camera1Triggers":2,"camera2Triggers":1,"camera3Triggers":1}}
{"id":12,"op":"plc_ack"}
{"id":13,"op":"plc_release"}
```

请求序号、工件SN、计划版本和相机计数的匹配由被测握手逻辑验收；控制 API 不替客户端过滤故障输入。要覆盖其他点表，可用 `configure_handshake` 完整替换字段映射。字段类型为 `bool/u8/u16/u32`，bool 必须有 `bit`：

```json
{"id":20,"op":"configure_handshake","fields":{"partStart":{"db":200,"byte":0,"bit":0},"requestSeq":{"db":200,"byte":4,"type":"u32"},"resultAck":{"db":200,"byte":0,"bit":2}}}
```

## 故障注入

`function` 为 `read/write/setup/any`；`count` 是要影响的匹配请求数，默认 1。可用 `db/byte/bit` 限定目标；byte 在请求范围内即匹配，bit 仅匹配 bit 请求。匹配请求之后才消耗次数。拒绝返回 S7 item 错误，默认 `0x03`，可设置 `code`；不改写被拒绝的 item。

```json
{"id":30,"op":"fault","function":"write","kind":"reject","db":100,"byte":76,"count":1}
{"id":31,"op":"fault","function":"read","kind":"delay","delay_ms":250,"count":1}
{"id":32,"op":"fault","function":"write","kind":"disconnect","after_apply":false}
{"id":33,"op":"fault","function":"write","kind":"disconnect","after_apply":true}
{"id":34,"op":"fault","kind":"heartbeat_stop"}
{"id":35,"op":"fault","kind":"heartbeat_resume"}
{"id":36,"op":"heartbeat","enabled":false,"interval_ms":500}
{"id":37,"op":"clear_faults"}
{"id":38,"op":"shutdown"}
```

`disconnect` 默认在执行前断线；`after_apply=true` 在执行后、发送响应前断线，用于验证“写入可能已经发生但未收到确认”的恢复。延迟只阻塞该 S7 连接，控制 API 仍可访问。停止心跳保持最后的 bit 值，用于验证真正的变化超时。`clear_faults` 只清除待执行的协议故障，不改变心跳状态。

## 验证范围

Python 测试直接通过 socket 发送固定十六进制报文，并逐字节断言 COTP CC、S7 ACK_DATA、PDU reference、DB 字节/bit 数据、以 bit 为单位的长度、multi-item 填充以及 S7 错误码；不调用服务自身的报文编码函数充当客户端。还覆盖分包/粘包、读写拒绝、延迟超时、执行前后断线、重新连接、心跳冻结、子进程控制 API、请求/ACK/释放和非默认 DB。

Windows 受限环境若在 `socket.connect` 返回 `WinError 10013`，需在获准的主机环境运行同一测试命令；不能跳过 TCP 断言后记录为通过。2026-10-09 已在主机环境运行上述命令，21 项全部通过。

这些 Python 测试证明受支持的 S7 线协议和测试服务行为。2026-10-09 已另行通过 17 项生产 `ly_plc::PlcEngine` 与本服务的线协议联调，以及 1 项事务文件恢复测试；命令与证据见 [S7 测试记录](../../docs/testing/TESTING.md#s7-一期握手)。接真实 Siemens CPU 的 PUT/GET、机架/插槽、网络和现场节拍验收仍属于后续硬件验证。
