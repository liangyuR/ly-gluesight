use super::*;
use crate::frame::CounterSource;

/// 段配方在途件：samples_burst（cam1–cam3 共用触发、一个直线段 N_exp = 4、容差 1），相机组序号 0–2、会话 11–13，
/// 按计数账定帧计数基线（模拟相机本会话没见过帧时从 0 起）。
fn burst_part(cycle_id: &str, ledgers: &Ledgers) -> Part {
    armed_part(
        cycle_id,
        ledgers,
        Some(CounterSource::Synthetic),
        Policy::burst_offline(None),
    )
}

/// 同 [`burst_part`]，指定布防前已知的计数来源（海康为空）与布防策略。
fn armed_part(
    cycle_id: &str,
    ledgers: &Ledgers,
    source: Option<CounterSource>,
    policy: Policy,
) -> Part {
    armed_part_with(
        crate::recipe::samples_burst().build().unwrap(),
        cycle_id,
        ledgers,
        source,
        policy,
    )
}

/// 同 [`armed_part`]，指定配方。
fn armed_part_with(
    recipe: Recipe,
    cycle_id: &str,
    ledgers: &Ledgers,
    source: Option<CounterSource>,
    policy: Policy,
) -> Part {
    let recipe = Arc::new(recipe);
    let cameras = recipe.plan_cameras();
    let identities: Vec<ArmCam> = cameras
        .iter()
        .enumerate()
        .map(|(cam, id)| ArmCam {
            cam: cam as u8,
            camera: id.clone(),
            session: cam as u64 + 11,
            source,
            counter_after_open: None,
        })
        .collect();
    let baselines = arm_frames(&cameras, ledgers, &policy, &identities).unwrap();
    Part {
        mode: crate::store::PartMode::Production,
        cycle_id: cycle_id.into(),
        bundle_id: None,
        production: None,
        candidate: None,
        sn: "SN-88".into(),
        scenario: Scenario::Normal,
        burst: Box::new(burst::BurstPart::arm(&recipe, baselines, policy.trusted)),
        device_triggers: None,
        routing_closed: false,
        image_measurement: false,
        proc_timeout: Duration::from_secs(1),
        drain_timeout: Duration::from_secs(2),
        armed_at: Instant::now(),
        end_at: None,
        fault: None,
        cams: vec![0, 1, 2],
        camera_ids: cameras,
        rig_gen: 1,
        source: RecordSource::Real,
        recording: None,
        recipe,
    }
}

/// 相机组序号 cam（会话 11 + cam）帧计数为 counter 的一帧。
pub(crate) fn burst_frame(cam: u8, counter: u64) -> Frame {
    Frame { cam, session: cam as u64 + 11, counter: CounterSource::Synthetic, frame_counter: counter, trigger_counter: counter,
        lost_packets: 0, ts: 1, manual: false, images: Vec::new() }
}

/// 测量池交回的一帧结果。
fn frame_result(
    part: &Part,
    camera: usize,
    seq: u64,
    result: Result<crate::burst_judge::FrameMetrics, measure::MeasureError>,
) -> FrameMeasured {
    FrameMeasured {
        cycle_id: part.cycle_id.clone(),
        camera_idx: camera,
        seq,
        segment: 1,
        result,
        ms: 5,
        queue_ms: None,
        engine_ms: None,
    }
}

/// 收一帧；被接受时交回合格的模拟测量结果。
fn deliver(part: &mut Part, cam: u8, counter: u64) -> burst::Arrival {
    let arrival = part.receive_burst(&burst_frame(cam, counter)).unwrap();
    if let burst::Arrival::Accepted { camera, seq } = arrival {
        let result = frame_result(
            part,
            camera,
            seq,
            Ok(measure::simulate_frame(
                Scenario::Normal,
                camera,
                seq,
                1,
                &part.recipe,
            )),
        );
        part.apply_burst(result).unwrap();
    }
    arrival
}

/// partEnd：PLC 报 device_triggers；expired 为真时收尾截止已过。
fn end(part: &mut Part, device_triggers: Option<u16>, expired: bool) {
    part.end_at = Some(Instant::now() - if expired { part.drain_timeout } else { Duration::ZERO });
    part.device_triggers = device_triggers;
}

fn judge_burst(part: &mut Part) -> BurstJudgement {
    let judgement = part.burst_judgement();
    part.burst.judgement = Some(judgement.clone());
    judgement
}

