use super::*;
use crate::burst_judge::Flags;
use crate::plan::SegmentKind::Straight;
use crate::recipe_capture::RecipeCaptureHost;
use crate::store::{PlcDelivery, RecordingEvidence};
use crate::workspace::burst::calibration_round;
use crate::workspace::burst_tests::{burst_workspace, capture_round, configs, DEVICE};

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
fn part(doc: &RecipeDoc) -> Value {
    let cameras = doc.plan_cameras();
    let frames: Vec<Value> = cameras.iter().enumerate().flat_map(|(camera, id)| (1..=GROUPS).map(move |seq| json!({
        "cycleId": CYCLE, "camera": camera, "cameraId": id, "seq": seq, "file": format!("{id}_s{seq:06}.pgm"),
        "frameCounter": 100 + seq, "manual": false, "width": 2, "height": 1, "ts": 1, "queued": true, "available": true,
    }))).collect();
    json!({
        "cycleId": CYCLE, "cameras": cameras, "groupCount": GROUPS, "missingFrames": [], "rawDropped": [], "frames": frames,
        "partMode": "maintenanceCollect",
        "devices": cameras.iter().map(|id| json!({"cameraId": id, "cameraConfig": DEVICE,
            "imaging": {"exposureUs": 60.0, "gainDb": 6.0}, "deviceSession": 3})).collect::<Vec<_>>(),
        "plc": {"requestSeq": 1, "productCode": doc.product_code},
    })
}

struct Case {
    part: Value,
    summary: PartSummary,
    recording: RecordingEvidence,
    current: BTreeMap<String, Value>,
    imported: BTreeMap<String, String>,
}

fn assess_case(doc: &RecipeDoc, case: Case) -> Assessed {
    assess(
        doc,
        &case.current,
        &case.imported,
        &case.summary,
        &case.recording,
        Ok((PathBuf::from("records"), case.part)),
    )
}

#[test]
fn history_part_incomplete_different_or_foreign_is_not_importable() {
    // 防的回归（在线投运 5.2 可用条件）：用缺帧、录制队满、相机配置变过、提前结束、别的型号、少相机、旧录制或离线的件示教与标定，
    // 或同一件重复导入
    let doc = burst_workspace(&[Straight]).doc;
    let base = || Case {
        part: part(&doc),
        summary: summary(CYCLE),
        recording: recording(None),
        current: configs(&doc, DEVICE),
        imported: BTreeMap::new(),
    };
    let ok = assess_case(&doc, base());
    assert!(
        ok.view.reasons.is_empty(),
        "完整件应能导入：{:?}",
        ok.view.reasons
    );
    assert!(ok.view.complete);
    assert_eq!(
        ok.round.as_ref().map(|r| r.frames.len()),
        Some(3 * GROUPS as usize)
    );
    let camera = doc.plan_cameras()[1].clone();
    let cases: Vec<(&str, Box<dyn Fn(&mut Case)>)> = vec![
        (
            "缺帧",
            Box::new(|c| {
                c.part["frames"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|f| !(f["camera"] == 1 && f["seq"] == 2));
                c.part["missingFrames"] = json!([{"camera": 1, "seq": 2}]);
            }),
        ),
        (
            "录制队满未录",
            Box::new(|c| {
                c.part["frames"][0]["available"] = json!(false);
                c.part["rawDropped"] = json!([{"camera": 0, "seq": 1}]);
            }),
        ),
        (
            "相机设备配置与当前不同",
            Box::new(move |c| {
                c.current.insert(camera.clone(), json!("device-b"));
            }),
        ),
        (
            "整件标记疑似提前终止",
            Box::new(|c| {
                c.summary.flags = Some(Flags {
                    short_part: true,
                    ..Flags::default()
                })
            }),
        ),
        (
            "PLC 提前结束",
            Box::new(|c| c.part["plc"]["earlyEnd"] = json!(true)),
        ),
        (
            "型号不属于本候选",
            Box::new(|c| c.part["plc"]["productCode"] = json!(9999)),
        ),
        (
            "录制相机不是计划相机",
            Box::new(|c| c.part["cameras"][2] = json!("cam9")),
        ),
        (
            "旧录制没有设备与 PLC 元数据",
            Box::new(|c| {
                c.part.as_object_mut().unwrap().remove("devices");
                c.part.as_object_mut().unwrap().remove("plc");
            }),
        ),
        (
            "帧号与帧组序号对不上",
            Box::new(|c| c.part["frames"][1]["frameCounter"] = json!(999)),
        ),
        (
            "离线记录",
            Box::new(|c| c.summary.source = RecordSource::OfflineSample),
        ),
        (
            "原图已按保留策略清理",
            Box::new(|c| {
                c.recording.state = RecordingState::NotRetained;
                c.recording.available = false;
            }),
        ),
        (
            "本候选已选用",
            Box::new(|c| {
                c.imported.insert(CYCLE.into(), "capture-1-0".into());
            }),
        ),
    ];
    for (name, edit) in cases {
        let mut case = base();
        edit(&mut case);
        let assessed = assess_case(&doc, case);
        assert!(
            assessed.round.is_none() && !assessed.view.reasons.is_empty(),
            "{name} 不能导入"
        );
    }
}

