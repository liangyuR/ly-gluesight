use std::collections::{HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::burst_judge::FrameState;
use crate::frame::FrameImage;
use crate::store::FrameRecord;

const METADATA_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;
const DECODED_FRAME_LIMIT: usize = 3;
const DECODED_FRAME_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

/// 录制根内的相对路径，且所在目录是本件的录制目录（`*_cycle_{cycleId}`）。
fn cycle_relative<'a>(cycle_id: &str, file: &'a str) -> Result<&'a Path, String> {
    let relative = Path::new(file);
    if cycle_id.is_empty()
        || cycle_id.len() > 128
        || !cycle_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || file.contains(':')
        || file.contains('\\')
        || !relative
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .and_then(|name| name.split_once("_cycle_"))
            .is_some_and(|(_, id)| id == cycle_id)
    {
        return Err("历史原图路径与 cycleId 不一致，拒绝读取".into());
    }
    Ok(relative)
}

fn metadata_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    Ok(root
        .join(relative)
        .parent()
        .ok_or("原图目录无效")?
        .join("part.json"))
}

fn metadata_bytes(path: &Path) -> Result<Vec<u8>, String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err("录制元数据缺失，不能核验历史原图完整身份".into())
        }
        Err(error) => return Err(format!("录制元数据无法读取：{error}")),
        Ok(_) => crate::image_file::checked_source_path(path)?,
    }
    std::fs::read(path).map_err(|error| format!("录制元数据读取失败：{error}"))
}

fn decode_metadata(bytes: &[u8], cycle_id: &str) -> Result<Value, String> {
    let metadata: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("录制元数据损坏：{error}"))?;
    if metadata["cycleId"].as_str() != Some(cycle_id) {
        return Err("录制元数据的 cycleId 与历史原图不一致".into());
    }
    Ok(metadata)
}

#[derive(Default)]
pub(super) struct FrameMetadataCache(Mutex<Option<CachedRecording>>);

struct CachedRecording {
    path: PathBuf,
    cycle_id: String,
    bytes: Vec<u8>,
    index: Arc<RecordingIndex>,
    images: VecDeque<CachedFrame>,
}

struct CachedFrame {
    name: String,
    original: Vec<u8>,
    image: Arc<FrameImage>,
}

impl CachedFrame {
    fn bytes(&self) -> usize {
        self.original.capacity() + self.image.pixels.capacity()
    }
}

impl FrameMetadataCache {
    /// 段配方一帧的录制原图，按 (计划相机下标, 帧组序号) 读（决策记录 D-SB-29，录制文件名 `{相机}_s{seq:06}.pgm`）：
    /// 路径在本件录制目录内、文件名由相机与 seq 推出；part.json 里这一帧的 cycleId、相机下标与编号、seq、帧号
    /// （历史记有触发计数时还有触发计数）都与历史记录一致且已落盘，解码尺寸等于录制尺寸。缺帧、未录制或任一项对不上都报错，
    /// 不拿别的帧顶替。
    pub(super) fn load(
        &self,
        root: &Path,
        cycle_id: &str,
        camera_id: &str,
        frame: &FrameRecord,
    ) -> Result<Arc<FrameImage>, String> {
        let root = checked_recording_root(root)?;
        let (relative, name) = recorded_frame_path(cycle_id, camera_id, frame)?;
        let path = metadata_path(&root, relative)?;
        let bytes = metadata_bytes(&path)?;
        if bytes.len() > METADATA_CACHE_MAX_BYTES {
            let metadata = decode_metadata(&bytes, cycle_id)?;
            return decode_recorded_frame(
                &root,
                relative,
                cycle_id,
                camera_id,
                frame,
                find_frame(&metadata, &name)?,
                &metadata["devices"],
            )
            .map(Arc::new);
        }
        let index = self.read_index(path, cycle_id, bytes)?;
        let entry = index.entry(&name)?;
        let size = recorded_dimensions(cycle_id, camera_id, frame, entry)?;
        let original = original_bytes(&root, relative)?;
        if let Some(image) = self.cached_image(&index, &name, &original) {
            return Ok(image);
        }
        let image = Arc::new(decode_image(
            relative,
            camera_id,
            size,
            &original,
            entry,
            &index.devices,
        )?);
        self.cache_image(
            &index,
            CachedFrame {
                name,
                original,
                image: Arc::clone(&image),
            },
        );
        Ok(image)
    }

