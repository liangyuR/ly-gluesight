//! 离线样本集 v1：`sample-set.json` + `parts/<partId>/<shotId>_<cameraId>_v<view>.<ext>`。
//! 业务关联只用 partId / shotId / cameraId / view，不做任何内容比对。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::frame::FrameImage;

pub const SCHEMA_VERSION: u64 = 1;
const EXTS: [&str; 7] = ["jpg", "jpeg", "png", "bmp", "tif", "tiff", "pgm"];
const MISSING_SHOWN: usize = 50;
const LOCAL_HINT: &str = "请把样本集放到本地非同步目录（不能是 junction、符号链接或 OneDrive 等同步目录）";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SampleLabel { Ok, Ng, Unknown }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleCamera { pub id: String, pub views: Vec<u8> }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleShot {
    pub shot_id: String,
    pub camera_id: String,
    pub views: Vec<u8>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub mm_per_px: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplePart {
    pub part_id: String,
    pub sn: String,
    pub label: SampleLabel,
    #[serde(default)]
    pub shot_labels: BTreeMap<String, String>,
    #[serde(default)]
    pub captured_at: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: u64,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub image_size: Option<[u32; 2]>,
    #[serde(default)]
    pub mm_per_px: Option<f32>,
    pub cameras: Vec<SampleCamera>,
    pub shots: Vec<SampleShot>,
    pub parts: Vec<SamplePart>,
}

/// 历史记录与离线运行记下的样本件引用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleRef {
    pub set_id: String,
    pub part_id: String,
    pub sn: String,
    pub label: SampleLabel,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartSummary { pub part_id: String, pub sn: String, pub label: SampleLabel }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleSetSummary {
    pub id: String,
    pub name: String,
    pub source: String,
    pub dir: String,
    pub shot_count: usize,
    pub cameras: Vec<SampleCamera>,
    pub mm_per_px: Option<f32>,
    pub parts: Vec<PartSummary>,
    pub missing: Vec<String>,
    pub missing_count: usize,
}

#[derive(Debug)]
pub struct SampleSet {
    root: PathBuf,
    manifest: Manifest,
    images: HashMap<(String, String, String, u8), PathBuf>,
    missing: Vec<String>,
}

fn safe_id(id: &str) -> bool {
    crate::recipe::valid_camera_id(id)
}

fn reject_link(path: &Path, metadata: &std::fs::Metadata) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(format!("样本集路径 {} 是 reparse point，拒绝读取；{LOCAL_HINT}", path.display()));
        }
    }
    if metadata.file_type().is_symlink() {
        return Err(format!("样本集路径 {} 是符号链接，拒绝读取；{LOCAL_HINT}", path.display()));
    }
    Ok(())
}

fn plain_entry(path: &Path) -> Result<Option<std::fs::Metadata>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => { reject_link(path, &metadata)?; Ok(Some(metadata)) }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取样本集路径 {} 失败：{e}", path.display())),
    }
}

fn bad_mm_per_px(value: Option<f32>) -> bool {
    value.is_some_and(|v| !v.is_finite() || v <= 0.0)
}

fn check_views(views: &[u8], what: &str) -> Result<(), String> {
    let unique: HashSet<_> = views.iter().collect();
    if views.is_empty() || unique.len() != views.len() || views.iter().any(|v| !(1..=3).contains(v)) {
        return Err(format!("{what} 的视角必须为不重复的 1–3"));
    }
    Ok(())
}

