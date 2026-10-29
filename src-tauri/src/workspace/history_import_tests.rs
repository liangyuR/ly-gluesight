use super::*;
use crate::recipe_capture::RecipeCaptureHost;
use crate::store::{PlcDelivery, RecordingEvidence};
use crate::workspace::burst_tests::{configs, DEVICE};

const CYCLE: &str = "collect-cycle-7";
const GROUPS: u64 = 3;

fn summary(cycle_id: &str) -> PartSummary {
    PartSummary {
        id: 7,
        ts: 1,
        sn: "SN-7".into(),
        recipe_id: None,
        recipe_version: None,
        recipe_revision: None,
        trigger_mode: None,
        verdict: crate::judge::Verdict::ErrInspect,
        plc_code: 0,
        fault_code: 0,
        reason: "维护采集".into(),
        drain_ms: None,
        frames_expected: 0,
        frames_received: 0,
        retest_of: None,
        cycle_id: Some(cycle_id.into()),
        bundle_id: None,
        delivery: PlcDelivery::default(),
        source: RecordSource::Real,
        sample: None,
        flags: None,
        burst: None,
        mode: PartMode::MaintenanceCollect,
    }
}

fn recording(directory: Option<&Path>) -> RecordingEvidence {
    RecordingEvidence {
        state: RecordingState::Complete,
        available: true,
        directory: directory.map(|d| d.to_string_lossy().into_owned()),
        errors: Vec::new(),
    }
}

/// 一期录制的 part.json 里导入要看的字段：三台计划相机各 GROUPS 帧，帧号 100 + seq，设备配置为 DEVICE。

#[test]
fn imported_round_frames_stay_readable_after_history_recording_is_deleted() {
    // 防的回归（在线投运 5.2 物化方式）：轮次直接引用历史目录或复制出错，历史记录被保留策略清理后示教帧与标定原图失效；
    // 物化的轮次（round.json 3、origin）下游读不了；重启后认不出这件已导入
    use std::sync::{mpsc::channel, Arc};
    use std::time::Duration;

    use crate::frame::{CounterSource, Frame, FrameImage};
    use crate::recorder::Recorder;
    use crate::settings::RecordMode;

    let root = super::super::tests::ImportTestDir::new();
    let records = root.0.join("records");
    let (tx, rx) = channel();
    let recorder = Recorder::new(
        records.clone(),
        Some(Arc::new(move |outcome| tx.send(outcome).unwrap())),
    );
    let doc = crate::recipe::samples_burst();
    let recipe = Arc::new(doc.build().unwrap());
    let cameras = doc.plan_cameras();
    let pixel = |camera: usize, seq: u64| 10 * seq as u8 + camera as u8;
    let mut rec = recorder
        .begin(RecordMode::All, "SN-7", recipe, CYCLE, None)
        .unwrap();
    for seq in 1..=GROUPS {
        for camera in 0..cameras.len() {
            let frame = Frame {
                cam: camera as u8,
                session: 3,
                counter: CounterSource::Synthetic,
                frame_counter: 100 + seq,
                trigger_counter: seq,
                lost_packets: 0,
                ts: 1,
                manual: false,
                images: vec![Arc::new(FrameImage::new(2, 1, vec![pixel(camera, seq), 1]))],
            };
            recorder.burst_frame(&mut rec, &frame, camera, seq);
        }
    }
    rec.annotate("partMode", json!("maintenanceCollect"));
    rec.annotate(
        "devices",
        json!(cameras
            .iter()
            .map(|id| json!({"cameraId": id, "cameraConfig": DEVICE,
        "imaging": {"exposureUs": 60.0, "gainDb": 6.0}, "deviceSession": 3}))
            .collect::<Vec<_>>()),
    );
    rec.annotate(
        "plc",
        json!({"requestSeq": 1, "productCode": doc.product_code}),
    );
    recorder.finish(
        rec,
        crate::judge::Verdict::ErrInspect,
        "维护采集",
        10,
        u64::MAX,
        Vec::new(),
    );
    let outcome = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("录制应完成收尾");
    let directory = outcome.directory.clone().expect("录制目录");
    let evidence = recording(Some(&directory));
    let assessed = assess(
        &doc,
        &configs(&doc, DEVICE),
        &BTreeMap::new(),
        &summary(CYCLE),
        &evidence,
        read_part(&records, &evidence),
    );
    let round = assessed
        .round
        .unwrap_or_else(|| panic!("录制件应能导入：{:?}", assessed.view.reasons));

    let capture = RecipeCaptureHost::open(root.0.join("recipe-capture")).unwrap();
    let round_id = capture.import_history(round).unwrap().round_id;
    std::fs::remove_dir_all(&directory).unwrap();

    let loaded = capture.get_burst(&round_id).unwrap();
    assert_eq!(
        (loaded.format, loaded.group_count, loaded.state),
        (3, GROUPS, crate::recipe_capture::CaptureState::Complete)
    );
    assert_eq!(
        loaded.origin.as_ref().map(RoundOrigin::cycle_id),
        Some(CYCLE)
    );
    for (camera, id) in cameras.iter().enumerate() {
        for seq in 1..=GROUPS {
            let image =
                crate::image_file::load(&capture.burst_frame_path(&round_id, id, seq).unwrap())
                    .unwrap();
            assert_eq!(
                image.pixels,
                [pixel(camera, seq), 1],
                "相机 {id} 第 {seq} 帧组"
            );
        }
    }
    drop(capture);
    let reopened = RecipeCaptureHost::open(root.0.join("recipe-capture")).unwrap();
    assert_eq!(
        imported_cycles(&reopened, &doc.id).unwrap().get(CYCLE),
        Some(&round_id),
        "重启后仍认得已导入的件"
    );
}
