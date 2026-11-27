use super::burst::CameraTeaching;
use super::burst_release::{burst_checks, graph_and_trial_checks, publish_blocker};
use super::burst_tests::{burst_workspace, DEVICE};
use super::*;
use crate::plan::{
    PartCalib, SegmentCalib,
    SegmentKind::{self, Corner, Straight},
};

/// 每段每台检测相机都选了示教帧（1000×700，采集时设备标签为 device）、画了样例走廊（落在帧内），相机有当量。
fn taught(kinds: &[SegmentKind], device: &str) -> Workspace {
    let mut w = burst_workspace(kinds);
    let corridor = crate::recipe::samples_burst().inspect[0].segments[0]
        .corridor
        .clone();
    for inspect in &mut w.doc.inspect {
        inspect.segments = (1..=kinds.len() as u32)
            .map(|segment| crate::recipe::SegmentCorridor {
                segment,
                corridor: corridor.clone(),
                sector_deg: None,
                mix_check: None,
                color_model: None,
                measure: None,
            })
            .collect();
    }
    for (camera, segment) in w
        .doc
        .plan_cameras()
        .into_iter()
        .flat_map(|c| (1..=kinds.len() as u32).map(move |s| (c.clone(), s)))
    {
        w.camera_teaching.push(CameraTeaching {
            camera: camera.clone(),
            round: "capture-teach".into(),
            seq: 1,
            frame: FrozenImage {
                mix_capture: None,
                id: format!("corridor-{camera}-{segment}"),
                view: 1,
                source: "capture:capture-teach#1".into(),
                captured_at: 1,
                size: [1000, 700],
                camera: camera.clone(),
                camera_tag: json!(device),
                calib_tag: Value::Null,
                geometry_tag: Value::Null,
                exposure_us: None,
                gain_db: None,
                history_id: None,
            },
            segment: Some(segment),
        });
    }
    w.teaching_piece = Some(crate::workspace::pieces_tests::piece("capture-teach", 1));
    w
}

/// 整件标定 ΣN_exp = total，并逐段写 N_exp（均分，余数给最后一段；与应用标定的结果同形）。
fn calibrate(w: &mut Workspace, total: u32) {
    let calib = SegmentCalib {
        rounds: 10,
        min: total - 1,
        max: total + 1,
        mean: f64::from(total),
    };
    w.doc.part_calib = Some(PartCalib {
        expected_total: total,
        calib: calib.clone(),
    });
    let n = w.doc.segments.len() as u32;
    for (i, segment) in w.doc.segments.iter_mut().enumerate() {
        let frames = if i as u32 + 1 == n {
            total - total / n * (n - 1)
        } else {
            total / n
        };
        (segment.expected_frames, segment.calib) = (
            Some(frames),
            Some(SegmentCalib {
                mean: f64::from(frames),
                min: frames - 1,
                max: frames + 1,
                ..calib.clone()
            }),
        );
    }
}

/// 设备就绪：只剩候选自身的检查项决定能否发布。
fn checks(w: &Workspace) -> Vec<Check> {
    burst_checks(w, Ok(()), Vec::new)
}

fn check<'a>(checks: &'a [Check], name: &str) -> &'a Check {
    checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("缺少验证项 {name}：{checks:?}"))
}

#[test]
fn publish_blocked_when_plan_cameras_not_three_or_corridor_missing() {
    // 防的回归（规格 §1 Plan.cameras = 3；§4 每台相机的检测走廊；D-SB-46）：少一台计划相机、某台没选示教帧、走廊超出示教帧、
    // 缺像素当量，仍能通过发布前检查；或都满足了却不能发布
    type Edit = fn(&mut Workspace);
    let cases: [(&str, Edit, bool); 5] = [
        ("三台相机都示教好", |_| {}, true),
        (
            "计划相机只有 2 台",
            |w| {
                w.doc.inspect.truncate(2);
                w.camera_teaching.retain(|t| t.camera != "cam3");
            },
            false,
        ),
        (
            "一台相机没选示教帧",
            |w| w.camera_teaching.retain(|t| t.camera != "cam2"),
            false,
        ),
        (
            "走廊超出示教帧",
            |w| w.doc.inspect[0].segments[1].corridor[0] = [1200.0, 480.0],
            false,
        ),
        ("缺像素当量", |w| w.doc.inspect[1].mm_per_px = None, false),
    ];
    for (name, edit, passed) in cases {
        let mut w = taught(&[Straight, Corner], DEVICE);
        calibrate(&mut w, 30);
        edit(&mut w);
        let checks = checks(&w);
        assert_eq!(
            (
                check(&checks, "走廊示教").passed,
                checks.iter().all(|c| c.passed)
            ),
            (passed, passed),
            "{name}：{checks:?}"
        );
    }
}

#[test]
fn publish_checks_report_graph_errors_for_each_affected_camera_before_measuring() {
    let mut w = taught(&[Straight, Corner], DEVICE);
    calibrate(&mut w, 30);
    let valid = burst_checks(&w, Ok(()), || {
        graph_and_trial_checks(&w, &w.doc.build().unwrap(), |_| None)
    });
    assert!(check(&valid, "测量图").passed);

    w.doc.segments[0].heading = Some(crate::plan::HeadingSettings::default());
    w.doc.inspect[0].nozzle = Some([999.000001, 480.0]);
    w.doc.segments[0].offset_ref = Some(crate::plan::OffsetRef {
        measure: crate::plan::OffsetMeasure::ToEdge,
        camera: "cam2".into(),
        x_ref: None,
        spread: None,
        edge: Some(crate::plan::EdgeParams {
            polarity: crate::plan::EdgePolarity::BrightToDark,
            search_mm: 1.0,
            contrast_min: 40.0,
            nominal_mm: None,
        }),
    });
    w.doc.inspect[2].segments[0].measure = Some(crate::recipe::MeasureMode::Color);
    let checks = burst_checks(&w, Ok(()), || {
        graph_and_trial_checks(&w, &w.doc.build().unwrap(), |_| {
            panic!("建图失败时不应进行试测")
        })
    });

    let blocker = publish_blocker(&checks).unwrap();
    assert_eq!(blocker.name, "测量图");
    assert_eq!(
        blocker.teaching_targets,
        ["cam1", "cam2", "cam3"].map(|camera| TeachingTarget {
            segment: Some(1),
            camera: camera.into(),
        })
    );
    for (camera, reason) in [
        ("cam1", "出胶点"),
        ("cam2", "找边搜索长度"),
        ("cam3", "还没取色"),
    ] {
        assert!(
            blocker.detail.split('；').any(|detail| {
                detail.starts_with(&format!("第 1 段相机 {camera}：")) && detail.contains(reason)
            }),
            "{blocker:?}"
        );
    }
}

#[test]
fn archived_release_without_teaching_targets_remains_readable() {
    let root = super::tests::ImportTestDir::new();
    let doc = crate::recipe::samples_burst();
    let path = root.0.join(format!("{}-v{}.json", doc.id, doc.version));
    let old = json!({
        "doc": doc, "bundleId": "original-bundle", "revision": 7, "validationSite": "real",
        "validation": {
            "revision": 7, "passed": true, "checkedAt": 1, "environmentTag": null,
            "checks": [{"name": "走廊示教", "passed": true, "detail": "示教已完成"}]
        }
    });
    std::fs::write(path, old.to_string()).unwrap();

    let release = archived_release(&root.0, &doc.id, doc.version).unwrap();
    assert_eq!(release.bundle_id, "original-bundle");
    let corridor = check(&release.validation.checks, "走廊示教");
    assert!(corridor.passed && corridor.teaching_targets.is_empty());
}
