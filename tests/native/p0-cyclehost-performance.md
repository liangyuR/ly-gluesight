# P0 步 7：真实 CycleHost 性能与演示验收

当前为工具准备稿。脚本语法、证据自检通过不代表完成 100 件原生验收；实际结果以新的报告和逐件 JSONL 为准。

## 实际运行层

p0-cyclehost-performance.mjs 连接一个已经启动、配置完成的专用 P0 Tauri 实例。通过现有 UI 选择已发布配方、normal/gap 工况并逐件点击“运行一件”，读取真实 CycleHost 测量和历史记录。脚本不修改系统设置、相机、配方、发布包或检测限值。

每个组合独立运行至少 100 件：single/normal、single/gap、tricam/normal、tricam/gap。每件四个拍照点；single 为 cam1/v1 四点，tricam 为 cam1/v1→v2→v3→v1。不同组合必须使用相应设备配置和完成正常示教、试测、验证、发布的真实配方。不得直接把 Rust Prepared 基准产生的包当作已完成桌面发布流程。

运行前：

- 使用步 7 构建，确保 cycle_part_data 返回 ms、queueMs、engineMs、coreMs；DLL 和全部目录在 C 盘。
- 专用应用标识包含 com.xyzrobotics.tujiaovision.p0-tests；PLC 协议为 simulator、在线；全部参与设备为触发模拟相机；启用真实 LyFlow 测量及“全部”录制。
- 保持 armMs=200。初始状态为 IDLE，模拟器未运行。脚本不会自动复位故障。
- 录制保留数至少 100；建议专用验收实例设为 500 件或以上，以便四个组合结束后仍可复核所有原图。容量建议至少 8 GiB，并确认 C 盘实际可用空间。单个 tricam/100 件约产生 1.2 GiB 原图；single 约 0.4 GiB。
- --fixture 传入该发布包的 recipe.json；--release 传入包含 manifest.json 的该包目录。脚本校验清单 FNV 等于每件运行的 bundleHash，并核对配方、拍照点、视角和 1280×1024 尺寸。
- --executable 和 --pid 必须来自当前原生实例。内存采样核对进程路径及启动时间，拒绝复用 PID 或应用中途重启。
- 暂停 Cargo、前端 coverage、Prepared 基准及其它 CPU 密集任务；同一时间仅运行一个原生验收实例。

PowerShell 示例：先将路径和 PID 赋为当前原生实例的已核对值，再执行。输出目录必须不存在，每个组合使用新目录。

~~~powershell
$taskPerfRoot = 'C:\Users\11601\project\ly-gluesight\tmp\p0-step6-audit'
$taskPerfRuntime = 'C:\Users\11601\AppData\Local\npm-cache\_npx\e41f203b7505f1fb\node_modules\playwright\index.mjs'
$taskPerfArgs = @(
  "$taskPerfRoot\tests\native\p0-cyclehost-performance.mjs",
  '--playwright-module', $taskPerfRuntime,
  '--cdp', 'http://127.0.0.1:9338',
  '--mode', 'tricam',
  '--scenario', 'normal',
  '--fixture', (Join-Path $taskPerfRelease 'recipe.json'),
  '--release', $taskPerfRelease,
  '--executable', $taskPerfExecutable,
  '--pid', [string]$taskPerfPid,
  '--output', $taskPerfOutput,
  '--parts', '100'
)
& node @taskPerfArgs
& node "$taskPerfRoot\scripts\p0-cyclehost-report.mjs" (Join-Path $taskPerfOutput 'cyclehost-report.json')
~~~

也可在已有 Playwright page 上调用导出的 runCycleHostPerformance(page, options)，使用同一组选项，不需要把 Playwright 加入项目依赖。单独 CLI 的 --playwright-module 指向已安装运行时；不会下载浏览器。CDP 只允许本机端点，自动选择唯一带 Tauri 内部接口的页面，不创建浏览器标签页。

## 指标和通过条件

每件需完成 ACK、原图落盘及握手释放，核对 cycleId、SN、recipeHash、bundleHash、shotId、设备、视角、设备内序号、测量点归属及原图完整性。原图必须是 1280×1024 Gray8 PGM；记录逐幅 SHA256。软件 exe、DLL、夹具和整个冻结发布目录在运行前后核对 SHA256。

