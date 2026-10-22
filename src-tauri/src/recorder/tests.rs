use super::*;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
const WRITER_HANG: Duration = Duration::from_secs(600);

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
    (Recorder { root, tx, queued: Arc::new(AtomicUsize::new(0)), callback: Some(callback), startup_error: None, cleanup: None, active: Arc::new(Mutex::new(HashSet::new())), scope: HistoryScope::Real }, rx, outcomes)
}

fn cleanup(protection: PendingProtection, warning: RetentionWarning) -> Cleanup {
    Cleanup { protection, warning, release: Arc::new(|_| Ok(())) }
}

fn wait(outcomes: &Receiver<RecordingOutcome>) -> RecordingOutcome {
    outcomes.recv_timeout(WRITER_HANG).expect("录制收尾必须产生结果")
}

fn metadata(outcome: &RecordingOutcome) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(outcome.directory.as_ref().unwrap().join("part.json")).unwrap()).unwrap()
}

#[test]
fn successful_finish_reports_only_existing_files_and_complete_metadata() {
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let recipe = recipe();
    let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe.clone(), "cycle-complete", Some("frozen-bundle")).unwrap();
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
        let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe(), cycle, None).unwrap();
        recorder.frame(&mut rec, &frame(seed), "cam1", 0, 1);
        recorder.finish(rec, verdict, cycle, 10, u64::MAX, Vec::new());
    }
    let first = wait(&outcomes);
    let second = wait(&outcomes);
    assert!(first.available && second.available, "{:?} {:?}", first.errors, second.errors);
    assert_ne!(first.directory, second.directory);
    for (outcome, seed, verdict) in [(&first, 10, "OK"), (&second, 30, "NG_WIDTH")] {
        assert_eq!(metadata(outcome)["sn"], "SN-42");
        assert_eq!(metadata(outcome)["verdict"], verdict);
        for file in &outcome.files {
            assert_eq!(*std::fs::read(recorder.root().join(&file.file)).unwrap().last().unwrap(), seed + file.view - 1);
        }
    }
}

/// 根目录被占：创建目录失败，不能谎称原图存在，也不能动占位文件。
fn record_into_blocked_root(dir: &TestDir) -> RecordingOutcome {
    std::fs::write(dir.root(), "blocked-root").unwrap();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe(), "cycle-disk-error", None).unwrap();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "原始判定 OK", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert_eq!(outcome.state, RecordingState::Failed);
    assert!(!outcome.available && outcome.directory.is_none() && outcome.files.is_empty());
    assert_eq!(std::fs::read_to_string(dir.root()).unwrap(), "blocked-root");
    outcome
}

/// 队列满：一帧的所有视角都被拒，收尾消息不受帧上限限制。
fn record_with_full_queue(dir: &TestDir) -> RecordingOutcome {
    let (recorder, rx, _) = controlled(dir.root());
    recorder.queued.store(QUEUE, Ordering::Relaxed);
    let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe(), "cycle-full", None).unwrap();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    assert_eq!(rec.dropped, 1);
    assert_eq!(rec.frames.len(), 3);
    assert!(rec.frames.iter().all(|frame| !frame.queued));
    recorder.finish(rec, Verdict::ErrInspect, "检测故障", 10, u64::MAX, Vec::new());
    let Msg::Finish(job) = rx.try_recv().unwrap() else { panic!("队列满也必须保留收尾消息") };
    let outcome = finish_recording(recorder.root(), job, WriteState::default(), None);
    assert!(!outcome.available && outcome.files.is_empty());
    let meta = metadata(&outcome);
    assert_eq!(meta["droppedFrames"], 1);
    assert!(meta["frames"].as_array().unwrap().iter().all(|frame| frame["available"] == false));
    outcome
}

/// 取图问题：先到一帧缺视角，同一拍再到完整一帧。
fn record_incomplete_then_repeated_capture(dir: &TestDir) -> RecordingOutcome {
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe(), "cycle-capture", None).unwrap();
    let mut one_view = frame(10);
    one_view.images.truncate(1);
    recorder.frame(&mut rec, &one_view, "cam1", 0, 1);
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "原始判定 OK", 10, u64::MAX, Vec::new());
    wait(&outcomes)
}

