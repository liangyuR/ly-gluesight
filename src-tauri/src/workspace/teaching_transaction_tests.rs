use super::burst_tests::{burst_workspace, capture_round, measured, DEVICE};
use super::*;
use crate::burst_judge::FrameState;
use crate::plan::SegmentKind::{Corner, Straight};
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture {
    host: WorkspaceHost,
    workspace: Workspace,
}

fn teaching(w: &Workspace, frame: &str, seq: u64, segment: u32) -> CameraTeaching {
    let camera = w.doc.plan_cameras()[0].clone();
    CameraTeaching {
        camera: camera.clone(),
        frame: FrozenImage {
            id: frame.into(),
            view: 1,
            source: format!("capture:round#{seq}"),
            captured_at: 1,
            size: [100, 100],
            camera,
            camera_tag: Value::Null,
            calib_tag: Value::Null,
            geometry_tag: Value::Null,
            exposure_us: None,
            gain_db: None,
            history_id: None,
        },
        round: "round".into(),
        seq,
        segment: Some(segment),
    }
}

fn path() -> Vec<[f32; 2]> {
    vec![[10.0, 50.0], [90.0, 50.0]]
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../output/tests")
            .join(format!(
                "teaching-{}-{}-{}",
                std::process::id(),
                ly_plc::now_ms(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let mut workspace = burst_workspace(&[Straight]);
        workspace.instance_id = 1;
        workspace.doc.spacing = 1.0;
        workspace.doc.inspect[0].mm_per_px = Some(0.1);
        workspace.teaching_piece = Some(super::pieces_tests::piece("round", 1));
        let host = WorkspaceHost {
            root,
            items: Mutex::new(HashMap::from([(
                workspace.doc.id.clone(),
                workspace.clone(),
            )])),
            notes: Vec::new(),
            live_burst: Mutex::new(LiveFrames::default()),
            live_views: Mutex::new(LiveViews::default()),
            station: Mutex::new(HashMap::new()),
            capture_seq: AtomicU64::new(0),
            comparisons: comparisons::Comparisons::default(),
            history_bundle: Mutex::new(None),
        };
        workspace = Self::prepare(&workspace, "original", 1, false)
            .commit(&host, path(), None)
            .unwrap();
        Self { host, workspace }
    }

    fn prepare(w: &Workspace, id: &str, seq: u64, color: bool) -> PreparedSegmentTeaching {
        let image = if color {
            FrameImage::rgb(100, 100, vec![seq as u8; 30_000])
        } else {
            FrameImage::new(100, 100, vec![seq as u8; 10_000])
        };
        let mut bytes = crate::image_file::pnm_header(100, 100, image.channels, None).into_bytes();
        bytes.extend_from_slice(&image.pixels);
        PreparedSegmentTeaching::stage(w, teaching(w, id, seq, 1), Arc::new(image), bytes, 10, None)
            .unwrap()
    }

    fn state(&self) -> (Vec<u8>, Vec<u8>, Value) {
        (
            std::fs::read(self.host.dir(&self.workspace.doc.id).join("workspace.json")).unwrap(),
            std::fs::read(image_file(&self.host, &self.workspace.doc.id, "original").unwrap())
                .unwrap(),
            serde_json::to_value(
                self.host
                    .items
                    .lock()
                    .unwrap()
                    .get(&self.workspace.doc.id)
                    .unwrap(),
            )
            .unwrap(),
        )
    }

    fn assert_no_new_frame(&self) {
        assert!(
            !image_file(&self.host, &self.workspace.doc.id, "replacement")
                .unwrap()
                .exists()
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.host.root);
    }
}

#[test]
fn successful_teaching_commits_matching_frame_and_corridor_in_one_revision() {
    let fixture = Fixture::new();
    let before = fixture.state();
    let prepared = Fixture::prepare(&fixture.workspace, "replacement", 2, false);
    let corridor = vec![[10.0, 60.0], [80.0, 60.0]];
    let saved = prepared
        .commit(&fixture.host, corridor.clone(), None)
        .unwrap();
    assert_eq!(saved.revision, fixture.workspace.revision + 1);
    let selected = &saved.camera_teaching[0];
    assert_eq!(
        (selected.seq, selected.frame.id.as_str()),
        (2, "replacement")
    );
    assert_eq!(
        saved.doc.inspect[0].segment_corridor(1).unwrap().corridor,
        corridor
    );
    let actual = crate::image_file::load(
        &image_file(&fixture.host, &saved.doc.id, &selected.frame.id).unwrap(),
    )
    .unwrap();
    assert_eq!(actual.pixels, vec![2; 10_000]);
    let after = fixture.state();
    assert_eq!(after.1, before.1);
    let persisted: Workspace = serde_json::from_slice(&after.0).unwrap();
    assert_eq!(persisted.doc, saved.doc);
    assert_eq!(
        (persisted.revision, persisted.instance_id),
        (saved.revision, saved.instance_id)
    );
    assert_eq!(persisted.camera_teaching.len(), saved.camera_teaching.len());
    let persisted_frame = &persisted.camera_teaching[0];
    assert_eq!(
        (
            &persisted_frame.camera,
            persisted_frame.segment,
            persisted_frame.seq,
            &persisted_frame.round
        ),
        (
            &selected.camera,
            selected.segment,
            selected.seq,
            &selected.round
        )
    );
    assert_eq!(
        (
            &persisted_frame.frame.id,
            &persisted_frame.frame.source,
            persisted_frame.frame.size
        ),
        (
            &selected.frame.id,
            &selected.frame.source,
            selected.frame.size
        )
    );
    assert_eq!(after.2, serde_json::to_value(&saved).unwrap());
}

#[test]
fn abandoned_preparation_leaves_candidate_and_frozen_image_unchanged() {
    let fixture = Fixture::new();
    let before = fixture.state();
    let prepared = Fixture::prepare(&fixture.workspace, "replacement", 2, false);
    drop(prepared);
    assert_eq!(fixture.state(), before);
    fixture.assert_no_new_frame();
}

#[test]
fn invalid_final_corridor_or_unusable_color_preserves_the_previous_teaching() {
    for (corridor, color, model) in [
        (vec![[-0.01, 50.0], [90.0, 50.0]], false, None),
        (vec![[10.0, 50.0], [100.0, 50.0]], false, None),
        (vec![[10.0, 50.0], [11.0, 50.0]], false, None),
        (
            path(),
            true,
            Some(r#"{"quality":{"usable":false}}"#.to_string()),
        ),
        (path(), true, Some(r#"{"quality":{}}"#.to_string())),
        (
            path(),
            true,
            Some(r#"{"quality":{"usable":true,"bgCount":0}}"#.to_string()),
        ),
        (
            path(),
            true,
            Some(r#"{"quality":{"usable":true,"glueCount":1}}"#.to_string()),
        ),
        (
            path(),
            true,
            Some(r#"{"quality":{"usable":true},"background":[]}"#.to_string()),
        ),
    ] {
        let fixture = Fixture::new();
        let before = fixture.state();
        let prepared = Fixture::prepare(&fixture.workspace, "replacement", 2, color);
        assert!(prepared.commit(&fixture.host, corridor, model).is_err());
        assert_eq!(fixture.state(), before);
        fixture.assert_no_new_frame();
    }
}

#[test]
fn changed_revision_or_instance_rejects_prepared_teaching_without_writing() {
    for recreate in [false, true] {
        let fixture = Fixture::new();
        let prepared = Fixture::prepare(&fixture.workspace, "replacement", 2, false);
        {
            let mut items = fixture.host.items.lock().unwrap();
            let current = items.get_mut(&fixture.workspace.doc.id).unwrap();
            if recreate {
                current.instance_id += 1;
            } else {
                current.revision += 1;
            }
            fixture.host.save(current).unwrap();
        }
        let before = fixture.state();
        assert!(prepared.commit(&fixture.host, path(), None).is_err());
        let camera = fixture.workspace.doc.plan_cameras()[0].clone();
        assert!(fixture
            .host
            .save_fitted_color_model(
                &fixture.workspace,
                &camera,
                1,
                r#"{"quality":{"usable":true}}"#.into()
            )
            .is_err());
        let mut segments = fixture.workspace.doc.segments.clone();
        segments[0].tolerance += 1;
        assert!(fixture
            .host
            .update_segments(
                &fixture.workspace.doc.id,
                fixture.workspace.revision,
                Some(fixture.workspace.instance_id),
                segments
            )
            .is_err());
        assert_eq!(fixture.state(), before);
        fixture.assert_no_new_frame();
    }
}

#[test]
fn occupied_image_id_rejects_commit_without_deleting_the_existing_file() {
    let fixture = Fixture::new();
    let before = fixture.state();
    let prepared = Fixture::prepare(&fixture.workspace, "replacement", 2, false);
    let occupied = image_file(&fixture.host, &fixture.workspace.doc.id, "replacement").unwrap();
    std::fs::write(&occupied, b"owned by another operation").unwrap();
    assert!(prepared.commit(&fixture.host, path(), None).is_err());
    assert_eq!(
        std::fs::read(&occupied).unwrap(),
        b"owned by another operation"
    );
    assert_eq!(fixture.state(), before);
}

#[test]
fn workspace_save_failure_removes_new_image_and_preserves_previous_teaching() {
    let fixture = Fixture::new();
    let before = fixture.state();
    std::fs::create_dir(
        fixture
            .host
            .dir(&fixture.workspace.doc.id)
            .join("workspace.tmp"),
    )
    .unwrap();
    let prepared = Fixture::prepare(&fixture.workspace, "replacement", 2, false);
    assert!(prepared.commit(&fixture.host, path(), None).is_err());
    assert_eq!(fixture.state(), before);
    fixture.assert_no_new_frame();
}

#[test]
fn narrow_trial_reads_only_its_window_and_preserves_frame_numbers_and_segment_context() {
    let fixture = Fixture::new();
    let mut w = burst_workspace(&[Straight, Corner, Straight]);
    w.instance_id = 7;
    w.teaching_piece = Some(super::pieces_tests::piece("round", 1));
    let camera = w.doc.plan_cameras()[0].clone();
    let round = capture_round("round", &w.doc, 30, DEVICE, &[(0, 13)]);
    use super::capture_review::{apply_capture_review, CaptureReviewEdit};
    apply_capture_review(&mut w, &round, CaptureReviewEdit::Save(vec![10, 20])).unwrap();
    apply_capture_review(&mut w, &round, CaptureReviewEdit::Confirm).unwrap();
    apply_capture_review(
        &mut w,
        &round,
        CaptureReviewEdit::Exclude {
            start: 14,
            end: 14,
            excluded: true,
            reason: "反光".into(),
        },
    )
    .unwrap();
    w.camera_teaching.push(teaching(&w, "segment-two", 15, 2));
    w.doc.inspect[0]
        .segments
        .push(crate::recipe::SegmentCorridor {
            segment: 2,
            corridor: path(),
            sector_deg: Some(45.0),
            color_model: None,
            measure: None,
        });
    w.doc.segments[1].heading = Some(crate::plan::HeadingSettings::default());
    let inputs = fixture.host.root.join("trial-inputs");
    std::fs::create_dir_all(&inputs).unwrap();
    for seq in [12, 15, 16] {
        crate::image_file::save_pgm(
            &inputs.join(format!("{seq}.pgm")),
            &FrameImage::new(5, 5, vec![seq as u8; 25]),
        )
        .unwrap();
    }
    std::fs::write(inputs.join("11.pgm"), b"broken image").unwrap();
    let measure = |seq| {
        let image = crate::image_file::load(&inputs.join(format!("{seq}.pgm")))?;
        Ok(measured(f64::from(image.pixels[0])))
    };
    let window = measure_teaching_range(
        &w,
        &round,
        &camera,
        2,
        Some(12),
        Some(16),
        Some(w.revision),
        measure,
    )
    .unwrap();
    assert_eq!(
        (window.start, window.end, window.from, window.to),
        (11, 20, 12, 16)
    );
    assert_eq!(
        (window.revision, window.instance_id),
        (w.revision, w.instance_id)
    );
    assert_eq!(window.doc, w.doc);
    assert_eq!(
        window
            .frames
            .iter()
            .map(|f| (f.seq, f.state))
            .collect::<Vec<_>>(),
        vec![
            (12, FrameState::Measured),
            (13, FrameState::Missing),
            (14, FrameState::Unmeasured),
            (15, FrameState::Measured),
            (16, FrameState::Measured),
        ]
    );
    assert_eq!(
        window
            .frames
            .iter()
            .filter_map(|f| f.x_mm)
            .collect::<Vec<_>>(),
        vec![12.0, 15.0, 16.0]
    );
    let full = measure_teaching_range(
        &w,
        &round,
        &camera,
        2,
        None,
        None,
        Some(w.revision),
        measure,
    )
    .unwrap();
    assert_eq!(full.frames[0].seq, 11);
    assert_eq!(full.frames[0].state, FrameState::Error);
    for frame in &window.frames {
        assert_eq!(
            serde_json::to_value(frame).unwrap(),
            serde_json::to_value(&full.frames[(frame.seq - full.start) as usize]).unwrap()
        );
    }
}

#[test]
fn reteaching_on_a_gray_frame_drops_a_pending_color_choice() {
    // 防的回归：段记为按胶色、待取色后换成灰度示教帧，灰度帧取不了色，这一段一直卡在“还没取色”发布不了
    let fixture = Fixture::new();
    let mut w = fixture.workspace.clone();
    w.doc.inspect[0].segments[0].measure = Some(crate::recipe::MeasureMode::Color);
    fixture
        .host
        .items
        .lock()
        .unwrap()
        .insert(w.doc.id.clone(), w.clone());
    let saved = Fixture::prepare(&w, "replacement", 2, false)
        .commit(&fixture.host, path(), None)
        .unwrap();
    assert_eq!(
        saved.doc.inspect[0].segment_corridor(1).unwrap().measure,
        None
    );
}
