# r2-lyflow-upgrade 决策记录

第二轮收尾工作线：GlueSight 升级到 lyFlow 1.1.4。1.1.4 包含 LyFlow#26–#29，已于 2026-10-19 正式发布（Release、NuGet 均成功）。评测数据见 [算法评测报告](../testing/algorithm-evaluation.md) §5，A-9 的背景见 [algorithm.md](algorithm.md)。

## L-1 用 `pnpm deps:lyflow:update` 升级，系列锁不变

- 问题：打包和开发取 1.1 系列最新正式版，1.1.3 的 glue 代码与 1.1.0 相同，36 张正常实拍图有 16 张误判 NG；`Cargo.toml` 的 `lyflow-client` 还停在 v1.1.1。
- 可选项：① 手改 `Cargo.toml` 与 `Cargo.lock`；② 执行 `pnpm deps:lyflow:update`；③ 把 `packaging/runtime.json` 钉到 1.1.4。
- 采用：②。脚本先恢复并校验系列最新正式 Release 的运行库，再写 `Cargo.toml` 的 tag 与 `Cargo.lock`。`Cargo.lock` 只保留 `lyflow-client` 的 version、source 两处变化（脚本顺带带出的 `libloading` 变化已还原）。`packaging/runtime.json` 只锁系列 `1.1`，不需要改：1.1.4 之后的 1.1.x 补丁版发布，`pnpm deps:lyflow:update` 会再次取到。
- 核对：tag `v1.1.4` 指向 `4fc6dba`，与 Release 的 `targetCommitish`、运行库 `lyflow-runtime.json` 的 `sourceRevision`、`Cargo.lock` 的 source 一致；`pnpm deps:lyflow:restore` 恢复出的 `engineVersion`、`abiVersion` 均为 1.1.4。
- 回退：还原 `Cargo.toml` 的 tag 与 `Cargo.lock` 的 `lyflow-client` 两处。运行库缓存按版本分目录，不需要清理。

## L-2 离线端到端的人工断胶件改为逐行渐变

> **段配方下已被取代（2026-10-23，见 [segment-burst.md](segment-burst.md) 第九节第 17 条）**：`pnpm test:e2e:offline` 默认按段配方驱动，样本集上建站时检测项全关、不用人工断胶件，NG 由合成图主流程的 `gap` 工况覆盖；本条的人工断胶件只在旧拍照点配方流程（`-Model legacy`，保留、停用）中使用，仍按本条。

- 问题：升级后 `pnpm test:e2e:offline` 在"缺陷样本验证"一步失败：p02 的验证结论是 OK（12 个拍照点全部合格），期望 NG_GAP。同一用例在 1.1.0 下通过。
- 原因：原来的断胶件把 S032 的 (880,140) 起 80×100 px 换成左移 50 px 处的像素。搬来的恰好是胶左侧的暗带（法兰阴影），抹掉胶的位置上反而多出一条暗带。1.1.4 把它测成胶（胶宽中位 4.1 mm、覆盖率 1.0），与未改图时没有区别；1.1.0 则把暗带和胶连成一片、整站丢弃，原用例靠这个假断胶行为通过。这不是 1.1.4 漏检真实断胶：评测报告 §5.5 的合成断胶（胶两侧背景线性插值）10 mm、20 mm 检出率都是 100%。
- 可选项：① 放宽检测参数让暗带不被当成胶（会削弱正常图的容忍，与 A-8 的默认值不符）；② 改成右移 80 px 复制（同样是搬真实纹理，换相机或姿态可能再次碰巧）；③ 逐行把区域换成左右紧邻像素的线性渐变（与评测合成缺陷同一做法）。
- 采用：③。同一张图只改这一处，1.1.4 下覆盖率 0.876（11 站无胶，约 11 mm 超过最大断口 6 mm），判 NG_GAP；右移 80 px 的结果与渐变相同。`eraseBmp` 去掉 `shift` 参数，夹具的 `defect.shift` 一并删除。人工断胶件仍只验证业务链路，不代表真实缺陷外观。
- 验证：1.1.4 运行库、同机满载下 `pnpm test:e2e:offline` 通过（首段 94.6 s、重启续测 9.6 s）：p01 判 OK（12 个拍照点全部合格、553 点），p02 判 NG_GAP（`S011-cam2 · J1 · 图 1 断胶 11.0 mm > 6.0 mm · s=12.0–23.0`），原包、规则重判、原图复测都是 NG_GAP。p01 此前在 1.1.0 下是 OK_WITH_EXCURSION，1.1.4 下没有局部超差。
- 回退：恢复搬运像素的实现，端到端在 1.1.4 及以上会在缺陷样本验证处失败。

## L-3 开发环境刷新本地运行库

`pnpm dev` 不恢复也不复制运行库。依赖升级合并后，每台开发机要刷新一次：

```powershell
git pull
pnpm install
pnpm deps:lyflow:local            # 查看是否固定了本地目录
pnpm deps:lyflow:local -Clear     # 固定过本地目录且要改用正式版时执行
pnpm deps:lyflow:dev              # 恢复 1.1.4 并复制到 src-tauri/target/debug/runtime/lyflow
```

- 设置过 `CARGO_TARGET_DIR` 时，运行库复制到该目录的 `debug/runtime/lyflow/`。
- 用户级 `GLUESIGHT_LYFLOW_RUNTIME`（`pnpm deps:lyflow:local` 写入）优先于 `runtime/lyflow`，也会被 `Test-OfflineE2E.ps1`、`Test-Soak.ps1` 这类脚本从进程环境继承；固定在本地编译目录的机器不执行 `-Clear`，验证用的仍是那个目录。验证升级时显式传 `-LyFlowRuntime`，或设成 `pnpm deps:lyflow:restore` 输出的目录。
- 主检出的 `src-tauri/target/dependency-runtime/runtime/lyflow`（1.1.0）是 P0 阶段手工放置的运行库，脚本和应用都不引用，不会随升级刷新。主检出的 `debug/runtime/lyflow` 目前是 1.1.3，需要重新执行 `pnpm deps:lyflow:dev`。
- 首次编译会按 tag `v1.1.4` 拉取 `lyflow-client`，需要能访问 lyFlow 的 GitHub 仓库。
