# Windows 安装包

在 Windows 项目根目录运行：

```powershell
pnpm install --frozen-lockfile
pnpm run package
```

命令依次构建生产前端、编译 Rust Release 程序，再生成 NSIS `.exe` 安装包。任一步失败都会停止，不会使用旧的前端产物继续发包。

默认产物位于 `src-tauri/target/release/bundle/nsis/`，文件名包含产品名、版本号、架构和 `setup.exe`。如设置了 `CARGO_TARGET_DIR`，以命令最后输出的安装包路径为准。`pnpm build` 仍然只构建前端。

## 构建环境

- Node.js、pnpm、Rust MSVC 工具链、Visual Studio C++ Build Tools 和 Windows SDK。
- 能访问 Cargo 依赖中的私有 Git 仓库；首次构建需要联网下载依赖、NSIS 等打包工具。
- 默认使用当前 Rust 工具链的架构；常规 Windows x64 开发环境生成 x64 安装包。

## 安装和使用

双击生成的 `setup.exe`，可选择简体中文或英文，默认安装到当前用户目录。目标电脑无需 Node.js、pnpm 或 Rust。

目标电脑需要 Microsoft Edge WebView2 Runtime。如果尚未安装，安装程序会联网下载并静默安装；离线电脑应提前部署 WebView2 Runtime。

安装包包含应用前端和桌面程序，不携带开发机的配方、历史记录或设备配置。实际图像检测需要另外部署与客户端匹配、包含 glue 算子的 `lyflow_core.dll` 及其同目录依赖 DLL，并在系统设置填写核心库路径，要求见 [测试说明](TESTING.md)。运行引擎的电脑还需要对应架构的 Microsoft Visual C++ 运行库。海康真实相机需要另外安装对应的 MVS SDK/驱动。

当前未配置代码签名证书。发布新版时同步更新 `package.json`、`src-tauri/Cargo.toml` 和 `src-tauri/tauri.conf.json` 中的版本，并更新锁文件。
