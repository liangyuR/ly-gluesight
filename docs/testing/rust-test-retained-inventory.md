# Rust 保留测试清单

数量（2026-10-26 以 `cargo test --manifest-path src-tauri/Cargo.toml --locked --lib -- --list` 核对）：**默认 167 项，没有手动忽略的用例**。其中 118 项在分组表中逐项列出源码、测试名称及保护规则，49 项列在末节“尚未逐项登记”。测试数量只在本清单维护，其他文档不写数字。

默认集合包含少量真实 S7 TCP 回环、SQLite、图片写盘与审计恢复集成测试，不能把 `cargo test --lib` 全部理解为纯单元测试。默认运行不需要现场相机、PLC、现场样本或 lyFlow DLL；S7 用例需要本机回环连接权限。不通过 `ignore` 隐藏删除项，也不把不同业务用例塞进一个大循环。运行方法与环境要求见[测试说明](TESTING.md)。

## 数量与入口

| 分组 | 逐项登记 | 其中手动忽略 |
| --- | ---: | ---: |
| 采集、帧组、计划与判定 | 36 | 0 |
| 生产周期、PLC与调度 | 32 | 0 |
| 工作区、发布与视觉 | 31 | 0 |
| 系统配置与资料安全 | 11 | 0 |
| 持久化、录制与离线 | 8 | 0 |
| 尚未逐项登记 | 49 | 0 |
| 合计 | 167 | 0 |

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --offline --locked --lib
```

默认集合没有 `#[ignore]` 用例。

## 采集、帧组、计划与判定

