# Robot + PLC 模拟服务

用于 GlueSight 开发、升级后的联调回归和无设备演示。Robot、PLC、虚拟相机触发桥各自独立运行；默认全部绑定 `127.0.0.1`。

Robot 向 PLC 写入型号、SN、计划拍照数，等待 GlueSight 布防，按配方拍照点发出虚拟脉冲，最后接收并确认检测结果。PLC 使用真实 Modbus TCP 报文。相机桥只传递触发，图像由 GlueSight 模拟相机生成，定位、测量和判定仍走 lyFlow。

## 常用命令

从仓库根目录运行。需要 Python 3.11+；完整的 `sim:test` 还需要 Node 22+ 和 pnpm。启动桌面联调还需要 PowerShell 7、Rust/MSVC、WebView2 以及兼容项目的 lyFlow core DLL。Python 服务和测试仅使用标准库，无需 pip 安装。

| 命令 | 用途 |
| --- | --- |
| `pnpm sim:test` | 自动回归模型服务；不需要桌面、DLL 或已配置实例 |
| `pnpm sim:services` | 仅启动 PLC、Robot 和网页联调台，适合开发接口 |
| `pnpm sim:build` | 构建专用调试版并启动四个进程；首次桌面启动和 Rust 升级后使用 |
| `pnpm sim:start` | 使用已构建的桌面程序启动，保留配方、原图和历史 |
| `pnpm sim:verify` | 对运行中的桌面联调执行五种工况，核对结果码、SN 和握手释放 |
| `pnpm sim:stop` | 等本件完成后停止登记的模拟进程，保留数据 |

第一次构建前运行 `pnpm install --frozen-lockfile`。也可以双击 `启动联调.cmd` / `停止联调.cmd`；首次未构建时脚本会提示使用 `sim:build`。

默认联调台为 <http://127.0.0.1:8766/>，PLC 为 `127.0.0.1:1502`，Unit ID 为 `1`。桌面应用标识固定为 `com.xyzrobotics.gluesight.robot-plc-demo`，数据保存在 `%APPDATA%` 下的同名目录。不要使用正式应用窗口连接相机桥。

`sim:services` 不运行图像检测；网页显示触发桥离线是预期状态。`sim:test` 会自行创建测试用视觉应答端，从真实 TCP/HTTP 验证协议流程。它不会被启动脚本接入桌面联调。

## 首次配置桌面

`sim:build` 可以从没有演示数据的环境启动。随后在专用 GlueSight 窗口完成以下配置；后续升级会复用配置。

1. 系统设置选择产品来源“PLC 下发产品代码”、飞拍测量“lyFlow”、帧录制“全部”。填写本机 `lyflow_core.dll` 路径并确认引擎就绪。DLL 的构建与 ABI 要求见 [项目测试说明](../../docs/TESTING.md)。
2. 设备与采集新建或配置 `cam1`，来源“模拟相机”、触发采集。演示配方只使用此相机。
3. 运行 `python scripts/robot-plc-demo/make-board.py`，在工位标定中导入输出的 `calibration-board.pgm`。内角点 `9×6`，格长 `3.2 mm`；理想合成板的比例约为 `0.08 mm/px`。
4. 根据 [配方夹具](fixtures/ROBOT-DEMO.json) 创建飞拍候选：`ROBOT-DEMO`、产品代码 `101`、相机 `cam1`；圆角矩形 `380×200 mm`、半径 `24 mm`、视野 `216×145 mm`、测量间距 `0.5 mm`。四点依次 `(95,50)`、`(285,50)`、`(285,150)`、`(95,150)`。位置公差、滤波和断胶限值参照 JSON。
5. 四帧逐一取新样本、框选含安装孔及附近轮廓的定位模板、试测并保存。定位最低分数 `0.61`，搜索范围 `4 mm`，对比度 `25`。本例模板矩形可用 `[1225,75,350,350]`、`[912,75,350,350]`、`[1242,1387,350,350]`、`[867,1387,350,350]`；图像尺寸应为 `2700×1813`。保存工件总览。
6. 运行 `python scripts/robot-plc-demo/make-samples.py` 从演示候选导出合成样本。导入良品组 `k1..k4.pgm`，期望 `OK`；再导入 `k1-gap.pgm` 配合 `k2..k4.pgm`，期望 `NG_GAP`。全部导入后统一勾选两组，运行规则与图像验证，通过后发布。
7. PLC 选择 Modbus TCP、本机端口 `1502`、站号 `1`、轮询 `50 ms`、超时 `1000 ms`、自动连接，并使用默认握手地址表。如果出厂示例配方引用不存在的相机，在本演示实例中删除这些示例配方。
8. 联调台显示“视觉、相机触发桥与握手均已就绪，可以运行”后，运行单件或执行 `pnpm sim:verify`。