impl Manifest {
    fn parse(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("sample-set.json 不是有效 JSON：{e}"))?;
        match value.get("schemaVersion").and_then(Value::as_u64) {
            Some(SCHEMA_VERSION) => {}
            Some(other) => return Err(format!("样本集 schemaVersion {other} 不受支持，只接受 {SCHEMA_VERSION}")),
            None => return Err("样本集缺少 schemaVersion".into()),
        }
        let manifest: Self = serde_json::from_value(value).map_err(|e| format!("sample-set.json 结构无效：{e}"))?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() { return Err("样本集 id 不能为空".into()); }
        if self.image_size.is_some_and(|[w, h]| w == 0 || h == 0) { return Err("样本集 imageSize 无效".into()); }
        if bad_mm_per_px(self.mm_per_px) { return Err("样本集 mmPerPx 必须为正数或 null".into()); }
        if self.cameras.is_empty() || self.shots.is_empty() || self.parts.is_empty() {
            return Err("样本集的 cameras、shots、parts 都不能为空".into());
        }
        let mut cameras = HashMap::new();
        for camera in &self.cameras {
            if !safe_id(&camera.id) { return Err(format!("相机编号 {} 无效", camera.id)); }
            check_views(&camera.views, &format!("相机 {}", camera.id))?;
            if cameras.insert(camera.id.as_str(), &camera.views).is_some() { return Err(format!("相机 {} 重复", camera.id)); }
        }
        for (k, shot) in self.shots.iter().enumerate() {
            let expected = format!("S{:03}", k + 1);
            if shot.shot_id != expected { return Err(format!("第 {} 个拍照点编号应为 {expected}，实际 {}", k + 1, shot.shot_id)); }
            let camera_views = cameras.get(shot.camera_id.as_str()).ok_or_else(|| format!("拍照点 {} 引用了不存在的相机 {}", shot.shot_id, shot.camera_id))?;
            check_views(&shot.views, &format!("拍照点 {}", shot.shot_id))?;
            if shot.views.iter().collect::<HashSet<_>>() != camera_views.iter().collect::<HashSet<_>>() {
                return Err(format!("拍照点 {} 的视角 {:?} 必须与相机 {} 的视角 {:?} 一致", shot.shot_id, shot.views, shot.camera_id, camera_views));
            }
            if bad_mm_per_px(shot.mm_per_px) { return Err(format!("拍照点 {} 的 mmPerPx 必须为正数或 null", shot.shot_id)); }
        }
        let mut parts = HashSet::new();
        for part in &self.parts {
            if !safe_id(&part.part_id) { return Err(format!("样本件编号 {} 无效", part.part_id)); }
            if part.sn.trim().is_empty() { return Err(format!("样本件 {} 缺少 sn", part.part_id)); }
            if !parts.insert(part.part_id.to_ascii_lowercase()) { return Err(format!("样本件 {} 重复", part.part_id)); }
        }
        Ok(())
    }
}

fn image_dimensions(path: &Path) -> Result<[u32; 2], String> {
    let (w, h) = image::ImageReader::open(path).and_then(|r| r.with_guessed_format()).map_err(|e| format!("无法读取：{e}"))?
        .into_dimensions().map_err(|e| format!("无法读取尺寸：{e}"))?;
    Ok([w, h])
}

