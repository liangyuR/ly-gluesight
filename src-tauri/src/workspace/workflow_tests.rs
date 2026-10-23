use super::*;
use super::workflow::{initialize_views, load_active, projected_frame, selected_views, store_active};

#[test]
fn representative_calibration_reuse_requires_matching_environment_and_retains_source_identity() {
    let w = workspace();
    let mut image = w.frames[0].views[0].clone();
    image.camera_tag = json!({"id":"device1","session":2});
    image.calib_tag = json!({"path":"device1-view1","value":{"data":{"mmPerPx":0.1}}});
    let check = CalibrationCheck { verification_id:"verification-a".into(), source_k:7, source_image_id:"capture-a-v1".into(),
        basis: workflow::CalibrationBasis::Manual { reference_mm:10.0, measured_mm:10.05, error_mm:0.05 }, passed:true,
        camera_tag:image.camera_tag.clone(), calib_tag:image.calib_tag.clone(), mm_per_px:Some(0.1) };
    assert!(workflow::reusable_calibration(&check, &image.camera_tag, &image.calib_tag, Some(0.1), &image));
    assert!(!workflow::reusable_calibration(&check, &json!({"id":"device2"}), &image.calib_tag, Some(0.1), &image));
    assert!(!workflow::reusable_calibration(&check, &image.camera_tag, &json!({"path":"device1-view2"}), Some(0.1), &image));
    assert!(!workflow::reusable_calibration(&check, &image.camera_tag, &image.calib_tag, Some(0.2), &image));
    let mut stale = image.clone(); stale.calib_tag = Value::Null;
    assert!(!workflow::reusable_calibration(&check, &image.camera_tag, &image.calib_tag, Some(0.1), &stale));
    let mut failed = check.clone(); failed.passed = false;
    assert!(!workflow::reusable_calibration(&failed, &image.camera_tag, &image.calib_tag, Some(0.1), &image));
}

#[test]
fn calibration_reuse_brings_scale_to_unset_same_camera_shots_and_keeps_differing_ones() {
    let mut doc = crate::recipe::samples().remove(1);
    let base = doc.shots[0].clone();
    doc.shots = (0..4).map(|k| {
        let mut shot = base.clone();
        shot.id = format!("P{k}"); shot.views.clear(); shot.view = 1; shot.path = Vec::new(); shot.mm_per_px = None; shot.skip = false;
        if k == 3 { shot.camera = format!("{}-other", base.camera); }
        shot
    }).collect();
    doc.shots[0].mm_per_px = Some(0.1);
    doc.shots[2].mm_per_px = Some(0.2);
    if let Some(imaging) = doc.cameras.get(&base.camera).cloned() { doc.cameras.insert(doc.shots[3].camera.clone(), imaging); }
    let mut w = Workspace::new(doc, None);
    for k in 0..4 {
        let camera = w.doc.shots[k].camera.clone();
        w.frames[k].views = vec![serde_json::from_value(json!({
            "id":format!("capture-{k}-v1"), "view":1, "source":"capture:test", "capturedAt":1, "size":[300,100],
            "camera":camera, "cameraTag":{"id":camera}, "calibTag":{"calib":camera}, "geometryTag":{}
        })).unwrap()];
        w.frames[k].image = Some(w.frames[k].views[0].clone());
        initialize_views(&mut w, k);
    }
    let camera = w.doc.shots[0].camera.clone();
    let check = CalibrationCheck { verification_id:"check-1".into(), source_k:0, source_image_id:"capture-0-v1".into(),
        basis: workflow::CalibrationBasis::Manual { reference_mm:10.0, measured_mm:10.05, error_mm:0.05 }, passed:true,
        camera_tag:json!({"id":camera}), calib_tag:json!({"calib":camera}), mm_per_px:Some(0.1) };
    let tags = |recipe: &Recipe, k: usize| Ok((json!({"id":recipe.shots[k].camera}), json!({"calib":recipe.shots[k].camera})));
    assert_eq!(workflow::reuse_calibration(&mut w, 0, 1, &check, tags).unwrap(), 1);
    assert_eq!(w.doc.shots[1].mm_per_px, Some(0.1));
    assert_eq!(w.doc.shots[1].views[0].mm_per_px, Some(0.1));
    assert_eq!(w.frames[1].view_states[0].calibration_check.as_ref().unwrap().verification_id, "check-1");
    assert_eq!(w.doc.shots[2].mm_per_px, Some(0.2));
    assert!(w.frames[2].view_states.iter().all(|s| s.calibration_check.is_none()));
    assert_eq!(w.doc.shots[3].mm_per_px, None);
    assert!(w.frames[3].view_states.iter().all(|s| s.calibration_check.is_none()));
    let mut failed = check.clone(); failed.passed = false;
    w.doc.shots[1].mm_per_px = None; w.doc.shots[1].views[0].mm_per_px = None;
    assert_eq!(workflow::reuse_calibration(&mut w, 0, 1, &failed, tags).unwrap(), 0);
    assert_eq!(w.doc.shots[1].mm_per_px, None);
}

