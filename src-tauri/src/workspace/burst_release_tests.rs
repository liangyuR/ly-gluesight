use super::burst::CameraTeaching;
use super::burst_release::{
    burst_checks, graph_and_trial_checks, publish_blocker, TeachTrial, TeachingTrialCache,
};
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
        graph_and_trial_checks(&w, &w.doc.build().unwrap(), |_, _| Vec::new()).0
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
        graph_and_trial_checks(&w, &w.doc.build().unwrap(), |_, _| {
            panic!("建图失败时不应进行试测")
        })
        .0
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
fn prepared_teaching_freezes_the_same_graph_as_the_formal_recipe() {
    let mut w = taught(&[Straight, Corner], DEVICE);
    calibrate(&mut w, 30);
    w.doc.stage = RecipeStage::Collect;
    w.doc.segments[0].id = Some("s1".into());
    w.doc.segments[0].heading = Some(crate::plan::HeadingSettings::default());
    for inspect in &mut w.doc.inspect {
        inspect.mm_per_px = Some(0.25);
        for segment in &mut inspect.segments {
            segment.corridor = vec![[80.0, 100.0], [240.0, 100.0]];
        }
    }
    let camera = &mut w.doc.inspect[0];
    for segment in &mut camera.segments {
        segment.color_model = Some(r#"{"version":1}"#.into());
    }
    camera.segments[0].mix_check = Some(crate::recipe::MixCheck {
        model: Some(json!({"datasetKind": "synthetic"})),
        model_id: Some("mix-reference-1".into()),
        camera_serial: Some("cam1-test".into()),
        background_rects: vec![[80, 30, 100, 8]],
        max_length_mm: 2.0,
        max_area_mm2: 4.0,
        drift_length_mm: 5.0,
    });
    for frame in w
        .camera_teaching
        .iter_mut()
        .filter(|frame| frame.camera == "cam1")
    {
        frame.frame.camera_tag = json!({"imageMode":"color"});
    }

    let (checks, prepared) = graph_and_trial_checks(&w, &w.doc.build().unwrap(), |_, _| Vec::new());
    assert!(publish_blocker(&checks).is_none(), "{checks:?}");
    let frozen = prepared.unwrap().frozen_graph();
    assert!(frozen["graphs"][0]["template"] == "corridor_color");
    assert!(frozen["headings"][0]["template"] == "heading_color");
    assert!(frozen["mix"][0]["retryParams"].is_object());

    let formal = formal_release_doc(&w, w.doc.version + 1)
        .unwrap()
        .build()
        .unwrap();
    let rebuilt = crate::production::burst_graphs(&formal, &[[1000, 700]; 3]).unwrap();
    assert_eq!(frozen, rebuilt);
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

#[test]
fn publish_preflight_rechecks_current_teaching_files_after_a_successful_result() {
    let root = super::tests::ImportTestDir::new();
    let mut w = taught(&[Straight, Corner], DEVICE);
    calibrate(&mut w, 30);
    let recipe = w.doc.build().unwrap();
    let pgm = |width, height| {
        let mut bytes = crate::image_file::pnm_header(width, height, 1, None).into_bytes();
        bytes.resize(bytes.len() + (width * height) as usize, 0x19);
        bytes
    };
    let original = pgm(1000, 700);
    let path_of = |frame: &CameraTeaching| root.0.join(format!("{}.pgm", frame.frame.id));
    for frame in &w.camera_teaching {
        std::fs::write(path_of(frame), &original).unwrap();
    }
    let target = w
        .camera_teaching
        .iter()
        .find(|frame| frame.camera == "cam2" && frame.segment == Some(2))
        .unwrap();
    let path = path_of(target);
    let expected = [TeachingTarget {
        camera: "cam2".into(),
        segment: Some(2),
    }];
    let cache = TeachingTrialCache::default();
    let preflight = || {
        burst_checks(&w, Ok(()), || {
            graph_and_trial_checks(&w, &recipe, |_, frames| {
                cache.check(
                    &w,
                    Some("test-engine"),
                    &recipe,
                    frames,
                    |frame| crate::release::read_resource_file(&path_of(frame)),
                    |(_, segment), frame, image| {
                        Some(TeachTrial {
                            segment,
                            camera: frame.camera.clone(),
                            incomplete: false,
                            outcome: if image.pixels.last() == Some(&0x19) {
                                Ok(())
                            } else {
                                Err("试测读取了替换后的图像".into())
                            },
                        })
                    },
                )
            })
            .0
        })
    };
    let valid = preflight();
    assert!(publish_blocker(&valid).is_none());
    assert!(check(&valid, "示教帧试测").passed);

    for (contents, reason) in [
        (None, None),
        (Some(b"P5\n1000 700\n255\n".to_vec()), Some("像素数据不足")),
        (Some(pgm(999, 700)), Some("与冻结时")),
    ] {
        if let Some(bytes) = contents {
            std::fs::write(&path, bytes).unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        let checks = preflight();
        let blocker = publish_blocker(&checks).unwrap();
        assert_eq!(blocker.name, "示教图像");
        assert_eq!(blocker.teaching_targets, expected);
        if let Some(reason) = reason {
            assert!(blocker.detail.contains(reason), "{blocker:?}");
        }
    }

    std::fs::write(&path, &original).unwrap();
    assert!(publish_blocker(&preflight()).is_none());
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut changed = original;
    *changed.last_mut().unwrap() = 0x31;
    std::fs::write(&path, &changed).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let checks = preflight();
    assert!(check(&checks, "示教图像").passed);
    assert!(check(&checks, "示教帧试测")
        .detail
        .contains("试测读取了替换后的图像"));
}

#[test]
fn late_publish_preflight_preserves_a_newer_successful_trial() {
    fn preflight(
        cache: &TeachingTrialCache,
        w: &Workspace,
        read: impl FnMut(&CameraTeaching) -> Result<Vec<u8>, String>,
        measure: impl FnMut((usize, u32), &CameraTeaching, FrameImage) -> Option<TeachTrial>,
    ) -> Vec<Check> {
        let recipe = w.doc.build().unwrap();
        graph_and_trial_checks(w, &recipe, |_, frames| {
            cache.check(w, Some("test-engine"), &recipe, frames, read, measure)
        })
        .0
    }

    let root = super::tests::ImportTestDir::new();
    let mut old = taught(&[Straight], DEVICE);
    calibrate(&mut old, 15);
    let mut current = old.clone();
    let path_of = |frame: &CameraTeaching| root.0.join(format!("{}.pgm", frame.frame.id));
    let mut pgm = crate::image_file::pnm_header(1000, 700, 1, None).into_bytes();
    pgm.resize(pgm.len() + 1000 * 700, 0x19);
    for frame in &old.camera_teaching {
        std::fs::write(path_of(frame), &pgm).unwrap();
    }
    let read = |frame: &CameraTeaching| crate::release::read_resource_file(&path_of(frame));
    let measure = |(_, segment), frame: &CameraTeaching, image: FrameImage| {
        assert_eq!(image.pixels.last(), Some(&0x19));
        Some(TeachTrial {
            segment,
            camera: frame.camera.clone(),
            outcome: Ok(()),
            incomplete: false,
        })
    };

    for (previous, latest) in [((1, 1), (1, 2)), ((1, 10), (2, 1))] {
        (old.instance_id, old.revision) = previous;
        (current.instance_id, current.revision) = latest;
        let cache = TeachingTrialCache::default();
        let mut start_current = true;
        let old_checks = preflight(
            &cache,
            &old,
            |frame| {
                if std::mem::take(&mut start_current) {
                    let result = preflight(&cache, &current, read, measure);
                    assert!(publish_blocker(&result).is_none(), "{latest:?}: {result:?}");
                    assert!(check(&result, "示教帧试测").passed);
                }
                read(frame)
            },
            measure,
        );
        assert!(check(&old_checks, "示教帧试测").passed);

        let mut cameras_read = Vec::new();
        let result = preflight(
            &cache,
            &current,
            |frame| {
                cameras_read.push(frame.camera.clone());
                read(frame)
            },
            |_, frame, _| {
                panic!(
                    "{previous:?} → {latest:?}: {} 试测结果被旧预检覆盖",
                    frame.camera
                )
            },
        );
        assert_eq!(cameras_read, current.doc.plan_cameras());
        assert!(publish_blocker(&result).is_none(), "{latest:?}: {result:?}");
        assert!(check(&result, "示教帧试测").passed);
    }
}