#[test]
fn group_count_well_below_batch_median_of_importable_parts_warns_without_blocking_import() {
    // 防的回归（维护采集补缺口）：采集版录制没有提前结束标记，帧组数明显偏少的件混进示教与标定没有任何提示；
    // 不可导入的件或件数太少时的中位数误报；提示变成阻断，提前结束的判断错时整件无法导入
    #[derive(Clone, Copy, PartialEq)]
    enum Part {
        Clean,
        Blocked,
        Imported,
    }
    use Part::*;
    let doc = burst_workspace(&[Straight]).doc;
    let cases: [(&str, &[(u64, Part)], &[usize]); 5] = [
        (
            "低于中位数 0.9 倍",
            &[(10, Clean), (10, Clean), (10, Clean), (8, Clean)],
            &[3],
        ),
        (
            "恰为中位数 0.9 倍",
            &[(10, Clean), (10, Clean), (10, Clean), (9, Clean)],
            &[],
        ),
        (
            "可导入件少于 3 件，不可导入件不计入",
            &[(10, Clean), (5, Clean), (10, Blocked)],
            &[],
        ),
        (
            "中位数只按可导入件算",
            &[
                (10, Clean),
                (10, Clean),
                (10, Clean),
                (2, Blocked),
                (2, Blocked),
                (2, Blocked),
                (2, Blocked),
            ],
            &[3, 4, 5, 6],
        ),
        (
            "已导入的件仍计入中位数",
            &[(10, Imported), (10, Imported), (10, Clean), (8, Clean)],
            &[3],
        ),
    ];
    for (name, parts, expected) in cases {
        let mut assessed: Vec<Assessed> = parts
            .iter()
            .map(|&(groups, state)| {
                let mut a = assess_case(
                    &doc,
                    Case {
                        part: part(&doc),
                        summary: summary(CYCLE),
                        recording: recording(None),
                        current: configs(&doc, DEVICE),
                        imported: BTreeMap::new(),
                    },
                );
                a.view.group_count = groups;
                match state {
                    Clean => {}
                    Blocked => {
                        a.view.reasons.push("缺帧 1 帧".into());
                        a.round = None;
                    }
                    Imported => {
                        a.view.imported_round = Some("R01".into());
                        a.view.reasons.push("本候选已选用这一件".into());
                        a.round = None;
                    }
                }
                a
            })
            .collect();
        flag_short_parts(&mut assessed);
        let warned: Vec<usize> = (0..assessed.len())
            .filter(|&i| !assessed[i].view.warnings.is_empty())
            .collect();
        assert_eq!(warned, expected, "{name}");
        for (i, a) in assessed.iter().enumerate() {
            if parts[i].1 == Clean {
                assert!(
                    a.round.is_some() && a.view.reasons.is_empty(),
                    "{name}：第 {i} 件有提示也应能导入"
                );
            }
        }
    }
}

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