fn snapshot(w: &Workspace, selected: u8) -> Value {
    let frame = projected_frame(w, 0, selected).unwrap();
    json!({"image":frame.image,"trial":frame.trial,"saved":frame.saved})
}

fn pending_workspace() -> Workspace {
    let mut w = workspace();
    for view in &mut w.doc.shots[0].views { view.enabled = view.view == 1; }
    w.pending = Some(Box::new(Release { doc: w.doc.clone(), bundle_id: "frozen-release-17".into(),
        base_revision: w.base_revision.clone(), revision: w.revision, frames: w.frames.clone(),
        overview: w.overview.clone(), validation: w.validation.clone().unwrap(), validation_site: ValidationSite::Real }));
    w
}

#[test]
fn only_failed_activation_can_be_abandoned_and_returns_to_draft() {
    for (publish, abandonable) in [
        (PublishState::Failed { message: "发布包缺失".into() }, true),
        (PublishState::Uncertain { message: "写盘失败".into() }, false),
        (PublishState::Waiting, false),
    ] {
        let mut w = pending_workspace();
        w.publish = publish.clone();
        let before = state_value(&w);
        assert!(w.abandon_publish(w.revision + 1).is_err());
        assert_eq!(w.abandon_publish(w.revision).is_ok(), abandonable, "{publish:?}");
        if abandonable {
            assert!(w.pending.is_none());
            assert_eq!(w.publish, PublishState::Draft);
            assert_eq!(w.base_revision, serde_json::from_value::<Workspace>(before).unwrap().base_revision);
            assert!(w.expect(w.revision).is_ok());
        } else {
            assert_eq!(state_value(&w), before, "{publish:?}");
        }
    }
}

#[test]
fn historical_sample_requires_the_teaching_plc_plan_even_with_identical_shot_fields() {
    assert!(workflow::require_capture_plan(None).is_err());
    assert!(workflow::require_capture_plan(Some(0)).is_err());
    assert_eq!(workflow::require_capture_plan(Some(17)).unwrap(), 17);
    let mut doc = workspace().doc;
    doc.plan_version = 17;
    let original = doc.build().unwrap();
    doc.version += 1;
    doc.shots[0].views[0].path[0][0] += 3.0;
    let candidate = doc.build().unwrap();
    assert!(same_history_capture_source(&original, &candidate, Some(17)));
    assert!(!same_history_capture_source(&original, &candidate, Some(18)));
    assert!(!same_history_capture_source(&original, &candidate, None));
    let mut missing = original.clone(); missing.plan_version = 0;
    assert!(!same_history_capture_source(&missing, &candidate, Some(0)));
    let mut other_camera = candidate; other_camera.shots[0].camera = "another-device".into();
    assert!(!same_history_capture_source(&original, &other_camera, Some(17)));
}

#[test]
fn whole_round_candidate_refuses_replacing_a_single_frame() {
    let mut w = pending_workspace();
    assert!(w.require_single_frame_source().is_ok());
    w.capture_id = Some("whole-round".into());
    assert!(w.require_single_frame_source().is_err());
}

/// 生效前的候选（带冻结的待生效版本）与生效写盘失败后保留下来的候选。
fn failed_activation() -> (Workspace, Workspace) {
    let before = pending_workspace();
    let mut after = before.clone();
    after.pending = None;
    after.doc.version += 1;
    after.base_revision = Some("already-active-v2".into());
    after.changed();
    after.retain_failed_activation(before.clone(), "disk write denied");
    (before, after)
}

#[test]
fn failed_activation_retains_the_same_frozen_release_and_blocks_editing() {
    let (before, after) = failed_activation();
    assert!(after.activation_uncertain());
    assert_eq!(state_value(&after.pending), state_value(&before.pending));
    assert_eq!(after.doc, before.doc);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.base_revision, before.base_revision);
    assert!(after.expect(after.revision).is_err());
}

#[test]
fn retrying_a_failed_activation_with_a_stale_revision_is_rejected_without_changing_state() {
    let (_, mut after) = failed_activation();
    let failed = state_value(&after);
    assert!(after.retry_publish(after.revision + 1).is_err());
    assert_eq!(state_value(&after), failed);
}

#[test]
fn restored_failed_activation_retries_the_same_frozen_release_only_once() {
    let (before, after) = failed_activation();
    let mut restored: Workspace = serde_json::from_value(state_value(&after)).unwrap();
    assert!(restored.activation_uncertain());
    restored.retry_publish(restored.revision).unwrap();
    assert!(!restored.activation_uncertain());
    assert_eq!(state_value(&restored.pending), state_value(&before.pending));
    assert_eq!(restored.revision, before.revision);
    assert!(restored.expect(restored.revision).is_err());
    assert!(restored.retry_publish(restored.revision).is_err());
}

