use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::frame_group::LossReason;

static NEXT_TEST: AtomicU64 = AtomicU64::new(1);
/// 后台每 50 ms tick 一次；测试只由 drain() 结束在途等待，避免机器高负载时被后台提前判定。
const TEST_DRAIN_MS: u64 = 30000;

fn host() -> Arc<RecipeCaptureHost> {
    let path = std::env::temp_dir().join(format!("gluesight-burst-{}-{}-{}", std::process::id(), ly_plc::now_ms(), NEXT_TEST.fetch_add(1, Ordering::Relaxed)));
    RecipeCaptureHost::open(path).unwrap()
}

fn device(cam: u8) -> CaptureDevice {
    let baseline = Some(cam as u64 * 100);
    CaptureDevice {
        cam, camera_id: format!("cam{}", cam + 1), camera_config: serde_json::json!({"viewCount": 1}), imaging: crate::recipe::DEFAULT_IMAGING,
        session: 10 + cam as u64, source: Some(CounterSource::Synthetic), frame_baseline: baseline,
    }
}

/// 三台单视角模拟相机 cam1–cam3：会话 10/11/12，帧计数基线 0/100/200。
fn start(host: &RecipeCaptureHost) -> CaptureRoundV2 {
    host.start_burst("recipe-a".into(), (0..3).map(device).collect(), TEST_DRAIN_MS, true).unwrap()
}

/// 第 cam 台相机帧计数为“基线 + n”的帧；像素值为 n，用来认出写盘的是哪一帧。
fn frame(cam: u8, n: u64) -> Frame {
    let counter = cam as u64 * 100 + n;
    Frame { cam, session: 10 + cam as u64, counter: CounterSource::Synthetic, frame_counter: counter, trigger_counter: counter, lost_packets: 0, ts: 0, manual: false,
        images: vec![Arc::new(FrameImage::new(2, 2, vec![n as u8; 4]))] }
}

fn offer(host: &RecipeCaptureHost, f: Frame) {
    assert!(host.offer(&f, None), "采集中的帧必须归本轮");
}

