# 分段连拍（feat/segment-burst-flyshot）测试清单

> 历史清单（2026-10-23 精简前）：本文保留分段连拍开发期间的测试新增、改写、删除和实测记录，不表示这些测试仍存在。当前约 100 项回归的逐项清单及删除依据见 [Rust 保留测试清单](rust-test-retained-inventory.md)。其中 7 项 `vision::taught::engine_tests` 与 2 项 `packaging` ignored 测试现已删除，本文相关名称和复现方式只作历史证据。

> 2026-10-23 测试范围更新：按用户决策删除依赖现场样本集的离线建站及重启、日志回放、特征定位桌面测试（原 `test:e2e:offline`、`test:e2e:log-replay`、`test:e2e:fixture` 及对应脚本）。下文相关命令、脚本和通过记录仅为历史事实，已不是可执行入口或当前覆盖声明；产品功能、长稳、硬件联调与底层业务测试不受影响。当前桌面入口仅保留生产落盘。

本文件由各工作包的返回结果汇总生成，列出新增测试及其保护的规则，以及删除/改写的存量测试及原因。

## 新增测试及各自保护的规则


**A1**
- `plan::tests::plan_identity_changes_only_with_sequence_kind_frames_cameras_or_product` — 规格 §1 计划版本：段类型、顺序、段数、相机顺序/绑定、各段 N_exp（总数不变也换）、产品代码、整件重新标定变化时换号；容差、限值、x_ref、检测区间、段长、标定统计变化时沿用（参数化）
- `plan::tests::any_uncalibrated_segment_falls_back_to_part_calib_or_none_otherwise_sum` — 规格 §1 + 决策补充第二节 expected_total 的取值优先级：各段都已标定取 ΣN_exp；否则取 PartCalib；都没有为 None
- `plan::tests::invalid_segment_plans_are_rejected` — 规格 §1/§5 模型约束（参数化 15 种非法输入：没有段、index 不从 1 起或跳号、相机 2 台/4 台/重复/空、弯道带胶偏检测或胶偏参考、胶偏参考相机不在计划内、a=b、a<0、区间非有限值、tolerance>1000、ΣN_exp>32767），并确认 tolerance=1000、ΣN_exp=32767 恰在上限时合法
- `plan::tests::wider_checks_takes_the_looser_limit_per_item` — 规格 §3/§4 未分段区域取直线/弯道中较宽一档（按绝对上下限计算；某项只要有一段停用即停用；max_range_mm 与 offset 置 None）
- `plan::tests::export_json_lists_segments_frames_tolerance_and_totals_with_null_when_uncalibrated` — 规格 §1 导出 JSON 契约（字段名快照，未标定为 null，protocolVersion 断言等于 PROTOCOL_VERSION 常量）；有 PartCalib 时导出整件帧组数

**A2**
- `frame_group::tests::frames_are_grouped_by_frame_counter_not_arrival_order` — 规格 §3：帧序号取 SDK frame_counter 减各相机基线，不按到达顺序
- `frame_group::tests::camera_missing_a_seq_marks_only_that_group_incomplete_including_trailing` — 规格 §3：某台缺帧时只有该组标 incomplete（包括末尾缺帧），帧组数 = 各相机最大 seq 的最大值
- `frame_group::tests::duplicate_seq_keeps_first_and_counter_at_or_below_baseline_is_stale` — 易错边界：重复帧不替换先到的帧；frame_counter ≤ 基线算 Stale，不计入本件，也不抬高 max_counter；用 max_counter 作下一件基线时，上一件迟到的帧算 Stale（D-SB-7）
- `frame_group::tests::camera_count_differing_by_more_than_one_raises_loss_alarm` — 规格 §3 丢帧判定（参数化 7 组计数，并固定 Triggers/Cameras 两种告警依据的内容）
- `frame_group::tests::frame_count_at_tolerance_edge_is_normal_and_one_beyond_is_low_or_high` — 规格 §2：N_exp ± tol 内为 Normal，越界一帧为 Low/High，下限饱和到 0（参数化）

**A3**
- `segmentation::tests::unsegmented_returns_one_pending_region_covering_all_groups` — 规格 §3：本轮整条胶路按未分段处理，是一个覆盖 1..=total 的 AlgorithmPending 区域
- `segmentation::tests::confident_boundary_gives_one_frame_each_side_to_corner` — 规格 §3：边界两侧各一帧归弯道；直线段只剩一帧时让给弯道，区域不重叠、不留缝（参数化）
- `segmentation::tests::low_confidence_boundary_merges_both_sides_into_unsegmented_without_plan_fallback` — 规格 §3：置信度不足时两侧合并为 Unsegmented(LowConfidence)，不回退计划边界，连续多个一起合并；算法缺给的边界同样处理（参数化）
- `segmentation::tests::search_window_is_previous_plus_n_exp_within_tol_plus_one` — 规格 §3 搜索规则：(prev+N_exp)±(tol+1)，下限不早于 prev+1（参数化）
- `segmentation::tests::segmenter_off_by_k_frames_reports_boundary_error_k_and_missing_boundaries_listed` — 规格 §3/§6 合成序列回归入口（参数化：真值→误差 0；整体偏后 2 帧→误差 2；缺一个边界→该边界未找到；Unsegmented→全部未找到）
- `segmentation::tests::same_spec_and_seed_give_identical_sequence_within_jitter_with_missing_frames_marked` — 规格 §6 合成序列：同 spec 同种子结果确定；各段帧数落在名义值 ± 起头/收尾/弯道波动内，且波动确实生效；指定缺帧标 incomplete（计划外新增）

**A5**
- `calibration::tests::n_exp_is_rounded_mean_with_min_max_recorded` — 规格 §2：N_exp 取均值四舍五入（.5 进位），并记录 rounds/min/max/mean（参数化：[10,11]→11、[10,10,11]→10、[12×10]→12）
- `calibration::tests::fewer_rounds_than_required_are_rejected_with_have_and_need` — 规格 §2：至少采集 min_rounds 轮（可配）才能标定，恰好 min_rounds 轮可以（计划外新增：计划步骤里有这条规则但没有对应用例）
- `calibration::tests::suggested_tolerance_covers_observed_spread_never_below_default` — 规格 §2：容差建议按 min/max 覆盖实际波动，且不低于默认容差（参数化）

**P1**
- `vision::taught::engine_tests::throughput_probe` — 不是回归测试，没有要防的回归。它是计划 P1 要求的一次性测量工具（#[ignore]，不进 CI，数值不设门槛）。断言只保证量的是正常路径：测点数大于 100、有胶比例大于 0.95、PGM 文件大小等于头加全部像素，否则耗时和速率数据没有意义。结论写在决策记录里，不依赖该测试长期通过。

**B1**
- `handshake::tests::plc_writing_protocol_5_cannot_arm` — §5 protocolVersion=6：PLC 写 5 不能布防，写 6 可以（改写自 malformed 测试里的版本用例）
- `handshake::tests::start_request_with_unequal_or_zero_counts_is_not_rejected_at_request_level` — §5 示教轮次可填估算值：请求层不再拒绝 shotCount=0 或 deviceShots≠shotCount（参数化）
- `handshake::tests::part_end_with_changed_request_identity_is_rejected` — §5 请求身份冻结：partEnd 时 requestSeq 或 deviceShots 变了仍拒绝（参数化；沿用原测试中的身份部分）
- `plc_session::tests::s7_wire_part_end_with_trigger_count_unlike_plan_completes_and_reports_triggers` — §5 verify_part_end 去掉计数相等校验：计划 4、PLC 报 3/5 时照常走完 End→结果→ACK→释放，End 带出报的数（参数化，经真实 S7 通讯）
- `plc_session::tests::journal_pending_from_protocol_5_is_archived_and_requires_reset` — 迁移（D-0）：协议 5 的在途事务原样归档为 .v5.bak，会话不进入 load_error，复位前仍是 ResetRequired，复位能完成；ACK 恢复不把审计里的协议 5 事务报成错误
- `cycle::tests::next_part_baseline_follows_plc_reported_triggers_not_plan_or_frames_seen` — 下一件基线按 PLC 报的实发触发数推：实发 3、计划 4 时不按计划数推（否则下一件首帧会被当成迟到帧）；末帧丢在传输里时不只按见到的帧推（否则下一件错位一帧）（参数化）

**C2**
- `frame::tests::frame_pool_over_budget_refuses_allocation_until_frames_released (parameterized: copy / Bayer-to-gray)` — D-SB-32: image memory is capped in bytes. Over budget, allocation is refused; the budget is not returned while any Arc still holds the image, and is returned once the last reference is released
- `camera::view_tests::burst_plan_camera_missing_or_not_single_view_is_rejected (parameterized)` — Spec §0, one trigger = one frame from each of three independent cameras (D-SB-11). A three-view camera, a camera with a composite layout, or a plan camera that is not in the rig is rejected for a segment recipe. This replaces the plan's camera_config_with_three_views_or_composite_layout_is_rejected
- `camera::view_tests::aggregate_bandwidth_over_one_port_budget_is_flagged (parameterized)` — Spec §6, bandwidth is the sum of the three cameras. 3×1280×1024 at 30 Hz (943.7 Mbps) is over 800 Mbps; at 25 Hz (786.4 Mbps) it is not; cameras with unknown resolution are left out of the total
- `tests/camera-page.test.tsx 链路带宽: 三台相机 25 Hz 合计超过/未超单口预算 → 显示/不显示带宽超预算警告 (it.each)` — Spec §6, a bandwidth overrun is visible in the UI. The panel requests the estimate at 25 Hz, always shows the total, and shows the warning only when over budget

