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

/// 维护采集件是示教与标定的样本：自动清理跳过带保留标记的目录，也不让它们挤掉保留件数。
#[test]
fn maintenance_collect_recordings_survive_retention() {
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let kept: Vec<_> = (0..2)
        .map(|i| {
            let path = old_recording(&dir.root(), i, 10);
            std::fs::write(path.join(KEEP_MARKER), b"").unwrap();
            path
        })
        .collect();
    let pruned = old_recording(&dir.root(), 5, 10);
    let newest = old_recording(&dir.root(), 6, 10);
    let mut rec = recorder
        .begin(RecordMode::All, "SN-M", recipe(), "collect-new", None)
        .unwrap();
    rec.annotate("partMode", serde_json::json!("maintenanceCollect"));
    recorder.burst_frame(&mut rec, &frame(10), 0, 1);
    recorder.finish(
        rec,
        Verdict::ErrInspect,
        "维护采集",
        1,
        u64::MAX,
        Vec::new(),
    );
    let outcome = wait(&outcomes);
    assert!(
        outcome
            .directory
            .as_ref()
            .unwrap()
            .join(KEEP_MARKER)
            .is_file(),
        "维护采集件收尾时要打保留标记"
    );
    assert_eq!(metadata(&outcome)["partMode"], "maintenanceCollect");
    assert!(
        kept.iter().all(|d| d.is_dir()),
        "保留数 1 时维护采集样本被清理"
    );
    assert!(
        newest.is_dir() && !pruned.is_dir(),
        "普通旧录制照常按保留数清理，维护采集样本不占保留件数"
    );
}