夹具 JSON 是配置规格，未包含示教哈希、标定结果或发布记录。启动脚本不会将未验证的夹具直接写成生产配方。生成器适用于上述默认四点几何；改胶路后应重新取样、示教、验证及发布。

## 运行、停止和错误恢复

1. 开始前看 Robot 按钮下方的提示。只有 PLC 可连接、相机触发桥在线、视觉就绪，且 C10/C11/C12/C13/C21/C22/C23 全部为 0 时，“运行一件”和“连续运行”才可用。仅 C20 视觉就绪为 1 不代表上一件已经收尾。HTTP `/start` 同样检查这些条件，旧网页或直接调用接口也不能绕过。
2. 单件完成后自动停止；连续运行时点击一次“本件后停止”。按钮会变为“停止请求已收到”，状态显示“正在停止 · 等待本件完成”。当前件继续拍照、检测、确认结果并释放握手，之后显示“已停止”。等待期间不能重新启动、复位或重复提交停止。
3. “本件后停止”用于正常收尾。如果本件发生触发无响应、结果 SN 不匹配等错误，联调台保留“需要检查”和具体原因，不会把错误显示为成功停止。先按提示恢复连接或检查 GlueSight 配置；Robot 已不在运行且 PLC 可连接后，可点“故障复位”。
4. 复位后显示“复位已请求”。复位只发送故障复位信号，不能修复缺失的相机、未发布的配方或离线的算法引擎。等按钮下方重新显示全部就绪，再运行一件确认恢复。
5. 如果网页显示“连接已中断”，按钮会全部禁用，PLC 当前值显示“—”，历史结果仍可查看。此时网页无法确认当前件是否结束；检查服务并等待自动重连，恢复后重新确认状态。PLC 单独离线时不能启动或复位；相机触发桥离线时不能启动。

## 修改配置

`demo.config.json` 是唯一的默认服务配置，含版本号、端口、站号、配方路径、运动时间和超时。相对路径以该 JSON 所在目录为基准。

本机差异可复制为本目录 `demo.local.json`（已忽略），再运行：

```powershell
pnpm sim:services -Config scripts/robot-plc-demo/demo.local.json
pnpm sim:stop -Config scripts/robot-plc-demo/demo.local.json
pnpm sim:build -Config scripts/robot-plc-demo/demo.local.json
pnpm sim:verify --config scripts/robot-plc-demo/demo.local.json
```

修改 `runtimeDir` 可以隔离服务日志；同时修改三个端口可以启动另一组纯模型服务。**桌面应用的数据目录由固定应用标识决定，同一用户只运行一个桌面演示实例。** 修改 PLC 端口或站号后，要在 GlueSight 通讯配置中同步修改；改配方后要保证 Robot 的 `id/productCode/shots` 与已发布配方一致。

`motionSeconds` 是每点的模拟运动时间，`settleSeconds` 是末次触发到运动结束的等待，`intervalSeconds` 是连续件间隔。`releaseSeconds` 是每件完成前保持 C10/C11/C12 为低的时间，默认 `0.2 s`；应至少覆盖两次 PLC 轮询，避免连续启动时漏掉下降沿。`timeouts` 分别约束就绪、布防、触发应答、结果和释放等待。

可绕过 PowerShell，单独启动服务（Windows/Linux 均可运行 Python 服务）：

```text
python scripts/robot-plc-demo/plc_service.py --config scripts/robot-plc-demo/demo.config.json
python scripts/robot-plc-demo/robot_service.py --config scripts/robot-plc-demo/demo.config.json
node scripts/robot-plc-demo/camera-bridge.mjs --config scripts/robot-plc-demo/demo.config.json
```

独立命令的进程需自行管理。桌面桥要求专用调试构建和 WebView2 调试端口，通常使用 `sim:start` 统一启动。

## 握手地址及工况

地址从 0 开始；32 位整数使用高字在前的 ABCD 顺序。PLC 提供 256 个线圈及 256 个寄存器，支持功能码 1/2/3/4/5/6/15/16；本模型中离散输入与线圈共用存储，输入寄存器与保持寄存器共用存储。

| 地址 | 含义 | 写入方 |
| --- | --- | --- |
| C0 | 心跳 | GlueSight |
| C10 / C11 / C12 / C13 | 工件开始 / 运动结束 / 结果确认 / 故障复位 | Robot |
| C20 / C21 / C22 / C23 | 视觉就绪 / 已布防 / 检测中 / 结果有效 | GlueSight |
| HR100..101 / HR102 / HR103 | 工件 SN / 产品代码 / 拍照数 | Robot |
| HR104..105 | 随动进度，飞拍模型保持 0 | Robot |
| HR110 / HR111 / HR112..113 | 结果码 / 异常码 / 结果 SN | GlueSight |

