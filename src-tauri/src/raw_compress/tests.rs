use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use super::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "gluesight-raw-compress-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 一张 32 × 16 的彩色录制原图（带 Bayer 注释的 P5），内容随 `seed` 不同。
fn raw(seed: u8) -> Vec<u8> {
    let mut bytes = b"P5\n# bayer RG\n32 16\n255\n".to_vec();
    bytes.extend((0..32 * 16u32).map(|i| ((i % 32) * 3 + (i / 32) * 5) as u8 ^ seed));
    bytes
}

/// 已收尾的录制目录 `<root>/20261110/20261110_120000_001_OK_cycle_<cycle>`，里面三帧原图。
fn part(root: &Path, cycle: &str) -> (PathBuf, Vec<(PathBuf, Vec<u8>)>) {
    let dir = root
        .join("20261110")
        .join(format!("20261110_120000_001_OK_cycle_{cycle}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("part.json"), "{}").unwrap();
    let frames = (1..=3u8)
        .map(|seq| {
            let path = dir.join(format!("cam1_s{seq:06}.pgm"));
            std::fs::write(&path, raw(seq)).unwrap();
            (path, raw(seq))
        })
        .collect();
    (dir, frames)
}

fn compress(
    dir: &Path,
    cycle: &str,
    activity: &Activity,
    quiet: Duration,
    external: bool,
) -> PartStats {
    compress_part(
        dir,
        cycle,
        activity,
        quiet,
        &|| external,
        &mut Memo::default(),
    )
    .0
}

#[test]
fn leftovers_of_interrupted_compression_never_cost_the_only_readable_original() {
    // 防的回归：压缩中途断电、被打断或写坏后，收拾残留时删掉了唯一能读的那份原图，或留下损坏的 .gsr 让 .pgm 被当成多余的删掉
    let original = raw(1);
    let good = gsr::encode(&original).unwrap();
    let mut damaged = good.clone();
    let last = damaged.len() - 1;
    damaged[last] ^= 0x55;
    let other = gsr::encode(&raw(2)).unwrap();
    // （场景，同名 .gsr 的内容，有没有 .pgm，有没有遗留的 .tmp，收拾后剩 .pgm，收拾后剩 .gsr）
    let cases: [(&str, Option<&[u8]>, bool, bool, bool, bool); 6] = [
        ("只遗留临时文件", None, true, true, true, false),
        ("并存且一致", Some(&good), true, false, false, true),
        (
            "并存但压缩文件损坏",
            Some(&damaged),
            true,
            false,
            true,
            false,
        ),
        (
            "并存但压缩文件是别的内容",
            Some(&other),
            true,
            true,
            true,
            false,
        ),
        ("只有压缩文件", Some(&good), false, false, false, true),
        (
            "只有损坏的压缩文件",
            Some(&damaged),
            false,
            false,
            false,
            true,
        ),
    ];
    for (name, stored, plain, temporary_left, keeps_plain, keeps_stored) in cases {
        let temp = TempDir::new("leftovers");
        let pgm = temp.0.join("cam1_s000001.pgm");
        let (compressed, tmp) = (pgm.with_extension("gsr"), temporary(&pgm));
        if plain {
            std::fs::write(&pgm, &original).unwrap();
        }
        if let Some(stored) = stored {
            std::fs::write(&compressed, stored).unwrap();
        }
        if temporary_left {
            std::fs::write(&tmp, &good[..good.len() / 2]).unwrap();
        }
        recover(&temp.0);
        assert_eq!(
            (pgm.exists(), compressed.exists(), tmp.exists()),
            (keeps_plain, keeps_stored, false),
            "{name}"
        );
        if name != "只有损坏的压缩文件" {
            assert_eq!(
                crate::image_file::read_original(&pgm).unwrap(),
                original,
                "{name}"
            );
        }
    }

    // 单帧流程中途失败（临时文件写不了）：原文件原样保留，不留下压缩文件
    let temp = TempDir::new("failed");
    let pgm = temp.0.join("cam1_s000001.pgm");
    std::fs::write(&pgm, &original).unwrap();
    std::fs::create_dir(temporary(&pgm)).unwrap();
    assert!(matches!(compress_frame(&pgm), FrameOutcome::Failed(_)));
    assert_eq!(std::fs::read(&pgm).unwrap(), original);
    assert!(!pgm.with_extension("gsr").exists());
}

#[test]
fn compression_starts_only_after_business_stops_and_stays_quiet() {
    // 防的回归：有登记的任务、在途业务，或业务刚停还没满空闲时间就开始压缩，与采集、复测、导入导出抢盘
    let temp = TempDir::new("idle");
    let (dir, frames) = part(&temp.0, "c1");
    let activity = Activity::default();
    let untouched = |why: &str, stats: PartStats| {
        assert!(stats.interrupted && stats.frames == 0, "{why}");
        assert!(frames.iter().all(|(path, _)| path.exists()), "{why}");
    };
    let busy = activity.busy();
    untouched(
        "有登记的任务",
        compress(&dir, "c1", &activity, Duration::ZERO, false),
    );
    drop(busy);
    untouched(
        "业务刚停",
        compress(&dir, "c1", &activity, Duration::from_secs(3600), false),
    );
    untouched(
        "有在途业务",
        compress(&dir, "c1", &activity, Duration::ZERO, true),
    );

    let stats = compress(&dir, "c1", &activity, Duration::ZERO, false);
    assert_eq!(
        (stats.frames, stats.failed, stats.interrupted),
        (3, 0, false)
    );
    assert!(stats.after < stats.before);
    for (path, original) in &frames {
        assert!(!path.exists() && path.with_extension("gsr").exists());
        assert_eq!(&crate::image_file::read_original(path).unwrap(), original);
    }
}

#[test]
fn unfinished_or_uncommitted_recordings_are_not_compressed() {
    // 防的回归：正在录制（_pending）、没写完 part.json、导入还没提交的目录被压缩，录制收尾核对或导入回滚看到的文件就变了
    let temp = TempDir::new("eligible");
    let activity = Activity::default();
    let (pending, _) = part(&temp.0.join("_pending-root"), "c1");
    let moved = temp.0.join("_pending").join(pending.file_name().unwrap());
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&pending, &moved).unwrap();
    let (unfinished, _) = part(&temp.0.join("a"), "c2");
    std::fs::remove_file(unfinished.join("part.json")).unwrap();
    let (importing, _) = part(&temp.0.join("b"), "c3");
    std::fs::write(importing.join(crate::history_package::PENDING), "p").unwrap();
    let (other, _) = part(&temp.0.join("c"), "c4");
    for (why, dir, cycle) in [
        ("正在录制", &moved, "c1"),
        ("没有 part.json", &unfinished, "c2"),
        ("导入未提交", &importing, "c3"),
        ("目录不是这件的", &other, "c5"),
    ] {
        let stats = compress(dir, cycle, &activity, Duration::ZERO, false);
        assert_eq!(stats.frames, 0, "{why}");
        let plain = std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| has_extension(&e.as_ref().unwrap().path(), "pgm"))
            .count();
        assert_eq!(plain, 3, "{why}");
    }
}

#[test]
fn registering_business_waits_for_the_frame_in_progress() {
    // 防的回归：清理、导出登记后不等手上这一帧做完就动目录，与压缩线程的改名、删除撞在一起
    let activity = Arc::new(Activity::default());
    let released = Arc::new(AtomicBool::new(false));
    let permit = activity.frame(Duration::ZERO).unwrap();
    let (worker_activity, worker_released) = (activity.clone(), released.clone());
    let registered = std::thread::spawn(move || {
        let _busy = worker_activity.busy();
        worker_released.load(Ordering::SeqCst)
    });
    std::thread::sleep(Duration::from_millis(50));
    released.store(true, Ordering::SeqCst);
    drop(permit);
    assert!(registered.join().unwrap(), "登记应等到这一帧做完");
}
