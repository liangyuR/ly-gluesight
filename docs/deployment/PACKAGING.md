# Windows 打包与 GitHub 构建

交付 Windows x64 NSIS `setup.exe`，携带 lyFlow、MVS 和必要的 VC++ 运行库。MVS 设备驱动及 WebView2 由目标电脑预装；安装程序不携带或下载 WebView2，不安装设备驱动。目标电脑无需 Node.js、pnpm、Rust 或完整 MVS 开发工具。

## 本机打包

构建机需要 PowerShell 7、项目要求的 Node.js/pnpm、Rust x64 MSVC 工具链、Visual Studio C++ Build Tools 和 Windows SDK。首次构建需要联网获取依赖和 NSIS 工具。

先准备正式 lyFlow 运行产物目录，包含当前应用所需 DLL、许可证及 `lyflow-runtime.json`：

```json
{
  "abiVersion": "1.1.0",
  "engineVersion": "1.1.0",
  "sourceRevision": "正式构建对应的完整40位Git源码修订号",
  "localValidationOnly": false
}
```

ABI 跟随 lyFlow 产品版本，必须为字符串。当前固定 `1.1.0`：产物描述、依赖清单、DLL 的 `lyflow_abi_version_string()` 与 `lyflow_version()`，以及应用所用客户端 `ABI_VERSION` 必须完全一致；数字 `15`、字符串 `"15"`、其他版本或缺少新 ABI 接口的旧 DLL 均直接拒绝，不做转换或旧 ABI 回退。`localValidationOnly` 必须显式为 `false`；DLL 的 `lyflow_source_revision()`、描述与清单必须为完全相同的 40 位 Git 修订，空修订或 dirty/本机诊断候选不得进入 GS 依赖包。脚本先读取 DLL 的实际 ABI/引擎版本，再调用测量相关接口；应用自检通过后还会核对客户端版本。打包继续调用应用真实的图像输入、胶路测量和标定流程验证兼容性。必须具备 `io.load_image`、`image.board_calib`、`image.load_calib`、`glue.taught_path`、`glue.bead_width` 算子。脚本不构建其他仓库，不自动寻找或采用历史临时产物。

```powershell
pnpm install --frozen-lockfile
$env:LYFLOW_RUNTIME_DIR = 'C:\release\lyflow\bin'
pnpm run package
```

MVS 默认从本机 `Common Files\MVS\Runtime\Win64_x64` 收集，VC++ CRT 通过 `vswhere` 查找。当前固定 MVS 版本为 `4.8.1.2`。参数优先于对应环境变量：

```powershell
pnpm run package -LyFlowRuntime 'C:\release\lyflow\bin' -MvsRuntime 'C:\release\mvs\Win64_x64'
```

其他可选参数：`-VcRuntime` / `VC_RUNTIME_DIR` 指定 x64 CRT；`-MvsLicense` 指定官方许可证目录；`-PrepareOnly` 仅收集并自检运行库，不编译或生成安装包。

MVS 完整保留运行目录及子目录，包括 DLL、CTI、配置文件、manifest 和 `ThirdParty`，许可证放入 `runtime/mvs/Licenses`。lyFlow 收集运行依赖，排除调试符号、开发数据、配方和历史记录。两套运行库分别部署到 `runtime/lyflow`、`runtime/mvs`。

脚本每次重建项目内暂存目录，先检查文件结构、x64 架构、接口、实际版本、算子及 MVS 初始化/枚举，再编译应用、验证实际测量，全部通过后生成安装包。无相机时枚举结果为空允许通过；接口调用失败不允许通过。任一步失败都会停止。

默认产物位于 `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/`。设置 `CARGO_TARGET_DIR` 时按指定构建目录输出。`pnpm build` 仅构建前端；直接 `tauri build` 不执行运行库收集和完整交付检查。

## 固定依赖包

云端 runner 无法读取本机 DLL，使用本仓库公开的固定依赖 Release。`packaging/runtime.json` 记录依赖版本、tag、资产名、MVS 版本、客户端 ABI 和必需组件。首版为 `deps-win-x64-v1` / `gluesight-runtime-win-x64-v1.zip`。

在准备好正式产物的本机执行：

```powershell
$env:LYFLOW_RUNTIME_DIR = 'C:\release\lyflow\bin'
pnpm run package:deps
pnpm run package:deps:publish -Archive '.\src-tauri\target\dependencies\gluesight-runtime-win-x64-v1.zip'
```