| 字段 | 实际含义 |
| --- | --- |
| ms | 提交测量至返回结果 |
| queueMs | 提交至 blocking worker 进入，包括通道、信号量及 blocking 执行器等待 |
| engineMs | worker 内真实图像 runner 墙钟时间，包括包装层 |
| coreMs | 成功原生图像测量器报告的时间 |
| partEndToPlcSubmissionMs | 数据库 drainMs：CycleHost 观察 partEnd 后至 awaited PLC result/done 写入完成；包含 PLC 写入等待，不含 ACK 和录制完成 |
| uiObservedCycleMs | 点击运行至观察 ACK、录制完成、模拟器 IDLE；含轮询和 UI 开销 |
| workingSet/private | 原生主进程在第 0、10、20…100 件握手和录制稳定后的工作集/私有字节；不包含 WebView2 子进程 |

输出 nearest-rank p50/p95/max、最小值和均值；内存输出每次原始采样、首尾变化及 bytes/part 最小二乘趋势。没有凭空设定内存斜率通过线，不能由一次有限时长运行宣称永不增长。第一件也计入；不声明进程、DLL、图或操作系统缓存为冷态。本机模拟器的固定运动/等待节拍不证明最大现场吞吐。

normal 严格期望 OK/PLC 1/fault 0，gap 严格期望 NG_GAP/PLC 13/fault 0。既有 P0-09 正常夹具实际返回 OK_WITH_EXCURSION/2 时仍采完 100 件、保存 accuracyFailures，completed=true、passed=false 并返回非零。不得放宽限值、把结果 2 重标为 1，或删除既有失败事实。若要新建直线合成夹具，应保留独立配方身份、原图来源及哈希，并完成真实桌面验证发布。

cyclehost-report.json 和 parts.jsonl 记录完整证据；中途失败保留已完成件及 error，completed=false。独立报告工具重算全部指标、逐件检查身份并重新校验文件哈希；任何伪造分位值、缺采样、错 cycleId 原图、丢视角、未知指标或篡改文件均不能通过。保存原图目录及所有引用文件后再做最终复核，避免保留策略删去较早组合原图。

这些结果属于真实软件运行层、真实 DLL 和合成像素；physicalValidation=false、s7HardwareValidation=false。PLC simulator 的完成不能替代 S7 实机、实体三目 SDK、线路、带宽或现场准确率验收。

## Robot/Modbus 五场景

Robot 演示采用另一个专用标识 com.xyzrobotics.gluesight.robot-plc-demo。使用 scripts/robot-plc-demo/README.md 的完整示教/样本/验证/发布过程，默认配方 ROBOT-DEMO-TRICAM，设备内四次触发、十二幅原图。正常夹具必须真实测得 PLC 1；若当前夹具仍得到 2，应保留失败报告，另建有明确来源的独立直线夹具完成演示，不改算法或限值掩盖问题。

确认 demo.config.json 或独立 demo.local.json 中的路径都指向 C 盘，runtimeDir 是独立新目录。专用窗口只能一个实例；PLC 默认 1502、Robot HTTP 8766、CDP 9340。先确认端口和现有进程身份。构建脚本支持显式 CARGO_TARGET_DIR；复制的是相应 debug/GlueSight.exe，不再固定读取仓库 src-tauri/target。

~~~powershell
& pwsh -NoProfile -File scripts/robot-plc-demo/Start-Demo.ps1 -Build -Config $taskDemoConfig
& python -X utf8 scripts/robot-plc-demo/run-regression.py --config $taskDemoConfig --scenarios normal gap lostFrame locateFail countMismatch
~~~

预期 result/fault 依次为 1/0、13/0、90/91、90/99、90/94。脚本核对真实 Robot HTTP、Modbus 返回、SN、设备触发数、逐点 ACK 身份、发布包、真实应用/引擎证据和握手释放，输出 runtimeDir/regression-latest.json；失败返回非零。报告会覆盖上次同名文件，每次运行后立即复制到新证据目录保留失败和成功版本。若首个场景失败，修复实际原因后再运行；需要分别诊断时用 --scenarios 指定单个场景并逐次保存报告，不能把模型 VisionPeer 测试应答当作桌面证据。

triggerAckMs、resultWaitMs、cycleMs 是桥/Robot 协议时间，不是排队或 DLL 执行时间。五场景本身不提供 100 件内存趋势，也不代表生产 S7 硬件通过。

## 工具自检

~~~powershell
node --check tests/native/p0-cyclehost-performance.mjs
node --check scripts/p0-cyclehost-report.mjs
node --test tests/native/p0-cyclehost-performance.test.mjs
~~~

自检只验证证据处理规则；所有测试数据均为显式构造，不会连接 CDP、启动应用或加载 DLL。

