# P0 步 7：真实 CycleHost 性能与演示验收

当前为工具准备稿。脚本语法、证据自检通过不代表完成 100 件原生验收；实际结果以新的报告和逐件 JSONL 为准。

## 实际运行层

p0-cyclehost-performance.mjs 连接一个已经启动、配置完成的专用 P0 Tauri 实例。通过现有 UI 选择已发布配方、normal/gap 工况并逐件点击“运行一件”，读取真实 CycleHost 测量和历史记录。脚本不修改系统设置、相机、配方、发布包或检测限值。

每个组合独立运行至少 100 件：single/normal、single/gap、tricam/normal、tricam/gap。每件四个拍照点；single 为 cam1/v1 四点，tricam 为 cam1/v1→v2→v3→v1。不同组合必须使用相应设备配置和完成正常示教、试测、验证、发布的真实配方。不得直接把 Rust Prepared 基准产生的包当作已完成桌面发布流程。

运行前：

- 使用步 7 构建，确保 cycle_part_data 返回 ms、queueMs、engineMs、coreMs；DLL 和全部目录在 C 盘。
- 专用应用标识包含 com.xyzrobotics.tujiaovision.p0-tests；PLC 协议为 simulator、在线；全部参与设备为触发模拟相机；启用真实 LyFlow 测量及“全部”录制。默认 --source sim 保留上述模拟相机守卫；显式回放控制见下节。
- 保持 armMs=200。初始状态为 IDLE，模拟器未运行。脚本不会自动复位故障。
- 录制保留数至少 100；建议专用验收实例设为 500 件或以上，以便四个组合结束后仍可复核所有原图。容量建议至少 8 GiB，并确认 C 盘实际可用空间。单个 tricam/100 件约产生 1.2 GiB 原图；single 约 0.4 GiB。
- --fixture 传入该发布包的 recipe.json；--release 传入包含 manifest.json 的该包目录。脚本校验清单 FNV 等于每件运行的 bundleId，并核对配方、拍照点、视角和 1280×1024 尺寸。
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

## 显式回放性能控制

--source 默认为 sim，不会因相机配置而自动放宽。只有显式传 --source replay --replay-dir <绝对C盘目录> 才启用独立回放控制；其它参数、四组合各100件、armMs=200、真实DLL、PLC simulator 和严格 normal/gap 判定保持一致。

回放控制必须使用 cam1、replayChannel=1、triggered 采集；viewCount 按 single=1、tricam=3。camera.replayDir 必须与 --replay-dir 解析为同一 C 盘目录，运行中相机配置不能变化。每个输入目录仅含四组明确命名的全分辨率 PGM：

- single：cam1_1_v1.pgm、cam1_2_v1.pgm、cam1_3_v1.pgm、cam1_4_v1.pgm。
- tricam：cam1_1_v1.pgm 至 cam1_4_v3.pgm，每组视角1、2、3都齐全。
- provenance.json 等非图像文件允许存在，也纳入完整输入目录树的运行前后 SHA。额外图像、缺视角、符号链接、错误命名或尺寸都会拒绝。

可将独立 CLEAN Prepared 像素夹具的 normal/partial_gap PGM 按字节复制到新的 normal/gap 输入目录；provenance.json 应保存每幅 source/target 路径、SHA 和夹具来源。Prepared 的 DLL通过报告只是像素来源证据，不是此处的桌面发布或运行证据。仍须在真实专用桌面上对新的独立候选按 k0→k3 依次取样、示教、真实试测、导入样本、验证和发布；--fixture/--release 指向桌面实际发布的包。

回放是循环读四组文件，必须确保开始时指向第一组。需要时在UI重新加载回放目录，然后重新核对候选示教/发布状态。工具不会修改配置、跳过示教或偷偷调整回放游标；若取到错误组会通过逐点图像校验明确失败。

在上节参数数组中加入：

~~~powershell
'--source', 'replay',
'--replay-dir', $taskReplayInput
~~~

每件四次真实 CycleHost 触发后，全部录制原图的整文件SHA及纯像素SHA必须分别等于输入的相同 k/view。报告包含 replayInputs、replayInputsAfter、逐幅 replayComparisons，以及每件真实 part.json 的 SHA 与文档快照。录制元数据必须证明 counter=synthetic、manual=false、lostPackets=0，cycleId/SN/发布包/设备/拍照点/所选视角/会话/设备内序号/帧计数/触发计数一致；不能用硬件计数或示教手动采图冒充此次运行。独立校验器重新读输入树、输出原图和 part.json，重算哈希及身份。

--scenario normal/gap 在回放控制中只指定期望标签。实际像素完全由 --replay-dir 决定，选择 gap 按钮不会把 normal 回放图变成断胶图。操作者必须切换到独立的 gap 输入目录并按真实配置流程应用。回放不会注入随机 Pose、丢帧或 locateFail，不能把此控制当作默认带噪模拟、Robot五场景、现场相机或算法准确率通过。原默认带噪 P0-09 normal实际2失败仍单独保留。报告显式写 source=replay 和范围说明，physicalValidation/s7HardwareValidation 保持 false。

## 指标和通过条件

每件需完成 ACK、原图落盘及握手释放，核对 cycleId、SN、recipeRevision、bundleId、shotId、设备、视角、设备内序号、测量点归属及原图完整性。原图必须是 1280×1024 Gray8 PGM；记录逐幅 SHA256。软件 exe、DLL、夹具和整个冻结发布目录在运行前后核对 SHA256。

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

cyclehost-report.json 和 parts.jsonl 记录完整证据。中途失败保留原有JSONL及 error，completed=false；只有工件通过结构/身份校验并成功追加JSONL后才推进 completedParts。失败工件不计入该件数，严格判定不符仍按前述 accuracyFailures 单独报告。

失败时另存 failed-attempt.json，报告 failedAttempt.artifact 引用文件路径、字节数和 SHA256。该文件保留 attempt 的件序号、阶段、accepted 状态、已观测工件数据，以及最后成功读取的轮询状态、detail、measurements、原图列表；异常时尽力补读已观测原图的 SHA 和 part.json 并完整保留，无法读取的项记入 evidenceReadErrors，不掩盖原始异常。超时保留最后轮询状态；若故障发生在该件已追加JSONL后的内存采样或最终检查阶段，accepted=true，已有 completedParts 不回退，但整个报告仍为未完成。写失败快照本身失败时保留 failedAttemptPersistenceError，不覆盖最初异常。独立报告工具重算全部指标、逐件检查身份并重新校验文件哈希；任何伪造分位值、缺采样、错 cycleId 原图、丢视角、未知指标或篡改文件均不能通过。保存原图目录及所有引用文件后再做最终复核，避免保留策略删去较早组合原图。

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

自检覆盖默认模拟守卫、显式回放输入树、逐幅像素同源、Synthetic元数据和独立100件报告校验；所有测试数据均为显式构造，不会连接 CDP、启动应用或加载 DLL。

