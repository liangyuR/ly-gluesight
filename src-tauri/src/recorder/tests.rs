use super::*;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("gluesight-recorder-{}-{}-{}", std::process::id(), ly_plc::now_ms(), NEXT_DIR.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn root(&self) -> PathBuf {
        self.0.join("records")
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn recipe() -> Arc<Recipe> {
    let mut doc = crate::recipe::samples().remove(1);
    doc.shots.truncate(1);
    doc.shots[0].view = 2;
    Arc::new(doc.build().unwrap())
}

fn frame(seed: u8) -> Frame {
    Frame {
        cam: 0,
        session: u64::MAX,
        counter: CounterSource::Synthetic,
        frame_counter: u64::MAX - 1,
        trigger_counter: u64::MAX - 2,
        lost_packets: 7,
        ts: 777,
        manual: false,
        images: (0..3).map(|view| Arc::new(FrameImage::new(1, 1, vec![seed + view]))).collect(),
    }
}

fn callback() -> (RecordingCallback, Receiver<RecordingOutcome>) {
    let (tx, rx) = channel();
    (Arc::new(move |outcome| tx.send(outcome).unwrap()), rx)
}

fn controlled(root: PathBuf) -> (Recorder, Receiver<Msg>, Receiver<RecordingOutcome>) {
    let (tx, rx) = channel();
    let (callback, outcomes) = callback();
    (Recorder { root, tx, queued: Arc::new(AtomicUsize::new(0)), callback: Some(callback), startup_error: None, protection: None, active: Arc::new(Mutex::new(HashSet::new())) }, rx, outcomes)
}

fn wait(outcomes: &Receiver<RecordingOutcome>) -> RecordingOutcome {
    outcomes.recv_timeout(Duration::from_secs(10)).expect("录制收尾必须产生结果")
}

fn metadata(outcome: &RecordingOutcome) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(outcome.directory.as_ref().unwrap().join("part.json")).unwrap()).unwrap()
}

#[test]
fn tricam_views_form_one_queue_item_with_original_identity() {
    let dir = TestDir::new();
    let (recorder, rx, _) = controlled(dir.root());
    let recipe = recipe();
    let mut rec = recorder.begin(RecordMode::All, 42, recipe.clone(), "cycle_one", Some("bundle-id")).unwrap();
    recorder.frame(&mut rec, &frame(21), "cam1", 0, 11);
    assert_eq!(recorder.queued.load(Ordering::Relaxed), 1);
    assert_eq!(rec.frames.len(), 3);
    let Msg::Frames(group) = rx.try_recv().unwrap() else { panic!("需要整组原图消息") };
    assert_eq!(group.cycle_id, "cycle_one");
    assert_eq!(group.images.len(), 3);
    for (i, (meta, image)) in group.images.iter().enumerate() {
        assert_eq!(meta.file, format!("k000_P1_cam1_v{}.pgm", i + 1));
        assert_eq!(meta.cycle_id, "cycle_one");
        assert_eq!(meta.bundle_id.as_deref(), Some("bundle-id"));
        assert_eq!(meta.recipe_revision, recipe.revision_id);
        assert_eq!((meta.k, meta.shot_id.as_str(), meta.selected_view, meta.session, meta.ordinal), (0, "P1", 2, u64::MAX, 11));
        assert_eq!(image.pixels, [21 + i as u8]);
    }
    assert!(rx.try_recv().is_err());
}