/// 回调交付的帧（丢掉的帧不在其中）走段配方节拍；partEnd 时 PLC 报 device_triggers，判完返回结论。
pub(crate) fn judge_burst_delivered(received: Vec<Frame>, device_triggers: Option<u16>) -> BurstJudgement {
    let mut p = burst_part("callback-dropped", &Ledgers::default());
    for frame in received {
        if let Some(burst::Arrival::Accepted { camera, seq }) = p.receive_burst(&frame) {
            let result = frame_result(
                &p,
                camera,
                seq,
                Ok(measure::simulate_frame(
                    Scenario::Normal,
                    camera,
                    seq,
                    1,
                    &p.recipe,
                )),
            );
            p.apply_burst(result).unwrap();
        }
    }
    end(&mut p, device_triggers, true);
    assert!(p.complete(), "已接受的帧都测完且收尾窗口已过");
    judge_burst(&mut p)
}

/// 离线产线跑完一件后的段配方判定：交付的帧都已测完，PLC 报的 device_triggers 多于各相机帧数时不再等剩余帧，
/// 按收尾截止已过判（与生产节拍等到 drain 截止后的结论相同）。
pub(crate) fn judge_burst_at_drain_deadline(received: Vec<Frame>, device_triggers: Option<u16>) -> BurstJudgement {
    let mut p = burst_part("offline-burst-line", &Ledgers::default());
    for frame in received {
        if let Some(burst::Arrival::Accepted { camera, seq }) = p.receive_burst(&frame) {
            let result = frame_result(
                &p,
                camera,
                seq,
                Ok(measure::simulate_frame(
                    Scenario::Normal,
                    camera,
                    seq,
                    1,
                    &p.recipe,
                )),
            );
            p.apply_burst(result).unwrap();
        }
    }
    end(&mut p, device_triggers, true);
    assert!(p.complete(), "收尾截止已过");
    judge_burst(&mut p)
}

/// 三台相机按帧计数 1..=groups 依次到齐，每帧都测完（帧计数基线为 0）。
fn deliver_groups(part: &mut Part, groups: u64) {
    for counter in 1..=groups {
        for cam in 0..3u8 { deliver(part, cam, counter); }
    }
}

/// 规格 §3：三路帧按 SDK 帧号（减各相机基线）配组，不按到达顺序。
#[test]
fn three_camera_frames_out_of_arrival_order_are_grouped_by_frame_counter_minus_baseline() {
    let mut ledgers = Ledgers::default();
    for (cam, base) in [(0u8, 100u64), (1, 200), (2, 300)] {
        ledgers.observe(&FrameMeta::from(&burst_frame(cam, base)));
    }
    let mut p = burst_part("burst-order", &ledgers);
    // cam1 倒序到达，三台交错
    for (cam, counter) in [(0, 104), (1, 201), (0, 103), (2, 302), (1, 202), (0, 102), (2, 301), (1, 204), (0, 101), (2, 304), (1, 203), (2, 303)] {
        assert!(matches!(deliver(&mut p, cam, counter), burst::Arrival::Accepted { .. }), "相机 {cam} 帧计数 {counter}");
    }
    end(&mut p, Some(4), false);
    let judgement = judge_burst(&mut p);
    assert_eq!((judgement.verdict, judgement.group_count, judgement.flags.incomplete), (Verdict::Ok, 4, false));
    let record = p.burst_record(&burst::summary(&judgement));
    let identities: Vec<(usize, u64, Option<u64>)> = record.frames.iter().map(|f| (f.camera, f.seq, f.frame_counter)).collect();
    let expected: Vec<(usize, u64, Option<u64>)> = (1..=4u64).flat_map(|seq| (0..3usize).map(move |camera| (camera, seq, Some([100, 200, 300][camera] + seq)))).collect();
    assert_eq!(identities, expected);
}

#[test]
fn part_end_waits_full_drain_window_regardless_of_reserved_trigger_count() {
    for count in [None, Some(0), Some(4), Some(u16::MAX)] {
        let mut p = burst_part("burst-end", &Ledgers::default());
        deliver_groups(&mut p, 4);
        assert!(!p.complete());
        end(&mut p, count, false);
        assert!(!p.complete());
        deliver_groups(&mut p, 5);
        assert!(!p.complete());
        end(&mut p, count, true);
        assert!(p.complete());
        assert_eq!(judge_burst(&mut p).group_count, 5);
    }
}

