//! 回放相机：三目帧按 `Frame{序号}_{视角}` 或 `{相机}_{序号}_v{视角}` 聚合。
//! 单目兼容旧录制 `cam{设备}_{序号}` 与按自然顺序排列的普通图片。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::vision::FrameImage;

const EXTS: [&str; 7] = ["pgm", "jpg", "jpeg", "png", "bmp", "tif", "tiff"];

struct Entry {
    channel: Option<u32>,
    camera: Option<String>,
    view: Option<u32>,
    seq: u64,
    name: String,
    path: PathBuf,
}

fn digits(s: &str) -> Option<u64> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

/// 文件名（不含扩展名）→ (通道, 序号)。通道从 1 开始。
fn parse(stem: &str) -> Option<(u32, u64)> {
    let s = stem.to_ascii_lowercase();
    if let Some(rest) = s.strip_prefix("cam") {
        let (c, n) = rest.split_once('_')?;
        return Some((u32::try_from(digits(c)?).ok()?, digits(n)?));
    }
    if let Some(rest) = s.strip_prefix("frame") {
        let (n, c) = rest.split_once('_')?;
        return Some((u32::try_from(digits(c)?).ok()?, digits(n)?));
    }
    None
}

/// 列出目录里属于某通道（从 1 开始；0 表示不分通道）的帧，按序号排好。
fn entries(dir: &Path) -> Result<Vec<Entry>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("读不了回放目录 {}：{e}", dir.display()))?;
    let mut entries = Vec::new();
    for e in rd {
        let e = e.map_err(|e| format!("读取回放目录项失败：{e}"))?;
        let path = e.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase) else { continue };
        if !EXTS.contains(&ext.as_str()) {
            continue;
        }
        let stem = path.file_stem().and_then(|x| x.to_str()).unwrap_or_default().to_string();
        let recorded = stem.rsplit_once("_v").and_then(|(prefix, v)| {
            let (camera, n) = prefix.rsplit_once('_')?;
            (!camera.is_empty()).then_some((camera.to_string(), digits(n)?, u32::try_from(digits(v)?).ok()?))
        });
        let (channel, camera, view, seq) = match recorded {
            Some((camera, n, v)) => (None, Some(camera), Some(v), n),
            None => match parse(&stem) {
                Some((c, n)) => (Some(c), None, None, n),
                None => (None, None, None, stem.bytes().filter(u8::is_ascii_digit).fold(0u64, |a, b| a.saturating_mul(10).saturating_add((b - b'0') as u64))),
            },
        };
        entries.push(Entry { channel, camera, view, seq, name: stem, path });
    }
    Ok(entries)
}

fn recorded_camera(entries: &[Entry], channel: u32) -> Result<String, String> {
    let mut cameras: Vec<_> = entries.iter().filter_map(|e| e.camera.as_deref()).collect();
    cameras.sort_by(|a, b| {
        let number = |s: &str| s.strip_prefix("cam").and_then(digits);
        number(a).unwrap_or(u64::MAX).cmp(&number(b).unwrap_or(u64::MAX)).then_with(|| a.cmp(b))
    });
    cameras.dedup();
    let chosen = if channel == 0 {
        cameras.first().copied()
    } else {
        let id = format!("cam{channel}");
        cameras.iter().copied().find(|c| *c == id).or_else(|| {
            cameras.get(channel as usize - 1).copied().filter(|c| c.strip_prefix("cam").and_then(digits).is_none())
        })
    };
    chosen.map(str::to_string).ok_or_else(|| format!("回放目录里没有设备 {channel}（可用设备：{}）", cameras.join("、")))
}

pub fn scan(dir: &Path, channel: u32) -> Result<Vec<PathBuf>, String> {
    let mut entries = entries(dir)?;
    if entries.iter().any(|e| e.camera.is_some()) {
        let camera = recorded_camera(&entries, channel)?;
        entries.retain(|e| e.camera.as_deref() == Some(camera.as_str()) && e.view == Some(1));
    }
    let named = entries.iter().any(|e| e.channel.is_some());
    if named && channel > 0 {
        entries.retain(|e| e.channel == Some(channel));
    } else if named {
        let first = entries.iter().filter_map(|e| e.channel).min();
        entries.retain(|e| e.channel == first);
    }
    entries.sort_by(|a, b| a.seq.cmp(&b.seq).then_with(|| a.name.cmp(&b.name)));
    if entries.is_empty() {
        return Err(if channel > 0 { format!("回放目录里没有通道 {channel} 的图片") } else { "回放目录里没有图片".into() });
    }
    Ok(entries.into_iter().map(|e| e.path).collect())
}