    fn cached_image(
        &self,
        index: &Arc<RecordingIndex>,
        name: &str,
        original: &[u8],
    ) -> Option<Arc<FrameImage>> {
        let cached = self.0.lock().unwrap();
        let recording = cached
            .as_ref()
            .filter(|item| Arc::ptr_eq(&item.index, index))?;
        recording
            .images
            .iter()
            .find(|frame| frame.name == name && frame.original == original)
            .map(|frame| Arc::clone(&frame.image))
    }

    fn cache_image(&self, index: &Arc<RecordingIndex>, frame: CachedFrame) {
        let incoming = frame.bytes();
        if incoming > DECODED_FRAME_CACHE_MAX_BYTES {
            return;
        }
        let mut cached = self.0.lock().unwrap();
        let Some(recording) = cached
            .as_mut()
            .filter(|item| Arc::ptr_eq(&item.index, index))
        else {
            return;
        };
        let images = &mut recording.images;
        images.retain(|previous| previous.name != frame.name);
        let mut bytes: usize = images.iter().map(CachedFrame::bytes).sum();
        while images.len() >= DECODED_FRAME_LIMIT
            || bytes + incoming > DECODED_FRAME_CACHE_MAX_BYTES
        {
            bytes -= images.pop_front().unwrap().bytes();
        }
        images.push_back(frame);
    }

    fn read_index(
        &self,
        path: PathBuf,
        cycle_id: &str,
        bytes: Vec<u8>,
    ) -> Result<Arc<RecordingIndex>, String> {
        let mut cached = self.0.lock().unwrap();
        if let Some(entry) = cached.as_ref().filter(|entry| {
            entry.path == path && entry.cycle_id == cycle_id && entry.bytes == bytes
        }) {
            return Ok(Arc::clone(&entry.index));
        }
        let index = Arc::new(RecordingIndex::new(decode_metadata(&bytes, cycle_id)?)?);
        *cached = Some(CachedRecording {
            path,
            cycle_id: cycle_id.to_owned(),
            bytes,
            index: Arc::clone(&index),
            images: VecDeque::new(),
        });
        Ok(index)
    }
}

fn find_frame<'a>(metadata: &'a Value, name: &str) -> Result<&'a Value, String> {
    let mut entries = metadata["frames"]
        .as_array()
        .ok_or("录制元数据缺少原图引用")?
        .iter()
        .filter(|entry| entry["file"].as_str() == Some(name));
    let entry = entries.next().ok_or("录制元数据缺少对应原图")?;
    if entries.next().is_some() {
        return Err("录制元数据的原图身份重复".into());
    }
    Ok(entry)
}

pub(super) struct VerifiedFrames {
    root: PathBuf,
    cycle_id: String,
    metadata: HashMap<PathBuf, RecordingIndex>,
}

struct RecordingIndex {
    entries: HashMap<String, Option<Value>>,
    devices: Value,
}

impl RecordingIndex {
    fn new(mut metadata: Value) -> Result<Self, String> {
        let Value::Array(frames) = metadata["frames"].take() else {
            return Err("录制元数据缺少原图引用".into());
        };
        let mut entries = HashMap::with_capacity(frames.len());
        for entry in frames {
            let Some(file) = entry["file"].as_str().map(str::to_owned) else {
                continue;
            };
            entries
                .entry(file)
                .and_modify(|existing| *existing = None)
                .or_insert(Some(entry));
        }
        Ok(Self {
            entries,
            devices: metadata["devices"].take(),
        })
    }

    fn entry(&self, name: &str) -> Result<&Value, String> {
        self.entries
            .get(name)
            .ok_or("录制元数据缺少对应原图")?
            .as_ref()
            .ok_or_else(|| "录制元数据的原图身份重复".into())
    }
}

impl VerifiedFrames {
    pub(super) fn new(root: &Path, cycle_id: &str) -> Result<Self, String> {
        Ok(Self {
            root: checked_recording_root(root)?,
            cycle_id: cycle_id.to_owned(),
            metadata: HashMap::new(),
        })
    }