**C1**
- `recipe::tests::publish_reuses_plan_version_for_same_identity_and_issues_new_otherwise` — 规格 §1：计划版本由配方库分配。只改容差和限值时沿用原号；N_exp 变化时发新号，且不复用用过的号（防止配方库分配没有接上段计划身份）
- `recipe::tests::schema_7_recipe_file_is_listed_unavailable` — 迁移 D-0：不带段模型字段的 schema 7 配方文件列为不可用，不加载运行（改写自 obsolete_recipe_file_is_listed_as_unavailable_and_never_loaded_for_running）
- `recipe::tests::uncalibrated_segments_block_ready_and_name_each_segment` — 规格 §1：未标定的计划不能发布，并逐段列出待标定的段；决策补充第二节：有整件标定时各段 N_exp 可以为空
- `recipe::tests::untaught_camera_blocks_ready_and_names_the_camera` — D-SB-20：三台相机都要示教走廊（至少 2 点、长度不小于 1 px）并有像素当量，缺哪台就点名哪台（参数化）
- `recipe::tests::recipe_mixing_shot_and_segment_models_or_out_of_range_burst_settings_is_rejected` — D-SB-1：拍照点和段两套模型不能混用；段配方只能飞拍；标定轮数在 10..=100；成像参数正好覆盖检测相机；整件标定的帧组数在 1–32767；旧配方不能带 inspect 或 partCalib（参数化）
- `recipe::tests::burst_recipe_snapshot_restores_its_path_segments` — 快照契约：Recipe JSON 的 segments 是旧测量段，段列表在 pathSegments；记录和历史按快照还原时两者不能混淆
- `plc_plan::tests::plc_plan_counts_are_sum_of_expected_frames_and_uncalibrated_plan_cannot_compile` — 规格 §1/§5：shotCount = deviceShots = ΣN_exp，计划相机按 inspect 顺序排列；有段待标定时编不出计划；有整件标定时取整件帧组数
- `cycle::admission::tests::admission_checks_product_then_plan_then_counts_against_sum` — 规格 §5：生产请求按 95→100→94 的顺序核对，94 与 ΣN_exp 比较（参数化，改写自 product_code_checked_before_plan_compile_and_trigger_counts_before_plan_version）
- `recipe_api::tests::new_recipe_template_has_three_cameras_and_uncalibrated_segments` — 规格 §1 与 D-SB-24：新建配方是段配方，相机按相机组顺序取前三台，各段待标定，成像参数随相机补齐并能通过配方校验

**A4**
- `burst_judge::tests::unsegmented_region_judges_frames_with_wider_limits_and_flags_offset_unjudged` — 规格 §3/§4：未分段区域照常判每帧指标、允差取较宽一档（wider_checks），不判胶偏，按 AlgorithmPending/LowConfidence 标胶偏未判（参数化）
- `burst_judge::tests::corner_region_never_judges_offset_and_does_not_flag_it` — 规格 §4：弯道不检胶偏，也不标胶偏未判
- `burst_judge::tests::straight_offset_trims_first_and_last_frame_then_judges_mean_and_max_deviation` — 规格 §4 直线段胶偏（参数化）：整段偏移超限→NgPosition；局部跑偏超限→NgPosition；只有首尾帧异常→OK；其他相机的 x 不参与
- `burst_judge::tests::straight_offset_with_to_edge_or_without_reference_is_unjudged_not_ng` — 默认做法 2：ToEdge 标测量量未实现，没有 x_ref / offset_ref 标未标定，都不判 NG（参数化；计划外新增）
- `burst_judge::tests::missing_camera_frames_mark_incomplete_without_changing_verdict` — 规格 §0/§3：缺帧（无记录 / Missing）、Error、Unmeasured、OutOfCorridor → 组与整件标 incomplete，其余相机照常判，OK 仍 OK、NG 仍 NG（参数化）
- `burst_judge::tests::region_frame_count_outside_tolerance_raises_anomaly_and_short_part_flags_only` — 规格 §2 帧数越界只告警；默认做法 3：低于 ΣN_exp−Σtol 另标 short_part；单段区域与本段 N_exp 比；结论都不变（参数化）
- `burst_judge::tests::any_camera_frame_ng_makes_region_ng` — D-SB-14：三台相机取最差，任一相机任一帧 NG 则区域与整件 NG
- `burst_judge::tests::frame_exceeding_gap_presence_width_or_overflow_limit_is_ng_with_matching_reason` — 规格 §4 每帧独立指标（参数化）：断口超长 / 有胶比例低→NgGap；胶宽超差按累计长度判（两处各 3 mm 累计 6 mm→NgWidth，一处 3 mm→OkWithExcursion）；溢胶→NgWidth 且原因含“溢胶”
- `burst_judge::tests::width_range_over_limit_is_ng_width_only_in_straight_region` — D-SB-16：胶宽均匀性只在直线区域判（NgWidth、原因“胶宽不均”），弯道与未分段不判（参数化）
- `burst_judge::tests::segment_with_inspect_range_reports_head_and_tail_as_uninspected` — 规格 §0/§1：检测区间外记未检区域 [0,a) 与 (b,length]；a=0 无起头一截、b≥段长无收尾一截、段长未知时 to_mm=None、没配区间不出

**FE2**
- `新建段配方尚未标定 → 各段预期帧数显示待标定且没有可编辑的输入，整件预期总帧数待标定 (tests/segments-page.test.tsx)` — §1 N_exp 只能由采集生成，新建各段待标定
- `直线段改成弯道 → 各段预期帧数清空、弯道不再有胶偏检测，保存的段列表带清空后的值 (tests/segments-page.test.tsx)` — §1 N_exp 只由采集生成、段序列或类型变化清空标定（D-SB-20）；§4 弯道不检胶偏
- `添加/删除/上移/改类型 → 全部 N_exp、标定统计与 x_ref 清空，段序号保持从 1 连续（参数化 4 例，tests/segment-edit.test.ts）` — D-SB-20；后端 validate_segments 序号连续
- `直线段改成弯道 → 按弯道默认重建检测项：不带胶偏检测、胶偏参考与均匀性，长度和检测区间保留 (tests/segment-edit.test.ts)` — validate_segments：弯道段不得带胶偏检测或胶偏参考
- `弯道段改成直线 → 带上直线段的胶偏检测，N_exp 待标定 (tests/segment-edit.test.ts)` — §4 直线段检胶偏
- `只改长度、容差或检测项 → 标定保留 (tests/segment-edit.test.ts)` — §1 计划身份不含容差、长度、限值，这些编辑不能清掉标定
- `保存前检查（参数化 6 例 + 检测项空值 + 合法参数，tests/segment-edit.test.ts）` — NaN 序列化成 null 后端读不了；容差为 0..=1000 整数；检测区间 0 ≤ a < b
- `只改容差、长度和检测区间 → 身份没变，不列任何同步项 (tests/plan-sync.test.ts)` — §1 限值、容差变化沿用计划号
- `整件总数/段序列/相机顺序/产品代码变化 → 列 planVersion 换新号和这一项，别的不列（参数化 4 例，tests/plan-sync.test.ts）` — D-SB-19 计划身份变化时 PLC 要同步 shotCount 等
- `首次发布 → 列出候选的全部身份项，待标定的整件总数写“待标定” (tests/plan-sync.test.ts)` — 首次发布的 PLC 对接值
- `整件预期总帧数未标定 → 发布不可用并列出待标定的段号 (tests/validation.test.tsx)` — §1 未标定不能发布（计划 test 1 的发布一半）
- `整件已标定、各段预期帧数待分段算法 → 发布可用，不因段待标定而禁用 (tests/validation.test.tsx)` — 决策补充第二节：发布门槛是 ΣN_exp 而不是各段 N_exp
- `整件预期总帧数 12 → 14 → 发布确认只列 planVersion 换新号和 shotCount / deviceShots 的变化 (tests/validation.test.tsx)` — §5 shotCount=ΣN_exp 需与 PLC 同步（计划 publish_confirm_lists_shot_count_change_when_identity_changes）
- `段配方整圈采集阻塞 → 走廊示教、多轮标定、验证与发布显示等待采集，段列表与默认参数不受影响 (tests/progress-page.test.tsx)` — 依赖采集的步骤不沿用后端的完成；步骤条与进度页一致
- `后端记走廊示教完成但仍有相机未示教 → 走廊示教显示进行中，主操作继续下一台未完成的相机 (tests/progress-page.test.tsx)` — 走廊状态取每台相机的示教任务（cameraTeachStatus）
- `段配方进度页复制计划导出 → 剪贴板里是带段序列、各段预期帧数和 ΣN_exp 的 JSON (tests/progress-page.test.tsx)` — §1 导出 JSON 给 PLC/机器人工程师（计划 plan_export_copies_json_with_segments_and_sum）
- `段配方整件已标定/待标定 → 卡片显示段数与 ΣN_exp ± Σtol，不再显示拍照点数（参数化 2 例，tests/library.test.tsx）` — 段配方 shots 为空，不能显示 0 个拍照点；§1 待标定
- `段配方默认参数 → 不展示触发方式与判定，直线与弯道各一套检测项，改弯道容差只进弯道、触发方式原值保留 (tests/recipe-editor.test.tsx)` — D-SB-23 段配方固定飞拍；§4 允差按 kind 分别配置
- `段配方 → 配方摘要显示段数、相机数与 ΣN_exp 待标定，不显示拍照点摘要 (tests/geometry-page.test.tsx)` — 段配方 shots 为空，不能提示尚无拍照点
- `已标定/待标定的段配方、旧拍照点配方 → 离线配方选项显示该配方的规模，段配方不显示 N=0（参数化 3 例，tests/recipe-run-select.test.tsx）` — 离线配方选择不误导：段配方显示 ΣN_exp 或待标定