#[test]
fn successful_finish_reports_only_existing_files_and_complete_metadata() {
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let recipe = recipe();
    let mut rec = recorder.begin(RecordMode::All, 42, recipe.clone(), "cycle-complete", Some("frozen-bundle")).unwrap();
    recorder.frame(&mut rec, &frame(21), "cam1", 0, 11);
    recorder.finish(rec, Verdict::NgWidth, "原始胶宽不合格", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert!(outcome.available, "{:?}", outcome.errors);
    assert_eq!(outcome.state, RecordingState::Complete);
    assert_eq!(outcome.cycle_id, "cycle-complete");
    assert!(outcome.directory.as_ref().unwrap().file_name().unwrap().to_string_lossy().ends_with("_NG_WIDTH_cycle_cycle-complete"));
    assert_eq!(outcome.files.len(), 3);
    for file in &outcome.files {
        assert_eq!((file.width, file.height), (1, 1));
        assert!(!file.file.contains('\\'));
        assert!(!file.file.contains(".."));
        let actual = recorder.root().join(&file.file);
        assert_eq!(std::fs::read(actual).unwrap(), [b"P5\n1 1\n255\n".as_slice(), &[20 + file.view]].concat());
    }
    let meta = metadata(&outcome);
    assert_eq!(meta["cycleId"], "cycle-complete");
    assert_eq!(meta["bundleId"], "frozen-bundle");
    assert_eq!(meta["recipeRevision"], recipe.revision_id);
    assert_eq!(meta["verdict"], "NG_WIDTH");
    assert_eq!(meta["reason"], "原始胶宽不合格");
    assert_eq!(meta["available"], true);
    assert_eq!(meta["plannedShots"].as_array().unwrap().len(), 1);
    assert!(meta["missingShots"].as_array().unwrap().is_empty());
    for (i, meta) in meta["frames"].as_array().unwrap().iter().enumerate() {
        assert_eq!(meta["session"].as_u64(), Some(u64::MAX));
        assert_eq!(meta["ordinal"], 11);
        assert_eq!(meta["shotId"], "P1");
        assert_eq!(meta["selectedView"], 2);
        assert_eq!(meta["view"], i + 1);
        assert_eq!(meta["counter"], "synthetic");
        assert_eq!(meta["frameCounter"].as_u64(), Some(u64::MAX - 1));
        assert_eq!(meta["triggerCounter"].as_u64(), Some(u64::MAX - 2));
        assert_eq!(meta["lostPackets"], 7);
        assert_eq!(meta["available"], true);
    }
}

#[test]
fn equal_sn_and_exposure_time_never_mix_different_cycles() {
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    for (cycle, seed, verdict) in [("cycle-a", 10, Verdict::Ok), ("cycle-b", 30, Verdict::NgWidth)] {
        let mut rec = recorder.begin(RecordMode::All, 42, recipe(), cycle, None).unwrap();
        recorder.frame(&mut rec, &frame(seed), "cam1", 0, 1);
        recorder.finish(rec, verdict, cycle, 10, u64::MAX, Vec::new());
    }
    let first = wait(&outcomes);
    let second = wait(&outcomes);
    assert!(first.available && second.available, "{:?} {:?}", first.errors, second.errors);
    assert_ne!(first.directory, second.directory);
    for (outcome, seed, verdict) in [(&first, 10, "OK"), (&second, 30, "NG_WIDTH")] {
        assert_eq!(metadata(outcome)["sn"], 42);
        assert_eq!(metadata(outcome)["verdict"], verdict);
        for file in &outcome.files {
            assert_eq!(*std::fs::read(recorder.root().join(&file.file)).unwrap().last().unwrap(), seed + file.view - 1);
        }
    }
}

#[test]
fn disabled_and_ng_only_ok_recordings_are_explicitly_unavailable() {
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    assert!(recorder.begin(RecordMode::Off, 42, recipe(), "cycle-off", None).is_none());
    let outcome = wait(&outcomes);
    assert_eq!(outcome.state, RecordingState::Off);
    assert!(!outcome.available);
    assert!(outcome.directory.is_none() && outcome.files.is_empty());
    assert!(!recorder.root().exists());
    for (cycle, verdict) in [("ng-only-ok", Verdict::Ok), ("ng-only-excursion", Verdict::OkWithExcursion)] {
        let mut rec = recorder.begin(RecordMode::Failed, 42, recipe(), cycle, None).unwrap();
        recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
        recorder.finish(rec, verdict, "合格件不保留", 10, u64::MAX, Vec::new());
        let outcome = wait(&outcomes);
        assert_eq!(outcome.state, RecordingState::NotRetained);
        assert!(!outcome.available);
        assert!(outcome.files.is_empty() && outcome.directory.is_none());
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    }
    let mut rec = recorder.begin(RecordMode::Failed, 42, recipe(), "ng-only-ng", None).unwrap();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    recorder.finish(rec, Verdict::NgWidth, "NG 保留", 10, u64::MAX, Vec::new());
    assert!(wait(&outcomes).available);
}

#[test]
fn queue_full_rejects_all_views_and_finish_bypasses_the_frame_limit() {
    let dir = TestDir::new();
    let (recorder, rx, _) = controlled(dir.root());
    recorder.queued.store(QUEUE, Ordering::Relaxed);
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "cycle-full", None).unwrap();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    assert_eq!(rec.dropped, 1);
    assert_eq!(rec.frames.len(), 3);
    assert!(rec.frames.iter().all(|frame| !frame.queued));
    recorder.finish(rec, Verdict::ErrInspect, "检测故障", 10, u64::MAX, Vec::new());
    let Msg::Finish(job) = rx.try_recv().unwrap() else { panic!("队列满也必须保留收尾消息") };
    let outcome = finish_recording(recorder.root(), job, WriteState::default());
    assert!(!outcome.available && outcome.files.is_empty());
    assert!(outcome.errors.iter().any(|error| error.contains("录制队列已满")));
    let meta = metadata(&outcome);
    assert_eq!(meta["droppedFrames"], 1);
    assert!(meta["frames"].as_array().unwrap().iter().all(|frame| frame["available"] == false));
}

