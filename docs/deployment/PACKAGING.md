# GlueSight Windows 发包

## 固定输入与兼容系列

应用版本由 `package.json` 管理，Tauri、Cargo 和 Cargo.lock 根包必须一致。`pnpm version:set 0.1.1` 显式升版，`pnpm version:check` 只做检查。正式 tag 为 `vX.Y.Z`，必须与应用版本一致并位于 main 历史中。历史 Release 和同版本资产禁止覆盖。

lyFlow 运行库来自公开仓库 `liangyuR/lyFlow` 的 GitHub Release 资产 `lyflow-runtime-win-x64-v<版本>.zip`，下载无需凭证。`packaging/runtime.json` 的 `lyflow` 段只锁系列（`{ "repository": "liangyuR/lyFlow", "series": "1.1" }`），开发与发包都取该系列最新正式 Release；普通 PR CI 不恢复运行库。`src-tauri/Cargo.toml` 的 `lyflow-client` 用同系列 tag 锁定。来源、校验项、缓存与升级见 [lyFlow 运行库依赖](../architecture/lyflow-dependency.md)。

```powershell
pnpm deps:lyflow:restore
pnpm deps:lyflow:update
pnpm deps:lyflow:update -Series 1.2
```

跨系列升级必须显式传 `-Series` 并重新验收；升级后重新编译并发布 GlueSight，不支持只替换已安装程序的 DLL。

MVS SDK `4.8.2.2`（取自 MVS 客户端 V5.1.0 安装包内的 SDK 运行库）、许可证与应用所需 CRT 独立固定在 `deps-win-x64-v3` 的 `gluesight-vendor-runtime-win-x64-v3.zip`。现场预装 MVS 客户端 V5.1.0 提供设备驱动，与随包 SDK 同版本。该包不包含 lyFlow。旧合并依赖包拒绝读取，不做格式回退。应用版本、lyFlow 版本与厂商包版本独立。

## 缓存

依赖缓存在 `src-tauri/target/dependency-cache`：lyFlow 按版本放在 `lyflow/<版本>/`，厂商包按 tag/资产身份隔离。下载在临时目录进行，校验通过才移动到正式缓存目录；命中后仍校验描述文件及实际 DLL。查询或下载 lyFlow 因网络失败时，退回系列内最高的已缓存且校验通过的版本并告警。

发包 CI 先解析 lyFlow 系列最新版本，再以版本为键缓存 `dependency-cache/lyflow`；另分别缓存 pnpm、Cargo、厂商依赖和 Tauri NSIS 工具。最终应用、安装包、组装运行库与验收报告不作为缓存复用。手动运行 `Windows package` 时设置 `use_cache=false` 可跳过全部缓存恢复和保存。

不计算发布包或引擎文件的内容哈希，也不锁定 Git 提交。校验使用版本、系列、文件清单、尺寸、路径、架构与 DLL 实际接口和算子。

## 本机打包与厂商包维护

```powershell
pnpm install --frozen-lockfile
pnpm package
```

默认恢复 lyFlow 系列最新运行库和厂商包，再完成前端、Rust 与 NSIS 构建（不做安装验证）；实际打包的 lyFlow 版本写入 `runtime/manifest.json` 与 `release-manifest.json`。不搜索相邻 lyFlow 构建目录，也不自动使用系统 MVS。诊断构建可显式提供 `-LyFlowRuntime`、`-MvsRuntime`、`-VcRuntime` 和 `-MvsLicense`，仍执行系列版本及实际组件检查。

厂商运行库变更时增加 `bundleVersion`、tag 和资产名，并从明确路径导出、验证和发布，禁止覆盖旧资产：

```powershell
pnpm package:deps -MvsRuntime 'C:\release\mvs\Win64_x64' -MvsLicense 'C:\release\mvs\License' -VcRuntime 'C:\release\crt'
pnpm package:deps:publish -Archive 'C:\release\gluesight-vendor-runtime-win-x64-v3.zip'
```

厂商包保留 MVS DLL、CTI、配置、ThirdParty 和许可证；CRT 目录仅容纳可分发 CRT DLL 与许可文件。安装资源保持 `runtime/lyflow`、`runtime/mvs`，另有 `runtime/crt`；应用所需 `vcruntime140.dll` 和 `vcruntime140_1.dll` 同时部署到 EXE 根目录。WebView2 和设备驱动仍由目标机预装。

## CI 与发布门禁

普通 PR 和 main 仅执行 Rust 格式检查、前端单元测试与 Rust 库单元测试，具体命令见 [测试记录与复现](../testing/TESTING.md#windows-ci-与手动验收)。Windows 发包工作流仅由手动运行或 `v*` tag 触发，PR 不自动执行完整发包门禁。只有 tag 成功后创建草稿 Release；负责人复核后正式发布。

构建使用固定 Node `24.15.0`、pnpm `12.15.0`、Rust `1.99.1`。workflow 保留显式 pnpm、Cargo、Tauri 标准命令，版本及依赖准备脚本与本机共用。构建任务只读，创建草稿 Release 的任务单独获得内容写权限。

完整门禁覆盖：干净 SDK 环境、固定包结构/架构/版本、lyFlow 系列与实际 DLL 版本、实际 DLL 算子、应用无界面自检与 NSIS 安装包生成，不做静默安装验证。软件约定安装在英文目录。安装包产品名及默认目录名使用 `GlueSight`，应用窗口保留中文名称。

产物包含应用安装包、运行库清单、验证报告与 `release-manifest.json`，记录应用修订、依赖版本/来源、工具版本、缓存命中及各阶段耗时。成功与失败均保留诊断资料。GitHub 冷、热两次完整 workflow 才能证明缓存服务的实际收益；本机缓存检查不能替代该证据。

软件自检覆盖合成图像胶路测量、空白图拒绝、偏移与棋盘格标定及 MVS 加载/枚举，不代表真机触发收图、现场精度或长稳验收。可使用安装后的无界面入口：

```powershell
& 'C:\Apps\GlueSight\GlueSight.exe' --check-packaged-runtime 'C:\reports\runtime.json'
```
