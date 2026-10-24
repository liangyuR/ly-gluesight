# S7 握手测试 PLC

这个独立测试服务通过真实 TCP 套接字提供 TPKT / COTP / S7comm 的连接协商、ReadVar 和 WriteVar。它用于测试现有 `ly-plc` S7 客户端和一期握手，不启动桌面、不访问实际 PLC。

仅依赖 Python 3.11+ 标准库。服务固定绑定 `127.0.0.1`，由系统分配随机端口，不接受固定端口或外部监听地址。它不是 Siemens CPU 模拟器，不模拟 PLC 扫描周期、实际 PUT/GET 权限、优化 DB 或硬件时序。

```powershell
python -m unittest discover -s scripts/s7-handshake/tests -v
python -u scripts/s7-handshake/s7_test_plc.py --port 0
```

启动后，stdout 第一行是 JSON：

```json
{"event":"ready","protocol":"s7","host":"127.0.0.1","port":49152,"pid":1234,"control":"stdio-json","plc_db":4,"pc_db":2,"db_size":1024}
```

`49152` 是示例；客户端必须使用实际返回端口。Rust 集成测试可用 `Command` 启动该文件，将 stdin/stdout 设为管道，读取 ready 后配置 `ly-plc` 的 S7 host/port。控制命令每行一个 JSON，响应为 `{"id":原id,"ok":true,"result":...}` 或 `{"id":原id,"ok":false,"error":"..."}`。stdout 不包含日志；协议事件从 `trace` 读取。EOF 或 `shutdown` 会退出服务。调用方应在测试结束时关闭 stdin、等待并回收子进程。

默认按握手协议 6（分段连拍；布局沿用协议 5 的交互表 V1.1）的 S7-1200 布局：PLC 区 DB4（PLC 写、PC 读）、PC 区 DB2（PC 写、PLC 读），PDU 240，SN 为 S7 STRING[50]。可选 `--plc-db 321 --pc-db 322` 改两个 DB 号（范围 1–65535，两者不能相同，也不能占用示教保留的 DB6 / DB12）；`--db-size` 为两个 DB 各自的大小，默认 1024，范围 90–65536；`--pdu-size` 默认 240，范围 240–960。S7 实际协商取客户端请求与服务端配置的较小值：PC 请求 480，默认按 240 协商（S7-1200 常见协商结果，也是轮询 DB4 90 字节、DB2 72 字节的较紧约束）；需要模拟更大协商结果时传 `--pdu-size 480`。参数不接受前缀缩写，旧的 `--db`、`--sn-format` 已删除，传入即报错退出。

```powershell
python -u scripts/s7-handshake/s7_test_plc.py --port 0 --plc-db 4 --pc-db 2
```

## 默认一期点表

多字节数值使用大端字节序。16 位字段为 Int（有符号 16 位，协议只用 0–32767），序号类字段为 DInt（有符号 32 位，协议只用 0–2147483647）。PLC `protocolVersion` 初始为 6（协议 5 的 PLC 程序会被 PC 拒绝），`partSn`/`resultSn` 初始为空串（头为 `32 00`），其他数据初始为 0；`plcHeartbeat` 每 500 ms 翻转。PC 字段由被测客户端写入。

协议 6 是三台独立相机共用 Line0 高电平门控，相机按固定帧率采集。点表仍只有一组计数：`shotCount`、`deviceShots` 是预期总数 ΣN_exp（测试 PLC 不做任何核对），`deviceTriggers` 为预留字段，PLC 写 0，PC 不以它判断采集或排空完成；不按相机分别计数。旧协议的六个 `camera1/2/3Shots`、`camera1/2/3Triggers` 标签已删除，控制 API 及自定义映射均直接拒绝，不读取兼容。PLC 区至少 90 字节（0–89），DBX0.5–0.7、DBB1、DBB22–27、DBB34–37 为保留；PC 区至少 72 字节（0–71），DBX0.6–0.7、DBB1 为保留（保持 0，不配置点位）。当前点表共 28 个字段，布局与协议 5 完全相同；协议 6 与协议 4（单个 DB1、WORD、200 SMART 字符串）布局不兼容。