**FE3**
- `teaching.test: 三台计划相机 → 左栏每台相机一项并标出示教状态，默认停在第一台没示教完的相机，没有拍照组选图与定位特征` — 规格 §0 所有帧都参与检测、不再按 Pose 分组选图；§4 保留边缘提取、特征定位界面停用
- `teaching.test: 旧拍照点配方（没有段计划）→ 示教页只说明已停用，不挂载示教工具` — 决策补充第一节：组选图与特征定位界面停用
- `teaching.test: 走廊有未保存的修改 → 其他相机不能切换并说明先保存，当前相机可继续编辑` — 走廊草稿按相机 id 存，切走后会丢
- `teaching.test: 走廊草稿合法 → 保存走廊把草稿并入这台相机的配方项整份保存，其余相机不变；缺像素当量 → 保存禁用` — 保存写对相机；已示教的最低条件（像素当量 + 至少两点）
- `teaching.test: 未选示教帧的相机选定轮次与帧组 → 用此帧示教按所选 (轮次, 帧组) 调 teachCamera 并带上已有走廊与当量` — 选帧提交的就是画面上选的帧，且不冲掉已有走廊
- `teaching.test: 所选帧组这台相机缺帧 → 不读图、用此帧示教禁用并说明缺帧` — 规格 §3 缺帧只告警，但缺的那一帧没有图可示教
- `teaching.test: 示教帧信息曝光/增益变化提示（参数化 3 例，改写）` — 配方曝光增益与采集时不同时提示
- `teaching.test: cameraTeachingAction 参数化（采集没完成先去采集 / 去第一台没示教完的相机 / 都完成无入口）` — 进度下一步不在采集未完成时把人带去示教
- `teaching.test: 段配方 → 总览画三台相机与段条与各相机示教状态，不出现拍照点帧框` — 段配方总览不再按拍照点摆放帧框
- `recipe-capture.test: 新轮次有缺帧和丢帧告警 → 显示帧组数、各相机帧数、不完整帧组与告警，PLC 预期总数只作参考，不出现数量核对` — 规格 §5 示教轮次不核对计数；§3 缺帧只告警
- `recipe-capture.test: 多相机旧/新轮次切换相机 → 按相机读取同一帧组的原图（参数化）` — 新轮次按帧号、旧轮次按触发序号读图，不串相机
- `recipe-capture.test: 异常轮保留原因，不能用于示教或标定` — 失败轮次不进入示教与标定
- `recipe-capture.test: 示教或标定用过的轮次不允许同时作为独立验证样本（旧配方 / 已示教 / 已入标定，参数化）` — 验证样本必须独立于示教与标定轮次
- `recipe-capture.test: 本轮已有相机用作示教帧 → 标明已用于哪几台相机；点加入标定 / 加入标定（标准件）→ 以对应类型调用；已入标定的轮次只能移出；已入样本库的轮次不能示教或标定；工件进行中禁止加入标定；旧拍照点配方只读无采用按钮` — 采集页的新操作与互斥规则；标准件轮次不与普通轮次混淆
- `recipe-capture.test: 切换帧组且新图未读回 → 保留当前图像不回到读取占位（改写）` — 预览不闪烁
- `recipe-calibration.test: 普通轮次不足配置轮数（含标准件不计入）→ 应用标定禁用并说明还差几轮；达到轮数后按当前修订提交` — 规格 §2 至少配置轮数的连续采集才能标定
- `recipe-calibration.test: 多段计划 → 各段标待标定（分段算法未就绪），整件仍给出预期帧组数与 ΣN_exp；单段计划显示 N_exp、min–max（均值）与建议容差` — 规格 §3 本轮未分段；决策补充第二节整件标定仍可发布
- `recipe-calibration.test: 容差输入预填建议值，点确认容差才提交，非整数不提交；采集历史完整轮次按类型加入；已加入的轮次可移出；旧配方只说明标定只用于段配方` — 规格 §2 容差由操作员确认后才写入
- `workspace-context.test: 相机换示教帧只清这台相机的旧草稿；草稿经后端保存后与已保存相同则清掉；保存失败保留草稿、重试成功才显示已保存且草稿并入对应相机整份提交` — 走廊草稿按相机 id 存、不丢不串、不永远显示未保存
- `teach-draft.test: 走廊草稿按 f32 比较；并入配方只改这台相机；走廊/当量/检测参数合法性参数化` — 草稿不被 f32 精度误判为未保存；自动保存写对相机；非法走廊不能保存为已示教
- `trial-result.test: 缺胶站胶宽显示未测而不是 0 且没有偏移列；缺帧/测量失败/未测/出走廊的帧不画逐站表、不判 NG` — 规格 §3 缺帧只算检测不完整；§4 中线偏移比对停用
- `teaching-flow.test（真实 Provider）: 走廊提取先保存草稿再按刷选引导提取（k=相机下标，图像为冻结示教帧）；选帧后冻结所选轮次与帧组并显示该帧；自动保存期间保留缩放与走廊、不能再改点；检测参数恢复默认重画搜索带；当量为空/0 隐藏搜索带；重采样、提取失败转笔划、连续失败提示加大笔刷等（均为相机走廊版本）` — 走廊示教的交互与保存顺序，刷选提取失败建议的连续判定

**FE4**
- `burst-view: 区域帧数在容差内/高于上限/低于下限 → 标帧数正常/偏多/偏少，结论标签不变（参数化，plan 的 region_panel_marks_frame_count_out_of_tolerance）` — §2/§6 帧数是否在容差内只告警
- `burst-view: 整条胶路是未分段区域且有未检区间 → 标未分段原因与胶偏未判，列出未检区域和未检项（plan 的 region_panel_lists_uninspected_ranges_and_unsegmented_region）` — §0/§6 未检区域与未分段区域
- `burst-view: 直线区域有胶偏统计、弯道区域标弯道不判且不挂胶偏未判` — §4 弯道不检胶偏、不标未判
- `burst-view: 某台相机缺帧 → 缺帧格为灰色缺帧状态、其余相机格照常、整组标不完整（plan 的 frame_group_strip_greys_missing_camera_cells）` — §3 不完整组展示
- `inspect: 整件结果带标记 → 判定卡只挂对应徽标、结论标签不变（参数化，plan 的 inspect_shows_incomplete_and_offset_unjudged_badges_without_changing_verdict_label）` — §0/§3 检测不完整、胶偏未判，§2 帧数异常、疑似提前终止，不改 OK/NG
- `inspect: 默认跟随最新帧组；点帧组条按 (相机, seq) 读各相机原图且缺帧的相机不读，返回当前帧组恢复跟随` — §3 按帧号归属、缺帧不影响其余相机
- `inspect: 整件布置用相机与段条显示段序列和区域结论，不再有逐拍照点视图` — §1 段模型替换拍照点模型
- `inspection-diagnostics: 帧组缺一台相机 → 按帧组列出每台相机，缺帧的相机写缺帧` — §3 缺帧可见
- `history-detail: 段配方记录按区域展示，原图按 (相机, seq) 读取，缺帧的相机没有原图可读` — §3 帧身份与缺帧
- `history-detail: 逐帧追溯每台相机每个帧组一行（缺帧也有），只看异常帧后只剩缺帧行` — §3 缺帧可追溯
- `history-detail: 候选规则重判得到区域结果 → 逐区域比对表标出不同的区域` — §1/§4 复测按区域比对的接线
- `history-replay: 重判结果逐区域比对，只有实际变化的区域标差异（参数化 5 例，含 0.01 mm 容差内一致）` — 按区域比对口径（plan 的 history_replay_compares_by_region 前半）
- `history-replay: 计划身份（段类型、某段 N_exp、相机顺序、预期总数）变了 → 不做逐区域比对（参数化 4 例）` — §1 计划身份（plan 的 refuses_changed_plan_identity 后半）
- `history-replay: 只改限值、容差、胶偏参考、检测区间 → 计划身份不变，仍可比对` — §1 plan_version 规则：这些变化沿用原计划
- `history-list: 段配方记录的帧列写帧组数/ΣN_exp 并挂疑似提前终止，旧记录不带标记` — 决策默认做法 3：疑似提前终止在历史中单独显示
- `rejudge-dialog: 填写直线段/弯道段允差 → 重判请求带 straight/corner 覆盖，位置与胶宽覆盖不变` — §1 允差按 kind 分别配置
- `cycle-timeline: 段配方选中一帧 → 只看选中帧后只留该相机该帧组的事件，点事件按钮回调选帧` — 时间线按 (camera, seq) 过滤与跳转

**FE5**
- `tests/site-mode.test.tsx: 离线来源样本集 → 只提供握手与脉冲串类工况（含分段连拍新增五个），不提供断胶、定位失败等需要改图像的工况（对应计划 sample_set_source_offers_only_handshake_scenarios_including_new_ones）`（现名“离线来源为样本集 → 工况下拉比合成图少的正是需要改图像的工况”，断言改为合成图减去样本集恰好是 excursion/gap/locateFail/locateFailGap/random）— 规格 §6：离线仿真工况与后端 Scenario::handshake() 白名单一致。样本集是真实原图，不能选需要改图像的工况（excursion/gap/locateFail/locateFailGap/random），新增的 cameraLost/pulseJitter/cornerSlowdown/notInspected/earlyEnd 和原有四个握手工况必须可选。已做删行检验：把 earlyEnd 从 handshakeScenarios 删掉后该用例失败，恢复后通过。

**FIX-B1**
- `cycle::tests::three_frames_seen_plc_reports_four_next_part_first_frame_is_counter_4` — D-SB-7：下一件基线 = max(原基线, 本件见到的最高计数)。见到 3 帧、PLC 报 4、计划 4 时，下一件计数 4 是第 1 帧；PLC 报数和计划数都不参与。
- `plc_session::tests::audit_transaction_from_protocol_5_is_skipped_by_ack_recovery_without_error` — D-0 迁移：审计中协议 5 的事务被 ACK 恢复跳过，不报日志错误，也不恢复出确认。删掉 recovery.rs 的协议版本跳过后，validate 会报版本错误，测试失败。

**FIX-A1C1**
- `plan::tests::invalid_segment_plans_are_rejected（扩展参数化用例：拒绝用例从 15 条增至 29 条，通过用例从 1 条增至 3 条）` — 新增 14 条拒绝用例，逐类覆盖非法输入：胶宽名义值为 0、上允差为负、下允差为负、允许超差长度为负、均匀性极差上限为负、胶宽限值不是有限值、断胶长度为负、有胶比例大于 1、有胶比例小于 0、溢胶上限为 0、胶偏整段偏移限值为负、胶偏局部跑偏限值为负、段长为 0、检测区间终点超出段长。新增 2 条通过用例：检测区间终点恰为段长，以及停用的检测项带非法限值时不校验（防止误把停用项也拦下）。
- `recipe::tests::corner_default_with_offset_or_default_tolerance_or_limits_out_of_range_is_rejected` — 弯道默认带胶偏检测、弯道默认容差超过 1000、直线默认有胶比例大于 1 时，validate_burst 都拒绝。防止 new_segment 从非法的 segment_defaults 生成过不了段校验的段；这三条分别覆盖直线和弯道两次 validate_kind_settings 调用。

**FIX-FE-LEGACY**
- `recipe-capture：旧拍照点配方的完整轮次尚未采用 → 点采用本轮图像按本轮提交采用` — 决策补充第一节：旧路径行为不变，旧配方能从采集采用轮次走到单帧示教
- `recipe-capture：新轮次的读图命令后端还没有 → 预览显示该功能待后端 D1 实现的占位` — 5.4 命令归属：D1 前缺命令显示待后端占位，不报原图不可用
- `teaching-flow：后端还没有走廊提取命令 → 画布提示该功能待后端 D2 实现，并建议改用笔划生成走廊` — 段配方走廊提取走 workspace_extract_corridor（不借用旧命令 k）；'Command not found' 经 pendingCall 转为占位

**FIX-FE5**
- `tests/flyshot-calibration.test.tsx：三目相机（旧配方）→ 可选图 2，取样与标定都带 view=2` — D-SB-11/D-SB-30：三目拼接相机（旧配方）的视角选择与 view 参数不能丢，丢掉后图 2、图 3 的 {device}-v{view} 标定无法在界面完成。变异检查：vision_calibrate 去掉 view 参数后该用例失败。