    pub(super) fn load(
        &mut self,
        camera_id: &str,
        frame: &FrameRecord,
    ) -> Result<FrameImage, String> {
        let root = &self.root;
        let cycle_id = self.cycle_id.as_str();
        let (relative, name) = recorded_frame_path(cycle_id, camera_id, frame)?;
        let directory = relative.parent().ok_or("原图目录无效")?;
        if !self.metadata.contains_key(directory) {
            let bytes = metadata_bytes(&metadata_path(root, relative)?)?;
            let indexed = RecordingIndex::new(decode_metadata(&bytes, cycle_id)?)?;
            self.metadata.insert(directory.to_path_buf(), indexed);
        }
        let indexed = &self.metadata[directory];
        decode_recorded_frame(
            root,
            relative,
            cycle_id,
            camera_id,
            frame,
            indexed.entry(&name)?,
            &indexed.devices,
        )
    }

    pub(super) fn mix_sample(
        &mut self,
        camera: &str,
        frame: &FrameRecord,
    ) -> Result<(FrameImage, String, crate::camera::WhiteBalance), String> {
        let image = self.load(camera, frame)?;
        let observed = image
            .capture
            .as_ref()
            .filter(|source| !source.synthetic)
            .ok_or("混胶现场取样需要本次 MVS 会话的彩色原图和已确认的白平衡")?;
        let (serial, wb) = (observed.serial.clone(), observed.white_balance);
        wb.validate()?;
        Ok((image, serial, wb))
    }
}

fn checked_recording_root(root: &Path) -> Result<PathBuf, String> {
    crate::image_file::checked_source_path(root)?;
    root.canonicalize()
        .map_err(|_| "原图目录不存在或已清理".into())
}

fn recorded_frame_path<'a>(
    cycle_id: &str,
    camera_id: &str,
    frame: &'a FrameRecord,
) -> Result<(&'a Path, String), String> {
    if frame.status == FrameState::Missing {
        return Err(format!(
            "相机 {camera_id} 第 {} 帧组缺帧，没有原图",
            frame.seq
        ));
    }
    let file = frame.raw_file.as_deref().ok_or_else(|| {
        format!(
            "相机 {camera_id} 第 {} 帧组的原图未录制或录制队满未保存",
            frame.seq
        )
    })?;
    let relative = cycle_relative(cycle_id, file)?;
    let name = format!("{camera_id}_s{:06}.pgm", frame.seq);
    if relative.file_name().and_then(|n| n.to_str()) != Some(name.as_str()) {
        return Err(format!(
            "历史原图 {file} 与相机 {camera_id} 第 {} 帧组的身份不一致，拒绝读取",
            frame.seq
        ));
    }
    Ok((relative, name))
}

fn decode_recorded_frame(
    root: &Path,
    relative: &Path,
    cycle_id: &str,
    camera_id: &str,
    frame: &FrameRecord,
    entry: &Value,
    devices: &Value,
) -> Result<FrameImage, String> {
    let size = recorded_dimensions(cycle_id, camera_id, frame, entry)?;
    let bytes = original_bytes(root, relative)?;
    decode_image(relative, camera_id, size, &bytes, entry, devices)
}

fn recorded_dimensions(
    cycle_id: &str,
    camera_id: &str,
    frame: &FrameRecord,
    entry: &Value,
) -> Result<[u32; 2], String> {
    if entry["cycleId"].as_str() != Some(cycle_id) || entry["available"].as_bool() != Some(true) {
        return Err("录制元数据的原图身份不一致或未落盘".into());
    }
    let frame_counter = frame
        .frame_counter
        .ok_or("历史帧记录缺少帧号，不能核验原图身份")?;
    if entry["camera"].as_u64() != Some(frame.camera as u64)
        || entry["cameraId"].as_str() != Some(camera_id)
        || entry["seq"].as_u64() != Some(frame.seq)
        || entry["frameCounter"].as_u64() != Some(frame_counter)
        || frame
            .trigger_counter
            .is_some_and(|trigger| entry["triggerCounter"].as_u64() != Some(trigger))
    {
        return Err("录制原图的相机、帧组序号或帧号与历史记录不一致".into());
    }
    let dimension = |field: &str| {
        entry[field]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
    };
    Ok([
        dimension("width").ok_or("录制原图宽度无效")?,
        dimension("height").ok_or("录制原图高度无效")?,
    ])
}

