use super::*;
use crate::frame::CounterSource;

/// 界面事件流 cycle://log 的行带结构化 cycleId：当前件在途时，上一件迟到的录制结果仍标上一件，
/// 未指明工件的记录标当前件；与工件无关的记录不带该字段。防的是迟到结果混进下一件的事件里。
#[test]
fn event_stream_line_carries_the_cycle_its_record_belongs_to() {
    let active = applog::Fields { cycle_id: Some("current".into()), ..applog::Fields::default() };
    let line = |explicit: Option<&str>, active: Option<&applog::Fields>| {
        let fields = applog::correlate(active, applog::Fields { cycle_id: explicit.map(Into::into), ..applog::Fields::default() });
        let record = applog::Record { ts: 1, level: applog::Level::Info, ev: "原图录制完成".into(), msg: String::new(), fields };
        serde_json::to_value(LogLine::from_record(record, "info")).unwrap()
    };
    for (case, explicit, active, expected) in [
        ("上一件迟到的结果", Some("late"), Some(&active), Some("late")),
        ("未指明工件的当前件记录", None, Some(&active), Some("current")),
        ("没有在途工件", None, None, None),
    ] {
        assert_eq!(line(explicit, active).get("cycleId").and_then(|v| v.as_str()), expected, "{case}");
    }
}

#[test]
fn interrupted_history_keeps_all_planned_shots_and_each_failure_reason() {
    let mut part = part("cycle-audit", &Ledgers::default());
    part.frames[0].status = FrameStatus::Done;
    part.frames[0].session = Some(18);
    part.frames[0].frame_counter = Some(81);
    part.frames[0].score = Some(0.92);
    part.frames[2].status = FrameStatus::Measuring;
    part.frames[3].status = FrameStatus::Error;
    part.frames[3].error = Some("lyFlow 运行失败".into());
    let shots = recorded_shots(Some(&part.recipe), &part.frames, "连接中断");
    assert_eq!(shots.len(), 4);
    assert_eq!(shots.iter().map(|shot| shot.view).collect::<Vec<_>>(), [1, 2, 3, 1]);
    assert_eq!(shots[0].status, FrameStatus::Done);
    assert_eq!(shots[0].session, Some(18));
    assert_eq!(shots[0].frame_counter, Some(81));
    assert_eq!(shots[1].status, FrameStatus::Missing);
    assert!(shots[1].error.as_ref().unwrap().contains("连接中断"));
    assert_eq!(shots[2].status, FrameStatus::Error);
    assert!(shots[2].error.as_ref().unwrap().contains("测量未完成"));
    assert_eq!(shots[3].status, FrameStatus::Error);
    assert_eq!(shots[3].error.as_deref(), Some("lyFlow 运行失败"));
    assert_eq!(part.frames[1].status, FrameStatus::Waiting);
    let refused = recorded_shots(Some(&part.recipe), &[], "未布防");
    assert_eq!(refused.len(), 4);
    assert!(refused.iter().all(|shot| shot.status == FrameStatus::Missing));
}

fn part(cycle_id: &str, ledgers: &Ledgers) -> Part {
    let mut doc = crate::recipe::samples().remove(1);
    for (k, shot) in doc.shots.iter_mut().enumerate() {
        shot.camera = "cam1".into();
        shot.view = [1, 2, 3, 1][k];
    }
    let recipe = Arc::new(doc.build().unwrap());
    let cameras = recipe.cameras();
    let identities: Vec<_> = cameras.iter().enumerate().map(|(cam, id)| ArmCam {
        cam: cam as u8, camera: id.clone(), session: cam as u64 + 1,
        source: Some(CounterSource::Synthetic), counter_after_open: None,
    }).collect();
    let router = ShotRouter::arm(&Plan::from_shots(&recipe.shots), ledgers, &Policy::offline(None), &identities).unwrap();
    Part {
        cycle_id: cycle_id.into(), bundle_id: Some("release-1".into()), production: None, candidate: None, sn: "SN-88".into(),
        scenario: crate::offline::Scenario::Normal, frames: recipe.shots.iter().map(|shot| FrameView {
            shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view, ..FrameView::waiting()
        }).collect(), measuring_since: vec![None; recipe.shot_count()],
        table: vec![PointState::Pending; recipe.point_count()], received: 0, extra: 0, queue: 0,
        router, burst: None, issued: None, device_triggers: None, routing_closed: false, image_measurement: false, proc_timeout: Duration::from_secs(1), drain_timeout: Duration::from_secs(2),
        armed_at: Instant::now(), end_at: None, fault: None, cams: (0..cameras.len() as u8).collect(),
        camera_ids: cameras, rig_gen: 1, source: RecordSource::Real, recording: None, recipe,
    }
}