#[test]
fn disk_failure_does_not_claim_originals_exist() {
    let dir = TestDir::new();
    std::fs::write(dir.root(), "blocked-root").unwrap();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "cycle-disk-error", None).unwrap();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "原始判定 OK", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert_eq!(outcome.state, RecordingState::Failed);
    assert!(!outcome.available && outcome.directory.is_none() && outcome.files.is_empty());
    assert!(outcome.errors.iter().any(|error| error.contains("创建录制目录失败")));
    assert_eq!(std::fs::read_to_string(dir.root()).unwrap(), "blocked-root");
}

#[test]
fn partial_view_write_reports_successes_but_marks_recording_incomplete() {
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "cycle-partial", None).unwrap();
    let mut frame = frame(10);
    frame.images[1] = Arc::new(FrameImage::new(1, 1, vec![10, 11]));
    recorder.frame(&mut rec, &frame, "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "原始判定 OK", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert_eq!(outcome.state, RecordingState::Incomplete);
    assert!(!outcome.available);
    assert_eq!(outcome.files.iter().map(|file| file.view).collect::<Vec<_>>(), [1, 3]);
    assert!(outcome.errors.iter().any(|error| error.contains("像素缓冲长度")));
    let meta = metadata(&outcome);
    assert_eq!(meta["verdict"], "OK");
    assert_eq!(meta["available"], false);
    assert_eq!(meta["frames"][1]["available"], false);
}

#[test]
fn metadata_write_failure_is_reported_even_when_images_were_written() {
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "cycle-meta-error", None).unwrap();
    std::fs::create_dir_all(rec.dir.join("part.json")).unwrap();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "原始判定 OK", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert!(!outcome.available);
    assert_eq!(outcome.files.len(), 3);
    assert!(outcome.errors.iter().any(|error| error.contains("part.json")));
}

#[test]
fn finish_decodes_all_views_without_matching_pixel_contents() {
    let dir = TestDir::new();
    let (recorder, rx, _) = controlled(dir.root());
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "cycle-tampered", None).unwrap();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    let Msg::Frames(group) = rx.try_recv().unwrap() else { panic!("需要原图消息") };
    let mut state = WriteState::default();
    for (meta, image) in group.images {
        state.results.insert((meta.k, meta.view), save_raw(&group.pending.join(&meta.file), &image));
    }
    std::fs::write(group.pending.join("k000_P1_cam1_v2.pgm"), b"P5\n1 1\n255\n\x99").unwrap();
    recorder.finish(rec, Verdict::Ok, "原始判定 OK", 10, u64::MAX, Vec::new());
    let Msg::Finish(job) = rx.try_recv().unwrap() else { panic!("需要收尾消息") };
    let outcome = finish_recording(recorder.root(), job, state);
    assert!(outcome.available, "{:?}", outcome.errors);
    assert_eq!(outcome.files.iter().map(|file| file.view).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(std::fs::read(recorder.root().join(&outcome.files[1].file)).unwrap(), b"P5\n1 1\n255\n\x99");
}

