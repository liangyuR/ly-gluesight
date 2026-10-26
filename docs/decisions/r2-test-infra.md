# 隔离实例与桌面测试设施决策记录（r2-test-infra）

范围：`scripts/Start-UiTestInstance.ps1`、`scripts/Test-*.ps1` 桌面测试入口、`tests/native/` 与 `tests/desktop/` 脚本，以及让桌面测试稳定跑通所需的最小产品修复（启动时 WebView2 资料目录长度校验、隐藏窗口启动失败退出）。运行方式见 [测试记录与复现](../testing/TESTING.md)。

## 1. 隔离实例不再继承调用方的输出管道

- 问题：`Start-UiTestInstance.ps1` 用 `Start-Process -RedirectStandardOutput/-RedirectStandardError` 启动程序。PowerShell 在重定向时以“继承全部可继承句柄”调用 CreateProcess，GUI 程序连带继承了调用方的 stdout/stderr 管道。调用方（后台任务、`$out = pwsh -File …`、CI）要等管道 EOF，实例不退出脚本就不算结束。
- 可选项：(a) 启动前临时清除 pwsh 标准句柄的可继承标志，再用 `Start-Process`；(b) 用 `cmd /c` 包一层重定向，再按父进程找实际 PID；(c) 自己调用 CreateProcess，用 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` 只让子进程继承 NUL 输入和两个日志文件句柄。
- 采用：(c)。(a) 只能覆盖标准句柄，调用方传入的其他可继承句柄仍会泄漏；(b) PID 要靠进程树反查，身份校验更绕。(c) 明确列出子进程可见的句柄，日志仍写 `stdout.log` / `stderr.log`，PID 直接取自 CreateProcess。
- 回退：把 `[GlueSightTest.IsolatedLauncher]::Start(...)` 换回原来的 `Start-Process ... -RedirectStandardOutput ... -RedirectStandardError ...`。

## 2. CDP 地址以实例清单为准，就绪标准改为“只剩主窗口页面”

- 问题：全新数据目录启动时，程序先建“资料准备”窗口，资料就绪后再建主窗口并关闭准备窗口。两个窗口的 WebView2 资料目录不同（`migration/bootstrap/webview/<SID>` 与 `webview/<SID>`），是两个浏览器进程，却拿到同一个 `--remote-debugging-port`：先启动的准备窗口占用 `127.0.0.1:<端口>`，主窗口的浏览器回落到 `[::1]:<端口>`。旧脚本只要任一地址能回应 `/json/version` 就算就绪，测试再用 `localhost` 连接，可能连到准备窗口所在的进程；准备窗口一关就报 `Target page closed`，或在那个进程里永远等不到主窗口页面。已初始化的数据目录重启时没有准备窗口，主窗口又回到 `127.0.0.1`。
- 可选项：(a) 继续用 `localhost` 并加重试；(b) 产品给准备窗口另配调试端口（调试端口经环境变量 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 对整个进程生效，不能按窗口区分，需要改产品启动参数）；(c) 让两个窗口共用一个资料目录（违背“初始化前目标目录只能有锁和 migration”的资料目录规则）；(d) 启动脚本同时轮询 `127.0.0.1` 与 `[::1]` 的 `/json/list`，直到出现且只出现主窗口页面、准备窗口页面已消失，再把主窗口所在地址写入 `instance.json` 的 `cdp`。
- 采用：(d)。只改测试设施，产品启动流程不变。测试脚本默认从清单读 `cdp`（`--cdp` 仍可显式覆盖），并核对端口与清单一致。就绪等待改为 120 s 截止时间；程序提前退出或超时都会附带 `stderr.log` 末尾 20 行。端口检查同时覆盖 IPv4 与 IPv6 回环。
- 回退：脚本里去掉 `cdp` 字段，各 Node 脚本恢复 `--cdp http://localhost:<端口>` 默认值。

## 3. 产品在启动时按实测临界值校验 WebView2 资料目录长度

- 问题：`GLUESIGHT_DATA_DIR` 较长时 WebView2 不创建窗口，wry 在等待 WebView2 控制器回调时一直阻塞主线程，进程挂起且没有任何日志。
- 实测（WebView2 Runtime 155.0.4283.62，用户 SID 46 字符，资料目录末级为 38 字符的测试标识）：

  | 场景 | 可用 | 挂起 |
  | --- | --- | --- |
  | 已初始化目录，只建主窗口（资料目录 `webview/<SID>`） | 主窗口资料目录 229 字符 | 230 字符 |
  | 全新目录，先建准备窗口（资料目录 `migration/bootstrap/webview/<SID>`） | 准备窗口资料目录 237 字符 | 238 字符 |

  两个临界点分别对应 Windows CreateDirectory 默认 248 字符上限下 `<资料目录>\EBWebView\Default`（主窗口需要建出浏览器 Default 配置目录）和 `<资料目录>\EBWebView`（准备窗口页面只需 EBWebView 根目录）能否建出。按资料目录（`GLUESIGHT_DATA_DIR`）换算，实测全新启动 ≤ 162 字符、已初始化目录 ≤ 174 字符可用（SID 46 字符时）。
