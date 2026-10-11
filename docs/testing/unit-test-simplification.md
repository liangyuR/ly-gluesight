# 单元测试精简记录（2026-10-11）

本文记录 PR #22 的第一轮去重结果，属于历史基线。后续前端大幅删减、取消覆盖率门槛和三个主流程的当前说明见 [前端轻量测试](frontend-lightweight-tests.md)。下文保留的文件列表与覆盖率要求不代表当前测试集。

本轮从 `939245b` 审查 43 个前端测试文件、38 个 Rust 测试模块，以及 Robot/PLC 模拟服务、S7 协议和原生验收脚本自身的 Node、Python、PowerShell 测试。由一个子智能体盘点全仓并处理后端，主智能体处理前端，再交叉核对删减依据。

这些测试混合了纯单元、页面交互、磁盘/数据库和本机回环集成。本轮删除确认重复、没有有效断言，以及用户明确不使用的浏览器查看模式用例；独立实现的相同规则仍分别测试，例如页面禁用条件与状态模型的生产启动拦截。生产代码、覆盖率门槛和条件忽略配置均未修改。

## 前端删减依据

| 范围 | 精简方式 | 保留覆盖 |
| --- | --- | --- |
| `navigation.test.tsx` | 16 项全路由冒烟缩减为 5 个入口，去掉浏览器模式提示断言 | 保留配方采集、相机标定嵌套路由及进度总览、PLC 配置/日志入口；全部导航链接、折叠、历史详情和未知路由回退仍检查 |
| `workflow-preview.test.tsx` | 全页面标题冒烟改为逐点示教代表页 | 公共导航选中和视图属性仍检查；各页真实操作由 `workflow-setup/recipe/production-operations` 经真实预览路由验证，路径引导与存储恢复仍保留 |
| 预览示教 | 删除单独的参数修改后重试用例，将独有断言移入现有失败恢复流程 | `workflow-recipe-operations.test.tsx` 仍检查修改后禁用保存、重试后启用、保存后禁用、保存状态及验证失效 |
| `feasibility.test.tsx` | 六字段重复的负数/空值/恢复矩阵改为一组 | 六字段零值接线检查全部保留；字段共用输入处理和数值校验，代表组覆盖负数、空值与恢复；临界值、溢出、实际参数同步仍保留 |
| `plc-logs.test.tsx` | 删除简略导出失败用例 | 在更完整的失败→刷新→重试用例保留错误提示、按钮解锁和原查询数据，异步过期、分页完整性及下载失败清理全部保留 |
| 浏览器查看模式 | 删除工作台上下文、配方库、默认参数页的三个浏览器专用用例，清理相机/标定混合用例中的浏览器部分 | 桌面相机数量限制、模拟工位内置标定仍保留；WebView 同样使用的下载清理、存储失败恢复也保留 |

## Rust 删除映射

以下删除仅位于测试模块，同时清理两项队列用例独占的 helper/import。

| 删除用例 | 保留覆盖 |
| --- | --- |
| `frame::time_full_frame_conversion` | 原例仅循环打印耗时，无性能门槛或正确性断言；`uniform_scene_gives_weighted_gray_in_every_phase`、`phases_agree_on_the_same_scene`、`pool_converts_into_reused_buffer` 保留像素、尺寸、边界与缓冲区复用检查 |
| `plc_plan::documented_example_counts` | `three_views_still_use_one_device_and_four_triggers`、`plan_binds_order_and_camera_counts_without_requiring_camera_arrival_order`、`plan_version_is_the_assigned_plan_version_not_the_recipe_revision` |
| `shot_router::missed_trigger_pulse_leaves_the_last_shot_missing` | `missed_trigger_pulse_desync_is_explicit_and_never_completes_a_shifted_part` 包含相同缺末帧状态，并验证跨轮错位与复位恢复 |
| `shot_router::plc_counts_follow_a_part_that_stopped_early` | `plc_counts_of_an_early_stopped_part_rebaseline_without_desync` 验证全部基线和下一整件绑定；迟到帧拒绝另有专例 |
| `store::version_one_database_is_backed_up_and_recreated_with_pending_recording_schema` | `mismatched_database_version_checkpoints_wal_and_keeps_complete_backup`、`old_schema_database_is_backed_up_and_replaced_without_migration` 和 `recording_evidence_defaults_pending_and_does_not_change_measurement_or_judgement` 分别验证旧库重建、完整备份和默认录制状态 |
| `camera::delivery::full_callback_channel_keeps_first_middle_and_last_missing_shots_in_place` | `full_callback_channel_finishes_missing_first_middle_and_last_as_plc_error` 保留首/中/末缺帧位置、身份与计数，并继续验证 PLC 故障和下一件恢复；队列账本另有专例 |
| `camera::delivery::extra_callback_lost_to_full_channel_is_still_rejected_by_router_summary` | `extra_callback_dropped_by_full_channel_overrides_complete_ok_with_plc_error` 验证多帧将 OK 改为故障及下一件恢复；`callback_extra_is_rejected_before_the_actor_routes_it` 保留路由摘要归因 |

