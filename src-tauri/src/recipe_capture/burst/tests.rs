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

/// 示教握手删除后，盘上已有的示教轮次（format 2）与历史件导入轮次（format 3）照常读入，不被改名成 round.broken。
#[test]
fn legacy_teaching_or_history_round_on_disk_then_loads_complete_and_keeps_its_record() {
    let cases = [
        (
            "示教 V1 轮次",
            json!({"format": 2, "teachingRoundId": 12, "simulated": true}),
        ),
        (
            "PLC 开始的示教实拍轮次",
            json!({"format": 2, "plcShotCount": 1500, "plcPlanVersion": 7, "deviceTriggers": 2, "simulated": false}),
        ),
        (
            "历史件导入轮次",
            json!({"format": 3, "origin": {"kind": "history", "cycleId": "abc_123", "recordId": 5, "standard": true}}),
        ),
    ];
    for (case, patch) in cases {
        let root = root();
        let round = patched(&complete_round(&root), patch);
        write_round(&root, &round);
        let host = RecipeCaptureHost::open(root.clone()).unwrap();
        assert!(
            root.join(ROUND).join("round.json").is_file(),
            "{case}：记录被改名"
        );
        let AnyRound::Burst(loaded) = host
            .get_any(ROUND)
            .unwrap_or_else(|e| panic!("{case}：{e}"));
        assert_eq!(loaded.state, CaptureState::Complete, "{case}");
        assert_eq!(host.list("recipe-a").unwrap().len(), 1, "{case}");
        assert!(
            host.burst_frame_path(ROUND, "cam3", GROUPS).is_ok(),
            "{case}"
        );
    }
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

/// 手动配置数据与历史件导入的轮次同一规则：只收已完成、不带 PLC 字段与示教握手编号的记录。
#[test]
fn prepared_or_history_round_then_only_complete_without_plc_fields_is_accepted() {
    let prepared = json!({"kind": "prepared", "label": "E2E 手动数据"});
    let history = json!({"kind": "history", "cycleId": "abc_123", "recordId": 5});
    let failed = json!({"state": "failed", "error": "中断", "failure": {"kind": "interrupted"}});
    let cases = [
        ("手动配置数据，已完成", prepared.clone(), json!({}), true),
        ("历史件，已完成", history.clone(), json!({}), true),
        (
            "手动配置数据带 PLC 计数",
            prepared.clone(),
            json!({"plcShotCount": 3, "plcPlanVersion": 1}),
            false,
        ),
        (
            "手动配置数据带设备触发数",
            prepared.clone(),
            json!({"deviceTriggers": 2}),
            false,
        ),
        (
            "手动配置数据带示教握手编号",
            prepared.clone(),
            json!({"teachingRoundId": 12}),
            false,
        ),
        (
            "手动配置数据未完成",
            prepared.clone(),
            failed.clone(),
            false,
        ),
        (
            "手动配置数据没有标签",
            json!({"kind": "prepared", "label": " "}),
            json!({}),
            false,
        ),
        (
            "历史件带 PLC 计数",
            history.clone(),
            json!({"plcShotCount": 3, "plcPlanVersion": 1}),
            false,
        ),
        ("历史件未完成", history, failed, false),
    ];
    for (case, origin, patch, accepted) in cases {
        let root = root();
        let mut round = patched(&complete_round(&root), patch);
        round["origin"] = origin;
        write_round(&root, &round);
        let host = RecipeCaptureHost::open(root).unwrap();
        assert_eq!(host.get_any(ROUND).is_ok(), accepted, "{case}");
    }
}