pub(super) fn workspace() -> Workspace {
    let mut doc = crate::recipe::samples().remove(1);
    doc.shots.truncate(1);
    doc.shots[0].views.clear();
    doc.shots[0].view = 1;
    let mut w = Workspace::new(doc, None);
    w.frames[0].views = (1..=3).map(|v| serde_json::from_value(json!({
        "id":format!("capture-test-v{v}"), "view":v, "source":"capture:test", "capturedAt":1,
        "size":[300,100], "camera":w.doc.shots[0].camera, "cameraTag":{}, "calibTag":{}, "geometryTag":{}
    })).unwrap()).collect();
    w.frames[0].image = Some(w.frames[0].views[0].clone());
    initialize_views(&mut w, 0);
    for v in &mut w.doc.shots[0].views {
        v.enabled = true;
        v.path = vec![[10.0, 10.0 * v.view as f32], [210.0, 10.0 * v.view as f32]];
        v.mm_per_px = Some(0.1);
        v.detect = None;
        v.limits = None;
    }
    w.doc.shots[0].views[1].detect = Some(w.doc.detect.clone());
    w.doc.shots[0].views[2].limits = Some(w.doc.limits.clone());
    for selected in 1..=3 {
        load_active(&mut w, 0, selected).unwrap();
        let r = w.doc.build().unwrap().for_view(0, selected).unwrap();
        let (geometry, params) = shot_tags(&r, 0).unwrap();
        let image_id = w.frames[0].image.as_ref().unwrap().id.clone();
        w.frames[0].views.iter_mut().find(|i| i.view == selected).unwrap().geometry_tag = geometry.clone();
        w.frames[0].image.as_mut().unwrap().geometry_tag = geometry.clone();
        w.frames[0].trial = Some(serde_json::from_value(json!({
            "imageId":image_id,"engineTag":"engine-1","paramsTag":params,"geometryTag":geometry,
            "passed":true,"score":0.8,"coverage":1.0,"elapsedMs":1,"reason":"有效试测",
            "measurement":{"verdict":Verdict::NgGap},"verdict":Verdict::NgGap
        })).unwrap());
        w.frames[0].saved = true;
        store_active(&mut w, 0);
    }
    load_active(&mut w, 0, 1).unwrap();
    w.validation = Some(Validation { revision: w.revision, passed: true, checked_at: 1, checks: Vec::new(), samples: Vec::new(), environment_tag: json!({}), fixture: Vec::new() });
    w
}

fn fixture_spec() -> crate::recipe::FixtureSpec {
    // 中线是 y = 10–30 的水平线、搜索半宽 150 px：框（含外圈）要在两端切向延长的走廊之外，只能放到 y > 184
    crate::recipe::FixtureSpec { rect: [100, 200, 32, 32], source: "capture-test-v1".into(), search_radius_px: 20.0, angle_range_deg: 0.0, min_score: 0.6, max_peak_ratio: 1.0, max_end_shift_mm: None, end_margin_mm: 0.0 }
}

#[test]
fn fixture_source_is_owned_by_the_backend_and_only_saving_params_rebinds_it() {
    // 防的回归：模板身份是 (来源图, 框, 外圈)。前端能改写来源、草稿或整份保存把没动过的旧框悄悄绑到重新冻结的新图上，
    // 或重新冻结后旧框照样裁新图试测——测量定义说是同一模板，像素却变了
    fn fresh() -> Workspace {
        let mut w = workspace();
        for image in &mut w.frames[0].views { image.size = [600, 300]; }
        w.frames[0].image = Some(w.frames[0].views[0].clone());
        w.doc.shots[0].fixture = Some(fixture_spec());
        store_active(&mut w, 0);
        // 重新冻结图 1：旧框的来源还是旧图
        w.frames[0].views[0].id = "capture-retake-v1".into();
        w.frames[0].image = Some(w.frames[0].views[0].clone());
        w
    }
    let stale = fresh();
    assert!(fixture_source(&stale.frames[0], 1, stale.doc.shots[0].fixture.as_ref().unwrap()).is_err(), "旧框不能裁重新冻结的图");
    fn params(w: &mut Workspace, f: crate::recipe::FixtureSpec) -> Result<(), String> {
        let shot = &w.doc.shots[0];
        let teach = ShotTeach { path: shot.path.clone(), mm_per_px: shot.mm_per_px.unwrap(), detect: shot.detect.clone(), limits: shot.limits.clone(), fixture: Some(f), rotation: None };
        apply_params(w, 0, teach)
    }
    fn draft(w: &mut Workspace, f: crate::recipe::FixtureSpec) -> Result<(), String> {
        let shot = &w.doc.shots[0];
        let teach = workflow::DraftTeach { path: shot.path.clone(), mm_per_px: shot.mm_per_px, detect: shot.detect.clone(), limits: shot.limits.clone(), fixture: Some(f) };
        workflow::apply_draft(w, 0, teach)
    }
    fn whole(w: &mut Workspace, f: crate::recipe::FixtureSpec) -> Result<(), String> {
        let mut doc = w.doc.clone();
        doc.shots[0].fixture = Some(f.clone());
        doc.shots[0].views[0].fixture = Some(f);
        update_doc(w, doc)
    }
    type Entry = fn(&mut Workspace, crate::recipe::FixtureSpec) -> Result<(), String>;
    // （入口，框是否改动，保存后的来源）；前端一律传伪造的来源
    let cases: [(&str, Entry, bool, &str); 5] = [
        ("保存示教参数、框不变 → 改绑当前冻结图", params, false, "capture-retake-v1"),
        ("草稿自动保存、框不变 → 沿用旧来源", draft, false, "capture-test-v1"),
        ("整份保存、框不变 → 沿用旧来源", whole, false, "capture-test-v1"),
        ("草稿自动保存、改了框 → 绑当前冻结图", draft, true, "capture-retake-v1"),
        ("整份保存、改了框 → 绑当前冻结图", whole, true, "capture-retake-v1"),
    ];
    for (name, entry, moved, want) in cases {
        let mut w = fresh();
        let mut f = w.doc.shots[0].fixture.clone().unwrap();
        if moved { f.rect[0] += 8; }
        f.source = "forged-by-frontend".into();
        entry(&mut w, f).unwrap_or_else(|e| panic!("{name}：{e}"));
        let shot = &w.doc.shots[0];
        assert_eq!(shot.fixture.as_ref().map(|f| f.source.as_str()), Some(want), "{name}");
        assert_eq!(shot.views[0].fixture.as_ref().map(|f| f.source.as_str()), Some(want), "{name}");
        assert_eq!(fixture_source(&w.frames[0], 1, shot.fixture.as_ref().unwrap()).is_ok(), want == "capture-retake-v1", "{name}");
    }
}

