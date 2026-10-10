# Robot + PLC 模拟服务

用于 GlueSight 的本机协议联调和图像演示。Robot、Modbus TCP PLC、虚拟设备触发桥各自独立运行，默认绑定 127.0.0.1。Robot 写型号、SN 和计划拍照数，等待布防，按拍照点顺序触发设备，最后读取并确认检测结果。

配方采用 schemaVersion 5。当前演示夹具每个拍照点选择一幅图，保存 id、poseId、camera、view、bead、path（所选图像里的像素中线）、mmPerPx、detect 和 limits；可选 calib、skip。标定缺省引用为 camera-v1/v2/v3。旧版本夹具须重新建立；多图示教使用配方工作台的整圈采集流程。

| 夹具 | 产品代码 | 设备与视角顺序 | 每件设备触发数 |
| --- | --- | --- | --- |
| [ROBOT-DEMO-TRICAM.json](fixtures/ROBOT-DEMO-TRICAM.json)，默认 | 102 | cam1/v1 → cam1/v2 → cam1/v3 → cam1/v1 | cam1: 4 |
| [ROBOT-DEMO.json](fixtures/ROBOT-DEMO.json) | 101 | cam1/v1，共四点 | cam1: 4 |
| [ROBOT-DEMO-3CAM.json](fixtures/ROBOT-DEMO-3CAM.json) | 103 | cam1/v1 → cam2/v1 → cam3/v1 → cam1/v1 | cam1: 2、cam2: 1、cam3: 1 |

三目表示一台设备、一次触发交付三个视角，共用设备会话和计数。默认四点产生四次触发、十二幅设备图像、四张选中图像；视角数量不会增加 PLC 拍照数。三设备夹具用于保留设备间交错的覆盖，不代表现场有三台设备。网页展示的是拍照点顺序及设备内触发序号。

## 运行命令

从仓库根目录运行。Python 3.11+ 的服务和测试只用标准库；控制台与桥测试需要 Node 22+。桌面演示另外需要 PowerShell 7、项目构建环境、WebView2 和支持示教胶路的 lyFlow core DLL。

| 命令 | 用途 |
| --- | --- |
| pnpm sim:test | Python 协议、配方、样本测试，以及 Node 控制台和桥契约测试；无需桌面或 DLL |
| pnpm sim:services | 只启动 Robot、PLC 和联调台 |
| pnpm sim:build | 构建专用调试版并启动桌面及三个服务；首次使用或 Rust 升级后运行 |
| pnpm sim:start | 复用已有构建和演示数据 |
| pnpm sim:verify | 对运行中的桌面执行五工况，核对结果、SN、设备计数、逐点身份与握手释放 |
| pnpm sim:stop | 本件完成后停止登记的演示进程，保留数据 |

无需 pnpm 的模型测试入口：

~~~powershell
python -m unittest discover -s scripts/robot-plc-demo/tests -v
node --test scripts/robot-plc-demo/tests/test-console.mjs
~~~

测试只用临时目录和系统分配的本机端口。Windows 沙箱若报 WinError 10013，应在获准的宿主环境运行本机 TCP/HTTP 测试。Node 的 RPC 测试使用显式测试应答，不证明桌面或 DLL 工作正常。

联调台默认地址为 http://127.0.0.1:8766/，PLC 为 127.0.0.1:1502，Unit ID 为 1。专用应用标识为 com.xyzrobotics.gluesight.robot-plc-demo，数据在 %APPDATA% 下的同名目录。同一用户只运行一个桌面演示实例。

## 首次配置桌面

1. 运行 pnpm sim:build，打开专用演示窗口。在系统设置启用 lyFlow 图像测量，填写当前 DLL 路径，确认引擎就绪，产品来源选择“PLC 下发产品代码”，帧录制选择“全部”。真实像素测量若仍报告“尚未接入”，应先完成算法接线；模型测试通过不能替代这一步。
2. 默认夹具配置一台 cam1：来源“模拟相机”、触发采集、viewCount 为 3。单视角夹具为 cam1/viewCount 1；三设备夹具需要 cam1、cam2、cam3 三台模拟设备，各 viewCount 1。
3. 按所选 JSON 创建候选配方，保持产品代码、全部逐点字段、站距、滤波、检测参数和限值一致。默认 P1→P4 的 Pose 为 Pose-1→Pose-4，视角为 1→2→3→1，图像为 1280×1024，人工像素当量为 0.112 mm/px，允许断口采用当前默认值 6 mm。
4. 每个拍照点取新样本。三目每次一起冻结三个视角，在该点选中的视角上保存 JSON 中的像素中线和当量，执行真实图像试测并保存。换视角会清空该点示教和试测，需重新保存。保存工件总览。
5. 运行 python scripts/robot-plc-demo/make-samples.py。脚本读取当前夹具对应的候选目录，核对 schema、设备、视角、尺寸和来源，导出新的样本目录。good/k1..k4.pgm 是四张选中图，导入为 OK；gap/k1..k4.pgm 是完整的缺胶组，导入为 NG_GAP。缺口位于第二个要检拍照点的中线中段，长度按该点断口限值及站距计算，不跨拍照点连接。
6. 在工作台选择两组样本，运行规则与图像验证，通过后发布。发布包及真实示教 DLL 未接线时，此步骤会失败，不能直接把夹具写成已验证生产配方。
7. PLC 使用 Modbus TCP、127.0.0.1:1502、站号 1、轮询 50 ms、超时 1000 ms、自动连接，采用下表的演示地址。若已有示例配方引用不存在的设备，在专用演示实例中修正这些引用。
8. 联调台显示视觉、桥和握手就绪后运行一件，再执行 pnpm sim:verify。桥会拒绝配方、视角或引擎与夹具不一致的触发。

