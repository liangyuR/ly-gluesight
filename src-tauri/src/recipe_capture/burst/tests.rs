use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::{json, Value};

use super::*;

static NEXT_TEST: AtomicU64 = AtomicU64::new(1);
const GROUPS: u64 = 2;
const ROUND: &str = "capture-1-0";

/// 没有任何检测记录的本机：历史件导入轮次的录制原图都查不到。
fn no_recordings() -> RecordingLookup {
    Arc::new(|_| Ok(None))
}

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "gluesight-burst-{}-{}-{}",
        std::process::id(),
        ly_plc::now_ms(),
        NEXT_TEST.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// 盘上一轮原图在本轮目录的完整旧轮次（format 3）：cam1–cam3 各 GROUPS 帧（像素值为帧组序号），写好图像与 frames.jsonl；
/// 返回 round.json 内容供用例改写后写入。
fn complete_round(root: &Path) -> Value {
    complete_round_with_groups(root, GROUPS)
}

fn complete_round_with_groups(root: &Path, groups: u64) -> Value {
    let dir = root.join(ROUND);
    let mut lines = String::new();
    let mut saved = vec![BTreeSet::new(); 3];
    let cameras = (0..3usize)
        .map(|camera| {
            let camera_id = format!("cam{}", camera + 1);
            std::fs::create_dir_all(dir.join(&camera_id)).unwrap();
            for seq in 1..=groups {
                let path = frame_rel_path(&camera_id, seq);
                crate::image_file::save_pgm(
                    &dir.join(&path),
                    &FrameImage::new(2, 2, vec![seq as u8; 4]),
                )
                .unwrap();
                let line = FrameLine {
                    camera,
                    seq,
                    frame_counter: 100 * camera as u64 + seq,
                    trigger_counter: None,
                    path,
                    width: 2,
                    height: 2,
                    ts: 0,
                };
                lines.push_str(&format!("{}\n", serde_json::to_string(&line).unwrap()));
                saved[camera].insert(seq);
            }
            CaptureCamera {
                camera_id,
                camera_config: json!({"viewCount": 1}),
                imaging: crate::camera::DEFAULT_IMAGING,
                synthetic_capture: None,
                device_session: 10 + camera as u64,
                received_count: 0,
            }
        })
        .collect();
    std::fs::write(dir.join(FRAMES_FILE), lines).unwrap();
    let mut round = CaptureRoundV2 {
        format: FORMAT_V3,
        round_id: ROUND.into(),
        recipe_id: "recipe-a".into(),
        cameras,
        state: CaptureState::Complete,
        plc_shot_count: None,
        _plc_plan_version: crate::workspace::Discarded,
        teaching_round_id: None,
        device_triggers: None,
        group_count: 0,
        camera_frames: Vec::new(),
        incomplete_groups: Vec::new(),
        missing_frames: Vec::new(),
        loss_alarms: Vec::new(),
        alarms: Vec::new(),
        created_at: 1,
        ended_at: Some(2),
        error: None,
        failure: None,
        simulated: false,
        origin: None,
    };
    apply(&mut round, summarize(groups, &saved));
    serde_json::to_value(&round).unwrap()
}

fn write_round(root: &Path, round: &Value) {
    std::fs::write(root.join(ROUND).join("round.json"), round.to_string()).unwrap();
}

fn patched(round: &Value, patch: Value) -> Value {
    let mut round = round.clone();
    for (key, value) in patch.as_object().unwrap() {
        round[key] = value.clone();
    }
    round
}