导出命令不联网上传。发布命令需要本机已安装并登录 GitHub CLI，明确执行后才创建公开的预发布依赖 Release，不覆盖既有 Release 或资产。依赖更新必须使用新的版本、tag 和资产名，并提交配置变更。发布前确认所收集的正式组件和许可证适合公开分发。

ZIP 根目录为 `runtime/`，包含两套运行库和 `manifest.json`。清单记录明确版本、修订、相对路径和尺寸，不使用内容摘要匹配；旧的带内容摘要字段的记录直接拒绝。下载不使用 `latest`，解压前拒绝越界路径、重复条目和链接。

也可以使用已解压的固定依赖包在本机打包：

```powershell
pnpm run package -RuntimeBundle 'C:\release\runtime-bundle'
```

此模式不混用本机来源参数或相应环境变量，仍执行相同验证。

## GitHub Actions

- **Windows CI**：PR、main 提交和手动运行执行前端类型检查、测试、生产构建、打包脚本专项测试和 Rust 非忽略回归；不要求相机或 MVS 驱动。
- **Windows package**：手动运行或推送 `vX.Y.Z` tag 时，从固定依赖 Release 下载 ZIP，调用同一本地打包脚本，完成静默安装及安装后的自检。依赖 Release 不存在时明确失败，不采用其他版本。
- 手动打包上传安装包、依赖清单和验证报告，保留 14 天。版本 tag 必须与 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 一致；验证成功后创建草稿 Release，由负责人检查后正式发布。

使用 GitHub 托管 `windows-2022` runner。Node.js/pnpm 版本读取项目声明，使用锁文件安装依赖；Rust 使用 stable。源码依赖仓库目前公开，CI 将 SSH GitHub 地址重写为 HTTPS，无需读取 Token。构建任务只读，创建草稿 Release 的任务单独获得写权限。

## 安装行为与验证边界

安装支持简体中文/英文，沿用当前用户安装方式，目前未配置代码签名或自动更新。安装包不携带开发机设备配置及生产数据。

系统设置的 lyFlow 核心库路径留空时使用内置引擎，已有手动路径继续生效。MVS 发布版固定使用随包运行库，缺失或失败直接报错，不回退到系统已安装的 MVS；开发版可使用系统安装目录。

应用提供无界面验证入口，不启动 Tauri 界面或生产采集：

```powershell
& 'C:\Apps\GlueSight\GlueSight.exe' --check-packaged-runtime 'C:\reports\runtime.json'
```

报告目录需事先存在。成功退出码为 0；失败为非零并记录原因。脚本报告分别记录 DLL 实际 `abiVersion`、描述中的 `declaredAbiVersion` 和引擎 `version`；应用报告记录 `clientAbiVersion`、实际版本与加载路径。验证覆盖合成胶条正常测量、空白图缺胶、新帧偏移、棋盘格标定与空白图拒绝，以及随包 MVS 加载和枚举。它证明软件运行组件及接口可用，不代表真机触发收图、现场精度或长稳验收。

应用首包版本保持 `0.1.0`，与 lyFlow 的 `1.1.0` 独立；应用的三处版本及 Cargo.lock 根包版本保持一致。历史 PR #26 合入旧打包分支，未进入 main；首次发包前必须补齐该 ABI 集成，不能仅根据 PR 的 merged 状态判断 main 已具备新版接口。

2026-10-12 核对并下载了 lyFlow [Windows release 38150280499](https://github.com/liangyuR/lyFlow/actions/runs/38150280499) 的 `windows-release` 产物，运行库 ZIP 为 `lyflow-runtime-win-x64-v1.1.0.zip`，源码修订为 `c625155c480a1cc7511f966a9fe234700f6c1404`。实际 DLL 的 ABI、引擎版本与正式描述、源码修订校验通过，随后依赖导出因缺少 `glue.taught_path` 明确失败。失败报告位于本机 `src-tauri/target/dependency-runtime/runtime-validation.json`；没有生成或发布固定依赖 ZIP、安装包或应用 Release。

需要提供包含全部必需算子的正式产物，并重新执行依赖导出、真实 DLL 视觉业务回归及 GitHub 完整打包流程。普通 Rust 回归与打包脚本专项通过不能替代此项产物验收；在缺失算子解决前，不应创建应用版本 tag 或宣称安装包可用。