#[test]
fn finish_rejects_same_length_corrupt_or_resized_raw_image() {
    for replacement in [b"P9\n2 3\n255\nabcdef".as_slice(), b"P5\n3 2\n255\nabcdef".as_slice()] {
        let dir = TestDir::new();
        let (recorder, rx, _) = controlled(dir.root());
        let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "cycle-invalid-image", None).unwrap();
        let mut input = frame(10);
        input.images = (0..3).map(|view| Arc::new(FrameImage::new(2, 3, vec![10 + view; 6]))).collect();
        recorder.frame(&mut rec, &input, "cam1", 0, 1);
        let Msg::Frames(group) = rx.try_recv().unwrap() else { panic!("需要原图消息") };
        let mut state = WriteState::default();
        for (meta, image) in group.images {
            state.results.insert((meta.k, meta.view), save_raw(&group.pending.join(&meta.file), &image));
        }
        let replaced = group.pending.join("k000_P1_cam1_v2.pgm");
        assert_eq!(std::fs::metadata(&replaced).unwrap().len(), replacement.len() as u64);
        std::fs::write(&replaced, replacement).unwrap();
        recorder.finish(rec, Verdict::Ok, "原始判定 OK", 10, u64::MAX, Vec::new());
        let Msg::Finish(job) = rx.try_recv().unwrap() else { panic!("需要收尾消息") };
        let outcome = finish_recording(recorder.root(), job, state);
        assert!(!outcome.available);
        assert_eq!(outcome.state, RecordingState::Incomplete, "损坏视角拒绝可用，保留其余完整原图");
        assert_eq!(outcome.files.iter().map(|file| file.view).collect::<Vec<_>>(), [1, 3]);
        assert!(outcome.errors.iter().any(|error| error.contains("解码")), "{:?}", outcome.errors);
        let meta = metadata(&outcome);
        assert_eq!(meta["verdict"], "OK");
        assert_eq!(meta["reason"], "原始判定 OK");
        assert_eq!(meta["frames"][1]["available"], false);
    }
}

#[test]
fn disconnected_writer_still_finishes_and_emits_a_failure_outcome() {
    let dir = TestDir::new();
    let (recorder, rx, outcomes) = controlled(dir.root());
    drop(rx);
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "cycle-disconnected", None).unwrap();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    assert_eq!(recorder.queued.load(Ordering::Relaxed), 0);
    recorder.finish(rec, Verdict::ErrInspect, "原始测量故障", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert_eq!(outcome.state, RecordingState::Failed);
    assert!(!outcome.available && outcome.files.is_empty());
    assert!(outcome.errors.iter().any(|error| error.contains("收尾改为同步保存")));
    assert_eq!(metadata(&outcome)["verdict"], "ERR_INSPECT");
}

#[test]
fn invalid_cycles_missing_images_and_duplicate_shots_are_never_available() {
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    for cycle in ["", "../escape", "C:drive", "folder\\escape", "folder/escape"] {
        assert!(recorder.begin(RecordMode::All, 42, recipe(), cycle, None).is_none());
        let outcome = wait(&outcomes);
        assert_eq!(outcome.state, RecordingState::Failed);
        assert!(!outcome.available && outcome.directory.is_none());
    }
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "cycle-empty", None).unwrap();
    let mut missing = frame(10);
    missing.images.clear();
    recorder.frame(&mut rec, &missing, "cam1", 0, 1);
    recorder.finish(rec, Verdict::ErrInspect, "缺帧", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert!(!outcome.available && outcome.files.is_empty());
    assert_eq!(metadata(&outcome)["missingShots"].as_array().unwrap().len(), 1);

    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "cycle-duplicate", None).unwrap();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    recorder.frame(&mut rec, &frame(30), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "原始判定 OK", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert!(!outcome.available);
    assert!(outcome.errors.iter().any(|error| error.contains("拒绝覆盖原图")));
    for file in &outcome.files {
        assert_eq!(*std::fs::read(recorder.root().join(&file.file)).unwrap().last().unwrap(), 9 + file.view);
    }
}

#[test]
fn pruning_preserves_current_and_in_use_recordings() {
    let dir = TestDir::new();
    let paths: Vec<_> = (1..=3).map(|i| dir.root().join("20261010").join(format!("20261010_00000{i}_cycle_{i}"))).collect();
    for path in &paths {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("image.pgm"), [0; 100]).unwrap();
    }
    let errors = prune(&dir.root(), 1, 1, &[paths[1].clone(), paths[2].clone()]);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!paths[0].exists());
    assert!(paths[1].exists() && paths[2].exists());
}