#[test]
fn fixture_part_is_importable_history_piece_with_source_label() {
    // 防的回归（recipe-build-flow「手动配置数据」）：capture_fixture 写的仿历史件与导入契约脱节——part.json 缺 partMode、
    // devices、plc，帧号或原图文件名不合规，记录行缺录制证据或不按本配方入库——端到端与长稳测试选不到示教件和标定件；
    // 来源标记丢失时选择器认不出手动配置件；记录的原图引用与录制目录对不上时，仿历史件不能作独立验证样本
    use crate::history_fixture::{FixtureFrames, FixtureSite};
    use crate::store::Store;

    let root = super::super::tests::ImportTestDir::new();
    let records = root.0.join("records");
    std::fs::create_dir(&records).unwrap();
    let db = root.0.join("inspection.db");
    drop(Store::open(&db).unwrap());
    let doc = crate::recipe::samples_burst();
    let cameras = doc.plan_cameras();
    let site = FixtureSite::open(
        &db,
        &records,
        doc.build().unwrap(),
        cameras.iter().map(|_| json!(DEVICE)).collect(),
        "unit",
    )
    .unwrap();
    let frames = (1..=GROUPS)
        .map(|seq| {
            (0..cameras.len())
                .map(|camera| {
                    Some(crate::frame::FrameImage::new(
                        2,
                        1,
                        vec![seq as u8, camera as u8],
                    ))
                })
                .collect()
        })
        .collect();
    let written = site
        .write("FIXTURE-1", FixtureFrames { frames, hz: 10.0 })
        .unwrap();
    drop(site);

    let store = Store::open(&db).unwrap();
    let (summary, recording) = store
        .recorded_parts(&doc.id, 10)
        .unwrap()
        .into_iter()
        .find(|(s, _)| s.id == written.record_id)
        .expect("仿历史件应按本配方的真机录制件入库");
    let assessed = assess(
        &doc,
        &configs(&doc, DEVICE),
        &BTreeMap::new(),
        &summary,
        &recording,
        read_part(&records, &recording),
    );
    assert!(
        assessed.view.reasons.is_empty(),
        "仿历史件应能导入：{:?}",
        assessed.view.reasons
    );
    assert!(assessed.view.complete);
    assert_eq!(assessed.view.mode, PartMode::MaintenanceCollect);
    assert_eq!(assessed.view.fixture.as_deref(), Some("unit"));
    assert_eq!(
        assessed.round.map(|r| r.frames.len()),
        Some(cameras.len() * GROUPS as usize)
    );
    let detail = store.detail(written.record_id).unwrap();
    for frame in &detail.burst.as_ref().unwrap().frames {
        let image = super::super::recorded::load_frame_verified(
            &records,
            &written.cycle_id,
            &cameras[frame.camera],
            frame,
        )
        .unwrap();
        assert_eq!(image.pixels, [frame.seq as u8, frame.camera as u8]);
    }
}

#[test]
fn imported_part_used_for_teaching_or_calibration_is_not_a_validation_sample() {
    // 防的回归（在线投运 5.3）：已导入并用于走廊示教或多轮标定的历史件又选作独立验证样本，用训练数据自证；
    // 录制时相机配置与当前不同的件也拿来验证
    let mut w = burst_workspace(&[Straight]);
    let camera = w.doc.plan_cameras()[0].clone();
    w.camera_teaching.push(serde_json::from_value(json!({"camera": camera, "round": "capture-teach", "seq": 1, "segment": 1,
        "frame": {"id": "t", "view": 1, "source": "", "capturedAt": 0, "size": [2, 1], "camera": camera, "cameraTag": null,
            "calibTag": null, "geometryTag": null, "exposureUs": null, "gainDb": null, "historyId": null}})).unwrap());
    let calibrated = capture_round("capture-calib", &w.doc, GROUPS, DEVICE, &[]);
    w.calibration
        .rounds
        .push(calibration_round(&w.doc, &calibrated, false, &[]));
    let origin = |cycle: &str| RoundOrigin::History {
        cycle_id: cycle.into(),
        record_id: 1,
        standard: false,
    };
    let origins: BTreeMap<String, RoundOrigin> = [
        ("capture-teach", origin("c-teach")),
        ("capture-calib", origin("c-calib")),
        ("capture-unused", origin("c-unused")),
    ]
    .into_iter()
    .map(|(round, origin)| (round.to_string(), origin))
    .collect();
    let current = configs(&w.doc, DEVICE);
    let devices = |config: &str| json!([{"cameraId": camera, "cameraConfig": config}]);
    let cases = [
        ("导入后用于走廊示教", Some("c-teach"), None, false),
        ("导入后加入多轮标定", Some("c-calib"), None, false),
        ("导入了但没用于示教或标定", Some("c-unused"), None, true),
        ("没导入过", Some("c-other"), Some(devices(DEVICE)), true),
        (
            "录制时相机配置与当前不同",
            Some("c-other"),
            Some(devices("device-b")),
            false,
        ),
    ];
    for (name, cycle, devices, allowed) in cases {
        assert_eq!(
            check_history_sample(&w, cycle, &origins, devices.as_ref(), &current).is_ok(),
            allowed,
            "{name}"
        );
    }
}