#[test]
fn switching_views_keeps_independent_centerlines_and_valid_ng_trials() {
    let mut w = workspace();
    // 图 1 的特征定位在切换视图来回之后仍在，不串到图 2
    w.doc.shots[0].fixture = Some(fixture_spec());
    store_active(&mut w, 0);
    let first = snapshot(&w, 1);
    let before = state_value(&w);
    assert!(select_view(&mut w, 0, 0).is_err());
    assert_eq!(state_value(&w), before);
    load_active(&mut w, 0, 2).unwrap();
    assert_eq!(w.doc.shots[0].path[0][1], 20.0);
    assert_eq!(w.doc.shots[0].fixture, None);
    assert_eq!(w.frames[0].image.as_ref().unwrap().view, 2);
    assert_eq!(w.frames[0].trial.as_ref().unwrap().verdict, Some(Verdict::NgGap));
    assert!(w.frames[0].saved);
    w.doc.shots[0].path[0][1] = 23.0;
    w.frames[0].trial = None;
    w.frames[0].saved = false;
    store_active(&mut w, 0);
    load_active(&mut w, 0, 1).unwrap();
    assert_eq!(snapshot(&w, 1), first);
    assert_eq!(w.doc.shots[0].fixture, Some(fixture_spec()));
    load_active(&mut w, 0, 2).unwrap();
    assert_eq!(w.doc.shots[0].path[0][1], 23.0);
    assert!(w.frames[0].trial.is_none());
    assert!(!w.frames[0].saved);
    assert!(projected_frame(&w, 0, 3).unwrap().saved);
}

#[test]
fn skipping_a_shot_deselects_its_views_but_keeps_images_and_saved_trials() {
    let mut w = workspace();
    let images = state_value(&w.frames[0].views);
    let mut doc = w.doc.clone();
    doc.shots[0].skip = true;
    update_doc(&mut w, doc).unwrap();
    assert!(selected_views(&w, 0).is_empty());
    assert_eq!(state_value(&w.frames[0].views), images);
    assert!(projected_frame(&w, 0, 1).unwrap().saved);
}

#[test]
fn editing_one_view_invalidates_only_that_trial() {
    // 改图 2 的中线或特征定位（测量定义）：只有图 2 的试测与已保存状态失效
    let edits: [(&str, fn(&mut crate::recipe::ShotViewSpec)); 2] = [
        ("中线", |v| v.path[0][1] += 1.0),
        ("特征定位", |v| v.fixture = Some(fixture_spec())),
    ];
    for (name, edit) in edits {
        let mut w = workspace();
        let others: Vec<_> = [1, 3].into_iter().map(|v| snapshot(&w, v)).collect();
        let mut doc = w.doc.clone();
        edit(&mut doc.shots[0].views[1]);
        update_doc(&mut w, doc).unwrap();
        let edited = projected_frame(&w, 0, 2).unwrap();
        assert!(edited.trial.is_none() && !edited.saved, "{name}");
        for (v, expected) in [1, 3].into_iter().zip(others) { assert_eq!(snapshot(&w, v), expected, "{name}"); }
    }
}

#[test]
fn changing_default_detect_keeps_the_trial_of_a_view_with_its_own_detect() {
    // 图 2 有自己的检测参数：改配方默认检测参数不影响它的试测
    let mut w = workspace();
    let mut doc = w.doc.clone();
    doc.detect.search_mm += 1.0;
    update_doc(&mut w, doc).unwrap();
    assert!(projected_frame(&w, 0, 2).unwrap().saved);
}