| 序号 | 源码 | 测试名称 | 保护规则 |
| ---: | --- | --- | --- |
| 1 | [plan.rs](../../src-tauri/src/plan.rs) | `plan_identity_changes_only_with_sequence_kind_frames_cameras_or_product` | 计划身份仅随段序列、类型、预期帧数、相机绑定和产品代码改变；限值编辑不换计划号 |
| 2 | [plan.rs](../../src-tauri/src/plan.rs) | `any_uncalibrated_segment_falls_back_to_part_calib_or_none_otherwise_sum` | 存在待标定段时回退整件标定，全部已标定时按各段帧数求和 |
| 3 | [plan.rs](../../src-tauri/src/plan.rs) | `invalid_segment_plans_are_rejected` | 拒绝非法相机绑定、段索引、检测限值和区间，防止非法计划发布 |
| 4 | [frame_group.rs](../../src-tauri/src/frame_group.rs) | `frames_are_grouped_by_frame_counter_not_arrival_order` | 三路帧按各自SDK帧号减基线配组，乱序交付不能串组 |
| 5 | [frame_group.rs](../../src-tauri/src/frame_group.rs) | `duplicate_seq_keeps_first_and_counter_at_or_below_baseline_is_stale` | 重复帧不得覆盖首次帧，旧帧不得进入下一工件或抬高基线 |
| 6 | [frame_group.rs](../../src-tauri/src/frame_group.rs) | `frame_counter_jump_beyond_seq_limit_is_out_of_range_and_creates_no_groups` | 异常巨大SDK帧号不得创建无界帧组或污染后续基线 |
| 7 | [frame_group.rs](../../src-tauri/src/frame_group.rs) | `frame_count_at_tolerance_edge_is_normal_and_one_beyond_is_low_or_high` | 帧数容差含上下边界，超出一帧必须报偏高或偏低 |
| 8 | [segmentation.rs](../../src-tauri/src/segmentation.rs) | `unsegmented_returns_one_pending_region_covering_all_groups` | 真实分段算法未就绪时整件必须作为一个未分段区域覆盖全部帧组 |
| 9 | [calibration.rs](../../src-tauri/src/calibration.rs) | `n_exp_is_rounded_mean_with_min_max_recorded` | 多轮标定按均值四舍五入生成预期帧数并记录真实波动统计 |
| 10 | [calibration.rs](../../src-tauri/src/calibration.rs) | `fewer_rounds_than_required_are_rejected_with_have_and_need` | 轮数不足不得产生可用标定，恰达要求可标定 |
| 11 | [calibration.rs](../../src-tauri/src/calibration.rs) | `suggested_tolerance_covers_observed_spread_never_below_default` | 建议容差覆盖标定中最差实际波动且不低于默认容差 |
| 12 | [camera.rs](../../src-tauri/src/camera.rs) | `released_camera_is_never_reopened_by_the_supervisor` | 释放给MVS客户端的相机不得被后台自动重连抢占 |
| 13 | [camera.rs](../../src-tauri/src/camera.rs) | `unresolved_manual_trigger_blocks_mvs_arming_until_resolved` | 仍有未解决Software触发时禁止真实相机布防，避免调试帧混入生产 |
| 14 | [camera.rs](../../src-tauri/src/camera.rs) | `burst_plan_camera_missing_or_not_single_view_is_rejected` | 当前整轮采集仅接受存在且单视角、Line0高电平门控、帧率一致的相机 |
| 15 | [camera.rs](../../src-tauri/src/camera.rs) | `software_capture_rejects_the_previous_callback_image_after_its_pending_flag_clears` | Software单帧取图必须获得请求后的当前会话新图，不能返回上次缓存 |
| 16 | [camera.rs](../../src-tauri/src/camera.rs) | `prior_trigger_completion_cannot_satisfy_the_current_capture` | 带精确采集票据的取图仅接受匹配会话和帧号，防止前后请求串图 |
| 17 | [camera.rs](../../src-tauri/src/camera.rs) | `sdk_frame_length_must_cover_width_height_and_pixel_size` | SDK图像长度不足、零尺寸和宽高溢出必须拒绝，防止越界读取 |
| 18 | [frame.rs](../../src-tauri/src/frame.rs) | `frame_pool_over_budget_refuses_allocation_until_frames_released` | 图像预算耗尽时禁止继续分配，必须等最后持有者释放后归还预算 |
| 19 | [shot_router.rs](../../src-tauri/src/shot_router.rs) | `frame_baseline_follows_highest_sdk_frame_counter_of_the_session_not_trigger_counter` | 整轮采集基线采用当前会话最高SDK帧号，不能误用Chunk触发号或回退值 |
| 20 | [camera/delivery.rs](../../src-tauri/src/camera/delivery.rs) | `duplicate_or_stale_callbacks_do_not_consume_another_manual_trigger` | 重复或过期回调不能消耗新的手动触发额度 |
| 21 | [camera/delivery.rs](../../src-tauri/src/camera/delivery.rs) | `dropped_callback_frame_becomes_incomplete_group_not_err` | 回调队列或内存预算丢帧只标不完整组，不能把当前段配方整件判成设备错误 |
| 22 | [burst_judge.rs](../../src-tauri/src/burst_judge.rs) | `unsegmented_region_judges_frames_with_wider_limits_and_flags_offset_unjudged` | 当前未分段区域逐帧按较宽限值判定，并明确胶偏未判状态 |
| 23 | [burst_judge.rs](../../src-tauri/src/burst_judge.rs) | `missing_camera_frames_mark_incomplete_without_changing_verdict` | 缺帧和测量失败只标不完整，其余相机有效NG仍须保留 |
| 24 | [burst_judge.rs](../../src-tauri/src/burst_judge.rs) | `region_frame_count_outside_tolerance_raises_anomaly_and_short_part_flags_only` | 帧数异常和疑似提前终止仅产生标志，不能覆盖胶路本身OK/NG |
| 25 | [burst_judge.rs](../../src-tauri/src/burst_judge.rs) | `any_camera_frame_ng_makes_region_ng` | 任一相机任一帧NG必须使区域及整件NG，防止沿用旧多视图放行规则 |
| 26 | [burst_judge.rs](../../src-tauri/src/burst_judge.rs) | `frame_exceeding_gap_presence_width_or_overflow_limit_is_ng_with_matching_reason` | 断胶、有胶比例、累计胶宽超差和溢胶必须按各自限值形成正确判定 |
| 27 | [recipe_capture/burst/tests.rs](../../src-tauri/src/recipe_capture/burst/tests.rs) | `plc_estimate_of_zero_or_above_64_is_only_recorded_and_round_completes_on_actual_frames` | PLC开始估算帧数不限制实收帧数，结束预留deviceTriggers写0仍按实收完成 |
| 28 | [recipe_capture/burst/tests.rs](../../src-tauri/src/recipe_capture/burst/tests.rs) | `camera_short_of_frames_completes_after_plc_end_and_drain_with_loss_alarm_only` | PLC结束后少帧相机在排空窗口结束仍完成，并记录准确缺帧和告警 |
| 29 | [recipe_capture/burst/tests.rs](../../src-tauri/src/recipe_capture/burst/tests.rs) | `camera_skipping_a_frame_counter_leaves_only_that_group_incomplete` | 采集写盘链路按帧号保留缺口，后续图像不能前移填补缺失帧 |
| 30 | [recipe_capture/burst/tests.rs](../../src-tauri/src/recipe_capture/burst/tests.rs) | `duplicate_counter_is_dropped_with_alarm_and_the_first_frame_is_kept` | 采集重复帧只告警并保留首次图像，不得覆盖已写原图 |
| 31 | [recipe_capture/burst/tests.rs](../../src-tauri/src/recipe_capture/burst/tests.rs) | `backward_counter_or_counter_not_above_baseline_fails_round_naming_the_camera` | SDK帧号回退或低于基线必须终止轮次并定位故障相机 |
| 32 | [recipe_capture/burst/tests.rs](../../src-tauri/src/recipe_capture/burst/tests.rs) | `lost_packet_budget_dropped_or_queue_full_frame_is_missing_with_alarm_and_round_completes` | 丢包、内存预算和写盘队列压力不伪造有效图，记缺帧告警后继续完成轮次 |
| 33 | [recipe_capture/burst/tests.rs](../../src-tauri/src/recipe_capture/burst/tests.rs) | `failed_round_reports_device_or_process_fault_not_count_mismatch` | 设备故障与中止超时分别映射PLC 98/99，不得退回旧计数不符故障码 |
| 34 | [recipe_capture/burst/tests.rs](../../src-tauri/src/recipe_capture/burst/tests.rs) | `thousands_of_frames_are_persisted_and_restored_after_restart` | 超过旧64帧限制的实采轮次逐帧写盘，重启恢复准确统计并判中断 |
| 35 | [recipe_capture/burst/tests.rs](../../src-tauri/src/recipe_capture/burst/tests.rs) | `persisted_record_cannot_redirect_images_or_forge_frames` | 采集记录拒绝越界图像路径、伪造帧、统计和身份，防止加载不可信轮次 |
| 36 | [recipe_capture/burst/tests.rs](../../src-tauri/src/recipe_capture/burst/tests.rs) | `camera_without_session_baseline_starts_and_groups_from_its_first_frame` | Line0 高电平门控下软件取不了帧：相机本会话没见过帧也必须能开始整圈采集，各相机以本轮首帧为第 1 组。 |