fn frame(cam: u8, counter: u64) -> Frame {
    Frame { cam, session: cam as u64 + 1, counter: CounterSource::Synthetic,
        frame_counter: counter + 9000, trigger_counter: counter, lost_packets: 0, ts: 1,
        manual: false, images: Vec::new() }
}

fn measured(part: &Part, k: usize) -> Measured {
    let shot = &part.recipe.shots[k];
    let indices: Vec<_> = part.recipe.owned_points(k).map(|j| j as u32).collect();
    Measured { views: Vec::new(), poses: Vec::new(), cycle_id: part.cycle_id.clone(), shot_id: shot.id.clone(),
        camera: shot.camera.clone(), bundle_id: part.bundle_id.clone(), sn: part.sn.clone(),
        k, cam: part.frames[k].cam, reference_failed: Vec::new(), score: 0.95, ms: 15,
        queue_ms: None, engine_ms: None, core_ms: None, error: None,
        d: vec![0.0; indices.len()], w: vec![4.0; indices.len()], st: vec![measure::ST_OK; indices.len()],
        idx: indices, px: Vec::new() }
}

fn complete_four(part: &mut Part, width: f32) {
    for k in 0..4 {
        let counter = k as u64 + 1;
        assert_eq!(part.receive_frame(&frame(0, counter)).unwrap(), Route::Bound { shot: k, ordinal: counter });
        let mut result = measured(part, k);
        result.w.fill(width);
        assert!(part.apply_result(result).is_some());
    }
}

pub(crate) async fn judge_last_frame_during_end(
    end: impl std::future::Future<Output = SessionEvent>,
    last_frame: impl std::future::Future<Output = Frame>,
) -> Judgement {
    let mut p = part("slow-end-journal", &Ledgers::default());
    p.drain_timeout = Duration::from_millis(200);
    for k in 0..3 {
        p.receive_frame(&frame(0, k as u64 + 1)).unwrap();
        p.apply_result(measured(&p, k)).unwrap();
    }
    let (event, frame) = tokio::join!(end, last_frame);
    let SessionEvent::End(ended_at, _) = event else { panic!("expected partEnd, got {event:?}") };
    p.end_at = Some(ended_at);
    if let Some(Route::Bound { shot, .. }) = p.receive_frame(&frame) {
        p.apply_result(measured(&p, shot));
    }
    p.judgement()
}

#[test]
fn tricam_four_shots_follow_one_counter_and_selected_views() {
    let mut p = part("tricam", &Ledgers::default());
    for k in 0..4 {
        assert_eq!(p.receive_frame(&frame(0, k as u64 + 1)).unwrap(), Route::Bound { shot: k, ordinal: k as u64 + 1 });
        assert!(p.apply_result(measured(&p, k)).is_some());
    }
    assert_eq!(p.frames.iter().map(|f| f.view).collect::<Vec<_>>(), [1, 2, 3, 1]);
    assert_eq!(p.judgement().verdict, Verdict::Ok);
    assert_eq!(p.triggers(), 4);
    let mut ng = part("tricam-ng", &Ledgers::default());
    complete_four(&mut ng, 9.0);
    assert!(!matches!(ng.judgement().verdict, Verdict::Ok | Verdict::OkWithExcursion | Verdict::ErrInspect));
}