/// 规格 §0 不裁帧、§2 帧数超出 N_exp ± tol 只告警：ΣN_exp 4 ± 1 时到了 7 组，超出的帧照常测、照常判（第 7 组的断胶判 NG），
/// 整件标帧数异常。
#[test]
fn frames_beyond_expected_total_are_all_measured_and_only_alarm() {
    let mut p = burst_part("burst-extra", &Ledgers::default());
    for counter in 1..=7u64 {
        for cam in 0..3u8 {
            let arrival = p.receive_burst(&burst_frame(cam, counter)).unwrap();
            let burst::Arrival::Accepted { camera, seq } = arrival else { panic!("相机 {cam} 第 {counter} 帧没被接受：{arrival:?}") };
            let mut metrics = measure::simulate_frame(Scenario::Normal, camera, seq, 1, &p.recipe);
            if (camera, seq) == (2, 7) { metrics.gap_max_mm = 100.0; }
            let result = frame_result(&p, camera, seq, Ok(metrics));
            p.apply_burst(result).unwrap();
        }
    }
    end(&mut p, Some(7), true);
    assert!(p.complete());
    let judgement = judge_burst(&mut p);
    assert_eq!((judgement.verdict, judgement.group_count, judgement.flags.frame_count_anomaly), (Verdict::NgGap, 7, true));
    let record = p.burst_record(&burst::summary(&judgement));
    assert_eq!(record.frames.iter().filter(|f| f.status == FrameState::Measured).count(), 21);
}

/// D-SB-40：各段 N_exp 已标定的多段配方，生产按比例分段（直线 4、弯道 4、直线 4，余量 1）：12 组的边界落在 4、8，
/// 两侧各 1 帧为过渡区，其余为直线 / 弯道区域；不再整条胶路未分段。
#[test]
fn calibrated_multi_segment_recipe_is_judged_by_proportional_regions() {
    use crate::plan::{
        new_segment,
        SegmentKind::{Corner, Straight},
    };
    use crate::segmentation::{RegionKind, UnsegReason};
    let mut doc = crate::recipe::samples_burst();
    doc.segments = [Straight, Corner, Straight]
        .into_iter()
        .enumerate()
        .map(|(k, kind)| crate::plan::PathSegment {
            expected_frames: Some(4),
            ..new_segment(k as u32 + 1, kind, &doc.segment_defaults)
        })
        .collect();
    doc.boundary_guard = 1;
    let mut p = armed_part_with(
        doc.build().unwrap(),
        "burst-proportional",
        &Ledgers::default(),
        Some(CounterSource::Synthetic),
        Policy::burst_offline(None),
    );
    deliver_groups(&mut p, 12);
    end(&mut p, Some(12), true);
    let judgement = judge_burst(&mut p);
    let transition = |first_segment, last_segment| RegionKind::Unsegmented {
        first_segment,
        last_segment,
        reason: UnsegReason::Transition,
    };
    let regions: Vec<(RegionKind, u64, u64)> = judgement
        .regions
        .iter()
        .map(|r| (r.region.kind.clone(), r.region.first_seq, r.region.last_seq))
        .collect();
    assert_eq!(
        regions,
        vec![
            (RegionKind::Straight { segment: 1 }, 1, 3),
            (transition(1, 2), 4, 5),
            (RegionKind::Corner { segment: 2 }, 6, 7),
            (transition(2, 3), 8, 9),
            (RegionKind::Straight { segment: 3 }, 10, 12),
        ]
    );
    assert!(!judgement.flags.unsegmented);
}