#[cfg(windows)]
#[test]
fn locked_old_recording_does_not_make_complete_current_recording_unavailable() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = TestDir::new();
    let old = dir.root().join("20000101").join("20000101_000000_cycle_old");
    std::fs::create_dir_all(&old).unwrap();
    let locked_path = old.join("image.pgm");
    std::fs::write(&locked_path, "old-recording").unwrap();
    let lock = std::fs::OpenOptions::new().read(true).share_mode(3).open(&locked_path).unwrap();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "cycle-retention-error", None).unwrap();
    recorder.frame(&mut rec, &frame(21), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "本件完整", 1, 1, Vec::new());
    let outcome = wait(&outcomes);
    assert_eq!(outcome.state, RecordingState::Complete);
    assert!(outcome.available);
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert_eq!(outcome.files.len(), 3);
    assert!(outcome.retention_errors.iter().any(|error| error.contains("清理旧录制失败") && error.contains("cycle_old")), "{:?}", outcome.retention_errors);
    assert_eq!(std::fs::read_to_string(&locked_path).unwrap(), "old-recording");
    let meta = metadata(&outcome);
    assert_eq!(meta["available"], true);
    assert_eq!(meta["errors"], json!([]));
    assert_eq!(meta["retentionErrors"], json!(outcome.retention_errors));
    assert!(outcome.files.iter().all(|file| recorder.root().join(&file.file).is_file()));
    let frames = crate::replay::scan_frames(outcome.directory.as_ref().unwrap(), 3, 0).unwrap();
    assert_eq!(frames.len(), 1);
    let images = crate::replay::load_entry(&frames[0]).unwrap();
    assert_eq!(images.len(), 3);
    for (view, image) in images.iter().enumerate() {
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.pixels, [21 + view as u8]);
    }
    drop(lock);
    assert!(prune(&dir.root(), 1, 1, &[outcome.directory.unwrap()]).is_empty());
    assert!(!old.exists());
}

#[test]
fn existing_target_is_preserved_and_partial_files_remain_at_the_actual_path() {
    let dir = TestDir::new();
    let (recorder, rx, _) = controlled(dir.root());
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "cycle-existing", None).unwrap();
    let pending = rec.dir.clone();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    let Msg::Frames(group) = rx.try_recv().unwrap() else { panic!("需要原图消息") };
    let mut state = WriteState::default();
    for (meta, image) in group.images {
        state.results.insert((meta.k, meta.view), save_raw(&group.pending.join(&meta.file), &image));
    }
    recorder.finish(rec, Verdict::Ok, "原始判定 OK", 10, u64::MAX, Vec::new());
    let Msg::Finish(job) = rx.try_recv().unwrap() else { panic!("需要收尾消息") };
    let target = job.target.clone().unwrap();
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("sentinel"), "keep-original").unwrap();
    let outcome = finish_recording(recorder.root(), job, state);
    assert!(!outcome.available);
    assert_eq!(outcome.directory.as_ref(), Some(&pending));
    assert_eq!(outcome.files.len(), 3);
    assert!(outcome.files.iter().all(|file| file.file.starts_with("_pending/")));
    assert_eq!(std::fs::read_to_string(target.join("sentinel")).unwrap(), "keep-original");
}

#[test]
fn concurrent_producers_never_exceed_the_group_queue_limit() {
    let dir = TestDir::new();
    let (recorder, rx, _) = controlled(dir.root());
    let recorder = Arc::new(recorder);
    let producers: Vec<_> = (0..64)
        .map(|i| {
            let recorder = recorder.clone();
            std::thread::spawn(move || {
                let mut rec = recorder.begin(RecordMode::All, i, recipe(), &format!("cycle-{i}"), None).unwrap();
                recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
                rec.dropped
            })
        })
        .collect();
    let dropped: u32 = producers.into_iter().map(|producer| producer.join().unwrap()).sum();
    assert_eq!(recorder.queued.load(Ordering::Relaxed), QUEUE);
    assert_eq!(dropped, 64 - QUEUE as u32);
    let groups: Vec<_> = rx.try_iter().collect();
    assert_eq!(groups.len(), QUEUE);
    assert!(groups.iter().all(|message| matches!(message, Msg::Frames(group) if group.images.len() == 3)));
}

fn pending(root: &Path, cycle: &str) -> PathBuf {
    let path = root.join("_pending").join(format!("20261010_000000_000_cycle_{cycle}"));
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("image.pgm"), "unfinished-raw").unwrap();
    path
}