- 可选项：(a) 只在测试脚本里检查；(b) 产品在 `storage::prepare` 阶段对两个 WebView2 资料目录统一用 229 上限；(c) 产品在 `storage::prepare` 阶段按窗口分别用实测临界值，且只检查本次启动会建的窗口；(d) 缩短 `migration/bootstrap` 或 `webview/<SID>` 的资料布局（改资料目录结构，影响迁移与其他工作线）。
- 采用：(c)。主窗口资料目录上限 `247 - len("\EBWebView\Default") = 229`，准备窗口资料目录上限 `247 - len("\EBWebView") = 237`，都按 UTF-16 字符数计（中文路径同样按一个字符计），各自恰好到实测可用的最大值。主窗口每次启动都建，总是检查；准备窗口只在资料目录尚未初始化（没有 `storage-layout.json`）时建，也只在这时检查，判断条件与 `lib.rs` 决定是否建准备窗口共用 `storage::initialized`。(b) 规则更单一，但会把准备窗口资料目录 230–237 字符这类实际能用的目录也拒绝掉，已初始化的目录重启时根本不建准备窗口，却同样被这条规则拒绝，错误信息“窗口无法创建”也与事实不符，所以不用。超限时 `prepare` 返回错误（含是哪个窗口、资料目录路径、实际长度与上限），走现有的启动失败路径：可见窗口显示资料准备错误页，隐藏窗口见第 4 条。正式版 `%ProgramData%\ly\GlueSight` 的准备窗口资料目录约 102 字符，远低于上限。
- 边界：只检查启动必需的目录。未超上限时，`EBWebView\Default\Network`、`Code Cache` 等更深的缓存目录仍可能建不出来，WebView2 将其视为非致命错误（实测可正常运行），不影响功能。WebView2 版本更新若改变临界值，测试启动脚本的 120 s 就绪超时会暴露。
- 回退：删除 `storage::check_webview_profiles` 及其在 `prepare` 中的调用。

## 4. 隐藏窗口（测试实例）启动失败时写 stderr 并以退出码 1 结束

- 问题：测试配置的窗口全部隐藏，资料准备窗口也随之隐藏。启动失败（数据目录独占锁被占、路径过长、资料初始化失败等）时，错误只显示在看不见的准备窗口里，进程停在等待用户点击，看起来就是挂起。`p0-single-instance-acceptance.mjs` 期望重复启动以非零退出码结束并输出“独占锁”；按 10-18 引入资料准备窗口后的代码，重复启动会停在隐藏的错误页而不退出。
- 采用：所有窗口都配置为隐藏时，启动失败先 `eprintln!` 原因，再以退出码 1 结束；可见窗口（正式版、开发版）的行为不变，仍显示资料准备错误页。原有失败路径只在资料初始化线程里写 stderr，现在 `prepare` 阶段的失败也写。
- 回退：去掉 `lib.rs` 中 `headless` 分支。

## 5. 测试资料根目录可配置：`-DataRoot` / `GLUESIGHT_TEST_DATA_ROOT`

- 问题：隔离实例的资料目录固定在 `output/playwright/<profile>-regression/<测试标识>`。主检出下约 107 字符，工作树（`.claude/worktrees/<名称>/`）下约 143 字符，再深一点或换更长的检出路径就超过第 3 条的上限。
- 采用：`Start-UiTestInstance.ps1` 新增 `-DataRoot`，未传时读环境变量 `GLUESIGHT_TEST_DATA_ROOT`，都没有时仍用原输出目录。资料目录 = `<DataRoot>\<测试标识>`，产品仍校验末级名称等于测试标识；`-ResetData` 只清理这一目录（父目录必须等于 DataRoot，且整条路径不含链接）。`Test-UiProductionFlow.ps1` 原样透传。不自动回落到临时目录：资料去向必须是调用方明确指定的位置，避免不同检出共用同一测试资料。
- 回退：去掉 `-DataRoot` 参数，资料目录恢复为输出目录下。

## 6. 生产落盘测试经侧栏进入离线控制台

- 问题：顶栏没有站点模式链接，测试不能再点它进入离线控制台。
- 采用：经侧栏“操作导航 → 离线模式”进入离线控制台，用控制台标题“当前：真机”“当前：离线 · 合成图”判断模式。断言含义不变：界面显示的模式与后端 `site_status` 一致。
- 回退：不适用（顶栏链接已不存在）。

## 9. 生产落盘接入 Playwright Test

- 问题：普通 Node CLI 自行组织生命周期、超时和证据，失败阶段不易定位。
- 采用：生产落盘（`pnpm test:ui:production`）用独立 Playwright Test 配置管理用例及报告，固定单 worker、无整条自动重跑、单用例 15 分钟。PowerShell 负责预检、公共构建及同 profile 执行期互斥；夹具复用原启动设施和身份核对，管理 CDP context、失败截图、trace 及最终关闭。构建与启动分离为 `Build-UiTestInstance.ps1` / `Start-UiTestInstance.ps1`，后者可独立使用。用例只有合成图段配方（`-Suite burst`）；无运行库时明确跳过，显式给出的坏运行库在构建前失败。
- 证据：业务 JSON 和实例日志路径不变；框架只管理 `flow-regression/runner/{html,junit.xml,test-results}`，不清理业务目录，取证失败不得覆盖业务错误。
- 回退：框架报告与业务数据独立，无资料迁移；依赖现场样本集的桌面测试已删除，只能从 Git 历史查看。
