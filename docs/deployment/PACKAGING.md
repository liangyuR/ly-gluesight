# Windows 安装包

在 Windows 项目根目录运行：

```powershell
pnpm install --frozen-lockfile
pnpm run package
```

命令自动收集 lyFlow 及其依赖 DLL、Visual C++ 运行库，校验 DLL 加载和示教胶路/标定算子，再构建生产前端、编译 Rust Release 程序，生成 NSIS `.exe` 安装包。任一步失败都会停止，不会生成缺少引擎的安装包。

默认产物位于 `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/`，文件名包含产品名、版本号、架构和 `setup.exe`。如设置了 `CARGO_TARGET_DIR`，以命令最后输出的安装包路径为准。`pnpm build` 仍然只构建前端。发包请使用 `pnpm run package`，直接运行 `tauri build` 不会收集引擎。

## 构建环境

- Windows x64 PowerShell 7、Node.js、pnpm、Rust x64 MSVC 工具链、Visual Studio C++ Build Tools 和 Windows SDK。
- 能访问 Cargo 依赖中的私有 Git 仓库；首次构建需要联网下载依赖、NSIS 等打包工具。
- 固定生成 Windows x64 安装包。

## 引擎来源

默认从相邻仓库 `../LyFlow/build/core/bin` 收集全部 DLL，通过 Visual Studio 的 `vswhere` 自动找到最新 x64 CRT。构建机需要先准备与当前客户端兼容、启用 glue 算子的 lyFlow 产物（Image ABI v15），必须包含 `io.load_image`、`image.board_calib`、`image.load_calib`、`glue.taught_path`、`glue.bead_width`；发包时自动部署这些产物，不重新编译 C++ 引擎。当前已验证的开发产物位于 `LyFlow/.claude/worktrees/glue-taught-path/build/core/bin`，可通过下列来源参数选择；旧版仅有定位/卡尺算子的 DLL 不满足当前配方。实际引擎验证见 [测试说明](../testing/TESTING.md)。

自定义来源可使用环境变量或命令参数：

```powershell
$env:LYFLOW_RUNTIME_DIR = 'D:\release\lyflow\bin'
$env:VC_RUNTIME_DIR = 'D:\release\Microsoft.VC145.CRT'
pnpm run package
```

```powershell
pnpm run package -LyFlowRuntime 'D:\release\lyflow\bin'
```

运行库暂存到 `src-tauri/target/package-runtime/lyflow`，每次发包都会重新收集，防止旧 DLL 混入；不复制 PDB、测试程序或开发数据。安装包将这批文件放到程序目录的 `runtime/lyflow`，包含逐文件 SHA-256 的 `manifest.json`，方便核对发包内容。

## 安装和使用

双击生成的 `setup.exe`，可选择简体中文或英文，默认安装到当前用户目录。目标电脑无需 Node.js、pnpm 或 Rust。

目标电脑需要 Microsoft Edge WebView2 Runtime。如果尚未安装，安装程序会联网下载并静默安装；离线电脑应提前部署 WebView2 Runtime。

安装包包含应用前端、桌面程序、lyFlow 及依赖 DLL、Visual C++ 运行库，无需手工复制引擎或单独安装 VC++ 运行库。系统设置的核心库路径留空时，自动使用内置引擎；已有手动路径继续优先使用，清空并保存即可切回内置版本。飞拍测量是否启用仍由原来的系统设置控制。

安装包不携带开发机的配方、历史记录或设备配置，也不包含相机驱动。海康真实相机需要另外安装对应的 MVS SDK/驱动。

当前未配置代码签名证书。发布新版时同步更新 `package.json`、`src-tauri/Cargo.toml` 和 `src-tauri/tauri.conf.json` 中的版本，并更新锁文件。
