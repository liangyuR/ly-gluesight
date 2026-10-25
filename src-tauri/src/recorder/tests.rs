use super::*;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
const WRITER_HANG: Duration = Duration::from_secs(30);

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
    Arc::new(crate::recipe::samples_burst().build().unwrap())
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
        images: vec![Arc::new(FrameImage::new(1, 1, vec![seed]))],
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


fn wait(outcomes: &Receiver<RecordingOutcome>) -> RecordingOutcome {
    outcomes.recv_timeout(WRITER_HANG).expect("录制收尾必须产生结果")
}

fn metadata(outcome: &RecordingOutcome) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(outcome.directory.as_ref().unwrap().join("part.json")).unwrap()).unwrap()
}


/// 按名字排在今天之前的一件旧录制（滚动清理按目录名的时间戳排新旧），内含一个 `bytes` 字节的文件。
fn old_recording(root: &Path, i: u32, bytes: u64) -> PathBuf {
    let path = root.join(format!("20000101/20000101_00000{i}_000_OK_cycle_old-{i}"));
    std::fs::create_dir_all(&path).unwrap();
    std::fs::File::create(path.join("image.pgm")).unwrap().set_len(bytes).unwrap();
    path
}

#[test]
fn offline_retention_prunes_only_offline_recordings_and_never_real_raw_images() {
    let dir = TestDir::new();
    let offline_root = dir.0.join("records-offline");
    let (offline_callback, offline_outcomes) = callback();
    let recorders = Recorders::new(Recorder::new(dir.root(), None), Recorder::new(offline_root.clone(), Some(offline_callback)));
    // 真机 3 件：离线清理若误用到真机目录，无论按真机还是离线的保留数都会删掉最旧的
    let real: Vec<_> = (0..3).map(|i| old_recording(&dir.root(), i, 10)).collect();
    let offline: Vec<_> = (0..3).map(|i| old_recording(&offline_root, i, 10)).collect();
    let settings = CycleSettings { record_keep: 1, offline_record_keep: 2, ..CycleSettings::default() };
    let mut rec = recorders.begin(RecordSource::OfflineSynthetic, RecordMode::All, "SN-2", recipe(), "offline-new", None).unwrap();
    recorders.burst_frame(&mut rec, &frame(10), 0, 1);
    recorders.finish(rec, Verdict::Ok, "离线件", &settings);
    assert!(wait(&offline_outcomes).directory.unwrap().starts_with(&offline_root));
    assert_eq!(offline.iter().map(|d| d.is_dir()).collect::<Vec<_>>(), [false, false, true], "离线按离线保留数 2 清理");
    assert!(real.iter().all(|d| d.is_dir()), "离线录制超过保留数后真机原图被清理");
}


/// 段配方录制：录制队列已有 `queued` 帧时依次录 `frames`（(计划相机下标, seq)），写盘后收尾。
fn record_burst(dir: &TestDir, queued: usize, frames: &[(usize, u64)]) -> RecordingOutcome {
    let (recorder, rx, _) = controlled(dir.root());
    recorder.queued.store(queued, Ordering::Relaxed);
    let recipe = Arc::new(crate::recipe::samples_burst().build().unwrap());
    let mut rec = recorder.begin(RecordMode::All, "SN-B1", recipe, "cycle-burst", None).unwrap();
    for &(camera, seq) in frames {
        recorder.burst_frame(&mut rec, &frame(seq as u8), camera, seq);
    }
    recorder.finish(rec, Verdict::Ok, "原始判定 OK", 10, u64::MAX, Vec::new());
    let mut state = WriteState::default();
    loop {
        match rx.try_recv().expect("录制收尾必须有收尾消息") {
            Msg::BurstFrame(job) => {
                let key = FrameKey { camera: job.meta.camera, seq: job.meta.seq };
                state.frame_results.insert(key, save_raw(&job.pending.join(&job.meta.file), &job.image));
            }
            Msg::Finish(job) => return finish_recording(recorder.root(), job, state, None),
            _ => panic!("段配方录制只发帧与收尾消息"),
        }
    }
}

/// 规格 §3 缺帧可追溯：相机没出图的帧按 (camera, seq) 列进 part.json 的 missingFrames，录制本身仍完整；
/// D-SB-32 录制背压不悄悄丢证据：队满没录的帧列进 rawDropped，证据标 Incomplete，写成的帧照常按 (camera, seq) 关联。
#[test]
fn recording_with_missing_or_queue_dropped_frames_lists_them_by_camera_seq() {
    let all = [(0, 1), (1, 1), (2, 1), (0, 2), (1, 2), (2, 2)];
    let key = |(camera, seq): (usize, u64)| json!({ "camera": camera, "seq": seq });
    // (场景, 录制前队列里已有的帧数, 交给录制的帧, missingFrames, rawDropped, 证据状态)
    let cases: [(&str, usize, Vec<(usize, u64)>, Vec<(usize, u64)>, Vec<(usize, u64)>, RecordingState); 2] = [
        ("相机 2 第 2 帧没出图", 0, all.iter().copied().filter(|&f| f != (1, 2)).collect(), vec![(1, 2)], vec![], RecordingState::Complete),
        ("录制队列只剩 4 帧余量", BURST_QUEUE - 4, all.to_vec(), vec![], vec![(1, 2), (2, 2)], RecordingState::Incomplete),
    ];
    for (case, queued, frames, missing, dropped, state) in cases {
        let dir = TestDir::new();
        let outcome = record_burst(&dir, queued, &frames);
        assert_eq!(outcome.state, state, "{case}: {:?}", outcome.errors);
        let meta = metadata(&outcome);
        assert_eq!(meta["missingFrames"], json!(missing.into_iter().map(key).collect::<Vec<_>>()), "{case}");
        assert_eq!(meta["rawDropped"], json!(dropped.iter().copied().map(key).collect::<Vec<_>>()), "{case}");
        let written: Vec<_> = frames.iter().copied().filter(|frame| !dropped.contains(frame)).collect();
        let linked: Vec<_> = outcome.files.iter().map(|file| file.frame.map(|f| (f.camera, f.seq))).collect();
        assert_eq!(linked, written.into_iter().map(Some).collect::<Vec<_>>(), "{case}");
    }
}

/// 维护采集件是示教与标定的样本：自动清理跳过带保留标记的目录，也不让它们挤掉保留件数。
#[test]
fn maintenance_collect_recordings_survive_retention() {
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let kept: Vec<_> = (0..2).map(|i| { let path = old_recording(&dir.root(), i, 10); std::fs::write(path.join(KEEP_MARKER), b"").unwrap(); path }).collect();
    let pruned = old_recording(&dir.root(), 5, 10);
    let newest = old_recording(&dir.root(), 6, 10);
    let mut rec = recorder.begin(RecordMode::All, "SN-M", recipe(), "collect-new", None).unwrap();
    rec.annotate("partMode", serde_json::json!("maintenanceCollect"));
    recorder.burst_frame(&mut rec, &frame(10), 0, 1);
    recorder.finish(rec, Verdict::ErrInspect, "维护采集", 1, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert!(outcome.directory.as_ref().unwrap().join(KEEP_MARKER).is_file(), "维护采集件收尾时要打保留标记");
    assert_eq!(metadata(&outcome)["partMode"], "maintenanceCollect");
    assert!(kept.iter().all(|d| d.is_dir()), "保留数 1 时维护采集样本被清理");
    assert!(newest.is_dir() && !pruned.is_dir(), "普通旧录制照常按保留数清理，维护采集样本不占保留件数");
}
