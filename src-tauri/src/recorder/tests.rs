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
        dev_ts: None,
        lost_packets: 7,
        ts: 777,
        manual: false,
        images: vec![Arc::new(FrameImage::new(1, 1, vec![seed]))],
        bayer: None,
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

/// 防的回归：彩色帧的主文件是带 `# bayer BG` 注释的原始马赛克，写盘核对按固定的 `P5\n宽 高\n255\n` 头算长度时
/// 每帧都判写入失败、整件录制不可用；主文件写成 RGB 或另写 `_bayer.pgm` 时与读者约定不符，读回不是同一幅彩色图。
#[test]
fn color_frame_records_bayer_mosaic_as_main_file_and_reads_back_rgb() {
    use crate::frame::{BayerMosaic, BayerPattern};
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let mut rec = recorder
        .begin(RecordMode::All, "SN-COLOR", recipe(), "color", None)
        .unwrap();
    // BG 相位纯色马赛克：R=200 G=100 B=50
    let mosaic = vec![50, 100, 100, 200];
    let frame = Frame {
        images: vec![Arc::new(FrameImage::rgb(2, 2, [200, 100, 50].repeat(4)))],
        bayer: Some(BayerMosaic {
            pattern: BayerPattern::Bg,
            image: Arc::new(FrameImage::new(2, 2, mosaic.clone())),
        }),
        ..frame(0)
    };
    recorder.burst_frame(&mut rec, &frame, 0, 1);
    recorder.finish(rec, Verdict::Ok, "彩色", 100, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert_eq!(
        (outcome.state, outcome.available),
        (RecordingState::Complete, true),
        "{:?}",
        outcome.errors
    );
    let directory = outcome.directory.clone().unwrap();
    let main = directory.join(outcome.files[0].file.rsplit('/').next().unwrap());
    let mut expected = b"P5\n# bayer BG\n2 2\n255\n".to_vec();
    expected.extend_from_slice(&mosaic);
    assert_eq!(std::fs::read(&main).unwrap(), expected);
    let meta = metadata(&outcome);
    let entry = &meta["frames"][0];
    assert_eq!(entry["imageMode"], "color");
    assert!(entry.get("bayer").is_none());
    assert!(!std::fs::read_dir(&directory).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with("_bayer.pgm")));
    let image = crate::image_file::load(&main).unwrap();
    assert_eq!(
        (image.channels, image.pixels.clone()),
        (3, [200, 100, 50].repeat(4))
    );
}

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

/// D-SB-60：录制关闭时要测图的件写临时录制。防的回归：收尾补测读不到原图（只能记检测不完整）；或录制关闭了，
/// 临时原图却留在盘上（NG 件按“只留 NG”被留下），录制结果也不再报“录制关闭”。
#[test]
fn scratch_recording_is_readable_during_the_part_and_never_kept() {
    for verdict in [Verdict::Ok, Verdict::NgGap] {
        let dir = TestDir::new();
        let (callback, outcomes) = callback();
        let recorder = Recorder::new(dir.root(), Some(callback));
        let mut rec = recorder.scratch("SN-S", recipe(), "scratch", None).unwrap();
        recorder.burst_frame(&mut rec, &frame(42), 0, 1);
        // 写完之前报“还在写”，写完才给可读的原图文件：补测按它等，不会读到半截文件
        let deadline = std::time::Instant::now() + WRITER_HANG;
        let file = loop {
            match rec.burst_frame_file(0, 1) {
                RecordedFrame::Ready(file) => break file,
                RecordedFrame::Writing => {}
                RecordedFrame::Missing => panic!("{verdict:?}：排队写盘的帧不能报没录"),
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{verdict:?}：临时原图写不出来"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        let image = crate::image_file::load(&file).expect("报已写完的原图必须读得出来");
        assert_eq!(image.pixels, vec![42], "{verdict:?}");
        assert_eq!(
            rec.burst_frame_file(0, 2),
            RecordedFrame::Missing,
            "没录的帧"
        );
        recorder.finish(rec, verdict, "", 10, u64::MAX, Vec::new());
        let outcome = wait(&outcomes);
        assert_eq!(
            (outcome.state, outcome.directory),
            (RecordingState::Off, None),
            "{verdict:?}"
        );
        let left: Vec<_> = walk(&dir.root());
        assert!(
            left.is_empty(),
            "{verdict:?}：录制关闭却留下了原图 {left:?}"
        );
    }
    // 防的回归：临时录制写盘失败（盘满、无权限）时文件不留，录制结果却报健康，盘故障报警被清掉而补测一直读不到原图
    let dir = TestDir::new();
    let (callback, outcomes) = callback();
    let recorder = Recorder::new(dir.root(), Some(callback));
    let mut rec = recorder
        .scratch("SN-F", recipe(), "scratch-fail", None)
        .unwrap();
    std::fs::create_dir_all(rec.dir.parent().unwrap()).unwrap();
    std::fs::write(&rec.dir, b"").unwrap();
    recorder.burst_frame(&mut rec, &frame(7), 0, 1);
    let deadline = std::time::Instant::now() + WRITER_HANG;
    while rec.burst_frame_file(0, 1) == RecordedFrame::Writing {
        assert!(std::time::Instant::now() < deadline, "写盘结果没回来");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        rec.burst_frame_file(0, 1),
        RecordedFrame::Missing,
        "写盘失败的帧补测不能读"
    );
    std::fs::remove_file(&rec.dir).unwrap();
    recorder.finish(rec, Verdict::Ok, "", 10, u64::MAX, Vec::new());
    let outcome = wait(&outcomes);
    assert_eq!(outcome.state, RecordingState::Off);
    assert!(
        outcome.errors.iter().any(|e| is_write_error(e)),
        "写盘失败没报出来：{:?}",
        outcome.errors
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .flat_map(|e| {
                    if e.path().is_dir() {
                        walk(&e.path())
                    } else {
                        vec![e.path()]
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}