#[test]
fn early_part_end_waits_for_every_shot_result() {
    let mut p = part("end-first", &Ledgers::default());
    let end = Instant::now();
    p.end_at = Some(end);
    assert!(p.expire(end + Duration::from_millis(10), Duration::from_secs(1), Duration::from_secs(3)).is_empty());
    assert!(!p.complete());
    complete_four(&mut p, 4.0);
    assert!(p.complete());
    assert_eq!(p.judgement().verdict, Verdict::Ok);
}

#[test]
fn duplicates_do_not_replace_missing_shots_or_satisfy_completion() {
    let mut p = part("duplicate", &Ledgers::default());
    for counter in 1..=3 {
        p.receive_frame(&frame(0, counter)).unwrap();
        p.apply_result(measured(&p, counter as usize - 1));
    }
    assert!(matches!(p.receive_frame(&frame(0, 3)).unwrap(), Route::Duplicate { shot: 2, .. }));
    assert_eq!(p.received, 3);
    assert!(!p.complete());
    let end = Instant::now();
    p.end_at = Some(end);
    p.expire(end + Duration::from_secs(3), Duration::from_secs(1), Duration::from_secs(2));
    assert!(p.complete());
    assert_eq!(p.frames[3].status, FrameStatus::Missing);
    assert_eq!(p.judgement().verdict, Verdict::ErrInspect);
    assert_eq!(p.judgement().fault_code, fault::EXTRA_FRAME);
}

#[test]
fn first_missing_frame_keeps_later_shots_and_expires_err91() {
    let mut p = part("missing-first", &Ledgers::default());
    for counter in 2..=4 {
        assert!(matches!(p.receive_frame(&frame(0, counter)).unwrap(), Route::Bound { .. }));
        p.apply_result(measured(&p, counter as usize - 1));
    }
    let end = Instant::now();
    p.end_at = Some(end);
    p.expire(end + Duration::from_secs(3), Duration::from_secs(1), Duration::from_secs(2));
    assert_eq!(p.frames[0].status, FrameStatus::Missing);
    assert_eq!(p.frames[1].shot_id, "P2");
    assert_eq!(p.judgement().fault_code, fault::MISSING_FRAME);
}

#[test]
fn counter_desync_ends_err91_with_expected_and_received_counters() {
    // 上一件按触发数推的下限比相机实际计数高一格（相机漏收过一个脉冲）：本件第一帧被当成迟到帧，最后一个拍照点缺帧
    let mut ledgers = Ledgers::default();
    ledgers.observe(&FrameMeta::from(&frame(0, 1)));
    ledgers.set_floor(&Floor { cam: 0, camera: "cam1".into(), session: 1, baseline: 2 });
    let mut p = part("desync", &ledgers);
    assert!(matches!(p.receive_frame(&frame(0, 2)).unwrap(), Route::Stale { counter: 2, baseline: Some(2) }));
    for counter in 3..=5 {
        let Route::Bound { shot, .. } = p.receive_frame(&frame(0, counter)).unwrap() else { panic!("counter {counter}") };
        p.apply_result(measured(&p, shot)).unwrap();
    }
    let end = Instant::now();
    p.end_at = Some(end);
    p.expire(end + Duration::from_secs(3), Duration::from_secs(1), Duration::from_secs(2));
    let judgement = p.judgement();
    assert_eq!(judgement.verdict, Verdict::ErrInspect);
    assert_eq!(judgement.fault_code, fault::MISSING_FRAME);
    // 应收 3–6（基线 2 之后 4 帧），实收 3–5 绑到 P1–P3，P4 缺
    assert_eq!(p.router.baseline(0), Some(2));
    assert_eq!((0..4).map(|k| p.router.bound(k)).collect::<Vec<_>>(), [Some(3), Some(4), Some(5), None]);
    assert_eq!(p.router.missing(), [3]);

    // 多出的触发：应收 1–4，第 5 帧超出计划 1 个
    let mut p = part("extra", &Ledgers::default());
    let routes: Vec<_> = (1..=5).map(|counter| p.receive_frame(&frame(0, counter)).unwrap()).collect();
    assert_eq!(routes[4], Route::Extra { ordinal: 5, planned: 4 });
    let judgement = p.judgement();
    assert_eq!(judgement.verdict, Verdict::ErrInspect);
    assert_eq!(judgement.fault_code, fault::EXTRA_FRAME);
}

