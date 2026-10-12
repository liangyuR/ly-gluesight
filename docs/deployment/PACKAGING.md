# GlueSight Windows 发包

## 固定输入与兼容系列

应用版本由 `package.json` 管理，Tauri、Cargo 和 Cargo.lock 根包必须一致。`pnpm version:set 0.1.1` 显式升版，`pnpm version:check` 只做检查。正式 tag 为 `vX.Y.Z`，必须与应用版本一致并位于 main 历史中。历史 Release 和同版本资产禁止覆盖。

LyFlow 通过 GitHub Packages 的 `LyFlow.Runtime.win-x64` 原生 NuGet 包分发。`packaging/runtime.json` 声明源、包 ID、兼容系列 `1.1`、最低版本和必需算子；`packaging/lyflow.lock.json` 记录完整版本、架构及源码修订。普通开发、CI 和发包只恢复锁定版本，不查询最新版本。

```powershell
pnpm deps:lyflow:update
pnpm deps:lyflow:update -Version 1.1.0
pnpm deps:lyflow:restore
```

更新命令从标准 NuGet v3 PackageBaseAddress 版本索引选择 `1.1.x` 正式补丁版本，同时更新运行库锁定记录、Rust 客户端 Git rev、Cargo.toml 和 Cargo.lock；失败恢复原文件。跨到 `1.2` 或 `2` 必须显式修改系列并重新验收。客户端与 DLL 的完整 ABI/产品版本继续严格相等；升级后重新编译并发布 GlueSight，不支持只替换已安装程序的 DLL。

MVS `4.8.1.2`、许可证与应用所需 CRT 独立固定在 `deps-win-x64-v2` 的 `gluesight-vendor-runtime-win-x64-v2.zip`。该包不包含 lyFlow。旧合并依赖包拒绝读取，不做格式回退。应用版本、lyFlow 版本与厂商包版本独立。

## 认证与缓存

本机默认从 `$USERPROFILE/.ssh/github_nuget_auth.txt` 读取凭证，也可通过 `LYFLOW_NUGET_TOKEN` 提供；用户名通过 `LYFLOW_NUGET_USERNAME` 提供，默认 `liangyuR`。凭证仅用于认证，不写入仓库、缓存或产物。GitHub Actions 使用包授权后的只读 `GITHUB_TOKEN`，或仓库 Secret `LYFLOW_NUGET_TOKEN`。NuGet 包须给 GlueSight 仓库配置 Actions 读取权限。

NuGet CLI 固定为官方 `6.14.0`，校验版本与微软签名。依赖缓存在 `src-tauri/target/dependency-cache`，按 NuGet 包 ID、完整版本、架构或厂商 tag/资产身份隔离。下载在临时目录进行，验证完整后才移动到正式缓存目录；命中后仍校验清单及实际 DLL。

NuGet 下载默认总期限 1200 秒，每 30 秒显示等待进度。慢链路可通过 `LYFLOW_NUGET_TIMEOUT_SECONDS` 调整为 30 至 3600 秒；超时清理本次临时下载，不建立完整版本缓存。版本查询只接受同一 HTTPS 源及 owner 路径的元数据地址，禁止自动跳转认证请求。

CI 分别缓存 pnpm、Cargo、NuGet 运行库、厂商依赖和 Tauri NSIS 工具。最终应用、安装包、组装运行库与验收报告不作为缓存复用。手动运行 `Windows package` 时设置 `use_cache=false` 可跳过全部缓存恢复和保存。

不计算发布包或引擎文件的内容哈希。校验使用版本、Git 修订、文件清单、尺寸、路径、架构与 DLL 实际接口和算子。

## 本机打包与厂商包维护

```powershell
pnpm install --frozen-lockfile
pnpm package
```

默认恢复锁定 NuGet 包和厂商包，再完成前端、Rust、NSIS 构建与静默安装验证。不搜索相邻 LyFlow 构建目录，也不自动使用系统 MVS。诊断构建可显式提供 `-LyFlowRuntime`、`-MvsRuntime`、`-VcRuntime` 和 `-MvsLicense`，仍执行锁定版本及实际组件检查。

厂商运行库变更时增加 `bundleVersion`、tag 和资产名，并从明确路径导出、验证和发布，禁止覆盖旧资产：

```powershell
pnpm package:deps -MvsRuntime 'C:\release\mvs\Win64_x64' -MvsLicense 'C:\release\mvs\License' -VcRuntime 'C:\release\crt'
pnpm package:deps:publish -Archive 'C:\release\gluesight-vendor-runtime-win-x64-v2.zip'
```

厂商包保留 MVS DLL、CTI、配置、ThirdParty 和许可证；CRT 目录仅容纳可分发 CRT DLL 与许可文件。安装资源保持 `runtime/lyflow`、`runtime/mvs`，另有 `runtime/crt`；应用所需 `vcruntime140.dll` 和 `vcruntime140_1.dll` 同时部署到 EXE 根目录。WebView2 和设备驱动仍由目标机预装。

## CI 与发布门禁

普通 PR 和 main 执行类型检查、前端测试/构建、发包脚本测试及 Rust 回归。影响发包、依赖配置、锁文件、工具链或 workflow 的 PR 额外执行完整 Windows 安装验证。手动或 tag 触发完整发包，只有 tag 成功后创建草稿 Release；负责人复核后正式发布。

构建使用固定 Node `24.15.0`、pnpm `12.11.2`、Rust `1.99.0`。workflow 保留显式 pnpm、Cargo、Tauri 标准命令，版本及依赖准备脚本与本机共用。构建任务只读，创建草稿 Release 的任务单独获得内容写权限。

完整门禁覆盖：干净 SDK 环境、固定包结构/架构/版本/源码修订、实际 DLL 算子、应用无界面自检、NSIS 静默安装及安装后的运行库和应用自检。CI 使用英文含空格的安装目录。安装包产品名及默认目录名使用 `GlueSight`，应用窗口保留中文名称。

当前 MVS SDK 使用 ANSI 路径接口；运行库路径应能由 Windows 当前系统代码页表示。本机 UTF-8 系统代码页下中文路径检查通过，GitHub runner 的中文安装目录发生 `MV_E_LOAD_LIBRARY`，路径编码限制是疑似原因。本轮不声明跨代码页的中文路径支持，也不将该错误当作无设备忽略。

产物包含应用安装包、运行库清单、验证报告与 `release-manifest.json`，记录应用修订、依赖版本/来源、工具版本、缓存命中及各阶段耗时。成功与失败均保留诊断资料。GitHub 冷、热两次完整 workflow 才能证明缓存服务的实际收益；本机缓存检查不能替代该证据。

软件自检覆盖合成图像胶路测量、空白图拒绝、偏移与棋盘格标定及 MVS 加载/枚举，不代表真机触发收图、现场精度或长稳验收。可使用安装后的无界面入口：

```powershell
& 'C:\Apps\GlueSight\GlueSight.exe' --check-packaged-runtime 'C:\reports\runtime.json'
```
