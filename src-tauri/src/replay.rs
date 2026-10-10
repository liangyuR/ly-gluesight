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
    hash: Option<String>,
}

#[derive(Clone, Debug)]
struct ReplayView {
    path: PathBuf,
    hash: String,
}

#[derive(Clone, Debug)]
pub struct ReplayFrame {
    views: Vec<ReplayView>,
}

impl ReplayFrame {
    pub fn paths(&self) -> Vec<PathBuf> {
        self.views.iter().map(|view| view.path.clone()).collect()
    }
}

fn content_hash(bytes: &[u8]) -> String {
    format!("fnv1a64:{}", crate::release::fnv_hex(bytes))
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
    if let Some(entries) = recorded_entries(dir)? { return Ok(entries); }
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
        entries.push(Entry { channel, camera, view, seq, name: stem, path, hash: None });
    }
    Ok(entries)
}

fn recorded_entries(dir: &Path) -> Result<Option<Vec<Entry>>, String> {
    let planned_files = std::fs::read_dir(dir).map_err(|error| format!("读取回放目录失败：{error}"))?
        .try_fold(false, |found, entry| {
            let entry = entry.map_err(|error| format!("读取回放目录项失败：{error}"))?;
            let name = entry.file_name();
            let stem = Path::new(&name).file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let planned = stem.strip_prefix('k').and_then(|s| s.split_once('_'))
                .is_some_and(|(k, rest)| digits(k).is_some() && rest.rsplit_once("_v").is_some_and(|(_, view)| digits(view).is_some()));
            Ok::<_, String>(found || planned)
        })?;
    let path = dir.join("part.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && planned_files =>
            return Err("逐拍照点录制缺少 part.json，禁止按普通图片回放".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取回放元数据失败：{error}")),
    };
    let meta: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|error| format!("回放元数据损坏：{error}"))?;
    let current_recording = planned_files || ["cycleId", "bundleHash", "plannedShots"].iter().any(|key| meta.get(key).is_some());
    let cycle_id = match meta["cycleId"].as_str() {
        Some(cycle_id) => cycle_id,
        None if current_recording => return Err("逐拍照点录制缺少有效 cycleId，禁止按普通图片回放".into()),
        None => return Ok(None),
    };
    if cycle_id.is_empty() || meta["available"].as_bool() != Some(true) {
        return Err("录制原图未完整落盘，不能作为完整回放源；请查看 part.json 的 errors".into());
    }
    let root = dir.canonicalize().map_err(|error| error.to_string())?;
    let frames = meta["frames"].as_array().ok_or("回放元数据缺少逐拍照点原图")?;
    let shots = meta["recipe"]["shots"].as_array().ok_or("回放元数据缺少配方拍照计划")?;
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for frame in frames {
        let k = frame["k"].as_u64().and_then(|k| usize::try_from(k).ok()).ok_or("回放拍照点编号无效")?;
        let shot = shots.get(k).ok_or("回放原图不属于配方拍照点")?;
        let file = frame["file"].as_str().ok_or("回放原图文件名缺失")?;
        let relative = Path::new(file);
        let view = frame["view"].as_u64().filter(|view| (1..=3).contains(view)).ok_or("回放原图视角无效")? as u32;
        let camera = frame["camera"].as_str().filter(|camera| !camera.is_empty()).ok_or("回放原图设备编号缺失")?;
        if frame["cycleId"].as_str() != Some(cycle_id) || frame["recipeHash"] != meta["recipe"]["hash"]
            || frame["bundleHash"] != meta["bundleHash"] || frame["camera"] != shot["camera"]
            || frame["shotId"] != shot["id"] || frame["selectedView"] != shot["view"] {
            return Err("录制原图的工件、发布包或拍照点身份不一致".into());
        }
        if frame["available"].as_bool() != Some(true) || relative.components().count() != 1
            || !matches!(relative.components().next(), Some(std::path::Component::Normal(_)))
            || file.contains(':') || file.contains('\\') || !seen.insert((k, view)) {
            return Err("回放原图未落盘、路径无效或拍照点视角重复".into());
        }
        let path = root.join(relative).canonicalize().map_err(|_| format!("回放原图 {file} 已丢失"))?;
        if !path.starts_with(&root) || !path.is_file() { return Err("回放原图路径越过录制目录".into()); }
        let bytes = std::fs::read(&path).map_err(|error| format!("读取回放原图失败：{error}"))?;
        let hash = content_hash(&bytes);
        if frame["hash"].as_str() != Some(hash.as_str()) {
            return Err(format!("回放原图 {file} 校验失败，文件已改变"));
        }
        entries.push(Entry { channel: None, camera: Some(camera.into()), view: Some(view), seq: k as u64,
            name: file.into(), path, hash: Some(hash) });
    }
    if entries.is_empty() || shots.iter().enumerate().any(|(k, shot)| !seen.contains(&(k, shot["view"].as_u64().unwrap_or(0) as u32))) {
        return Err("回放原图没有覆盖原配方的完整拍照计划".into());
    }
    Ok(Some(entries))
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

fn single_entries(dir: &Path, channel: u32) -> Result<Vec<Entry>, String> {
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
    Ok(entries)
}

pub fn scan(dir: &Path, channel: u32) -> Result<Vec<PathBuf>, String> {
    single_entries(dir, channel).map(|entries| entries.into_iter().map(|entry| entry.path).collect())
}

fn grouped_entries(dir: &Path, view_count: u8, channel: u32) -> Result<Vec<Vec<Entry>>, String> {
    if view_count == 1 {
        return single_entries(dir, channel).map(|entries| entries.into_iter().map(|entry| vec![entry]).collect());
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
    let mut groups: BTreeMap<u64, [Option<Entry>; 3]> = BTreeMap::new();
    for entry in entries {
        let view = if recorded { entry.view } else { entry.channel };
        let Some(view @ 1..=3) = view else {
            return Err(format!("三目回放图片 {} 的视角无效，需要视角 1、2、3", entry.path.display()));
        };
        let group = groups.entry(entry.seq).or_default();
        let slot = &mut group[view as usize - 1];
        if let Some(existing) = slot {
            return Err(format!("回放帧 {} 的视角 {view} 重复：{}、{}", entry.seq, existing.path.display(), entry.path.display()));
        }
        *slot = Some(entry);
    }
    groups.into_iter().map(|(seq, files)| {
        files.into_iter().enumerate().map(|(i, file)| file.ok_or_else(|| format!("回放帧 {seq} 缺少视角 {}，三目帧必须同时包含视角 1、2、3", i + 1))).collect()
    }).collect()
}

pub fn scan_views(dir: &Path, view_count: u8, channel: u32) -> Result<Vec<Vec<PathBuf>>, String> {
    grouped_entries(dir, view_count, channel).map(|groups| groups.into_iter().map(|group|
        group.into_iter().map(|entry| entry.path).collect()).collect())
}

pub fn scan_frames(dir: &Path, view_count: u8, channel: u32) -> Result<Vec<ReplayFrame>, String> {
    grouped_entries(dir, view_count, channel)?.into_iter().map(|group| {
        let views = group.into_iter().map(|entry| {
            let hash = match entry.hash {
                Some(hash) => hash,
                None => content_hash(&std::fs::read(&entry.path)
                    .map_err(|error| format!("读取回放原图 {} 失败：{error}", entry.path.display()))?),
            };
            Ok(ReplayView { path: entry.path, hash })
        }).collect::<Result<Vec<_>, String>>()?;
        Ok(ReplayFrame { views })
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

fn verified_bytes(view: &ReplayView) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(&view.path).map_err(|error| format!("读取回放原图 {} 失败：{error}", view.path.display()))?;
    if content_hash(&bytes) != view.hash {
        return Err(format!("回放原图 {} 校验失败，文件已改变；请恢复原图或重新加载回放目录", view.path.display()));
    }
    Ok(bytes)
}

pub fn probe_entry(frame: &ReplayFrame) -> Result<(), String> {
    for view in &frame.views {
        let bytes = verified_bytes(view)?;
        let fail = |error: &dyn std::fmt::Display| format!("解码 {} 失败：{error}", view.path.display());
        image::ImageReader::new(std::io::Cursor::new(bytes.as_slice()))
            .with_guessed_format().map_err(|error| fail(&error))?
            .into_dimensions().map_err(|error| fail(&error))?;
    }
    Ok(())
}

pub fn load_entry(frame: &ReplayFrame) -> Result<Vec<Arc<FrameImage>>, String> {
    frame.views.iter().map(|view| {
        let bytes = verified_bytes(view)?;
        let img = image::load_from_memory(&bytes).map_err(|error| format!("解码 {} 失败：{error}", view.path.display()))?.into_luma8();
        let (width, height) = img.dimensions();
        Ok(Arc::new(FrameImage::new(width, height, img.into_raw())))
    }).collect()
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

    fn cycle_recording(dir: &Path) -> serde_json::Value {
        let mut frames = Vec::new();
        let mut shots = Vec::new();
        for k in 0..4 {
            let camera = format!("cam{}", k % 2 + 1);
            let shot_id = format!("P{}", k + 1);
            let selected = [1, 2, 3, 1][k];
            shots.push(serde_json::json!({"id":shot_id,"camera":camera,"view":selected}));
            for view in 1..=3 {
                let file = format!("k{k:03}_{shot_id}_{camera}_v{view}.pgm");
                let bytes = b"P5\n1 1\n255\n\x80";
                std::fs::write(dir.join(&file), bytes).unwrap();
                frames.push(serde_json::json!({"k":k,"shotId":shot_id,"camera":camera,"view":view,
                    "selectedView":selected,"cycleId":"cycle-first","bundleHash":"frozen-pack",
                    "recipeHash":"recipe-hash","available":true,"file":file,"ts":1000+k*100,
                    "hash":format!("fnv1a64:{}",crate::release::fnv_hex(bytes)),"seq":k+1,
                    "session":7,"frameCounter":k+1,"triggerCounter":k+1}));
            }
        }
        serde_json::json!({"cycleId":"cycle-first","bundleHash":"frozen-pack","available":true,
            "recipe":{"hash":"recipe-hash","shots":shots},"startedTs":1000,"frames":frames})
    }

    #[test]
    fn cycle_recording_replays_each_device_using_shot_metadata_and_complete_views() {
        let dir = Directory::new(&[]);
        let meta = cycle_recording(&dir.0);
        std::fs::write(dir.0.join("part.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        let first = scan_views(&dir.0, 3, 1).unwrap();
        let second = scan_views(&dir.0, 3, 2).unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);
        assert_eq!(names_of(&first[1]), ["k002_P3_cam1_v1.pgm", "k002_P3_cam1_v2.pgm", "k002_P3_cam1_v3.pgm"]);
        assert_eq!(timeline_views(&dir.0, &first).unwrap(), Some(vec![0, 200]));
        assert_eq!(timeline_views(&dir.0, &second).unwrap(), Some(vec![100, 300]));
        assert_eq!(scan(&dir.0, 1).unwrap().len(), 2);
        assert_eq!(load_views(&first[1]).unwrap().len(), 3);
    }

    #[test]
    fn cycle_recording_refuses_wrong_identity_tampering_missing_or_duplicate_view() {
        let dir = Directory::new(&[]);
        let original = cycle_recording(&dir.0);
        for field in ["cycleId", "bundleHash", "recipeHash", "shotId", "camera", "selectedView"] {
            let mut changed = original.clone();
            changed["frames"][0][field] = serde_json::json!("wrong");
            std::fs::write(dir.0.join("part.json"), serde_json::to_vec(&changed).unwrap()).unwrap();
            assert!(scan_views(&dir.0, 3, 1).unwrap_err().contains("身份"), "{field}");
        }
        let mut duplicate = original.clone();
        duplicate["frames"].as_array_mut().unwrap().push(original["frames"][0].clone());
        std::fs::write(dir.0.join("part.json"), serde_json::to_vec(&duplicate).unwrap()).unwrap();
        assert!(scan_views(&dir.0, 3, 1).unwrap_err().contains("重复"));
        let mut missing = original.clone();
        missing["frames"].as_array_mut().unwrap().remove(2);
        std::fs::write(dir.0.join("part.json"), serde_json::to_vec(&missing).unwrap()).unwrap();
        assert!(scan_views(&dir.0, 3, 1).unwrap_err().contains("缺少视角 3"));
        std::fs::write(dir.0.join("part.json"), serde_json::to_vec(&original).unwrap()).unwrap();
        std::fs::write(dir.0.join(original["frames"][0]["file"].as_str().unwrap()), b"changed").unwrap();
        assert!(scan_views(&dir.0, 3, 1).unwrap_err().contains("校验失败"));
    }

    #[test]
    fn frozen_recording_rejects_same_size_replacement_and_recovers_all_views() {
        let dir = Directory::new(&[]);
        let meta = cycle_recording(&dir.0);
        std::fs::write(dir.0.join("part.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        let frames = scan_frames(&dir.0, 3, 1).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(names_of(&frames[0].paths()), ["k000_P1_cam1_v1.pgm", "k000_P1_cam1_v2.pgm", "k000_P1_cam1_v3.pgm"]);
        probe_entry(&frames[0]).unwrap();
        let images = load_entry(&frames[0]).unwrap();
        assert_eq!(images.len(), 3);
        assert!(images.iter().all(|image| image.pixels[0] == 128));
        let path = &frames[0].views[1].path;
        let original = std::fs::read(path).unwrap();
        let changed = b"P5\n1 1\n255\n\x07";
        assert_eq!(original.len(), changed.len());
        std::fs::write(path, changed).unwrap();
        let error = load_entry(&frames[0]).unwrap_err();
        assert!(error.contains("k000_P1_cam1_v2.pgm") && error.contains("文件已改变"), "{error}");
        assert!(probe_entry(&frames[0]).unwrap_err().contains("文件已改变"));
        assert!(scan_frames(&dir.0, 3, 1).unwrap_err().contains("校验失败"));
        std::fs::write(path, &original).unwrap();
        probe_entry(&frames[0]).unwrap();
        let restored = load_entry(&frames[0]).unwrap();
        assert_eq!(restored.len(), 3);
        assert!(restored.iter().all(|image| image.pixels[0] == 128));
    }

    #[test]
    fn frozen_frame_n_triplets_keep_sequence_and_reject_changes_after_scan() {
        let dir = Directory::new(&["Frame10_3.pgm", "Frame2_2.pgm", "Frame10_1.pgm", "Frame2_3.pgm", "Frame10_2.pgm", "Frame2_1.pgm"]);
        let frames = scan_frames(&dir.0, 3, 0).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(names_of(&frames[0].paths()), ["Frame2_1.pgm", "Frame2_2.pgm", "Frame2_3.pgm"]);
        assert_eq!(names_of(&frames[1].paths()), ["Frame10_1.pgm", "Frame10_2.pgm", "Frame10_3.pgm"]);
        let path = &frames[1].views[2].path;
        let original = std::fs::read(path).unwrap();
        std::fs::write(path, b"P5\n1 1\n255\n\x99").unwrap();
        assert!(load_entry(&frames[1]).unwrap_err().contains("Frame10_3.pgm"));
        assert_eq!(load_entry(&frames[0]).unwrap().len(), 3);
        std::fs::write(path, &original).unwrap();
        let restored = load_entry(&frames[1]).unwrap();
        assert_eq!(restored.len(), 3);
        assert!(restored.iter().all(|image| image.pixels[0] == 128));
    }

    #[test]
    fn frozen_single_views_keep_ordinary_order_and_legacy_channel_selection() {
        for (names, channel, expected) in [
            (vec!["image10.pgm", "image2.pgm"], 0, vec!["image2.pgm", "image10.pgm"]),
            (vec!["cam1_000001.pgm", "cam2_000002.pgm", "cam2_000001.pgm"], 2, vec!["cam2_000001.pgm", "cam2_000002.pgm"]),
        ] {
            let dir = Directory::new(&names);
            let frames = scan_frames(&dir.0, 1, channel).unwrap();
            let paths: Vec<_> = frames.iter().flat_map(ReplayFrame::paths).collect();
            assert_eq!(names_of(&paths), expected);
            assert!(frames.iter().all(|frame| load_entry(frame).unwrap().len() == 1));
            let path = &frames[0].views[0].path;
            let original = std::fs::read(path).unwrap();
            std::fs::write(path, b"P5\n1 1\n255\n\x07").unwrap();
            assert!(load_entry(&frames[0]).unwrap_err().contains("文件已改变"));
            std::fs::write(path, &original).unwrap();
            assert_eq!(load_entry(&frames[0]).unwrap()[0].pixels[0], 128);
        }
    }

    #[test]
    fn frozen_frame_reports_deleted_file_and_matching_hash_decode_failure() {
        let dir = Directory::new(&["image.pgm"]);
        let frames = scan_frames(&dir.0, 1, 0).unwrap();
        let path = &frames[0].views[0].path;
        let original = std::fs::read(path).unwrap();
        std::fs::remove_file(path).unwrap();
        let error = load_entry(&frames[0]).unwrap_err();
        assert!(error.contains("image.pgm") && error.contains("读取回放原图"), "{error}");
        std::fs::write(path, &original).unwrap();
        assert_eq!(load_entry(&frames[0]).unwrap().len(), 1);
        std::fs::write(path, b"invalid image").unwrap();
        let invalid = scan_frames(&dir.0, 1, 0).unwrap();
        assert!(probe_entry(&invalid[0]).unwrap_err().contains("解码"));
        assert!(load_entry(&invalid[0]).unwrap_err().contains("解码"));
    }

    #[test]
    fn planned_recording_never_falls_back_to_unverified_single_view_replay() {
        let dir = Directory::new(&[]);
        let original = cycle_recording(&dir.0);
        for cycle in [None, Some(serde_json::json!(17)), Some(serde_json::Value::Null)] {
            let mut meta = original.clone();
            match cycle {
                Some(value) => meta["cycleId"] = value,
                None => { meta.as_object_mut().unwrap().remove("cycleId"); }
            }
            std::fs::write(dir.0.join("part.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
            for count in [1, 3] { assert!(scan_views(&dir.0, count, 1).unwrap_err().contains("cycleId")); }
        }
        std::fs::remove_file(dir.0.join("part.json")).unwrap();
        assert!(scan_views(&dir.0, 1, 1).unwrap_err().contains("part.json"));
        let minimal = serde_json::json!({"startedTs":1000,"frames":[]});
        std::fs::write(dir.0.join("part.json"), serde_json::to_vec(&minimal).unwrap()).unwrap();
        assert!(scan_views(&dir.0, 1, 1).unwrap_err().contains("cycleId"));
    }
}
