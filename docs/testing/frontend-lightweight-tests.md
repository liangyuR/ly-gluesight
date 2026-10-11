# 前端轻量测试（2026-10-11）

UI 测试以一次创建配方、完整示教、工件启动到落盘三个代表流程为主。删除依赖大批合成业务状态的页面流程、重复字段/状态矩阵和布局文案断言；保留小范围的输入解析、草稿保护、异步隔离、生产锁与文件导出完整性测试。没有修改生产业务代码或删减后端测试。

## 三个主流程

| 流程 | 实现与边界 |
| --- | --- |
| 创建配方 | `tests/recipe-create-flow.test.tsx`：真实配方库填写编号/名称/产品代码，创建候选，真实 `WorkspaceProvider` 选中新候选，经进度页进入采集页；仅模拟 Tauri `invoke/listen`，使用零拍照点的空候选 |
| 完整示教 | `tests/teaching-flow.test.tsx`：两个已采集拍照点，各选图 1，逐帧在真实图像组件点中线、填像素当量、保存中线、试测、保存并下一幅，全部 2/2 完成后进入总览；真实页面与 Provider，接口返回最小分阶段数据，jsdom 使用恒等坐标替代布局 |
| 工件启动到落盘 | `tests/native/production-flow-ui.mjs`：隐藏的 `flow-tests` 独立桌面实例，通过 UI 配置模拟 PLC、模拟测量及全量原图录制，启动一件，等待节拍握手结束，查看历史原图；独立只读 SQLite 连接验证本件、版本快照、逐点测量和逐拍照点记录，并读取真实 PGM/元数据文件与后端解码结果 |

前两个流程验证前端接线与交互，不证明后端持久化或实际测量。第三个流程运行真实应用后端和磁盘，因此单独作为桌面集成测试，不混入 jsdom 单元测试计数。三个流程均不替代真实 SDK、PLC、LyFlow 引擎或现场准确率验收。

## 删减结果

基线为合并 PR #22 后的 `e60664b`，修改前实际运行 963/963 通过。首次从 43 个文件、963 个用例收缩到 40 个文件、369 个用例；继续删除指南颜色、路由高亮、原型控件、正常采集/导入 API 接线，并将连接/地址表输入矩阵改为代表项、合并 CSV 导出断言后，当前为 39 个文件、325 个用例，较原基线减少 638 个（66.3%）。两个 `.ts` 纯状态测试的 71 个用例保留。草稿、图源身份、修订并发、生产锁、周期隔离、异步失败及完整导出等保护仍保留。

| 范围 | 处理 |
| --- | --- |
| workflow 本地交互原型 | 删除整页预览、建站/配方/生产操作四个套件及专用夹具；纯状态模型保留，局部组件只留输入、重入/取消与焦点 |
| 配方与示教 | 删除默认参数页的完整模拟套件，编辑器/采集/示教/验证/标定等收缩为关键约束；完整创建与示教各用一个流程验证 |
| 在线、历史与设置 | 删除展示状态、筛选文案、字段接线等矩阵；保留切型/复位锁、cycleId 隔离、过期请求、双分区保存与失败恢复 |
| PLC 与设备 | 删除重复页面加载/预设展示/配置操作；保留写入权限、生产锁、地址/连接输入解析、来源切换和日志完整导出 |

删掉的行为中，部分只是降低自动回归范围，并不声称与保留用例等价。完整配置矩阵、布局、原型场景切换/背景上传等不再由 UI 单元测试保证；复杂业务验收需要有真实数据的桌面测试。

## 覆盖率

已移除全部全局和单文件覆盖率门槛。`test:coverage` 继续统计整个 `src`，未缩小统计范围或把未执行页面隐藏。报告仅供排查，不作为 UI 交付要求。

当前报告：语句 63.71%、分支 58.92%、函数 53.71%、行 70.82%。下降来自主动删减页面和原型流程自动覆盖。

