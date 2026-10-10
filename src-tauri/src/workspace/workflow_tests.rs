use super::*;
use super::workflow::{initialize_views, load_active, projected_frame, selected_views, store_active};

#[test]
fn adopting_or_validating_capture_requires_a_nonzero_plc_plan_version() {
    for invalid in [None, Some(0)] {
        assert!(workflow::require_capture_plan(invalid).unwrap_err().contains("PLC 轨迹计划版本"));
    }
    for version in [1, 17, u32::MAX] {
        assert_eq!(workflow::require_capture_plan(Some(version)).unwrap(), version);
    }
}

#[test]
fn changed_nested_calibration_updates_inherited_scale_but_preserves_manual_override() {
    let old = json!({"path":"calib.json","value":{"data":{"mmPerPx":0.1}}});
    assert_eq!(workflow::calibration_scale(&old), Some(0.1));
    assert_eq!(workflow::updated_calibration_scale(Some(0.1), &old, Some(0.2)), Some(0.2));
    assert_eq!(workflow::updated_calibration_scale(None, &old, Some(0.2)), Some(0.2));
    assert_eq!(workflow::updated_calibration_scale(Some(0.3), &old, Some(0.2)), Some(0.3));
    assert_eq!(workflow::updated_calibration_scale(Some(0.1), &old, None), None);
    assert_eq!(workflow::calibration_scale(&json!({"value":{"mmPerPx":0.4}})), Some(0.4));
}

#[test]
fn representative_calibration_reuse_requires_matching_environment_and_retains_source_identity() {
    let w = workspace();
    let mut image = w.frames[0].views[0].clone();
    image.camera_tag = json!({"id":"device1","session":2});
    image.calib_tag = json!({"path":"device1-view1","value":{"data":{"mmPerPx":0.1}}});
    let check = CalibrationCheck { verification_id:"verification-a".into(), source_k:7, source_image_id:"capture-a-v1".into(),
        reference_mm:10.0, measured_mm:10.05, error_mm:0.05, passed:true,
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

fn snapshot(w: &Workspace, selected: u8) -> Value {
    let frame = projected_frame(w, 0, selected).unwrap();
    json!({"image":frame.image,"trial":frame.trial,"saved":frame.saved})
}

fn pending_workspace() -> Workspace {
    let mut w = workspace();
    w.pending = Some(Box::new(Release { doc: w.doc.clone(), bundle_id: "frozen-release-17".into(),
        base_revision: w.base_revision.clone(), revision: w.revision, frames: w.frames.clone(),
        overview: w.overview.clone(), validation: w.validation.clone().unwrap() }));
    w
}

#[test]
fn whole_round_source_blocks_every_single_frame_replacement_without_mutation() {
    let mut w = workspace();
    assert!(w.require_single_frame_source().is_ok());
    w.capture_id = Some("capture-original".into());
    let before = state_value(&w);
    assert!(w.require_single_frame_source().unwrap_err().contains("整圈"));
    assert_eq!(state_value(&w), before);
}

#[test]
fn historical_sample_requires_the_teaching_plc_plan_even_with_identical_shot_fields() {
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
fn capture_start_requires_an_existing_unfrozen_candidate() {
    let w = workspace();
    let id = w.doc.id.clone();
    let host = WorkspaceHost { root: PathBuf::new(), items: Mutex::new(HashMap::from([(id.clone(), w)])), notes: vec![],
        live: Mutex::new(LiveFrames::default()), station: Mutex::new(HashMap::new()),
        capture_seq: std::sync::atomic::AtomicU64::new(0), comparisons: comparisons::Comparisons::default() };
    assert!(host.capture_for_candidate("missing", || -> Result<(), String> { panic!("不存在的候选不能启动采集") }).is_err());
    assert_eq!(host.capture_for_candidate(&id, || Ok(17)).unwrap(), 17);
    host.items.lock().unwrap().insert(id.clone(), pending_workspace());
    assert!(host.capture_for_candidate(&id, || -> Result<(), String> { panic!("冻结的候选不能启动采集") }).is_err());
}

#[test]
fn failed_activation_retains_the_same_frozen_release_and_requires_explicit_retry() {
    let before = pending_workspace();
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

fn workspace() -> Workspace {
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
}

#[test]
fn deselect_and_reselect_preserves_unchanged_trial_but_invalidates_full_validation() {
    let mut w = workspace();
    let original = snapshot(&w, 2);
    let revision = w.revision;
    let mut doc = w.doc.clone();
    doc.shots[0].views[1].enabled = false;
    update_doc(&mut w, doc).unwrap();
    assert_eq!(selected_views(&w, 0), [1, 3]);
    assert_eq!(snapshot(&w, 2), original);
    assert!(w.validation.is_none());
    assert_eq!(w.revision, revision + 1);
    let mut doc = w.doc.clone();
    doc.shots[0].views[1].enabled = true;
    update_doc(&mut w, doc).unwrap();
    assert_eq!(selected_views(&w, 0), [1, 2, 3]);
    load_active(&mut w, 0, 2).unwrap();
    assert!(w.frames[0].saved);
    assert_eq!(snapshot(&w, 2), original);
}

#[test]
fn default_detection_change_only_invalidates_views_without_overrides() {
    let mut w = workspace();
    let overridden = snapshot(&w, 2);
    let mut doc = w.doc.clone();
    doc.detect.search_mm += 1.0;
    update_doc(&mut w, doc).unwrap();
    for selected in [1, 3] {
        let frame = projected_frame(&w, 0, selected).unwrap();
        assert!(!frame.saved && frame.trial.is_none());
        assert!(frame.image.is_some());
    }
    assert_eq!(snapshot(&w, 2), overridden);
    assert!(w.validation.is_none());
}

#[test]
fn default_limits_change_only_invalidates_views_without_overrides() {
    let mut w = workspace();
    let overridden = snapshot(&w, 3);
    let mut doc = w.doc.clone();
    doc.limits.max_gap_len += 1.0;
    update_doc(&mut w, doc).unwrap();
    for selected in [1, 2] {
        let frame = projected_frame(&w, 0, selected).unwrap();
        assert!(!frame.saved && frame.trial.is_none());
    }
    assert_eq!(snapshot(&w, 3), overridden);
    assert!(w.validation.is_none());
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

#[test]
fn skipping_is_explicit_and_does_not_destroy_teaching() {
    let mut w = workspace();
    let images = state_value(&w.frames[0].views);
    let mut doc = w.doc.clone();
    doc.shots[0].skip = true;
    update_doc(&mut w, doc).unwrap();
    assert!(selected_views(&w, 0).is_empty());
    assert!(w.frames[0].view_states.iter().all(|v| v.saved));
    assert_eq!(state_value(&w.frames[0].views), images);
    let mut doc = w.doc.clone();
    doc.shots[0].skip = false;
    doc.shots[0].views.iter_mut().for_each(|v| v.enabled = false);
    update_doc(&mut w, doc).unwrap();
    assert!(selected_views(&w, 0).is_empty());
    assert!(!w.doc.shots[0].skip);
    assert!(!w.doc.build().unwrap().shots[0].taught());
}
