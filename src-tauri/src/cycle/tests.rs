use super::*;
use crate::frame::CounterSource;

#[test]
fn interrupted_history_keeps_all_planned_shots_and_each_failure_reason() {
    let mut part = part("cycle-audit", &Ledgers::default());
    part.frames[0].status = FrameStatus::Done;
    part.frames[0].session = Some(18);
    part.frames[0].frame_counter = Some(81);
    part.frames[0].score = Some(0.92);
    part.frames[2].status = FrameStatus::Measuring;
    part.frames[3].status = FrameStatus::LocateFailed;
    part.frames[3].error = Some("定位失败".into());
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
    assert_eq!(shots[3].status, FrameStatus::LocateFailed);
    assert_eq!(shots[3].error.as_deref(), Some("定位失败"));
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
        run_id: 1, cycle_id: cycle_id.into(), bundle_id: Some("release-1".into()), production: None, sn: 88,
        scenario: crate::offline::Scenario::Normal, frames: recipe.shots.iter().map(|shot| FrameView {
            shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view, ..FrameView::waiting()
        }).collect(), measuring_since: vec![None; recipe.shot_count()],
        table: vec![PointState::Pending; recipe.point_count()], received: 0, extra: 0, queue: 0,
        router, issued: None, issued_verified: false, routing_closed: false, image_measurement: false, proc_timeout: Duration::from_secs(1), drain_timeout: Duration::from_secs(2),
        armed_at: Instant::now(), end_at: None, fault: None, cams: (0..cameras.len() as u8).collect(),
        camera_ids: cameras, rig_gen: 1, recording: None, recipe,
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
    Measured { views: Vec::new(), run_id: part.run_id, cycle_id: part.cycle_id.clone(), shot_id: shot.id.clone(),
        camera: shot.camera.clone(), bundle_id: part.bundle_id.clone(), sn: part.sn,
        k, cam: part.frames[k].cam, located: true, score: 0.95, ms: 15,
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
    let SessionEvent::End(ended_at) = event else { panic!("expected partEnd, got {event:?}") };
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
    assert!(judgement.reason.starts_with("拍照点 P4（cam1）没有收到帧：相机 cam1 触发计数错位：本件应收 3–6，实际收到 2–5；2 不大于本件基线 2"),
        "{}", judgement.reason);

    // 多出的触发：第一个超计划帧就是结果原因，写明应收与实际计数
    let mut p = part("extra", &Ledgers::default());
    for counter in 1..=5 { p.receive_frame(&frame(0, counter)).unwrap(); }
    let judgement = p.judgement();
    assert_eq!(judgement.fault_code, fault::EXTRA_FRAME);
    assert!(judgement.reason.contains("相机 cam1 触发计数错位：本件应收 1–4，收到 5，超出计划 1 个"), "{}", judgement.reason);
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

#[test]
fn malformed_measurement_cannot_write_another_shot_or_report_ok() {
    let mut p = part("bad-output", &Ledgers::default());
    p.receive_frame(&frame(0, 1)).unwrap();
    let mut result = measured(&p, 0);
    result.idx[0] = u32::MAX;
    let rejected = p.apply_result(result).unwrap();
    assert!(rejected.error.is_some());
    assert!(rejected.idx.is_empty());
    assert!(p.recipe.owned_points(0).all(|j| p.table[j] == PointState::Invalid));
    assert!(p.recipe.owned_points(1).all(|j| p.table[j] == PointState::Pending));
    assert_eq!(p.judgement().fault_code, fault::PROCESS_TIMEOUT);
}

#[test]
fn identity_batches_commit_together_and_failed_batches_expose_no_partial_ids() {
    let path = std::env::temp_dir().join(format!("gluesight-cycle-batch-{}-{}.sqlite", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let ids;
    {
        let store = Store::open(&path).unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TRIGGER reject_third BEFORE INSERT ON cycle_ids
            WHEN (SELECT count(*) FROM cycle_ids) = 2 BEGIN SELECT RAISE(ABORT, 'disk failure'); END;").unwrap();
        assert!(store.reserve_cycle_ids(4).is_err());
        assert_eq!(conn.query_row("SELECT count(*) FROM cycle_ids", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        conn.execute_batch("DROP TRIGGER reject_third;").unwrap();
        assert!(store.reserve_cycle_ids(0).is_err());
        assert!(store.reserve_cycle_ids(33).is_err());
        ids = store.reserve_cycle_ids(32).unwrap();
        assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), 32);
    }
    {
        let reopened = Store::open(&path).unwrap();
        assert!(!ids.contains(&reopened.reserve_cycle_id().unwrap()));
        let conn = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(conn.query_row("SELECT count(*) FROM cycle_ids", [], |row| row.get::<_, i64>(0)).unwrap(), 33);
    }
    std::fs::remove_file(path).unwrap();
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