夹具不包含标定结果或发布记录。make-samples.py 只导出冻结的模拟原图并在图像中移除一段胶，不自动试测、判定或发布。provenance.json 明确记录 imageEngineValidated=false、physicalValidation=false、逐幅尺寸和路径、选中视角及设备触发数。默认三目还导出 replay/cam1_1_v1..v3.pgm 等十二幅图，可按设备帧分组回放；单视角导出四幅。导出的每组目录使用独立名称，避免混入上次样本。

自定义候选可以用 --workspace 指定 workspace.json 所在目录，用 --output 指定新的输出目录。工位标定用的 make-board.py 仍可独立使用，其合成板比例为 0.08 mm/px，与本夹具的人工当量不同。

## 配置与恢复

demo.config.json 是默认服务配置，版本为 1。路径相对该文件解析。复制成 demo.local.json（已忽略）后，可修改 robot.recipe 来切换夹具，修改 runtimeDir 隔离输出，修改三个端口隔离模型服务：

~~~powershell
pnpm sim:services -Config scripts/robot-plc-demo/demo.local.json
pnpm sim:build -Config scripts/robot-plc-demo/demo.local.json
pnpm sim:verify --config scripts/robot-plc-demo/demo.local.json
pnpm sim:stop -Config scripts/robot-plc-demo/demo.local.json
~~~

- 开始前必须同时满足 PLC 可连接、触发桥在线、视觉就绪，以及 C10/C11/C12/C13/C21/C22/C23 全部为 0。HTTP /start 同样检查，旧页面无法绕过。
- “本件后停止”会完成当前件并释放握手；重复停止幂等。等待期间不能开始新件或复位。触发、结果或写入出错时保留具体错误，不显示为成功停止。
- Robot 空闲且 PLC 可连接时可执行故障复位。复位发送 C13 脉冲，不能修复缺失设备、错误视角、未发布配方或不可用 DLL。
- HTTP 断开时禁用操作、将 PLC 值显示为未知；恢复后重新读取状态。PLC 单独离线时不能启动或复位，桥离线时不能启动。
- releaseSeconds 默认 0.2 秒，保持 C10/C11/C12 为低，至少应覆盖两次 PLC 轮询。motionSeconds 是每点运动时间，settleSeconds 是最后触发后等待，intervalSeconds 是件间隔。

Start-Demo.ps1 的 -Recipe 覆盖同时传给 Robot 和桥。升级后先 sim:stop，再重新构建、启动。启动和停止使用实际 PID、可执行路径与启动时间确认进程身份；升级源码或配置后应先停止再重启。

## PLC 契约与工况

地址从 0 开始，32 位整数按高字在前 ABCD 顺序。演示 PLC 有 256 个线圈及寄存器，支持功能码 1/2/3/4/5/6/15/16。离散输入与线圈共用存储，输入寄存器与保持寄存器共用存储。

| 地址 | 含义 | 写入方 |
| --- | --- | --- |
| C0 | 心跳 | GlueSight |
| C10 / C11 / C12 / C13 | 工件开始 / 运动结束 / 结果确认 / 故障复位 | Robot |
| C20 / C21 / C22 / C23 | 视觉就绪 / 已布防 / 检测中 / 结果有效 | GlueSight |
| HR100..101 / HR102 / HR103 | SN / 产品代码 / 拍照点数 | Robot |
| HR104..105 | 保留 | — |
| HR110 / HR111 / HR112..113 | 结果码 / 异常码 / 结果 SN | GlueSight |

设备计数作为 HTTP 状态和结果证据记录，没有新增 Modbus 地址。生产 S7 的设备槽、序号 ACK 和 planReserved 仍按 [S7 契约](../../docs/integration/plc-s7-phase1.md)；单台三目四点在 S7 只占一个设备槽，cameraShots=[4,0,0]。

| 工况 | 保留的结果码 / 异常码期望 | 设备触发数 / 预期到帧数 |
| --- | --- | --- |
| normal | 1 / 0 | 4 / 4 |
| gap | 13 / 0 | 4 / 4；逐点段内缺胶 |
| lostFrame | 90 / 91 | 4 / 3；最后一帧被抑制 |
| locateFail | 90 / 99 | 4 / 4；最后一点胶线平移 130px |
| countMismatch | 90 / 94 | 0 / 0；布防前拒绝 |