## 生产周期、PLC与调度

| 序号 | 源码 | 测试名称 | 保护规则 |
| ---: | --- | --- | --- |
| 37 | [cycle/tests.rs](../../src-tauri/src/cycle/tests.rs) | `three_camera_frames_out_of_arrival_order_are_grouped_by_frame_counter_minus_baseline` | 三台相机按各自帧计数基线配组，乱序到达不得改变帧身份或整件合格判定。 |
| 38 | [cycle/tests.rs](../../src-tauri/src/cycle/tests.rs) | `part_end_waits_full_drain_window_regardless_of_reserved_trigger_count` | partEnd后必须等完整排空窗口，deviceTriggers为0、缺省或任意值均不能缩短等待。 |
| 39 | [cycle/tests.rs](../../src-tauri/src/cycle/tests.rs) | `frames_beyond_expected_total_are_all_measured_and_only_alarm` | 超出预期帧数的图像全部检测，超额帧中的断胶仍应判NG，帧数异常仅告警。 |
| 40 | [cycle/tests.rs](../../src-tauri/src/cycle/tests.rs) | `next_part_baseline_is_each_camera_highest_frame_counter_seen` | 下一件基线只取每台相机已见最高帧号，迟到帧隔离且漏触发相机下一件首帧不被误丢。 |
| 41 | [cycle/tests.rs](../../src-tauri/src/cycle/tests.rs) | `camera_reconnect_during_part_is_err_98` | 生产期间相机重连，无论之后是否再收到帧，都应判设备故障ERR98。 |
| 42 | [cycle/tests.rs](../../src-tauri/src/cycle/tests.rs) | `no_frame_from_any_camera_is_err_91` | 整件三相机没有任何帧时等到收尾截止并判ERR91，不能提前判合格。 |
| 43 | [cycle/tests.rs](../../src-tauri/src/cycle/tests.rs) | `unmeasured_frame_marks_group_incomplete_not_err_99` | 队满、单帧失败或超时仅使相关组不完整；超时后的成功结果不得覆盖终态。 |
| 44 | [cycle/tests.rs](../../src-tauri/src/cycle/tests.rs) | `tail_frame_received_before_drain_deadline_finishes_measurement_before_result` | 排空截止前接受的尾帧仍须等测量结束，截止后新帧不能进入已结束采集。 |
| 45 | [cycle/tests.rs](../../src-tauri/src/cycle/tests.rs) | `same_sn_reinspection_rejects_measurement_from_previous_cycle` | 相同SN重检时上一cycle的测量不得填入当前相同相机/帧组槽位或提前完成当前件。 |
| 46 | [cycle/admission.rs](../../src-tauri/src/cycle/admission.rs) | `admission_checks_product_before_plan` | 生产接件先核对产品代码、再核对计划可编译性，错误码优先级不得颠倒。 |
| 47 | [cycle/interrupted.rs](../../src-tauri/src/cycle/interrupted.rs) | `interrupted_part_record_carries_plan_totals_and_err_98` | 进程中断后的段配方补记录保留预期帧数和容差，空帧记录判ERR98并满足数据库写入契约。 |
| 48 | [cycle_ids.rs](../../src-tauri/src/cycle_ids.rs) | `durable_ids_remain_unique_and_the_pool_stays_bounded_during_refills` | 异步补充持久工件ID后不得重复发放，池与在途补充分配总量始终有界。 |
| 49 | [arming.rs](../../src-tauri/src/arming.rs) | `serial_arm_waits_for_side_effect_settlement_before_cleanup_and_rejects_late_ok` | 带副作用的串行布防超时必须等待原操作落定再清理，迟到成功不得被当作按时成功。 |
| 50 | [measure.rs](../../src-tauri/src/measure.rs) | `timed_out_blocking_frame_keeps_capacity_and_late_completion_is_not_reported_twice` | 阻塞测量超时后仍占真实执行容量，不能提前接新件；迟到结果不重复上报，原工作结束后恢复容量。 |
| 51 | [measure.rs](../../src-tauri/src/measure.rs) | `full_frame_queue_reports_the_rejected_frame_as_unmeasured_and_still_measures_every_queued_frame` | 测量队列满必须为被拒帧返回Unmeasured，已经排队的所有帧仍能测量且不会重复交付。 |
| 52 | [production/tests.rs](../../src-tauri/src/production/tests.rs) | `offline_validated_release_runs_only_in_offline_mode` | 仅经过离线验证的发布版本不能进入真机生产。 |
| 53 | [production/warmup_tests.rs](../../src-tauri/src/production/warmup_tests.rs) | `timed_out_warmup_keeps_the_permit_and_late_success_cannot_replace_failure` | 预热超时后执行许可须继续占用，后续排队超时不能偷跑，迟到成功不得覆盖不可重试失败。 |
| 54 | [plc_plan.rs](../../src-tauri/src/plc_plan.rs) | `plc_plan_counts_are_sum_of_expected_frames_and_uncalibrated_plan_cannot_compile` | PLC段计划计数和容差取各段之和，未标定不能生产，有整件标定则使用整件预期帧数。 |
| 55 | [handshake.rs](../../src-tauri/src/handshake.rs) | `contract_rejects_plc_and_pc_fields_sharing_bytes` | PLC输入和PC输出不得共享同一字节，即使位号不同；同属PC的不同位可共用。 |
| 56 | [handshake.rs](../../src-tauri/src/handshake.rs) | `start_requires_distinct_completed_polls_same_identity_and_connection` | 开始请求必须经不同已完成轮询确认，期间请求身份与PLC连接保持不变才能布防。 |
| 57 | [handshake.rs](../../src-tauri/src/handshake.rs) | `plc_writing_previous_protocol_cannot_start` | 现行协议版本允许开始，上一版协议必须拒绝开始。 |
| 58 | [handshake.rs](../../src-tauri/src/handshake.rs) | `ack_requires_matching_result_identity_and_retained_result_state` | ACK须匹配结果序号和SN且结果状态仍保持，错误确认不能释放结果。 |
| 59 | [handshake.rs](../../src-tauri/src/handshake.rs) | `heartbeat_requires_a_change_and_constant_reads_do_not_extend_deadline` | 心跳必须实际变化才算存活，重复读取同一值不能延长截止，超时后变化不能撤销失活。 |
| 60 | [plc_session/tests.rs](../../src-tauri/src/plc_session/tests.rs) | `s7_wire_disconnect_after_result_applied_never_commits_success` | 结果已写入但连接在确认前断开时不得报提交成功，保留结果和事务，重连后明确复位并保留审计。 |
| 61 | [plc_session/tests.rs](../../src-tauri/src/plc_session/tests.rs) | `s7_wire_heartbeat_fault_stops_outputs_before_slow_io_finishes` | 阻塞落盘期间心跳失活仍须及时撤销armed并置故障，落盘结束或心跳恢复不得自动丢弃未结事务。 |
| 62 | [plc_session/tests.rs](../../src-tauri/src/plc_session/tests.rs) | `s7_wire_teaching_plan_arms_only_in_a_capture_transaction` | 示教计划只有绑定capture_id后才能布防，经真实S7完成布防及ACK释放。 |
| 63 | [plc_session/recovery.rs](../../src-tauri/src/plc_session/recovery.rs) | `repeated_sn_and_sequence_never_merge_different_cycles` | ACK恢复按cycle分开，同一SN和PLC请求序号复用不能合并不同工件的确认。 |
| 64 | [cycle/tests.rs](../../src-tauri/src/cycle/tests.rs) | `hik_camera_without_frames_this_session_arms_and_counts_from_its_first_frame` | 海康相机本会话没见过帧、开流计数起点未知时生产必须能布防，首帧算第 1 帧，收尾后上一件迟到帧不得计入下一件。 |
| 65 | [teaching.rs](../../src-tauri/src/teaching.rs) | `plc_register_violation_names_the_offending_field` | PLC 示教寄存器不合规时必须指到具体字段：已接受未回显握手编号、原因码与状态不符等协议边界。 |
| 66 | [teaching.rs](../../src-tauri/src/teaching.rs) | `acceptance_before_echo_waits_and_accepts_once_echo_matches` | PLC 先置已接受、晚写回显时等待接受期间不得判故障，回显对上后照常接受；接受之后回显为 0 仍是违规。 |
| 67 | [plc/tests.rs](../../src-tauri/src/plc/tests.rs) | `export_spanning_several_pages_writes_every_log_once_in_order` | PLC 日志分页导出不得漏写、重复或乱序。 |
| 68 | [teaching.rs](../../src-tauri/src/teaching.rs) | `repeated_fault_with_same_reason_is_reported_once` | 示教事务未结、PLC 断线时同一故障只上报一次，原因变化才再报，防止节拍每拍重写故障日志。 |

