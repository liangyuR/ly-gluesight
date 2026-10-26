# lyFlow 运行库依赖

## 来源

- 唯一来源是公开仓库 `liangyuR/lyFlow` 的 GitHub Release，资产名 `lyflow-runtime-win-x64-v<版本>.zip`。解压后的根目录即运行库目录（`lyflow_core.dll`、依赖 DLL、VC++ CRT、`lyflow-runtime.json`、`licenses/`）。
- 下载不需要凭证：查询优先用已登录的 `gh api`，未安装或未登录时改用匿名 GitHub API；资产走公开下载地址。

## 系列锁定

- `packaging/runtime.json` 的 `lyflow` 段只写仓库与系列：`{ "repository": "liangyuR/lyFlow", "series": "1.1" }`。带其他字段（如旧的 NuGet 源、包 ID、最低版本）的配置直接拒绝。
- 恢复时取该系列最新的正式 Release：排除 draft 与 prerelease，tag 必须是 `v主.次.补丁` 且属于该系列，必须带上述资产，补丁号按数值取最大。
- `src-tauri/Cargo.toml` 的 `lyflow-client` 必须是唯一一条 `{ git = "https://github.com/liangyuR/lyFlow.git", tag = "vX.Y.Z" }`，tag 属于同一系列，不允许 `rev` 或 `branch`。Cargo.lock 里的提交记录由 Cargo 维护，不另作校验。
- 不锁定内容哈希，也不把 Git 提交写进本仓库的锁定或校验。

## 校验项

恢复、缓存命中和打包时都读取运行库目录的 `lyflow-runtime.json`：

- `engineVersion` 与 `abiVersion` 属于系列（主.次相同），且 `engineVersion` 等于所选 Release 的版本号；
- `architecture` 为 `x64`，`localValidationOnly` 显式为 `false`；
- `files[].path` 为安全相对路径且文件全部存在，清单包含 `lyflow_core.dll`；
- `operators[].id` 包含 `packaging/runtime.json` 的全部 `requiredOperators`；
- 目录内 DLL 均为 x64 PE；在独立进程中加载 `lyflow_core.dll`，实际引擎版本等于所选版本、ABI 属于系列、注册表自检无问题、必需算子齐全。

任一项失败都不写缓存。

`requiredOperators` 是必需算子的唯一来源：打包脚本按它生成 `runtime/manifest.json` 的算子清单并做上面的校验；`src-tauri/build.rs` 在编译时把它带进程序（`GLUESIGHT_REQUIRED_OPERATORS`），加载核心库时 `check_operators` 按同一份清单核对。测量图与标定图里出现的算子都必须在清单里。

段配方的每帧走廊测量用 `glue.taught_path` 与 `glue.bead_width`，标定用 `io.load_image`、`image.board_calib` 与 `image.load_calib`。

联合开发 lyFlow 未发布的改动时用本地构建：`GLUESIGHT_LYFLOW_RUNTIME` 指向本地 `build/core/bin`（只在调试构建生效，跳过 `lyflow-runtime.json` 系列核对）；真引擎测试用 `LYFLOW_CORE_DLL` 指向其中的 `lyflow_core.dll`。

## 缓存

- 位置 `src-tauri/target/dependency-cache/lyflow/<版本>/`。先查询系列最新版本；该版本已缓存且校验通过则不下载，缓存损坏则删除重下。
- 下载在 `.download-*` 临时目录完成，校验通过后才移入正式目录。
- 查询或下载因网络失败时，退回系列内最高的已缓存且校验通过的版本并告警；都没有则失败。最新 Release 本身校验失败时直接报错，不退回旧缓存。
- CI（`Windows package`）先解析系列最新版本，再以 `lyflow-<仓库>-<版本>-x64` 为键缓存该目录。

## 开发启动

`pnpm dev` 通过 `scripts/Start-Dev.ps1` 启动 `tauri dev`，透传启动参数与退出码，不恢复、不复制运行库。Windows 下每次启动都会重新读取用户级 `GLUESIGHT_LYFLOW_RUNTIME`，覆盖继承的进程值；用户级配置为空时清除继承的残留值。首次开发或锁定系列变化后先准备运行库：

