use super::progress::{compute, CalibrationState, Devices, ShotState, StepId, StepStatus};
use super::workflow::{CalibrationBasis, CalibrationCheck};
use super::workflow_tests::workspace;
use super::*;

fn captured() -> Workspace {
    let mut w = workspace();
    w.capture_id = Some("round-1".into());
    w
}

/// 当前设备与冻结图完全一致。
fn matching(w: &Workspace) -> Devices {
    let views = w.frames.iter().enumerate()
        .flat_map(|(k, f)| f.views.iter().map(move |i| ((k, i.view), (i.camera_tag.clone(), i.calib_tag.clone()))))
        .collect();
    Devices { views, engine: Some(json!("engine-1")) }
}

fn declare(w: &mut Workspace, basis: CalibrationBasis, passed: bool) {
    for state in &mut w.frames[0].view_states {
        state.calibration_check = Some(CalibrationCheck { verification_id: format!("check-v{}", state.view), source_k: 0,
            source_image_id: format!("capture-test-v{}", state.view), basis: basis.clone(), passed,
            camera_tag: json!({}), calib_tag: json!({}), mm_per_px: Some(0.1) });
    }
}

#[test]
fn view_state_follows_trial_and_saved_records() {
    type Edit = fn(&mut Workspace);
    let cases: [(&str, Edit, ShotState, bool); 6] = [
        ("试测有效且已保存", |_| {}, ShotState::Taught, false),
        ("试测有效未保存", |w| { for s in &mut w.frames[0].view_states { s.saved = false; } w.frames[0].saved = false; }, ShotState::TrialPassed, false),
        ("试测后改中线", |w| w.doc.shots[0].views[0].path[1][0] += 5.0, ShotState::PendingTrial, true),
        ("试测执行异常", |w| { for s in &mut w.frames[0].view_states { s.trial.as_mut().unwrap().passed = false; } w.frames[0].trial.as_mut().unwrap().passed = false; }, ShotState::TrialFailed, false),
        ("有中线缺像素当量", |w| w.doc.shots[0].views[0].mm_per_px = None, ShotState::Draft, false),
        ("设为不检", |w| w.doc.shots[0].skip = true, ShotState::Skipped, false),
    ];
    for (name, edit, expected, stale) in cases {
        let mut w = captured();
        edit(&mut w);
        let progress = compute(&w, Some(1), &matching(&w));
        let view = &progress.shots[0].views[0];
        assert_eq!((view.state, view.trial_stale), (expected, stale), "{name}");
        assert_eq!(progress.shots[0].state, expected, "{name}");
    }
}

#[test]
fn calibration_step_accepts_sample_set_declaration_until_scale_changes() {
    type Edit = fn(&mut Workspace);
    let cases: [(&str, Edit, StepStatus, fn(&CalibrationState) -> bool); 5] = [
        ("未验证", |_| {}, StepStatus::NotStarted, |c| *c == CalibrationState::Unverified),
        ("样本集声明当量", |w| declare(w, CalibrationBasis::SampleSet { set_id: "mx11".into() }, true), StepStatus::Done,
            |c| *c == CalibrationState::SampleSet { set_id: "mx11".into(), mm_per_px: 0.1 }),
        ("改动当量后回到人工验证", |w| { declare(w, CalibrationBasis::SampleSet { set_id: "mx11".into() }, true); for v in &mut w.doc.shots[0].views { v.mm_per_px = Some(0.12); } },
            StepStatus::NotStarted, |c| *c == CalibrationState::Stale),
        ("人工验证通过", |w| declare(w, CalibrationBasis::Manual { reference_mm: 10.0, measured_mm: 10.05, error_mm: 0.05 }, true), StepStatus::Done,
            |c| matches!(c, CalibrationState::Verified { .. })),
        ("人工验证超差", |w| declare(w, CalibrationBasis::Manual { reference_mm: 10.0, measured_mm: 10.3, error_mm: 0.3 }, false), StepStatus::Blocked,
            |c| matches!(c, CalibrationState::Failed { .. })),
    ];
    for (name, edit, status, calibration) in cases {
        let mut w = captured();
        edit(&mut w);
        let progress = compute(&w, None, &matching(&w));
        assert_eq!(progress.step(StepId::Calibration).status, status, "{name}");
        assert!(calibration(&progress.shots[0].views[0].calibration), "{name}: {:?}", progress.shots[0].views[0].calibration);
    }
}

fn sample_set_camera(set: &str) -> Value {
    let config = crate::camera::CameraConfig { id: "cam1".into(), source: CameraSource::SampleSet, ..Default::default() };
    camera_state(&config, Some(set))
}

