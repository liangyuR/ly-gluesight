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
    let copied: CalibrationCheck = serde_json::from_value(state_value(&check)).unwrap();
    assert_eq!(copied.verification_id, "verification-a");
    assert_eq!(copied.source_k, 7);
    assert_eq!(copied.source_image_id, "capture-a-v1");
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
fn plc_gate_waiter_is_queued_ahead_of_later_contenders() {
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let order = Arc::new(Mutex::new(Vec::new()));
    let holder = gate.clone().blocking_lock_owned();
    let waiter = {
        let (gate, order) = (gate.clone(), order.clone());
        std::thread::spawn(move || workflow::lock_plc_gate(&gate, std::time::Duration::from_secs(3), "测试").map(|_guard| order.lock().unwrap().push("waiter")))
    };
    std::thread::sleep(std::time::Duration::from_millis(100));
    {
        let (gate, order) = (gate.clone(), order.clone());
        std::thread::spawn(move || {
            let _guard = gate.blocking_lock();
            order.lock().unwrap().push("later");
            std::thread::sleep(std::time::Duration::from_millis(3500));
        });
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    drop(holder);
    assert!(waiter.join().unwrap().is_ok());
    assert_eq!(order.lock().unwrap().first(), Some(&"waiter"));
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
fn failed_activation_retains_the_same_frozen_release_and_requires_explicit_retry() {
    let mut before = pending_workspace();
    before.capture_id = Some("whole-round".into());
    let unchanged = state_value(&before);
    assert!(before.require_single_frame_source().is_err());
    assert_eq!(state_value(&before), unchanged);
    let release = state_value(&before.pending);
    let mut after = before.clone();
    after.pending = None;
    after.doc.version += 1;
    after.base_revision = Some("already-active-v2".into());
    after.changed();
    after.retain_failed_activation(before.clone(), "disk write denied");
    assert!(after.activation_uncertain());
    assert_eq!(state_value(&after.pending), release);
    assert_eq!(after.doc, before.doc);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.base_revision, before.base_revision);
    assert!(after.expect(after.revision).is_err());
    let failed = state_value(&after);
    assert!(after.retry_publish(after.revision + 1).is_err());
    assert_eq!(state_value(&after), failed);
    let mut restored: Workspace = serde_json::from_value(failed).unwrap();
    assert!(restored.activation_uncertain());
    restored.retry_publish(restored.revision).unwrap();
    assert!(!restored.activation_uncertain());
    assert_eq!(state_value(&restored.pending), release);
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
    w.validation = Some(Validation { revision: w.revision, passed: true, checked_at: 1, checks: Vec::new(), samples: Vec::new(), environment_tag: json!({}) });
    w
}

#[test]
fn switching_views_keeps_independent_centerlines_and_valid_ng_trials() {
    let mut w = workspace();
    let first = snapshot(&w, 1);
    let before = state_value(&w);
    assert!(select_view(&mut w, 0, 0).is_err());
    assert_eq!(state_value(&w), before);
    load_active(&mut w, 0, 2).unwrap();
    assert_eq!(w.doc.shots[0].path[0][1], 20.0);
    assert_eq!(w.frames[0].image.as_ref().unwrap().view, 2);
    assert_eq!(w.frames[0].trial.as_ref().unwrap().verdict, Some(Verdict::NgGap));
    assert!(w.frames[0].saved);
    w.doc.shots[0].path[0][1] = 23.0;
    w.frames[0].trial = None;
    w.frames[0].saved = false;
    store_active(&mut w, 0);
    load_active(&mut w, 0, 1).unwrap();
    assert_eq!(snapshot(&w, 1), first);
    load_active(&mut w, 0, 2).unwrap();
    assert_eq!(w.doc.shots[0].path[0][1], 23.0);
    assert!(w.frames[0].trial.is_none());
    assert!(!w.frames[0].saved);
    assert!(projected_frame(&w, 0, 3).unwrap().saved);
    let images = state_value(&w.frames[0].views);
    let mut doc = w.doc.clone();
    doc.shots[0].skip = true;
    update_doc(&mut w, doc).unwrap();
    assert!(selected_views(&w, 0).is_empty());
    assert_eq!(state_value(&w.frames[0].views), images);
    assert!(projected_frame(&w, 0, 1).unwrap().saved);
}

#[test]
fn editing_one_view_invalidates_only_that_trial_and_restoring_defaults_requires_retrial() {
    let mut w = workspace();
    let others: Vec<_> = [1, 3].into_iter().map(|v| snapshot(&w, v)).collect();
    let mut doc = w.doc.clone();
    doc.shots[0].views[1].path[0][1] += 1.0;
    update_doc(&mut w, doc).unwrap();
    assert!(projected_frame(&w, 0, 2).unwrap().trial.is_none());
    for (v, expected) in [1, 3].into_iter().zip(others) { assert_eq!(snapshot(&w, v), expected); }
    let mut w = workspace();
    let mut doc = w.doc.clone();
    doc.detect.search_mm += 1.0;
    update_doc(&mut w, doc).unwrap();
    assert!(projected_frame(&w, 0, 2).unwrap().saved);
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
        created_at: 1, ended_at: Some(2), error: None, simulated: true,
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