#[test]
fn recording_failures_report_their_cause_and_only_disk_failures_are_write_errors() {
    // (场景, 制造故障并返回录制结果, 必须出现的错误, 是否归为写盘故障)
    let cases: [(&str, fn(&TestDir) -> RecordingOutcome, &[&str], bool); 3] = [
        ("根目录被占", record_into_blocked_root, &["创建录制目录失败"], true),
        ("队列满", record_with_full_queue, &["录制队列已满"], false),
        ("缺视角且重复到达", record_incomplete_then_repeated_capture, &["原图不完整", "拒绝覆盖原图"], false),
    ];
    for (case, record, expected_errors, write_error) in cases {
        let dir = TestDir::new();
        let outcome = record(&dir);
        for expected in expected_errors {
            assert!(outcome.errors.iter().any(|error| error.contains(expected)), "{case}: {:?}", outcome.errors);
        }
        assert_eq!(outcome.errors.iter().any(|error| is_write_error(error)), write_error, "{case}: {:?}", outcome.errors);
    }
}

#[test]
fn metadata_write_failure_is_reported_even_when_images_were_written() {
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe(), "cycle-meta-error", None).unwrap();
    std::fs::create_dir_all(rec.dir.join("part.json")).unwrap();
    recorder.frame(&mut rec, &frame(10), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "原始判定 OK", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert!(!outcome.available);
    assert_eq!(outcome.files.len(), 3);
    assert!(outcome.errors.iter().any(|error| error.contains("part.json")));
}

#[test]
fn finish_rejects_same_length_corrupt_or_resized_raw_image() {
    for replacement in [b"P9\n2 3\n255\nabcdef".as_slice(), b"P5\n3 2\n255\nabcdef".as_slice()] {
        let dir = TestDir::new();
        let (recorder, rx, _) = controlled(dir.root());
        let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe(), "cycle-invalid-image", None).unwrap();
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
        let outcome = finish_recording(recorder.root(), job, state, None);
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
fn invalid_cycles_missing_images_and_duplicate_shots_are_never_available() {
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    for cycle in ["", "../escape", "C:drive", "folder\\escape", "folder/escape"] {
        assert!(recorder.begin(RecordMode::All, "SN-42", recipe(), cycle, None).is_none());
        let outcome = wait(&outcomes);
        assert_eq!(outcome.state, RecordingState::Failed);
        assert!(!outcome.available && outcome.directory.is_none());
    }
    let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe(), "cycle-empty", None).unwrap();
    let mut missing = frame(10);
    missing.images.clear();
    recorder.frame(&mut rec, &missing, "cam1", 0, 1);
    recorder.finish(rec, Verdict::ErrInspect, "缺帧", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert!(!outcome.available && outcome.files.is_empty());
    assert_eq!(metadata(&outcome)["missingShots"].as_array().unwrap().len(), 1);

    let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe(), "cycle-duplicate", None).unwrap();
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
    let current = recorder.begin(RecordMode::All, "SN-42", recipe(), "active-current", None).unwrap();
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
fn failed_history_guard_retains_pending_and_only_reports_retention_warnings() {
    let dir = TestDir::new();
    let root = dir.root();
    let orphan = pending(&root, "unproven-orphan");
    let (callback, outcomes) = callback();
    let (warn_tx, warnings) = channel();
    let recorder = Recorder::guarded(root, Some(callback), cleanup(Arc::new(|| Err("历史引用读取失败".into())),
        Arc::new(move |errors| warn_tx.send(errors.to_vec()).unwrap())));
    let startup = warnings.recv_timeout(WRITER_HANG).unwrap();
    assert!(startup.iter().any(|error| error.contains("历史引用读取失败")));
    let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe(), "guard-error-complete", None).unwrap();
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
fn unpromoted_finish_remains_protected_before_async_audit_persists_references() {
    let dir = TestDir::new();
    let root = dir.root();
    let (callback, outcomes) = callback();
    let recorder = Recorder::guarded(root.clone(), Some(callback), cleanup(Arc::new(|| Ok(Vec::new())), Arc::new(|_| {})));
    let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe(), "partial-before-audit", None).unwrap();
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
    let mut next = recorder.begin(RecordMode::All, "SN-42", recipe(), "next-complete", None).unwrap();
    recorder.frame(&mut next, &frame(21), "cam1", 0, 1);
    recorder.finish(next, Verdict::Ok, "下一件完整", 10, u64::MAX, Vec::new());
    let complete = wait(&outcomes);
    assert!(complete.available);
    assert!(pending.exists());
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
    let recorder = Recorder::guarded(dir.root(), Some(callback), cleanup(Arc::new(move || Ok(vec![guarded.clone()])), Arc::new(|_| {})));
    let mut rec = recorder.begin(RecordMode::All, "SN-42", recipe(), "prune-new", None).unwrap();
    recorder.frame(&mut rec, &frame(21), "cam1", 0, 1);
    recorder.finish(rec, Verdict::Ok, "完整", 1, 1, Vec::new());
    let outcome = wait(&outcomes);
    assert!(outcome.available);
    assert!(protected.join("image.pgm").is_file());
    assert!(!disposable.exists());
}