pub fn scan_views(dir: &Path, view_count: u8, channel: u32) -> Result<Vec<Vec<PathBuf>>, String> {
    if view_count == 1 {
        return scan(dir, channel).map(|files| files.into_iter().map(|file| vec![file]).collect());
    }
    if view_count != 3 {
        return Err("视角数量只能是 1 或 3".into());
    }
    let mut entries = entries(dir)?;
    if entries.is_empty() {
        return Err("回放目录里没有图片".into());
    }
    if let Some(entry) = entries.iter().find(|e| e.channel.is_some() && e.name.to_ascii_lowercase().starts_with("cam")) {
        return Err(format!("三目回放不能使用旧录制 {}：cam 编号代表独立设备，不能按相同序号合并为同一次触发", entry.path.display()));
    }
    let recorded = entries.iter().any(|e| e.camera.is_some());
    if recorded {
        if entries.iter().any(|e| e.camera.is_none()) {
            return Err("三目回放目录混用了录制与其他图片命名，请按设备和帧序号整理".into());
        }
        let camera = recorded_camera(&entries, channel)?;
        entries.retain(|e| e.camera.as_deref() == Some(camera.as_str()));
    }
    let mut groups: BTreeMap<u64, [Option<PathBuf>; 3]> = BTreeMap::new();
    for entry in entries {
        let view = if recorded { entry.view } else { entry.channel };
        let Some(view @ 1..=3) = view else {
            return Err(format!("三目回放图片 {} 的视角无效，需要视角 1、2、3", entry.path.display()));
        };
        let group = groups.entry(entry.seq).or_default();
        let slot = &mut group[view as usize - 1];
        if let Some(existing) = slot {
            return Err(format!("回放帧 {} 的视角 {view} 重复：{}、{}", entry.seq, existing.display(), entry.path.display()));
        }
        *slot = Some(entry.path);
    }
    groups.into_iter().map(|(seq, files)| {
        files.into_iter().enumerate().map(|(i, file)| file.ok_or_else(|| format!("回放帧 {seq} 缺少视角 {}，三目帧必须同时包含视角 1、2、3", i + 1))).collect()
    }).collect()
}

pub fn timeline_views(dir: &Path, files: &[Vec<PathBuf>]) -> Result<Option<Vec<i64>>, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Meta {
        started_ts: i64,
        frames: Vec<FrameTs>,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FrameTs {
        file: String,
        ts: i64,
        view: Option<u8>,
        camera: Option<String>,
        cam: Option<u8>,
        seq: Option<u64>,
        k: Option<u64>,
        frame_counter: Option<u64>,
        trigger_counter: Option<u64>,
        session: Option<u64>,
        counter: Option<serde_json::Value>,
        cycle_id: Option<serde_json::Value>,
    }
    let path = dir.join("part.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读不了回放元数据 {}：{e}", path.display())),
    };
    let meta: Meta = serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).map_err(|e| format!("回放元数据 {} 损坏：{e}", path.display()))?;
    let mut by_file = BTreeMap::new();
    for frame in &meta.frames {
        if by_file.insert(frame.file.as_str(), frame).is_some() {
            return Err(format!("回放元数据 part.json 中 {} 的条目重复", frame.file));
        }
    }
    let times: Result<Vec<_>, String> = files.iter().map(|views| {
        let group: Result<Vec<_>, String> = views.iter().map(|file| {
            let name = file.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("回放文件名无法识别：{}", file.display()))?;
            by_file.get(name).copied().ok_or_else(|| format!("回放元数据 part.json 缺少 {name} 的条目"))
        }).collect();
        let group = group?;
        let first = group.first().ok_or("回放帧没有图像")?;
        for (index, frame) in group.iter().enumerate() {
            let checks = [
                (frame.ts == first.ts, "曝光时刻"),
                (frame.camera == first.camera, "设备编号 camera"),
                (frame.cam == first.cam, "设备序号 cam"),
                (frame.seq == first.seq, "帧序号 seq"),
                (frame.k == first.k, "拍照点 k"),
                (frame.frame_counter == first.frame_counter, "帧计数 frameCounter"),
                (frame.trigger_counter == first.trigger_counter, "触发计数 triggerCounter"),
                (frame.session == first.session, "设备会话 session"),
                (frame.counter == first.counter, "计数来源 counter"),
                (frame.cycle_id == first.cycle_id, "工件 cycleId"),
                (frame.view.is_some() == first.view.is_some(), "视角 view 完整性"),
            ];
            if let Some((_, label)) = checks.into_iter().find(|(same, _)| !same) {
                return Err(format!("回放同一帧的{label}不一致：{}、{}", first.file, frame.file));
            }
            if group.len() == 3 && frame.view.is_some_and(|view| view as usize != index + 1) {
                return Err(format!("回放元数据 {} 的视角 view 与文件分组不一致，需要视角 {}", frame.file, index + 1));
            }
        }
        first.ts.checked_sub(meta.started_ts).ok_or_else(|| format!("回放元数据 {} 的相对时刻超出范围", first.file))
    }).collect();
    times.map(Some)
}

