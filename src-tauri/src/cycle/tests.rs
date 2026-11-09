use super::*;
use crate::frame::CounterSource;
use crate::offline::Scenario;

/// 段配方在途件：samples_burst（cam1–cam3 共用触发、一个直线段 N_exp = 4），相机组序号 0–2、会话 11–13，
/// 按计数账定帧计数基线（模拟相机本会话没见过帧时从 0 起）。
fn burst_part(cycle_id: &str, ledgers: &Ledgers) -> Part {
    armed_part(
        cycle_id,
        ledgers,
        Some(CounterSource::Synthetic),
        Policy::burst_offline(None),
    )
}

/// 同 [`burst_part`]，指定接件前已知的计数来源（海康为空）与接件策略。
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
    // 合成时钟：第 n 帧的时间戳 = n × 合成帧周期，下限 = 帧计数基线对应的时刻（与原来的帧号配组同一口径）
    let starts: Vec<crate::frame_clock::ClockStart> = baselines
        .iter()
        .map(|b| crate::frame_clock::ClockStart {
            period_ns: crate::frame_clock::SYNTH_PERIOD_NS as f64,
            floor: b.baseline.unwrap_or(0) * crate::frame_clock::SYNTH_PERIOD_NS,
            anchored: true,
            delay_ns: 0,
            first_gate: 0,
        })
        .collect();
    Part {
        mode: crate::store::PartMode::Production,
        cycle_id: cycle_id.into(),
        bundle_id: None,
        production: None,
        sn: "SN-88".into(),
        burst: Box::new(burst::BurstPart::arm(
            &recipe,
            baselines,
            policy.trusted,
            &starts,
        )),
        device_triggers: None,
        routing_closed: false,
        proc_timeout: Duration::from_secs(1),
        drain_timeout: Duration::from_secs(2),
        gate: None,
        gate_guard: None,
        admitted_at: Instant::now(),
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
    Frame {
        cam,
        session: cam as u64 + 11,
        counter: CounterSource::Synthetic,
        frame_counter: counter,
        trigger_counter: counter,
        lost_packets: 0,
        ts: 1,
        dev_ts: Some(counter * crate::frame_clock::SYNTH_PERIOD_NS),
        manual: false,
        images: Vec::new(),
        bayer: None,
    }
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
        trace: None,
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

/// 离线产线跑完一件后的段配方判定：交付的帧都已测完，PLC 报的 device_triggers 多于各相机帧数时不再等剩余帧，
/// 按收尾截止已过判（与生产节拍等到 drain 截止后的结论相同）。

/// 三台相机按帧计数 1..=groups 依次到齐，每帧都测完（帧计数基线为 0）。
fn deliver_groups(part: &mut Part, groups: u64) {
    for counter in 1..=groups {
        for cam in 0..3u8 { deliver(part, cam, counter); }
    }
}

/// D-SB-45：帧到达时按累计 N_exp 预测所属段，只由预测段的检测相机用该段走廊送测，其余相机记“本段不检测”、不送测；
/// 收尾后按比例边界解析区域，对判定需要、但到达时没用对应走廊送测过的（帧, 相机）补测，补测完才收尾；
/// 要测图却没录到原图的记没测（检测不完整，D-SB-60）。直线 5 帧（三台相机）+ 弯道 5 帧（只由第一台相机），本件 12 帧：比例边界在 6、
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
    // 移到另一段或进了过渡区的帧仍用到达时的走廊判定；不等补测完就判定；或没录到原图时把没测的帧当测成；
    // 两段都开逐帧定向时（D-SB-47，测量不依赖段归属）仍把用相邻段定向图测过的帧拿去补测；
    // 两段的搜索扇区拖成不同方向（PR #218 评审）或测量区间按段设得不同时（测量图不同）却当成同一测量图不补测
    for (case, keep_images, heading, differ) in [
        ("补测有原图", true, false, None),
        ("要测图却没录到原图", false, false, None),
        ("两段都开逐帧定向", true, true, None),
        ("两段都开逐帧定向、扇区方向不同", true, true, Some("sector")),
        ("两段都开逐帧定向、测量区间不同", true, true, Some("window")),
    ] {
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
            sector_deg: (differ == Some("sector")).then_some(90.0),
            color_model: None,
            measure: None,
        });
        doc.segments.iter_mut().for_each(|s| {
            let length_mm = if differ == Some("window") && s.index == 2 {
                20.0
            } else {
                10.0
            };
            s.heading = heading.then(|| crate::plan::HeadingSettings {
                tolerance_deg: 5.0,
                window: Some(crate::recipe::HeadingWindow {
                    start_mm: 1.0,
                    length_mm,
                }),
            })
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
        let recording_dir = crate::offline::test_dir::TestDir::new("cycle-post", case);
        if keep_images {
            let recorder = crate::recorder::Recorder::new(vec![recording_dir.to_path_buf()], None);
            let mut recording = recorder
                .begin(
                    crate::settings::RecordMode::All,
                    Some(&recording_dir.to_path_buf()),
                    &p.sn,
                    p.recipe.clone(),
                    &p.cycle_id,
                    None,
                )
                .unwrap();
            for seq in 1..=12 {
                for camera in 0..3 {
                    let mut frame = burst_frame(camera as u8, seq);
                    frame.images = vec![Arc::new(crate::frame::FrameImage::new(1, 1, vec![128]))];
                    recorder.burst_frame(&mut recording, &frame, camera, seq);
                }
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while (1..=12).any(|seq| {
                (0..3).any(|camera| {
                    !matches!(
                        recording.burst_frame_file(camera, seq),
                        crate::recorder::RecordedFrame::Ready(_)
                    )
                })
            }) {
                assert!(Instant::now() < deadline, "{case}：测试原图未落盘");
                std::thread::sleep(Duration::from_millis(5));
            }
            p.recording = Some(recording);
        }
        end(&mut p, Some(12), true);
        assert!(!p.complete(), "{case}：还有要补测的帧，不能收尾");
        let mut jobs: Vec<(usize, u64, u32)> = p
            .start_post_measure(Instant::now())
            .into_iter()
            .map(|(c, s, g, _)| (c, s, g))
            .collect();
        jobs.sort();
        if keep_images {
            let expected: &[(usize, u64, u32)] = if heading && differ.is_none() {
                &[(1, 6, 1), (1, 7, 1), (2, 6, 1), (2, 7, 1)]
            } else {
                &[(0, 6, 1), (1, 6, 1), (1, 7, 1), (2, 6, 1), (2, 7, 1)]
            };
            assert_eq!(jobs, expected, "{case}");
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

/// D-SB-55：帧按相机时间戳编号。检测中相机重开（会话变了、帧号从 1 重数，时间戳连续）照常收帧、不判 ERR，收尾核对会话也不报错；
/// 时间戳回退（断电或时钟被复位）的相机之后的帧不收，缺的帧组只算检测不完整，整件不判 ERR。
#[test]
fn camera_reopen_during_part_keeps_numbering_by_timestamp() {
    let mut p = burst_part("reopen", &Ledgers::default());
    for counter in 1..=4 {
        for cam in 0..3 {
            deliver(&mut p, cam, counter);
        }
    }
    let reopened = Frame {
        session: 99,
        frame_counter: 1,
        ..burst_frame(1, 5)
    };
    assert_eq!(
        p.receive_burst(&reopened),
        Some(burst::Arrival::Accepted { camera: 1, seq: 5 }),
        "重开后时间戳连续：第 5 帧"
    );
    let cameras: Vec<ArmCam> = (0..3u8)
        .map(|cam| ArmCam {
            cam,
            camera: p.camera_ids[cam as usize].clone(),
            session: if cam == 1 { 99 } else { cam as u64 + 11 },
            source: Some(CounterSource::Synthetic),
            counter_after_open: None,
        })
        .collect();
    assert!(
        p.burst.check_sessions(&cameras).is_ok(),
        "会话变了不再是故障"
    );
    let reset = Frame {
        dev_ts: Some(crate::frame_clock::SYNTH_PERIOD_NS),
        ..burst_frame(2, 5)
    };
    assert_eq!(
        p.receive_burst(&reset),
        Some(burst::Arrival::ClockBack { camera: 2 })
    );
    assert_eq!(
        p.receive_burst(&burst_frame(2, 5)),
        Some(burst::Arrival::ClockBack { camera: 2 }),
        "回退后这台相机都不收"
    );
    deliver(&mut p, 0, 5);
    let result = frame_result(
        &p,
        1,
        5,
        Ok(measure::simulate_frame(
            Scenario::Normal,
            1,
            5,
            1,
            &p.recipe,
        )),
    );
    p.apply_burst(result).unwrap();
    end(&mut p, Some(5), false);
    let judgement = judge_burst(&mut p);
    assert_ne!(
        judgement.verdict,
        Verdict::ErrInspect,
        "缺第 5 帧组的相机只算检测不完整"
    );
    assert!(p.fault.is_none());
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

/// 防的回归（D-SB-54）：PLC 报结束后不管门控已关闭都固定等满收尾窗口；或门控还开着（还有帧要来）就提前收尾、丢掉后面的帧。
#[test]
fn closed_gate_ends_the_drain_after_the_guard_but_an_open_gate_waits_for_the_window() {
    let gate = Arc::new(crate::gate::Gate::default());
    let mut p = burst_part("gate-drain", &Ledgers::default());
    p.gate = Some(gate.clone());
    p.gate_guard = Some(Duration::from_millis(80));
    deliver_groups(&mut p, 4);
    end(&mut p, Some(0), false);
    let now = Instant::now();
    for cam in 0..3 {
        gate.observe(cam, true, now);
    }
    assert!(!p.complete(), "门控还开着：等收尾窗口");
    for cam in 0..3 {
        gate.observe(cam, false, now);
    }
    assert!(!p.complete(), "刚看到关闭：最后一帧可能还在路上，等满余量");
    let closed = Instant::now() - Duration::from_millis(100);
    for cam in 0..3 {
        gate.observe(cam, true, closed - Duration::from_millis(1));
        gate.observe(cam, false, closed);
        gate.observe(cam, false, Instant::now());
    }
    assert!(p.complete(), "关闭已满余量：不再等收尾窗口");
    assert!(
        p.receive_burst(&burst_frame(0, 5)).is_none(),
        "截止后到的帧不收"
    );
}

/// 防的回归（D-SB-55）：门控关闭时各相机最后一帧都已到（下一次曝光本该在关闭之后），还要白等余量；
/// 或有相机还差最后一帧（在途）时就截止。
#[test]
fn drain_ends_at_gate_close_once_every_camera_has_its_last_frame() {
    let t = crate::frame_clock::SYNTH_PERIOD_NS;
    let gate = Arc::new(crate::gate::Gate::default());
    let mut p = burst_part("last-frame", &Ledgers::default());
    p.gate = Some(gate.clone());
    p.gate_guard = Some(Duration::from_secs(1));
    for counter in 1..=4 {
        for cam in 0..2 {
            deliver(&mut p, cam, counter);
        }
    }
    deliver(&mut p, 2, 1);
    deliver(&mut p, 2, 2);
    deliver(&mut p, 2, 3);
    end(&mut p, Some(0), false);
    let now = Instant::now();
    for cam in 0..3 {
        gate.edge(cam, false, 4 * t + t / 2, now);
        gate.observe(cam, true, now);
        gate.observe(cam, false, now);
    }
    assert!(!p.complete(), "相机 2 的第 4 帧（关闭前曝光）还没到：等");
    deliver(&mut p, 2, 4);
    assert!(p.complete(), "三台都到了关闭前的最后一帧：不等余量");
    // PR #215 评审：又开了一次很短的门控（4.6T 开、4.8T 关），新门控的帧还在路上，前一门控的末帧不能当最后一帧
    for cam in 0..3 {
        gate.edge(cam, true, 4 * t + 6 * t / 10, now);
        gate.edge(cam, false, 4 * t + 8 * t / 10, now);
    }
    assert!(!p.complete(), "最近一次门控的帧还没到：等");
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

/// 维护模式下采集版只核对相机，没示教走廊、没标定也能开工；正常模式不收采集版；正式版不论模式都要走廊与标定。
/// 防就绪检查与空闲节拍把维护采集版按正式版要求挡在“尚无可开工配方”。
#[test]
fn run_kind_collect_runs_only_in_maintenance_and_formal_always_needs_corridors() {
    use crate::recipe::RecipeStage::{Collect, Formal};
    let cameras: Vec<String> = (1..=3).map(|k| format!("cam{k}")).collect();
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

/// D-SB-62：生产按门控（帧时间戳停顿）分笔，笔结构记进判定，每笔开头按配方跳过、列为未检区域。
#[test]
fn production_splits_passes_by_gate_pause_and_skips_each_pass_head() {
    // 防的回归：门控停顿后的帧被当成同一笔（笔开头没跳过，起涂短胶误判断胶），或笔结构没进记录、重判与复测拿不到
    let mut recipe = crate::recipe::samples_burst().build().unwrap();
    recipe.pass_skip.head = 1;
    let mut p = armed_part_with(
        recipe,
        "passes",
        &Ledgers::default(),
        Some(CounterSource::Synthetic),
        Policy::burst_offline(None),
    );
    let period = crate::frame_clock::SYNTH_PERIOD_NS;
    for counter in 1..=6u64 {
        let pause = if counter >= 4 { period * 21 / 2 } else { 0 };
        for cam in 0..3u8 {
            let frame = Frame {
                dev_ts: Some(counter * period + pause),
                ..burst_frame(cam, counter)
            };
            if let Some(burst::Arrival::Accepted { camera, seq }) = p.receive_burst(&frame) {
                let simulated =
                    measure::simulate_frame(Scenario::Normal, camera, seq, 1, &p.recipe);
                let result = frame_result(&p, camera, seq, Ok(simulated));
                p.apply_burst(result).unwrap();
            }
        }
    }
    end(&mut p, Some(6), true);
    let judgement = judge_burst(&mut p);
    let skipped: Vec<(Option<u32>, Option<u64>)> = judgement
        .uninspected
        .iter()
        .map(|u| (u.pass, u.from_seq))
        .collect();
    assert_eq!(
        (judgement.passes.clone(), skipped),
        (vec![3, 3], vec![(Some(1), Some(1)), (Some(2), Some(4))])
    );
}