## 工作区、发布与视觉

| 序号 | 源码 | 测试名称 | 保护规则 |
| ---: | --- | --- | --- |
| 69 | [candidate.rs](../../src-tauri/src/candidate.rs) | `real_or_occupied_environment_rejects_candidate_debug` | 真实设备模式、缺离线样本或存在采集/生产任务时拒绝候选试运行，防止调试与生产争抢设备。 |
| 70 | [recipe.rs](../../src-tauri/src/recipe.rs) | `publish_reuses_plan_version_for_same_identity_and_issues_new_otherwise` | 限值与容差变化沿用 PLC 计划号，重新标定改变预期帧数必须分配未使用的新计划号。 |
| 71 | [recipe.rs](../../src-tauri/src/recipe.rs) | `burst_recipe_snapshot_restores_its_path_segments` | 历史快照还原必须读取 pathSegments，不能把旧测量段 segments 当作当前段计划。 |
| 72 | [recipe.rs](../../src-tauri/src/recipe.rs) | `published_versions_are_assigned_once_and_crash_retries_cannot_overwrite_a_revision` | 当前段配方发布幂等重试沿用版本；相同修订的冲突内容、过期基版本均被拒绝。 |
| 73 | [recipe.rs](../../src-tauri/src/recipe.rs) | `failed_publish_write_does_not_consume_the_frozen_target_version` | 发布写盘失败后重试仍能使用原冻结目标版本，不能提前消耗版本号。 |
| 74 | [workspace/burst_tests.rs](../../src-tauri/src/workspace/burst_tests.rs) | `calibration_apply_writes_expected_frames_and_calib_but_leaves_tolerance_until_confirmed` | 标定输出预期帧数与统计，建议容差须用户确认才写入，避免静默放宽生产门槛。 |
| 75 | [workspace/burst_tests.rs](../../src-tauri/src/workspace/burst_tests.rs) | `calibration_with_fewer_rounds_than_recipe_setting_is_rejected` | 正常标定轮次不足时不修改配方，标准件轮次不能充当普通标定轮次。 |
| 76 | [workspace/burst_tests.rs](../../src-tauri/src/workspace/burst_tests.rs) | `multi_segment_calibration_uses_confirmed_manual_cuts_of_every_round_or_leaves_segments_pending` | 多段计划只按每个标定轮次已确认的手动分段逐段标定，ΣN_exp 等于各段之和；有轮次只是边界草稿时各段保持待标定并点名该轮，不能把整件总数摊给各段。 |
| 77 | [workspace/burst_tests.rs](../../src-tauri/src/workspace/burst_tests.rs) | `standard_rounds_set_x_ref_for_straight_segment_and_refuse_corner` | 胶偏参考只从标准件指定相机的中间帧得出，弯道与未实现的 ToEdge 不能生成错误参考。 |
| 78 | [workspace/burst_tests.rs](../../src-tauri/src/workspace/burst_tests.rs) | `editing_expected_frames_directly_is_rejected_and_kind_change_clears_calibration` | 拒绝客户端伪造标定值；段类型、段序列或胶偏参考相机改变时使相关标定失效。 |
| 79 | [workspace/burst_tests.rs](../../src-tauri/src/workspace/burst_tests.rs) | `new_candidate_from_copied_burst_recipe_builds_without_its_calibration` | 复制段配方创建候选保留合法成像参数，但不继承另一配方的预期帧数、标定统计和胶偏基准。 |
| 80 | [workspace/burst_tests.rs](../../src-tauri/src/workspace/burst_tests.rs) | `calibration_round_writes_per_frame_features_and_boundaries_files` | 特征文件按计划相机与帧序写入缺帧、失败帧及真实指标，边界文件保留未分段状态，避免后续算法读错样本。 |
| 81 | [workspace/burst_release_tests.rs](../../src-tauri/src/workspace/burst_release_tests.rs) | `validation_without_expected_total_blocks_publish_and_names_each_segment` | 没有整件预期帧数不能发布；具备整件标定的多段配方不被尚未实现的逐段标定永久阻塞。 |
| 82 | [workspace/burst_release_tests.rs](../../src-tauri/src/workspace/burst_release_tests.rs) | `publish_blocked_when_plan_cameras_not_three_or_corridor_missing` | 三台计划相机、示教帧、帧内走廊、像素当量及当前设备参数必须一致才能通过发布验证。 |
| 83 | [workspace/burst_release_tests.rs](../../src-tauri/src/workspace/burst_release_tests.rs) | `sample_round_used_for_teaching_or_calibration_or_from_other_device_is_rejected` | 示教/标定用轮次不得当独立验证样本；相机身份或设备参数不匹配时拒绝入验证库。 |
| 84 | [workspace/burst_release_tests.rs](../../src-tauri/src/workspace/burst_release_tests.rs) | `burst_candidate_debug_run_gets_teaching_frame_sizes_in_plan_camera_order` | 候选调试按计划相机顺序交付各自示教帧尺寸，避免串用相机尺寸。 |
| 85 | [workspace/burst_release_tests.rs](../../src-tauri/src/workspace/burst_release_tests.rs) | `changed_frame_rate_blocks_calibration_until_original_configuration_restored` | 当前帧率不同于标定采集帧率时阻止使用旧标定，恢复原配置后才放行。 |
| 86 | [release.rs](../../src-tauri/src/release.rs) | `tampered_burst_bundle_is_rejected_on_load` | 当前段发布包加载时拒绝走廊篡改、相机换序、旧schema伪装、尺寸变化、清单外文件和缺标定。 |
| 87 | [release.rs](../../src-tauri/src/release.rs) | `bundle_changed_on_disk_after_publish_is_rejected_by_per_part_verify` | 当前段发布包加载后每件验证仍能发现资源被替换、删改、变目录或清单丢失。 |
| 88 | [vision/tests.rs](../../src-tauri/src/vision/tests.rs) | `calibration_page_view_on_single_view_camera_also_writes_the_key_burst_recipe_reads` | 标定页面按图1保存的单视角标定也能被段配方按设备读取，三目图N标定不能污染设备标定。 |
| 89 | [vision/taught/tests.rs](../../src-tauri/src/vision/taught/tests.rs) | `engine_station_output_reduces_to_frame_width_gap_presence_and_x_mm` | 生产走廊输出换算正确的胶宽、最长连续缺胶、有胶比例、法向偏移和相机帧身份。 |
| 90 | [vision/taught/tests.rs](../../src-tauri/src/vision/taught/tests.rs) | `malformed_engine_output_fails_only_that_frame` | 算法输出违约或尺寸不匹配只拒绝本帧，不能错算指标或破坏后续正常帧的测量。 |
| 91 | [vision/taught/tests.rs](../../src-tauri/src/vision/taught/tests.rs) | `bead_leaving_through_a_corridor_end_is_out_of_corridor_but_breaks_and_centred_stops_stay_gaps` | 胶条从走廊侧向离开与真实断胶/停胶必须区分，不能错误判缺胶或放过真断胶。 |
| 92 | [vision/taught/tests.rs](../../src-tauri/src/vision/taught/tests.rs) | `corridor_leaving_the_frame_or_camera_outside_the_plan_is_refused` | 走廊须位于像素中心边界内且归属计划相机，防止无法测量或挂错相机指标。 |
| 93 | [workspace/centerline.rs](../../src-tauri/src/workspace/centerline.rs) | `brush_follows_requested_glue_instead_of_larger_shadow` | 走廊提取沿用户画笔选择的胶条，而非旁边面积更大的阴影，并兼容明暗极性。 |
| 94 | [workspace/history_samples.rs](../../src-tauri/src/workspace/history_samples.rs) | `burst_candidate_history_reads_camera_seq_from_that_cameras_own_shot_sequence` | 候选历史按相机自己的帧序读受管样本原图，缺帧不能取别的相机替代，历史计划身份不符须拒绝。 |
| 95 | [workspace/recorded.rs](../../src-tauri/src/workspace/recorded.rs) | `recorded_burst_frame_is_read_by_camera_and_seq_and_identity_mismatch_is_refused` | 真实 Recorder 写出后按 camera/seq 还原原始像素，拒绝跨件、跨相机、错帧号/触发数或缺帧的原图引用。 |
| 96 | [workspace/transfer/tests.rs](../../src-tauri/src/workspace/transfer/tests.rs) | `exported_candidate_round_trips_with_referenced_round_images_and_matching_checksums` | 配方迁移包须带上候选引用的采集轮次原图、示教帧与标定特征，解包内容与来源逐字节一致、清单校验和正确。 |
| 97 | [workspace/transfer/tests.rs](../../src-tauri/src/workspace/transfer/tests.rs) | `tampered_or_unknown_packages_are_rejected_without_writing_outside_staging` | 迁移包含路径穿越、校验和不符、清单外条目或不认识的格式版本时整包拒绝，不能写出暂存目录。 |
| 98 | [workspace/transfer/tests.rs](../../src-tauri/src/workspace/transfer/tests.rs) | `conflicting_round_id_is_renamed_with_all_candidate_references_and_import_starts_as_draft` | 本机已有同名采集轮次时导入轮次改编号，示教帧、多轮标定、复核记录与标定特征一起改名；导入候选回到草稿，不带来源机发布状态、基准版本与历史记录样本。 |
| 99 | [workspace/transfer/tests.rs](../../src-tauri/src/workspace/transfer/tests.rs) | `import_with_existing_recipe_id_or_product_code_is_rejected` | 导入不能与本机已有配方编号（不区分大小写）或产品代码冲突。 |