工件 SN 只用 S7 STRING[50]，共 52 字节：首字节为声明最大长度 50，第二字节为实际长度，随后 50 字节为字符，未用字符补 0。SN 为 1–50 个 ASCII 可打印字符（0x20–0x7E），去首尾空白后不能为空；原样保存与回显，不做 trim 或大小写变换。PLC 只在 `resultSn` 与 `partSn` 的头字节及每个字符逐字节相等时接受结果。

| 方向 | 地址 | 字段 |
| --- | --- | --- |
| PLC → PC | DB4.DBX0.0 / .1 / .2 / .3 / .4 | partStart / partEnd / resultAck / faultReset / plcHeartbeat |
| PLC → PC | DB4.DBW2 | protocolVersion（Int，6） |
| PLC → PC | DB4.DBD4 | requestSeq |
| PLC → PC | DB4.DBW8 / DBW10 | productCode / shotCount（Int，预期总数 ΣN_exp） |
| PLC → PC | DB4.DBD12 / DBD16 | planVersion / planReserved（填 0） |
| PLC → PC | DB4.DBW20 | deviceShots（Int，预期总数 ΣN_exp） |
| PLC → PC | DB4.DBD28 | ackSeq |
| PLC → PC | DB4.DBW32 | deviceTriggers（Int，预留，填 0） |
| PLC → PC | DB4.DBB38–89 | partSn（S7 STRING[50]） |
| PC → PLC | DB2.DBX0.0 / .1 / .2 / .3 / .4 / .5 | visionReady / armed / busy / done / visionFault / pcHeartbeat |
| PC → PLC | DB2.DBW2 | pcProtocolVersion（Int，6） |
| PC → PLC | DB2.DBD4 | resultSeq |
| PC → PLC | DB2.DBW8 / DBW10 | resultCode / faultCode（Int） |
| PC → PLC | DB2.DBD12 / DBD16 | acceptedSeq / acceptedPlanReserved（填 0） |
| PC → PLC | DB2.DBB20–71 | resultSn（S7 STRING[50]） |

## 示教 V1 数据区

服务额外创建 DB6（PC 写）和 DB12（PLC 写），各恰好 10 字节，不受 `--db-size` 影响。五个 Int 在偏移 0 / 2 / 4 / 6 / 8：

| DB | 字段顺序 |
| --- | --- |
| DB6 | teachPcVersion / teachPcState / teachRoundId / teachPcReason / teachPcReserved |
| DB12 | teachPlcVersion / teachPlcState / teachRoundIdEcho / teachPlcReason / teachPlcReserved |

DB12 初始版本 1、状态 50、原因 5，要求复位；其余为 0。服务不自动执行示教状态机，调用方通过 `plc_values` 或原始 DB 写入模拟状态和异常，也可向 DB6 / DB12 定向注入读写失败。控制命令一批写入原子应用，可拆为多条命令模拟状态最后提交的中间快照。协议细节见 [示教时序](../../docs/integration/teaching-capture-sequence.md)，现场 SCL 见 [整站接入](site/README.md)。

```json
{"id":40,"op":"plc_values","values":{"teachPlcVersion":1,"teachRoundIdEcho":7,"teachPlcReason":0,"teachPlcState":10}}
{"id":41,"op":"read_db","db":6,"byte":0,"size":10}
{"id":42,"op":"fault","function":"write","kind":"reject","db":6,"byte":2,"count":1}
```

## 控制命令

`read_db` 返回 `hex` 和字节数组 `data`。`write_db` 仅允许 `hex` / `data` 二选一。`db` 按真实 DB 号寻址，省略时使用 PLC 区 DB（默认 4）；访问 PC 区须显式传 `"db":2`。

```json
{"id":1,"op":"read_db","db":4,"byte":0,"size":90}
{"id":2,"op":"write_db","byte":4,"hex":"00000011"}
{"id":3,"op":"set_bit","byte":0,"bit":1,"value":true}
{"id":4,"op":"create_db","db":200,"size":1024}
{"id":5,"op":"status"}
{"id":6,"op":"trace","clear":true}
```

