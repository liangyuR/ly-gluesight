# 前端 UI 逻辑测试

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

## lyFlow 原始图像注入回归

客户端固定到主线 `5b796c3`（Image ABI v15），使用 `RunSpec.image_inputs` 注入完整 u8 灰度帧。运行库必须包含 `io.load_image`、`image.board_calib`、`image.load_calib`、`glue.locate`、`glue.station_calipers`；在 lyFlow 仓库设置 `LYFLOW_PACKS=glue` 后构建 core，系统设置填 DLL 绝对路径。

普通后端测试：在 `src-tauri` 运行 `cargo test --offline --lib`。另有 3 项实际 DLL 集成测试，默认明确标为 ignored；设置 `LYFLOW_CORE_DLL` 后运行 `cargo test --offline --lib -- --include-ignored --test-threads=1`。`--offline` 要求依赖已缓存，首次准备依赖时需先允许 Cargo 获取依赖。

较早验证记录（2026-10-09、当时的测试集）：35 项普通测试加 3 项实际 DLL 集成测试，共 38 项通过，实际 DLL 测试均已执行。本轮修复后的默认测试集已增加，当前记录为 37 项通过、3 项 ignored；本轮没有重新运行这 3 项实际 DLL 集成测试，不能把较早的 38 项结果作为当前完整测试集已全部通过的证明。

测试验证完整像素的棋盘标定（已知 0.125 mm/px）、飞拍毫米距离/胶宽、有工件→空白→有工件的缓存隔离、模拟缺胶在实际卡尺平均后跨帧合并。还验证点表列长、编号与坐标；缺失胶宽或定位失败不能变为有效测量。样本组导入失败验证清理暂存文件，候选新建验证 Windows 大小写与产品代码冲突。生成的合成测试图在 `output/engine-tests`，不作为客户缺陷准确率的证据。