#[test]
fn view_falling_back_to_changed_default_detect_requires_retrial() {
    // 默认检测参数已改，图 2 去掉自己的参数改用默认：实际参数变了，原试测作废
    let mut w = workspace();
    let mut doc = w.doc.clone();
    doc.detect.search_mm += 1.0;
    update_doc(&mut w, doc).unwrap();
    let mut doc = w.doc.clone();
    doc.shots[0].views[1].detect = None;
    update_doc(&mut w, doc).unwrap();
    assert!(!projected_frame(&w, 0, 2).unwrap().saved);
    assert!(projected_frame(&w, 0, 2).unwrap().trial.is_none());
}

const THREE_CAMERAS: [&str; 3] = ["cam1", "cam2", "cam3"];

fn three_camera_round(round_id: &str) -> crate::recipe_capture::CaptureRound {
    use crate::recipe_capture::{CaptureCamera, CaptureFrame, CaptureRound, CaptureShot, CaptureState, CaptureView};
    let cameras = THREE_CAMERAS;
    CaptureRound {
        round_id: round_id.into(), recipe_id: "MTR-HSG-B".into(),
        cameras: cameras.iter().map(|c| CaptureCamera { camera_id: (*c).into(), camera_config: json!({}), imaging: crate::recipe::DEFAULT_IMAGING, device_session: 1, received_count: 2 }).collect(),
        planned_count: Some(2), plc_planned_count: Some(2), plc_actual_count: Some(2), plc_plan_version: Some(1), state: CaptureState::Complete,
        created_at: 1, ended_at: Some(2), error: None, failure: None, simulated: true,
        frames: (1..=2u64).map(|n| CaptureShot { shot_id: format!("S{n:03}"), ordinal: n as u32, cameras: cameras.iter().map(|c| CaptureFrame {
            camera_id: (*c).into(), frame_counter: n, trigger_counter: Some(n), original_path: String::new(),
            views: vec![CaptureView { view: 1, path: String::new(), width: 4, height: 4 }] }).collect() }).collect(),
    }
}

/// 回归：普通采集模式下能把已入库的独立验证样本轮次再采用为示教，到验证时才报“示教采集不能作为独立验证样本”。
#[test]
fn round_already_banked_as_validation_sample_is_rejected_for_teaching() {
    let mut w = Workspace::new(crate::recipe::samples().remove(1), None);
    w.sample_bank.push(BankSample { id: "round-sample".into(), name: "独立采集".into(), geometry_tag: Value::Null, expected: crate::judge::Verdict::Ok,
        created_at: 1, capture_id: Some("round-sample".into()), camera_state: Value::Null });
    let before = w.doc.shots.clone();
    let error = super::workflow::adopt_shots(&mut w, &three_camera_round("round-sample"), false).unwrap_err();
    assert!(error.contains("独立验证样本"), "{error}");
    assert_eq!(w.doc.shots.len(), before.len(), "拒绝采用时不能改动候选拍照点");
    super::workflow::adopt_shots(&mut w, &three_camera_round("round-teach"), false).unwrap();
}

/// 回归：三相机整圈采集采用时，只加了 S001-cam1 就校验配方会误判 cam2 未被使用而失败。
#[test]
fn three_camera_round_adopts_every_camera_shot_into_a_recipe_that_builds() {
    let cameras = THREE_CAMERAS;
    let round = three_camera_round("round-3cam");
    let mut w = Workspace::new(crate::recipe::samples().remove(1), None);
    super::workflow::adopt_shots(&mut w, &round, false).unwrap();
    assert_eq!(w.doc.shots.iter().map(|s| (s.id.as_str(), s.pose_id.as_str(), s.camera.as_str())).collect::<Vec<_>>(), [
        ("S001-cam1", "S001", "cam1"), ("S001-cam2", "S001", "cam2"), ("S001-cam3", "S001", "cam3"),
        ("S002-cam1", "S002", "cam1"), ("S002-cam2", "S002", "cam2"), ("S002-cam3", "S002", "cam3"),
    ]);
    assert_eq!(w.doc.cameras.keys().map(String::as_str).collect::<Vec<_>>(), cameras);
    w.doc.build().unwrap();
}
#[test]
fn selecting_group_image_keeps_drafts_and_capture_plan_but_measures_only_selected_camera() {
    let mut w = workspace();
    let template = w.doc.shots[0].clone();
    let frame = w.frames[0].clone();
    let imaging = *w.doc.cameras.values().next().unwrap();
    w.doc.shots.clear();
    w.frames.clear();
    w.doc.cameras.clear();
    for pose in ["S001", "S002"] {
        for camera in ["cam1", "cam2", "cam3"] {
            let mut shot = template.clone();
            shot.id = format!("{pose}-{camera}");
            shot.pose_id = pose.into();
            shot.camera = camera.into();
            w.doc.cameras.insert(camera.into(), imaging);
            w.doc.shots.push(shot);
            let mut image = frame.clone();
            image.k = w.frames.len();
            w.frames.push(image);
        }
    }
    w.select_group_image(3, 1).unwrap();
    let other = state_value(&w.doc.shots[3..].to_vec());
    let draft = w.doc.shots[0].path.clone();
    let plan = crate::plc_plan::trigger_groups(&w.doc.build().unwrap()).unwrap();
    w.select_group_image(0, 1).unwrap();
    w.select_group_image(1, 2).unwrap();
    assert!(w.doc.shots[0].skip && w.doc.shots[2].skip);
    assert_eq!(w.doc.shots[1].enabled_views(), vec![2]);
    assert_eq!(w.doc.shots[1].view, 2);
    assert_eq!(w.doc.shots[0].path, draft);
    assert_eq!(state_value(&w.doc.shots[3..].to_vec()), other);
    assert_eq!(crate::plc_plan::trigger_groups(&w.doc.build().unwrap()).unwrap(), plan);
    assert!(w.group_selection_errors().is_empty());
    let before = state_value(&w);
    assert!(w.select_group_image(1, 4).is_err());
    assert_eq!(state_value(&w), before);
}