| 工况 | 结果码 / 异常码 | 默认四点配方收到帧数 |
| --- | --- | --- |
| `normal` | 1 / 0 | 4 |
| `gap` | 13 / 0 | 4 |
| `lostFrame` | 90 / 91 | 3 |
| `locateFail` | 90 / 92 | 4 |
| `countMismatch` | 90 / 94 | 0，布防前拒绝 |

以上期望对应仓库默认夹具。胶路或规则改变后，应重新验证期望，不能为了让升级检查变绿而放宽断言。

Robot HTTP 接口：`GET /state` 获取状态和 PLC 快照，包括 `stopping`、`canStart`、`startBlockedReason` 和 `canReset`；`POST /start` 接收 `scenario`、`count`（1..100）、`continuous`，`POST /stop` 在本件后停止，`POST /reset` 仅空闲时复位。重复停止请求幂等，不会中断当前件或重复写入停止事件。`/start` 返回 `runId`；结果也带该 ID，因此检查不依赖历史件数。`GET /trigger` 与 `POST /trigger-ack` 由相机桥使用；ticket ID 包含进程会话 ID，重复、过期或不匹配的应答会被拒绝。

## 升级验证与数据

```powershell
pnpm sim:stop
pnpm sim:test
pnpm sim:build
pnpm sim:verify
```

`sim:test` 使用临时目录和操作系统分配的空闲端口，不读取演示 AppData、不需要联网下载，也不改动已运行的服务。测试覆盖 TCP 分片/粘包、多客户端、字序、异常报文、空闲连接与半包超时、五工况握手、连续停止与重复停止、完整待机检查、离线与复位反馈、错误 SN、缺失触发应答、重复启动、Origin 与输入校验、重启恢复、断电造成的末行损坏，以及回归失败覆盖旧通过报告。可在 CI 直接运行 `python -m unittest discover -s scripts/robot-plc-demo/tests -v`。

联调台控件状态的回归使用 Node 内置测试器，无需安装额外依赖：`node --test scripts/robot-plc-demo/tests/test-console.mjs`。它覆盖在线状态过期、请求提交中、停止等待、残留握手及 PLC 断开后的按钮行为。

`sim:verify` 会真实增加五件演示记录，逐件核对结果码、异常码、SN 和握手释放；失败时返回非零退出码并保留报告。只跑指定工况：`pnpm sim:verify --scenarios normal gap`。它不清空历史、不核对截图或图像精度；图像算法本身的原生回归见项目测试说明。

默认运行输出在 `output/playwright/robot-plc-demo/`（已被 Git 忽略）：

- `services.json`：进程 PID、启动时间、可执行路径和源码/配置哈希。停止脚本只关闭身份匹配的进程。
- `plc-wire.jsonl`：实际值变化；`robot-events.jsonl`：握手事件；`robot-results.jsonl`：完整结果，重启恢复。
- `regression-latest.json`：最近一次五工况检查报告，包括失败原因。
- `*.stdout.log` / `*.stderr.log`：服务日志；模拟程序、生成图、截图也放在此目录。

首次手动试用的截图、旧报告和脚本保留在本机输出目录，不作为升级回归的固定输入。源码、配置、夹具、测试和使用说明保存在本目录；DLL、exe、原图、日志、AppData 和依赖目录不应提交。

## 维护入口

- `simulator_config.py`：配置校验与默认工况期望；增加配置字段时同步示例和测试。
- `plc_service.py` / `modbus_wire.py`：PLC 内存与 Modbus 编解码。
- `robot_service.py`：运动/握手状态机、结果恢复、HTTP API。
- `console.html` / `console.mjs`：网页结构与状态显示；新增静态资源时同步 `robot_service.py` 的资源白名单。启动脚本的源码哈希包含 `.mjs`，改动后需要停止再启动服务。
- `camera-bridge.mjs`：WebView2 通道与虚拟脉冲，只调用 `sim_robot_trigger`。
- `src-tauri/src/sim.rs`：图像工况与受保护的外部触发；`src-tauri/src/simimage.rs`：合成图像。

外部触发仅在专用调试应用标识、回环 Modbus PLC、模拟相机、已布防工件、匹配 SN 和连续 k 下允许。正式发布构建拒绝此命令。模型不包含品牌机器人运动学、碰撞检查或实物精度验收。