**FIX-FE2-TESTS**
- `段配方的直线/弯道段默认检测项 → 开关恰为该类型应有的检测项（参数化两行）` — 规格 §4：胶偏只在直线段，两类段默认检测项各一套
- `段配方 → 下拉只有极性，开关只有两类段的默认检测项` — 段配方编辑器不出现触发方式与位置、胶宽判定
- `段配方改弯道段默认帧数容差 → 草稿里弯道容差为新值，直线段容差不变` — 两类段默认参数互不串
- `段配方改标定轮数 → 草稿带新轮数，触发方式保持原值不被改写` — 编辑器不展示触发方式后不得改写它
- `直线段改成弯道：各段预期帧数清空为待标定并提示标定已清空` — 改段类型走 restructure，N_exp 清空（D-SB-20）
- `直线段改成弯道：该段检测项换成弯道的一套，说明弯道不判胶偏` — 页面改类型接线到 changeKind，弯道不带胶偏
- `直线段改成弯道：立即保存清空后的段列表，弯道段不带胶偏检测与胶偏参考` — 保存给后端的段列表是清空后的值
- `已标定的配方，改帧数容差/长度/检测项（参数化三行） → 各段预期帧数与胶偏参考保留，保存的段列表带这项改动` — 非结构性编辑走 patch，不清标定，防抖后保存
- `离线配方选项显示该配方的规模（参数化四行：已标定/待标定/旧拍照点/摘要未取到的段配方）` — 段配方不显示 N=0，规模取自后端真实输出的 RecipeSummary

**C5**
- `store::tests::burst_record_round_trips_frames_flags_regions_and_device_triggers` — 规格 §3/§6、D-SB-29：段配方按帧落库的契约往返（含缺帧的帧记录、区域判定、标记、deviceTriggers、各相机帧数、列表 flags、计划帧 = ΣN_exp、收到帧 = 帧组数），审计重放能认出同一内容，任一帧不同则认不出
- `store::tests::insert_accepts_any_frame_count_but_rejects_duplicate_camera_seq_or_foreign_camera` — §2 帧数可波动；§3 帧身份 (camera, seq) 唯一、相机属于计划相机；D-SB-1 帧记录只属于段配方；被拒记录不留任何行（参数化）
- `store::tests::raw_file_refs_are_add_only_per_camera_seq` — 原图证据按 (camera, seq) 只增不换，重复写幂等，有不存在的帧整批不写，路径必须在录制根内
- `audit::tests::burst_part_replays_from_spool_and_links_raw_files_by_camera_seq_beyond_shot_limits` — 段配方记录经持久 spool（FORMAT 4）重启入库，原图引用按 (camera, seq) 关联，不受旧模型 k<64、视角 1–3 限制（帧组数 70），录制才算可用
- `audit::tests::spool_with_old_format_refuses_readiness` — 迁移：没有 format 或 format 3 的旧 spool 不读不迁、不悄悄丢弃，保留原文件并拒绝就绪（改写自存量用例，参数化）
- `history::tests::rejudge_burst_part_with_kind_override_keeps_incomplete_flag` — §1 允差按段类型（直线段覆盖值不作用于弯道区域）；§0 检测不完整标记与阈值无关，重判后照常带出（参数化）
- `history::tests::rejudge_skips_when_plan_identity_changed_but_allows_tolerance_only_change` — §1 计划身份（段类型、相机顺序、整件预期帧组数）或走廊变化时用当前候选重判跳过记 LayoutChanged；帧数容差、胶宽允差变化可直接重判（参数化）
- `history::tests::csv_export_of_burst_part_contains_expected_total_camera_frames_and_flags_columns` — 导出契约：段配方记录 CSV 末尾 ΣN_exp、Σtol、各相机帧数、deviceTriggers、标记五列；旧记录留空；列数与表头一致
- `recorder::tests::recording_with_missing_or_queue_dropped_frames_lists_them_by_camera_seq` — §3 相机缺帧 → part.json missingFrames 且录制仍完整；D-SB-32 录制队满 → rawDropped + 证据 Incomplete，写成的帧按 (camera, seq) 关联（参数化）

**C4**
- `vision::taught::tests::engine_station_output_reduces_to_frame_width_gap_presence_and_x_mm` — 规格 §4 每帧独立指标：胶宽当量换算、最长断胶（不是断胶站总数）、有胶比例、x_mm 正向与均值、FrameMetrics.camera 下标
- `vision::taught::tests::malformed_engine_output_fails_only_that_frame` — 规格 §0 一帧失败只影响本帧：8 种输出违约和图像尺寸不符都返回 Err，之后正常帧照常测（参数化）
- `vision::taught::tests::bead_leaving_through_a_corridor_end_is_out_of_corridor_but_breaks_and_centred_stops_stay_gaps` — 默认做法 4：胶条从走廊侧边走出记 OutOfCorridor；真断胶、停胶/急转处居中缺胶、整帧无胶仍按缺胶判（参数化 7 种）
- `vision::taught::tests::corridor_leaving_the_frame_or_camera_outside_the_plan_is_refused` — 走廊须落在示教帧内（像素中心边界 宽−1/高−1）；只给计划相机建图，相机下标不挂错
- `release::tests::tampered_burst_bundle_is_rejected_on_load` — 发布包不可变契约作用于 schema 4：走廊改动且清单大小同步改、清单相机互换、版本改成 3、示教帧换尺寸、多出文件、缺标定（参数化 6 种；已做变异检查：去掉走廊重算比对后该测试失败）
- `measure::tests::full_frame_queue_reports_the_rejected_frame_as_unmeasured_and_still_measures_every_queued_frame` — 规格 §0 不裁帧 / D-SB-32：段配方测量队列满时被拒的帧记 Unmeasured，每帧恰好一个结果，排队的帧照常测完
- `vision::taught::engine_tests::engine_burst_publish_load_measure_frame_round_trip` — #[ignore]，需要 LYFLOW_CORE_DLL：lyFlow 全链路 publish_burst → load → Prepared::load → 预热 → measure_frame，覆盖胶宽与 x 的换算（用 lyFlow 1.1.5 实测通过）

**D1**
- `recipe_capture::burst::tests::plc_estimate_of_zero_or_above_64_is_only_recorded_and_round_completes_on_actual_frames` — 规格 §5：示教轮次 shotCount 可为估算值（参数化 0/65/200/32767），不限 1–64、不核对，按实际帧完成
- `recipe_capture::burst::tests::camera_short_of_frames_completes_after_plc_end_and_drain_with_loss_alarm_only` — §5 结束时不核对计数；§3 某相机少帧只记丢帧告警与不完整帧组，整轮照常完成
- `recipe_capture::burst::tests::camera_skipping_a_frame_counter_leaves_only_that_group_incomplete` — §3 按 SDK 帧号配组、不按到达顺序；缺一个帧号只让那一组缺帧，后续帧不补位
- `recipe_capture::burst::tests::duplicate_counter_is_dropped_with_alarm_but_backward_or_stale_counter_fails_round` — 易错边界（参数化）：重复帧号丢弃告警并保留先到的帧；帧号回退或不大于基线整轮失败
- `recipe_capture::burst::tests::lost_packet_budget_dropped_or_queue_full_frame_is_missing_with_alarm_and_round_completes` — D-SB-8（参数化）：丢包、图像内存预算满、写盘队列满的帧记缺帧并告警，不判整轮失败
- `recipe_capture::burst::tests::failed_round_reports_device_or_process_fault_not_count_mismatch` — D-SB-33（参数化 8 种）：新轮次失败报 90+98（断连/会话变化/手动帧）或 90+99（中止/超时/PLC 断连），完成报 1/0；旧轮次仍报 90/94
- `recipe_capture::burst::tests::thousands_of_frames_are_persisted_and_restored_after_restart` — §0 帧数远超 64：3 相机×1500 帧逐帧写盘，接收中重启后按 frames.jsonl 恢复统计并判中断
- `recipe_capture::burst::tests::persisted_record_cannot_redirect_images_or_forge_frames` — 记录防伪（参数化 7 种改写 round.json / frames.jsonl）：路径越界、越界相机、重复帧、删帧、帧计数不符、多报帧数、改身份都拒绝读取和取图
- `plc_session::tests::s7_wire_teaching_plan_arms_with_any_plc_counts_only_in_a_capture_transaction` — §5 示教事务不核对 shotCount/deviceShots（参数化 0/0、65/200、32767/1）；示教计划不能用于未绑定采集轮次的生产事务
- `shot_router::tests::frame_baseline_follows_highest_sdk_frame_counter_of_the_session_not_trigger_counter` — D-SB-10：新轮次 seq 的基线按本会话最高 SDK 帧计数记，不用 Chunk 触发计数，回退帧不压低、换会话重计

**FE-POLISH**
- `recipe-calibration: 段 1 键入了容差 3 但没确认，换成另一个配方 / 重新应用标定 → 容差输入回到建议值 2（参数化）` — 上一个配方或上一次标定键入的容差，不能遮盖新配方/新标定的建议值
- `segments-page: 改了帧数容差 / 容差清空成空值、防抖没到期就离开 → 提交 1 / 0 次（参数化）` — 防抖内离开段列表页不丢未提交的合法编辑；校验不过的编辑不提交
- `segments-page: 换了配方、防抖没到期 → 不经 act，直接提交到原配方` — 换配方时经 act 提交会抬高请求序号、打断 select 对新配方的加载
- `segments-page: 上一份编辑还在提交时又改了容差并离开 → 用它返回的新修订提交新编辑` — 新编辑不丢，也不因沿用旧修订被后端按修订冲突拒绝
- `rejudge-dialog: 打开批量重判 → 直线段、弯道段两行输入全部置灰，旧的位置与胶宽两行可填` — 后端 Overrides 没有 straight/corner 字段，填了会被静默忽略，不能让操作员以为试算生效
- `rejudge-dialog: 填了位置上公差 → 批量重判请求只带旧的位置与胶宽覆盖，不带 straight/corner` — 旧拍照点记录的试算参数请求格式不变
- `plan-pending: 走廊示教页 / 整圈采集页 → 显示待后端 D3 实现的占位，不落到旧拍照点页面（参数化）` — 旧/新分流以 segments 是否为空为准；D3 前 plan 为 null 的段配方不进入会被 validate_burst 拒绝的旧页面