## 系统配置与资料安全

| 序号 | 源码 | 测试名称 | 保护规则 |
| ---: | --- | --- | --- |
| 100 | [disk_guard.rs](../../src-tauri/src/disk_guard.rs) | `low_free_space_pauses_and_recovers_after_recheck_without_restart` | 磁盘空间不足阻止开工，空间恢复后无需重启即可重新就绪 |
| 101 | [image_file.rs](../../src-tauri/src/image_file.rs) | `frozen_image_id_rewritten_with_different_pixels_is_rejected_and_same_pixels_pass` | 冻结图像ID对应像素变化必须拒绝，防止使用变更原图执行示教与标定 |
| 102 | [instance.rs](../../src-tauri/src/instance.rs) | `data_directory_is_exclusive_until_the_instance_exits` | 同一资料目录必须独占，前一实例退出后才能重新取得锁 |
| 103 | [managed_samples.rs](../../src-tauri/src/managed_samples.rs) | `imported_sample_survives_source_removal_and_duplicate_id_cannot_overwrite_it` | 导入样本独立保存且重复ID不能覆盖已导入数据 |
| 104 | [settings.rs](../../src-tauri/src/settings.rs) | `arm_budget_outside_100_to_5000_ms_is_rejected_so_pc_gives_up_before_plc_arm_timeout` | PC布防预算只能在100至5000ms，不能超出PLC布防等待契约 |
| 105 | [site/tests.rs](../../src-tauri/src/site/tests.rs) | `entering_offline_is_refused_when_preconditions_fail` | 已有生产或采集事务、设备异常时拒绝切换离线环境 |
| 106 | [storage.rs](../../src-tauri/src/storage.rs) | `wal_history_and_teaching_references_are_copied_without_changing_source` | 搬迁资料必须包括未checkpoint的WAL与示教引用，原资料不被改写 |
| 107 | [applog/codes.rs](../../src-tauri/src/applog/codes.rs) | `every_code_is_unique_and_registered_in_the_code_table_document` | 运行日志事件编码是给 Agent 的契约：编码不重复，且与编码表文档的编码、事件名、级别逐条一致。 |
| 108 | [applog.rs](../../src-tauri/src/applog.rs) | `old_lines_without_time_or_code_stay_queryable_and_new_lines_keep_ts_level_first` | 日志 schema 2 新增字段后旧行仍可查询，新行仍以 ts、level 开头，快速筛选与分段裁剪不失效。 |
| 109 | [applog/throttle.rs](../../src-tauri/src/applog/throttle.rs) | `repeats_within_window_write_first_event_then_one_summary_with_full_count` | 高频重复事件窗口内只照写首次并在到期时输出一条摘要，计数与样本范围完整，不丢计数也不退回逐帧刷屏。 |
| 110 | [diagnostics.rs](../../src-tauri/src/diagnostics.rs) | `plc_logs_fetched_newest_first_are_written_oldest_first_with_matching_t` | 诊断包 PLC 日志按时间正序写出且每行本地时间 t 与 ts 同一时刻，可与运行日志对齐。 |