#[test]
fn changed_offline_retention_prunes_next_recording_without_removing_real_raw_images() {
    let dir = TestDir::new();
    let (real_callback, real_outcomes) = callback();
    let (offline_callback, offline_outcomes) = callback();
    let recorders = Recorders::new(Recorder::new(dir.root(), Some(real_callback)), Recorder::new(dir.0.join("records-offline"), Some(offline_callback)));
    let settings_path = dir.0.join("cycle.json");
    let mut settings = CycleSettings { record_keep: 1, offline_record_keep: 3, ..CycleSettings::default() };
    let mut rec = recorders.begin(RecordSource::Real, RecordMode::All, "SN-1", recipe(), "real-cycle", None).unwrap();
    recorders.frame(&mut rec, &frame(1), "cam1", 0, 1);
    recorders.finish(rec, Verdict::Ok, "真机件", &settings);
    let real = wait(&real_outcomes).directory.unwrap();
    let mut offline = Vec::new();
    for (i, source) in [RecordSource::OfflineSample, RecordSource::OfflineSynthetic, RecordSource::OfflineSample, RecordSource::OfflineSample].into_iter().enumerate() {
        if i == 3 {
            assert!(offline.iter().all(|d: &PathBuf| d.is_dir()));
            settings.offline_record_keep = 2;
            settings.validate().unwrap();
            settings.save(&settings_path).unwrap();
            let (saved, note) = CycleSettings::load(&settings_path);
            assert!(note.is_none());
            settings = saved;
        }
        std::thread::sleep(Duration::from_millis(5));
        let mut rec = recorders.begin(source, RecordMode::All, "SN-2", recipe(), &format!("offline-{i}"), None).unwrap();
        recorders.frame(&mut rec, &frame(10), "cam1", 0, 1);
        recorders.finish(rec, Verdict::Ok, "离线件", &settings);
        offline.push(wait(&offline_outcomes).directory.unwrap());
    }
    assert!(real.join("part.json").is_file(), "离线录制超过保留数后真机原图被清理");
    assert!(real.starts_with(recorders.root(RecordSource::Real)));
    assert_eq!(offline.iter().map(|d| d.is_dir()).collect::<Vec<_>>(), [false, false, true, true]);
    assert!(offline.iter().all(|d| d.starts_with(recorders.root(RecordSource::OfflineSample))));

    std::fs::File::create(offline[2].join("capacity-fixture.pgm")).unwrap().set_len(500_000_000).unwrap();
    settings.offline_record_keep = 100;
    settings.offline_record_max_gb = 0.5;
    settings.validate().unwrap();
    settings.save(&settings_path).unwrap();
    let (settings, note) = CycleSettings::load(&settings_path);
    assert!(note.is_none());
    let mut rec = recorders.begin(RecordSource::OfflineSynthetic, RecordMode::All, "SN-3", recipe(), "offline-capacity", None).unwrap();
    recorders.frame(&mut rec, &frame(20), "cam1", 0, 1);
    recorders.finish(rec, Verdict::Ok, "离线容量限制", &settings);
    let latest = wait(&offline_outcomes);
    assert!(latest.available);
    assert!(!offline[2].exists());
    assert!(offline[3].join("part.json").is_file());
    assert!(real.join("part.json").is_file());
}

#[test]
fn applying_lower_retention_prunes_both_roots_without_next_part_and_keeps_protected_recording() {
    let dir = TestDir::new();
    let offline_root = dir.0.join("records-offline");
    let part = |root: &Path, i: u32| {
        let path = root.join(format!("20000101/20000101_00000{i}_000_cycle_p{i}"));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("image.pgm"), b"raw").unwrap();
        path
    };
    let real: Vec<_> = (0..3).map(|i| part(&dir.root(), i)).collect();
    let offline: Vec<_> = (0..3).map(|i| part(&offline_root, i)).collect();
    let guarded = real[0].clone();
    let recorders = Recorders::new(
        Recorder::guarded(dir.root(), None, cleanup(Arc::new(move || Ok(vec![guarded.clone()])), Arc::new(|_| {}))),
        Recorder::new(offline_root, None));
    recorders.apply_retention(&CycleSettings { record_keep: 1, offline_record_keep: 2, ..CycleSettings::default() });
    let present = |paths: &[PathBuf]| paths.iter().map(|p| p.is_dir()).collect::<Vec<_>>();
    let deadline = std::time::Instant::now() + WRITER_HANG;
    while (present(&real), present(&offline)) != (vec![true, false, true], vec![false, true, true]) {
        assert!(std::time::Instant::now() < deadline, "保存保留设置后未按新上限清理：real={:?} offline={:?}", present(&real), present(&offline));
        std::thread::sleep(Duration::from_millis(10));
    }
}
