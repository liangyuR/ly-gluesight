# Windows 打包与 GitHub 构建

交付 Windows x64 NSIS `setup.exe`，携带 lyFlow、MVS 和必要的 VC++ 运行库。MVS 设备驱动及 WebView2 由目标电脑预装；安装程序不携带或下载 WebView2，不安装设备驱动。目标电脑无需 Node.js、pnpm、Rust 或完整 MVS 开发工具。

## 本机打包

构建机需要 PowerShell 7、项目要求的 Node.js/pnpm、Rust x64 MSVC 工具链、Visual Studio C++ Build Tools 和 Windows SDK。首次构建需要联网获取依赖和 NSIS 工具。

先准备正式 lyFlow 运行产物目录，包含当前应用所需 DLL、许可证及 `lyflow-runtime.json`：

```json
{
  "abiVersion": 15,
  "engineVersion": "与 lyflow_version 接口返回值一致的实际版本",
  "sourceRevision": "正式构建对应的明确源码修订号"
}
```

ABI 字段是产物提供方的声明，不是动态探测结果。打包还会调用应用真实的图像输入、胶路测量和标定流程验证兼容性。必须具备 `io.load_image`、`image.board_calib`、`image.load_calib`、`glue.taught_path`、`glue.bead_width` 算子。脚本不构建其他仓库，不自动寻找或采用历史临时产物。

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

报告目录需事先存在。成功退出码为 0；失败为非零并记录原因。验证覆盖合成胶条正常测量、空白图缺胶、新帧偏移、棋盘格标定与空白图拒绝，以及随包 MVS 加载和枚举。它证明软件运行组件及接口可用，不代表真机触发收图、现场精度或长稳验收。

当前实现的首版依赖包仍等待正式 lyFlow 产物。脚本、工作流和失败路径可独立验证，但在正式产物提供并完成全流程前，不能把代码回归标为已生成并验收首个安装包。
