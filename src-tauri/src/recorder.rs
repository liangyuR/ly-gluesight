//! 帧录制：整组视角写成 `{编号}_{序号}_v{视角}.pgm`，相机编号与拍照点、视角一并写入元数据。
//! 外加 `part.json`（配方快照、逐帧元数据、结果），目录可以直接给回放相机用。
//! 写盘在后台线程，队列满了丢帧计数，不拖慢检测节拍。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use chrono::Local;
use serde::Serialize;
use serde_json::json;

use crate::frame::{Frame, FrameImage};
use crate::judge::Verdict;
use crate::recipe::Recipe;
use crate::replay;
use crate::settings::RecordMode;

/// 排队等写盘的帧最多这么多，再多就丢帧计数（收尾消息不受限，不能丢）。
const QUEUE: usize = 48;

#[cfg(test)]
mod view_tests {
    use super::*;
    use crate::frame::CounterSource;

    #[test]
    fn triplets_are_queued_together_and_recorded_under_the_same_shot() {
        let (tx, rx) = channel();
        let recorder = Recorder { root: PathBuf::new(), tx, queued: Arc::new(AtomicUsize::new(0)) };
        let recipe = Arc::new(crate::recipe::samples().remove(1).build().unwrap());
        let mut recording = recorder.begin(RecordMode::All, 12, recipe).unwrap();
        for k in 0..4 {
            let frame = Frame { cam: 0, session: 1, counter: CounterSource::Synthetic,
                frame_counter: k as u64 + 10, trigger_counter: k as u64 + 10,
                lost_packets: 0, ts: k as i64, manual: false,
                images: (1..=3).map(|v| Arc::new(FrameImage::new(1, 1, vec![v]))).collect() };
            recorder.frame(&mut recording, &frame, "cam1", k);
        }
        assert_eq!(recording.seq, [4]);
        assert_eq!(recording.frames.len(), 12);
        for (k, group) in recording.frames.chunks_exact(3).enumerate() {
            assert!(group.iter().all(|m| m.k == k && m.seq == k as u32 + 1 && m.trigger_counter == k as u64 + 10));
            assert_eq!(group.iter().map(|m| m.view).collect::<Vec<_>>(), [1, 2, 3]);
        }
        assert_eq!(recorder.queued.load(Ordering::Relaxed), 4);
        for _ in 0..4 {
            let Msg::Frames { images } = rx.try_recv().unwrap() else { panic!("expected frame group") };
            assert_eq!(images.len(), 3);
            for (i, (path, image)) in images.iter().enumerate() {
                assert!(path.file_name().unwrap().to_str().unwrap().ends_with(&format!("_v{}.pgm", i + 1)));
                assert_eq!(image.pixels, vec![i as u8 + 1]);
            }
        }
        assert!(rx.try_recv().is_err());
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameMeta {
    k: usize,
    view: u8,
    cam: u8,
    /// 相机编号
    camera: String,
    seq: u32,
    file: String,
    ts: i64,
    frame_counter: u64,
    trigger_counter: u64,
}

enum Msg {
    Frames { images: Vec<(PathBuf, Arc<FrameImage>)> },
    Finish { pending: PathBuf, target: Option<PathBuf>, meta: serde_json::Value, keep: u32, max_bytes: u64, in_use: Vec<PathBuf> },
}

/// 正在录制的一件。
pub struct Recording {
    started: i64,
    dir: PathBuf,
    name: String,
    sn: u32,
    recipe: Arc<Recipe>,
    mode: RecordMode,
    seq: Vec<u32>,
    frames: Vec<FrameMeta>,
    dropped: u32,
}

pub struct Recorder {
    root: PathBuf,
    tx: Sender<Msg>,
    queued: Arc<AtomicUsize>,
}

impl Recorder {
    pub fn new(root: PathBuf) -> Self {
        let (tx, rx) = channel();
        let (r, q) = (root.clone(), Arc::new(AtomicUsize::new(0)));
        let q2 = q.clone();
        std::thread::Builder::new().name("frame-recorder".into()).spawn(move || writer(r, rx, q2)).ok();
        Self { root, tx, queued: q }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn begin(&self, mode: RecordMode, sn: u32, recipe: Arc<Recipe>) -> Option<Recording> {
        if mode == RecordMode::Off {
            return None;
        }
        let name = format!("{}_SN{sn}", Local::now().format("%Y%m%d_%H%M%S_%3f"));
        Some(Recording {
            started: ly_plc::now_ms(),
            dir: self.root.join("_pending").join(&name),
            name,
            sn,
            recipe,
            mode,
            seq: Vec::new(),
            frames: Vec::new(),
            dropped: 0,
        })
    }

    /// camera：拍这一帧的相机编号。
    pub fn frame(&self, rec: &mut Recording, f: &Frame, camera: &str, k: usize) {
        if f.images.is_empty() { return; }
        let cam = f.cam as usize;
        if rec.seq.len() <= cam {
            rec.seq.resize(cam + 1, 0);
        }
        rec.seq[cam] += 1;
        if self.queued.load(Ordering::Relaxed) >= QUEUE {
            rec.dropped += 1;
            return;
        }
        self.queued.fetch_add(1, Ordering::Relaxed);
        let metadata: Vec<_> = f.images.iter().enumerate().map(|(i, _)| {
            let view = i as u8 + 1;
            FrameMeta { k, view, cam: f.cam, camera: camera.to_string(), seq: rec.seq[cam],
                file: format!("{camera}_{:06}_v{view}.pgm", rec.seq[cam]), ts: f.ts,
                frame_counter: f.frame_counter, trigger_counter: f.trigger_counter }
        }).collect();
        let images = metadata.iter().zip(&f.images).map(|(m, image)| (rec.dir.join(&m.file), image.clone())).collect();
        if self.tx.send(Msg::Frames { images }).is_err() {
            self.queued.fetch_sub(1, Ordering::Relaxed);
            rec.dropped += 1;
            return;
        }
        rec.frames.extend(metadata);
    }

    /// in_use：回放相机正在用的目录，滚动删除时跳过。
    pub fn finish(&self, rec: Recording, verdict: Verdict, reason: &str, keep: u32, max_bytes: u64, in_use: Vec<PathBuf>) {
        let failed = !matches!(verdict, Verdict::Ok | Verdict::OkWithExcursion);
        let keep_this = rec.mode == RecordMode::All || failed;
        let tag = serde_json::to_value(verdict).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let target = keep_this.then(|| self.root.join(&rec.name[..8]).join(format!("{}_{tag}", rec.name)));
        let meta = json!({
            "startedTs": rec.started,
            "sn": rec.sn,
            "verdict": verdict,
            "reason": reason,
            "recipe": &*rec.recipe,
            "frames": rec.frames,
            "droppedFrames": rec.dropped,
        });
        // 收尾消息不能丢，否则临时目录留在那里；通道不限长，这里不会阻塞检测节拍
        let _ = self.tx.send(Msg::Finish { pending: rec.dir, target, meta, keep, max_bytes, in_use });
    }
}

fn writer(root: PathBuf, rx: Receiver<Msg>, queued: Arc<AtomicUsize>) {
    // 上次程序中途退出留下的半件录制：没有结果，也不计入保留件数与总大小
    let _ = std::fs::remove_dir_all(root.join("_pending"));
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Frames { images } => {
                for (path, image) in images {
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = replay::save_pgm(&path, &image);
                }
                queued.fetch_sub(1, Ordering::Relaxed);
            }
            Msg::Finish { pending, target, meta, keep, max_bytes, in_use } => {
                let _ = std::fs::create_dir_all(&pending);
                let _ = std::fs::write(pending.join("part.json"), serde_json::to_string_pretty(&meta).unwrap_or_default());
                match target {
                    Some(t) => {
                        if let Some(dir) = t.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = std::fs::rename(&pending, &t);
                    }
                    None => {
                        let _ = std::fs::remove_dir_all(&pending);
                    }
                }
                prune(&root, keep as usize, max_bytes, &in_use);
            }
        }
    }
}

fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
}

/// 只留最新的 keep 件、且总大小不超过 max_bytes（目录名以时间开头，按名字排序即按时间）。
fn prune(root: &Path, keep: usize, max_bytes: u64, in_use: &[PathBuf]) {
    let in_use: Vec<PathBuf> = in_use.iter().filter_map(|p| p.canonicalize().ok()).collect();
    let mut parts: Vec<PathBuf> = Vec::new();
    let Ok(days) = std::fs::read_dir(root) else { return };
    for day in days.flatten().map(|d| d.path()).filter(|p| p.is_dir() && !p.ends_with("_pending")) {
        if let Ok(rd) = std::fs::read_dir(&day) {
            parts.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
    }
    parts.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    let mut total = 0u64;
    for (i, p) in parts.iter().enumerate() {
        total += dir_bytes(p);
        // 最新的一件总是留着
        if i > 0 && (i >= keep || total > max_bytes) && !p.canonicalize().is_ok_and(|c| in_use.contains(&c)) {
            let _ = std::fs::remove_dir_all(p);
            if let Some(day) = p.parent() {
                let _ = std::fs::remove_dir(day);
            }
        }
    }
}