这些是需要真实桌面/DLL 验证的契约期望。Python VisionPeer 只为协议测试写应答寄存器。locateFail 保留历史工况键名，在逐点示教模型里由图像偏移产生；当前真实示教 DLL 将整个检测区无胶作为测量错误 99，部分断口仍为 NG_GAP 13。本表依此语义更新，桌面五工况仍须实跑，不能用测试应答代替。

GET /state 提供 contractVersion=2、recipe、plannedTriggers、deviceTriggers、cycleId、recipeRevision、bundleId、可操作状态及 PLC 快照。POST /start 接收 scenario、count（1..100）、continuous，返回 runId。POST /stop 在本件后停止，POST /reset 只允许空闲时执行。

GET /trigger 的 ticket 除 sn/k/scenario 外，还带 recipeId、productCode、shotCount、shotId、poseId、camera、view 和设备内 ordinal。桥先读取 cycle_snapshot、cycle_layout（当前修订 ID）、camera_rig_config、cycle_get_settings、engine_status 和 app_info，核对全部参与设备、逐点参数、cycleId 与发布资源身份，然后只调用已有 sim_robot_trigger(sn,k,scenario)。临发脉冲前再查工件，禁止中途换件或换包。成功 ACK 必须回传相同身份，Robot 把整件 ACK 绑定到同一 cycleId。已执行 ticket 缓存 ACK，丢失 HTTP 应答后的重试不会产生第二次脉冲；桥中途重启不能接续 k>0，需恢复后重跑。

真实三目 SDK 的交付格式、同步计数和线路仍待现场确认。桥只允许模拟设备，不配置 SDK 拆包或实体触发线路。

## 故障矩阵与验收边界

| 故障 | 当前入口或验证层 |
| --- | --- |
| 设备间交错到达 | 三设备夹具保留 cam1→cam2→cam3→cam1 及各设备序号；任意到达顺序应单独跑 Rust shot_router 的 interleaved_frames 测试 |
| 丢首/中/尾帧、多余触发 | 桌面支持 lostFrame 丢尾；首/中丢帧及额外计数由 Rust shot_router 故障矩阵验证 |
| 错误设备、错误/缺失视角 | Node 桥契约测试证明发脉冲前拒绝；真实图像缺视角仍需 Rust/桌面验证 |
| 上一件迟到、同 SN 重检 | 桥测试拒绝 cycleId 或发布包变更；原帧与迟到测量隔离由 Rust shot_router/cycle 验证 |
| 触发桥失联、拒绝 ACK、ACK 身份不符 | Python 的真实 HTTP/Modbus 测试证明不计完成、清输入并保留错误 |
| 队列满、DLL 卡住 | 应另跑 Rust measure 的排队截止、占用容量与迟到结果测试；本脚本没有虚构的 DLL 卡死注入命令，真实 DLL 卡死与尾延迟仍需专项回归 |
| PLC 写拒绝 | Python 测试通过实际 Modbus 异常响应注入，核对不计完成及后续恢复 |
| 磁盘写错误 | Python 测试使结果文件不可写，核对不计完成及恢复；录制/发布磁盘错误需另跑对应 Rust 测试 |

sim:test 通过只证明上述脚本范围。P0 步 7 完整验收还需要真实 DLL 在线图像闭环、真实故障恢复、排队/引擎耗时、内存趋势，以及现场 SDK 和准确率证据。运行层的 native 测试要求见 [P0 计划](../../docs/architecture/p0-plan.md) 和 [测试记录](../../docs/testing/TESTING.md)。

## 回归报告与数据

~~~powershell
pnpm sim:stop
pnpm sim:test
pnpm sim:build
pnpm sim:verify
~~~

sim:verify 真实增加五件演示记录，按 runId 关联结果，逐件检查结果码、异常码、SN、握手释放和设备触发数。只运行指定工况可用 --scenarios normal gap。失败返回非零并覆盖旧的通过报告。

图像工况的报告要求真实应用/引擎应答、非空 cycleId、recipeRevision 与不可变 bundleId，记录应用版本、引擎版本、DLL 实际路径和版本、夹具尺寸和明确配方版本、逐点 ACK 和设备计数。测试应答端不能作为真实图像回归证据。不计算或比较图像、配方、发布包和引擎文件的内容摘要。triggerAckMs 是桥命令的应答时间，resultWaitMs 是 partEnd 后等待结果的时间，cycleMs 是 Robot 整件时间；它们不等于引擎或排队耗时。报告的 unmeasured 明示这些未测量项及现场精度缺口。

默认输出在 output/playwright/robot-plc-demo/（已忽略）：

- services.json：进程身份、配置路径与配方路径；停止脚本只关闭登记身份匹配的进程。
- plc-wire.jsonl、robot-events.jsonl、robot-results.jsonl：实际协议值变化、事件和结果；重启恢复，末行写入中断时保留备份。
- regression-latest.json：最近一次真实联调报告，包含失败原因。
- samples/：按次生成的选中视角样本、全部冻结视角回放图及 provenance.json。
- stdout/stderr 日志、演示程序与截图。

源码、配置、夹具、测试和本说明可提交。DLL、exe、客户原图、AppData、依赖和运行输出不应提交。
