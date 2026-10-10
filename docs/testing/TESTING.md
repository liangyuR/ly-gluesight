# 前端 UI 逻辑测试

> 当前版本已按 AGENTS.md 移除业务内容哈希匹配。本文既有摘要、字节篡改门禁和旧版本验收仅为历史记录；新实现以明确 ID/版本、实际结构、路径/尺寸与设备计数为准。旧性能数据不能代表本次去哈希版本。

本次去哈希回归：Rust 317 项、S7 真实回环 24 项、真实 DLL 五项、前端 964 项全部通过，类型与生产构建通过。旧 v2 SQLite 保留原列和原记录；按实际 ID/版本迁移，缺失或歧义快照保留历史但禁止复测，旧发布目录通过独立 ID 映射读取。删除/改名/重启版本递增、历史版本下限、原图录制尺寸及 Windows 跨件 junction 拒绝均有实际回归。原生桌面使用提交 `72a7a5e` 的全新隔离 profile 完成三视角采图、示教、验证和发布；normal / gap 各一件得到严格 `OK` / `NG_GAP`，布防 20 / 23 ms，每件十二张原图完整。两件原包及候选共四次历史比较、二十四视图显示、同 profile 第二实例拒绝及重启后原记录/二十四原图恢复均通过，详见 [新桌面证据](evidence/p0-no-hash-native-c.json)。旧 400 件结果不套用到新版。详见 [后端证据](evidence/p0-no-hash-backend-c.json) 与 [前端及工具证据](evidence/p0-no-hash-frontend-c.json)。

独立 Robot / PLC 模拟服务的配置、启动和升级回归见 [模拟服务说明](../../scripts/robot-plc-demo/README.md)。`pnpm sim:test` 运行隔离的协议测试；`pnpm sim:verify` 对运行中的专用桌面实例执行五工况联调。

> 2026-10-09：随动（follow）检测工况已整体移除，一期只做三相机硬触发飞拍；随动标定、随动配方、相关测试与文档条目同步删除。

项目使用 Vitest、React Testing Library、user-event 和 jsdom。测试操作真实的 React 页面与表单；需要桌面数据的页面使用受控接口返回值，避免连接现场相机、PLC 或改写生产配置。工作台状态测试使用真实 WorkspaceProvider，异步竞态通过可手动完成的 Promise 验证。

## 2026-10-10 · P0 步 T 软件基础