```powershell
pnpm deps:lyflow:dev   # 恢复运行库，清空后完整复制到 src-tauri/target/debug/runtime/lyflow/
pnpm dev
```

设置 `CARGO_TARGET_DIR` 时复制到该目录下的 `debug/runtime/lyflow/`。默认从程序同目录的 `runtime/lyflow/` 加载引擎；目录缺失或不属于锁定系列时应用照常启动，引擎状态显示原因，重新执行 `pnpm deps:lyflow:dev` 即可。

固定使用本地编译目录时执行：

```powershell
pnpm deps:lyflow:local "C:\work\LyFlow\build\core\bin"
pnpm deps:lyflow:local          # 查看用户级保存值和当前进程值
pnpm deps:lyflow:local -Clear   # 清除用户级配置
```

命令检查目录中的 `lyflow_core.dll` 后，将绝对路径保存到当前 Windows 用户的 `GLUESIGHT_LYFLOW_RUNTIME` 环境变量。目录应保留完整的依赖 DLL；配置命令不编译、不复制、不加载运行库。无效目录不会覆盖已有配置。

设置或清除后直接重新运行 `pnpm dev`，无需重开终端或 IDE；已运行的 GlueSight 仍需重启。该命令以保存的用户级配置为准，不使用旧终端的进程级覆盖；配置目录缺少 `lyflow_core.dll` 时直接报错，不退回默认运行库。清除后下一次 `pnpm dev` 恢复默认运行库目录。若要准备正式运行库，可先执行 `pnpm deps:lyflow:dev`。

该配置不写入仓库，作用于当前 Windows 用户；正式构建忽略此变量。需要临时指定其他目录时，使用 `pnpm dev:joint -LyFlowRuntime <目录>`，其显式目录仍优先。直接运行 `pnpm tauri dev` 或可执行文件不会经过自动刷新入口。

指向编译输出目录后，每次关闭 GlueSight、重新编译 LyFlow、再启动 GlueSight 即可使用新 DLL。指向复制出的快照目录则固定使用该快照。

## 联合开发

同时改 lyFlow 引擎和客户端时用 `pnpm dev:joint`（`scripts/Start-JointDev.ps1`），至少指定一个参数：

| 参数 | 行为 |
| --- | --- |
| `-LyFlowRuntime <目录>` | 为本次进程设置 `GLUESIGHT_LYFLOW_RUNTIME=<目录>`，debug 构建优先读取，不落盘。 |
| `-LyFlowSource <lyFlow 仓库根>` | 先 `cargo update -p lyflow-client` 把锁切到本地源码，再通过 `tauri dev -- --config 'patch."https://github.com/liangyuR/lyFlow.git".lyflow-client.path=...'` 让 Cargo 使用 `<源>/crates/lyflow-client`。 |

```powershell
pnpm dev:joint -LyFlowRuntime C:\work\lyFlow\out\runtime -LyFlowSource C:\work\lyFlow
```

两个参数都只影响本次启动；用过 `-LyFlowSource` 后执行 `git checkout -- src-tauri/Cargo.lock` 丢弃本地锁文件变化，不要提交。

## 命令

```powershell
pnpm deps:lyflow:restore            # 恢复到缓存并输出运行库目录
pnpm deps:lyflow:dev                # 恢复并复制到 debug 构建的 runtime/lyflow
pnpm deps:lyflow:update             # 当前系列最新 Release
pnpm deps:lyflow:update -Series 1.2 # 切换系列
```

升级命令查询该系列最新 Release，下载并校验通过后把 `series` 写回 `packaging/runtime.json`、把 `lyflow-client` 的 tag 改为 `v<最新版本>`，再执行 `cargo update -p lyflow-client`。升级时网络失败直接报错，不使用缓存；任一步失败都会把 `runtime.json`、`Cargo.toml`、`Cargo.lock` 恢复原样。

## 打包

`Prepare-RuntimeDependencies.ps1` 使用同一恢复函数取运行库，放进安装包 `runtime/lyflow/`，并在 `runtime/manifest.json` 的 `lyflow` 段记录 `repository`、`series`、实际 `version`、`abiVersion` 与客户端 tag；`release-manifest.json` 从该清单复制这些字段。