/// D-SB-45：帧到达时按累计 N_exp 预测所属段，只由预测段的检测相机用该段走廊送测，其余相机记“本段不检测”、不送测；
/// 收尾后按比例边界解析区域，对判定需要、但到达时没用对应走廊送测过的（帧, 相机）用留下的原图补测，补测完才收尾；
/// 原图没留住的记没测（检测不完整）。直线 5 帧（三台相机）+ 弯道 5 帧（只由第一台相机），本件 12 帧：比例边界在 6、
/// 预测边界在 5。
#[test]
fn frames_are_measured_with_predicted_segment_corridor_and_shifted_frames_are_remeasured_after_drain(
) {
    use crate::plan::{
        new_segment,
        SegmentKind::{Corner, Straight},
    };
    use crate::recipe::SegmentCorridor;
    // 防的回归：不检测某段的相机也被送测（没有它在这段的走廊，只能量出检测不完整）；比例边界相对预测边界移动后，
    // 移到另一段或进了过渡区的帧仍用到达时的走廊判定；不等补测完就判定；或原图没留住时把没测的帧当测成
    for (case, keep_images) in [("原图留住", true), ("原图没留住", false)] {
        let mut doc = crate::recipe::samples_burst();
        doc.segments = vec![
            crate::plan::PathSegment {
                expected_frames: Some(5),
                ..new_segment(1, Straight, &doc.segment_defaults)
            },
            crate::plan::PathSegment {
                expected_frames: Some(5),
                cameras: vec![doc.inspect[0].camera.clone()],
                ..new_segment(2, Corner, &doc.segment_defaults)
            },
        ];
        doc.boundary_guard = 1;
        let corridor = doc.inspect[0].segments[0].corridor.clone();
        doc.inspect[0].segments.push(SegmentCorridor {
            segment: 2,
            corridor,
        });
        let mut p = armed_part_with(
            doc.build().unwrap(),
            "burst-post",
            &Ledgers::default(),
            Some(CounterSource::Synthetic),
            Policy::burst_offline(None),
        );
        for counter in 1..=12 {
            for cam in 0..3u8 {
                let Some(burst::Arrival::Accepted { camera, seq }) =
                    p.receive_burst(&burst_frame(cam, counter))
                else {
                    panic!("{case}")
                };
                if keep_images {
                    p.burst.retain_image(camera, seq, None);
                }
                if let Some(segment) = p.burst.arrival_corridor(camera, seq) {
                    let result = FrameMeasured {
                        segment,
                        ..frame_result(
                            &p,
                            camera,
                            seq,
                            Ok(measure::simulate_frame(
                                Scenario::Normal,
                                camera,
                                seq,
                                segment,
                                &p.recipe,
                            )),
                        )
                    };
                    p.apply_burst(result).unwrap();
                }
            }
        }
        let arrival = |p: &Part, camera: usize, seq: u64| p.burst.arrival_corridor(camera, seq);
        assert_eq!(
            (arrival(&p, 0, 5), arrival(&p, 0, 6), arrival(&p, 1, 5)),
            (None, None, None),
            "{case}：测完后不再是送测中"
        );
        end(&mut p, Some(12), true);
        assert!(!p.complete(), "{case}：还有要补测的帧，不能收尾");
        let mut jobs: Vec<(usize, u64, u32)> = p
            .start_post_measure(Instant::now())
            .into_iter()
            .map(|(c, s, g, _)| (c, s, g))
            .collect();
        jobs.sort();
        if keep_images {
            assert_eq!(
                jobs,
                [(0, 6, 1), (1, 6, 1), (1, 7, 1), (2, 6, 1), (2, 7, 1)],
                "{case}"
            );
            assert!(!p.complete(), "{case}：补测完之前不收尾");
            for (camera, seq, segment) in jobs {
                let result = FrameMeasured {
                    segment,
                    ..frame_result(
                        &p,
                        camera,
                        seq,
                        Ok(measure::simulate_frame(
                            Scenario::Normal,
                            camera,
                            seq,
                            segment,
                            &p.recipe,
                        )),
                    )
                };
                p.apply_burst(result).unwrap();
            }
        } else {
            assert!(jobs.is_empty(), "{case}");
        }
        assert!(p.complete(), "{case}");
        let judgement = judge_burst(&mut p);
        assert_eq!(
            judgement.flags.incomplete, !keep_images,
            "{case}：{judgement:?}"
        );
        let record = p.burst_record(&burst::summary(&judgement));
        let state = |camera: usize, seq: u64| {
            record
                .frames
                .iter()
                .find(|f| (f.camera, f.seq) == (camera, seq))
                .unwrap()
                .status
        };
        assert_eq!(
            state(1, 10),
            FrameState::NotInspected,
            "{case}：弯道段不由第二台相机检测，不测"
        );
    }
}

