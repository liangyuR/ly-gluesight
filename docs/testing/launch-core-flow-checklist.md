# 核心业务流程上线核对清单

核对日期：2026-10-19，基于 main `1f8b2f7`。范围：新建配方 → 默认参数 → 整圈采集 → 单帧示教 → 标定验证 → 验证与发布 → 生产握手 → 历史记录。规则来源为 [配方创建与多图示教](../architecture/recipe-creation-workflow.md)、[一期 S7 握手契约](../integration/plc-s7-phase1.md)、[测试记录](TESTING.md) 与代码；取舍见 [core-flow 决策](../decisions/core-flow.md)。

无真机：PLC 由进程内虚拟 S7 产线（`src-tauri/src/offline`）扮演，相机为样本集回放，测量用本机 lyFlow 1.1.0。实拍样本集为 `2026-10-17-normal-sampleset`（12 点位 × 3 相机，1280×1024），副本写入模拟标定 0.112 mm/px（D-CF-1）。

## 验证方式

- **实例**：隔离 e2e 实例（`scripts/Start-UiTestInstance.ps1 -Profile e2e -DebugPort 9341`）经 CDP 调后端命令逐步驱动，用实拍样本集模拟标定副本；新建配方、3 相机整圈采集、12 个拍照点组示教、独立样本验证、离线发布、投产与各类异常工况均实际跑过。
- **E2E**：`pnpm test:e2e:offline`（含重启续测），覆盖范围见 [离线端到端](TESTING.md#离线端到端)。
- **单测**：`cargo test --lib` 中注明的测试。
- **代码**：只做代码核对，没有运行验证。

结果：**通过**；**已修复**（发现缺陷并提交 PR，修复后复验通过）；**保留**（与文档或直觉不一致，按决策保留）；**未验证**（附原因）。

## 1. 新建配方与默认参数

| # | 规则 | 验证方式 | 结果 |
| --- | --- | --- | --- |
| 1.1 | 新建空草稿，配方编号（不区分大小写）与产品代码唯一 | 实例：同产品代码再建被拒「产品代码 2 已被配方 DRAFT-1 使用」；单测 `workspace::tests::deleting_and_recreating_candidate_keeps_old_snapshots_and_rejects_same_identity_conflicts` | 通过。备注：`recipe_template` 只避开已发布配方的产品代码，不避开草稿；配方库界面自行计算未用代码，不受影响 |
| 1.2 | 未发布草稿不能开工：不能被人工选为检测配方、不能离线运行，就绪报告标“未发布” | 实例：`cycle_select_recipe`「配方不存在」，`offline_run`「配方不存在」，就绪报告「未发布：在配方工作台验证并发布后才能运行」 | 通过 |
| 1.3 | PLC 下发的产品代码没有已发布配方时拒收（95） | 代码：`start_part` 按产品代码查找失败即 `refuse(NO_RECIPE)` | 未验证运行：虚拟产线只按已发布配方下发产品代码 |
| 1.4 | 修改默认参数只让使用默认值的图重新试测，整套验证失效 | 单测 `workspace::workflow_tests::editing_one_view_invalidates_only_that_trial_and_restoring_defaults_requires_retrial` | 通过 |
| 1.5 | 全新站点写入样例配方 MTR-HSG-A/B，真机与样本集模式下不可运行 | 实例：就绪报告显示其“没有不可变发布包” | 保留（D-CF-8） |

## 2. 整圈采集（示教采集握手）

| # | 规则 | 验证方式 | 结果 |
| --- | --- | --- | --- |
| 2.1 | PLC 开始时锁定 1–64 次触发，PLC 结束且各相机收满才完整；一次触发每台相机一帧 | 实例：3 相机 × 12 次触发 = 36 个拍照点，完整；单测 `recipe_capture::tests::plc_start_latches_only_a_valid_count_once_and_preserves_it_on_duplicate_start`、`three_cameras_one_trigger_is_one_shot_with_an_image_per_camera_and_completes_only_together` | 通过 |
| 2.2 | 采集事务走完结果、ACK 与释放握手（完成码 1），不生成检测记录、不计入 OK/NG | 实例：离线产线「整圈采集 … 完成，resultCode = 1」；E2E 断言 3 轮采集（示教 1 轮、验证样本 2 轮）后离线检测记录只有投产的 2 件 | 通过 |
| 2.3 | 示教轮次不能作为独立验证样本 | 实例：`workspace_capture_sample` 用示教轮次被拒「示教采集不能同时作为独立验证样本」 | 通过 |
| 2.4 | 异常轮次封锁其设备会话，重连建立新会话后才能重采，迟到旧帧被拒 | 单测 `failed_round_requires_a_new_session_and_rejects_late_old_session_frames_for_all_sources` | 通过 |
| 2.5 | 退出时进行中的轮次在重启后标为异常；重启后可直接重采 | 实例：修复前重启后一律被拒「请重新连接这些相机」（新会话编号与上次同为 1/2/3）；修复后中断轮次标「应用退出导致本轮采集中断」，新一轮直接开始。单测 `interrupted_round_does_not_block_the_same_session_number_after_restart` | 已修复（#97，D-CF-5） |
| 2.6 | 样本集声明当量的拍照点采用后自动带标定依据 | E2E：所有检测图的标定依据为 `sampleSet`，当量 0.112 | 通过 |
| 2.7 | 采集中途取消：轮次记异常、须整圈重采；在途采集事务向 PLC 回写 90/94 后照常确认释放 | 单测 `failure_requires_reconnecting_only_cameras_that_may_still_have_frames_and_names_them_all`；代码 `finish_capture_s7` | 通过（单测与代码核对）；采集事务进行中取消的 PLC 回写未在实例运行 |

## 3. 单帧示教与标定验证

| # | 规则 | 验证方式 | 结果 |
| --- | --- | --- | --- |
| 3.1 | 每个拍照点组（Pose）恰好选一张检测图，其余图只采集不检测 | 实例与 E2E：12 组各选一台相机；单测 `missing_or_multiple_group_images_block_teaching_validation_and_publish_progress` | 通过 |
| 3.2 | 整组不检 | 代码：0 张检测图的组被验证与发布拦下 | 保留（D-CF-7），与配方创建文档“标记不检测”不一致 |
| 3.3 | 算法正常执行形成 NG 也是有效试测，可保存示教 | 实例：S002-cam2 默认参数试测 NG_GAP 仍为有效试测；改窄搜索后为 OK | 通过 |
| 3.4 | 改中线或参数只让该图重新试测 | 单测 `editing_one_view_invalidates_only_that_trial_and_restoring_defaults_requires_retrial` | 通过 |
| 3.5 | 样本集声明当量作为标定依据，用户改当量后回到人工标定 | 单测 `workspace::progress_tests::calibration_step_accepts_sample_set_declaration_until_scale_changes`；E2E 全程不调用人工标定 | 通过 |
| 3.6 | 人工标定误差 ±0.1 mm；代表点复用须设备、标定、比例一致 | 单测 `representative_calibration_reuse_requires_matching_environment_and_retains_source_identity` | 通过（无参考件，未做实物精度） |

## 4. 验证与发布

| # | 规则 | 验证方式 | 结果 |
| --- | --- | --- | --- |
| 4.1 | 至少一套独立正常整圈样本，期望合格时 OK 与 OK_WITH_EXCURSION 都算符合 | 实例与 E2E：正常样本实际 OK_WITH_EXCURSION 判为符合，7 项检查全部通过；单测 `qualified_expectation_accepts_ok_and_ok_with_excursion_only` | 通过 |
| 4.2 | 缺陷样本结果如实保存，不作为必选门槛 | E2E：人工断胶件期望 NG_GAP、实际 NG_GAP | 通过 |
| 4.3 | 样本与示教的 PLC 计划版本一致 | 单测 `historical_sample_requires_the_teaching_plc_plan_even_with_identical_shot_fields` | 通过 |
| 4.4 | 验证后候选修订、设备、标定、引擎或生产版本变化须重新验证 | 代码：`workspace_publish` 核对验证修订、`environment` 与生产基础版本 | 通过（代码核对） |
| 4.5 | 发布生成不可变包，在工件边界生效；失败保留同一包人工重试 | E2E：发布状态 active；单测 `failed_activation_retains_the_same_frozen_release_and_requires_explicit_retry` | 通过 |
| 4.6 | 离线发布的版本带 `validationSite=offline` | 实例与 E2E | 通过 |
| 4.7 | 离线发布的版本真机拒绝开工，引导回工作台；离线仍可运行 | 实例与 E2E 重启续测：真机就绪报告「该版本在离线环境验证发布，需在真机重新验证并发布」；单测 `offline_validated_release_runs_only_in_offline_mode` | 通过 |
| 4.8 | 计划版本按拍照计划字段分配，新配方取新号 | 实例：第二个配方分到新计划版本，发布日志提示 PLC 同步 | 通过 |

## 5. 生产握手（虚拟 S7）

| # | 规则 | 验证方式 | 结果 |
| --- | --- | --- | --- |
| 5.1 | 正常一件：partStart → armed → 触发计数 → partEnd → done → resultAck → 释放 → 下一件 | 实例：连续投产多件，结果码 2，ACK 后回到空闲；单测 `offline_line_over_s7_completes_parts_with_matched_ack_and_returns_to_idle` | 通过 |
| 5.2 | OK 与 OK_WITH_EXCURSION 都算合格（PLC 码 1/2，今日统计计为合格） | 实例：2 件 OK_WITH_EXCURSION，统计 ok=2 | 通过 |
| 5.3 | 配方未发布就开工 | 见 1.2、1.3 | 通过 / 1.3 未运行 |
| 5.4 | 人工选型号时 productCode 不符拒收 95 | 实例：修复前报 ERR 100「拍照计划不一致」；修复后 ERR 95「PLC 产品代码 3 与所选配方 NEW-1（产品代码 1）不一致」。单测 `product_code_mismatch_is_reported_before_count_and_count_before_plan_version` | 已修复（#98，D-CF-4） |
| 5.5 | shotCount 与配方计划不符拒收 94；计划版本不符拒收 100 | 单测同上 | 已修复（#98） |
| 5.6 | deviceShots≠shotCount、planReserved 非零、requestSeq 重复 / 回退 / 为零：不布防、不出结果，只置 visionFault | 单测 `invalid_start_request_faults_without_arming_or_result_while_a_valid_one_starts` | 通过（#100 新增测试） |
| 5.7 | partEnd 时实际触发数与计划不符：视觉故障，在途件记 ERR 98 | 实例：「PLC 设备实际触发数 11 与计划 12 不符」，visionFault | 通过 |
| 5.8 | 传输丢帧：ERR 91，结果照常确认 | 实例：缺 S004 组三台相机的帧，原因含应收与实收计数 | 通过 |
| 5.9 | 运动超时：ERR 97 | 实例：「布防后 30 s 内未收到 partEnd」 | 通过 |
| 5.10 | ACK 序号不符：保持 done，视觉故障；记录保留原结论、交付记失败 | 实例：「ackSeq 与本次 resultSeq 不匹配，保持 done」 | 通过 |
| 5.11 | 中途取消（立即停止）：PLC 撤销请求，PC 故障并记 ERR 98，faultReset 后恢复 | 实例：「在途工件的请求或计划被修改」→ 下一件经 faultReset 后正常 | 通过 |
| 5.12 | 故障时只把仍在等确认的工件改记交付失败；持续故障不重复写追溯 | 实例：修复前持久追溯故障期间 spool 序号涨到 2565，强制退出后重启被拒；修复后空闲时注入心跳中断，已确认工件交付不变、spool 为空。单测 `only_a_result_awaiting_plc_ack_is_marked_failed_by_a_later_fault` | 已修复（#101，D-CF-6） |
| 5.13 | 无可开工配方时 PLC 复位提示 | 实例：PLC 项显示「设备尚未就绪，不能复位并置视觉就绪」，原因实为无可开工配方 | 保留（D-CF-9，跨工作线） |

## 6. 历史记录与站点切换

| # | 规则 | 验证方式 | 结果 |
| --- | --- | --- | --- |
| 6.1 | 离线件与真机件隔离：真机历史与统计不含离线件 | 实例与 E2E：真机历史 0 条 | 通过 |
| 6.2 | 离线件带样本件引用；退出离线后同 SN 真机件不带 | E2E；单测 `real_part_with_the_same_part_sn_after_leaving_offline_gets_no_sample_reference` | 通过 |
| 6.3 | 历史导出 CSV、原包重现、规则重判、原图复测结论一致，不改原记录与候选 | E2E：三种复测均 NG_GAP | 通过 |
| 6.4 | 退出离线恢复 PLC、相机组与录制设置；重启回到真机并记住上次样本集 | E2E；实例 | 通过 |
| 6.5 | 重启后历史、配方修订、示教身份与发布包不变，离线原发布包预热后结论不变 | E2E 重启续测 | 通过 |
| 6.6 | 同 id 样本集已导入时再选外部目录被拒，改用受管副本 | 实例：「样本集 ID … 已导入，不能覆盖」；离线页提供“使用已有副本” | 通过 |
| 6.7 | 重启时 S7 未结事务保留，中性输入下明确复位后才结束；ACK 已落盘的件重启后恢复为已确认 | 单测（需 Python S7 peer，默认忽略，本轮以 `--include-ignored` 运行）`s7_wire_restart_pending_requires_neutral_explicit_reset_and_audit`、`s7_wire_restart_after_persisted_ack_recovers_exact_cycle_before_and_after_reset` | 通过 |
| 6.8 | 进行中整圈采集在重启后标为异常、可直接重采 | 见 2.5 | 已修复（#97） |

## 7. 本轮运行结果

- `cargo test --lib`（合并 #97、#98、#99、#100、#101 后）：274 通过、0 失败、37 忽略（main 基线 271 通过、37 忽略）。
- `pnpm test:e2e:offline`（实拍样本集，#99 后）：全新站点首段约 73–80 s、重启续测约 8–10 s，合并各分支后仍通过；p01 OK_WITH_EXCURSION（码 2），人工断胶件 p02 NG_GAP（码 13，`S011-cam2 · 断胶 11.0 mm > 6.0 mm`），三种复测均 NG_GAP。
- `pnpm exec tsc -b` 通过；`pnpm exec vitest run` 175 项通过（本工作线未改前端）。

## 8. 本轮未覆盖

- 真实 PLC、相机、硬触发与现场节拍；人工标定的实物精度；真实缺陷检出（人工断胶件只用于业务链路）。
- 1.3 PLC 下发未知产品代码：虚拟产线无法构造，只做代码核对。
- 高负载下离线布防预算（200 ms）超时与持久追溯故障不可恢复：已报跨工作线，未改。
