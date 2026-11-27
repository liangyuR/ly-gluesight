use super::*;
use crate::recipe_capture::RecipeCaptureHost;
use crate::store::{PlcDelivery, RecordingEvidence};
use crate::workspace::burst_tests::{configs, DEVICE};

const CYCLE: &str = "collect-cycle-7";
const GROUPS: u64 = 3;

fn summary(cycle_id: &str) -> PartSummary {
    PartSummary {
        id: 7,
        project_id: "default".into(),
        ts: 1,
        sn: "SN-7".into(),
        tags: Vec::new(),
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
fn imported_round_references_recording_images_without_copying_them() {
    // 防的回归（在线投运 5.2）：选示教件又把录制原图复制或硬链接进 recipe-capture（多占一份盘、项目占用重复计入）；
    // 引用的轮次（round.json 4、origin）按 cycleId 找不到录制、读出的不是录制里那一帧；录制没了却不提示原图不可用；
    // 重启后认不出这件已导入。原图空闲压缩过的件（盘上是 .gsr，IS-2）选作示教件时结果相同
    for compressed in [false, true] {
        imported_round_references_recording(compressed);
    }
}

fn imported_round_references_recording(compressed: bool) {
    use std::sync::{mpsc::channel, Arc};
    use std::time::Duration;

    use crate::frame::{CounterSource, Frame, FrameImage};
    use crate::recorder::Recorder;
    use crate::settings::RecordMode;

    let root = super::super::tests::ImportTestDir::new();
    let records = crate::project::dir(&root.0, "default").join("records");
    let (tx, rx) = channel();
    let recorder = Recorder::new(
        vec![records.clone()],
        Some(Arc::new(move |outcome| tx.send(outcome).unwrap())),
    );
    let doc = crate::recipe::samples_burst();
    let recipe = Arc::new(doc.build().unwrap());
    let cameras = doc.plan_cameras();
    let pixel = |camera: usize, seq: u64| 10 * seq as u8 + camera as u8;
    let mut rec = recorder
        .begin(RecordMode::All, Some(&records), "SN-7", recipe, CYCLE, None)
        .unwrap();
    for seq in 1..=GROUPS {
        for camera in 0..cameras.len() {
            let frame = Frame {
                cam: camera as u8,
                session: 3,
                counter: CounterSource::Synthetic,
                frame_counter: 100 + seq,
                trigger_counter: seq,
                dev_ts: None,
                lost_packets: 0,
                ts: 1,
                manual: false,
                images: vec![Arc::new(FrameImage::new(2, 1, vec![pixel(camera, seq), 1]))],
                bayer: None,
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
    if compressed {
        assert_eq!(
            crate::raw_compress::compress_all(&directory) as u64,
            GROUPS * cameras.len() as u64
        );
    }
    let evidence = recording(Some(&directory));
    let part_summary = summary(CYCLE);
    let current = configs(&doc, DEVICE);
    let imported = BTreeMap::new();
    let mut roots = crate::project::records::RecordRoots::new(&root.0);
    let listing = assess(
        &doc,
        &current,
        &imported,
        &part_summary,
        &evidence,
        roots
            .resolve(&part_summary.project_id, evidence.directory.as_deref())
            .and_then(|root| read_part(root, &evidence)),
        false,
    );
    let assessed = assess(
        &doc,
        &current,
        &imported,
        &part_summary,
        &evidence,
        read_part(&records, &evidence),
        true,
    );
    assert_eq!(
        serde_json::to_value(&listing.view).unwrap(),
        serde_json::to_value(&assessed.view).unwrap(),
    );
    let round = assessed
        .round
        .unwrap_or_else(|| panic!("录制件应能导入：{:?}", assessed.view.reasons));

    let found = directory.clone();
    let lookup: crate::recipe_capture::RecordingLookup =
        Arc::new(move |cycle| Ok((cycle == CYCLE).then(|| found.clone())));
    let capture_root = root.0.join("recipe-capture");
    let capture = RecipeCaptureHost::open(capture_root.clone(), lookup.clone()).unwrap();
    let round_id = capture.import_history(round).unwrap().round_id;
    let mut files: Vec<String> = std::fs::read_dir(capture_root.join(&round_id))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["frames.jsonl", "round.json"], "轮次目录不应有原图");

    let loaded = capture.get_burst(&round_id).unwrap();
    assert_eq!(
        (loaded.format, loaded.group_count, loaded.state),
        (4, GROUPS, crate::recipe_capture::CaptureState::Complete)
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
    assert_eq!(capture.images_unavailable(&round_id).unwrap(), None);
    std::fs::remove_dir_all(&directory).unwrap();
    assert!(capture.images_unavailable(&round_id).unwrap().is_some());
    assert!(capture.burst_frame_path(&round_id, &cameras[0], 1).is_err());
    drop(capture);
    let reopened = RecipeCaptureHost::open(capture_root, lookup).unwrap();
    assert_eq!(
        imported_cycles(&reopened, &doc.id).unwrap().get(CYCLE),
        Some(&round_id),
        "重启后仍认得已导入的件"
    );
}

#[test]
fn camera_config_differs_from_recording_then_piece_still_usable_with_warning() {
    // 防的回归：项目与示教件导到另一台机器后相机设备配置与录制时不同，选示教件被拦下
    let doc = crate::recipe::samples_burst();
    let cameras = doc.plan_cameras();
    let frames: Vec<Value> = (1..=GROUPS)
        .flat_map(|seq| {
            cameras.iter().enumerate().map(move |(camera, id)| {
                json!({"camera": camera, "cameraId": id, "seq": seq, "frameCounter": 100 + seq,
                    "available": true, "file": format!("{id}_s{seq:06}.pgm"), "width": 2, "height": 1, "ts": 1})
            })
        })
        .collect();
    let part = json!({
        "cycleId": CYCLE,
        "cameras": cameras,
        "groupCount": GROUPS,
        "frames": frames,
        "devices": cameras.iter().map(|id| json!({"cameraId": id, "cameraConfig": DEVICE,
            "imaging": {"exposureUs": 60.0, "gainDb": 6.0}, "deviceSession": 3})).collect::<Vec<_>>(),
        "plc": {"requestSeq": 1, "productCode": doc.product_code},
    });
    let assessed = assess(
        &doc,
        &configs(&doc, "device-on-another-pc"),
        &BTreeMap::new(),
        &summary(CYCLE),
        &recording(Some(Path::new("recorded"))),
        Ok((PathBuf::from("recorded"), part)),
        true,
    );
    assert!(
        assessed.round.is_some() && assessed.view.reasons.is_empty(),
        "{:?}",
        assessed.view.reasons
    );
    assert_eq!(assessed.view.warnings.len(), cameras.len());
}

#[test]
fn recording_timing_uses_all_valid_timestamps_including_unsaved_images() {
    let doc = crate::recipe::samples_burst();
    let cameras = doc.plan_cameras();
    let timestamps = [
        [Some(200), Some(0), Some(100)],
        [Some(400), Some(0), None],
        [Some(20), Some(20), None],
    ];
    let mut frames = Vec::new();
    for (camera, (id, times)) in cameras.iter().zip(timestamps).enumerate() {
        for (index, ts) in times.into_iter().enumerate() {
            let seq = index + 1;
            frames.push(json!({
                "camera": camera, "cameraId": id, "seq": seq, "ts": ts,
                "available": (camera, seq) != (0, 3), "frameCounter": 100 + seq,
                "file": format!("{id}_s{seq:06}.pgm"), "width": 2, "height": 1,
            }));
        }
    }
    let part =
        json!({"cycleId": CYCLE, "cameras": cameras, "groupCount": GROUPS, "frames": frames});
    let assessed = assess(
        &doc,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &summary(CYCLE),
        &recording(Some(Path::new("recorded"))),
        Ok((PathBuf::from("recorded"), part)),
        false,
    );
    assert!(!assessed.view.complete);
    assert_eq!(assessed.view.duration_ms, Some(400));
    assert_eq!(assessed.view.measured_fps, Some(6.25));
}