/// D-SB-7：下一件每台相机的帧计数基线 = max(原基线, 本件见到的最高帧计数)，PLC 报的 deviceTriggers 与计划数都不参与。
/// 上一件的帧在收尾后才到（帧计数不高于见到的最高值）不能计入下一件；某台相机漏收一个脉冲（PLC 报 4、它只到 3 帧）时，
/// 它下一件的首帧仍算第 1 帧——按“基线 + deviceTriggers”推会把它判成迟到帧，而且每件都丢。
#[test]
fn next_part_baseline_is_each_camera_highest_frame_counter_seen() {
    for (case, cam, delivered, late, next_first) in [
        ("上一件帧计数 3 在收尾后才到", 0u8, [1, 2, 4], Some(3), 5),
        ("漏收一个脉冲的相机", 2u8, [1, 2, 3], None, 4),
    ] {
        let mut first = burst_part("first", &Ledgers::default());
        for counter in delivered { deliver(&mut first, cam, counter); }
        first.device_triggers = Some(4);
        let mut ledgers = Ledgers::default();
        first.close_routing(&mut ledgers);
        let mut next = burst_part("next", &ledgers);
        if let Some(late) = late {
            assert!(matches!(next.receive_burst(&burst_frame(cam, late)), Some(burst::Arrival::Stale { .. })), "{case}");
        }
        assert_eq!(next.receive_burst(&burst_frame(cam, next_first)), Some(burst::Arrival::Accepted { camera: cam as usize, seq: 1 }), "{case}");
    }
}

/// 现场 bug：Line0 高电平门控时软件取不了帧，海康相机开流后本会话没见过帧、开流计数起点未知也要能布防；
/// 各相机以本件首帧定基线（首帧算第 1 帧），收尾后下一件的基线接着本件见到的最高帧计数。
#[test]
fn hik_camera_without_frames_this_session_arms_and_counts_from_its_first_frame() {
    let mut p = armed_part(
        "hik-first",
        &Ledgers::default(),
        None,
        Policy::burst_production(None),
    );
    let sdk = |cam: u8, counter: u64| Frame {
        counter: CounterSource::SdkFrame,
        ..burst_frame(cam, counter)
    };
    for (cam, first) in [(0u8, 57u64), (1, 3), (2, 1)] {
        for seq in 1..=2 {
            assert_eq!(
                p.receive_burst(&sdk(cam, first + seq - 1)),
                Some(burst::Arrival::Accepted {
                    camera: cam as usize,
                    seq
                }),
                "相机 {cam}"
            );
        }
    }
    let mut ledgers = Ledgers::default();
    p.close_routing(&mut ledgers);
    let mut next = armed_part("hik-next", &ledgers, None, Policy::burst_production(None));
    assert!(
        matches!(
            next.receive_burst(&sdk(0, 58)),
            Some(burst::Arrival::Stale { .. })
        ),
        "上一件的迟到帧"
    );
    assert_eq!(
        next.receive_burst(&sdk(0, 59)),
        Some(burst::Arrival::Accepted { camera: 0, seq: 1 })
    );
}

/// 会话变化仍是设备故障：布防后相机重连过，之后的帧号对不上本件，整件判 ERR 98——重连后又来帧，或重连后没再来帧（收尾时核对会话）。
#[test]
fn camera_reconnect_during_part_is_err_98() {
    for case in ["重连后又来一帧", "重连后没再来帧"] {
        let mut p = burst_part(case, &Ledgers::default());
        for counter in 1..=4 {
            for cam in 0..3 { deliver(&mut p, cam, counter); }
        }
        if case == "重连后又来一帧" {
            assert!(matches!(p.receive_burst(&Frame { session: 99, ..burst_frame(1, 5) }), Some(burst::Arrival::Fault(_))));
        } else {
            let cameras: Vec<ArmCam> = (0..3u8).map(|cam| ArmCam { cam, camera: p.camera_ids[cam as usize].clone(),
                session: if cam == 1 { 99 } else { cam as u64 + 11 }, source: Some(CounterSource::Synthetic), counter_after_open: None }).collect();
            let reason = p.burst.check_sessions(&cameras).unwrap_err();
            p.fault = Some((fault::DEVICE_LOST, reason));
        }
        end(&mut p, Some(4), false);
        let judgement = judge_burst(&mut p);
        assert_eq!((judgement.verdict, judgement.fault_code), (Verdict::ErrInspect, fault::DEVICE_LOST), "{case}");
    }
}