#[test]
fn round_list_uses_the_last_format_to_decide_whether_to_validate_the_record() {
    let root = root();
    let host = RecipeCaptureHost::open(root.clone(), no_recordings()).unwrap();
    let round = patched(&complete_round(&root), json!({"plcShotCount": GROUPS}));
    write_round(&root, &round);
    assert_eq!(host.list("recipe-a").unwrap().len(), 1);
    let text = round.to_string();
    let path = root.join(ROUND).join("round.json");

    std::fs::write(
        &path,
        format!(r#"{},"format":1}}"#, &text[..text.len() - 1]),
    )
    .unwrap();
    assert!(host.list("recipe-a").unwrap().is_empty());

    std::fs::write(&path, format!(r#"{{"format":1,{}"#, &text[1..])).unwrap();
    assert!(host.list("recipe-a").is_err());
}

/// 应用退出时仍在接收的轮次：重启后判中断，统计按 frames.jsonl 恢复。
#[test]
fn round_left_receiving_on_disk_then_reopen_marks_it_interrupted_with_counts_from_frames() {
    let root = root();
    let round = patched(
        &complete_round(&root),
        json!({"format": 2, "state": "receiving", "teachingRoundId": 12, "endedAt": null, "groupCount": 0, "cameraFrames": [0, 0, 0], "incompleteGroups": [], "missingFrames": [], "lossAlarms": []}),
    );
    write_round(&root, &round);
    let host = RecipeCaptureHost::open(root, no_recordings()).unwrap();
    let AnyRound::Burst(restored) = host.get_any(ROUND).unwrap();
    assert_eq!(
        (restored.state, restored.failure),
        (CaptureState::Failed, Some(CaptureFailureV2::Interrupted))
    );
    assert_eq!(
        (restored.group_count, restored.camera_frames),
        (GROUPS, vec![GROUPS as u32; 3])
    );
}

/// 记录防伪：round.json 与 frames.jsonl 任一处被改（图像指到别处、伪造或删减帧、改统计、改身份）都拒绝读取和取图。
#[test]
fn persisted_record_cannot_redirect_images_or_forge_frames() {
    let root = root();
    let original_round = patched(
        &complete_round(&root),
        json!({"origin": {"kind": "prepared", "label": "手动数据"}}),
    )
    .to_string();
    let dir = root.join(ROUND);
    let original_lines = std::fs::read_to_string(dir.join(FRAMES_FILE)).unwrap();
    let read = |round_text: &str, lines_text: &str| {
        std::fs::write(dir.join("round.json"), round_text).unwrap();
        std::fs::write(dir.join(FRAMES_FILE), lines_text).unwrap();
        let reopened = RecipeCaptureHost::open(root.clone(), no_recordings()).unwrap();
        (
            reopened.get_any(ROUND).map(|_| ()),
            reopened.burst_frame_path(ROUND, "cam1", 1).map(|_| ()),
        )
    };
    assert_eq!(
        read(&original_round, &original_lines),
        (Ok(()), Ok(())),
        "未改动的记录"
    );
    let at = |lines: &[Value], camera: u64, seq: u64| {
        lines
            .iter()
            .position(|l| l["camera"] == camera && l["seq"] == seq)
            .unwrap()
    };
    type Tamper = fn(&mut Value, &mut Vec<Value>, &dyn Fn(&[Value], u64, u64) -> usize);
    let cases: [(&str, Tamper); 7] = [
        ("图像路径指到轮次目录外", |_, lines, at| {
            let i = at(lines, 0, 1);
            lines[i]["path"] = "../../outside.pgm".into();
        }),
        ("帧记录引用本轮之外的相机", |_, lines, at| {
            let i = at(lines, 0, 2);
            lines[i]["camera"] = 7.into();
        }),
        ("重复的帧记录", |_, lines, at| {
            let i = at(lines, 1, 1);
            lines.push(lines[i].clone());
        }),
        ("删掉一帧但仍称完整", |_, lines, at| {
            let i = at(lines, 2, 2);
            lines.remove(i);
        }),
        ("帧计数与帧组序号对不上", |_, lines, at| {
            let i = at(lines, 0, 2);
            lines[i]["frameCounter"] = 7.into();
        }),
        ("多报一台相机的帧数", |round, _, _| {
            round["cameraFrames"][0] = 3.into();
            round["cameras"][0]["receivedCount"] = 3.into();
        }),
        ("记录换成别的轮次身份", |round, _, _| {
            round["roundId"] = "capture-other".into();
        }),
    ];
    for (case, tamper) in cases {
        let mut forged: Value = serde_json::from_str(&original_round).unwrap();
        let mut lines: Vec<Value> = original_lines
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        tamper(&mut forged, &mut lines, &at);
        let text = lines.iter().map(|l| format!("{l}\n")).collect::<String>();
        let (got, image) = read(&forged.to_string(), &text);
        assert!(
            got.is_err() && image.is_err(),
            "{case}：{got:?} / {image:?}"
        );
    }
    let host = RecipeCaptureHost::open(root.clone(), no_recordings()).unwrap();
    assert!(host.get_any("capture-../../outside").is_err());
}

/// 以前导入的历史件轮次（format 3，原图在本轮目录）在导出、复制项目时没带原图：防的回归是这类示教件从此读不了图、
/// 只能重选示教件。本轮目录缺图时按来源 cycleId 读本机录制里的同一帧；本机没有这件记录时报原图不可用，不读别处的图。
#[test]
fn old_history_round_without_local_images_reads_the_source_recording() {
    let root = root();
    let round = patched(
        &complete_round(&root),
        json!({"origin": {"kind": "history", "cycleId": "cycle-a", "recordId": 7}}),
    );
    write_round(&root, &round);
    let recording = root.join("records").join("20261107_cycle_cycle-a");
    std::fs::create_dir_all(&recording).unwrap();
    for camera in ["cam1", "cam2", "cam3"] {
        for seq in 1..=GROUPS {
            std::fs::rename(
                root.join(ROUND).join(frame_rel_path(camera, seq)),
                recording.join(recording_file(camera, seq)),
            )
            .unwrap();
        }
    }
    let found = recording.clone();
    let available = Arc::new(AtomicBool::new(true));
    let lookup = Arc::clone(&available);
    let host = RecipeCaptureHost::open(
        root.clone(),
        Arc::new(move |cycle| {
            Ok((lookup.load(Ordering::Relaxed) && cycle == "cycle-a").then(|| found.clone()))
        }),
    )
    .unwrap();
    assert_eq!(host.images_unavailable(ROUND).unwrap(), None);
    let image = crate::image_file::load(&host.burst_frame_path(ROUND, "cam2", 2).unwrap()).unwrap();
    assert_eq!(image.pixels, vec![2u8; 4]);

    let pgm = recording.join(recording_file("cam2", 2));
    let encoded = crate::image_file::gsr::encode(&std::fs::read(&pgm).unwrap()).unwrap();
    std::fs::write(pgm.with_extension("gsr"), encoded).unwrap();
    std::fs::remove_file(pgm).unwrap();
    let restored =
        crate::image_file::load(&host.burst_frame_path(ROUND, "cam2", 2).unwrap()).unwrap();
    assert_eq!(restored.pixels, image.pixels);

    available.store(false, Ordering::Relaxed);
    assert!(host.images_unavailable(ROUND).unwrap().is_some());
    assert!(host.burst_frame_path(ROUND, "cam2", 2).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn long_recording_availability_keeps_the_first_missing_frame_diagnostic() {
    let root = root();
    let groups = (FRAME_AVAILABILITY_BATCH_MIN / 3 + 2) as u64;
    let round = patched(
        &complete_round_with_groups(&root, groups),
        json!({"format": FORMAT, "origin": {"kind": "history", "cycleId": "cycle-a", "recordId": 7}}),
    );
    write_round(&root, &round);
    let dir = root.join(ROUND);
    let recording = root.join("records").join("20261107_cycle_cycle-a");
    std::fs::create_dir_all(&recording).unwrap();
    let log = dir.join(FRAMES_FILE);
    let mut lines = String::new();
    for text in std::fs::read_to_string(&log).unwrap().lines() {
        let mut line: FrameLine = serde_json::from_str(text).unwrap();
        let local = dir.join(&line.path);
        line.path = recording_file(&format!("cam{}", line.camera + 1), line.seq);
        std::fs::rename(local, recording.join(&line.path)).unwrap();
        lines.push_str(&serde_json::to_string(&line).unwrap());
        lines.push('\n');
    }
    std::fs::write(&log, lines).unwrap();
    let source = recording.clone();
    let host = RecipeCaptureHost::open(root, Arc::new(move |_| Ok(Some(source.clone())))).unwrap();

    let gsr_pgm = recording.join(recording_file("cam1", groups - 2));
    let encoded = crate::image_file::gsr::encode(&std::fs::read(&gsr_pgm).unwrap()).unwrap();
    std::fs::write(gsr_pgm.with_extension("gsr"), encoded).unwrap();
    std::fs::remove_file(gsr_pgm).unwrap();
    assert_eq!(host.images_unavailable(ROUND).unwrap(), None);

    let name = recording_file("cam1", groups - 1);
    let plain = recording.join(&name);
    let upper = recording.join(name.to_ascii_uppercase());
    let temp = recording.join("renaming.tmp");
    std::fs::rename(&plain, &temp).unwrap();
    std::fs::rename(&temp, &upper).unwrap();
    assert_eq!(
        host.images_unavailable(ROUND).unwrap().is_none(),
        crate::image_file::exists(&plain)
    );
    std::fs::rename(&upper, &temp).unwrap();
    std::fs::rename(&temp, &plain).unwrap();

    let first_missing = recording.join(recording_file("cam1", groups));
    std::fs::remove_file(&first_missing).unwrap();
    std::fs::create_dir(first_missing).unwrap();
    std::fs::remove_file(recording.join(recording_file("cam2", 1))).unwrap();
    let reason = host.images_unavailable(ROUND).unwrap().unwrap();
    assert!(
        reason.contains(&format!("cam1 第 {groups} 帧组")),
        "{reason}"
    );
}

#[test]
fn long_legacy_round_keeps_local_pgm_priority_and_accepts_source_gsr() {
    let root = root();
    let groups = FRAME_AVAILABILITY_BATCH_MIN as u64 + 3;
    let round = patched(
        &complete_round_with_groups(&root, groups),
        json!({"origin": {"kind": "history", "cycleId": "cycle-a", "recordId": 7}}),
    );
    write_round(&root, &round);
    let dir = root.join(ROUND);
    let recording = root.join("records").join("20261107_cycle_cycle-a");
    std::fs::create_dir_all(&recording).unwrap();
    let source = recording.clone();
    let available = Arc::new(AtomicBool::new(false));
    let lookup = Arc::clone(&available);
    let host = RecipeCaptureHost::open(
        root,
        Arc::new(move |cycle| {
            Ok((lookup.load(Ordering::Relaxed) && cycle == "cycle-a").then(|| source.clone()))
        }),
    )
    .unwrap();
    assert_eq!(host.images_unavailable(ROUND).unwrap(), None);

    let local = dir.join(frame_rel_path("cam1", 1));
    std::fs::copy(&local, recording.join(recording_file("cam1", 1))).unwrap();
    for seq in 17..=36 {
        let path = dir.join(frame_rel_path("cam1", seq));
        let target = recording.join(recording_file("cam1", seq));
        if seq == 36 {
            let encoded = crate::image_file::gsr::encode(&std::fs::read(&path).unwrap()).unwrap();
            std::fs::write(target.with_extension("gsr"), encoded).unwrap();
            std::fs::remove_file(path).unwrap();
        } else {
            std::fs::rename(path, target).unwrap();
        }
    }
    available.store(true, Ordering::Relaxed);
    assert_eq!(host.images_unavailable(ROUND).unwrap(), None);
    assert_eq!(
        host.burst_frame_path(ROUND, "cam1", 1).unwrap(),
        local.canonicalize().unwrap()
    );

    let pgm = dir.join(frame_rel_path("cam1", groups));
    let encoded = crate::image_file::gsr::encode(&std::fs::read(&pgm).unwrap()).unwrap();
    std::fs::write(pgm.with_extension("gsr"), encoded).unwrap();
    std::fs::remove_file(pgm).unwrap();
    let reason = host.images_unavailable(ROUND).unwrap().unwrap();
    assert!(
        reason.contains(&format!("cam1 第 {groups} 帧组")),
        "{reason}"
    );
}

#[test]
fn failed_round_reads_only_frames_present_in_its_camera_log() {
    let root = root();
    let mut round: CaptureRoundV2 = serde_json::from_value(complete_round(&root)).unwrap();
    round.state = CaptureState::Failed;
    round.error = Some("采集中断".into());
    round.failure = Some(CaptureFailureV2::Interrupted);
    round.group_count = 0;
    round.camera_frames.fill(0);
    for camera in &mut round.cameras {
        camera.received_count = 0;
    }
    let dir = root.join(ROUND);
    std::fs::write(dir.join("round.json"), serde_json::to_vec(&round).unwrap()).unwrap();
    let log = dir.join(FRAMES_FILE);
    let text = std::fs::read_to_string(&log).unwrap();
    let kept: Vec<_> = text
        .lines()
        .rev()
        .filter(|text| {
            let line: FrameLine = serde_json::from_str(text).unwrap();
            (line.camera, line.seq) != (0, 2)
        })
        .collect();
    std::fs::write(&log, kept.join("\n")).unwrap();

    let host = RecipeCaptureHost::open(root, no_recordings()).unwrap();
    assert!(dir.join(frame_rel_path("cam1", 2)).is_file());
    assert!(host.burst_frame_path(ROUND, "cam1", 2).is_err());
    assert_eq!(
        host.burst_frame_path(ROUND, "cam2", 2).unwrap(),
        dir.join(frame_rel_path("cam2", 2)).canonicalize().unwrap()
    );
}

#[test]
fn complete_round_rejects_a_missing_coordinate_that_was_actually_recorded() {
    let root = root();
    let mut round = patched(
        &complete_round(&root),
        json!({"origin": {"kind": "prepared", "label": "缺帧坐标"}}),
    );
    let log = root.join(ROUND).join(FRAMES_FILE);
    let text = std::fs::read_to_string(&log).unwrap();
    let kept: Vec<_> = text
        .lines()
        .filter(|line| {
            let frame: FrameLine = serde_json::from_str(line).unwrap();
            (frame.camera, frame.seq) != (0, 2)
        })
        .collect();
    std::fs::write(&log, kept.join("\n")).unwrap();
    round["cameras"][0]["receivedCount"] = json!(1);
    round["cameraFrames"] = json!([1, 2, 2]);
    round["lossAlarms"] = json!(loss_alarms(&[1, 2, 2]));
    round["missingFrames"] = json!([{"camera": 0, "seq": 2}]);
    round["incompleteGroups"] = json!([2]);
    write_round(&root, &round);
    let host = RecipeCaptureHost::open(root.clone(), no_recordings()).unwrap();
    assert!(host.burst_frame_path(ROUND, "cam2", 2).is_ok());

    round["missingFrames"][0]["seq"] = json!(1);
    round["incompleteGroups"] = json!([1]);
    write_round(&root, &round);
    assert!(host.burst_frame_path(ROUND, "cam2", 2).is_err());
}

#[test]
fn unreadable_frame_log_is_reported_before_an_earlier_invalid_frame() {
    let root = root();
    let round = patched(
        &complete_round(&root),
        json!({"origin": {"kind": "prepared", "label": "日志解码错误"}}),
    );
    write_round(&root, &round);
    let log = root.join(ROUND).join(FRAMES_FILE);
    let existing = std::fs::read_to_string(&log).unwrap();
    let mut first: FrameLine = serde_json::from_str(existing.lines().next().unwrap()).unwrap();
    first.width = 0;
    std::fs::write(
        &log,
        format!("{}\n{{", serde_json::to_string(&first).unwrap()),
    )
    .unwrap();
    let host = RecipeCaptureHost::open(root, no_recordings()).unwrap();
    let error = host.burst_frame_path(ROUND, "cam1", 1).unwrap_err();
    assert!(error.starts_with("采集帧记录无法读取"), "{error}");
}

#[test]
fn frame_read_after_image_cleanup_rejects_the_missing_image() {
    let root = root();
    let round = patched(
        &complete_round(&root),
        json!({"origin": {"kind": "prepared", "label": "手动数据"}}),
    );
    write_round(&root, &round);
    let host = RecipeCaptureHost::open(root.clone(), no_recordings()).unwrap();
    let path = host.burst_frame_path(ROUND, "cam2", 2).unwrap();
    let image = crate::image_file::load(&path).unwrap();
    assert_eq!(image.pixels, vec![2; 4]);
    std::fs::remove_file(path).unwrap();
    assert!(host.burst_frame_path(ROUND, "cam2", 2).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn frame_index_rechecks_equal_length_metadata_edits_after_reading_the_round() {
    for (name, from, to, padding) in [
        ("round.json", "capture-1-0", "capture-9-0", 0),
        (
            FRAMES_FILE,
            r#""frameCounter":102"#,
            r#""frameCounter":103"#,
            0,
        ),
        (
            FRAMES_FILE,
            r#""frameCounter":102"#,
            r#""frameCounter":103"#,
            ROUND_INDEX_CACHE_MAX_BYTES,
        ),
    ] {
        let root = root();
        let round = patched(
            &complete_round(&root),
            json!({"origin": {"kind": "prepared", "label": "元数据变更"}}),
        );
        write_round(&root, &round);
        let file = root.join(ROUND).join(name);
        let mut original = std::fs::read_to_string(&file).unwrap();
        if padding > 0 {
            original.insert_str(original.find('\n').unwrap(), &" ".repeat(padding));
            std::fs::write(&file, &original).unwrap();
        }
        let modified = file.metadata().unwrap().modified().unwrap();
        let host = RecipeCaptureHost::open(root, no_recordings()).unwrap();
        assert!(host.burst_frame_path(ROUND, "cam2", 2).is_ok());

        let changed = original.replacen(from, to, 1);
        assert_ne!(changed, original);
        assert_eq!(changed.len(), original.len());
        std::fs::write(&file, changed).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(file.metadata().unwrap().modified().unwrap(), modified);
        assert!(host.burst_frame_path(ROUND, "cam2", 2).is_err());
    }
}

#[test]
fn thumbnail_range_reads_only_requested_frames_and_rejects_unbounded_requests() {
    let root = root();
    let round = patched(
        &complete_round(&root),
        json!({"origin": {"kind": "prepared", "label": "缩略图范围测试"}}),
    );
    write_round(&root, &round);
    for seq in 1..=GROUPS {
        crate::image_file::save_pgm(
            &root.join(ROUND).join(frame_rel_path("cam1", seq)),
            &FrameImage::new(8, 8, vec![seq as u8; 64]),
        )
        .unwrap();
    }
    let host = RecipeCaptureHost::open(root.clone(), no_recordings()).unwrap();
    for (start, count, expected) in [(1, 1, vec![1]), (2, 32, vec![2])] {
        let mut reads = Vec::new();
        let bytes = host
            .burst_frame_thumbs(ROUND, "cam1", start, count, |path| {
                reads.push(path.to_path_buf());
                crate::image_file::load(path)
            })
            .unwrap();
        let expected_paths: Vec<_> = expected
            .iter()
            .map(|seq| {
                root.join(ROUND)
                    .join(frame_rel_path("cam1", *seq))
                    .canonicalize()
                    .unwrap()
            })
            .collect();
        assert_eq!(reads, expected_paths);
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 1);
        assert_eq!(&bytes[16..], &[1, expected[0] as u8]);
    }
    crate::image_file::save_pgm(
        &root.join(ROUND).join(frame_rel_path("cam1", 1)),
        &FrameImage::new(1601, 1281, vec![7; 1601 * 1281]),
    )
    .unwrap();
    let bytes = host
        .burst_frame_thumbs(ROUND, "cam1", 1, 1, crate::image_file::load)
        .unwrap();
    let width = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let height = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    assert!(width > 0 && width <= 160 && height > 0 && height <= 128);
    assert_eq!(bytes.len(), 17 + (width * height) as usize);
    assert!(bytes[17..].iter().all(|pixel| *pixel == 7));
    for (start, count) in [(0, 1), (1, 0), (1, 33), (GROUPS + 1, 1)] {
        assert!(host
            .burst_frame_thumbs(ROUND, "cam1", start, count, |_| {
                panic!("invalid range must not read an image")
            })
            .is_err());
    }
    std::fs::remove_dir_all(root).unwrap();
}
