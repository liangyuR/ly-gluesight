# 测试记录与复现

> 本文写怎么跑测试、测试覆盖了什么，以及 P0 各步验证结论的汇总。逐轮的详细结果、日志路径和审查记录在 git 历史与对应 PR 里。业务资源不做内容哈希匹配（见仓库 AGENTS.md），旧记录里的摘要校验与篡改门禁不再适用。

> 2026-10-09：随动（follow）检测工况已整体移除，一期只做硬触发飞拍；随动标定、随动配方、相关测试与文档条目同步删除。

项目使用 Vitest、React Testing Library、user-event 和 jsdom。测试操作真实的 React 页面与表单；需要桌面数据的页面使用受控接口返回值，避免连接现场相机、PLC 或改写生产配置。工作台状态测试使用真实 WorkspaceProvider，异步竞态通过可手动完成的 Promise 验证。

## 运行

首次安装依赖：`pnpm install --frozen-lockfile`。

| 命令 | 用途 |
| --- | --- |
| `pnpm test` | 一次运行全部前端测试 |
| `pnpm test:watch` | 开发时持续运行相关测试 |
| `pnpm test:coverage` | 全部测试、覆盖率报告与覆盖率门槛检查 |
| `pnpm typecheck` | 同时检查应用和测试的 TypeScript 类型 |
| `pnpm build` | 验证生产前端构建 |

单独运行：`pnpm exec vitest run tests/teaching.test.tsx`。

HTML 覆盖率报告输出到 `coverage/index.html`，机器可读摘要为 `coverage/coverage-summary.json`。报告统计整个 `src`，包括未被测试执行的文件；只排除启动入口、类型声明和统一导出文件。测试代码和测试夹具不计入覆盖率。

覆盖率门槛：语句 85%、分支 80%、函数 80%、行 85%。工作台状态另要求行 90% / 分支 70%，流程状态模型另要求行 80% / 分支 70%，工件总览另要求行 90% / 分支 80%。测试文件以 2 个工作进程运行，避免桌面机器一次启动过多测试进程。

本机受限环境里默认 fork 测试进程可能停滞，或依赖链接解析异常。这时可直接运行已有依赖：`node node_modules/vitest/vitest.mjs run --coverage --pool=threads --maxWorkers=2`、`node node_modules/typescript/bin/tsc -b`、`node node_modules/typescript/bin/tsc --noEmit -p tsconfig.test.json`、`node node_modules/vite/bin/vite.js build`。Windows Codex 受限环境若遇到原生 realpath 的 EPERM，按项目的 Windows 命令环境指引加载兼容脚本；仍不可读时在获准的主机环境运行相同测试，不要改应用代码或关闭失败检查来掩盖环境错误。

### Rust 后端