fn decode_image(
    relative: &Path,
    camera_id: &str,
    size: [u32; 2],
    bytes: &[u8],
    entry: &Value,
    devices: &Value,
) -> Result<FrameImage, String> {
    let file = relative.to_string_lossy();
    checked_dimensions(bytes, Some(size), &file)?;
    let mut image = crate::image_file::decode(bytes)
        .map_err(|error| format!("原图 {file} 解码失败：{error}"))?;
    if image.is_color() && entry["imageMode"] == "color" && entry["session"].as_u64().is_some() {
        if let Some(devices) = devices.as_array() {
            let mut matching = devices.iter().filter(|d| d["cameraId"] == camera_id);
            if let Some(d) = matching
                .next()
                .filter(|d| entry["session"] == d["deviceSession"])
            {
                if matching.next().is_none() {
                    image.capture = crate::frame::MixCapture::recorded(d);
                }
            }
        }
    }
    Ok(image)
}

fn original_bytes(root: &Path, relative: &Path) -> Result<Vec<u8>, String> {
    let source = root.join(relative);
    // 核对实际文件到读完之间不让空闲压缩把 .pgm 换成 .gsr
    let _busy = crate::raw_compress::activity().busy();
    // 空闲压缩过的原图在盘上是同名的 .gsr，读出的仍是原始 PGM 字节
    let stored = crate::image_file::stored_path(&source)
        .ok_or_else(|| "原图未保留或已按保留策略清理".to_string())?;
    crate::image_file::checked_source_path(&stored)?;
    let path = stored
        .canonicalize()
        .map_err(|_| "原图未保留或已按保留策略清理".to_string())?;
    if !path.starts_with(root) || !path.is_file() {
        return Err("原图路径越过录制目录或不是文件".into());
    }
    crate::image_file::read_original(&source).map_err(|error| format!("原图读取失败：{error}"))
}