/// 等写盘线程把已入队的帧写完。
fn settle(host: &RecipeCaptureHost) {
    let until = Instant::now() + Duration::from_secs(60);
    while host.burst.lock().unwrap().as_ref().is_some_and(|r| r.round.state.active() && r.cams.iter().any(|c| c.written != c.queued)) {
        assert!(Instant::now() < until, "写盘线程没有在 60 s 内写完");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn drain(host: &RecipeCaptureHost) {
    host.burst.lock().unwrap().as_mut().unwrap().deadline = Instant::now();
    host.tick_burst();
}

// 写盘线程落后（写盘队列满、写盘超时）在测试里造不出来：真实写盘线程写 2×2 的图总是赶得上取图回调，没法让它停顿。
// 只能改本轮的内部计数来模拟，改动集中在下面这几个辅助函数里；用例本身只通过 offer / burst_plc_ended / current_burst 等入口断言结果。

/// 让第 cam 台相机多出 pending 帧“已入队未写完”。
fn add_write_backlog(host: &RecipeCaptureHost, cam: usize, pending: u32) {
    host.burst.lock().unwrap().as_mut().unwrap().cams[cam].queued += pending;
}

/// 还原 [`add_write_backlog`]，让之后的 settle / finish 不会一直等不存在的帧。
fn clear_write_backlog(host: &RecipeCaptureHost, cam: usize, pending: u32) {
    host.burst.lock().unwrap().as_mut().unwrap().cams[cam].queued -= pending;
}

/// PLC 已结束、仍有 1 帧没写完，写盘期限到期，再 tick 一次。
fn expire_write_deadline_with_backlog(host: &RecipeCaptureHost) {
    add_write_backlog(host, 0, 1);
    host.burst.lock().unwrap().as_mut().unwrap().write_deadline = Some(Instant::now());
    drain(host);
}

/// 写完已入队的帧，PLC 结束，drain 窗口到期。
fn finish(host: &RecipeCaptureHost, device_triggers: Option<u16>) -> CaptureRoundV2 {
    settle(host);
    host.burst_plc_ended(device_triggers).unwrap();
    drain(host);
    host.current_burst().unwrap()
}

fn pixel(host: &RecipeCaptureHost, round_id: &str, camera_id: &str, seq: u64) -> u8 {
    crate::image_file::load(&host.burst_frame_path(round_id, camera_id, seq).unwrap()).unwrap().pixels[0]
}

/// 规格 §5：示教轮次 PLC 的 shotCount 可填估算值（含 0、超过 64），PC 只记录，不限 1–64、不核对，轮次照实际帧完成。
/// 0 是下限、65 是旧上限 64 的下一个、32767 是线协议能写的最大值；65 到 32767 之间是同一类，不再列。
#[test]
fn plc_estimate_of_zero_or_above_64_is_only_recorded_and_round_completes_on_actual_frames() {
    for estimate in [0u16, 65, 32767] {
        let host = host();
        start(&host);
        host.burst_plc_started(estimate, 7).unwrap();
        for n in 1..=3 { for cam in 0..3 { offer(&host, frame(cam, n)); } }
        let done = finish(&host, Some(0));
        assert_eq!(done.state, CaptureState::Complete, "估算值 {estimate}：{:?}", done.error);
        assert_eq!((done.plc_shot_count, done.group_count), (Some(estimate), 3), "估算值 {estimate}");
    }
}

/// 规格 §5 结束时不再核对计数、§3 丢帧只告警：某台相机少帧时收到 PLC 结束并过 drain 窗口照常完成，记丢帧告警与不完整帧组。
#[test]
fn camera_short_of_frames_completes_after_plc_end_and_drain_with_loss_alarm_only() {
    let host = host();
    start(&host);
    host.burst_plc_started(10, 1).unwrap();
    for n in 1..=10 { for cam in 0..3 { if cam < 2 || n <= 7 { offer(&host, frame(cam, n)); } } }
    let done = finish(&host, Some(10));
    assert_eq!((done.state, done.error), (CaptureState::Complete, None));
    assert_eq!((done.group_count, done.camera_frames.clone(), done.incomplete_groups.clone()), (10, vec![10, 10, 7], vec![8, 9, 10]));
    assert_eq!(done.loss_alarms, [LossAlarm { camera: 2, count: 7, reason: LossReason::Cameras { other: 0, other_count: 10 } }]);
}

/// 规格 §3：帧按 SDK 帧号配组，不按到达顺序——某台相机少一个帧号只让那一组缺帧，它后面的帧仍在自己的组里，不往前补位。
#[test]
fn camera_skipping_a_frame_counter_leaves_only_that_group_incomplete() {
    let host = host();
    let round = start(&host);
    host.burst_plc_started(5, 1).unwrap();
    for n in [1, 2, 4, 5] { offer(&host, frame(1, n)); }
    for n in 1..=5 { for cam in [2, 0] { offer(&host, frame(cam, n)); } }
    let done = finish(&host, Some(5));
    assert_eq!(done.state, CaptureState::Complete, "{:?}", done.error);
    assert_eq!((done.incomplete_groups.clone(), done.missing_frames.clone()), (vec![3], vec![MissingFrame { camera: 1, seq: 3 }]));
    assert_eq!(pixel(&host, &round.round_id, "cam2", 4), 4);
    assert!(host.burst_frame_path(&round.round_id, "cam2", 3).is_err(), "缺的那一帧没有图");
}

/// 易错边界：同一相机重复的帧号丢弃并告警、保留先到的那帧，本轮照常继续。
#[test]
fn duplicate_counter_is_dropped_with_alarm_and_the_first_frame_is_kept() {
    let host = host();
    let round = start(&host);
    host.burst_plc_started(3, 1).unwrap();
    offer(&host, frame(0, 1));
    offer(&host, frame(0, 2));
    let mut late_copy = frame(0, 2);
    late_copy.images = vec![Arc::new(FrameImage::new(2, 2, vec![255; 4]))];
    offer(&host, late_copy);
    settle(&host);
    let now = host.current_burst().unwrap();
    assert_eq!(now.state, CaptureState::Receiving, "{:?}", now.error);
    assert_eq!(now.alarms, [CaptureAlarm::Duplicate { camera: 0, seq: 2 }]);
    assert_eq!(pixel(&host, &round.round_id, "cam1", 2), 2, "保留先到的那帧");
}

/// 现场 bug：Line0 高电平门控时软件取不了帧，开流后本会话没见过帧也要能开始；各相机以本轮首帧定基线，帧组从 1 起。
#[test]
fn camera_without_session_baseline_starts_and_groups_from_its_first_frame() {
    let host = host();
    let devices = (0..3)
        .map(|cam| CaptureDevice {
            frame_baseline: None,
            ..device(cam)
        })
        .collect();
    let round = host
        .start_burst("recipe-a".into(), devices, TEST_DRAIN_MS, true)
        .unwrap();
    host.burst_plc_started(3, 1).unwrap();
    for n in 1..=3 {
        for cam in 0..3 {
            offer(&host, frame(cam, n + 40));
        }
    }
    let done = finish(&host, Some(0));
    assert_eq!(done.state, CaptureState::Complete, "{:?}", done.error);
    assert_eq!(
        (done.group_count, done.incomplete_groups.clone()),
        (3, vec![])
    );
    assert_eq!(pixel(&host, &round.round_id, "cam2", 1), 41);
}

/// 易错边界：帧号回退或不大于基线说明计数不可信（可能是上一轮的迟到帧），整轮失败并指到该相机。
#[test]
fn backward_counter_or_counter_not_above_baseline_fails_round_naming_the_camera() {
    for (case, counters) in [("帧号回退", vec![1, 3, 2]), ("帧号不大于基线", vec![0])] {
        let host = host();
        start(&host);
        host.burst_plc_started(3, 1).unwrap();
        for &n in &counters { offer(&host, frame(0, n)); }
        settle(&host);
        let now = host.current_burst().unwrap();
        assert_eq!(now.state, CaptureState::Failed, "{case}：{:?}", now.error);
        assert_eq!(now.failure, Some(CaptureFailureV2::Camera { camera_id: "cam1".into() }), "{case}");
    }
}

/// D-SB-8：丢包、图像内存预算满、写盘队列满的帧不写图，记缺帧并告警，整轮照常完成。
#[test]
fn lost_packet_budget_dropped_or_queue_full_frame_is_missing_with_alarm_and_round_completes() {
    // 第 1 台相机的第 2 帧以三种方式拿不到图：丢包、图像内存预算满（camera.rs 没拷图）、写盘队列满
    let cases: [(&str, fn(&RecipeCaptureHost, Frame), CaptureAlarm); 3] = [
        ("丢包", |h, mut f| { f.lost_packets = 3; offer(h, f); }, CaptureAlarm::Corrupt { camera: 0, seq: 2 }),
        ("图像内存预算满", |h, mut f| { f.images.clear(); h.offer_dropped(&f); }, CaptureAlarm::BudgetDropped { camera: 0, seq: 2 }),
        ("写盘队列满", |h, f| {
            add_write_backlog(h, 0, MAX_FRAMES_PER_CAMERA);
            offer(h, f);
            clear_write_backlog(h, 0, MAX_FRAMES_PER_CAMERA);
        }, CaptureAlarm::QueueFull { camera: 0, seq: 2 }),
    ];
    for (case, lose, alarm) in cases {
        let host = host();
        start(&host);
        host.burst_plc_started(3, 1).unwrap();
        for n in 1..=3 {
            for cam in 0..3 {
                if (cam, n) == (0, 2) { lose(&host, frame(cam, n)); } else { offer(&host, frame(cam, n)); }
            }
        }
        let done = finish(&host, Some(3));
        assert_eq!(done.state, CaptureState::Complete, "{case}：{:?}", done.error);
        assert_eq!(done.missing_frames, [MissingFrame { camera: 0, seq: 2 }], "{case}");
        assert_eq!(done.alarms, [alarm], "{case}");
    }
}

/// D-SB-33：采集失败报 90 配 98（相机断连、会话变化等设备原因）或 99（中止、超时等处理原因），不再报计数不符 94；
/// 旧拍照点配方的旧轮次仍报 94（旧路径行为不变）。
#[test]
fn failed_round_reports_device_or_process_fault_not_count_mismatch() {
    let cases: [(&str, fn(&RecipeCaptureHost), (u16, u16)); 8] = [
        ("完成", |h| { h.burst_plc_started(0, 1).unwrap(); offer(h, frame(0, 1)); finish(h, Some(1)); }, (1, 0)),
        ("相机断连", |h| { h.burst_plc_started(0, 1).unwrap(); h.disconnected(1); }, (90, 98)),
        ("设备会话变化", |h| { h.burst_plc_started(0, 1).unwrap(); let mut f = frame(1, 1); f.session = 99; offer(h, f); }, (90, 98)),
        ("手动触发帧", |h| { h.burst_plc_started(0, 1).unwrap(); let mut f = frame(1, 1); f.manual = true; offer(h, f); }, (90, 98)),
        ("操作员中止", |h| h.interrupt("操作员中止本轮采集"), (90, 99)),
        ("等待开始超时", drain, (90, 99)),
        ("PLC 断连", |h| { h.burst_plc_started(0, 1).unwrap(); h.fail("PLC 连接中断"); }, (90, 99)),
        ("写盘超时", |h| {
            h.burst_plc_started(0, 1).unwrap();
            h.burst_plc_ended(Some(1)).unwrap();
            expire_write_deadline_with_backlog(h);
        }, (90, 99)),
    ];
    for (case, make, expected) in cases {
        let host = host();
        start(&host);
        make(&host);
        assert_eq!(host.current_any().unwrap().plc_report(), Some(expected), "{case}");
    }

}

/// 规格 §0 连拍帧数远超 64：3 台相机各 1500 帧逐帧写盘；接收中应用退出后重启，本轮判中断，统计按 frames.jsonl 恢复且与写盘一致。
#[test]
fn thousands_of_frames_are_persisted_and_restored_after_restart() {
    let host = host();
    let round = start(&host);
    host.burst_plc_started(1500, 1).unwrap();
    for n in 1..=1500 { for cam in 0..3 { offer(&host, frame(cam, n)); } }
    settle(&host);
    let reopened = RecipeCaptureHost::open(host.root.clone()).unwrap();
    let AnyRound::Burst(restored) = reopened.get_any(&round.round_id).unwrap() else { panic!("新轮次读成了旧轮次") };
    assert_eq!((restored.state, restored.failure), (CaptureState::Failed, Some(CaptureFailureV2::Interrupted)));
    assert_eq!((restored.group_count, restored.camera_frames.clone()), (1500, vec![1500; 3]));
    assert!(restored.missing_frames.is_empty());
    assert_eq!(pixel(&reopened, &round.round_id, "cam3", 1500), 1500u64 as u8);
}

/// 记录防伪：完整轮次重启后从磁盘读，round.json 与 frames.jsonl 任一处被改（图像指到别处、伪造或删减帧、改统计、改身份）都拒绝读取和取图。
#[test]
fn persisted_record_cannot_redirect_images_or_forge_frames() {
    let host = host();
    let round = start(&host);
    host.burst_plc_started(2, 1).unwrap();
    for n in 1..=2 { for cam in 0..3 { offer(&host, frame(cam, n)); } }
    assert_eq!(finish(&host, Some(2)).state, CaptureState::Complete);
    let dir = host.root.join(&round.round_id);
    let original_round = std::fs::read_to_string(dir.join("round.json")).unwrap();
    let original_lines = std::fs::read_to_string(dir.join(FRAMES_FILE)).unwrap();
    let read = |round_text: &str, lines_text: &str| {
        std::fs::write(dir.join("round.json"), round_text).unwrap();
        std::fs::write(dir.join(FRAMES_FILE), lines_text).unwrap();
        let reopened = RecipeCaptureHost::open(host.root.clone()).unwrap();
        (reopened.get_any(&round.round_id).map(|_| ()), reopened.burst_frame_path(&round.round_id, "cam1", 1).map(|_| ()))
    };
    assert_eq!(read(&original_round, &original_lines), (Ok(()), Ok(())), "未改动的记录");
    let at = |lines: &[serde_json::Value], camera: u64, seq: u64| lines.iter().position(|l| l["camera"] == camera && l["seq"] == seq).unwrap();
    type Tamper = fn(&mut serde_json::Value, &mut Vec<serde_json::Value>, &dyn Fn(&[serde_json::Value], u64, u64) -> usize);
    let cases: [(&str, Tamper); 7] = [
        ("图像路径指到轮次目录外", |_, lines, at| { let i = at(lines, 0, 1); lines[i]["path"] = "../../outside.pgm".into(); }),
        ("帧记录引用本轮之外的相机", |_, lines, at| { let i = at(lines, 0, 2); lines[i]["camera"] = 7.into(); }),
        ("重复的帧记录", |_, lines, at| { let i = at(lines, 1, 1); lines.push(lines[i].clone()); }),
        ("删掉一帧但仍称完整", |_, lines, at| { let i = at(lines, 2, 2); lines.remove(i); }),
        ("帧计数与帧组序号对不上", |_, lines, at| { let i = at(lines, 0, 2); lines[i]["frameCounter"] = 7.into(); }),
        ("多报一台相机的帧数", |round, _, _| { round["cameraFrames"][0] = 3.into(); round["cameras"][0]["receivedCount"] = 3.into(); }),
        ("记录换成别的轮次身份", |round, _, _| { round["roundId"] = "capture-other".into(); }),
    ];
    for (case, tamper) in cases {
        let mut forged: serde_json::Value = serde_json::from_str(&original_round).unwrap();
        let mut lines: Vec<serde_json::Value> = original_lines.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        tamper(&mut forged, &mut lines, &at);
        let text = lines.iter().map(|l| format!("{l}\n")).collect::<String>();
        let (got, image) = read(&forged.to_string(), &text);
        assert!(got.is_err() && image.is_err(), "{case}：{got:?} / {image:?}");
    }
    assert!(host.get_any("capture-../../outside").is_err());
}

#[test]
fn teaching_ready_receives_before_plc_acceptance_and_persists_without_production_metadata() {
    let host = host();
    let initial = start(&host);
    host.burst_ready(12).unwrap();
    for cam in 0..3 {
        offer(&host, frame(cam, 1));
    }
    let done = finish(&host, None);
    assert_eq!(done.state, CaptureState::Complete, "{:?}", done.error);
    assert_eq!(done.group_count, 1);
    assert_eq!(done.teaching_round_id, Some(12));
    assert!(done.plc_plan_version.is_none() && done.plc_shot_count.is_none());
    let saved = host.get_burst(&initial.round_id).unwrap();
    assert_eq!(saved.state, CaptureState::Complete);
    assert_eq!(saved.teaching_round_id, Some(12));
    assert_eq!(pixel(&host, &initial.round_id, "cam1", 1), 1);
}