#[test]
fn missing_or_multiple_group_images_block_teaching_validation_and_publish_progress() {
    for selected in [0, 1, 2] {
        let mut w = workspace();
        w.capture_id = Some("round-1".into());
        for image in &mut w.doc.shots[0].views { image.enabled = image.view <= selected; }
        let progress = progress::compute(&w, None, &progress::Devices::default());
        assert_eq!(w.group_selection_errors().is_empty(), selected == 1);
        if selected != 1 {
            for step in [progress::StepId::Teach, progress::StepId::Validation, progress::StepId::Publish] {
                assert_eq!(progress.step(step).status, progress::StepStatus::Blocked);
            }
        }
    }
}

#[test]
fn invalid_frozen_group_selection_rejects_activation_and_retry_even_when_current_draft_is_valid() {
    for selected in [0, 1, 2] {
        let mut w = pending_workspace();
        w.publish = PublishState::Failed { message: "等待重试".into() };
        for image in &mut w.pending.as_mut().unwrap().doc.shots[0].views { image.enabled = image.view <= selected; }
        assert!(w.group_selection_errors().is_empty());
        assert_eq!(w.pending.as_ref().unwrap().check_group_selection().is_ok(), selected == 1);
        let before = state_value(&w);
        assert_eq!(w.retry_publish(w.revision).is_ok(), selected == 1);
        if selected != 1 { assert_eq!(state_value(&w), before); }
    }
}

#[test]
fn rotation_can_only_be_enabled_by_saving_params_with_an_explicit_confirmation() {
    // 防的回归（定稿 §10 决定二）：草稿自动保存或整份保存悄悄把转角从 0 调大；保存示教参数不带独立证据与说明、
    // 没有当前修订的验证统计或转角证据不满足前两个条件也能开转角；或守卫把调小转角也拦下
    fn base(angle: f32) -> Workspace {
        let mut w = workspace();
        for image in &mut w.frames[0].views { image.size = [600, 300]; }
        w.frames[0].image = Some(w.frames[0].views[0].clone());
        w.doc.shots[0].fixture = Some(crate::recipe::FixtureSpec { angle_range_deg: angle, ..fixture_spec() });
        store_active(&mut w, 0);
        // 当前修订的验证：转角证据满足前两个条件
        let mut report = fixture_report(true, true);
        report.shot_id = w.doc.shots[0].id.clone();
        report.rotation.auto_conditions = true;
        w.validation.as_mut().unwrap().fixture = vec![report];
        w
    }
    fn params(w: &mut Workspace, f: crate::recipe::FixtureSpec, rotation: Option<RotationConfirmation>) -> Result<(), String> {
        let shot = &w.doc.shots[0];
        let teach = ShotTeach { path: shot.path.clone(), mm_per_px: shot.mm_per_px.unwrap(), detect: shot.detect.clone(), limits: shot.limits.clone(), fixture: Some(f), rotation };
        apply_params(w, 0, teach)
    }
    fn draft(w: &mut Workspace, f: crate::recipe::FixtureSpec, _: Option<RotationConfirmation>) -> Result<(), String> {
        let shot = &w.doc.shots[0];
        workflow::apply_draft(w, 0, workflow::DraftTeach { path: shot.path.clone(), mm_per_px: shot.mm_per_px, detect: shot.detect.clone(), limits: shot.limits.clone(), fixture: Some(f) })
    }
    fn whole(w: &mut Workspace, f: crate::recipe::FixtureSpec, _: Option<RotationConfirmation>) -> Result<(), String> {
        let mut doc = w.doc.clone();
        doc.shots[0].fixture = Some(f.clone());
        doc.shots[0].views[0].fixture = Some(f);
        update_doc(w, doc)
    }
    type Entry = fn(&mut Workspace, crate::recipe::FixtureSpec, Option<RotationConfirmation>) -> Result<(), String>;
    let confirmed = |note: &str| Some(RotationConfirmation { basis: RotationBasis::TwoTemplates, note: note.into() });
    type Evidence = fn(&mut Workspace);
    let keep: Evidence = |_| {};
    let stale: Evidence = |w| w.validation.as_mut().unwrap().revision += 1;
    let unmet: Evidence = |w| w.validation.as_mut().unwrap().fixture[0].rotation.auto_conditions = false;
    // （名称，原转角，入口，新转角，确认，验证证据，是否接受）
    let cases: [(&str, f32, Entry, f32, Option<RotationConfirmation>, Evidence, bool); 8] = [
        ("草稿自动保存开转角", 0.0, draft, 3.0, None, keep, false),
        ("整份保存开转角", 0.0, whole, 3.0, None, keep, false),
        ("保存示教参数、没有确认", 0.0, params, 3.0, None, keep, false),
        ("保存示教参数、说明为空", 0.0, params, 3.0, confirmed("  "), keep, false),
        ("保存示教参数、验证不是当前修订的", 0.0, params, 3.0, confirmed("同一视图两个模板估出的 θ 一致"), stale, false),
        ("保存示教参数、转角证据不满足前两个条件", 0.0, params, 3.0, confirmed("同一视图两个模板估出的 θ 一致"), unmet, false),
        ("保存示教参数、证据满足且带独立证据", 0.0, params, 3.0, confirmed("同一视图两个模板估出的 θ 一致"), keep, true),
        ("草稿自动保存关转角", 3.0, draft, 0.0, None, unmet, true),
    ];
    for (name, before, entry, after, rotation, evidence, accepted) in cases {
        let mut w = base(before);
        evidence(&mut w);
        let result = entry(&mut w, crate::recipe::FixtureSpec { angle_range_deg: after, ..fixture_spec() }, rotation);
        assert_eq!(result.is_ok(), accepted, "{name}：{result:?}");
        let saved = w.doc.shots[0].fixture.as_ref().unwrap().angle_range_deg;
        assert_eq!(saved, if accepted { after } else { before }, "{name}");
    }
}