#[test]
fn timeout_is_terminal_before_end_and_late_measurement_is_ignored() {
    let mut p = part("timeout", &Ledgers::default());
    p.receive_frame(&frame(0, 1)).unwrap();
    let result = measured(&p, 0);
    let now = p.measuring_since[0].unwrap() + Duration::from_secs(2);
    assert_eq!(p.expire(now, Duration::from_secs(1), Duration::from_secs(3)), [0]);
    assert_eq!(p.frames[0].status, FrameStatus::Error);
    assert_eq!(p.queue, 0);
    assert!(p.apply_result(result).is_none());
    assert_eq!(p.judgement().fault_code, fault::PROCESS_TIMEOUT);
}

#[test]
fn same_sn_reinspection_rejects_old_cycle_and_every_other_identity_mismatch() {
    let mut first = part("first", &Ledgers::default());
    first.receive_frame(&frame(0, 1)).unwrap();
    let old = measured(&first, 0);
    let mut ledgers = Ledgers::default();
    first.router.close(&mut ledgers, None);
    let mut next = part("next", &ledgers);
    assert!(matches!(next.receive_frame(&frame(0, 2)).unwrap(), Route::Stale { .. }));
    assert_eq!(next.receive_frame(&frame(0, 5)).unwrap(), Route::Bound { shot: 0, ordinal: 1 });
    assert_eq!(next.sn, old.sn);
    assert!(next.apply_result(old).is_none());
    for variant in 0..4 {
        let mut wrong = measured(&next, 0);
        match variant { 0 => wrong.shot_id = "P2".into(), 1 => wrong.camera = "cam2".into(),
            2 => wrong.bundle_id = Some("old-release".into()), _ => wrong.cam = 2 }
        assert!(next.apply_result(wrong).is_none());
    }
    assert_eq!(next.queue, 1);
    assert!(next.apply_result(measured(&next, 0)).is_some());
}

/// 协议 6 不再核对 partEnd 时 PLC 报的触发数等于计划：下一件基线按 PLC 报的实发数推。
/// 按计划数推（实发 3、计划 4）会把下一件第一帧当成上一件的迟到帧；只按见到的帧推（第 4 帧丢在传输里）会让下一件错位一帧。
#[test]
fn next_part_baseline_follows_plc_reported_triggers_not_plan_or_frames_seen() {
    for (case, reported, delivered, next_first) in [
        ("实发少于计划", 3, 3, 4),
        ("实发等于计划、末帧丢在传输里", 4, 3, 5),
    ] {
        let mut p = part(case, &Ledgers::default());
        p.issued = Some(vec!["cam1".into()]);
        for counter in 1..=delivered { p.receive_frame(&frame(0, counter)).unwrap(); }
        p.device_triggers = Some(reported);
        let mut ledgers = Ledgers::default();
        p.close_routing(&mut ledgers, None);
        let mut next = part("next", &ledgers);
        assert_eq!(next.receive_frame(&frame(0, next_first)), Some(Route::Bound { shot: 0, ordinal: 1 }), "{case}");
    }
}

/// 测点号不对、无基准站带着 d（违反“无基准站没有位置值”的契约）都按测量出错处理。
#[test]
fn malformed_measurement_cannot_write_another_shot_or_report_ok() {
    let corruptions: [fn(&mut Measured); 2] = [
        |m| m.idx[0] = u32::MAX,
        |m| (m.st[0], m.d[0]) = (crate::measure::ST_NO_REF, 1.5),
    ];
    for (case, corrupt) in corruptions.into_iter().enumerate() {
        let mut p = part("bad-output", &Ledgers::default());
        p.receive_frame(&frame(0, 1)).unwrap();
        let mut result = measured(&p, 0);
        corrupt(&mut result);
        let rejected = p.apply_result(result).unwrap();
        assert!(rejected.error.is_some(), "case {case}");
        assert!(rejected.idx.is_empty(), "case {case}");
        assert!(p.recipe.owned_points(0).all(|j| p.table[j] == PointState::Invalid), "case {case}");
        assert!(p.recipe.owned_points(1).all(|j| p.table[j] == PointState::Pending), "case {case}");
        assert_eq!(p.judgement().fault_code, fault::PROCESS_TIMEOUT, "case {case}");
    }
}