/// 决策（默认做法 7）：三台相机整件一帧都没有时判 ERR 91；PLC 照常报了触发数时等到收尾截止再判。
#[test]
fn no_frame_from_any_camera_is_err_91() {
    let mut p = burst_part("burst-empty", &Ledgers::default());
    end(&mut p, Some(4), false);
    assert!(!p.complete(), "PLC 报 4、一帧没到：等到收尾截止");
    end(&mut p, Some(4), true);
    assert!(p.complete());
    let judgement = judge_burst(&mut p);
    assert_eq!((judgement.verdict, judgement.plc_code, judgement.fault_code), (Verdict::ErrInspect, 90, fault::MISSING_FRAME));
}

/// D-SB-13（不裁帧时的背压）：一帧没测成——测量队满、单帧超时、测量失败、超过处理时限仍没结果——只记该组与整件检测不完整，
/// 其余帧照常判，不判 ERR 99。
#[test]
fn unmeasured_frame_marks_group_incomplete_not_err_99() {
    use measure::MeasureError;
    for (case, error) in [
        ("测量队满", Some(MeasureError::QueueFull)),
        ("单帧超时", Some(MeasureError::Timeout)),
        ("测量失败", Some(MeasureError::Failed("lyFlow 运行失败".into()))),
        ("超过处理时限仍没结果", None),
    ] {
        let mut p = burst_part(case, &Ledgers::default());
        for counter in 1..=4 {
            for cam in 0..3u8 {
                if (cam, counter) != (1, 2) {
                    deliver(&mut p, cam, counter);
                    continue;
                }
                p.receive_burst(&burst_frame(cam, counter)).unwrap();
                if let Some(error) = error.clone() {
                    let result = frame_result(&p, 1, 2, Err(error));
                    p.apply_burst(result).unwrap();
                }
            }
        }
        end(&mut p, Some(4), true);
        p.expire_burst(Instant::now() + p.proc_timeout, p.proc_timeout);
        if error.is_none() {
            let late = frame_result(
                &p,
                1,
                2,
                Ok(measure::simulate_frame(
                    Scenario::Normal,
                    1,
                    2,
                    1,
                    &p.recipe,
                )),
            );
            assert!(p.apply_burst(late).is_none());
        }
        assert!(p.complete(), "{case}");
        let judgement = judge_burst(&mut p);
        assert_eq!((judgement.verdict, judgement.fault_code), (Verdict::Ok, 0), "{case}");
        assert!(judgement.flags.incomplete, "{case}");
        assert_eq!(judgement.regions[0].incomplete_groups, vec![2], "{case}");
    }
}

#[test]
fn tail_frame_received_before_drain_deadline_finishes_measurement_before_result() {
    let mut p = burst_part("tail-measurement", &Ledgers::default());
    deliver_groups(&mut p, 4);
    end(&mut p, Some(0), false);
    assert_eq!(p.receive_burst(&burst_frame(0, 5)), Some(burst::Arrival::Accepted { camera: 0, seq: 5 }));
    end(&mut p, Some(0), true);
    assert!(p.expire_burst(Instant::now(), p.proc_timeout).is_empty());
    assert!(!p.complete());
    assert!(p.receive_burst(&burst_frame(1, 5)).is_none());
    let result = frame_result(
        &p,
        0,
        5,
        Ok(measure::simulate_frame(
            Scenario::Normal,
            0,
            5,
            1,
            &p.recipe,
        )),
    );
    assert!(p.apply_burst(result).is_some());
    assert!(p.complete());
    let judgement = judge_burst(&mut p);
    let record = p.burst_record(&burst::summary(&judgement));
    assert_eq!(record.frames.iter().find(|frame| frame.camera == 0 && frame.seq == 5).unwrap().status, FrameState::Measured);
}

#[test]
fn same_sn_reinspection_rejects_measurement_from_previous_cycle() {
    let mut previous = burst_part("previous", &Ledgers::default());
    previous.receive_burst(&burst_frame(0, 1)).unwrap();
    let old = frame_result(
        &previous,
        0,
        1,
        Ok(measure::simulate_frame(
            Scenario::Normal,
            0,
            1,
            1,
            &previous.recipe,
        )),
    );
    let mut current = burst_part("current", &Ledgers::default());
    assert_eq!(current.sn, previous.sn);
    current.receive_burst(&burst_frame(0, 1)).unwrap();
    end(&mut current, Some(0), true);
    assert!(current.apply_burst(old).is_none());
    assert!(!current.complete());
    let valid = frame_result(
        &current,
        0,
        1,
        Ok(measure::simulate_frame(
            Scenario::Normal,
            0,
            1,
            1,
            &current.recipe,
        )),
    );
    assert!(current.apply_burst(valid).is_some());
    assert!(current.complete());
}