#[test]
fn progress_compares_frozen_images_with_current_devices() {
    type Edit = fn(&mut Workspace, &mut Devices);
    let cases: [(&str, Edit, ShotState, bool, fn(&CalibrationState) -> bool); 7] = [
        ("设备与采集时一致", |_, _| {}, ShotState::Taught, false, |c| matches!(c, CalibrationState::SampleSet { .. })),
        ("配方改了这台相机的曝光与增益", |w, _| set_imaging(w, "cam1", crate::recipe::CameraImaging { exposure_us: 80.0, gain_db: 2.0 }).unwrap(),
            ShotState::Taught, false, |c| matches!(c, CalibrationState::SampleSet { .. })),
        ("相机当前不可用", |_, d| d.views.clear(), ShotState::Recapture, false, |c| *c == CalibrationState::Stale),
        ("换同名相机的另一套样本集", |w, d| {
            for image in &mut w.frames[0].views { image.camera_tag = sample_set_camera("mx11"); }
            for state in &mut w.frames[0].view_states { state.calibration_check.as_mut().unwrap().camera_tag = sample_set_camera("mx11"); }
            for current in d.views.values_mut() { current.0 = sample_set_camera("isv"); }
        }, ShotState::Recapture, false, |c| *c == CalibrationState::Stale),
        ("工位标定改变", |_, d| { d.views.get_mut(&(0, 1)).unwrap().1 = json!({"value": {"mmPerPx": 0.2}}); }, ShotState::PendingTrial, true, |c| *c == CalibrationState::Stale),
        ("图像引擎改变", |_, d| d.engine = Some(json!("engine-2")), ShotState::PendingTrial, true, |c| matches!(c, CalibrationState::SampleSet { .. })),
        ("换了原图", |w, _| w.frames[0].views[0].id = "capture-retake-v1".into(), ShotState::PendingTrial, true, |c| matches!(c, CalibrationState::SampleSet { .. })),
    ];
    for (name, edit, state, stale, calibration) in cases {
        let mut w = captured();
        declare(&mut w, CalibrationBasis::SampleSet { set_id: "mx11".into() }, true);
        let mut devices = matching(&w);
        edit(&mut w, &mut devices);
        let progress = compute(&w, None, &devices);
        let view = &progress.shots[0].views[0];
        assert_eq!((view.state, view.trial_stale), (state, stale), "{name}");
        assert!(calibration(&view.calibration), "{name}: {:?}", view.calibration);
        assert_eq!(progress.step(StepId::Capture).status == StepStatus::Blocked, state == ShotState::Recapture, "{name}");
    }
}

#[test]
fn temporary_camera_group_replacement_leaves_saved_teaching_and_revision_untouched() {
    let mut w = captured();
    declare(&mut w, CalibrationBasis::Manual { reference_mm: 10.0, measured_mm: 10.02, error_mm: 0.02 }, true);
    let real = matching(&w);
    let record = state_value(&w);
    let offline = Devices { views: real.views.keys().map(|key| (*key, (sample_set_camera("mx11"), Value::Null))).collect(), engine: real.engine.clone() };
    let replaced = compute(&w, None, &offline);
    assert!(replaced.shots[0].views.iter().all(|v| v.state == ShotState::Recapture));
    assert_eq!(state_value(&w), record);
    let restored = compute(&w, None, &real);
    assert!(restored.shots[0].views.iter().all(|v| v.state == ShotState::Taught && matches!(v.calibration, CalibrationState::Verified { .. })));
    assert_eq!(restored.step(StepId::Teach).status, StepStatus::Done);
    assert_eq!(w.revision, serde_json::from_value::<Workspace>(record).unwrap().revision);
}

#[test]
fn publish_step_reads_explicit_publish_state() {
    let release = |w: &Workspace| Some(Box::new(Release { doc: w.doc.clone(), bundle_id: "bundle".into(), base_revision: None,
        revision: w.revision, frames: w.frames.clone(), overview: w.overview.clone(), validation: w.validation.clone().unwrap(),
        validation_site: ValidationSite::Real }));
    type Edit = fn(&mut Workspace, Option<Box<Release>>);
    let cases: [(&str, Edit, StepStatus); 6] = [
        ("验证通过未发布", |_, _| {}, StepStatus::NotStarted),
        ("待生效", |w, r| { w.pending = r; w.publish = PublishState::Waiting; }, StepStatus::InProgress),
        ("生效失败", |w, r| { w.pending = r; w.publish = PublishState::Failed { message: "包缺失".into() }; }, StepStatus::Blocked),
        ("生效状态不确定", |w, r| { w.pending = r; w.publish = PublishState::Uncertain { message: "写盘失败".into() }; }, StepStatus::Blocked),
        ("已生效", |w, _| { w.validation = None; w.publish = PublishState::Active { version: 3, revision: w.revision, warning: None, validation_site: ValidationSite::Real }; }, StepStatus::Done),
        ("生效后又改了候选", |w, _| { w.publish = PublishState::Active { version: 3, revision: w.revision, warning: None, validation_site: ValidationSite::Real }; w.changed(); }, StepStatus::NotStarted),
    ];
    for (name, edit, status) in cases {
        let mut w = captured();
        declare(&mut w, CalibrationBasis::SampleSet { set_id: "mx11".into() }, true);
        if let Some(v) = &mut w.validation { v.passed = true; }
        let pending = release(&w);
        edit(&mut w, pending);
        let progress = compute(&w, Some(2), &matching(&w));
        assert_eq!(progress.step(StepId::Publish).status, status, "{name}");
    }
}

#[test]
fn qualified_expectation_accepts_ok_and_ok_with_excursion_only() {
    use Verdict::*;
    for (expected, actual, matched) in [
        (Ok, Ok, true), (Ok, OkWithExcursion, true), (Ok, NgGap, false), (Ok, ErrInspect, false),
        (NgGap, NgGap, true), (NgGap, Ok, false), (NgGap, NgWidth, false),
    ] {
        let sample = Sample { history_id: None, sample_id: Some("round-2".into()), expected };
        let judgement = judge::Judgement { verdict: actual, ..judge::Judgement::error(0, "候选判定") };
        assert_eq!(SampleResult::judged(&sample, "独立采集".into(), 0, judgement).passed, matched, "{expected:?} / {actual:?}");
    }
}