#[test]
fn late_frame_and_result_cannot_beat_the_deadline_tick() {
    let mut p = part("late-frame", &Ledgers::default());
    p.end_at = Some(Instant::now() - Duration::from_secs(3));
    assert!(p.receive_frame(&frame(0, 1)).is_none());
    assert_eq!(p.received, 0);
    assert_eq!(p.frames[0].status, FrameStatus::Missing);
    assert_eq!(p.judgement().fault_code, fault::MISSING_FRAME);

    let mut p = part("late-result", &Ledgers::default());
    p.receive_frame(&frame(0, 1)).unwrap();
    let result = measured(&p, 0);
    p.end_at = Some(Instant::now() - Duration::from_secs(3));
    assert!(p.apply_result(result).is_none());
    assert_eq!(p.frames[0].status, FrameStatus::Error);
    assert_eq!(p.judgement().fault_code, fault::PROCESS_TIMEOUT);
}

#[test]
fn device_reconnection_without_another_frame_is_detected() {
    let mut p = part("reconnect", &Ledgers::default());
    complete_four(&mut p, 4.0);
    let mut cameras: Vec<_> = p.camera_ids.iter().enumerate().map(|(cam, camera)| ArmCam {
        cam: cam as u8, camera: camera.clone(), session: cam as u64 + 1,
        source: Some(CounterSource::Synthetic), counter_after_open: None,
    }).collect();
    assert!(p.router.check_sessions(&cameras).is_ok());
    cameras[0].session += 100;
    let reason = p.router.check_sessions(&cameras).unwrap_err();
    p.fault = Some((fault::DEVICE_LOST, reason));
    assert_eq!(p.judgement().fault_code, fault::DEVICE_LOST);
}

pub(crate) fn judge_callback_channel(
    baseline: &Ledgers, received: Vec<Frame>, observed: &Ledgers,
) -> (Judgement, Vec<FrameView>) {
    let base = baseline.get(0).and_then(|ledger| ledger.last).unwrap_or(0);
    let mut p = part(&format!("callback-full-{base}"), &Ledgers::default());
    let camera = ArmCam { cam: 0, camera: "cam1".into(), session: 11,
        source: Some(CounterSource::ChunkTrigger), counter_after_open: Some(0) };
    p.router = ShotRouter::arm(&Plan::from_shots(&p.recipe.shots), baseline,
        &Policy::production(None), &[camera]).unwrap();
    for received in received {
        if let Some(Route::Bound { shot, ordinal }) = p.receive_frame(&received) {
            assert_eq!(shot as u64 + 1, ordinal);
            assert_eq!(received.trigger_counter, base + ordinal);
            p.apply_result(measured(&p, shot)).unwrap();
        }
    }
    if let Some(ledger) = observed.get(0) {
        if let Err(error) = p.router.check_observed(0, ledger) {
            p.fault.get_or_insert(error);
        }
    }
    let end = Instant::now();
    p.end_at = Some(end);
    p.expire(end + Duration::from_secs(3), Duration::from_secs(1), Duration::from_secs(2));
    (p.judgement(), p.frames)
}

/// 离线产线按触发顺序交付的帧（true 为传输中丢失），走生产节拍的帧路由与判定。
pub(crate) fn judge_delivered(lost: &[bool]) -> Judgement {
    let mut p = part("offline-line", &Ledgers::default());
    for (index, lost) in lost.iter().enumerate() {
        if *lost { continue; }
        if let Some(Route::Bound { shot, .. }) = p.receive_frame(&frame(0, index as u64 + 1)) {
            p.apply_result(measured(&p, shot)).unwrap();
        }
    }
    let end = Instant::now();
    p.end_at = Some(end);
    p.expire(end + Duration::from_secs(3), Duration::from_secs(1), Duration::from_secs(2));
    p.judgement()
}