impl SampleSet {
    pub fn load(dir: &Path) -> Result<Self, String> {
        crate::image_file::checked_source_path(dir).map_err(|e| format!("{e}；{LOCAL_HINT}"))?;
        let root = dir.canonicalize().map_err(|e| format!("样本集目录 {} 无法访问：{e}", dir.display()))?;
        let file = root.join("sample-set.json");
        plain_entry(&file)?.filter(|m| m.is_file()).ok_or("样本集目录缺少 sample-set.json")?;
        let text = std::fs::read_to_string(&file).map_err(|e| format!("读取 sample-set.json 失败：{e}"))?;
        let manifest = Manifest::parse(&text)?;
        let parts_dir = root.join("parts");
        plain_entry(&parts_dir)?.filter(|m| m.is_dir()).ok_or("样本集目录缺少 parts 子目录")?;
        let mut images = HashMap::new();
        let mut missing = Vec::new();
        let mut size = manifest.image_size;
        for part in &manifest.parts {
            let dir = parts_dir.join(&part.part_id);
            let mut files: HashMap<String, PathBuf> = HashMap::new();
            if plain_entry(&dir)?.is_some_and(|m| m.is_dir()) {
                for entry in std::fs::read_dir(&dir).map_err(|e| format!("读取样本件目录 {} 失败：{e}", dir.display()))? {
                    let entry = entry.map_err(|e| format!("读取样本件目录 {} 失败：{e}", dir.display()))?;
                    let path = entry.path();
                    let Some(ext) = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase) else { continue };
                    if !EXTS.contains(&ext.as_str()) { continue; }
                    reject_link(&path, &entry.metadata().map_err(|e| format!("读取 {} 失败：{e}", path.display()))?)?;
                    let stem = path.file_stem().and_then(|x| x.to_str()).unwrap_or_default().to_ascii_lowercase();
                    if let Some(previous) = files.insert(stem, path.clone()) {
                        return Err(format!("样本图重复：{}、{}", previous.display(), path.display()));
                    }
                }
            }
            for shot in &manifest.shots {
                for &view in &shot.views {
                    let stem = format!("{}_{}_v{view}", shot.shot_id, shot.camera_id);
                    let Some(path) = files.get(&stem.to_ascii_lowercase()) else {
                        missing.push(format!("{}/{stem}", part.part_id));
                        continue;
                    };
                    match image_dimensions(path) {
                        Ok(found) if size.is_none_or(|expected| expected == found) => {
                            size = Some(found);
                            images.insert((part.part_id.clone(), shot.shot_id.clone(), shot.camera_id.clone(), view), path.clone());
                        }
                        Ok([w, h]) => {
                            let [ew, eh] = size.unwrap_or_default();
                            missing.push(format!("{}/{stem}（尺寸 {w}×{h} 与 {ew}×{eh} 不符）", part.part_id));
                        }
                        Err(error) => missing.push(format!("{}/{stem}（{error}）", part.part_id)),
                    }
                }
            }
        }
        Ok(Self { root, manifest, images, missing })
    }

    pub fn manifest(&self) -> &Manifest { &self.manifest }

    pub fn missing(&self) -> &[String] { &self.missing }

    pub fn part(&self, part_id: &str) -> Option<&SamplePart> {
        self.manifest.parts.iter().find(|p| p.part_id == part_id)
    }

    pub fn reference(&self, part_id: &str) -> Option<SampleRef> {
        self.part(part_id).map(|p| SampleRef { set_id: self.manifest.id.clone(), part_id: p.part_id.clone(), sn: p.sn.clone(), label: p.label })
    }

    pub fn shot_mm_per_px(&self, shot_id: &str) -> Option<f32> {
        let shot = self.manifest.shots.iter().find(|s| s.shot_id == shot_id)?;
        shot.mm_per_px.or(self.manifest.mm_per_px)
    }

    pub fn has_camera(&self, camera_id: &str) -> bool {
        self.manifest.cameras.iter().any(|c| c.id == camera_id)
    }

    /// 某件某点某相机的 1..=view_count 视角；任何一幅缺失、解码失败或尺寸不符都按本帧缺失处理，不拿别的图顶替。
    pub fn load_views(&self, part_id: &str, shot_id: &str, camera_id: &str, view_count: u8) -> Result<Vec<Arc<FrameImage>>, String> {
        (1..=view_count).map(|view| {
            let path = self.images.get(&(part_id.to_string(), shot_id.to_string(), camera_id.to_string(), view))
                .ok_or_else(|| format!("样本集缺图：{part_id}/{shot_id}_{camera_id}_v{view}"))?;
            let image = crate::image_file::load(path)?;
            if let Some(size) = self.manifest.image_size.filter(|size| *size != [image.width, image.height]) {
                return Err(format!("样本图 {} 尺寸 {}×{} 与样本集 imageSize {}×{} 不符", path.display(), image.width, image.height, size[0], size[1]));
            }
            Ok(Arc::new(image))
        }).collect()
    }

    pub fn summary(&self) -> SampleSetSummary {
        let m = &self.manifest;
        SampleSetSummary {
            id: m.id.clone(), name: m.name.clone(), source: m.source.clone(), dir: { let dir = self.root.display().to_string(); dir.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(dir) },
            shot_count: m.shots.len(), cameras: m.cameras.clone(), mm_per_px: m.mm_per_px,
            parts: m.parts.iter().map(|p| PartSummary { part_id: p.part_id.clone(), sn: p.sn.clone(), label: p.label }).collect(),
            missing: self.missing.iter().take(MISSING_SHOWN).cloned().collect(),
            missing_count: self.missing.len(),
        }
    }
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;

    pub struct Fixture(pub PathBuf);

    impl Fixture {
        /// 2 件 × 4 点 × 单相机 cam1；像素值 = 件序号×100 + 点序号×10 + 视角，按身份可分辨。
        pub fn new(views: &[u8]) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let root = std::env::temp_dir().join(format!("gluesight-samples-{}-{nanos}-{seq}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let manifest = serde_json::json!({
                "schemaVersion": 1, "id": "fixture", "name": "测试样本集", "source": "unit", "imageSize": [2, 2], "mmPerPx": null,
                "cameras": [{"id": "cam1", "views": views}],
                "shots": (1..=4).map(|k| serde_json::json!({"shotId": format!("S{k:03}"), "cameraId": "cam1", "views": views})).collect::<Vec<_>>(),
                "parts": [
                    {"partId": "p01", "sn": "A01", "label": "ok"},
                    {"partId": "p02", "sn": "B02", "label": "ng", "shotLabels": {"S002": "breaking"}},
                ],
            });
            std::fs::write(root.join("sample-set.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
            for part in 1..=2u8 {
                let dir = root.join("parts").join(format!("p{part:02}"));
                std::fs::create_dir_all(&dir).unwrap();
                for shot in 1..=4u8 {
                    for &view in views {
                        let value = Self::pixel(part, shot, view);
                        image::save_buffer(dir.join(format!("S{shot:03}_cam1_v{view}.png")), &[value; 4], 2, 2, image::ColorType::L8).unwrap();
                    }
                }
            }
            Self(root)
        }

        pub fn pixel(part: u8, shot: u8, view: u8) -> u8 { part * 100 + shot * 10 + view }

        pub fn manifest(&self) -> Value {
            serde_json::from_slice(&std::fs::read(self.0.join("sample-set.json")).unwrap()).unwrap()
        }

        pub fn write_manifest(&self, value: &Value) {
            std::fs::write(self.0.join("sample-set.json"), serde_json::to_vec(value).unwrap()).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.0.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("gluesight-samples-")) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::Fixture;
    use super::*;

    #[test]
    fn manifest_contract_rejects_bad_versions_and_reports_missing_images() {
        type Edit = fn(&Fixture, &mut Value);
        let cases: [(&str, Edit, Result<usize, &str>); 9] = [
            ("完整样本集", |_, _| {}, Ok(0)),
            ("未知版本", |_, m| m["schemaVersion"] = 2.into(), Err("schemaVersion 2")),
            ("缺少版本", |_, m| { m.as_object_mut().unwrap().remove("schemaVersion"); }, Err("schemaVersion")),
            ("点号与顺序不符", |_, m| m["shots"][1]["shotId"] = "S005".into(), Err("S002")),
            ("逐点当量非正", |_, m| m["shots"][2]["mmPerPx"] = 0.0.into(), Err("S003")),
            ("拍照点视角只是相机视角的子集", |_, m| m["cameras"][0]["views"] = serde_json::json!([1, 2]), Err("S001")),
            ("一张图尺寸与 imageSize 不符", |f, _| image::save_buffer(f.0.join("parts/p01/S002_cam1_v1.png"), &[0; 9], 3, 3, image::ColorType::L8).unwrap(), Ok(1)),
            ("缺一张图", |f, _| std::fs::remove_file(f.0.join("parts/p02/S003_cam1_v1.png")).unwrap(), Ok(1)),
            ("整件目录缺失", |f, _| std::fs::remove_dir_all(f.0.join("parts/p01")).unwrap(), Ok(4)),
        ];
        for (name, edit, expected) in cases {
            let fixture = Fixture::new(&[1]);
            let mut manifest = fixture.manifest();
            edit(&fixture, &mut manifest);
            fixture.write_manifest(&manifest);
            match (SampleSet::load(&fixture.0), expected) {
                (Ok(set), Ok(missing)) => {
                    assert_eq!(set.missing().len(), missing, "{name}: {:?}", set.missing());
                    if name == "缺一张图" { assert_eq!(set.missing(), ["p02/S003_cam1_v1"]); }
                }
                (Err(error), Err(needle)) => assert!(error.contains(needle), "{name}: {error}"),
                (result, expected) => panic!("{name}: {:?} / {expected:?}", result.map(|s| s.missing().to_vec())),
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn junction_at_set_root_parts_or_part_directory_is_rejected() {
        fn junction(link: &Path, target: &Path) {
            let output = std::process::Command::new("cmd.exe").args(["/D", "/C", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        }
        let cases: [(&str, fn(&Fixture) -> PathBuf); 3] = [
            ("样本集根目录", |f| { let link = f.0.with_extension("link"); junction(&link, &f.0); link }),
            ("parts 目录", |f| {
                let moved = f.0.join("moved-parts");
                std::fs::rename(f.0.join("parts"), &moved).unwrap();
                junction(&f.0.join("parts"), &moved);
                f.0.clone()
            }),
            ("样本件目录", |f| {
                let moved = f.0.join("moved-p01");
                std::fs::rename(f.0.join("parts").join("p01"), &moved).unwrap();
                junction(&f.0.join("parts").join("p01"), &moved);
                f.0.clone()
            }),
        ];
        for (name, link) in cases {
            let fixture = Fixture::new(&[1]);
            let dir = link(&fixture);
            let loaded = SampleSet::load(&dir);
            if dir != fixture.0 { let _ = std::fs::remove_dir(&dir); }
            assert!(loaded.is_err(), "{name}");
        }
    }

    #[test]
    fn shot_mm_per_px_shot_value_overrides_global_then_falls_back_else_none() {
        let cases: [(&str, Option<f32>, Option<f32>, &str, Option<f32>); 4] = [
            ("逐点覆盖全局", Some(0.25), Some(0.125), "S002", Some(0.125)),
            ("未声明逐点回落全局", Some(0.25), Some(0.125), "S001", Some(0.25)),
            ("都未声明", None, None, "S002", None),
            ("拍照点不存在", Some(0.25), Some(0.125), "S009", None),
        ];
        for (name, global, shot, query, expected) in cases {
            let fixture = Fixture::new(&[1]);
            let mut manifest = fixture.manifest();
            manifest["mmPerPx"] = serde_json::json!(global);
            manifest["shots"][1]["mmPerPx"] = serde_json::json!(shot);
            fixture.write_manifest(&manifest);
            let set = SampleSet::load(&fixture.0).unwrap();
            assert_eq!(set.shot_mm_per_px(query), expected, "{name}");
        }
    }
}