fn checked_dimensions(bytes: &[u8], expected: Option<[u32; 2]>, file: &str) -> Result<(), String> {
    if let Some(expected) = expected {
        let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|error| format!("原图 {file} 解码失败：{error}"))?;
        let (width, height) = reader
            .into_dimensions()
            .map_err(|error| format!("原图 {file} 解码失败：{error}"))?;
        if [width, height] != expected {
            return Err(format!("原图 {file} 的尺寸与录制尺寸不符"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::ImportTestDir;
    use serde_json::json;

    fn recorded_frame() -> (ImportTestDir, PathBuf, FrameRecord, Value) {
        let root = ImportTestDir::new();
        let directory = root.0.join("sample_cycle_record-1");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("cam1_s000007.pgm"),
            b"P5\n2 1\n255\n\x19\x60",
        )
        .unwrap();
        let frame = FrameRecord {
            camera: 0,
            seq: 7,
            status: FrameState::Measured,
            frame_counter: Some(11),
            trigger_counter: Some(17),
            region: None,
            metrics: None,
            raw_file: Some("sample_cycle_record-1/cam1_s000007.pgm".into()),
        };
        let entry = json!({"file":"cam1_s000007.pgm", "cycleId":"record-1", "camera":0, "cameraId":"cam1",
            "seq":7, "frameCounter":11, "triggerCounter":17, "width":2, "height":1, "available":true});
        (root, directory, frame, entry)
    }

    fn manifest(entries: Vec<Value>) -> Vec<u8> {
        serde_json::to_vec(&json!({"cycleId":"record-1", "frames":entries})).unwrap()
    }

    #[test]
    fn duplicated_file_identity_rejects_single_and_indexed_reads() {
        let (root, directory, frame, entry) = recorded_frame();
        let path = directory.join("part.json");
        let write_manifest = |entries| std::fs::write(&path, manifest(entries)).unwrap();
        let cache = FrameMetadataCache::default();
        let read_single = || cache.load(&root.0, "record-1", "cam1", &frame);
        write_manifest(vec![entry.clone()]);
        let image = read_single().unwrap();
        assert_eq!(image.pixels, [0x19, 0x60]);
        let read_indexed = || {
            VerifiedFrames::new(&root.0, "record-1")
                .unwrap()
                .load("cam1", &frame)
        };
        assert_eq!(read_indexed().unwrap().pixels, image.pixels);
        write_manifest(vec![
            entry.clone(),
            json!({"file":"cam1_s000007.pgm"}),
            entry,
        ]);
        assert!(read_single().is_err());
        assert!(read_indexed().is_err());
    }

    #[test]
    fn cached_recording_index_revalidates_changed_manifest_and_reads_current_pixels() {
        let (root, directory, frame, mut entry) = recorded_frame();
        let path = directory.join("part.json");
        let original_path = directory.join("cam1_s000007.pgm");
        std::fs::write(&path, manifest(vec![entry.clone()])).unwrap();
        let cache = FrameMetadataCache::default();
        let read = |frame: &FrameRecord| cache.load(&root.0, "record-1", "cam1", frame);
        assert_eq!(read(&frame).unwrap().pixels, [0x19, 0x60]);
        let original_stamp = std::fs::metadata(&original_path).unwrap();
        let updated_pixels = b"P5\n2 1\n255\n\x31\x60";
        std::fs::write(&original_path, updated_pixels).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&original_path)
            .unwrap()
            .set_modified(original_stamp.modified().unwrap())
            .unwrap();
        assert_eq!(read(&frame).unwrap().pixels, [0x31, 0x60]);
        std::fs::remove_file(&original_path).unwrap();
        assert!(read(&frame).is_err());
        std::fs::write(&original_path, updated_pixels).unwrap();

        let stamp = std::fs::metadata(&path).unwrap();
        entry["frameCounter"] = json!(12);
        let updated = manifest(vec![entry]);
        assert_eq!(updated.len() as u64, stamp.len());
        std::fs::write(&path, updated).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(stamp.modified().unwrap())
            .unwrap();
        assert!(read(&frame).is_err());
        let updated_frame = FrameRecord {
            frame_counter: Some(12),
            ..frame
        };
        assert_eq!(read(&updated_frame).unwrap().pixels, [0x31, 0x60]);
        std::fs::remove_file(path).unwrap();
        assert!(read(&updated_frame).is_err());
    }

    #[test]
    fn changed_recorded_white_balance_updates_capture_for_the_same_bayer_image() {
        let (root, directory, frame, mut entry) = recorded_frame();
        entry["height"] = json!(2);
        entry["imageMode"] = json!("color");
        entry["session"] = json!(3);
        let mut pixels =
            crate::image_file::pnm_header(2, 2, 1, Some(crate::frame::BayerPattern::Rg))
                .into_bytes();
        pixels.extend([20, 80, 120, 230]);
        std::fs::write(directory.join("cam1_s000007.pgm"), pixels).unwrap();
        let manifest = |red| {
            json!({
                "cycleId": "record-1", "frames": [entry],
                "devices": [{
                    "cameraId": "cam1", "deviceSession": 3,
                    "cameraConfig": {"source": "mvs", "serial": "test-camera"},
                    "imaging": {"whiteBalance": {"unit": "ratio", "red": red, "green": 1.0, "blue": 1.0}},
                }],
            })
            .to_string()
        };
        let path = directory.join("part.json");
        let cache = FrameMetadataCache::default();
        let read = || cache.load(&root.0, "record-1", "cam1", &frame).unwrap();
        std::fs::write(&path, manifest(1.0)).unwrap();
        let previous = read();
        assert_eq!(previous.capture.as_ref().unwrap().white_balance.red, 1.0);

        std::fs::write(&path, manifest(2.0)).unwrap();
        let updated = read();
        assert_eq!(updated.pixels, previous.pixels);
        assert_eq!(updated.capture.as_ref().unwrap().white_balance.red, 2.0);
    }

    #[test]
    fn oversized_manifest_remains_readable() {
        let (root, directory, frame, entry) = recorded_frame();
        let mut bytes = manifest(vec![entry]);
        bytes.resize(METADATA_CACHE_MAX_BYTES + 1, b' ');
        std::fs::write(directory.join("part.json"), bytes).unwrap();
        let image = FrameMetadataCache::default()
            .load(&root.0, "record-1", "cam1", &frame)
            .unwrap();
        assert_eq!(image.pixels, [0x19, 0x60]);
    }
}