并发隔离、PLC 握手、采集完整性、发布资源身份、持久化/重启恢复和跨语言协议测试保留。脚本与 native Node 测试未发现可证明等价的整例，不作删减。同目录的桌面/性能验收脚本也没有作为单元测试删除。

覆盖率对照发现全路由冒烟额外执行了浏览器模式的 API 降级代码。用户明确应用只在桌面运行，因此不为这部分保留全路由重复实例；桌面共用的进度总览和 PLC 页面入口仍选入代表组。覆盖率统计继续包含浏览器分支，未调整统计范围来掩盖下降。

## 验证结果

共精简 40 个执行用例：前端 996 → 963，Rust 默认执行用例 420 → 413。测试源码净减 169 行。原有 44 个 Rust 条件忽略项不计为删减。

| 检查 | 本轮结果 |
| --- | --- |
| 前端全量及覆盖率 | 43 文件、963 passed，0 failed |
| Rust 默认全量 | 413 passed，0 failed，44 原有 ignored |
| Node 脚本 | 143 passed |
| Robot Python / S7 Python | 40 / 21 passed |
| PowerShell 启动保护 | 3 cases passed |
| TypeScript 应用及测试 | 两项类型检查通过 |
| Git 差异空白检查 | `git diff --check` 通过 |

| V8 覆盖率 | 精简前 | 精简后 | 原门槛 |
| --- | ---: | ---: | ---: |
| 语句 | 90.54% | 90.05% | 85% |
| 分支 | 88.41% | 88.05% | 80% |
| 函数 | 86.96% | 86.34% | 80% |
| 行 | 93.60% | 93.09% | 85% |

覆盖率包含已不要求专门验证的浏览器分支，因此有小幅回落；全局以及工作台状态、流程状态模型、工件总览的原有专项门槛全部通过。不把本机并行负载下的运行时长差异宣称为性能收益。

## 复现

在仓库根目录执行；Windows 依赖解析或回环 TCP 权限限制按 [测试说明](TESTING.md) 处理。

```powershell
node node_modules/vitest/vitest.mjs run --coverage --pool=threads --maxWorkers=2
node node_modules/typescript/bin/tsc -b
node node_modules/typescript/bin/tsc --noEmit -p tsconfig.test.json
cargo test --manifest-path src-tauri/Cargo.toml --lib --offline
node --test tests/native/p0-clean-cyclehost-setup.test.mjs tests/native/p0-cyclehost-performance.test.mjs scripts/robot-plc-demo/tests/test-console.mjs
python -m unittest discover -s scripts/robot-plc-demo/tests -p 'test_*.py'
python -m unittest discover -s scripts/s7-handshake/tests -p 'test_*.py'
& ./scripts/robot-plc-demo/tests/test_start_guard.ps1
```

覆盖率前后对照均使用 V8、threads、2 workers；运行报告写入本机 `tmp/frontend-before.json`、`tmp/frontend-after.json` 和 `tmp/coverage-before`、`tmp/coverage-after`。后端/脚本日志为 `tmp/unit-test-*.log`，这些运行产物不提交。

Rust 的 44 个原有条件忽略项包括 35 个 S7 wire 回环集成、8 个外部 DLL 集成和 1 个由父测试启动的耐久性子进程夹具。父测试本轮通过并实际启动、终止了该夹具；S7 wire 的 Rust 集成入口及外部 DLL 集成本轮未运行。Python S7 的 21 项是独立测试集。没有进行现场或真实设备验收。