**C3**
- `cycle::tests::three_camera_frames_out_of_arrival_order_are_grouped_and_part_judged_after_end` — 规格 §3：按 SDK 帧号配组，不按到达顺序；收尾规则：partEnd 前不判，每台相机 max_seq ≥ deviceTriggers−1 时判，PLC 报得更多就等；结果按 C5 约束能落库
- `cycle::tests::frames_beyond_expected_total_are_all_measured_and_only_alarm` — 规格 §0 不裁帧、§2 超过 ΣN_exp±Σtol 只告警：超出的帧照常判（能判出 NG），整件标帧数异常
- `cycle::tests::next_part_baseline_is_each_camera_highest_frame_counter_seen` — D-SB-7 段配方下一件基线 = max(原基线, 本件见到的最高帧计数)：上一件收尾后才到的帧不计入下一件；漏收一个脉冲的相机，下一件首帧仍算第 1 帧（合并了计划中的两条测试；改成“基线+deviceTriggers”后测试失败，已验证）
- `cycle::tests::camera_reconnect_during_part_is_err_98` — 检测中会话变化（重连后又来帧，或重连后没再来帧）仍判 ERR 98
- `cycle::tests::no_frame_from_any_camera_is_err_91` — 三台相机整件都没有帧判 ERR 91；PLC 报了触发数时等到收尾截止再判
- `cycle::tests::unmeasured_frame_marks_group_incomplete_not_err_99` — D-SB-13：一帧没测成（队满/超时/测量失败/截止时仍无结果）只标该组与整件检测不完整，不判 ERR 99
- `camera::delivery::tests::dropped_callback_frame_becomes_incomplete_group_not_err` — 规格 §3 丢帧只告警：回调通道满或图像内存预算满丢掉的帧，只让该组标不完整，整件不判 ERR
- `cycle::interrupted::tests::interrupted_part_record_carries_plan_totals_and_err_98` — 中断件补记录带 ΣN_exp 与容差、没有帧、判 ERR 98，按 C5 约束能落库
- `cycle::readiness::tests::readiness_requires_sample_set_cameras_to_match_plan_cameras` — 规格 §1 Plan.cameras：段配方按计划相机核对样本集（缺相机或没有视角 1 都不能开工）
- `frame_group::tests::frame_counter_jump_beyond_seq_limit_is_out_of_range_and_creates_no_groups` — F0-5：帧号跳变使 seq 超上限时判 OutOfRange，不生成海量帧组，也不抬高下一件基线
- `shot_router::tests::late_frames_of_the_previous_part_cannot_satisfy_the_next_part` — 从 feb3a7c 恢复：旧拍照点配方下限 = 基线 + PLC 报数（或计划数），上一件迟到的帧不能凑数下一件

**D2**
- `workspace::burst_tests::calibration_apply_writes_expected_frames_and_calib_but_leaves_tolerance_until_confirmed` — 规格 §2：应用标定写入 N_exp、min/max/mean 和整件标定；建议容差只出现在视图里，操作员确认后才写进配方
- `workspace::burst_tests::calibration_with_fewer_rounds_than_recipe_setting_is_rejected` — 规格 §2：轮数不能少于配方设定（可配），标准件轮次不计入；失败时配方不变（参数化：11 轮加 3 轮标准件、12 轮）
- `workspace::burst_tests::multi_segment_calibration_writes_part_total_and_leaves_segments_pending_on_segmenter` — 决策补充第二节 / D-SB-21：多段计划只写整件 ΣN_exp，各段保持待标定，并注明“分段算法未就绪”
- `workspace::burst_tests::round_with_different_camera_config_cannot_join_calibration` — 规格 §2 同一设备：以下轮次都拒绝加入——设备配置与已加入的轮次不同、采集后设备配置变了、相机集合不等于计划相机（参数化 4 例）
- `workspace::burst_tests::standard_rounds_set_x_ref_for_straight_segment_and_refuse_corner` — 规格 §0/§1、D-SB-18：x_ref 只用标准件轮次，去掉首尾帧，取参考相机（未配置时取第一台计划相机）；ToEdge 不计算并注明“测量量未实现”；弯道段没有 x_ref（参数化 4 例）
- `workspace::burst_tests::editing_expected_frames_directly_is_rejected_and_kind_change_clears_calibration` — 规格 §0/§1、D-SB-20：手填 N_exp、标定统计、整件标定或 x_ref 都会被拒绝，配方不变；改段类型或增加段时清空标定；只改容差时保留；换胶偏参考相机只清空 x_ref（参数化 8 例）
- `workspace::burst_tests::new_candidate_from_copied_burst_recipe_builds_without_its_calibration` — 修复的 bug：清掉相机参数后段配方无法新建候选（校验报检测相机缺曝光/增益）。同时防止复制来源配方时把 N_exp、标定、x_ref 带进新候选（规格 §0）
- `workspace::burst_tests::calibration_round_writes_per_frame_features_and_boundaries_files` — 规格 §6 文件格式契约：features.jsonl 每台相机每个帧组一行，按计划相机、seq 排列，相机写编号；缺帧记 missing，测量失败记 error。boundaries.json 内容为 SegOutcome

**D3**
- `workspace::burst_release_tests::validation_without_expected_total_blocks_publish_and_names_each_segment` — 规格 §1：计划未标定就不能发布。决策补充第二节：有 expected_total 就能发布，各段 N_exp 可以为空。“段标定”一项逐段列出 N_exp 或待定原因（参数化 3 例）
- `workspace::burst_release_tests::publish_blocked_when_plan_cameras_not_three_or_corridor_missing` — 规格 §1 Plan.cameras = 3，§4 每台相机要有检测走廊：少一台相机、没选示教帧、走廊超出帧、缺像素当量、示教后设备配置变了，都不能通过验证（参数化 6 例）
- `workspace::burst_release_tests::validation_sample_with_frame_count_outside_tolerance_only_warns` — 规格 §2：帧组数超出 ΣN_exp ± Σtol 只告警，样本照常判定、仍算符合预期，原因里写明（参数化 2 例）
- `workspace::burst_release_tests::sample_round_used_for_teaching_or_calibration_or_from_other_device_is_rejected` — 规格 §0 独立验证、D-SB-35：用过走廊示教或多轮标定的轮次，以及设备配置或相机不一致的轮次，都不能作验证样本；被拒的轮次不进样本库（参数化 5 例）
- `workspace::burst_release_tests::progress_reports_calibration_blocker_before_validation` — 规格 §1 待标定状态要在进度里看得到：验证步骤的阻塞和“下一步”指向多轮标定；验证没通过时，走廊示教一项跳到对应相机、段标定一项跳到标定页（参数化 2 例）
- `workspace::recorded::tests::recorded_burst_frame_is_read_by_camera_and_seq_and_identity_mismatch_is_refused` — D-SB-29 原图按 (camera, seq) 引用：真实 Recorder 录完后按相机帧读回原像素；帧号、触发计数、相机、cycleId 对不上或缺帧时一律拒绝
- `workspace::history_samples::tests::burst_candidate_history_reads_camera_seq_from_that_cameras_own_shot_sequence` — D-SB-28 样本集 v1 的段配方解释：第 seq 帧取这台相机自己的第 seq 个拍照点；帧数不够时不拿别的帧顶替；历史计划相机与配方快照不一致时拒绝

**E1**
- `offline::tests::jittered_pulse_train_within_tolerance_is_ok_and_beyond_tolerance_only_alarms` — 规格 §2/§6：S7 闭环下帧数在 ΣN_exp ± Σtol 内为正常、超出只记帧数异常且结论仍 OK；deviceTriggers 写实际脉冲数（按种子参数化，种子须覆盖容差内外两类）
- `offline::tests::single_camera_loss_yields_incomplete_ok_not_err` — 规格 §0/§3：cameraLost 只让第 2 台相机缺第 3 帧（区域各相机帧数 [4,3,4]、不完整组 [3]），整件检测不完整、结论 OK、不判 ERR、会话回到空闲
- `offline::tests::trigger_frame_count_gap_over_one_alarms_without_fault` — 规格 §3/§5：相机帧数与 deviceTriggers（或另两台）相差 >1 才告警、相差 1 不告警，结论不变、不升级故障；含 PLC 多计 2 个脉冲（countMismatch）
- `offline::tests::early_part_end_is_judged_with_short_part_flag_and_frame_count_anomaly` — 默认做法 3：PLC 提前置 partEnd 时用已收到的帧照常判，帧组数 < ΣN_exp − Σtol 标 short_part 与帧数异常
- `offline::tests::sample_set_line_serves_each_camera_its_ordered_frames` — D-SB-28：样本集 v1 按相机帧序列取图，第 seq 个脉冲取各相机第 seq 帧，帧数少的相机后面的帧组缺帧（检测不完整）
- `offline::tests::pace_interval_of_15ms_is_accepted_and_14ms_rejected` — 规格 §0：相机上限 66 fps 的离线触发间隔边界（14/15/5000/5001 ms）
- `offline::line::tests::nominal_profile_keeps_calibrated_segments_and_sums_to_part_expected_total` — 离线剖面取自配方：待标定段按段长估算（无段长取默认），有整件标定时缩放到 ΣN_exp 且已标定段不动，正常件不被当成帧数异常
- `simimage::tests::rendered_burst_sequence_corner_frames_match_truth_boundaries` — 规格 §3/§6：按 PulseProfile 出图的弯道帧恰好落在 segmentation::synthetic 同一剖面与种子的真值边界之间；渲染不按段类型转动或剖面与回归入口种子脱节时失败

**FE-WIRE**
- `rejudge-dialog: 填写位置上公差、直线段上公差与弯道段连续超差允许长度 → 批量重判请求各自带在 position、straight、corner 里` — 对外契约：直线段 / 弯道段两行写进后端 history::Overrides 的 straight / corner（camelCase 字段），旧拍照点的 position / width 覆盖不受影响。如果再次置灰或写错分组，用例会失败。变异检查已做：把弯道段输入置灰后用例失败。

**E2**
- `tests/native/throughput-gate.test.mjs：吞吐门槛：各条件逐一不满足 → 不达标并点明原因，全部满足 → 达标（参数化 8 组）`（复核修复后改为 15 个用例，参数化部分的名称为“<条件> → 不达标，失败种类只有 <kind>”，见末节）— 3×25 Hz 合并门槛的判定规则：Unmeasured=0、预算丢帧=0、测量队列峰值低于容量 50%（恰好 50% 不达标）、测量失败/帧通道满丢帧/没有出件都不达标；已用变异验证（>= 改 >、去掉 Unmeasured 判定都会失败）
- `offline-e2e 段配方主流程（e2e，合成图三相机 + 真实 lyFlow，另跑样本集冒烟）` — 规格 §6 顺序验收：段模型 + 未分段 + 10 轮标定（N_exp=均值四舍五入、标定统计只含 10 轮、建议容差 max(默认,N−min,max−N) 确认后写入、标准件得出 x_ref）+ 发布（导出 shotCount=ΣN_exp、协议 6）+ 投产（帧组数在 ΣN_exp±Σtol、缺帧只标检测不完整不改 OK/NG、未检项、断胶 NG_GAP）+ 历史/CSV 末五列 + 重启续测
- `production-flow-ui 段配方段（e2e）` — D-SB-29 落库契约：parts 段配方列、part_frames 每相机每帧一行（缺帧也有）、不写 part_shots/part_points、原图按 {相机}_s{seq:06}.pgm 命名并与 part.json 一致
- `log-replay 段配方（e2e）` — 原包重跑按区域逐一与原记录一致（划分、结论、帧数、不完整组、胶宽统计）；原图缺失时不悄悄当作完整；时间线与运行日志按 (相机, seq) 对帧
- `soak/faults 段配方判据与 pulseJitter 场景` — 段配方下缺帧/坏帧/缺图的件必须标检测不完整（不再按错误的 OK 判），脉冲首尾 ±1 不判疑似提前终止