#[test]
fn pending_sweep_removes_orphans_but_preserves_history_and_active_recordings() {
    let dir = TestDir::new();
    let root = dir.root();
    let orphan = pending(&root, "orphan");
    let referenced = pending(&root, "referenced-partial");
    let (recorder, _, _) = controlled(root.clone());
    let current = recorder.begin(RecordMode::All, 42, recipe(), "active-current", None).unwrap();
    std::fs::create_dir_all(&current.dir).unwrap();
    std::fs::write(current.dir.join("image.pgm"), "active-raw").unwrap();
    let reference = referenced.clone();
    let protection: PendingProtection = Arc::new(move || Ok(vec![reference.clone()]));
    let errors = PendingCleaner::default().sweep(&root, &recorder.active, &protection, &[]);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!orphan.exists());
    assert_eq!(std::fs::read_to_string(referenced.join("image.pgm")).unwrap(), "unfinished-raw");
    assert_eq!(std::fs::read_to_string(current.dir.join("image.pgm")).unwrap(), "active-raw");
}

#[test]
fn pending_sweep_is_bounded_and_continues_past_its_first_batch() {
    let dir = TestDir::new();
    let root = dir.root();
    for i in 0..PENDING_SWEEP_LIMIT + 3 { pending(&root, &format!("orphan-{i}")); }
    let protection: PendingProtection = Arc::new(|| Ok(Vec::new()));
    let active = Mutex::new(HashSet::new());
    let mut cleaner = PendingCleaner::default();
    assert!(cleaner.sweep(&root, &active, &protection, &[]).is_empty());
    assert_eq!(std::fs::read_dir(root.join("_pending")).unwrap().count(), 3);
    assert!(cleaner.sweep(&root, &active, &protection, &[]).is_empty());
    assert_eq!(std::fs::read_dir(root.join("_pending")).unwrap().count(), 0);
}

#[test]
fn failed_history_guard_retains_pending_and_only_reports_retention_warnings() {
    let dir = TestDir::new();
    let root = dir.root();
    let orphan = pending(&root, "unproven-orphan");
    let (callback, outcomes) = callback();
    let (warn_tx, warnings) = channel();
    let recorder = Recorder::guarded(root, Some(callback), Arc::new(|| Err("历史引用读取失败".into())),
        Arc::new(move |errors| warn_tx.send(errors.to_vec()).unwrap()));
    let startup = warnings.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(startup.iter().any(|error| error.contains("历史引用读取失败")));
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "guard-error-complete", None).unwrap();
    recorder.frame(&mut rec, &frame(21), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "本件完整", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert!(outcome.available && outcome.state == RecordingState::Complete);
    assert!(outcome.errors.is_empty());
    assert!(outcome.retention_errors.iter().any(|error| error.contains("历史引用读取失败")));
    assert_eq!(metadata(&outcome)["retentionErrors"], json!(outcome.retention_errors));
    assert!(orphan.exists());
}