#[test]
fn end_params_cannot_be_written_by_draft_whole_doc_or_params_saves_only_carried_or_reset() {
    // 防的回归（定稿 §10 决定一）：示教余量 m 与两端搬移上限只该来自验证建议；前端载荷经草稿自动保存、整份保存或
    // 保存示教参数把 m 设大、上限放宽，两端永远不判、护栏失效，验证与发布照样通过。原样带过（含改框）与回到默认不拦
    fn draft(w: &mut Workspace, f: crate::recipe::FixtureSpec) -> Result<(), String> {
        let shot = &w.doc.shots[0];
        workflow::apply_draft(w, 0, workflow::DraftTeach { path: shot.path.clone(), mm_per_px: shot.mm_per_px, detect: shot.detect.clone(), limits: shot.limits.clone(), fixture: Some(f) })
    }
    fn whole(w: &mut Workspace, f: crate::recipe::FixtureSpec) -> Result<(), String> {
        let mut doc = w.doc.clone();
        doc.shots[0].fixture = Some(f.clone());
        doc.shots[0].views[0].fixture = Some(f);
        update_doc(w, doc)
    }
    fn params(w: &mut Workspace, f: crate::recipe::FixtureSpec) -> Result<(), String> {
        let shot = &w.doc.shots[0];
        apply_params(w, 0, ShotTeach { path: shot.path.clone(), mm_per_px: shot.mm_per_px.unwrap(), detect: shot.detect.clone(), limits: shot.limits.clone(), fixture: Some(f), rotation: None })
    }
    type Entry = fn(&mut Workspace, crate::recipe::FixtureSpec) -> Result<(), String>;
    let applied = crate::recipe::FixtureSpec { max_end_shift_mm: Some(2.2), end_margin_mm: 1.3, ..fixture_spec() };
    // （名称，入口，原参数，新参数，是否接受）
    let cases: [(&str, Entry, (Option<f32>, f32), (Option<f32>, f32), bool); 6] = [
        ("草稿自动保存设示教余量", draft, (None, 0.0), (None, 50.0), false),
        ("整份保存放宽两端上限", whole, (Some(2.2), 1.3), (Some(20.0), 1.3), false),
        ("保存示教参数改示教余量", params, (Some(2.2), 1.3), (Some(2.2), 5.0), false),
        ("草稿自动保存、改框、原样带过", draft, (Some(2.2), 1.3), (Some(2.2), 1.3), true),
        ("保存示教参数、改框、原样带过", params, (Some(2.2), 1.3), (Some(2.2), 1.3), true),
        ("整份保存回到默认", whole, (Some(2.2), 1.3), (None, 0.0), true),
    ];
    for (name, entry, before, after, accepted) in cases {
        let mut w = fixture_workspace();
        let set = |f: &mut crate::recipe::FixtureSpec, (max, m): (Option<f32>, f32)| { f.max_end_shift_mm = max; f.end_margin_mm = m; };
        let mut old = applied.clone();
        set(&mut old, before);
        w.doc.shots[0].fixture = Some(old);
        store_active(&mut w, 0);
        let mut new = applied.clone();
        set(&mut new, after);
        new.rect[0] += 8;
        let result = entry(&mut w, new);
        assert_eq!(result.is_ok(), accepted, "{name}：{result:?}");
        let f = w.doc.shots[0].views[0].fixture.clone().unwrap();
        assert_eq!((f.max_end_shift_mm, f.end_margin_mm), if accepted { after } else { before }, "{name}");
    }
}