**review-fix-segment-burst-backend**
- `workspace::burst_release_tests::ready_burst_candidate_yields_debug_run_inputs_in_plan_camera_order`（eace42b 拆成两个用例，现名见末节）— 段配方候选试运行不能被旧拍照点的 Recipe::ready 拒绝（决策 §九-13/14、规格 §6）；示教帧尺寸必须按计划相机（inspect）顺序交给 Candidate::new_burst。做过变异检验：去掉 debug_recipe 的分流后测试失败。
- `vision::tests::calibration_page_view_on_single_view_camera_also_writes_the_key_burst_recipe_reads` — D-SB-30 前后端契约：标定页对单视角相机传 view=1，写出的键必须包含段配方 camera_calib_key 读取的 {device}，读回键仍为 {device}-v1；三目拼接相机的图 N 不写进段配方的设备键。
- `store::tests::burst_record_replay_is_recognised_only_when_every_frame_matches` — 从原落库往返用例拆出（不是新规则）：审计重放身份，帧内容相同即认出，任一帧指标不同则认不出（D-SB-29）。
- `cycle::tests::part_completes_only_after_end_once_every_camera_reaches_device_triggers_minus_one` — 从原配组用例拆出：收尾规则，partEnd 前不判；报 6 时等待；报 5 时判定（规格 §3）。
- `cycle::tests::judged_burst_part_record_is_accepted_by_store` — 从原配组用例拆出：节拍生成的段配方记录满足 C5 落库约束（D-SB-29）。

**review-fix-e2-fe-polish-wire**
- `产线没跑满设定时长就停了 → 不达标，失败种类只有 stoppedEarly（tests/native/throughput-gate.test.mjs）` — 空转或中途故障停掉的运行不能通过吞吐门槛（site_status.running 变 false 时 stoppedEarly 必须参与判定）
- `实际帧率只有需求的 88%（要求至少 90%） → 不达标，失败种类只有 notSustained` — 门槛含义是持续满负载：实际帧率低于 3×hz 的 90% 默认判不达标，防止判定被放宽或丢掉（样本集平均 25.9 vs 75 不能再 passed=true）
- `样本集式的突发（实际帧率 25.9，需求 75）、显式放行 → 分项达标，但持续负载仍标未验证` — --burst true / -AllowBurstOnly 的放行路径只能放行分项，sustainedVerified 必须如实为 false，不能冒充持续门槛通过
- `产线没跑满设定时长就停了、即使显式放行突发 → 仍不达标` — 放行开关不能盖住提前停止
- `segments-page-leave：换了配方 → 编辑提交到原配方，新配方照常加载出来（tests/segments-page-leave.test.tsx，真实 WorkspaceProvider）` — 换配方时卸载提交不得经 act（act 抬高请求序号会打断 select 对新配方的加载，工作区停在空白）；删行检验：分支条件改成恒走 act 时该用例失败
- `segments-page-leave：仍在这个配方 → 提交后工作区拿到返回的新修订` — 仍在同一配方时卸载提交必须经 act，让工作区拿到新修订，否则之后的保存被后端按修订冲突拒绝；删行检验：改成恒直接提交时该用例失败
- `segments-page：改了帧数容差、防抖没到期就离开 → 提交改动后的段列表，带当前配方与修订（从参数化用例拆出）` — 卸载时合法编辑不丢，且带正确配方与修订提交
- `segments-page：帧数容差清空成空值（校验不过）、防抖没到期就离开 → 不提交（从参数化用例拆出）` — 校验不过的段列表不会被提交给后端

**review-fix-DOC2-E2-D1**
- `soak-faults-assess: 脉冲少了 2 个、帧组数 10 低于下限 11 并标疑似提前终止、结论仍合格 → 不算失败` — 防止回到“波动件不应标疑似提前终止”的随机失败判定（样本集标定容差 1，首尾都少 1 个脉冲约 1/9 概率触发）
- `soak-faults-assess: 帧组数 12 在容差内却标了疑似提前终止 → 失败` — 防止标记核对被整个去掉：帧组数没低于 ΣN_exp−Σtol 时标疑似提前终止是真缺陷
- `soak-faults-assess: 脉冲数波动让本件结论变成 NG → 失败` — 规格 §0：脉冲数波动只改帧数、不改 OK/NG，防止结论被改而长稳仍判通过
- `duplicate_counter_is_dropped_with_alarm_and_the_first_frame_is_kept（拆分自原混合用例）` — 重复帧号只丢弃并告警、保留先到的帧、本轮不失败
- `backward_counter_or_counter_not_above_baseline_fails_round_naming_the_camera（拆分自原混合用例，两种输入一个参数化）` — 帧号回退或不大于基线说明计数不可信，整轮失败并指到该相机
- `s7_wire_teaching_plan_in_a_capture_transaction_arms_with_any_plc_counts（拆分）` — 规格 §5：示教采集事务不核对 shotCount/deviceShots（0、32767 且与 deviceShots 不等）
- `s7_wire_teaching_plan_without_a_capture_round_is_rejected（拆分）` — 示教采集计划不能用于没有绑定采集轮次的生产事务

## 删除或改写的存量测试