#[cfg(windows)]
#[test]
fn pending_sweep_never_follows_directory_or_subtree_junctions() {
    fn junction(link: &Path, target: &Path) {
        let output = std::process::Command::new("cmd.exe").args(["/D", "/C", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let dir = TestDir::new();
    let root = dir.root();
    let nested = pending(&root, "nested-link");
    let outside = dir.0.join("outside-records");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), "preserve-outside").unwrap();
    let link = root.join("_pending").join("20261010_000000_000_cycle_linked-part");
    junction(&link, &outside);
    let nested_link = nested.join("linked-child");
    junction(&nested_link, &outside);
    let protection: PendingProtection = Arc::new(|| Ok(Vec::new()));
    let active = Mutex::new(HashSet::new());
    let errors = PendingCleaner::default().sweep(&root, &active, &protection, &[]);
    assert!(errors.iter().any(|error| error.contains("包含链接")));
    assert!(nested.exists() && link.exists());
    assert_eq!(std::fs::read_to_string(outside.join("sentinel")).unwrap(), "preserve-outside");
    std::fs::remove_dir(&nested_link).unwrap();
    std::fs::remove_dir(&link).unwrap();
    std::fs::remove_dir_all(root.join("_pending")).unwrap();
    junction(&root.join("_pending"), &outside);
    let errors = PendingCleaner::default().sweep(&root, &active, &protection, &[]);
    assert!(errors.iter().any(|error| error.contains("包含链接")));
    assert_eq!(std::fs::read_to_string(outside.join("sentinel")).unwrap(), "preserve-outside");
    std::fs::remove_dir(root.join("_pending")).unwrap();
}

#[test]
fn pending_sweep_does_not_query_history_when_only_active_recordings_exist() {
    let dir = TestDir::new();
    let root = dir.root();
    let current = pending(&root, "active-only");
    let active = Mutex::new(HashSet::from([current.clone()]));
    let protection: PendingProtection = Arc::new(|| panic!("仅有活跃录制时不能查询全量历史"));
    assert!(PendingCleaner::default().sweep(&root, &active, &protection, &[]).is_empty());
    assert!(current.exists());
}

#[test]
fn unpromoted_finish_remains_protected_before_async_audit_persists_references() {
    let dir = TestDir::new();
    let root = dir.root();
    let (callback, outcomes) = callback();
    let recorder = Recorder::guarded(root.clone(), Some(callback), Arc::new(|| Ok(Vec::new())), Arc::new(|_| {}));
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "partial-before-audit", None).unwrap();
    let pending = rec.dir.clone();
    let target = root.join(&rec.name[..8]).join(format!("{}_OK_cycle_{}", rec.name, rec.cycle_id));
    std::fs::create_dir_all(&target).unwrap();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "保留部分证据", 10, u64::MAX, Vec::new());
    let partial = wait(&outcomes);
    assert_eq!(partial.state, RecordingState::Incomplete);
    assert_eq!(partial.directory.as_ref(), Some(&pending));
    assert!(partial.files.iter().all(|file| recorder.root().join(&file.file).is_file()));
    assert!(recorder.active.lock().unwrap().contains(&pending));
    let mut next = recorder.begin(RecordMode::All, 42, recipe(), "next-complete", None).unwrap();
    recorder.frame(&mut next, &frame(21), "cam1", 0, 1);
    recorder.finish(next, Verdict::Ok, "下一件完整", 10, u64::MAX, Vec::new());
    let complete = wait(&outcomes);
    assert!(complete.available);
    assert!(pending.exists());
}

#[test]
fn pending_cleanup_claim_is_exclusive_and_releases_the_registry_lock() {
    let path = PathBuf::from("pending-claim");
    let active = Mutex::new(HashSet::new());
    let claim = PendingClaim::acquire(&active, &path).unwrap();
    assert!(active.try_lock().unwrap().contains(&path));
    assert!(PendingClaim::acquire(&active, &path).is_none());
    drop(claim);
    assert!(!active.lock().unwrap().contains(&path));
}


#[test]
fn rolling_prune_preserves_durable_pending_recording_and_removes_unprotected_old_recording() {
    let dir = TestDir::new();
    let protected = dir.root().join("20000101/20000101_000000_cycle_durable");
    let disposable = dir.root().join("20000101/20000101_000001_cycle_old");
    for path in [&protected, &disposable] {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("image.pgm"), b"original").unwrap();
    }
    let guarded = protected.clone();
    let (callback, outcomes) = callback();
    let recorder = Recorder::guarded(dir.root(), Some(callback), Arc::new(move || Ok(vec![guarded.clone()])), Arc::new(|_| {}));
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "prune-new", None).unwrap();
    recorder.frame(&mut rec, &frame(21), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "完整", 1, 1, Vec::new());
    let outcome = wait(&outcomes);
    assert!(outcome.available);
    assert!(protected.join("image.pgm").is_file());
    assert!(!disposable.exists());
}

#[test]
fn failed_durable_guard_prevents_rolling_prune_of_old_evidence() {
    let dir = TestDir::new();
    let old = dir.root().join("20000101/20000101_000000_cycle_unresolved");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("image.pgm"), b"original").unwrap();
    let (callback, outcomes) = callback();
    let recorder = Recorder::guarded(dir.root(), Some(callback), Arc::new(|| Err("待入库日志损坏".into())), Arc::new(|_| {}));
    let mut rec = recorder.begin(RecordMode::All, 42, recipe(), "guard-new", None).unwrap();
    recorder.frame(&mut rec, &frame(21), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "完整", 1, 1, Vec::new());
    let outcome = wait(&outcomes);
    assert!(outcome.available);
    assert_eq!(std::fs::read(old.join("image.pgm")).unwrap(), b"original");
    assert!(outcome.retention_errors.iter().any(|error| error.contains("跳过滚动清理")));
}