## 持久化、录制与离线

| 序号 | 源码 | 测试名称 | 保护规则 |
| ---: | --- | --- | --- |
| 111 | [audit/tests.rs](../../src-tauri/src/audit/tests.rs) | `durable_spool_replays_sqlite_commit_before_receipt_cleanup_without_duplicate_or_regression` | 数据库已提交而回执尚未清理时重放，不能重复落库或回退判定与交付证据 |
| 112 | [audit/tests.rs](../../src-tauri/src/audit/tests.rs) | `burst_part_replayed_from_reopened_spool_links_raw_files_by_camera_seq` | 段配方审计重启按相机和帧号恢复原图引用，70 帧都能对上 |
| 113 | [history/tests.rs](../../src-tauri/src/history/tests.rs) | `rejudge_skips_when_plan_identity_changed_but_allows_tolerance_only_change` | 计划身份变化拒绝重判，只有容差变化仍允许对照 |
| 114 | [recorder/tests.rs](../../src-tauri/src/recorder/tests.rs) | `offline_retention_prunes_only_offline_recordings_and_never_real_raw_images` | 离线保留策略不能删除真机原图 |
| 115 | [recorder/tests.rs](../../src-tauri/src/recorder/tests.rs) | `recording_with_missing_or_queue_dropped_frames_lists_them_by_camera_seq` | 缺帧和录图队列丢帧必须按相机帧号标记不完整，不能冒充完整录制 |
| 116 | [store/tests.rs](../../src-tauri/src/store/tests.rs) | `offline_parts_stay_out_of_real_history_statistics_and_rejudge_and_vice_versa` | 真机与离线记录在历史、统计及重判中相互隔离 |
| 117 | [store/tests.rs](../../src-tauri/src/store/tests.rs) | `burst_record_round_trips_frames_flags_regions_and_device_triggers` | 段配方逐帧、缺帧标记、区域判定和预留字段完整落库读回 |
| 118 | [store/tests.rs](../../src-tauri/src/store/tests.rs) | `insert_accepts_any_frame_count_but_rejects_duplicate_camera_seq_or_foreign_camera` | 帧数波动允许入库，重复帧身份或非计划相机必须整条拒绝且不留部分行 |

