# GlueSight · 胶路智检

在线涂胶检测上位机：Tauri 2（Rust）+ React，图像测量引擎为 lyFlow，相机为海康 MVS，PLC 为西门子 S7。

## 环境要求

- Windows x64
- Node.js ≥ 24.15，pnpm 12（版本见 `package.json` 的 `engines` 与 `packageManager`）
- Rust 1.99.1（`rust-toolchain.toml` 锁定，rustup 会自动安装）与 MSVC 生成工具、WebView2
- PowerShell 7（`pwsh`），用于依赖与打包脚本
- 可选：海康 MVS SDK。未安装时只能使用模拟相机

## 开发运行

```powershell
pnpm install --frozen-lockfile
pnpm deps:lyflow:dev   # 准备 lyFlow 运行库，首次开发或锁定系列变化后执行
pnpm dev               # tauri dev：启动 Vite 并编译运行 debug 版；编译失败或应用异常退出时整条链路一起退出，修好后重新运行
```

`pnpm deps:lyflow:dev` 从 `liangyuR/lyFlow` 的 GitHub Release 取 `packaging/runtime.json` 锁定系列的最新运行库，复制到 `src-tauri/target/debug/runtime/lyflow/`。不执行也能启动，但没有图像测量引擎，引擎状态会显示原因。

只调试前端界面时可用 `pnpm dev:web` 单独启动 Vite（端口 1420），不含后端。

同时改 lyFlow 引擎或客户端时使用 `pnpm dev:joint`，参数见 [lyFlow 运行库依赖](docs/architecture/lyflow-dependency.md#联合开发)。

固定使用本地编译的运行库：`pnpm deps:lyflow:local "C:\work\LyFlow\build\core\bin"`。`pnpm dev` 每次启动都会读取保存的配置，无需重开终端或 IDE；不带参数查看配置，带 `-Clear` 清除。仅开发构建生效，配置作用于当前 Windows 用户。临时指定其他目录请用 `pnpm dev:joint -LyFlowRuntime <目录>`。

### lyFlow 运行库下载与更新

| 用途 | 命令 | 结果 |
| --- | --- | --- |
| 下载或恢复运行库到缓存 | `pnpm deps:lyflow:restore` | 恢复到 `src-tauri/target/dependency-cache/lyflow/<版本>/` 并输出目录 |
| 准备本地开发运行库 | `pnpm deps:lyflow:dev` | 恢复运行库并复制到 debug 构建的 `runtime/lyflow/`；设置 `CARGO_TARGET_DIR` 时使用该目录 |
| 升级当前系列依赖 | `pnpm deps:lyflow:update` | 获取锁定系列最新正式 Release，并更新 `src-tauri/Cargo.toml` 的客户端 tag 与 `src-tauri/Cargo.lock` |

本地开发升级后，重新安装开发运行库并启动应用：

```powershell
pnpm deps:lyflow:update
pnpm deps:lyflow:dev
pnpm dev
```

若此前用 `pnpm deps:lyflow:local` 固定了本地运行库目录，改用上述 Release 运行库前先执行 `pnpm deps:lyflow:local -Clear`。

跨系列升级使用 `pnpm deps:lyflow:update -Series <主.次>`，升级后需重新验收。缓存回退、版本校验和联合开发说明见 [lyFlow 运行库依赖](docs/architecture/lyflow-dependency.md)。

若执行 `pnpm lyflow:fetch` 提示 `Command "lyflow:fetch" not found`，原因是项目未定义该脚本；下载运行库使用 `pnpm deps:lyflow:restore`，本地开发使用 `pnpm deps:lyflow:dev`。即使前面的依赖安装已成功，也仍会因找不到命令而失败。

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

恢复 lyFlow 与厂商运行库，完成前端、Rust 与 NSIS 安装包构建。版本管理、依赖缓存与厂商包维护见 [Windows 打包](docs/deployment/PACKAGING.md)。

## 文档

更多架构、现场接入与测试文档见 [docs/README.md](docs/README.md)。