## 本轮验证

| 检查 | 结果 |
| --- | --- |
| 前端全量及覆盖率报告 | 39 文件，325/325 通过 |
| `pnpm test:ui` | 两个完整前端流程，2/2 通过 |
| 隔离桌面生产流程 | 正常工件 OK，6 帧、460 测量点、6 张真实 PGM；独立 SQLite 读回、元数据及历史页面原图解码通过 |
| TypeScript 应用/测试 | 均通过 |
| 生产前端与隔离 Tauri 构建 | 通过；现有构建警告未作为失败或测试证据 |
| Git 差异空白检查 | 通过 |
| 一键三流程与清理 | `pnpm test:ui:all` 通过，测试进程已自动关闭 |
| 启动失败清理 | 注入 `instance.json` 写入故障，启动失败后无遗留测试进程；恢复后自定义端口复用测试通过 |
| 项目依赖 | 带空格的新目录冻结锁文件全新安装与生产测试模块加载通过，使用项目 Playwright Core 1.64.0 |

本机原始基线为 `tmp/ui-lightweight-before.json`，首次精简结果为 `tmp/ui-lightweight-final.json`，继续精简后的全量结果为 `tmp/ui-lightweight-round2.json`。一键三流程日志为 `tmp/ui-portable-flows.log`。桌面流程报告和截图为 `output/playwright/flow-regression/result.json`、`production-flow.png`；SQLite 与原图位于独立 `com.xyzrobotics.tujiaovision.flow-tests` 数据目录。本轮没有运行真实设备或 LyFlow 图像引擎验收。

## 运行

```powershell
pnpm test
pnpm test:ui
pnpm test:coverage
pnpm typecheck
pnpm build
```

新机器先安装 Node 24.15+、pnpm 11.1.3。真实桌面落盘流程还需要 Windows、PowerShell 7、Rust MSVC 工具链、Visual Studio C++ 构建工具和 WebView2，均为 Tauri 开发所需环境；首次 Cargo 构建需要能读取 `Cargo.toml` 中的私有 Git 依赖。Playwright Core 1.64.0 随项目锁文件安装，按[官方 WebView2 接入方式](https://playwright.dev/docs/webview2)连接应用 WebView2，无需下载 Playwright 浏览器。无需本机图片、外部 DLL 或个人 npm 缓存。

```powershell
pnpm install --frozen-lockfile
pnpm test:ui:all
```

`test:ui:all` 顺序运行两个前端流程，再自动构建、启动隐藏隔离实例、运行落盘测试，最后核对进程路径与启动时间并关闭本次启动的实例；失败时也执行关闭。单跑生产流程使用 `pnpm test:ui:production`；复用已构建程序使用 `pnpm test:ui:production -SkipBuild`，端口冲突可传 `-DebugPort 9349`。这些命令不需要任何绝对路径参数。

脚本检查应用标识、隔离目录、模拟设备来源与本机调试端口。配置和启动经真实 UI，后端接口仅用于读取断言。SQLite、原图、运行报告及截图均属于本机测试产物，不提交。隔离配置与历史会留给后续复核；该 profile 仅用于这套模拟流程，不导入外部相机或生产配置。

如需调试已有隔离实例，仍可执行 `node tests/native/production-flow-ui.mjs --cdp http://127.0.0.1:9339 --instance output/playwright/flow-regression/instance.json --output output/playwright/flow-regression/result.json`。旧 P0 专项 `.mjs` 也统一使用项目 Playwright Core；其中真实引擎与回放验收仍须另备对应 DLL 和图片，不属于这三条主流程的无外部样本运行范围。

Windows 沙箱依赖解析、测试临时目录及主机权限处理见 [测试说明](TESTING.md)。本轮实际执行使用 Vitest `threads` / 2 workers 和独立的工作区 TEMP/TMP；类型检查在主机上下文通过，未修改类型检查规则来规避环境错误。