/// 维护采集件只录图不送测：收下的帧不进测量等待，运动结束后收尾窗口一过就能收尾，不会卡在“测量中”。
#[test]
fn collect_part_completes_at_drain_deadline_without_measuring() {
    let mut part = burst_part("maintenance-collect", &Ledgers::default());
    part.mode = crate::store::PartMode::MaintenanceCollect;
    part.burst.collect = true;
    for cam in 0..3 {
        assert!(matches!(
            part.receive_burst(&burst_frame(cam, 1)),
            Some(burst::Arrival::Accepted { seq: 1, .. })
        ));
    }
    end(&mut part, None, false);
    assert!(!part.complete(), "收尾窗口内仍收帧");
    end(&mut part, None, true);
    assert!(part.complete(), "采集件不等测量结果");
    assert_eq!(part.burst.group_count(), 1);
}

/// 维护模式下采集版只核对相机，没示教走廊、没标定也能开工；正常模式不收采集版；正式版不论模式都要走廊与标定。
/// 防就绪检查与空闲节拍把维护采集版按正式版要求挡在“尚无可开工配方”。
#[test]
fn run_kind_collect_runs_only_in_maintenance_and_formal_always_needs_corridors() {
    use crate::recipe::RecipeStage::{Collect, Formal};
    let cameras: Vec<String> = (0..3).map(crate::recipe::legacy_camera_id).collect();
    let recipe = |stage, taught: bool| {
        let mut doc = if taught {
            crate::recipe::samples_burst()
        } else {
            crate::recipe::burst_template("NEW-1".into(), "新产线".into(), 21, &cameras)
        };
        doc.stage = stage;
        doc.build().unwrap()
    };
    let cases = [
        (
            "维护 + 未示教采集版 → 只录图",
            recipe(Collect, false),
            true,
            Some(RunKind::Collect),
        ),
        ("正常 + 采集版 → 拒收", recipe(Collect, false), false, None),
        (
            "维护 + 未示教正式版 → 仍要走廊",
            recipe(Formal, false),
            true,
            None,
        ),
        (
            "正常 + 已示教正式版 → 生产",
            recipe(Formal, true),
            false,
            Some(RunKind::Production),
        ),
    ];
    for (name, recipe, maintenance, expected) in cases {
        assert_eq!(run_kind(&recipe, maintenance).ok(), expected, "{name}");
    }
}

/// 维护模式没有可开工配方时配方项只提示、不阻塞：来件照常布防放行（MM-3），PC 不撤就绪；正常模式仍阻塞。
#[test]
fn no_runnable_recipe_blocks_only_outside_maintenance() {
    use readiness::{Check, Facts, ItemKey, ItemState, Product, RecipeOption, Site};
    for (maintenance, ready, state) in [
        (false, false, ItemState::Blocked),
        (true, true, ItemState::Waiting),
    ] {
        let report = readiness::evaluate(Facts {
            candidate_debug: None,
            site: Site::Real,
            plc_connected: true,
            session_behind: false,
            session: Some(crate::plc_session::SessionPhase::Idle),
            session_message: None,
            phase: Phase::Idle,
            fault: None,
            cameras: Check::Ok,
            engine: Some(Ok("1.1.5".into())),
            workers: Ok(()),
            audit: Check::Ok,
            record: crate::settings::RecordMode::Off,
            product: Product::Plc,
            activation_uncertain: false,
            maintenance,
            recipes: vec![RecipeOption {
                id: "NEW-1".into(),
                name: "新产线".into(),
                version: 1,
                product_code: 21,
                published: true,
                runnable: false,
                reason: Some("相机 cam1 尚未示教检测走廊".into()),
                target: None,
                waiting: None,
            }],
        });
        let recipe = report
            .items
            .iter()
            .find(|i| i.key == ItemKey::Recipe)
            .unwrap();
        assert_eq!(
            (report.ready, recipe.state),
            (ready, state),
            "维护模式 {maintenance}"
        );
    }
}