- [B1] `handshake::tests::part_end_compares_device_count_and_frozen_request` — 改写为 part_end_with_changed_request_identity_is_rejected：计数相等的部分被 §5 推翻，只保留身份冻结部分
- [B1] `handshake::tests::malformed_or_unreleased_request_cannot_arm` — 删去 deviceShots≠shotCount 用例（被 §5 推翻，改由 start_request_with_unequal_or_zero_counts... 覆盖反向规则）和协议版本 2 用例（移到 plc_writing_protocol_5_cannot_arm）；其余用例保留
- [B1] `plc_session::tests::s7_wire_end_trigger_count_mismatch_rejects_completion_and_requires_neutral_reset（及辅助函数 reject_end_and_reset）` — §5 规定计数不符不再判故障；改写为 s7_wire_part_end_with_trigger_count_unlike_plan_completes_and_reports_triggers
- [B1] `offline::tests::offline_line_scenarios_end_in_err_result_or_handshake_fault（CountMismatch 用例）` — CountMismatch 不再期望握手故障，改为断言按实收帧判缺帧 ERR (90/91) 并回到 Idle；LostFrame、AckMismatch 两个用例不变
- [C2] `camera::delivery::tests full_frame_channel_advances_callback_ledger_counts_drops_and_recovers / full_callback_channel_finishes_missing_first_middle_and_last_as_plc_error / extra_callback_dropped_by_full_channel_overrides_complete_ok_with_plc_error` — Mechanical change only: the hard-coded counters 65..73 became FRAME_QUEUE + n. With the queue at 1024 the old counters collided with frames already queued. Assertions are unchanged; the plan leaves the ERR-expectation rewrite to C3
- [DOC1] `test_single_device_counts_use_the_protocol5_wire_offsets（改名为 test_shared_trigger_counts_sit_at_their_fixed_wire_offsets，断言不变）` — 原名写死“单设备”“协议 5”，与协议 6 三台相机共用触发不符；它仍然保护 deviceShots(DB4.DBW20)、deviceTriggers(DB4.DBW32) 的线上偏移，不删除
- [C1] `recipe::tests::obsolete_recipe_file_is_listed_as_unavailable_and_never_loaded_for_running` — 改名为 schema_7_recipe_file_is_listed_unavailable；obsolete_store 改为写出不带段模型字段的 schema 7 文件（字面值 7），其余 obsolete_* 用例也随之覆盖 schema 7
- [C1] `cycle::admission::tests::product_code_checked_before_plan_compile_and_trigger_counts_before_plan_version` — 94 的比较对象改为 ΣN_exp，改写为段配方版本 admission_checks_product_then_plan_then_counts_against_sum
- [C1] `plc_session::tests::s7_wire_teaching_capture_uses_single_device_and_holds_until_release` — 单设备、20 拍照点的计划已过时。改名为 s7_wire_teaching_capture_holds_capture_id_until_release，改用三相机计划夹具，只保留“绑定 capture_id 直到释放”
- [C1] `plc_session/tests.rs Rig 与 offline/tests.rs Bench 的计划夹具` — 改为 samples_burst 的段计划（三台相机，ΣN_exp = 4），断言不变。现在所有会话用例都会经过段计划的 validate_plan 路径
- [FE2] `tests/progress-page.test.tsx：groups() 辅助与 4 个“每组只选一张图”用例（每组只选一图且存在未选相机阻塞；检测图为 %s 不计有效示教；未选相机报告重采；所有组有效但后台仍指向未选相机）` — 计划 tests_to_delete_or_rewrite 列出（progress-page.test:49-109）：规格 §0 所有帧参与检测、不裁帧，每组选一图的规则被推翻；由段配方走廊示教进度用例取代
- [FE2] `tests/validation.test.tsx：拍照点数变化 → 发布确认直接列出 PLC 要同步的触发数（原 :65-71）` — 计划列出：shotCount 改为 ΣN_exp 与段序列；改写为段配方的发布确认用例
- [FE2] `tests/recipe-create-flow.test.tsx：新建模板与进度断言` — 后端新建模板改为段配方（D-SB-24）：模板改为三台相机加一个待标定直线段，断言按段模型步骤显示；并给该长流程用例加 20 s 超时（同机并行时默认 5 s 偶尔不够）
- [FE3] `tests/fixture-teach.test.tsx（整个删除，7 个用例）` — 全部用例走示教页的特征定位界面（定位框、喷嘴确认、转角证据、量具确认件），该界面不再挂载；规格 §4 特征定位停用。计划 tests_to_delete 把它记在 FE6 名下，FE6 已取消，但本包停用挂载后必然失效且类型不再通过 typecheck
- [FE3] `teaching-flow.test: 按 Pose 组选图、默认选图/失败重试、同组切换检测相机、保存并进入下一组、逐点试测候选宽度、标定验证复用、样本集声明当量等 9 个用例` — 组选图被规格 §0 推翻；试测、按组保存、逐点中线比对与标定验证复用在段模型中不存在；其中检测参数恢复默认、当量隐藏搜索带、自动保存、刷选提取、重采样、提取失败转笔划、连续失败建议已改写为相机走廊版本。旧配方的总览、独立验证样本、验证发布、步骤导航 4 个跨页用例原样保留
- [FE3] `teaching.test: 试测过期、保存参数失败不试测、拍照组不能切换、最后一组、试测结论中文 5 个用例` — 试测与按组保存不再存在；保留并改写示教帧曝光/增益变化提示，其余换成走廊示教用例
- [FE3] `recipe-capture.test: 本轮已采用为示教图像、个别拍照点尺寸不一致 两个用例；异常轮、验证样本互斥、已入样本库、工件进行中、切换帧组 等用例改写` — “采用本轮图像”与按拍照点展开不再成立（示教轮次不核对计数）；其余改写成段配方版本。设备准备、相机勾选、PLC、飞拍超速等与计数无关的用例原样保留
- [FE3] `workspace-context.test: 拍照点视角变化只清该点草稿、保存失败保留编辑内容 两个用例` — 草稿键由拍照点下标改为相机 id，改写为按相机 id 的版本并补一条草稿保存后清掉的规则
- [FE3] `teach-draft.test: 特征定位草稿保留、新定位框经后端保存后与草稿相同 两个用例` — 走廊草稿不含定位框，特征定位草稿路径已不在本页使用；改写为走廊草稿用例
- [FE3] `trial-result.test: 逐点偏移表两个用例` — TrialResult 删除相对示教中线偏移，改为 FrameMetrics 的逐站胶宽、断胶与 x，用例相应改写
- [FE5] `tests/site-mode.test.tsx: 原用例「离线来源样本集 → 只提供握手类工况，不提供断胶、定位失败等需要改图像的工况」` — 改写（同一用例改名并扩充）：计划 tests_to_delete_or_rewrite 要求工况列表增加新工况；断言补入五个新工况，并把 random 列入样本集不可选。
- [FE5] `tests/flyshot-calibration.test.tsx（视角选择相关断言）、tests/plc-s7-template.test.tsx（协议版本文案断言）` — 计划要求删除失效断言，但这两个文件里没有视角选择或协议版本文案断言（已逐行读过），所以没有删除，也没有改动这两个文件。
- [FIX-B1] `cycle::tests::next_part_baseline_follows_plc_reported_triggers_not_plan_or_frames_seen` — 它保护的规则（按 PLC 报数推基线）与 D-SB-7 相反，已改写为 three_frames_seen_plc_reports_four_next_part_first_frame_is_counter_4。
- [FIX-B1] `plc_session::tests::journal_pending_from_protocol_5_is_archived_and_requires_reset` — 拆分。原名测试只保留归档和复位，ACK 恢复部分移到 audit_transaction_from_protocol_5_is_skipped_by_ack_recovery_without_error。
- [FIX-B1] `shot_router::tests::late_frames_of_the_previous_part_cannot_satisfy_the_next_part` — 已删除。它保护的“下限 = 基线 + 已发触发数”（断言基线 44）被 D-SB-7 推翻。新规则由 cycle 层测试经 close_routing → ShotRouter::close → arm 覆盖，不在下层重复测。
- [FIX-B1] `cycle::tests::same_sn_reinspection_rejects_old_cycle_and_every_other_identity_mismatch` — 只改了前置步骤。原来依赖按计划数推出的基线 4（帧 2 迟到、帧 5 为首帧），改成新规则下的基线 1（帧 1 迟到、帧 2 为首帧）。身份校验断言没改。
- [FIX-B1] `cycle::tests::counter_desync_ends_err91_with_expected_and_received_counters` — 只改注释，去掉“按触发数推的下限”这一说法。测试逻辑没改。
- [FIX-RUST-TESTS] `burst_judge::tests::corner_region_never_judges_offset_and_does_not_flag_it -> corner_region_does_not_flag_offset_unjudged` — 改写：原用例的弯道段本来就没有 offset 检测，改错 Corner 分支也过；现在弯道段带启用的胶偏检测与参考值，改错分支会失败。保护规则：规格 §4 弯道区域不判胶偏、不标“胶偏未判”。
- [FIX-RUST-TESTS] `vision::taught::engine_tests::throughput_probe` — 删除 #[test] 形式：探针没有要防的回归，按准入改为 examples/throughput_probe.rs，不在 cargo test 内。
- [FIX-FE-LEGACY] `teaching：旧拍照点配方（没有段计划）→ 示教页只说明该配方已停用` — 旧配方现在走 LegacyTeachingPage；由恢复的 G0 前单帧示教用例（试测过期、保存参数失败、未试测修改不能切组、最后一组、局部超差、采集信息提示）覆盖
- [FIX-FE-LEGACY] `teaching：下一步（cameraTeachingAction 参数化）` — 函数无生产调用方，函数与测试一并删除
- [FIX-FE-LEGACY] `teaching：三台计划相机首个用例、段配方总览用例` — 去掉断言拍照组选图/定位特征/帧框“不存在”的行，标题同步
- [FIX-FE-LEGACY] `recipe-capture：旧拍照点配方的完整轮次 → 只读查看，没有采用按钮` — 固化旧配方停用；改为采用本轮图像用例，并恢复 FE3 删除的旧配方采集用例（多相机按拍照点读图、异常轮不能采用、已采用进入单帧示教、已入样本库不能采用、工件进行中禁止采用、尺寸不一致、切换拍照点保留当前图）
- [FIX-FE-LEGACY] `recipe-capture：多相机%s切换相机（旧轮次一项）` — 旧配方现由 LegacyCapturePage 处理；该项改为段配方读旧格式轮次（D1 前），保留对段配方采集页按触发序号读图的覆盖
- [FIX-FE-LEGACY] `trial-result：FrameMetrics 版本两个用例` — FrameTrialResult 没有页面挂载；文件恢复为 G0 前旧 TrialResult 用例
- [FIX-FE-LEGACY] `recipe-calibration：多段计划各段标待标定、单段计划显示 N_exp/min–max、旧配方只说明标定只用于段配方` — 只回显夹具字段或静态文案；“按所选类型加入”去掉断言失败轮次按钮不存在的行
- [FIX-FE-LEGACY] `inspection-diagnostics：段配方选中的帧组缺一台相机` — 整行 textContent 拼接改为按表头取“相机/帧状态/设备帧计数”列断言
- [FIX-FE-LEGACY] `workspace-context：两条走廊草稿用例与 teaching 三条走廊草稿用例` — 段配方草稿改名 corridorDrafts/setCorridorDraft；同时恢复两条按拍照点下标的旧草稿用例
- [FIX-FE-LEGACY] `teaching-flow：段配方提取相关用例` — 命令改为 workspace_extract_corridor、参数 camera 取代 k；并按原样恢复 0ec2cc3 中按拍照组的单帧示教用例
- [FIX-FE-LEGACY] `teach-draft：恢复两条拍照点 fixture 草稿用例` — teach.ts 旧函数仍由 LegacyTeachingPage 使用，FE3 删除的用例按原样恢复
- [FIX-FE5] `tests/site-mode.test.tsx：离线来源样本集 → 只提供握手与脉冲串类工况（含分段连拍新增五个）…` — 该用例镜像 offline.ts 常量白名单，并把后端不存在的五个变体固化为期望；改写为合成图与样本集下拉差集断言，保护“样本集不能选需要改图像的工况”。变异检查：白名单混入 gap 后新用例失败。
- [FIX-FE2-TESTS] `tests/recipe-editor.test.tsx 段配方默认参数四合一用例` — 违反一个行为一个测试，改写为四个用例
- [FIX-FE2-TESTS] `tests/segments-page.test.tsx 直线段改成弯道 三合一用例` — 混合清空、隐藏胶偏、保存参数，拆成三个用例
- [FIX-FE2-TESTS] `tests/segment-edit.test.ts 只改长度、容差或检测项 → 标定保留` — 对象展开的同义反复，改为经 SegmentsPage 接线的参数化用例
- [FIX-FE2-TESTS] `tests/recipe-run-select.test.tsx 参数化用例` — 夹具字段后端不输出，改为 RecipeSummary 真实字段，并加摘要未取到的行
- [C5] `audit::tests::old_format_spool_file_is_kept_and_refuses_readiness_with_cleanup_hint` — spool FORMAT 3→4，改写为 spool_with_old_format_refuses_readiness，参数化覆盖“没有 format”和“format 3”，按计划保留原用例、改版本号
- [C5] `history::tests 存量 5 个用例（rejudge_rows 调用）与 row() 夹具` — 机械修改：rejudge_rows 新增帧记录读取参数，旧记录传入 no_frames（读了即 panic）；StoredMeasurement 夹具补 burst: None。断言不变
- [C5] `store::tests 存量夹具 save_as / neighbors / durable_record_identity 的 PartRecord` — 机械修改：补 burst: None。断言不变
- [C5] `audit::tests 夹具 part() / outcome()` — 机械修改：RecordedPart 补 burst: None，RecordedRawFile 补 frame: None
- [C4] `vision::tests::calibration_keys_are_distinct_for_each_view_of_one_device → device_key_and_each_view_key_of_one_device_never_collide` — 标定键 {device} 与 {device}-v{view} 现在并存。保留原有视图键的断言，新增：设备键与任一视图键不冲突，非法设备编号被拒绝。
- [D1] `tests/recipe-capture.test.tsx「新轮次的读图命令后端还没有 → 预览显示该功能待后端 D1 实现的占位」` — D1 已注册 recipe_capture_frame_image，前端 captureFrameImage 改为 desktopCall 直接调用，PendingBackendError(D1) 路径不再可达
- [FE-POLISH] `rejudge-dialog: 填写直线段、弯道段胶宽允差 → 批量重判请求带 straight / corner 覆盖` — 后端还不读这两个字段，输入已置灰，原用例不再成立；改写为上面两条。C5 读取后需把 SEGMENT_OVERRIDES_SUPPORTED 改为 true 并换掉置灰用例
- [FE-POLISH] `恢复 validation: 拍照点数变化 → 发布确认直接列出 PLC 要同步的触发数（旧配方），原文恢复` — G2 的 FE2 误删；旧配方发布确认分支（plcSyncItems、candidatePlc 对比）仍保留
- [FE-POLISH] `恢复 progress-page 旧配方组选图用例（groups 构造 + 每组只选一图/ unselected,conflict,missing,stale / 未选相机报告重采 / 所有组有效但后台仍指向未选相机），原文恢复，全部通过` — LegacyProgressPage 仍保留，旧路径行为不变
- [FE-POLISH] `恢复 recipe-calibration 断言：失败轮次不提供加入按钮` — 修复时删过头；异常轮不能用于标定。已做变异验证：去掉 state==='complete' 过滤后该断言失败
- [FE-POLISH] `恢复 segments-page 断言：换成弯道后检胶偏开关消失` — 原先只断言文案；已做变异验证：同时显示文案与开关时该断言失败
- [C3] `cycle::tests::three_frames_seen_plc_reports_four_next_part_first_frame_is_counter_4` — 删除：它固定的是 FIX-B1 给旧配方改的 max(基线, high) 规则，这条规则已撤回。改为恢复 FIX-B1 之前的 next_part_baseline_follows_plc_reported_triggers_not_plan_or_frames_seen；段配方的新规则由 next_part_baseline_is_each_camera_highest_frame_counter_seen 保护
- [C3] `cycle::tests::same_sn_reinspection_rejects_old_cycle_and_every_other_identity_mismatch` — 前置步骤恢复到 FIX-B1 之前：close(&mut ledgers, None) 按计划数推基线，迟到帧/首帧回到计数 2/5
- [C3] `cycle::tests::counter_desync_ends_err91_with_expected_and_received_counters` — 只恢复原注释；旧规则回来后，“下限高于实际计数”在生产中又能出现，这条测试仍然有效
- [C3] `burst_judge::tests::straight_offset_with_to_edge_or_without_reference_is_unjudged_not_ng` — 改名为 straight_offset_with_to_edge_without_reference_or_without_usable_frames_is_unjudged_not_ng，参数表加一组 NoFrames（F0-3）
- [C3] `camera::tests::aggregate_bandwidth_over_one_port_budget_is_flagged` — F0-4 计入 GigE 协议开销：期望值改为 826.2/660.9/275.4 Mbps；三台 25 Hz 现在判为超预算
- [C3] `handshake::tests::plc_writing_protocol_5_cannot_arm` — F2-3：参数表改为由 PROTOCOL_VERSION 派生
- [E1] `offline::tests::offline_line_scenarios_end_in_err_result_or_handshake_fault` — 改名 legacy_recipe_line_scenarios_end_in_err_result_or_handshake_fault 并改用旧拍照点计划的测试台：保护仍存在的旧配方行为（丢帧/少发触发判 ERR 90、ACK 错位进故障），按硬性规则旧路径保留；段配方计数只告警由新用例覆盖
- [E1] `offline::tests::sample_set_line_fetches_each_trigger_by_part_shot_and_view_and_claims_the_sample_by_part_sn` — 改用旧拍照点计划测试台（此前是段计划配旧节拍混用），保护旧配方按件/拍照点/视角取图与认领，保留
- [E1] `offline::tests 正常件/低磁盘/布防延迟/崩溃恢复用例` — 测试台默认改为段计划的脉冲串（run_burst + 段配方判定），PartOrder 改用新构造；断言不变
- [E1] `site::tests::entering_offline_derives_temporary_environment_by_source` — 合成图一行改为沿用现场相机并补足到 3 台单视角；新增现场已有三台单视角不补的一行
- [FE-WIRE] `tests/plan-pending.test.tsx（段配方而 plan 为 null → 显示待后端 D3 占位，整文件删除）` — 后端自 D2 起只对 doc.segments 非空的候选输出 plan，PlanPending/awaitingPlan 占位随之删除。新旧页面分流已由 teaching / teaching-flow / recipe-capture 的现有用例覆盖。
- [FE-WIRE] `teaching-flow: 后端还没有走廊提取命令 → 画布提示该功能待后端 D2 实现` — workspace_extract_corridor 已注册，pendingBackend 已删除，占位不再存在。
- [FE-WIRE] `rejudge-dialog: 打开批量重判 → 直线段、弯道段两行输入全部置灰；填了位置上公差 → 请求只带旧的位置与胶宽覆盖` — C5 已读取 straight/corner，置灰取消。两条用例合并改写为上面的契约用例。
- [FE-WIRE] `recipe-capture: 段配方多相机切换相机的旧格式轮次参数项` — 只改了用例标签和夹具注释，去掉过期的“D1 前后端仍给”说明，断言没变。
- [E2] `tests/native/offline-e2e.mjs 主流程` — 计划推翻旧的 plannedCount×相机数、按 Pose 示教、人工断胶件流程：默认改写为段配方流程；旧流程未删，保留为 --model legacy（新建起点改 legacyTemplate），legacy 实跑通过
- [E2] `tests/native/log-replay-offline.mjs` — 改为按区域比对（默认 burst）；旧的逐拍照点比对保留为 --model legacy，实跑通过
- [E2] `tests/native/production-flow-ui.mjs 离线相机组断言` — E1 起合成图离线环境把相机补足到 3 台单视角，断言改为沿用真机相机组并补足；旧配方一段（含定位失败工况）原样保留
- [E2] `scripts/soak/faults.mjs 的 cameraLostFrame 错误 OK 判据` — 规格 §0 缺帧不影响 OK/NG：段配方下加入 MAY_PASS 并改核对检测不完整标记；旧配方判据不变
- [E2] `tests/native/fixture-teach-e2e.mjs` — 计划写删除，按旧模型只停用不删除保留并标注仅旧配方；新建起点改 legacyTemplate，并在页面已停在画框状态时不再点按钮（首次运行卡在第二个框），实跑通过
- [review-fix-segment-burst-backend] `workspace::burst_tests::multi_segment_calibration_writes_part_total_and_leaves_segments_pending_on_segmenter` — 原断言 pending == Some("分段算法未就绪") 是界面文案全文比较，改为只断言有待定原因（pending.is_some()）。
- [review-fix-segment-burst-backend] `workspace::burst_tests::standard_rounds_set_x_ref_for_straight_segment_and_refuse_corner` — 原断言 x_ref_pending == Some("测量量未实现") 是文案全文比较，改为布尔：是否列出待定原因（只有 ToEdge 为是）。
- [review-fix-segment-burst-backend] `store::tests::burst_record_round_trips_frames_flags_regions_and_device_triggers` — 一个用例测了两个行为。matches_record 部分拆到 burst_record_replay_is_recognised_only_when_every_frame_matches，原用例只保留落库往返。
- [review-fix-segment-burst-backend] `cycle::tests::three_camera_frames_out_of_arrival_order_are_grouped_and_part_judged_after_end` — 拆成三个用例：three_camera_frames_out_of_arrival_order_are_grouped_by_frame_counter_minus_baseline（配组）、part_completes_only_after_end_once_every_camera_reaches_device_triggers_minus_one（收尾规则）、judged_burst_part_record_is_accepted_by_store（落库约束）。
- [review-fix-segment-burst-backend] `offline::tests::trigger_frame_count_gap_over_one_alarms_without_fault` — 改名为 plc_counting_two_extra_pulses_alarms_every_camera_without_fault，只保留 countMismatch 一组。差 1 不告警、差 2 告警这两组与 frame_group::tests::camera_count_differing_by_more_than_one_raises_loss_alarm 属同一等价类，已删除。
- [review-fix-e2-fe-polish-wire] `吞吐门槛：各条件逐一不满足 → 不达标并点明原因，全部满足 → 达标` — 1 个用例里循环塞 8 组不同规则、用正则匹配中文失败文案、含重复断言，违反“一个行为一个测试”与“不测文案”；改写为每条规则一个用例（Unmeasured、预算丢帧、队列峰值恰为一半的边界、测量失败、帧通道满、没有出件，另加基线、边界下侧），只断言 passed 与 failureKinds 种类。assessThroughput 因此新增 failureKinds 返回值
- [review-fix-e2-fe-polish-wire] `$edit、防抖没到期就离开 → 提交 $calls 次` — 把“合法编辑要提交”和“校验不过不提交”两个行为放进一个参数化用例，测试体里 if (calls) 分支，并断言 ws.act 调用次数（mock 调用次数）；拆成两个用例，只验证 updateSegments 收到的配方、修订与内容
- [review-fix-e2-fe-polish-wire] `换了配方、防抖没到期 → 不经 act，直接提交到原配方` — 核心断言 expect(ws.act).not.toHaveBeenCalled() 只镜像实现选择，要防的回归在 act 被 mock 时观察不到；删除，改由 tests/segments-page-leave.test.tsx 里带真实 WorkspaceProvider 的集成用例承担（断言工作区最终状态）
- [review-fix-DOC2-E2-D1] `duplicate_counter_is_dropped_with_alarm_but_backward_or_stale_counter_fails_round` — 一个用例混了两种行为并用 if fails 分叉断言，拆成上面两个
- [review-fix-DOC2-E2-D1] `s7_wire_teaching_plan_arms_with_any_plc_counts_only_in_a_capture_transaction` — 同时验证“计数不核对”与“未绑定轮次被拒绝”，且计数组 (65,200) 与 (32767,1) 同类；拆成两个用例并去掉 (65,200)，launch-core-flow-checklist.md 的引用同步
- [review-fix-DOC2-E2-D1] `plc_estimate_of_zero_or_above_64_is_only_recorded_and_round_completes_on_actual_frames` — 参数去掉 200（与 65 同一等价类），保留 0、65、32767
- [review-fix-DOC2-E2-D1] `lost_packet_budget_dropped_or_queue_full_frame_is_missing_with_alarm_and_round_completes / failed_round_reports_device_or_process_fault_not_count_mismatch` — 私有内部状态的改动集中成三个辅助函数，丢包/预算满/队列满改表驱动；行为断言不变；真实写盘线程在测试里造不出停顿，未改成纯公开入口
- [review-fix-DOC2-E2-D1] `offline-e2e.mjs assertBurstPart（脚本内断言，非单元测试）` — 波动件不再要求落在 ± Σtol 内，改为只核对标记与帧组数一致；其他件仍要求落在容差内、不告警