范围与剩余验收见 [P0 计划的步 T](../architecture/p0-plan.md#步-t--三目设备接入d-11)。模拟与回放支持单台设备一次触发交付三个视角，配方格式为 4。

| 验证 | 本轮结果 |
| --- | --- |
| Rust 默认回归 | 108 项通过，18 项默认忽略（17 项 S7 外部进程用例、1 项真实 DLL 标定用例） |
| S7 会话专项 | 18 项通过，0 失败，耗时 133.60 秒；其中 1 项也包含在默认回归中，不重复累计 |
| 前端全量 | 41 个文件、898 项通过 |
| 应用及测试类型检查、生产构建 | 通过 |
| 前端覆盖率 | 语句 91.39%、分支 89.83%、函数 88.72%、行 93.83%，门槛全部通过 |

新增验证包括：视角越界和缺失拒绝、单设备四拍照点 `1→2→3→1` 的计数与计划、同次冻结图像与局部示教失效、前次触发不能满足本次取样、严格回放分组与元数据核对、三幅录制按拍照点还原所选视角，以及前端旧请求/旧草稿隔离。工位标定仍使用视角 1，其他视角不能自动继承该当量。

Rust 在 `src-tauri` 下运行 `cargo test --offline --locked --lib` 与 `cargo test --offline --locked --lib plc_session -- --include-ignored --test-threads=1`，日志分别保存在本机 `src-tauri/target/p0-step-t-rust.log`、`src-tauri/target/p0-step-t-s7.log`。S7 专项使用本机回环测试服务，不连接生产 PLC。

前端在本机直接运行已有依赖：`node node_modules/vitest/vitest.mjs run --coverage --pool=threads --maxWorkers=2`；源码与测试分别运行 `node node_modules/typescript/bin/tsc -b`、`node node_modules/typescript/bin/tsc --noEmit -p tsconfig.test.json`，构建运行 `node node_modules/vite/bin/vite.js build`。默认 fork 测试进程在本机受限环境出现停滞，本轮使用宿主 Node 的线程池执行相同测试集，未修改项目的测试配置。

本轮未执行真实 DLL 或真机验收。逐拍照点图像算法仍明确报告尚未接入，三目海康取图等待 SDK 交付形式确认；没有把模拟值作为示教试测、发布或现场检测准确率的证据。以下 2026-10-09 的记录保留为历史基线。

## S7 一期握手

协议、点表、PLC 参考程序和现场步骤见 [S7 一期交付说明](../integration/plc-s7-phase1.md)。线协议测试服务见 [S7 测试 PLC](../../scripts/s7-handshake/README.md)，使用真实 TPKT/COTP/S7 报文，仅监听本机回环地址。

- Python 报文与故障注入：`python -m unittest discover -s scripts/s7-handshake/tests -v`。
- Rust 契约和计划：在 `src-tauri` 下运行 `cargo test --offline --lib handshake` 与 `cargo test --offline --lib plc_plan`。
- Rust 会话与生产 S7 驱动联调：在 `src-tauri` 下运行 `cargo test --offline --lib plc_session -- --include-ignored --test-threads=1 --nocapture`，需要 Python 3 和本机 TCP 权限；不会连接生产 PLC。这些外部进程测试默认忽略，普通 `cargo test` 不代表已完成此项验证；可用 `S7_TEST_PYTHON` 指定 Python 可执行文件。
- 前端：`pnpm exec vitest run tests/plc-page.test.tsx tests/plc-editors.test.tsx tests/plc-dialogs.test.tsx tests/connection-form.test.tsx tests/plc-operation-lock.test.tsx tests/plc-s7-template.test.tsx tests/plc-recipe-plan.test.tsx`。

2026-10-09，S7 一期改动后的验证结果：

| 验证范围 | 结果 | 边界与证据 |
| --- | --- | --- |
| Rust 默认回归 | 59 项通过，0 失败，20 项默认忽略 | 忽略项中 17 项 S7 线协议已另外执行；3 项实际 DLL 测试本轮未执行 |
| Rust S7 会话专项 | 18 项通过，0 失败，耗时 103.46 秒 | 17 项真实 `ly_plc::PlcEngine` 与回环 S7 服务联调，另 1 项事务文件恢复；后者也包含在默认 59 项中，不重复累计 |
| Python S7 报文及故障注入 | 21 项通过 | 主机环境运行 TCP 测试，不使用生产 PLC |
| 前端 PLC 回归 | 7 个文件、127 项通过 | 模板、计划导出、操作锁、重连、页面和表单 |
| 前端类型与构建 | 应用及测试类型检查、生产构建通过 | 未执行本轮完整 UI 套件，也未重新生成覆盖率 |

会话专项日志在 `src-tauri/target/s7-session-tests/verification.log`，同目录各案例保存 `final-status.json`、`final-wire.json`、事务/审计文件与 DB 快照；这些本机产物不随源码提交。覆盖同 SN 不同序号、计划拒收、早/迟 ACK、写拒绝、执行后断线、释放失败、重启和腐坏日志、慢落盘、停止心跳及校验后重连。

`git diff --check` 通过。`cargo fmt --all -- --check` 未通过：全库存在大量与默认 rustfmt 不一致的排版，包括本轮未修改的 `caliper.rs` 等文件；本轮未执行全库自动重排。此项为格式差异，不能记作格式检查通过。

提交 PR 前，使用仅包含本次提交内容的独立快照，重新通过 `cargo test --offline --locked --lib`（59 项）、应用和测试类型检查及生产前端构建。固定到 `docs` 的架构图通过 5 个视图切换、8 个工作包展开、S7 状态、无外部资源和相对链接检查；这些结构与脚本检查不代表真实浏览器布局验收。

上述软件验证不替代 TIA Portal 编译、实际 CPU 通讯、三相机触发身份及现场长稳验收。下面的 942/37 项为本次 S7 改动之前的历史记录。

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

2026-10-09 本轮操作逻辑修复后的已完成验证：

| 验证范围 | 本轮结果 | 记录 |
| --- | --- | --- |
| 前端 UI 逻辑 | 39 个测试文件，942 项全部通过 | `output/playwright/robot-plc-demo/tutorial-capture/frontend-tests.log` |
| Rust 后端默认测试 | 37 项通过、0 失败；另有 3 项实际 DLL 测试按默认设置忽略 | `output/playwright/robot-plc-demo/tutorial-capture/rust-tests.log` |
| 模拟服务 | 22 项 Python 回归、5 项 Node 网页状态回归通过 | [本轮操作逻辑修复记录](UX_FIXES_2026-10-09.md) |

上述前端运行使用 `pnpm test`，没有重新生成覆盖率；本轮 Rust 默认测试也没有执行被忽略的实际 DLL 集成测试。测试日志和运行产物位于本机忽略目录，不随源码提交。

较早验证记录（同日、上述修复之前）：39 个测试文件、933 项测试通过，应用与测试类型检查及前端/隔离桌面构建通过。当时前端语句覆盖率为 90.35%、分支 89.43%、函数 87.50%、行 91.83%，覆盖率门槛通过；这些百分比仅对应当时的代码和测试。更早的扩充前基线为 26 个文件、251 项测试，语句 67.58%、分支 62.42%、函数 56.01%、行 73.05%。

Windows Codex 受限环境若遇到原生 realpath 的 EPERM，按项目的 Windows 命令环境指引加载兼容脚本。若依赖链接或测试工作进程的临时文件仍不可读取，应在获准的主机环境运行相同测试；不要改变应用代码或关闭失败检查来掩盖环境错误。

## 已覆盖的逻辑

| 测试文件 | 关键行为 |
| --- | --- |
| `workspace-context.test.tsx` | 配方恢复、快速切换、后台刷新竞态、草稿保护、保存修订、重复操作、防抖预览、事件清理 |
| `teaching.test.tsx` | 指定帧、图像绑定、参数变更失效、保存与下一帧、生产期间取样限制 |
| `validation.test.tsx` | 修订和样本期望失效、发布确认、失败保留弹窗、历史来源、原图样本导入前置条件 |
| `library.test.tsx` | 生产与候选去重、筛选、复制编号与产品代码、新建与删除确认、失败恢复 |
| `recipe-editor.test.tsx` | 候选与生产保存、拍照点解析、圆弧 bulge、分段胶宽限值、过期预览的成功和失败返回 |
| `geometry-page.test.tsx` | 真实编辑器与工作区的草稿/保存连接、候选切换与示教入口、工作中限制、物理覆盖与搜索余量 |
| `history.test.tsx` | 日期与结果筛选、分页复位、导出范围、空结果、查询返回顺序与失败恢复 |
| `history-detail.test.tsx` | 原图与测量完整性限制、规则与原图复测、原始记录保留、历史帧用于示教 |
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
| `cycle-visuals.test.tsx` | 曲线点选择与清空、帧关联、沿程和胶宽、快照哈希隔离 |
| `rejudge-retest.test.tsx` | 批量试算参数、原图复测入口、选件与筛选、缺图规则、重复操作、旧结果及错误隔离 |
| `workflow-model.test.ts` | 示教与验证失效、历史备份恢复、工件边界发布快照、运行限制、代表性样本 |
| `workflow-preview.test.tsx` | 所有流程页面的直接路由、存储损坏回退、情景筛选、重置、真实示教交互 |
| `workflow-setup-operations.test.tsx`、`workflow-recipe-operations.test.tsx`、`workflow-production-operations.test.tsx`、`workflow-preview-components.test.tsx` | 预览的完整建站、多个本地候选、删除确认、六帧示教/恢复、总览布置/背景、样本验证/边界发布、连续运行/停止、历史对照/导出、设置、缩放与叠加 |
| `navigation.test.tsx` | 默认与未知路由、历史复测路由优先级、侧栏折叠、浏览器提示、页面错误重试 |
| `sidebar.test.tsx`、`operation-guide.test.tsx` | 正式导航分组与整栏折叠、设备状态与卸载清理、全部引导链接和流程前置条件 |

## 已修复并有回归测试的问题

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
15. 已完成工件的模拟缺口过短，经过卡尺平均只剩一个缺胶点；实际原生测试验证扩大后的缺口跨两帧合并为 NG_GAP。
16. 首次拖模板插入参数提示后画布位置变化，拖动使用变化后的坐标导致模板偏移；整次拖动固定按下时的坐标转换。
17. 浏览器预览新建/复制覆盖原工作台、缺少删除确认、PLC 点位换页丢失、连续运行只执行一件；修复本地保存和等待/停止语义。
18. 数字输入清空后变成零，无法正常逐字输入负角度和负平移；空值保持无效并阻止提交，预览绘图使用有限占位坐标。

## 真实桌面 UI 验证

`scripts/Start-UiTestInstance.ps1` 使用 `tests/native/tauri-ui-test.json` 构建隐藏测试实例，应用标识为 `com.xyzrobotics.tujiaovision.ui-tests`，相机与 PLC 配置和历史目录与生产实例独立。默认调试端口 9337；输出文件、进程信息与哈希在 `output/playwright/ui-regression`。`-SkipBuild` 只允许哈希匹配的已有测试实例。

通过 Playwright CLI 连接该实例后，先执行 `tests/native/guard.js` 只读确认目录和模拟器；再从 UI 执行回放配置、离线图片导入、画布点击、保存标定，以及 PLC 还原/保存/断开/连接/数值写入。运行步骤保存在 `tests/native/*-ui.js`，后台命令仅用于只读断言；目录选择、参数更改、保存和写入均经过真实 UI。

复用前先检查脚本中的本机路径。部分 `tests/native/flyshot-*-ui.js` 和 `replay-ui.js` 仍固定读取 `D:\project\ly-gluesight\output\...` 的标定板、原图样本或回放目录，`flyshot-engine-ui.js` 固定填写 `D:\project\LyFlow\build\core\bin\lyflow_core.dll`，部分截图也使用固定输出路径。更换电脑或检出目录后，需准备相应样本、兼容的真实 DLL 和可写输出目录，并把这些路径调整到实际位置；离线原图和 DLL 不随仓库提供。先启动上述隔离实例并通过 `guard.js`，再按场景前置顺序执行。仅检出源码或运行 `pnpm test` 不会自动完成这些原生 UI 测试。

以下为较早一轮真实桌面 UI 验证记录，不能替代上表所列本轮测试结果：海康离线样本的回放预览为 640×512，原图 1280×1024。浏览器控制台没有告警或运行错误。报告和截图输出到 `output/playwright/ui-regression`。

随后通过 UI 配置模拟相机、实际 lyFlow DLL 和完整原图录制，导入棋盘原图得到 0.07999676 mm/px，完成四张 2700×1813 原图的模板拖动、试测和保存。良品/断胶原图组验证后发布 v1，再从 UI 运行真实图像检测：良品 2238 点判为 OK；跨 k2/k3 的 3.0 mm 断胶判为 NG_GAP，四帧完整，PLC 结果码分别为 1 和 13。

历史 UI 完成筛选、实际 CSV 导出、规则重判、四帧原图复测、对照切换和选帧缩放，原始记录保持不变；历史图回流示教后真实拖动、试测保存、精确恢复原图和参数、重新验证及取消发布也通过。通讯日志完成点位筛选、趋势、关闭跟随、导出与清空筛选。实际文件已检查：历史 CSV 5 条，结果码日志 CSV 9 条且含 UTF-8 BOM。WebView2 直接写入系统下载目录时可能不发送 CDP download 事件，因此 `plc-log-export-ui.js` 返回时间范围，运行后需核实新文件才能计为通过。

13 组实际操作的合并报告为 `output/playwright/ui-regression/report.json`；三个阶段保留 `report.*-phase.json` 和各自实例标识/可执行文件哈希，截图及已核实的 CSV 同目录保存。后两个阶段使用按名义几何生成的合成灰度图与实际图像算法，证明图像链路和操作流程，不代表现场实物缺陷准确率。

## 测试边界与后续缺口

jsdom 没有真实布局、灰度图解码与设备连接能力。测试环境为 ResizeObserver、scrollTo 和原生 dialog 提供最小替代实现；这些测试不会证明窗口布局、SVG 拖动坐标、原生弹窗行为、WebView 像素输出或硬件通讯正确。

基本操作、分层证据和边界逐项记录在 `docs/testing/UI_OPERATIONS.md`。界面测试覆盖用户动作和接口参数，原生测试覆盖像素与持久化，隔离桌面脚本覆盖完整示教/验证/发布/检测/复盘及真实拖动。已有离线检测脚本继续承担样本验证；实际设备通讯、现场节拍和实物缺陷准确率需要设备与现场验收。

新增业务测试应同时检查用户可见结果和关键接口参数，包含前置条件不满足、请求失败、重复操作或旧请求晚返回的路径。避免仅检查静态文本或给截图数量作为逻辑覆盖率。

## 2026-10-10 · P0 步 3

分支 `codex/p0-step3-cycle`。本节验证设备计数路由、逐拍照点终态、持久 cycleId、同 SN 重检隔离、排队及实际 DLL 工作线程超时占用；尚不证明逐点图像算法的现场准确率。

- Rust 默认回归 138 项通过、18 项默认忽略。命令为 `cargo test --offline --locked --lib`，日志为 `src-tauri/target/p0-step3-rust.log`。
- 前端 42 个测试文件、925 项通过。应用与测试类型检查、生产构建通过；覆盖率为语句 91.43%、分支 90.03%、函数 88.73%、行 93.90%，全部门槛通过。
- S7 回环专项 18 项通过，包括同 SN 不同请求序号、迟到 ACK、提前 End、写入失败、慢落盘、断连、重启恢复与心跳中断。使用本机受控 Python 夹具，未连接现场 PLC。
- 新增回归覆盖三设备四拍照点交错到达及 OK/NG/ERR、三目视角 1→2→3→1、首帧缺失、重复帧、提前 End、截止后帧和结果、同 SN 旧 cycle 结果、错误拍照点/相机/发布包身份、无效测量输出、重连没有新帧、工作线程卡住和迟到返回。
- 实时图像和前端异步数据统一按 cycleId 匹配；历史页临时显示身份 `history:<recordId>`，真实历史 cycleId 与发布包、PLC 交付状态将在步 6 入库。
- 未确认的 MVS 手动触发不再按时间自动解除布防限制；重复或倒退计数不能确认新手动触发。无法确认完成时必须重开相机。真机计数来源和开流起点仍按 W0 台架结果配置。

前端命令为 `node node_modules/vitest/vitest.mjs run --coverage --pool=threads --maxWorkers=2`、两套 `tsc` 检查和 `node node_modules/vite/bin/vite.js build`。S7 命令为 `cargo test --offline --locked --lib plc_session::tests -- --include-ignored --test-threads=1`。日志分别保存在本机忽略目录 `tmp/p0-step3-frontend.log` 和 `src-tauri/target/p0-step3-s7.log`。Windows 沙箱内依赖链接解析异常，以上验证在限定范围的宿主环境执行。

## 2026-10-10 · P0 步 4

分支 `codex/p0-step4-release-runtime`。默认 Rust 回归 157 项通过、21 项默认忽略；S7 回环专项 19 项通过；实际 DLL 专项 3 项通过。前端 42 个文件、925 项通过，应用和测试类型检查、生产构建通过；覆盖率仍为语句 91.43%、分支 90.03%、函数 88.73%、行 93.90%。日志为 `src-tauri/target/p0-step4-{rust,s7,native}.log` 和 `tmp/p0-step4-frontend.log`。

默认和 S7 命令沿用步 3。DLL 专项需设置 `LYFLOW_CORE_DLL`，运行 `cargo test --offline --locked --lib native_ -- --ignored --test-threads=1 --nocapture`；可设置 `GLUESIGHT_VISION_TEST_DIR` 保存夹具、测量图及逐站结果。本次证据在 `tmp/p0-step4-native/report.json`，记录每张夹具和测量图 FNV-1a 64；DLL 版本 `1.1.0`、FNV-1a 64 `014eb64e169b3cfc`、SHA256 `B784205274B8A06611322E814E894CE654F174EF5F329CCBB5E7AF54BFFB3125`。

- 棋盘标定使用完整原图像素，新帧不沿用旧棋盘结果。
- 示教胶路的暗胶、亮胶、缺口、空白后新帧、偏移、反向中线、毫米换算及冻结图核对均使用真实 DLL；空白整区明确报测量失败，不能成为 OK。
- 冻结发布包覆盖三设备四拍照点、视角 1→2→3→1、不同原图尺寸；外部原图和标定修改不改变包内资源及读数，缺失/篡改资源拒绝。实际预热 215 ms、稳定单帧 1 ms，仅为本机小型合成图测量，不代表现场分辨率或整件尾延迟。
- 真实 S7 回环新增慢 End 落盘：End 接收时冻结截止起点，落盘排队后的最后帧不能延长截止，输出 ERR 91。补丁已独立推送至步 3 分支 `94765c7`。
- `scripts/Test-PackagedRuntime.ps1` 已使用实际 DLL 验证，部署自检与运行时采用相同的示教胶路/标定算子清单。

本节证明资源冻结和真实测量接入。完整 UI 建配方到在线检测、历史原包复测及现场准确率仍由后续步骤验收。

## 2026-10-10 · P0 步 5

分支 `codex/p0-step5-workspace`。Rust 默认回归 167 项通过、21 项忽略，S7 回环专项 19 项通过；前端 42 个文件、942 项通过，应用与测试类型检查、生产构建通过。覆盖率为语句 91.48%、分支 89.96%、函数 88.83%、行 93.99%，所有门槛通过。日志为本机 `tmp/p0-step5-{rust,s7,ui}-final.log`。

隔离实例由 `scripts/Start-UiTestInstance.ps1 -Profile p0` 构建，标识为 `com.xyzrobotics.tujiaovision.p0-tests`；全部 PLC、相机均为模拟，测量使用真实 DLL。UI 建立 `P0-TRICAM-UI`，逐点选视角 1→2→3→1、绘制中线、填写 0.112 mm/px、试测保存、导入样本、验证并发布。随后只修改候选，分别检查视角、Pose、标定引用、相机绑定的局部失效与恢复，生产 v1 及原包的 19 个文件哈希不变。新增 cam3 后无需页面刷新即出现在规划下拉。

保持原 T_arm=200 ms，normal 和 gap 两件分别在 102 / 108 ms 布防，均为四次触发、四帧、307 测点；normal 为 `OK_WITH_EXCURSION` / PLC 2，gap 为 `NG_GAP` / PLC 13，P2 断口 8 mm 超过原 6 mm 限值。两个 cycleId 在开工前已有持久化证据。含排队的逐帧总耗时分别为 19/18/19/15 ms 与 18/20/19/18 ms；这些数值不是纯 DLL 耗时或性能压测。原图选用视角四张全部可用；随后发现实时图像容器裁剪，最终显示在迁移 C: 后已正式复验通过，十二视角完整录制留给步 6。

[桌面事实与哈希清单](evidence/p0-step5-native.json)记录了测试程序、DLL、原包、两个工件及原始证据。原始日志和截图保留在该清单列出的本机忽略目录，未把大型二进制或夹具图像提交到仓库。`tests/native/p0-*-ui.js` 保存实际 UI 操作与只读断言，复用时先调整本机 DLL、样本和证据目录。样本导出本轮依赖步 7 隔离树的 `make-samples.py`，当前分支不声明从全新检出自动完成桌面验收。

首次严格良品预期 OK 与 DLL 实际局部超差不一致的结果仍保留；流程用例明确改为预期实际 `OK_WITH_EXCURSION`，没有放宽产品限值，不能作为准确率达标证明。逐件 INSERT 导致的两次 201 / 202 ms 布防失败也保留；修复采用布防前已提交 ID 池。空池初始瞬态未被本轮 UI 捕获，由受控慢分配、失败退避、panic、非法批次和整批回滚回归覆盖。

实时图像布局调整后，临时 CSS 诊断在 1440×900 / 1280×800 窗口中看到完整图像与中线，图像区域分别约 245×196 / 240×192 px。诊断结束后已撤销注入；最终源代码构建成功（43.97 s），程序 SHA256 为 `C8CD775817B5280786DEA7E8EEC3AAB2C44A5824E6367C85E701646C4451AC2F`。`tests/native/p0-frame-visibility-ui.js` 保存四帧 × 两尺寸正式验收，但最终程序在用户代码执行前被宿主 I/O 阻塞，脚本尚未运行。这些诊断截图不能作为最终程序显示通过的证据。

2026-10-10 11:18 起系统反复报告存储设备重置及磁盘 0 I/O 重试，D: 所在东芝磁盘状态为 `Pred Fail`。已停止构建和压测；步 6 新增录制中断恢复、ACK 恢复、回放哈希验证与并发 DLL 运行身份等实现尚未做本次最终全量验证，步 7 仍为准备中的隔离模块。恢复存储后继续逐步验证并将草稿 PR 转为可审查状态。[精确阻塞记录](evidence/p0-storage-blocked.json)说明已通过与待执行的边界。

2026-10-10 迁移 C: 后恢复验收：以独立标识 `com.xyzrobotics.tujiaovision.p0-tests.recovery` 复制原隔离测试数据，只读 SQLite backup 校验通过；生产 AppData 未修改。由 C: 源码重建原生程序（1m49s），SHA256 `FB6DEBC2F7589513B46FFE62E8CAF94638C722CADDE841AF08D54519AC415DBC`，使用相同 SHA256 的真实 LyFlow DLL。正式脚本四帧 × 两尺寸共八项通过，无 CSS 注入；1440×900 / 1280×800 图像区域分别为 245.039×196.031 / 239.922×191.938 px，原图四角与中线完整可见，缩小后曲线可通过正常滚动访问，恢复窗口后图像仍完整。新 normal 工件 SN `791610945`、cycleId `2baa0f05943a3f37200013922ba024c8`，布防 127 ms，四帧 307 测点，实际 `OK_WITH_EXCURSION` / PLC 2，含排队逐帧总耗时 19/19/22/17 ms；原包 19 个文件与原隔离配置逐文件 SHA256 相同，候选和生产设置未变化。详见[恢复验收证据](evidence/p0-step5-recovery.json)。旧存储阻塞、临时诊断及严格良品失败保留为历史记录，本次验收不构成现场准确率或压测证明。


2026-10-10 Codex PR #11 审查补充：标定文件保存后广播 `calibration://changed`；工作台与相机配置变化一样刷新候选环境。编辑文档、未保存示教或操作期间保留待刷新标记，恢复可接受状态后补取；跨候选和过期请求不能覆盖当前编辑。新增回归覆盖标定失效、编辑/示教/操作解除、读请求途中新编辑以及旧请求隔离。定向 2 文件 58 项测试和两套 TypeScript 检查通过，日志位于迁移恢复目录 `p0-step5-review-focused-c.log`、`p0-step5-review-types-app-c.log`、`p0-step5-review-types-tests-c.log`。

## 2026-10-10 · P0 步 6（C 盘正式验收）

分支 `codex/p0-step6-history` 已集成逐拍照点历史、三视角录制、原图哈希校验、原发布包重现、候选比较、录制中断恢复及持久 ACK 恢复。数据库版本不符时备份后重建，原检测结论和 PLC 交付状态分别保留。ACK 恢复只接受合法的持久请求与结果身份，按精确 cycleId 与 SN 更新已有记录；迟到更新不能使 ACK 回退。回放在打开后再次使用冻结哈希验证实际解码字节，文件变化时拒绝取图。

用户确认迁移完成后，`c359cb6d615f2fbd3a141a4334e15fb4e599cb33` 在 C: 新目录恢复验证：Rust 默认回归 240 项通过、26 项默认忽略；前端 42 个文件、947 项全部通过，耗时 166.32 秒。覆盖率为语句 91.31%、分支 89.80%、函数 88.54%、行 93.93%，全局及三个专项门槛全部通过；应用和测试 TypeScript 检查、生产前端构建均通过。Rust 在同提交的 C: recovery 工作树执行，前端在 `C:\Users\11601\project\ly-gluesight\tmp\p0-step6-history-final` 执行，全部依赖与构建输出位于 C:。

[迁移后回归记录](evidence/p0-step6-regression-c.json)保存命令、日志路径及 SHA256；本机日志根目录为 `C:\Users\11601\AppData\Local\Temp\gluesight-p0-recovery-20261010`。S7 include-ignored 回环专项随后通过 22 项（45.98 秒）；真实 DLL 同标签并发测试通过，48 次并发运行的身份与像素归属均被检查。真实 DLL 原包重现测试首次失败：第二拍使用整幅空白图却预期 `NG_GAP`，实际返回 `ERR_INSPECT`，失败断言位于原 `workspace/reproduce.rs:101`。随后仅修正测试夹具：抹去 x100..140 的 10 mm 局部胶段，保留原 6 mm 限值，检查局部断胶为 `NG_GAP` / PLC 13、其他三拍读数不变；另独立检查整幅空白为 `ERR_INSPECT` 且不生成测点索引。使用同一真实 DLL 重跑通过 1 项（1.05 秒），证据同时保留首次失败和修复后日志 SHA256。默认忽略项不能全部计为已通过，此前中间版本的 Rust 207 项、S7 21 项只作历史基线。

C: 隔离原生程序已完成正式验收（构建 1m22s，SHA256 `85F926EC6358E5D710FB4668CDA0C3DBEE6307F4E3F716D86927120DD84BB484`）。旧数据库实测 `user_version=0`，自动生成完整性校验通过且保留 6 条记录的 `.bak`，新库版本为 2。normal / gap 分别在 112 / 143 ms 布防，四帧 307 测点，每件十二张原图完整落盘并保留逐文件哈希；结论分别为实际 `OK_WITH_EXCURSION` / PLC 2、`NG_GAP` / PLC 13（8 mm > 6 mm）。normal 实际观察到录制 Pending（0 原图）→ Complete（12 原图）、PLC Submitted→Acknowledged。

`p0-history-ui.js` 在原生历史页逐一显示两件全部 24 个原始视角（1280×1024，SVG 高 324 px，居中滚动后全部位于可见窗口），检测所选视角才显示中线并允许用于示教；CSV 导出及原包重现、候选规则重判、候选原图复测均通过，原始检测记录未改。`p0-history-independent-ui.js` 验证选择其他配方候选以及本配方 Pose 尚未保存时，仍可按原发布包重现，候选比较按钮保持禁用；未保存 Pose 保留后恢复原值，保存候选、生产配置和 19 个冻结文件哈希未改。

`p0-ack-process-recovery.mjs` 使用真实 TCP S7 测试服务发送匹配计划请求 101 / SN `70100101`。生产策略安全拒绝 Synthetic 相机，真实上报 `ERR_INSPECT` / PLC 90 / fault 98；在历史已 Submitted 后，用外部 SQLite `BEGIN IMMEDIATE` 锁阻止更新，测试服务发出匹配 ACK。握手主日志已持久化 `acknowledged=true`、数据库仍 Submitted 时终止仅本次创建的隔离进程；释放锁并重启后，实际 `Machine.new` 将精确 cycleId `46224c90d1642f9e411ead533e4abef9` / SN 记录恢复为 Acknowledged。除交付字段外，原错误结论、四条缺帧记录和空测点载荷不变；没有伪造收据或绕过生产相机政策。中断发生在 ACK 审计行写入前，仅主日志恢复的时间取请求开始时间并与原交付时间取最大值，不能将该值解释为精确 ACK 时刻。首次脚本 WebView 导航竞态失败及历史截图的固定页眉滚动边界均保留；脚本等待真实加载、居中滚动后复验通过。[桌面与进程恢复证据](evidence/p0-step6-native-c.json)保存原报告、截图、CSV、原图及冻结包 SHA256。

复用前先创建 `output/playwright/p0-history`，启动已核验标识的隔离原生程序并连接模拟 PLC，再运行三个 `async(page)` 脚本。ACK 进程专项要求先关闭现存隔离实例，传入 `--executable`、`--instance`（包含精确隔离标识及 SHA256）、`--appdata`、新的 `--output` 目录、后端导出的 `--template`、`--playwright-module`、`--fixture scripts/s7-handshake/s7_test_plc.py`；脚本仅接受 C: 的 `.p0-tests.recovery` 实例，结束后还原 PLC 配置并归档本轮握手证据。软件验收通过不替代现场硬件、准确率或步 7 的故障和性能验收。

2026-10-10 PR #12 审查说明：数据库策略遵循 [P0 计划 D-0](../architecture/p0-plan.md)，任何与当前 `DB_VERSION` 不符的版本均改名备份后新建，包含高于当前版本的数据库；因此不采用“未来版本只拒绝打开”的建议。已有真实 SQLite 回归 `store::tests::mismatched_database_version_checkpoints_wal_and_keeps_complete_backup` 使用未来版本 73 和尚在 WAL 的旧数据，核对备份保留版本与全部数据、新库使用当前版本且不保留旧表、再次打开不重复备份。

旧录制保留清理与本件录制完整性独立：清理错误保存在 `RecordingOutcome.retention_errors` 与 `part.json.retentionErrors`，审计单独记录 warn；本件原图、元数据或哈希失败仍进入完整性 `errors` 并禁止可用状态。新增 Windows 真实文件占用回归触发旧件删除失败，检查本件仍 Complete、三视角可按冻结哈希回放，释放占用后旧件可删除；另检查审计入库完整状态、原始判定与三视角引用不受清理告警影响，同一回调不重复告警。本次在 C: 独立工作树执行 `cargo test --offline --lib --manifest-path src-tauri/Cargo.toml`，默认全量 242 项通过、0 失败、26 项忽略，测试耗时 2.75 秒；上述新增两项及未来版本 WAL 用例均实际通过。日志 `C:\Users\11601\AppData\Local\Temp\gluesight-p0-recovery-20261010\p0-step6-retention-review-rust-c.log`，SHA256 `F700B6C4C22C671E047AEE91976ECF14AED94C600416D383E9AE130E4E6C7613`。

PR #12 孤立 `_pending` 清理补充（已执行新增及全量回归，结果见下段）：生产录制线程在启动及 Finish 前分批回收未引用的 pending 目录，每轮最多检查 64 项并保留扫描游标；仅在找到非活跃候选时读取一次当前 profile 的历史保护集合。`parts.recording_directory` 与 `part_shots.raw_files` 中的 pending 引用、回放在用路径及本进程活跃录制均保留；未成功搬离 pending 的收尾继续受本进程保护，避免异步审计尚未入库时丢失部分证据。引用查询或解析失败时保留目录，错误只记独立 retention 告警。清理占用通过短锁登记，磁盘检查与删除不持有活跃录制互斥锁；同名录制不能覆盖正在清理的目录。只删除当前 records 根下、命名符合录制规则且没有链接的普通 pending 目录；Windows reparse、子目录、超过 256 个文件均保守保留并报告。路径和子树检查与删除之间仍存在外部进程同时替换目录的非原子边界。

新增 8 项回归覆盖短锁互斥清理占用、孤立目录回收、历史部分证据及当前活跃录制保护、64 项分批续扫、引用查询失败时完整性不降级、Windows junction 不跟随、仅活跃目录不查询历史、异步审计窗口内失败收尾保护，以及 SQLite 的目录/原图引用及损坏 JSON 防护。实际集成步骤 5 预热恢复后，全量 Rust 257 项通过；随后补 AppData 进程独占锁，在数据库恢复及录制清理前取得，退出或崩溃后由操作系统释放，避免同 profile 第二实例触碰活跃录制。真实 Windows 文件锁测试与完整回归最终 258 项通过、0 失败、26 项默认忽略，耗时 2.79 秒；先前测试在持锁时用另一句柄写标记的 OS 33 失败日志保留，测试改为锁前写入、释放后核验，生产锁逻辑不变。最终原生重复启动及400件在步骤7执行。见[临时录制恢复证据](evidence/p0-step6-pending-review-c.json)。

PR #12 审计瞬态数据库故障重试补充：审计待写项不再在第 3 次失败后永久停止周期重试，按 1/2/4/8/16/30 秒退避，之后最多每 30 秒继续尝试。失败计数和截止时间饱和计算；新事件合并原始判定、ACK 与原图/录制证据，不重置或绕过同一工件已有退避。完整保存后清除退避，已保存字段继续去重，部分写入成功后只重试未保存字段。新增真实 SQLite `BEGIN IMMEDIATE` 写锁回归保持锁跨越 4 次入库失败，释放后仅调用周期 retry，核对原结论/测点、精确 cycleId/SN、ACK、全部三视角引用和 Complete 录制证据恢复，同时相同 SN 的其他工件不变；另覆盖事件突发不绕退避、计数饱和及成功后不重复写入。

此机制仍是有界内存暂存：待写最多 128 个 cycle、总缓存最多 512 项，30 分钟未收到该 cycle 新事件的待写项仍会淘汰并报错。超过容量、超时淘汰或进程终止时，尚未入库的内容仍不能保证保存；本轮未新增持久 spool 或生产停机联锁。不可将持续重试表述为掉电持久追溯保证。原生 400 件性能验收及验证器结束后，在 C: 独立分支执行 `cargo test --offline --locked --lib --manifest-path src-tauri/Cargo.toml`，默认完整回归 262 项通过、0 失败、26 项忽略，编译 1m24s、测试 2.99 秒；上述真实 SQLite 锁恢复及新增回归均实际通过。日志 `C:\Users\11601\AppData\Local\Temp\gluesight-p0-recovery-20261010\p0-step6-audit-retry-c.log`，SHA256 `3F4402B4E2C679CBDBF2ACF0315361A27152CEDA5F6BC5D7D9DFE832A1C4D206`。

## 2026-10-10 · P0 步 7（最终设置刷新修复后回归）

`c94606a` 集成步骤 5 检测设置变更通知、步骤 6 临时录制清理和进程独占锁，保留步骤 7 的 Prepared 与发布逐件核验优化。设置保存成功后，工作台按既有延后刷新规则重新核验引擎身份，不覆盖文档或示教草稿。

C: 全量 Rust 275 项通过、0 失败、30 项默认忽略（3.31 秒）；前端 42 文件、960 项通过（175.62 秒），覆盖率语句 91.37%、分支 89.78%、函数 88.63%、行 93.96%，全部门槛通过。两套 TypeScript 检查和生产构建通过，最终前端资源 `index-BvtVge_I.js`。日志及 SHA256 见 [最终回归记录](evidence/p0-step7-regression-c.json) 的 `settingsRefreshFinal`。本段不把尚未结束的最终 400 件与五演示计为通过。

预热临界排队 P1 进一步修复于 `6a110a6`：许可与 blocking pool 排队合计最多 30 秒，实际 worker 开始后独立获得完整 30 秒；排队过期仍可重试，执行超时仍永久拒绝且持有许可至返回。新增真实单线程 blocking pool 测试覆盖排队 600 ms 后执行 300 ms、各自 800 ms 上限的成功场景。步骤 5 全量 175 项、8 项 Gate 通过；最终步骤 7 全量 276 项通过、30 项默认忽略（3.16 秒）。前端、验收脚本、Prepared 与发布核验对比 `39b5c34` 未变，继续使用上面的 960 项及构建证据。见回归 JSON 的 `warmupBudgetsFinal`；先前合并绝对截止合同及其 174 项日志保留为历史证据。

PR #12 ACK 启动恢复范围补充：先由 SQLite 一次查询仍为 Pending/Submitted/Failed 且有 cycleId 的交付集合，查询失败记录错误并跳过恢复，不将错误视为空集合；已 Acknowledged、NotRequired 及已清理工件不进入恢复候选，也不逐件更新数据库。当前主握手日志仍从磁盘重新验证，匹配未决 cycle 的持久 ACK 仍可恢复；协议 pending 与 ResetRequired 的安全拒绝不变。只有未决目标进入候选缓存并检查精确请求/结果身份冲突，没有未决交付时完全跳过 audit 文件。部分索引 `parts_unresolved_delivery` 在原数据库版本内幂等创建，首次升级创建索引仍需一次 SQLite 历史扫描。

没有压缩、截断或改写 append-only ACK 审计，也没有引入 checkpoint。非空未决集合时，仍需流式读取并解析整个 audit JSONL，残留耗时为 O(日志字节数)，内存为 O(未决 cycle 数 + 当前单条记录字节数)，SQLite 恢复调用最多为本次未决目标数；不能宣称已消除所有生命周期文件扫描。新增回归覆盖 10000 条无关旧收据不入候选缓存、目标冲突拒绝、空集合不读 audit 且主日志错误仍报告，以及真实 SQLite 重启后的 128 件已 ACK/128 件已清理历史、精确同 SN cycle 筛选、主日志 ACK 回退、部分索引查询计划、查询错误不伪装空集合和原审计字节保留。原生验收结束后在 C: 独立分支执行默认全量 Rust，266 项通过、0 失败、26 项默认忽略，编译 42.45 秒、测试 3.08 秒；S7 include-ignored 回环专项 22 项通过、0 失败，耗时 48.73 秒，其中持久 ACK 重启及 Reset 后恢复实际通过。真实 SQLite 样本中的 128 件已清理历史先实际入库，再由 `purge_before` 删除并核对不存在。首轮新增夹具一次分配 128 个 ID 超过既有每批 32 个契约，保留失败日志后仅改为四批分配，生产逻辑未因此改变。日志根目录为 C: 恢复目录；`p0-step6-ack-scope-rust-c.log` SHA256 `2EC57AB38EFC251CFE9A36EC712677B872A7D0D9BDDACCE753945886C47CF0DD`，`p0-step6-ack-scope-s7-c.log` SHA256 `A30721BBE4D9B609E8DE707159D3D008CF12E911F63BD72FBB0578F3BD97BC19`，首次失败为 `p0-step6-ack-scope-rust-first-c.log`。

## lyFlow 原始图像注入回归（较早记录）

客户端固定到主线 `5b796c3`（Image ABI v15），使用 `RunSpec.image_inputs` 注入完整 u8 灰度帧。运行库必须包含 `io.load_image`、`image.board_calib`、`image.load_calib`、`glue.locate`、`glue.station_calipers`；在 lyFlow 仓库设置 `LYFLOW_PACKS=glue` 后构建 core，系统设置填 DLL 绝对路径。

普通后端测试：在 `src-tauri` 运行 `cargo test --offline --lib`。另有 3 项实际 DLL 集成测试，默认明确标为 ignored；设置 `LYFLOW_CORE_DLL` 后运行 `cargo test --offline --lib -- --include-ignored --test-threads=1`。`--offline` 要求依赖已缓存，首次准备依赖时需先允许 Cargo 获取依赖。

较早验证记录（2026-10-09、当时的测试集）：35 项普通测试加 3 项实际 DLL 集成测试，共 38 项通过，实际 DLL 测试均已执行。本轮修复后的默认测试集已增加，当前记录为 37 项通过、3 项 ignored；本轮没有重新运行这 3 项实际 DLL 集成测试，不能把较早的 38 项结果作为当前完整测试集已全部通过的证明。

测试验证完整像素的棋盘标定（已知 0.125 mm/px）、飞拍毫米距离/胶宽、有工件→空白→有工件的缓存隔离、模拟缺胶在实际卡尺平均后跨帧合并。还验证点表列长、编号与坐标；缺失胶宽或定位失败不能变为有效测量。样本组导入失败验证清理暂存文件，候选新建验证 Windows 大小写与产品代码冲突。生成的合成测试图在 `output/engine-tests`，不作为客户缺陷准确率的证据。


## 2026-10-10 · PR #11 预热排队超时修复

预热仍共用一个许可，排队与实际执行各有独立的 30 秒上限。排队窗口包含许可等待与 blocking 执行器排队；只有 blocking 工作线程真正开始后才起算完整执行窗口，排队不扣减执行预算。尚未开始的超时转为带 1 秒冷却的可重试状态；每次尝试只安排一个延迟 Refresh，按届时所选配方重新检查，重复 Refresh 不能并发启动同一项。已开始的失败、panic 或超时仍永久拒绝该缓存项，后台线程返回前不释放许可，超时后的迟到成功不能成为 Ready。

较早合并预算实现的 `production::warmup_tests` 7 项 gate 回归覆盖累计排队后恢复、blocking 池排队过期不执行、实际执行挂起保留许可及迟到结果隔离、实际失败隔离、panic 释放许可和并发重试去重。该次验证未覆盖迟到启动仍需完整执行窗口，保留为历史尝试。原生 100 件恢复控制完成后，在空闲窗口实际执行 `cargo test --offline --locked --manifest-path src-tauri/Cargo.toml --lib`，默认完整回归 174 项通过、0 失败、21 项忽略，耗时 0.88 秒；上述 7 项 gate 均通过。见[预热审查修复证据](evidence/p0-step5-warmup-review-c.json)。受控 Gate 不冒充真实 DLL 永久卡死注入。


## 2026-10-10 · PR #11 系统设置环境刷新修复

`cycle_save_settings` 成功持久化并应用设置后广播 `cycle://settings-changed`。工作台订阅该事件并复用现有延迟刷新：干净候选重新读取后端环境指纹并显示试测失效；未保存配置、中线草稿及操作期间合并待刷新标记，解除后补读，晚到响应和旧候选响应不能覆盖新编辑或新选择。无新增阈值或生产 Gate。

C: 独立分支 `codex/p0-step5-settings-refresh` 定向 `workspace-context.test.tsx` 与 `settings.test.tsx` 共 85 项通过（17.16 秒），应用及测试 TypeScript 检查通过。日志位于恢复目录 `p0-step5-settings-refresh-focused-c.log`、`p0-step5-settings-refresh-types-app-c.log`、`p0-step5-settings-refresh-types-tests-c.log`。为保持原生验收窗口，此 helper 未运行 Cargo、全覆盖率或原生 UI；集成后统一验证。


PR #11 执行窗口修正：新增真实单线程 blocking 池 gate，先排队约 600 ms，再执行约 300 ms，排队和执行预算各 800 ms；跨越原申请起点的 800 ms 后仍保持 Preparing，释放执行 gate 后成功。该用例同时防止错误地从取得许可而非工作线程真正开始计执行时间。实际执行超时仍永久 Failed，许可直至线程返回才释放，迟到成功不追认；未开始的排队超时仍可冷却重试。本轮在 C: 共享 target 执行默认完整 Rust 回归，175 项通过、0 失败、21 项忽略，测试耗时 0.93 秒；8 项预热 gate 均通过。日志 `p0-step5-warmup-budgets-c.log` 与源码 SHA256 见同一预热审查修复证据的 `executionWindowCorrection`，旧 174 项结果及合并窗口合同完整保留为 `historicalAttempt`，不再作为最终合同。前端未修改，本轮未重复前端覆盖率或原生 UI。


## 2026-10-10 · 四组实际 CycleHost 400 件基线与审计最终回归

封存构建源码 `d41d19e`、运行时代码 `6a110a6`，可执行文件 SHA256 `E304148D45DFF745B6B8C61159FFF5B58C6D5CC9695220B49FCD0FB56835F171`。单目/三目 × normal/gap 每组 100 件全部完成；正常件全部严格 OK/1、断胶件全部 NG/13，1600 次测量及 3200 张 1280×1024 原图逐幅匹配输入，四组独立验证器通过。布防限制保持 200 ms，最大 112 ms；仅隔离 profile 的 recordKeep 从 500 调至 1000 以保留全部证据，算法、ROI、阈值未变。

| 组合 | 件数 | arm P50/P95/最大 ms | frame P95 ms | partEnd→PLC提交 P95 ms | private增量 B |
|---|---:|---|---:|---:|---:|
| tricam/normal | 100 | 51/66/69 | 27 | 31 | 17510400 |
| tricam/gap | 100 | 51/75/110 | 26 | 31 | 8597504 |
| single/normal | 100 | 56/76/112 | 26 | 31 | 1933312 |
| single/gap | 100 | 58/75/90 | 25 | 31 | 1134592 |

耗时为 nearest-rank；frame 包含排队，尾延迟截止 PLC 提交，不是 ACK 或纯 DLL 时间。private 只含原生主进程，不含 WebView2，有限 100 件趋势不能证明长期无泄漏。开发 PLC 模拟器与确定性回放不能替代现场 W0/W7。完整指标、内存斜率与原报告 SHA 见 [400 件基线](evidence/p0-step7-cyclehost-c.json)。suite 的 `toolSourceCommit=7390825` 是初始工具标签，实际工具字节已另行封存并记录 SHA。

400 件结束后集成审计重试修复 `e5af201`，完整 Rust **279 项通过、0 失败、30 项默认忽略**（2.94 秒）。前端 960 项及类型/构建继续适用：前端、Prepared、CycleHost、发布逐件核验相对上述基线未变。新审计程序的同 profile 重复启动、四组合各 1 件检查与五个 Robot/Modbus 演示另行执行；不将旧程序 400 件标为新审计程序的性能结果。瞬态锁恢复已经真实 SQLite 四次失败后仅靠周期 retry 验证；128 待写/512 缓存/30 分钟未收新事件的淘汰边界仍然存在。

## 2026-10-10 · PR #12 比较结果磁盘保留

原包重现、候选规则重判和候选原图复测的比较 JSON，按每条历史记录合计保留最新 100 份（所有候选及来源共享窗口，按 createdAt、id 降序）。列表先取该窗口再筛候选；窗口之外的比较文件实际删除，不再仅截断返回值。启动及每日历史清理同时修剪现有窗口并删除已无 SQLite 历史身份的比较目录。历史编号可复用，比较以 cycleId 再校验；新工件不会读取旧身份的比较。

比较计算仍在锁外进行；保存、比较目录清理与历史 DELETE 使用同一进程内门。计算期间历史若已过期，落盘时拒绝保存，不能重建不可达目录。现有 AppData 实例锁约束同配置的协作进程。比较清理警告独立记录，不更改检测结论、PLC 交付状态或原图录制完整性。

清理先完整读取并核对目录内 JSON 的历史编号及文件身份，再删除过期项；所有路径祖先、目录及文件拒绝符号链接和 Windows reparse point，不递归跟随未知目录。损坏 JSON、未知文件、数据库查询或文件删除失败时保留未确认的证据并报警，后续保存需先成功清理已有窗口，不能在故障状态持续追加。原子新写若保留清理失败会尝试回退新文件；回退也失败时残留最多本次新增一份，下一次写入前仍必须先清理。崩溃留下的暂存文件同样需要人工确认，不会自动当作无效证据删除。

新增真实临时文件系统 / SQLite 回归覆盖磁盘最新 100 份、多候选共享窗口、已清理历史及既有孤儿、并发复测、过期在途结果、历史编号复用、损坏 JSON / 数据库错误、Windows 文件锁回退和三层 junction 拒绝。在 ACK helper 释放共享 target 后，独占运行 `cargo test --offline --locked --manifest-path src-tauri/Cargo.toml --lib`，269 项通过、0 失败、26 项忽略；编译 1m19s，测试 3.12 秒，以上 7 项新回归均实际通过。日志为 `C:\Users\11601\AppData\Local\Temp\gluesight-p0-recovery-20261010\p0-step6-comparison-retention-c.log`。此 helper 不重复原生桌面或测量验收。


## 2026-10-10 · P0-15 待入库日志与生产闭锁

原有内存重试不保证进程退出后的未入库记录可恢复。本分支将完整工件结果、PLC交付事件和录制结果写入独立有界待入库日志，实际落盘后才接受结果提交。SQLite失败期间原始事件保留；容量耗尽、未知临时文件、日志损坏或写入失败阻止后续布防。启动先完整回放日志，再将真正未完成的Pending录制标为失败，最后恢复PLC事务和初始化生产配方。

结果提交前的待入库日志接受等待使用现有procMs作为独立有界窗口；它不更改每帧测量截止或armMs。提交后的PLC交付及实际提交耗时另写持久事件。drainMs仍为实际观察partEnd至PLC结果/done写入完成的总耗时，包含先行审计等待；未确认提交则不伪造耗时。结果已提交而后续审计失败时，保留原检测结论并进入FAULT。开发Modbus确认事件保存失败时不释放DONE；生产S7另外保留原持久握手日志。

历史删除只在审计无待回放事件且保持保护锁时执行。录制临时清理与日期滚动保留同时保护数据库Pending引用和待入库日志引用；无法确认保护范围时保留证据并报告清理告警。容量上限为背压边界，不能以内存淘汰代替持久化成功。文件系统、SQLite与PLC并不构成跨设备原子事务，仍不声明断电零丢失或现场准确率完成。

本保证从最终工件记录或事件被耐久接受开始。Acquire阶段尚未生成最终Record时强杀进程，cycleId和S7事务仍可追溯，但内存测量、尚未回调的原图或尚未生成的最终结论不在此恢复保证内；本分支未新增布防前Started全量记录。

源码 `8ffc091f10226a78bfd68640a5d9cf93fa618419` 完成 Rust **335通过/0失败/30默认忽略**（3.87秒）、S7真实回环 **24通过**（50.54秒）、真实LyFlow DLL **5通过**（1.26秒），前端类型与Vite构建通过。新增18项默认回归覆盖TTL/容量淘汰后的重启回放、真实SQLite锁、写库后未删收据的幂等回放、坏JSON/tmp/容量、Windows写拒绝和junction、实际子进程kill后恢复、原图提升目录保护、Submission乱序与实际耗时、同cycle内容冲突、快照后追加ACK保护、启动录制恢复顺序及滚动保留保护。

全新 `.p0-tests.audit` 原生实例使用显式四shot几何与Sim相机，`vision=false`只用于协议持久化专项；没有示教或发布包，也不宣称任何图像检测通过。实际S7序号101、SN740000101被生产计数政策安全拒绝为90/98。请求之前实际持有SQLite `BEGIN IMMEDIATE`；仅在测试服务对DONE写入延迟一次600ms时观察完整Insert已落盘、DONE=false、SQLite零主行，随后实际DONE=true。实际ACK进入S7持久日志后中断本次原生进程，SQLite仍零主行；释放锁并启动同源码新进程，spool先恢复工件，Machine再关联精确ACK，最终1条原判定、4条shot、录制Failed/零原图、`drainMs=null`与Acknowledged交付，spool归零。中断时未观测到ACK spool事件，因此本次实际覆盖S7日志回退；单元回归另覆盖ACK事件已经在spool的路径。全部4个受控进程以及前置bootstrap均已退出。

[紧凑证据](evidence/p0-audit-durability-c.json)引用C盘原始报告与日志，按实际字段、文件路径和字节数核查，无内容指纹匹配。首次编译缺少旧Recorder测试夹具字段、首次功能回归发现Submission早于Insert被错误退避，均保留原日志；修复生产分支后严格首轮回放通过，未放宽断言或改成多轮重试。600ms观测延迟及仅该专项的S7连接2000ms超时不作为性能结果，也不修改armMs=200或默认procMs=3000。

复现工具为 `tests/native/p0-audit-process-recovery.mjs`，构建隔离overlay为 `tests/native/tauri-audit-test.json`。准备C盘全新配置、显式四shot配方及S7 DB100模板，程序manifest须包含相同identifier、executable和sourceGate；profile与每次输出目录必须不存在。参数为 `--executable --appdata --output --template --fixture --instance --recipe --settings --cameras --python --playwright-module --source-gate`，默认CDP9342。相机输入为 `{cameras:[...],nextId:2}`；模板可为配置本体或含template的导出对象。不得复制已有数据库或发布资产替代全新配置。

## 2026-10-10 · PR #15 审计故障与就绪检查并发修复

Codex P1审查发现：spool写失败先释放锁、随后发布故障，布防或历史清理可能观察到“无故障且spool为空”。源码 `fc1e38a` 让append、健康探针、事件读取与收据删除的错误在spool锁内先锁存，ready/with_ready统一按spool→failure取锁；日志在释放spool后发布。异步落盘超时仍能独立锁存故障，不等待可能阻塞的磁盘I/O。

完整Rust338项通过、0失败、30忽略，21.13秒。新增3项确定性并发与Windows真实文件锁回归，证明失败时就绪及清理动作均不放行，spool锁被占用时超时故障仍及时发布。见[并发修复证据](evidence/p0-audit-review-atomic-c.json)。本次未重跑原生中断专项；此前 `8ffc091` 的报告保持原来源，后续压力与性能使用包含本修复的新源码。


## 2026-10-10 · PR #15 录制收尾就绪屏障

第二项Codex P1指出：结果和ACK已经入库、spool清空时，异步Recorder收尾仍可能尚未回调。源码 `04c3839` 在Recorder.begin前登记明确cycleId，包含Off与提前失败的同步回调；仅在Recording终态事件同spool锁内实际落盘成功后解除登记。ready与历史清理同时检查未决录制、故障、writer存活及spool为空。终态持久写失败保留登记并锁存故障，异步超时发布仍不等待磁盘锁。

3项新增回归实际执行：真实Recorder保存三视角并收尾，真实SQLite已确认ACK且spool为空时将其实际回调门控，证明布防和清理均拒绝，终态落盘且入库后恢复；真实容量拒写保留未决录制与故障；真实Off同步回调先于Insert仍不会提前就绪。定向3/3通过，完整默认Rust341通过、0失败、30忽略，3.76秒。见[录制屏障证据](evidence/p0-audit-recording-barrier-c.json)。

慢录制时ACK后可暂态拒绝下一件；原检测结论及已确认ACK保持，终态耐久接受并回放后自动恢复。该登记仅用于当前进程的就绪屏障，未新增Acquire持久记录；启动仍先回放spool，再恢复真正中断的数据库Pending录制，最后恢复PLC握手。本次未重跑原生中断、压力或400件性能，先前原生恢复继续归属8ffc091。
