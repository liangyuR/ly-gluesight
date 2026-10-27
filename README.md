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

## 测试

```powershell
pnpm typecheck
pnpm test                                   # 前端单元测试
cargo test --lib --manifest-path src-tauri/Cargo.toml
```

Windows PR CI 仅执行 Format 检测、前端单元测试（`pnpm test`）和 Rust 默认库回归（`cargo test --locked --lib`）。Format 用 rustfmt 解析变更的 Rust 文件，检查新增、修改行的格式，新文件全检；所有变更文件另用 `git diff --check` 检查空白。类型检查、样本转换、打包与长稳脚本检查及桌面 E2E 均不在普通 PR CI 中执行。

按 [测试说明](docs/testing/TESTING.md) 选择受影响文件或模块运行。Rust 默认集合包含少量 S7 回环、SQLite 与原图落盘集成测试，不需要现场硬件或 lyFlow DLL。算法评测通过独立 example 手动运行。

桌面 E2E 保留手动入口，使用 Playwright Test 连接应用 WebView2，无需下载 Playwright 浏览器。合成图段配方生产验证需要 lyFlow，无需现场样本集：

```powershell
pnpm test:ui:production -Suite burst -ResetData
```

生产落盘入口保留原命令并支持 `-Grep` 筛选，默认构建一次，再由夹具管理实例和失败证据。单独准备构建使用 `pnpm test:desktop:build -Profile flow`；测试加 `-SkipBuild` 可复用该 profile 的产物，但不会检查源码是否更新。HTML、JUnit 和失败截图、trace 位于 `output/playwright/<profile>-regression/runner/`；原业务 JSON 位置保持不变。

长稳与故障注入见[长稳运行说明](docs/testing/soak-report.md)。参数、隔离实例、现场验收与证据边界见[测试说明](docs/testing/TESTING.md)。

## 打包

```powershell
pnpm package
```

恢复 lyFlow 与厂商运行库，完成前端、Rust 与 NSIS 安装包构建。版本管理、依赖缓存与厂商包维护见 [Windows 打包](docs/deployment/PACKAGING.md)。

## 文档

更多架构、现场接入与测试文档见 [docs/README.md](docs/README.md)。
