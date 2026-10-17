# GlueSight · 胶路智检

在线涂胶检测上位机：Tauri 2（Rust）+ React，图像测量引擎为 lyFlow，相机为海康 MVS，PLC 为西门子 S7。

## 环境要求

- Windows x64
- Node.js ≥ 24.15，pnpm 12（版本见 `package.json` 的 `engines` 与 `packageManager`）
- Rust 1.99.0（`rust-toolchain.toml` 锁定，rustup 会自动安装）与 MSVC 生成工具、WebView2
- PowerShell 7（`pwsh`），用于依赖与打包脚本
- 可选：海康 MVS SDK。未安装时只能使用模拟相机

## 开发运行

```powershell
pnpm install --frozen-lockfile
pnpm deps:lyflow:dev   # 准备 lyFlow 运行库，首次开发或锁定系列变化后执行
pnpm dev               # tauri dev：启动 Vite 并编译运行 debug 版
```

`pnpm deps:lyflow:dev` 从 `liangyuR/lyFlow` 的 GitHub Release 取 `packaging/runtime.json` 锁定系列的最新运行库，复制到 `src-tauri/target/debug/runtime/lyflow/`。不执行也能启动，但没有图像测量引擎，引擎状态会显示原因。

只调试前端界面时可用 `pnpm dev:web` 单独启动 Vite（端口 1420），不含后端。

同时改 lyFlow 引擎或客户端时使用 `pnpm dev:joint`，参数见 [lyFlow 运行库依赖](docs/architecture/lyflow-dependency.md#联合开发)。

## 测试

```powershell
pnpm typecheck
pnpm test                                   # 前端单元测试
cargo test --lib --manifest-path src-tauri/Cargo.toml
```

离线端到端、生产流程 UI 测试等见 [测试记录与复现](docs/testing/TESTING.md)。

## 打包

```powershell
pnpm package
```

恢复 lyFlow 与厂商运行库，完成前端、Rust 与 NSIS 安装包构建并做静默安装验证。版本管理、依赖缓存与厂商包维护见 [Windows 打包](docs/deployment/PACKAGING.md)。

## 文档

更多架构、现场接入与使用教程见 [docs/README.md](docs/README.md)。