/// 只读文件头，确认这种格式解得开。
pub fn probe(path: &Path) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("解码 {} 失败：{e}", path.display());
    image::ImageReader::open(path).map_err(|e| fail(&e))?.with_guessed_format().map_err(|e| fail(&e))?.into_dimensions().map_err(|e| fail(&e))?;
    Ok(())
}

/// 读成 8 位灰度。彩色 / 16 位图按亮度换算。
pub fn load(path: &Path) -> Result<FrameImage, String> {
    let img = image::open(path).map_err(|e| format!("解码 {} 失败：{e}", path.display()))?.into_luma8();
    let (width, height) = img.dimensions();
    Ok(FrameImage::new(width, height, img.into_raw()))
}

pub fn load_views(files: &[PathBuf]) -> Result<Vec<Arc<FrameImage>>, String> {
    files.iter().map(|path| load(path).map(Arc::new)).collect()
}

/// 存成 PGM（P5）。帧录制用，不压缩、写得快。
pub fn save_pgm(path: &Path, img: &FrameImage) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
    write!(f, "P5\n{} {}\n255\n", img.width, img.height).map_err(|e| e.to_string())?;
    f.write_all(&img.pixels).map_err(|e| e.to_string())?;
    f.flush().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(PathBuf);

    impl Directory {
        fn new(names: &[&str]) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let dir = std::env::temp_dir().join(format!("gluesight-replay-{}-{nanos}-{seq}", std::process::id()));
            std::fs::create_dir(&dir).unwrap();
            for name in names {
                std::fs::write(dir.join(name), b"P5\n1 1\n255\n\x80").unwrap();
            }
            Self(dir)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            assert!(self.0.file_name().unwrap().to_str().unwrap().starts_with("gluesight-replay-"));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names_of(files: &[PathBuf]) -> Vec<String> {
        files.iter().map(|f| f.file_name().unwrap().to_str().unwrap().to_string()).collect()
    }

    #[test]
    fn names() {
        assert_eq!(parse("Frame12_3"), Some((3, 12)));
        assert_eq!(parse("cam2_000041"), Some((2, 41)));
        assert_eq!(parse("IMG_0001"), None);
        assert_eq!(parse("Frame12_4294967297"), None);
    }

    #[test]
    fn groups_three_views_by_sequence_instead_of_file_position() {
        let dir = Directory::new(&["Frame10_3.pgm", "Frame2_2.pgm", "Frame10_1.pgm", "Frame2_3.pgm", "Frame10_2.pgm", "Frame2_1.pgm"]);
        let groups = scan_views(&dir.0, 3, 0).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(names_of(&groups[0]), ["Frame2_1.pgm", "Frame2_2.pgm", "Frame2_3.pgm"]);
        assert_eq!(names_of(&groups[1]), ["Frame10_1.pgm", "Frame10_2.pgm", "Frame10_3.pgm"]);
        assert_eq!(load_views(&groups[0]).unwrap().len(), 3);
    }

    #[test]
    fn missing_or_duplicate_view_rejects_the_directory() {
        let dir = Directory::new(&["Frame1_1.pgm", "Frame1_3.pgm", "Frame2_1.pgm", "Frame2_2.pgm", "Frame2_3.pgm"]);
        let error = scan_views(&dir.0, 3, 0).unwrap_err();
        assert!(error.contains("帧 1 缺少视角 2"), "{error}");
        std::fs::write(dir.0.join("Frame1_2.pgm"), b"").unwrap();
        std::fs::write(dir.0.join("Frame1_2.jpg"), b"").unwrap();
        let error = scan_views(&dir.0, 3, 0).unwrap_err();
        assert!(error.contains("帧 1 的视角 2 重复"), "{error}");
        assert!(error.contains("Frame1_2.pgm") && error.contains("Frame1_2.jpg"));
    }

    #[test]
    fn invalid_and_unnamed_views_never_fill_a_missing_slot() {
        for invalid in ["Frame1_0.pgm", "Frame1_4.pgm", "IMG1.pgm"] {
            let dir = Directory::new(&["Frame1_1.pgm", "Frame1_2.pgm", invalid]);
            assert!(scan_views(&dir.0, 3, 0).unwrap_err().contains("视角无效"));
        }
        let dir = Directory::new(&["cam1_000001.pgm", "cam3_000001.pgm", "cam2_000001.pgm"]);
        assert!(scan_views(&dir.0, 3, 0).unwrap_err().contains("独立设备"));
        assert_eq!(names_of(&scan(&dir.0, 2).unwrap()), ["cam2_000001.pgm"]);
        let mixed = Directory::new(&["Frame1_1.pgm", "Frame1_2.pgm", "cam3_000001.pgm"]);
        assert!(scan_views(&mixed.0, 3, 0).unwrap_err().contains("独立设备"));
        assert!(scan_views(&dir.0, 2, 0).is_err());
    }

    #[test]
    fn recorded_views_select_one_device_before_grouping() {
        let dir = Directory::new(&["cam2_000002_v3.pgm", "cam1_000001_v1.pgm", "cam2_000002_v1.pgm", "cam1_000001_v3.pgm", "cam2_000002_v2.pgm", "cam1_000001_v2.pgm"]);
        assert_eq!(names_of(&scan_views(&dir.0, 3, 0).unwrap()[0]), ["cam1_000001_v1.pgm", "cam1_000001_v2.pgm", "cam1_000001_v3.pgm"]);
        assert_eq!(names_of(&scan_views(&dir.0, 3, 2).unwrap()[0]), ["cam2_000002_v1.pgm", "cam2_000002_v2.pgm", "cam2_000002_v3.pgm"]);
        assert!(scan_views(&dir.0, 3, 3).unwrap_err().contains("没有设备 3"));
        std::fs::write(dir.0.join("Frame1_1.pgm"), b"").unwrap();
        assert!(scan_views(&dir.0, 3, 0).unwrap_err().contains("混用"));
    }

    #[test]
    fn single_view_replay_keeps_the_original_channel_behavior() {
        let dir = Directory::new(&["Frame2_1.pgm", "Frame1_2.pgm", "Frame2_2.pgm", "Frame1_1.pgm"]);
        let groups = scan_views(&dir.0, 1, 2).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(names_of(&groups[0]), ["Frame1_2.pgm"]);
        assert_eq!(names_of(&groups[1]), ["Frame2_2.pgm"]);
        let recorded = Directory::new(&["cam2_000001_v1.pgm", "cam2_000001_v2.pgm", "cam2_000001_v3.pgm"]);
        assert_eq!(names_of(&scan_views(&recorded.0, 1, 2).unwrap()[0]), ["cam2_000001_v1.pgm"]);
        let missing = Directory::new(&["cam1_000001_v2.pgm"]);
        assert!(scan_views(&missing.0, 1, 0).is_err());
    }

    #[test]
    fn grouped_timeline_requires_all_views_from_the_same_trigger() {
        let dir = Directory::new(&["cam1_000001_v1.pgm", "cam1_000001_v2.pgm", "cam1_000001_v3.pgm"]);
        let groups = scan_views(&dir.0, 3, 0).unwrap();
        let mut frames: Vec<_> = names_of(&groups[0]).into_iter().map(|file| serde_json::json!({"file": file, "ts": 1250})).collect();
        let write = |frames: &[serde_json::Value]| std::fs::write(dir.0.join("part.json"), serde_json::to_vec(&serde_json::json!({"startedTs":1000,"frames":frames})).unwrap()).unwrap();
        write(&frames);
        assert_eq!(timeline_views(&dir.0, &groups).unwrap(), Some(vec![250]));
        frames[2]["ts"] = serde_json::json!(1300);
        write(&frames);
        assert!(timeline_views(&dir.0, &groups).unwrap_err().contains("曝光时刻不一致"));
        frames.pop();
        write(&frames);
        assert!(timeline_views(&dir.0, &groups).unwrap_err().contains("缺少 cam1_000001_v3.pgm"));
    }

    #[test]
    fn recorded_metadata_must_be_readable_complete_and_unambiguous() {
        let dir = Directory::new(&["Frame1_1.pgm", "Frame1_2.pgm", "Frame1_3.pgm"]);
        let groups = scan_views(&dir.0, 3, 0).unwrap();
        assert_eq!(timeline_views(&dir.0, &groups).unwrap(), None);
        std::fs::write(dir.0.join("part.json"), b"broken json").unwrap();
        assert!(timeline_views(&dir.0, &groups).unwrap_err().contains("损坏"));
        std::fs::write(dir.0.join("part.json"), br#"{"startedTs":1,"frames":[{"file":"Frame1_1.pgm","ts":2},{"file":"Frame1_1.pgm","ts":2}]}"#).unwrap();
        assert!(timeline_views(&dir.0, &groups).unwrap_err().contains("条目重复"));
        std::fs::write(dir.0.join("part.json"), br#"{"startedTs":1,"frames":[{"file":"Frame1_1.pgm"}]}"#).unwrap();
        assert!(timeline_views(&dir.0, &groups).unwrap_err().contains("损坏"));
    }

    #[test]
    fn grouped_metadata_requires_matching_device_and_trigger_identity() {
        let dir = Directory::new(&["cam1_000001_v1.pgm", "cam1_000001_v2.pgm", "cam1_000001_v3.pgm"]);
        let groups = scan_views(&dir.0, 3, 0).unwrap();
        let original: Vec<_> = names_of(&groups[0]).into_iter().enumerate().map(|(i, file)| serde_json::json!({
            "file": file, "ts": 1250, "view": i + 1, "camera": "cam1", "cam": 0,
            "seq": 1, "k": 2, "frameCounter": 6, "triggerCounter": 9, "session": 7,
            "counter": "synthetic", "cycleId": "piece-1"
        })).collect();
        let write = |frames: &[serde_json::Value]| std::fs::write(dir.0.join("part.json"), serde_json::to_vec(&serde_json::json!({"startedTs":1000,"frames":frames})).unwrap()).unwrap();
        write(&original);
        assert_eq!(timeline_views(&dir.0, &groups).unwrap(), Some(vec![250]));
        for (field, different) in [
            ("camera", serde_json::json!("cam2")), ("cam", serde_json::json!(1)),
            ("seq", serde_json::json!(2)), ("k", serde_json::json!(3)),
            ("frameCounter", serde_json::json!(7)), ("triggerCounter", serde_json::json!(10)),
            ("session", serde_json::json!(8)), ("counter", serde_json::json!("sdkFrame")),
            ("cycleId", serde_json::json!("piece-2")), ("view", serde_json::json!(1)),
        ] {
            let mut frames = original.clone();
            frames[2][field] = different;
            write(&frames);
            assert!(timeline_views(&dir.0, &groups).unwrap_err().contains(field), "{field}");
            frames[2].as_object_mut().unwrap().remove(field);
            write(&frames);
            assert!(timeline_views(&dir.0, &groups).unwrap_err().contains(field), "{field}");
        }
    }

    #[test]
    fn a_failed_view_decode_rejects_the_whole_frame() {
        let dir = Directory::new(&["Frame1_1.pgm", "Frame1_2.pgm", "Frame1_3.pgm"]);
        std::fs::write(dir.0.join("Frame1_2.pgm"), b"broken").unwrap();
        let groups = scan_views(&dir.0, 3, 0).unwrap();
        assert!(load_views(&groups[0]).unwrap_err().contains("Frame1_2.pgm"));
    }
}