在 `src-tauri` 下运行 `cargo test --offline --locked --lib`。`--offline` 要求依赖已缓存，首次准备依赖时需先允许 Cargo 获取。S7 外部进程用例和需要真实 DLL 的用例默认标为 ignored，普通 `cargo test` 不代表已完成这两项，分别见 [S7 一期握手](#s7-一期握手) 与 [真实 DLL 回归](#真实-dll-回归)。

`cargo fmt --all -- --check` 不通过：全库有大量与默认 rustfmt 不一致的排版，没有做全库重排，不能记作格式检查通过。

### 真实 DLL 回归

客户端固定到 lyFlow 主线 `5b796c3`（Image ABI v15），通过 `RunSpec.image_inputs` 注入完整 u8 灰度帧。运行库必须包含 `io.load_image`、`image.board_calib`、`image.load_calib`、`glue.taught_path`、`glue.bead_width`（LyFlow `feat/glue-taught-path` 分支，在 lyFlow 仓库设置 `LYFLOW_PACKS=glue` 后构建 core）；缺算子时加载明确拒绝，安装包自检 `scripts/Test-PackagedRuntime.ps1` 用同一份算子清单。系统设置填 DLL 绝对路径。

真实 DLL 用例名都以 `native_` 开头。设置 `LYFLOW_CORE_DLL` 后运行：

```powershell
cargo test --offline --locked --lib native_ -- --ignored --test-threads=1 --nocapture
```

- `GLUESIGHT_VISION_TEST_DIR`：保存示教胶路用例的夹具、测量图与逐站结果，缺省写系统临时目录。
- `GLUESIGHT_P0_REPORT_DIR`：全分辨率冻结包回归 `native_full_resolution_frozen_bundle_single_and_tricam_regression` 的报告目录，必须是新目录；每种模式至少 100 件（`GLUESIGHT_P0_PARTS`）、每件四次测量。报告用 `python scripts/p0-regression-report.py <report.json>` 核验。
- `GLUESIGHT_P0_FIXTURE_STYLE`：缺省 `simulated_metal`（带纹理与噪声）；`clean_step_edge` 为洁净控制夹具，见 [故障矩阵](p0-step7-fault-matrix.md#夹具)。

性能类运行时不要并行跑 Cargo、前端覆盖率或其他高负载任务。

### 模拟服务

独立 Robot / PLC 模拟服务的配置、启动和升级回归见 [模拟服务说明](../../scripts/robot-plc-demo/README.md)。`pnpm sim:test` 运行隔离的协议测试；`pnpm sim:verify` 对运行中的专用桌面实例执行五工况联调。演示使用开发 Modbus，不代表 S7 生产验收。

## S7 一期握手

协议、点表、PLC 参考程序和现场步骤见 [S7 一期交付说明](../integration/plc-s7-phase1.md)。线协议测试服务见 [S7 测试 PLC](../../scripts/s7-handshake/README.md)，使用真实 TPKT/COTP/S7 报文，仅监听本机回环地址。

- Python 报文与故障注入：`python -m unittest discover -s scripts/s7-handshake/tests -v`。
- Rust 契约和计划：在 `src-tauri` 下运行 `cargo test --offline --lib handshake` 与 `cargo test --offline --lib plc_plan`。
- Rust 会话与生产 S7 驱动联调：在 `src-tauri` 下运行 `cargo test --offline --locked --lib plc_session -- --include-ignored --test-threads=1 --nocapture`，需要 Python 3 和本机 TCP 权限，不会连接生产 PLC；可用 `S7_TEST_PYTHON` 指定 Python 可执行文件。Windows 沙箱拒绝 localhost 连接（WinError 10013）时，在获准主机环境运行，不能跳过 TCP 后记为通过。
- 前端：`pnpm exec vitest run tests/plc-page.test.tsx tests/plc-editors.test.tsx tests/plc-dialogs.test.tsx tests/connection-form.test.tsx tests/plc-operation-lock.test.tsx tests/plc-s7-template.test.tsx tests/plc-recipe-plan.test.tsx`。

会话专项日志在 `src-tauri/target/s7-session-tests/verification.log`，同目录各案例保存 `final-status.json`、`final-wire.json`、事务/审计文件与 DB 快照；这些本机产物不随源码提交。

软件验证不替代 TIA Portal 编译、实际 CPU 通讯、三目设备触发身份及现场长稳验收。

## 真实桌面 UI 验证

`scripts/Start-UiTestInstance.ps1` 构建并启动隐藏的隔离测试实例，相机、PLC 配置和历史目录与生产实例独立：

- 默认 `-Profile ui`：`tests/native/tauri-ui-test.json`，标识 `com.xyzrobotics.tujiaovision.ui-tests`，输出 `output/playwright/ui-regression`。
- `-Profile p0`：`tests/native/tauri-p0-test.json`，标识 `com.xyzrobotics.tujiaovision.p0-tests`，输出 `output/playwright/p0-regression`。

默认调试端口 9337，进程信息写在输出目录的 `instance.json`。`-SkipBuild` 复用已有测试程序，只核对 `instance.json` 里的实例标识与程序路径一致，不一致时要求重新构建。

通过 Playwright CLI 连接该实例后，先执行 `tests/native/guard.js` 只读确认目录和模拟器，再按场景前置顺序运行 `tests/native/*-ui.js`。后台命令只用于只读断言；目录选择、参数更改、保存和写入都经过真实 UI。P0 流程用 `p0-*-ui.js`（建候选、示教、验证发布、在线、局部失效、实时图像显示、历史与原包重现等）；旧名义胶路模型的 `flyshot-*-ui.js` 没有改写，不再适用于当前配方。

- 复用前先检查脚本里的本机路径：部分脚本固定写了本机的样本、回放目录、DLL 与输出路径（含旧的 D: 路径），需改成本机路径。离线原图和 DLL 不随仓库提供。
- ACK 进程恢复专项 `p0-ack-process-recovery.mjs` 要求先关闭现存隔离实例，传入 `--executable`、`--instance`、`--appdata`、新的 `--output` 目录、后端导出的 `--template`、`--playwright-module` 与 `--fixture scripts/s7-handshake/s7_test_plc.py`；结束后还原 PLC 配置并归档本轮握手证据。
- 实际 CycleHost 性能脚本的准备与参数见 `tests/native/p0-cyclehost-performance.md`。
- WebView2 直接写入系统下载目录时可能不发 CDP download 事件，`plc-log-export-ui.js` 只返回时间范围，运行后需核实新文件才能计为通过。

仅检出源码或运行 `pnpm test` 不会自动完成这些原生 UI 测试。

## 已覆盖的逻辑

| 测试文件 | 关键行为 |
| --- | --- |
| `workspace-context.test.tsx` | 配方恢复、快速切换、后台刷新竞态、草稿保护、保存修订、重复操作、防抖预览、事件清理、相机 / 标定 / 系统设置变更后的延后刷新 |
| `teaching.test.tsx` | 指定帧、图像绑定、参数变更失效、保存与下一帧、生产期间取样限制 |
| `validation.test.tsx` | 修订和样本期望失效、发布确认（含计划版本是否要 PLC 同步）、失败保留弹窗、历史来源、原图样本导入前置条件 |
| `library.test.tsx` | 生产与候选去重、筛选、复制编号与产品代码、新建与删除确认、失败恢复 |
| `recipe-editor.test.tsx` | 候选与生产保存、逐拍照点编辑（编号、Pose、相机、视角、胶条、不检、标定引用）、换视角清空中线、检测参数与限值覆盖（含最低有胶比例）、旧预览晚到 |
| `geometry-page.test.tsx` | 真实编辑器与工作区的草稿/保存连接、候选切换与示教入口、工作中限制、飞拍可行性取拍照点相机曝光、不检与示教比例 |
| `history.test.tsx` | 日期与结果筛选、分页复位、导出范围、空结果、查询返回顺序与失败恢复 |
| `history-detail.test.tsx` | 原包重现、三目按保存视角查看原图、缺帧 / 测量出错 / PLC 确认 / 录制失败展示、规则与原图复测、原始记录保留、历史帧用于示教 |
| `inspect.test.tsx` | 各生产阶段的切型限制、切换确认、故障复位、新件不显示旧结论 |
| `settings.test.tsx` | 型号来源切换、两个设置分区保存互不覆盖、失败重试 |
| `camera-config.test.tsx` | 采集模式、触发沿、后台固定序列号与未保存编辑、参数警告 |
| `camera-page.test.tsx`、`camera-preview.test.tsx`、`frame-preview.test.tsx` | 相机增删与候选/生产引用保护、源切换、软触发/回放下一张、刷新失败重试、配置变化与连续预览作用域、历史帧/叠加显示 |
| `offline-image-import.test.tsx` | 离线原图的格式/大小、二进制、重复操作、读取竞态、工位/修订/帧绑定、错误恢复 |
| `overview.test.tsx` | 选帧、方向键及边界、自动布置、背景导入/移除/读取竞态、显示与物理规划分离 |
| `calibration.test.tsx` | 冻结样本前置条件、标定尺寸加载、图像与工位绑定、失败保留原标定 |
| `dry-run.test.tsx` | 当前相机帧计数、触发计数、跳号、最短间隔、启动失败 |
| `feasibility.test.tsx` | 实际参数更新、运动模糊/触发间隔/视野重叠、无效输入不得显示可行 |
| `sim-controls.test.tsx` | 工况切换、单件/连续、连接前置条件、重复启动、停止失败、配方删除 |
| `plc-page.test.tsx` | 加载恢复、还原/保存/连接、重复操作、写入门槛、心跳点删除和地址表替换 |
| `plc-logs.test.tsx` | 组合筛选、搜索分页、时间段校验、查询竞态、趋势、跟随清理、全部页 CSV 导出 |
| `plc-dialogs.test.tsx` | 数值与布尔写入、非有限数值、等待期间重复回车、焦点循环与恢复、Esc 与遮罩 |
| `plc-editors.test.tsx` | 协议端口、S7 预设、点位必填、ID 与标签去重、地址检查、JSON 导入 |
| `connection-form.test.tsx` | 完整十六进制/数字格式、端口/TSAP/MC 范围、空值不变零、S7 1200/1500 相同参数仍保留所选型号、协议切换 |
| `image-viewer.test.tsx` | 原图加载状态、缩放边界、叠加开关、换图重置、历史帧和过期读取 |
| `visual-status.test.ts` | 工件与配方快照隔离、胶路颜色、缺帧、重复测量不回退、最终 NG 状态 |
| `cycle-visuals.test.tsx` | 逐拍照点视图、帧选择、示教中线与站状态着色、段内断口位置、曲线公差与超差 |
| `cycle-isolation.test.tsx` | 在线周期按 cycleId 隔离：同 SN 新周期清除旧测量，旧周期事件、快照和迟到请求不覆盖当前周期 |
| `rejudge-retest.test.tsx` | 批量试算参数、原图复测入口、选件与筛选、缺图规则、重复操作、旧结果及错误隔离 |
| `workflow-model.test.ts` | 示教与验证失效、历史备份恢复、工件边界发布快照、运行限制、代表性样本 |
| `workflow-preview.test.tsx` | 所有流程页面的直接路由、存储损坏回退、情景筛选、重置、真实示教交互 |
| `workflow-setup-operations.test.tsx`、`workflow-recipe-operations.test.tsx`、`workflow-production-operations.test.tsx`、`workflow-preview-components.test.tsx` | 预览的完整建站、多个本地候选、删除确认、示教/恢复、总览布置/背景、样本验证/边界发布、连续运行/停止、历史对照/导出、设置、缩放与叠加 |
| `navigation.test.tsx` | 默认与未知路由、历史复测路由优先级、侧栏折叠、浏览器提示、页面错误重试 |
| `sidebar.test.tsx`、`operation-guide.test.tsx` | 正式导航分组与整栏折叠、设备状态与卸载清理、全部引导链接和流程前置条件 |

## 已修复并有回归测试的问题

以下为 2026-10-09 前修复的前端问题；第 15、16 条涉及的卡尺与模板拖动已随旧模型删除。

1. PLC 写入输入允许 Infinity / -Infinity。
2. PLC 写入等待期间，回车绕过禁用按钮重复发起写入。
3. 工作台后台刷新晚到，覆盖新选择的配方或刚开始编辑的草稿。
4. 历史查询晚到，覆盖新筛选条件的结果或显示旧查询的错误。
5. 配方编辑的旧预览晚到，清除当前配置错误并错误恢复保存按钮。
6. PLC 保存/连接和模拟节拍启动缺少等待期间保护；地址表替换遗留心跳引用；未保存点位仍显示旧读数。
7. 通讯日志的旧查询覆盖新筛选，导出错误没有反馈。
8. 空跑在丢包或最短间隔不满足单帧时间时仍可能显示通过。
9. 总览背景读取晚到覆盖新配方；可行性计算把零值输入显示为满足。
10. 保存旧配方的请求晚到会在新配方显示成功或错误；保存成功后列表刷新失败错误撤销下一步资格。
11. 取新样本、导入整组原图、示教与标定在读取中换帧或切换候选，旧结束回调解除新请求等待。
12. 成功绑定历史帧或保存后更新修订，会阻止正常进入示教或下一帧；恢复原始示教仍显示旧参数草稿。
13. S7 1200/1500 共用连接参数时选择项回退；TSAP 非法值被 JSON 序列化成 null，误判为没有修改。
14. JSON 地址表接受无效枚举/类型、空 ID 和重复项；点位旧地址检查晚到会误标当前地址有效。
15. 已完成工件的模拟缺口过短，经过卡尺平均只剩一个缺胶点。
16. 首次拖模板插入参数提示后画布位置变化，拖动使用变化后的坐标导致模板偏移。
17. 浏览器预览新建/复制覆盖原工作台、缺少删除确认、PLC 点位换页丢失、连续运行只执行一件；修复本地保存和等待/停止语义。
18. 数字输入清空后变成零，无法正常逐字输入负角度和负平移；空值保持无效并阻止提交，预览绘图使用有限占位坐标。

## 测试边界与后续缺口

jsdom 没有真实布局、灰度图解码与设备连接能力。测试环境为 ResizeObserver、scrollTo 和原生 dialog 提供最小替代实现；这些测试不会证明窗口布局、SVG 拖动坐标、原生弹窗行为、WebView 像素输出或硬件通讯正确。

基本操作、分层证据和边界逐项记录在 [UI 操作验收](UI_OPERATIONS.md)。界面测试覆盖用户动作和接口参数，原生测试覆盖像素与持久化，隔离桌面脚本覆盖完整示教/验证/发布/检测/复盘。实际设备通讯、现场节拍和实物缺陷准确率需要设备与现场验收。

新增业务测试应同时检查用户可见结果和关键接口参数，包含前置条件不满足、请求失败、重复操作或旧请求晚返回的路径。避免仅检查静态文本或给截图数量作为逻辑覆盖率。

## P0 验证汇总

各步范围与完成标准见 [P0 计划](../architecture/p0-plan.md)。下表只留仍有参考意义的数字；各分支当时的测试项数、日志路径和首次失败记录见 git 历史。除特别注明外，都是模拟相机 / 模拟 PLC 上的软件验证，不代表现场准确率或现场节拍。

| 阶段 | 验证了什么 | 关键数字 |
| --- | --- | --- |
| S7 一期握手（2026-10-09） | 真实 `ly_plc::PlcEngine` 与本机回环 S7 服务联调：同 SN 不同序号、计划拒收、早 / 迟 ACK、写拒绝、执行后断线、释放失败、重启与腐坏日志、慢落盘、停止心跳、校验后重连 | 17 项线协议场景 + 1 项事务文件恢复；Python 报文与故障注入 21 项 |
| 旧名义胶路模型（2026-10-09 前） | 原生 UI 的回放、离线导入、标定、模板示教、发布、检测、历史复测与日志导出 | 模型已在步 1b 删除，结果不再适用 |
| 步 0 随动移除 | 飞拍模拟闭环、S7 握手、计划导出与移除前基线一致 | — |
| 步 T 三目软件基础 | 视角越界 / 缺失拒绝；单设备四点 1→2→3→1 的计数与计划；同次冻结与局部示教失效；严格回放分组；录制按拍照点还原所选视角 | 单设备四拍照点 `cameraShots = [4, 0, 0]` |
| 步 3 节拍与 cycleId | 三设备四拍照点交错到达的 OK / NG / ERR；首帧缺失、重复帧、提前 End、截止后帧与结果；同 SN 旧 cycle 结果；错误拍照点 / 相机 / 发布包身份；工作线程卡住与迟到返回 | — |
| 步 4 发布包与真实测量 | 真实 DLL：暗胶、亮胶、缺口、偏移、反向中线、毫米换算；冻结包覆盖三设备四拍照点、不同分辨率，发布后改外部标定不影响读数，缺失或结构不符拒绝；慢 End 落盘不延长截止（ERR 91） | 冷预热 215 ms、稳定单帧 1 ms（本机小型合成图） |
| 步 5 工作台全流程 | 隔离桌面：一台模拟三目、真实 DLL，四点 1→2→3→1 绘制中线、试测、样本验证、发布、在线 normal / gap；视角、Pose、标定引用、相机绑定的局部失效；新增相机无需刷新 | T_arm=200 ms 下布防 102 / 108 ms；每件 4 帧、307 测点；gap 8 mm > 6 mm 判 NG_GAP / PLC 13；normal 实际 `OK_WITH_EXCURSION`（P0-09）；C: 重建后四帧 × 1440×900 / 1280×800 显示通过 |
| 步 6 历史与 ACK 恢复 | 原生历史逐拍照点显示两件全部 24 个视角；CSV、原包重现、候选重判与原图复测；ACK 已写入握手主日志、SQLite 未更新时终止进程，重启后恢复为 Acknowledged，原结论不变 | normal / gap 布防 112 / 143 ms；每件 12 张原图 |
| 步 7 Prepared 洁净控制 | `clean_step_edge` 夹具（1280×1024），真实 DLL、冻结包串行测量 | 单目 / 三目各 100 件、400 次测量；Prepared P95 11.603 / 11.855 ms；串行四次 P95 44.195 / 44.247 ms（不是 PLC 收尾延迟）。默认带噪夹具首件严格 OK 失败（P0-09） |
| 步 7 实际 CycleHost | 单目 / 三目 × normal / gap 四组，开发 PLC 模拟器 + 确定性回放；正常件全部 OK / 1、断胶件全部 NG / 13，原图逐幅匹配输入 | 每组 100 件，共 1600 次测量、3200 张原图；arm P95：三目 normal 66、三目 gap 75、单目 normal 76、单目 gap 75 ms，最大 112 ms（T_arm=200 ms）；frame P95 25–27 ms（含排队）；partEnd→PLC 提交 P95 31 ms。去哈希前的程序（运行时 `6a110a6`），之后没有重跑 |
| 去哈希（PR #14） | 全新三视角 profile 的示教、验证、发布；normal / gap 各一件；两件原包及候选四次比较、24 个视图；同 profile 第二实例被拒；重启后原记录与 24 张原图恢复 | normal / gap 严格 `OK` / `NG_GAP`，布防 20 / 23 ms；每件 12 张原图 |

原 `docs/testing/evidence/` 下的 18 个机器生成的运行记录（JSON，对应已被取代的提交）已移出目录树，仍可在提交 cf1b25d 查看：`git show cf1b25d:docs/testing/evidence/<文件名>`。真实 DLL 回归设置 `GLUESIGHT_P0_REPORT_DIR` 后会重新生成报告（见 [真实 DLL 回归](#真实-dll-回归)）。

## PR #17 集成验证（历史基线）

PR #17 已合入 main，PR #11–#14 已由其覆盖并关闭。该集成在 PR #8–#14 之上改了判定规则、PLC 计划版本、引擎身份并删除旧数据兼容（见 [P0 计划](../architecture/p0-plan.md)）。

| 验证 | 命令 | 结果 |
| --- | --- | --- |
| Rust 默认回归 | `cargo test --offline --locked --lib` | 339 项通过、0 失败、35 项默认忽略（需真实 DLL 或回环夹具） |
| S7 回环专项 | `cargo test --offline --locked --lib plc_session -- --include-ignored --test-threads=1` | 41 项通过（含 29 项线协议场景），57.76 秒 |
| 前端全量 | `pnpm test` | 42 个文件、965 项通过 |
| 类型检查与构建 | `pnpm typecheck`、`pnpm build` | 通过 |
| 真实 DLL 回归 | `cargo test --offline --locked --lib native_ -- --ignored --test-threads=1` | PR #17 当时未运行；C: 现有 DLL 缺少胶路算子，专项尝试在加载检查被拒绝；负责人明确本轮不做算法或真实 DLL 回归。整拍照点无胶的 NG_GAP 仍待后续真实 DLL 确认 |

S7 运行时修复随本次集成一并验证：空闲时 PC 输出被清零（PLC 重启 / DB 重新初始化）自动重写、60 s 内反复被改写判故障、PLC 残留 partEnd / resultAck 只等待、设备未就绪撤下 visionReady；缺帧 / 多帧原因带相机与应收 / 实收计数；提前判 ERR 的件按 PLC 确认时的已发触发数推下一件基线；SDK 帧长度不足丢图不越界；PLC 字段超出 UInt 范围报错不截断；旧 `planHash` 点名直接拒绝。


## PR #15 更新与回归范围

PR #15 已更新到 main `451be27`，本轮业务回归通过，等待审查；PR #16 以 #15 为 base 并已纳入最新 main，本轮业务回归通过，等待审查。上表是 PR #17 的历史基线，不能作为本轮通过数字。

- 审计事件先进入持久 `audit-spool`，最终检测记录耐久接受后才允许 PLC DONE；录制终态和数据库入库完成前拒绝下一件布防。
- 启动先重放 spool，再处理仍中断的 Pending 录制和未引用临时目录；受 spool 引用的目录保留。重放身份采用 cycleId 与实际业务字段，冲突保留全部证据并闭锁。
- 验证数据库瞬态失败重试、spool 写入或读取失败、损坏文件、同 cycleId 内容冲突、目录 create / rename 探针、worker 异常与录制失败；故障不得输出错误 OK 或被普通复位绕过。
- S7 验证首次有效 ACK 只交付一次，录制 / 入库屏障存在时等待，解除后自动释放；等待期间心跳、输入变化与断线故障仍按协议处理。
- 执行 Rust 默认、S7 回环、前端全量、类型检查与构建；按负责人要求，本轮不做算法或真实 DLL 回归。旧 PR #15 的 `docs/testing/evidence/` 记录只证明当时提交，不能替代这次 main 整合回归。

四组 400 件性能复跑、默认带噪夹具的 P0-09、W0 / W7 与现场检测准确率仍待完成。

| PR #15 本轮验证 | 结果 |
| --- | --- |
| Rust 默认 `cargo test --offline --locked --manifest-path src-tauri/Cargo.toml --lib` | 374 通过、0 失败、41 默认忽略 |
| S7 会话 `... --lib plc_session -- --include-ignored --test-threads=1` | 46 通过、0 失败；含审计完成但设备离线、设备恢复、Acquire/Drain 设备故障与首次 ACK 只落一次 |
| 前端 `node node_modules/vitest/vitest.mjs run --coverage` | 42 文件、965 通过；语句 / 分支 / 函数 / 行 91.34% / 89.81% / 88.59% / 93.93%，全部门槛通过 |
| 两组 TypeScript 与 Vite 生产构建 | 通过；保留既有大 chunk 提示 |
| CycleHost 准备与性能工具 Node 测试 | 57 通过 |
| Robot Python / Node、S7 Python、报告验证器 self-test | 39 / 17 / 21 / 10 通过 |

冲突解决保留 main 的 D-0、计划版本、判定规则及 ACK 实际触发数基线，保留 PR #15 的目录健康探针和首次 ACK 耐久接受。新增审计等待恢复复用空闲设备检查，避免相机离线时短暂置高 Ready。S7 首轮 44 通过 / 1 失败来自旧测试要求 ACK 收尾设备离线即 Fault；按 main 规则改为离线正常释放、Ready 保持低，并额外验证 Acquire / Drain 的设备故障仍闭锁，最终 46 项全部通过。

本机完整日志位于 `output/pr15-main-regression/`（不提交）：保留沙箱路径读取 / 依赖解析失败、前端停滞尝试、首次 S7 失败与最终通过日志。现有 C: DLL 缺少 `glue.taught_path` / `glue.bead_width`，首次专项 6 项在加载检查失败；负责人随后明确本轮不做算法或真实 DLL 回归。没有新原生桌面、400 件、Robot 五工况或现场硬件通过结论。


## PR #16 更新与回归范围

PR #16 以 PR #15 为 base，包含最新 main 与 #15 的持久审计及闭锁修复。上节数字仅来自 #15 的回归；#16 在本次整合源码上独立完成下表业务回归。按负责人明确要求，本轮不做算法或真实 DLL 回归。

- 批量原图改用严格 base64 字符串，解码前检查编码长度；保留单图 15 MB、整组 80 MB、拍照点完整且唯一、图像格式 / 结构 / 尺寸及整组原子导入限制。验证坏编码、非字符串、空图、超限、真实 PGM 像素往返与失败不留下部分样本。
- 默认构建不启用 `p0-pressure-test`；启用后也必须为 `com.xyzrobotics.tujiaovision.p0-tests.pressure` 调试实例。默认回归与 feature 回归分别执行，生产标识和 release 实例必须拒绝压力入口。
- `ArmBudget` 验证多个短阶段不能重置总截止；入口到 PLC 布防确认共用 `T_arm=200 ms`。串行 PLC 写入等待副作用完成，再拒绝超过截止的成功，核验错误路径撤销输出 / 故障处理；阶段日志包含 `budgetMs`、`elapsedMs`、阶段累计及区间耗时。
- 业务验证包含 Rust 默认与压力 feature、S7 回环、前端全量 / 覆盖率、类型检查、构建及 Node / Python 工具回归。压力桌面脚本、400 件性能和 Robot 五工况必须以实际执行证据另行记录，不能由工具单测推定通过。

`p0-current400-arm-failure-c.json` 等 PR #16 历史证据保留当时第 53 件 arm 863 ms、400 件未通过的事实，不代表当前 ArmBudget 修复后的结果。P0-09、真实 DLL、W0 / W7 和现场准确率仍未验收。


| PR #16 本轮验证 | 结果 |
| --- | --- |
| Rust 默认 `cargo test --offline --locked --manifest-path src-tauri/Cargo.toml --lib` | 385 通过、0 失败、42 默认忽略 |
| 压力 feature `... --lib --features p0-pressure-test` | 387 通过、0 失败、42 默认忽略；含生产 / release 标识拒绝和过期门控恢复 |
| 完整 S7 会话 `... --lib plc_session -- --include-ignored --test-threads=1` | 47 通过、0 失败；包含布防截止等待原始日志副作用收尾、故障日志不会被迟到成功覆盖 |
| 前端 `node node_modules/vitest/vitest.mjs run --coverage` | 42 文件、969 通过；语句 / 分支 / 函数 / 行 91.34% / 89.78% / 88.59% / 93.96%，全部门槛通过 |
| 两组 TypeScript 与 Vite 生产构建 | 通过；保留既有大 chunk 提示 |
| CycleHost 准备与性能工具 Node 测试 | 125 通过 |
| Robot Python / Node、S7 Python、报告验证器 self-test | 39 / 17 / 21 / 10 通过 |
| 新增原生工具脚本语法 | current-cyclehost-suite、cyclehost-queue-pressure、robot-current-setup 的 node --check 通过 |

本轮顺序整合 main `451be27` 与 PR #15 `373b1a0`，保留 #15 的目录健康探针、单次 ACK、审计等待与设备状态联动、DONE 前持久记录和 main 的 D-0 / planVersion / 实际触发数规则。`arming::tests` 验证同步与异步阶段共用截止、健康探针超时不会继续布防、PLC 串行副作用先收尾再拒绝迟到成功。前端包含 40 张 1280×1024 PGM 顺序读取 / base64 完整像素传输，后端含真实文件写入与失败原子清理回归。

本机完整日志位于 `output/pr16-main-regression/`（不提交）。本轮没有运行实际桌面队列压力、400 件性能、Robot 五工况或现场硬件；以上 feature / 工具单测不能代替这些执行。算法和真实 DLL 回归按负责人明确要求不纳入本轮。

## 配方创建软件交付（2026-10-11）

基线为 main `0370742`，交付分支 `codex/recipe-capture-workflow`。本轮实现空草稿、整圈拼接采集、按点多图示教、独立整圈验证与冻结发布；详见 [业务设计](../architecture/recipe-creation-workflow.md) 和 [交付进度](../architecture/recipe-creation-progress.md)。按负责人要求，无需现场验收、忽略算法，本轮不执行算法质量或原生 DLL 专项。

| 验证 | 最终结果与证据 |
| --- | --- |
| Rust `cargo test --offline --manifest-path src-tauri/Cargo.toml --lib` | **417 通过、0 失败、44 条件性忽略**；`tmp/recipe-backend-final-host.log` |
| S7 线协议回环 `s7_wire_`（`--ignored --test-threads=1`） | **35 通过、0 失败、0 忽略**，73.63 s；含 20 次单设备教学采集身份、结果序号、ACK 与释放。`tmp/recipe-s7-wire-tests.log` |
| `pnpm test -- --pool=threads --maxWorkers=2` | **43 文件、990 测试通过，0 失败/忽略**，119.55 s；`tmp/frontend-reports/recipe-delivery-final.json`、同名 `.log` |
| `pnpm typecheck`、`pnpm build` | 通过；1977 modules；保留既有大 chunk 提示。`tmp/frontend-reports/recipe-delivery-{typecheck,build}.log` |
| Robot 演示 Python / Node | **40 / 18 通过**；包含结构版本 5、逐图标定默认引用和多图契约。`tmp/recipe-demo-python-tests.log` |
| 最终 Tauri 桌面构建 | 独立标识、默认 feature、隐藏窗口、debug 无安装包构建通过。`output/playwright/recipe-workflow/build-final.log` |
| 桌面真实 IPC / 重启恢复 | **通过**；`output/playwright/recipe-workflow/{ipc-report,restart-report}.json` |

Rust 最终新增边界包括嵌套标定比例、人工比例覆盖、标定复用追溯、多图独立状态、未改变图恢复完成状态、默认参数局部失效、无首帧基线/旧帧/跳号/重复、失败轮次全部来源的新会话要求、整圈候选禁止单帧替换、历史样本非零 PLC 计划版本一致，以及同一冻结包失败重试与不确定生效状态闭锁。S7 夹具改为按当前计划报告设备触发数，并执行全部线协议回归。

桌面检查通过实际 WebView2→Tauri command→磁盘保存/恢复执行：新建空草稿；采用生成的 20 点完整采集记录，每点三幅实际 PNG；分别选用 1/2/3 幅，共 39 幅独立草稿；保存标定检查输入；拒绝示教轮次自证和单帧恢复；加入第二轮独立样本；验证未完成时拒绝发布；确认不生成生产工件记录或生产版本；应用重启后恢复轮次、逐图草稿、选择和编辑位置。

这条桌面检查显式使用**已保存完整采集夹具**，没有从模拟夹具推定现场收图成功；使用 `--skip-engine` 跳过试测与成功发布，验证通过的算法样本、成功发布和工件在途切换没有新增桌面验收结论。采集异常与发布切换状态由 Rust/S7 软件测试分别覆盖，不将分层测试合称为硬件端到端验收。

桌面工具为 `tests/native/recipe-workflow-ipc.mjs`，配置为 `tests/native/tauri-recipe-workflow.json`。复现步骤：

1. 运行 `pnpm build` 和 `pnpm tauri build --debug --no-bundle --config tests/native/tauri-recipe-workflow.json`。
2. 运行 `python tests/native/recipe-workflow-images.py output/playwright/recipe-workflow/images`（需要 Pillow）。
3. 设置当前进程 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9351`，用 `Start-Process -WindowStyle Hidden` 启动该独立构建。数据标识必须为 `com.xyzrobotics.gluesight.recipe-workflow-20261011`，不能使用生产 profile。
4. 运行 `node tests/native/recipe-workflow-ipc.mjs --playwright-module <本机playwright/index.mjs> --images output/playwright/recipe-workflow/images --output output/playwright/recipe-workflow --skip-engine`。
5. 重启同一隔离实例，重复上条命令并加 `--restart`。首次执行拒绝覆盖已存在的同名测试候选；需要重跑时先关闭此实例并备份其独立 profile。

Windows 沙箱临时目录权限及 realpath 限制会使测试启动失败；本轮以 C 盘项目、隔离 TEMP 及限定宿主执行取得最终证据。前端默认 fork 在此机器有既知停滞，最终使用线程池，没有改依赖或跳过 UI suite。临时失败日志与运行数据不进入交付提交。