/// 超预算拒绝本件由 arming 的用例覆盖；这里只守“预算超时不毒化追溯”。
#[tokio::test(start_paused = true)]
async fn arm_budget_overrun_in_audit_health_refuses_part_without_poisoning_audit() {
    let mut budget = ArmBudget::with_start(tokio::time::Instant::now(), Duration::from_millis(30));
    let poisoned = std::cell::Cell::new(false);
    let (release, blocked) = std::sync::mpsc::channel::<()>();
    let result = arm_health_check(&mut budget, move || { let _ = blocked.recv(); Ok(()) }, |_| poisoned.set(true)).await;
    drop(release);
    assert!(result.is_err(), "前提：探针卡住，布防预算用完");
    assert!(!poisoned.get(), "布防预算偶发超时不能把追溯判成需重启恢复的故障");
}

/// 离线定位失败工况经模拟测量进整件判定：定位失败那帧胶正常 → ERR 92；同一帧还断胶 → NG 优先判 NG_GAP。两种都保留帧的定位失败徽标。
#[test]
fn offline_locate_fail_scenarios_judge_err92_or_ng_gap_and_keep_the_reference_badge() {
    for (scenario, expected) in [
        (Scenario::LocateFail, (Verdict::ErrInspect, 90, fault::LOCATE_FAILED)),
        (Scenario::LocateFailGap, (Verdict::NgGap, 13, 0)),
    ] {
        let mut p = part(&format!("{scenario:?}"), &Ledgers::default());
        p.scenario = scenario;
        for k in 0..4 {
            p.receive_frame(&frame(0, k as u64 + 1)).unwrap();
            let shot = &p.recipe.shots[k];
            let job = Job { cycle_id: p.cycle_id.clone(), shot_id: shot.id.clone(), camera: shot.camera.clone(),
                bundle_id: p.bundle_id.clone(), production: None, candidate: None, submitted_at: Instant::now(),
                timeout: Duration::from_secs(1), image_measurement: false, sn: p.sn.clone(), k, cam: p.frames[k].cam,
                recipe: p.recipe.clone(), scenario, image: None, images: Vec::new() };
            assert!(p.apply_result(measure::simulate(&job)).is_some());
        }
        let j = p.judgement();
        assert_eq!((j.verdict, j.plc_code, j.fault_code), expected, "{scenario:?}：{}", j.reason);
        let failed = scenario.locate_fail_frame(p.n()).unwrap();
        let frame = &p.frames[failed];
        assert_eq!(frame.status, FrameStatus::Done);
        assert_eq!(frame.reference_failed, [measure::ReferenceFailure { view: frame.view, reason: "定位失败".into() }]);
        assert!(p.recipe.segments.iter().zip(&j.segments).filter(|(g, _)| g.shot == failed).all(|(_, s)| s.reference_invalid));
    }
}

// ───────────── 段配方：三路按帧号配组、每帧送测、收尾规则、未分段判定（旧拍照点配方的用例见上） ─────────────

/// 段配方在途件：samples_burst（cam1–cam3 共用触发、一个直线段 N_exp = 4、容差 1），相机组序号 0–2、会话 11–13，
/// 按计数账定帧计数基线（模拟相机本会话没见过帧时从 0 起）。
fn burst_part(cycle_id: &str, ledgers: &Ledgers) -> Part {
    let recipe = Arc::new(crate::recipe::samples_burst().build().unwrap());
    let cameras = recipe.plan_cameras();
    let identities: Vec<ArmCam> = cameras.iter().enumerate().map(|(cam, id)| ArmCam {
        cam: cam as u8, camera: id.clone(), session: cam as u64 + 11, source: Some(CounterSource::Synthetic), counter_after_open: None,
    }).collect();
    let policy = Policy::burst_offline(None);
    let baselines = arm_frames(&cameras, ledgers, &policy, &identities).unwrap();
    Part {
        cycle_id: cycle_id.into(), bundle_id: None, production: None, candidate: None, sn: "SN-88".into(), scenario: Scenario::Normal,
        frames: Vec::new(), measuring_since: Vec::new(), table: Vec::new(), received: 0, extra: 0, queue: 0, router: unrouted(),
        burst: Some(Box::new(burst::BurstPart::arm(&recipe, baselines, policy.trusted))), issued: None, device_triggers: None,
        routing_closed: false, image_measurement: false, proc_timeout: Duration::from_secs(1), drain_timeout: Duration::from_secs(2),
        armed_at: Instant::now(), end_at: None, fault: None, cams: vec![0, 1, 2], camera_ids: cameras, rig_gen: 1,
        source: RecordSource::Real, recording: None, recipe,
    }
}