`plc_request` 原子写入全部 `values` 并默认置 `partStart=true`；`partSn`/`resultSn` 取 JSON 字符串，按上述 SN 规则校验后写入 S7 STRING 头字节、字符和补 0，不合规时整批拒绝；Int 字段接受 -32768–32767，DInt 字段接受 -2147483648–2147483647，可写负数构造越界场景。`plc_ack` 要求 `resultSn` 与 `partSn` 逐字节相等，否则拒绝且不改写内存；相等时默认将当前 `resultSeq` 回显到 `ackSeq`，然后置 `resultAck=true`。调用方负责在看到有效 `done` 及匹配结果后发 ACK。`plc_release` 在 PC `done`、`busy` 均为 false 时清除 `partStart/partEnd/resultAck`。服务不自动发新件或自动确认错误结果，测试可通过 `plc_values` 或原始 DB 写入刻意构造旧序号、错误 ACK 和协议违规场景。

```json
{"id":10,"op":"plc_request","values":{"requestSeq":17,"partSn":"SN-12345","productCode":7,"shotCount":4,"planVersion":1,"planReserved":0,"deviceShots":4}}
{"id":11,"op":"plc_values","values":{"partEnd":true,"deviceTriggers":0}}
{"id":12,"op":"plc_ack"}
{"id":13,"op":"plc_release"}
```

请求序号、计划版本和设备计数的匹配由被测握手逻辑验收；控制 API 不替客户端过滤这些故障输入。不合规 SN 只能用 `write_db` 直接构造原始 STRING 字节。要覆盖其他点表，可用 `configure_handshake` 完整替换字段映射，但不能恢复已删除的六个 camera 标签。字段类型为 `bool/u8/u16/i16/u32/i32/string`，bool 必须有 `bit`；默认点表 16 位字段为 `i16`、32 位字段为 `i32`；`string` 固定为 S7 STRING[50]，须从偶数字节开始；头字节不合规时配置会写入空串。`db` 省略时为 PLC 区 DB：

```json
{"id":20,"op":"configure_handshake","fields":{"partStart":{"db":200,"byte":0,"bit":0},"requestSeq":{"db":200,"byte":4,"type":"i32"},"resultAck":{"db":200,"byte":0,"bit":2}}}
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

`disconnect` 默认在执行前断线；`after_apply=true` 在执行后、发送响应前断线，用于验证“写入可能已经发生但未收到确认”的恢复。延迟只阻塞该 S7 连接，控制 API 仍可访问。停止心跳保持最后的 bit 值，用于验证真正的变化超时。`clear_faults` 只清除待执行的协议故障，不改变心跳状态。

## 验证范围

Python 测试直接通过 socket 发送固定十六进制报文，并逐字节断言 COTP CC、S7 ACK_DATA、PDU reference、DB 字节/bit 数据、以 bit 为单位的长度、multi-item 填充以及 S7 错误码；不调用服务自身的报文编码函数充当客户端。还覆盖分包/粘包、读写拒绝、延迟超时、执行前后断线、重新连接、心跳冻结、子进程控制 API、请求/ACK/释放和非默认 DB 号。

Windows 受限环境若在 `socket.connect` 返回 `WinError 10013`，需在获准的主机环境运行同一测试命令；不能跳过 TCP 断言后记录为通过。2026-10-22 已在主机环境运行上述命令，27 项全部通过，包含 DB4/DB2 线报文偏移（协议 5 与协议 6 布局相同）、S7 STRING 头字节与补 0、不合规 SN 原子拒绝、`resultSn` 不等时拒绝 ACK，以及 PC 经 S7 写入 DB2.DBB20 `resultSn` 的线报文。

2026-10-24 已在获准主机环境运行同一命令，28 项全部通过；新增真实 TCP 用例验证 DB6 / DB12 与 DB2 / DB4 独立、拒绝写状态时保留旧请求、DB12 越界访问返回错误。它不验证 SCL 状态机执行。

这些 Python 测试证明受支持的 S7 线协议和测试服务行为。2026-10-09 的 17 项生产 `ly_plc::PlcEngine` 线协议联调及 1 项事务文件恢复测试属于旧协议历史证据，不能作为协议 5 或协议 6 的通过记录；命令与历史证据见 [S7 测试记录](../../docs/testing/TESTING.md#s7-一期握手)。`phase1_plc.scl` 是 S7-1200/1500 接入参考（GlueSightPlcArea 设为 DB4、GlueSightPcArea 设为 DB2），仅完成静态核对，尚未在 TIA Portal 编译。接真实 Siemens CPU 的 PUT/GET、机架/插槽、网络和现场节拍验收仍属于后续硬件验证。