fn fixture_workspace() -> Workspace {
    let mut w = workspace();
    for image in &mut w.frames[0].views { image.size = [600, 300]; }
    w.frames[0].image = Some(w.frames[0].views[0].clone());
    w.doc.shots[0].fixture = Some(fixture_spec());
    store_active(&mut w, 0);
    w
}

fn fixture_report(passed: bool, applicable: bool) -> crate::vision::fixture_quality::FixtureViewReport {
    use crate::vision::fixture_quality::{EndSuggestion, FixtureViewReport};
    FixtureViewReport { k: 0, view: 1, shot_id: "P1".into(), passed, samples: 12, checks: Vec::new(), sigma_mm: Some(0.02), mean_mm: Some(0.0),
        nominal_mm: 0.0, tolerance_mm: 0.5, search_use: Some(0.1), rotation: Default::default(), rows: Vec::new(),
        suggestion: Some(EndSuggestion { p99_mm: 1.2, max_end_shift_mm: 2.2, end_margin_mm: 1.3, applicable, note: String::new() }) }
}

#[test]
fn end_suggestion_is_applied_only_from_the_current_validation_and_invalidates_that_views_trial() {
    // 防的回归（决定一）：拿过期验证（或不能采用）的建议写进配方；写了 maxEndShiftMm 与示教余量却不作废该视图的试测与验证
    // （名称，验证修订是否当前，建议能否采用，是否接受）
    for (name, current, applicable, accepted) in [("验证已过期", false, true, false), ("建议不能采用", true, false, false), ("当前验证的建议", true, true, true)] {
        let mut w = fixture_workspace();
        let revision = w.revision;
        let validation = w.validation.as_mut().unwrap();
        validation.fixture = vec![fixture_report(true, applicable)];
        if !current { validation.revision = revision + 1; }
        let result = super::fixture::apply_fixture_suggestion(&mut w, revision, 0, 1);
        assert_eq!(result.is_ok(), accepted, "{name}：{result:?}");
        let f = w.doc.shots[0].views[0].fixture.clone().unwrap();
        let state = |view: u8| w.frames[0].view_states.iter().find(|s| s.view == view).unwrap();
        if accepted {
            assert_eq!((f.max_end_shift_mm, f.end_margin_mm), (Some(2.2), 1.3), "{name}");
            assert_eq!(w.doc.shots[0].fixture, Some(f), "{name}：所选视图的拍照点级 fixture 同步");
            assert!(state(1).trial.is_none() && !state(1).saved && w.frames[0].trial.is_none(), "{name}：该视图试测作废");
            assert!(state(2).trial.is_some(), "{name}：其他视图试测不动");
            assert!(w.revision > revision, "{name}：修订号前进，验证随之失效");
        } else {
            assert_eq!((f.max_end_shift_mm, f.end_margin_mm), (None, 0.0), "{name}");
            assert_eq!(w.revision, revision, "{name}");
        }
    }
}

#[test]
fn template_dir_cleanup_removes_old_sources_and_boxes_but_keeps_the_current_box_and_fresh_writes() {
    // 防的回归（G4 第 7 项）：模板按 (来源, 框, 外圈) 寻址，每次换框、重新冻结都留下一个目录，工作台目录无限累积；
    // 或清理把当前框正在用的模板、写到一半的新模板删掉
    let root = super::tests::ImportTestDir::new();
    let mut w = fixture_workspace();
    let old = super::fixture::template_dir_name(w.doc.shots[0].fixture.as_ref().unwrap());
    let mut moved = fixture_spec();
    moved.rect[0] += 8;
    w.doc.shots[0].fixture = Some(moved.clone());
    store_active(&mut w, 0);
    let current = super::fixture::template_dir_name(&moved);
    let fixtures = root.0.join("fixtures");
    let retake = format!("capture-retake-v1-r{}_{}_32_32-p4", moved.rect[0], moved.rect[1]);
    for name in [&old, &current, &retake] {
        std::fs::create_dir_all(fixtures.join(name)).unwrap();
        std::fs::write(fixtures.join(name).join("template.pgm"), b"P5\n1 1\n255\n\0").unwrap();
    }
    let fresh_tmp = fixtures.join(&current).join("template.pgm.tmp-1-0");
    let stale_tmp = fixtures.join(&current).join("template.pgm.tmp-1-1");
    std::fs::write(&fresh_tmp, b"partial").unwrap();
    std::fs::write(&stale_tmp, b"partial").unwrap();
    std::fs::File::options().write(true).open(&stale_tmp).unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600)).unwrap();
    super::fixture::prune_fixture_dirs(&root.0, &w);
    assert!(!fixtures.join(&old).exists(), "旧框的目录");
    assert!(!fixtures.join(&retake).exists(), "别的来源的目录");
    assert!(fixtures.join(&current).join("template.pgm").is_file(), "当前框的模板");
    assert!(fresh_tmp.is_file() && !stale_tmp.exists(), "只删放了很久的临时文件");
}