/// 相机组序号 cam（会话 11 + cam）帧计数为 counter 的一帧。
pub(crate) fn burst_frame(cam: u8, counter: u64) -> Frame {
    Frame { cam, session: cam as u64 + 11, counter: CounterSource::Synthetic, frame_counter: counter, trigger_counter: counter,
        lost_packets: 0, ts: 1, manual: false, images: Vec::new() }
}

/// 测量池交回的一帧结果。
fn frame_result(part: &Part, camera: usize, seq: u64, result: Result<crate::burst_judge::FrameMetrics, measure::MeasureError>) -> FrameMeasured {
    FrameMeasured { cycle_id: part.cycle_id.clone(), camera_idx: camera, seq, result, ms: 5, queue_ms: None, engine_ms: None }
}

/// 收一帧；被接受时交回合格的模拟测量结果。
fn deliver(part: &mut Part, cam: u8, counter: u64) -> burst::Arrival {
    let arrival = part.receive_burst(&burst_frame(cam, counter)).unwrap();
    if let burst::Arrival::Accepted { camera, seq } = arrival {
        let result = frame_result(part, camera, seq, Ok(measure::simulate_frame(Scenario::Normal, camera, seq, &part.recipe)));
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
    let judgement = part.burst_judgement().unwrap();
    part.burst.as_mut().unwrap().judgement = Some(judgement.clone());
    judgement
}

/// 回调交付的帧（丢掉的帧不在其中）走段配方节拍；partEnd 时 PLC 报 device_triggers，判完返回结论。
pub(crate) fn judge_burst_delivered(received: Vec<Frame>, device_triggers: Option<u16>) -> BurstJudgement {
    let mut p = burst_part("callback-dropped", &Ledgers::default());
    for frame in received {
        if let Some(burst::Arrival::Accepted { camera, seq }) = p.receive_burst(&frame) {
            let result = frame_result(&p, camera, seq, Ok(measure::simulate_frame(Scenario::Normal, camera, seq, &p.recipe)));
            p.apply_burst(result).unwrap();
        }
    }
    end(&mut p, device_triggers, false);
    assert!(p.complete(), "已接受的帧都测完、各相机 max_seq ≥ deviceTriggers − 1");
    judge_burst(&mut p)
}

/// 规格 §3：三路帧按 SDK 帧号（减各相机基线）配组，不按到达顺序；收尾规则：partEnd 前不判，partEnd 后已接受的帧都测完
/// 且每台相机 max_seq ≥ deviceTriggers − 1 即判，PLC 报得更多时等剩余帧或收尾截止。判定结果能按 C5 的记录约束落库。
#[test]
fn three_camera_frames_out_of_arrival_order_are_grouped_and_part_judged_after_end() {
    let mut ledgers = Ledgers::default();
    for (cam, base) in [(0u8, 100u64), (1, 200), (2, 300)] {
        ledgers.observe(&FrameMeta::from(&burst_frame(cam, base)));
    }
    let mut p = burst_part("burst-order", &ledgers);
    // cam1 倒序到达，三台交错
    for (cam, counter) in [(0, 104), (1, 201), (0, 103), (2, 302), (1, 202), (0, 102), (2, 301), (1, 204), (0, 101), (2, 304), (1, 203), (2, 303)] {
        assert!(matches!(deliver(&mut p, cam, counter), burst::Arrival::Accepted { .. }), "相机 {cam} 帧计数 {counter}");
    }
    assert!(!p.complete(), "partEnd 前不判");
    end(&mut p, Some(6), false);
    assert!(!p.complete(), "PLC 报 6、各相机只到第 4 帧：等剩余帧或收尾截止");
    end(&mut p, Some(5), false);
    assert!(p.complete(), "各相机 max_seq 4 ≥ deviceTriggers 5 − 1");
    let judgement = judge_burst(&mut p);
    assert_eq!((judgement.verdict, judgement.group_count, judgement.flags.incomplete), (Verdict::Ok, 4, false));
    let summary = burst::summary(&judgement);
    let record = p.burst_record(&summary).unwrap();
    let identities: Vec<(usize, u64, Option<u64>)> = record.frames.iter().map(|f| (f.camera, f.seq, f.frame_counter)).collect();
    let expected: Vec<(usize, u64, Option<u64>)> = (1..=4u64).flat_map(|seq| (0..3usize).map(move |camera| (camera, seq, Some([100, 200, 300][camera] + seq)))).collect();
    assert_eq!(identities, expected);
    let directory = crate::offline::test_dir::TestDir::new("cycle-burst-tests", "store");
    let store = Store::open(&directory.join("inspection.db")).unwrap();
    let delivery = PlcDelivery { state: PlcDeliveryState::Pending, updated_at: 1, message: None };
    store.insert(&crate::store::PartRecord { ts: 1, sn: &p.sn, recipe: Some(&p.recipe), judgement: &summary, drain_ms: None, frames: &[],
        frames_expected: 4, frames_received: 4, triggers: 5, table: None, software_version: "test", cycle_id: Some(&p.cycle_id), bundle_id: None,
        delivery: &delivery, shots: &[], source: RecordSource::Real, sample: None, burst: Some(&record) }).unwrap();
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
            let mut metrics = measure::simulate_frame(Scenario::Normal, camera, seq, &p.recipe);
            if (camera, seq) == (2, 7) { metrics.gap_max_mm = 100.0; }
            let result = frame_result(&p, camera, seq, Ok(metrics));
            p.apply_burst(result).unwrap();
        }
    }
    end(&mut p, Some(7), false);
    assert!(p.complete());
    let judgement = judge_burst(&mut p);
    assert_eq!((judgement.verdict, judgement.group_count, judgement.flags.frame_count_anomaly), (Verdict::NgGap, 7, true));
    let record = p.burst_record(&burst::summary(&judgement)).unwrap();
    assert_eq!(record.frames.iter().filter(|f| f.status == FrameState::Measured).count(), 21);
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
        first.close_routing(&mut ledgers, None);
        let mut next = burst_part("next", &ledgers);
        if let Some(late) = late {
            assert!(matches!(next.receive_burst(&burst_frame(cam, late)), Some(burst::Arrival::Stale { .. })), "{case}");
        }
        assert_eq!(next.receive_burst(&burst_frame(cam, next_first)), Some(burst::Arrival::Accepted { camera: cam as usize, seq: 1 }), "{case}");
    }
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
            let reason = p.burst.as_ref().unwrap().check_sessions(&cameras).unwrap_err();
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

/// D-SB-13（不裁帧时的背压）：一帧没测成——测量队满、单帧超时、测量失败、到收尾截止仍没结果——只记该组与整件检测不完整，
/// 其余帧照常判，不像旧拍照点配方那样判 ERR 99。
#[test]
fn unmeasured_frame_marks_group_incomplete_not_err_99() {
    use measure::MeasureError;
    for (case, error) in [
        ("测量队满", Some(MeasureError::QueueFull)),
        ("单帧超时", Some(MeasureError::Timeout)),
        ("测量失败", Some(MeasureError::Failed("lyFlow 运行失败".into()))),
        ("到收尾截止仍没结果", None),
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
        end(&mut p, Some(4), error.is_none());
        p.expire_burst(Instant::now(), p.proc_timeout, p.drain_timeout);
        assert!(p.complete(), "{case}");
        let judgement = judge_burst(&mut p);
        assert_eq!((judgement.verdict, judgement.fault_code), (Verdict::Ok, 0), "{case}");
        assert!(judgement.flags.incomplete, "{case}");
        assert_eq!(judgement.regions[0].incomplete_groups, vec![2], "{case}");
    }
}