## 复核修复新增的测试（2d30515 / c977cbd / eace42b）

- `tests/native/throughput-gate.test.mjs`：15 个用例，用合成的“帧到达”记录驱动吞吐门槛判定——件间握手 0.9 s 且窗口内满 3×25 Hz → 达标；窗口内只有需求的 88% → notSustained；只有 9 件 → sustainedTooShort；15 件中第 8 件掉速、最长连续 7 件 → sustainedTooShort；显式放行但有帧未测成 → burstOnlyAccepted=false（保护 burstOnlyAccepted 必须同时要求 passed）。
- `vision::tests::camera_id_ending_in_v1_device_calibration_never_overwrites_another_cameras_legacy_calibration`（参数化三组）：相机 id 以 -v1 结尾时，段配方的设备标定不覆盖另一台相机的旧配方图 1 标定（设备标定放 `calib/device/` 子目录）。
- `vision::tests::view_number_out_of_range_or_device_id_that_forms_a_path_is_rejected`：改写自 `device_key_and_each_view_key_of_one_device_never_collide`，视角越界或设备 id 含路径分隔符时拒绝。
- `workspace::burst_release_tests::ready_burst_candidate_is_frozen_for_debug_run_by_burst_readiness`：就绪的段配方候选按 ready_burst 冻结出段配方（改用 Recipe::ready 时失败）。
- `workspace::burst_release_tests::burst_candidate_debug_run_gets_teaching_frame_sizes_in_plan_camera_order`：经 debug_run_sizes 的真实分派得到 DebugSizes::Burst，示教帧尺寸按计划相机顺序（改回旧拍照点尺寸或按登记顺序时失败）。