## 尚未逐项登记

以下 49 项在上次逐项整理（2026-10-23）后随相机维护、生产通讯协议 7、示教采集握手、维护模式与项目管理加入，已在默认集合中执行，保护规则待逐项补入上表；补登时按准入规则核对，说不清保护规则的删除。

| 模块 | 测试名称 |
| --- | --- |
| `camera::access_tests` | `unresolved_plc_without_active_capture_allows_maintenance_but_keeps_configuration_locked` |
| `cycle::tests` | `collect_part_completes_at_drain_deadline_without_measuring` |
| `inspection::tests` | `saved_teaching_db8_points_move_to_db12_without_relocating_other_fields` |
| `instance::tests` | `restarted_instance_waits_for_the_previous_one_to_release_the_directory` |
| `offline::line::tests` | `idle_teaching_cancel_releases_and_fault_only_clears_on_explicit_reset` |
| `offline::line::tests` | `teaching_capture_completes_using_db6_db12_without_production_request` |
| `offline::tests` | `simulator_echo_mismatch_faults_then_plc_reset_restores_ready` |
| `offline::tests` | `simulator_resets_pc_then_part_completes_back_to_idle` |
| `plc_session::tests` | `maintenance_part_arms_with_pc_mode_1_and_accepts_only_result_code_0` |
| `plc_session::tests` | `normal_part_cannot_arm_without_plan_or_report_code_0` |
| `plc_session::tests` | `plc_reset_request_completes_without_operator_then_intake_pause_withdraws_ready` |
| `plc_session::tests` | `s7_wire_part_returns_ready_only_after_plc_latches_result` |
| `plc_session::tests` | `software_reset_waits_for_plc_idle_before_ready` |
| `production::tests` | `collect_recipe_is_refused_by_normal_production_admission` |
| `project::manage::tests` | `create_with_conflicting_or_invalid_id_or_duplicate_name_is_refused_and_existing_project_untouched` |
| `project::manage::tests` | `delete_current_is_refused_and_deleted_project_leaves_list_at_once_then_purge_clears_trash` |
| `project::tests` | `invalid_current_pointer_then_operator_must_choose_instead_of_another_project_opening` |
| `project::tests` | `no_project_on_this_machine_then_default_is_created_and_opened` |
| `project::transfer::tests` | `copied_project_gets_its_own_paths_and_plan_versions_and_leaves_the_source_untouched` |
| `project::transfer::tests` | `copy_without_records_leaves_out_inspection_db_and_recordings` |
| `project::transfer::tests` | `damaged_or_tampered_package_is_refused_without_leaving_a_project_behind` |
| `project::transfer::tests` | `imported_production_recipe_gets_an_unused_local_plan_version_and_keeps_its_revision` |
| `project::transfer::tests` | `imported_project_references_point_into_the_new_project_not_the_source` |
| `recipe::tests` | `collect_then_formal_publishes_but_formal_never_downgrades_to_collect` |
| `recipe::tests` | `recipe_file_without_stage_reads_formal_and_collect_survives_file_and_snapshot` |
| `recipe::tests` | `switching_to_another_project_with_shared_plan_file_then_plan_version_exceeds_previous_project` |
| `recipe_capture::burst::tests` | `teaching_ready_receives_before_plc_acceptance_and_persists_without_production_metadata` |
| `recorder::tests` | `maintenance_collect_recordings_survive_retention` |
| `site::tests` | `work_in_progress_or_unwritten_records_then_project_switch_is_refused` |
| `store::tests` | `database_without_mode_column_keeps_records_as_production_after_upgrade` |
| `store::tests` | `today_counts_exclude_maintenance_parts_but_history_lists_them` |
| `teaching::tests` | `cancel_before_acceptance_requires_plc_ack_and_idle_before_next_request` |
| `teaching::tests` | `int_round_number_exhaustion_or_corrupt_journal_never_resets_sequence_silently` |
| `teaching::tests` | `payload_reason_written_before_new_state_is_only_consumed_when_state_changes` |
| `teaching::tests` | `plc_end_without_observed_acceptance_begins_drain_and_blocks_another_round` |
| `teaching::tests` | `real_pending_round_cannot_be_cleared_by_offline_reset` |
| `teaching::tests` | `rejected_round_requires_matching_release_and_its_number_is_not_reused` |
| `teaching::tests` | `restart_preserves_unfinished_round_and_requires_explicit_reset_before_new_number` |
| `teaching::tests` | `unpublished_idle_payload_does_not_accept_or_fault_request_or_early_cancel` |
| `workspace::capture_review::tests` | `corridor_teaching_and_exclusion_cannot_reference_the_same_group` |
| `workspace::capture_review::tests` | `exclusion_changes_image_statistics_without_compressing_counts_and_restore_remeasures` |
| `workspace::capture_review::tests` | `invalid_boundaries_leave_existing_review_unchanged` |
| `workspace::capture_review::tests` | `mismatched_capture_configuration_rejects_template_without_changing_target` |
| `workspace::capture_review::tests` | `plan_change_then_revert_keeps_review_stale_and_template_invalid` |
| `workspace::capture_review::tests` | `removing_an_applied_calibration_source_clears_derived_values_before_exclusion` |
| `workspace::capture_review::tests` | `reordered_capture_cameras_reuse_the_same_template` |
| `workspace::capture_review::tests` | `template_requires_confirmation_and_application_preserves_exclusions_as_draft` |
| `workspace::collect_release_tests` | `missing_camera_imaging_or_formal_exists_then_collect_check_fails` |
| `workspace::collect_release_tests` | `untaught_uncalibrated_candidate_then_collect_checks_pass_and_store_saves_collect` |
