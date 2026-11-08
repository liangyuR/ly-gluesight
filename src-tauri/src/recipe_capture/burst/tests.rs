use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

use super::*;

static NEXT_TEST: AtomicU64 = AtomicU64::new(1);
const GROUPS: u64 = 2;
const ROUND: &str = "capture-1-0";

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!("gluesight-burst-{}-{}-{}", std::process::id(), ly_plc::now_ms(), NEXT_TEST.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// 盘上一轮完整轮次：cam1–cam3 各 GROUPS 帧（像素值为帧组序号），写好图像与 frames.jsonl；返回 round.json 内容供用例改写后写入。
fn complete_round(root: &Path) -> Value {
    let dir = root.join(ROUND);
    let mut lines = String::new();
    let mut saved = vec![BTreeSet::new(); 3];
    let cameras = (0..3usize)
        .map(|camera| {
            let camera_id = format!("cam{}", camera + 1);
            std::fs::create_dir_all(dir.join(&camera_id)).unwrap();
            for seq in 1..=GROUPS {
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
                device_session: 10 + camera as u64,
                received_count: 0,
            }
        })
        .collect();
    std::fs::write(dir.join(FRAMES_FILE), lines).unwrap();
    let mut round = CaptureRoundV2 {
        format: FORMAT,
        round_id: ROUND.into(),
        recipe_id: "recipe-a".into(),
        cameras,
        state: CaptureState::Complete,
        plc_shot_count: None,
        plc_plan_version: None,
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
    apply(&mut round, summarize(GROUPS, &saved));
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

/// 应用退出时仍在接收的轮次：重启后判中断，统计按 frames.jsonl 恢复。
#[test]
fn round_left_receiving_on_disk_then_reopen_marks_it_interrupted_with_counts_from_frames() {
    let root = root();
    let round = patched(
        &complete_round(&root),
        json!({"format": 2, "state": "receiving", "teachingRoundId": 12, "endedAt": null, "groupCount": 0, "cameraFrames": [0, 0, 0], "incompleteGroups": [], "missingFrames": [], "lossAlarms": []}),
    );
    write_round(&root, &round);
    let host = RecipeCaptureHost::open(root).unwrap();
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
        let reopened = RecipeCaptureHost::open(root.clone()).unwrap();
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
        ("图像路径指到轮次目录外", |_, lines, at| { let i = at(lines, 0, 1); lines[i]["path"] = "../../outside.pgm".into(); }),
        ("帧记录引用本轮之外的相机", |_, lines, at| { let i = at(lines, 0, 2); lines[i]["camera"] = 7.into(); }),
        ("重复的帧记录", |_, lines, at| { let i = at(lines, 1, 1); lines.push(lines[i].clone()); }),
        ("删掉一帧但仍称完整", |_, lines, at| { let i = at(lines, 2, 2); lines.remove(i); }),
        ("帧计数与帧组序号对不上", |_, lines, at| { let i = at(lines, 0, 2); lines[i]["frameCounter"] = 7.into(); }),
        ("多报一台相机的帧数", |round, _, _| { round["cameraFrames"][0] = 3.into(); round["cameras"][0]["receivedCount"] = 3.into(); }),
        ("记录换成别的轮次身份", |round, _, _| { round["roundId"] = "capture-other".into(); }),
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
        assert!(got.is_err() && image.is_err(), "{case}：{got:?} / {image:?}");
    }
    let host = RecipeCaptureHost::open(root.clone()).unwrap();
    assert!(host.get_any("capture-../../outside").is_err());
}

/// 导出时没带示教件原图的轮次：只用帧记录逐行相同的轮次补回，补回后原图可读、来源轮次删除；帧记录不同时拒绝，不移入任何原图。
#[test]
fn round_without_images_is_refilled_only_from_a_round_with_identical_frames() {
    let root = root();
    let round = patched(
        &complete_round(&root),
        json!({"origin": {"kind": "prepared", "label": "手动数据"}}),
    );
    write_round(&root, &round);
    let cameras = ["cam1", "cam2", "cam3"];
    let copy_round = |id: &str, ts: i64| {
        let dir = root.join(id);
        for camera in cameras {
            std::fs::create_dir_all(dir.join(camera)).unwrap();
            for seq in 1..=GROUPS {
                let path = frame_rel_path(camera, seq);
                std::fs::copy(root.join(ROUND).join(&path), dir.join(&path)).unwrap();
            }
        }
        let lines = std::fs::read_to_string(root.join(ROUND).join(FRAMES_FILE))
            .unwrap()
            .replace("\"ts\":0", &format!("\"ts\":{ts}"));
        std::fs::write(dir.join(FRAMES_FILE), lines).unwrap();
        std::fs::write(
            dir.join("round.json"),
            patched(&round, json!({"roundId": id})).to_string(),
        )
        .unwrap();
    };
    copy_round("capture-2-0", 0);
    copy_round("capture-3-0", 5);
    for camera in cameras {
        std::fs::remove_dir_all(root.join(ROUND).join(camera)).unwrap();
    }
    let host = RecipeCaptureHost::open(root.clone()).unwrap();
    assert!(host.images_missing(ROUND).unwrap());
    assert!(host.refill_images(ROUND, "capture-3-0").is_err());
    assert!(host.images_missing(ROUND).unwrap(), "帧记录不同不应补入");
    host.refill_images(ROUND, "capture-2-0").unwrap();
    assert!(!host.images_missing(ROUND).unwrap());
    let image = crate::image_file::load(&host.burst_frame_path(ROUND, "cam2", 2).unwrap()).unwrap();
    assert_eq!(image.pixels, vec![2u8; 4]);
    assert!(!root.join("capture-2-0").exists() && !root.join("capture-3-0").exists());
}

#[test]
fn frame_read_after_image_cleanup_rejects_the_missing_image() {
    let root = root();
    let round = patched(
        &complete_round(&root),
        json!({"origin": {"kind": "prepared", "label": "手动数据"}}),
    );
    write_round(&root, &round);
    let host = RecipeCaptureHost::open(root.clone()).unwrap();
    let path = host.burst_frame_path(ROUND, "cam2", 2).unwrap();
    let image = crate::image_file::load(&path).unwrap();
    assert_eq!(image.pixels, vec![2; 4]);
    std::fs::remove_file(path).unwrap();
    assert!(host.burst_frame_path(ROUND, "cam2", 2).is_err());
    std::fs::remove_dir_all(root).unwrap();
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
    let host = RecipeCaptureHost::open(root.clone()).unwrap();
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
