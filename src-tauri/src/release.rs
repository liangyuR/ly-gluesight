use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::sync::Arc;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::recipe::{Recipe, RecipeDoc, ShotSpec};

pub const RELEASE_SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Versions {
    pub engine: String,
    pub graph: String,
}

#[derive(Clone, Debug)]
pub enum ResourceSource {
    File(PathBuf),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct ShotInput {
    pub k: usize,
    pub image: Option<ResourceSource>,
    pub calibration: Option<ResourceSource>,
}

#[derive(Clone, Debug)]
pub struct PublishInput {
    pub recipe: RecipeDoc,
    pub versions: Versions,
    pub graph: ResourceSource,
    pub shots: Vec<ShotInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileDigest {
    pub path: String,
    pub hash: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShotManifest {
    pub k: usize,
    pub shot_id: String,
    pub camera: String,
    pub view: u8,
    pub skip: bool,
    pub size: Option<[u32; 2]>,
    pub image: Option<String>,
    pub calibration: Option<String>,
    pub centerline: String,
    pub points: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub recipe_id: String,
    pub recipe_hash: String,
    pub recipe_version: u32,
    pub versions: Versions,
    pub recipe: String,
    pub graph: String,
    pub shots: Vec<ShotManifest>,
    pub files: Vec<FileDigest>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Centerline {
    pub schema_version: u32,
    pub shot_id: String,
    pub camera: String,
    pub view: u8,
    pub size: Option<[u32; 2]>,
    pub spacing: f32,
    pub mm_per_px: Option<f32>,
    pub path: Vec<[f32; 2]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeasurePoints {
    pub schema_version: u32,
    pub shot_id: String,
    pub camera: String,
    pub view: u8,
    pub size: Option<[u32; 2]>,
    pub spacing: f32,
    pub indices: Vec<usize>,
    pub points: Vec<[f32; 2]>,
}

#[derive(Clone, Debug)]
pub struct ShotResources {
    pub k: usize,
    pub shot: ShotSpec,
    pub size: Option<[u32; 2]>,
    pub image: Option<PathBuf>,
    pub calibration: Option<PathBuf>,
    pub centerline: PathBuf,
    pub points: PathBuf,
    pub graph: PathBuf,
    pub centerline_data: Centerline,
    pub points_data: MeasurePoints,
}

#[derive(Clone, Debug)]
pub struct ReleaseBundle {
    pub hash: String,
    pub root: PathBuf,
    pub manifest: ReleaseManifest,
    pub recipe: RecipeDoc,
    snapshot: Arc<ReleaseSnapshot>,
}

#[derive(Debug)]
struct ReleaseSnapshot {
    hash: String,
    manifest: ReleaseManifest,
    recipe: RecipeDoc,
    manifest_bytes: Vec<u8>,
    files: BTreeMap<String, Vec<u8>>,
}

impl ReleaseBundle {
    pub fn verify(&self) -> Result<(), String> {
        let snapshot = &self.snapshot;
        if self.hash != snapshot.hash || self.manifest != snapshot.manifest || self.recipe != snapshot.recipe {
            return Err("发布包与已加载快照不一致".into());
        }
        let mut checked = BTreeSet::new();
        let manifest = read_file_once(&self.root.join("manifest.json"), &mut checked)?;
        if manifest != snapshot.manifest_bytes || fnv_hex(&manifest) != self.hash {
            return Err("发布清单 hash 不符，发布包已被改动".into());
        }
        for entry in &snapshot.manifest.files {
            let bytes = read_file_once(&self.root.join(&entry.path), &mut checked)?;
            if bytes.len() as u64 != entry.bytes || fnv_hex(&bytes) != entry.hash
                || snapshot.files.get(&entry.path) != Some(&bytes)
            {
                return Err(format!("发布资源 {} 的 hash、大小或内容不符，文件已被改动", entry.path));
            }
        }
        let mut actual = BTreeSet::new();
        inventory_once(&self.root, &self.root, &mut actual, &mut checked)?;
        let mut expected: BTreeSet<_> = snapshot.files.keys().cloned().collect();
        expected.insert("manifest.json".into());
        if actual != expected { return Err("发布目录存在清单以外的文件或缺失文件".into()); }
        Ok(())
    }

    pub fn shot(&self, k: usize) -> Result<ShotResources, String> {
        self.verify()?;
        let shot = self.recipe.shots.get(k).ok_or("发布包内没有该拍照点")?.clone();
        let entry = self.manifest.shots.get(k).ok_or("发布清单内缺少拍照点")?;
        let centerline = resource_path(&self.root, &entry.centerline)?;
        let points = resource_path(&self.root, &entry.points)?;
        Ok(ShotResources {
            k, shot, size: entry.size,
            image: entry.image.as_ref().map(|p| resource_path(&self.root, p)).transpose()?,
            calibration: entry.calibration.as_ref().map(|p| resource_path(&self.root, p)).transpose()?,
            centerline_data: read_json(&centerline)?,
            points_data: read_json(&points)?,
            centerline, points,
            graph: resource_path(&self.root, &self.manifest.graph)?,
        })
    }
}

pub fn publish(releases_root: &Path, input: PublishInput) -> Result<ReleaseBundle, String> {
    let recipe = input.recipe.build()?;
    recipe.ready()?;
    safe_id(&recipe.id)?;
    validate_versions(&input.versions)?;
    let mut inputs = BTreeMap::new();
    for shot in input.shots {
        let k = shot.k;
        if k >= recipe.shot_count() {
            return Err(format!("发布资源引用了不存在的拍照点 {k}"));
        }
        if inputs.insert(k, shot).is_some() {
            return Err(format!("拍照点 {k} 的发布资源重复"));
        }
    }
    if inputs.len() != recipe.shot_count() {
        return Err("发布资源未覆盖全部拍照点".into());
    }
    let mut files = BTreeMap::new();
    add_file(&mut files, "recipe.json".into(), json_bytes(&input.recipe)?)?;
    let graph = source_bytes(input.graph)?;
    json_object(&graph, "测量图")?;
    add_file(&mut files, "graph.json".into(), graph)?;
    let mut shots = Vec::new();
    for (k, shot) in recipe.shots.iter().enumerate() {
        let source = inputs.remove(&k).ok_or("发布资源缺少拍照点")?;
        if shot.measured() && (source.image.is_none() || source.calibration.is_none()) {
            return Err(format!("拍照点 {} 缺少发布原图或标定资源", shot.id));
        }
        let prefix = format!("shots/{k:02}");
        let (image, size) = if let Some(source) = source.image {
            let bytes = source_bytes(source)?;
            let (size, extension) = image_details(&bytes, &shot.id)?;
            check_coordinates(shot, size)?;
            let name = format!("{prefix}/image.{extension}");
            add_file(&mut files, name.clone(), bytes)?;
            (Some(name), Some(size))
        } else { (None, None) };
        let calibration = if let Some(source) = source.calibration {
            let bytes = source_bytes(source)?;
            json_object(&bytes, &format!("拍照点 {} 标定", shot.id))?;
            let name = format!("{prefix}/calibration.json");
            add_file(&mut files, name.clone(), bytes)?;
            Some(name)
        } else { None };
        let centerline = format!("{prefix}/centerline.json");
        let points = format!("{prefix}/points.json");
        add_file(&mut files, centerline.clone(), json_bytes(&centerline_data(&recipe, k, size))?)?;
        add_file(&mut files, points.clone(), json_bytes(&points_data(&recipe, k, size))?)?;
        shots.push(ShotManifest { k, shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view, skip: shot.skip, size, image, calibration, centerline, points });
    }
    let manifest = ReleaseManifest {
        schema_version: RELEASE_SCHEMA,
        recipe_id: recipe.id.clone(), recipe_hash: recipe.hash.clone(), recipe_version: recipe.version,
        versions: input.versions, recipe: "recipe.json".into(), graph: "graph.json".into(), shots,
        files: files.iter().map(|(path, bytes)| FileDigest { path: path.clone(), hash: fnv_hex(bytes), bytes: bytes.len() as u64 }).collect(),
    };
    let manifest_bytes = json_bytes(&manifest)?;
    let hash = fnv_hex(&manifest_bytes);
    let root = absolute_path(releases_root)?;
    ensure_directory(&root)?;
    let recipe_root = root.join(&recipe.id);
    ensure_directory(&recipe_root)?;
    let target = recipe_root.join(&hash);
    if path_exists(&target)? {
        return existing_bundle(&target, &recipe.id, &hash, &manifest_bytes, &files);
    }
    let stage = Staging::new(&recipe_root)?;
    for (name, bytes) in &files {
        let path = resource_path(&stage.path, name)?;
        ensure_directory(path.parent().ok_or("发布资源没有父目录")?)?;
        write_new(&path, bytes)?;
    }
    write_new(&stage.path.join("manifest.json"), &manifest_bytes)?;
    load_directory(&stage.path, &recipe.id, &hash)?;
    if path_exists(&target)? {
        return existing_bundle(&target, &recipe.id, &hash, &manifest_bytes, &files);
    }
    match fs::rename(&stage.path, &target) {
        Ok(()) => load(&root, &recipe.id, &hash),
        Err(_) if path_exists(&target)? => existing_bundle(&target, &recipe.id, &hash, &manifest_bytes, &files),
        Err(e) => Err(format!("原子发布目录 {} 失败：{e}", target.display())),
    }
}

pub fn load(releases_root: &Path, recipe_id: &str, bundle_hash: &str) -> Result<ReleaseBundle, String> {
    safe_id(recipe_id)?;
    valid_hash(bundle_hash)?;
    let root = absolute_path(releases_root)?.join(recipe_id).join(bundle_hash);
    load_directory(&root, recipe_id, bundle_hash)
}

fn load_directory(root: &Path, recipe_id: &str, bundle_hash: &str) -> Result<ReleaseBundle, String> {
    safe_id(recipe_id)?;
    valid_hash(bundle_hash)?;
    reject_links(root, false)?;
    let manifest_bytes = read_file(&root.join("manifest.json"))?;
    if fnv_hex(&manifest_bytes) != bundle_hash {
        return Err("发布清单 hash 不符，发布包已被改动".into());
    }
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes).map_err(|e| format!("发布清单无法解析：{e}"))?;
    if manifest.schema_version != RELEASE_SCHEMA {
        return Err(format!("发布包格式版本 {}，当前为 {RELEASE_SCHEMA}，需要重新发布", manifest.schema_version));
    }
    if manifest.recipe_id != recipe_id || manifest.recipe != "recipe.json" || manifest.graph != "graph.json" {
        return Err("发布清单的配方身份或资源引用无效".into());
    }
    validate_versions(&manifest.versions)?;
    valid_hash(&manifest.recipe_hash)?;
    let mut files = BTreeMap::new();
    let mut names = BTreeSet::new();
    for entry in &manifest.files {
        validate_relative(&entry.path)?;
        valid_hash(&entry.hash)?;
        if entry.path == "manifest.json" || !names.insert(entry.path.to_ascii_lowercase()) {
            return Err(format!("发布清单资源重复或自引用：{}", entry.path));
        }
        let bytes = read_file(&resource_path(root, &entry.path)?)?;
        if bytes.len() as u64 != entry.bytes || fnv_hex(&bytes) != entry.hash {
            return Err(format!("发布资源 {} 的 hash 或大小不符，文件已被改动", entry.path));
        }
        files.insert(entry.path.as_str(), bytes);
    }
    let recipe: RecipeDoc = parse_resource(&files, &manifest.recipe)?;
    let built = recipe.build()?;
    built.ready()?;
    if recipe.id != manifest.recipe_id || built.hash != manifest.recipe_hash || built.version != manifest.recipe_version || manifest.shots.len() != built.shot_count() {
        return Err("发布清单与配方快照的身份、版本或拍照点数量不一致".into());
    }
    json_object(file_bytes(&files, &manifest.graph)?, "发布测量图")?;
    let mut referenced = BTreeSet::from([manifest.recipe.clone(), manifest.graph.clone()]);
    for (k, shot) in built.shots.iter().enumerate() {
        let entry = &manifest.shots[k];
        if entry.k != k || entry.shot_id != shot.id || entry.camera != shot.camera || entry.view != shot.view || entry.skip != shot.skip {
            return Err(format!("发布清单拍照点 {k} 的身份与配方不一致"));
        }
        let prefix = format!("shots/{k:02}");
        if entry.centerline != format!("{prefix}/centerline.json") || entry.points != format!("{prefix}/points.json") {
            return Err(format!("拍照点 {} 的几何资源引用无效", shot.id));
        }
        if shot.measured() && (entry.image.is_none() || entry.calibration.is_none()) {
            return Err(format!("拍照点 {} 缺少发布原图或标定资源", shot.id));
        }
        for name in [Some(&entry.centerline), Some(&entry.points), entry.image.as_ref(), entry.calibration.as_ref()].into_iter().flatten() {
            validate_relative(name)?;
            if !referenced.insert(name.clone()) {
                return Err(format!("拍照点资源被重复引用：{name}"));
            }
        }
        if let Some(image) = &entry.image {
            let (size, extension) = image_details(file_bytes(&files, image)?, &shot.id)?;
            if entry.size != Some(size) || image != &format!("{prefix}/image.{extension}") {
                return Err(format!("拍照点 {} 的原图尺寸或引用与清单不一致", shot.id));
            }
            check_coordinates(shot, size)?;
        } else if entry.size.is_some() {
            return Err(format!("拍照点 {} 没有原图却声明了尺寸", shot.id));
        }
        if let Some(calibration) = &entry.calibration {
            if calibration != &format!("{prefix}/calibration.json") {
                return Err(format!("拍照点 {} 的标定资源引用无效", shot.id));
            }
            json_object(file_bytes(&files, calibration)?, &format!("拍照点 {} 标定", shot.id))?;
        }
        let centerline: Centerline = parse_resource(&files, &entry.centerline)?;
        let points: MeasurePoints = parse_resource(&files, &entry.points)?;
        if centerline != centerline_data(&built, k, entry.size) || points != points_data(&built, k, entry.size) {
            return Err(format!("拍照点 {} 的中线或测点与配方快照不一致", shot.id));
        }
    }
    let declared: BTreeSet<_> = manifest.files.iter().map(|f| f.path.clone()).collect();
    if referenced != declared {
        return Err("发布清单存在未声明、未引用或重复资源".into());
    }
    let mut actual = BTreeSet::new();
    inventory(root, root, &mut actual)?;
    let mut expected = declared;
    expected.insert("manifest.json".into());
    if actual != expected {
        return Err("发布目录存在清单以外的文件或缺失文件".into());
    }
    let snapshot = Arc::new(ReleaseSnapshot {
        hash: bundle_hash.into(), manifest: manifest.clone(), recipe: recipe.clone(), manifest_bytes,
        files: files.into_iter().map(|(name, bytes)| (name.to_owned(), bytes)).collect(),
    });
    Ok(ReleaseBundle { hash: bundle_hash.into(), root: root.to_path_buf(), manifest, recipe, snapshot })
}

fn centerline_data(recipe: &Recipe, k: usize, size: Option<[u32; 2]>) -> Centerline {
    let shot = &recipe.shots[k];
    Centerline { schema_version: RELEASE_SCHEMA, shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view, size, spacing: recipe.spacing, mm_per_px: shot.mm_per_px, path: shot.path.clone() }
}

fn points_data(recipe: &Recipe, k: usize, size: Option<[u32; 2]>) -> MeasurePoints {
    let shot = &recipe.shots[k];
    let indices: Vec<_> = recipe.owned_points(k).collect();
    let points = indices.iter().map(|&j| [recipe.points.x[j], recipe.points.y[j]]).collect();
    MeasurePoints { schema_version: RELEASE_SCHEMA, shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view, size, spacing: recipe.spacing, indices, points }
}

fn check_coordinates(shot: &ShotSpec, size: [u32; 2]) -> Result<(), String> {
    if shot.path.iter().any(|p| p[0] < 0.0 || p[1] < 0.0 || p[0] >= size[0] as f32 || p[1] >= size[1] as f32) {
        return Err(format!("拍照点 {} 的中线超出原图 {}×{}", shot.id, size[0], size[1]));
    }
    Ok(())
}

fn image_details(bytes: &[u8], shot: &str) -> Result<([u32; 2], &'static str), String> {
    let format = image::guess_format(bytes).map_err(|e| format!("拍照点 {shot} 原图格式无效：{e}"))?;
    let extension = match format {
        image::ImageFormat::Pnm => "pgm",
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::Png => "png",
        image::ImageFormat::Bmp => "bmp",
        image::ImageFormat::Tiff => "tiff",
        _ => return Err(format!("拍照点 {shot} 原图格式不支持")),
    };
    let image = image::load_from_memory_with_format(bytes, format).map_err(|e| format!("拍照点 {shot} 原图解码失败：{e}"))?;
    if image.width() == 0 || image.height() == 0 {
        return Err(format!("拍照点 {shot} 原图尺寸无效"));
    }
    Ok(([image.width(), image.height()], extension))
}

fn validate_versions(versions: &Versions) -> Result<(), String> {
    for (label, version) in [("引擎", &versions.engine), ("测量图", &versions.graph)] {
        if version.trim().is_empty() || version.len() > 256 || version.chars().any(char::is_control) {
            return Err(format!("{label}版本不能为空、超长或包含控制字符"));
        }
    }
    Ok(())
}

fn json_object(bytes: &[u8], label: &str) -> Result<(), String> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("{label} JSON 无效：{e}"))?;
    if !value.as_object().is_some_and(|object| !object.is_empty()) {
        return Err(format!("{label}需要非空 JSON 对象"));
    }
    Ok(())
}

fn source_bytes(source: ResourceSource) -> Result<Vec<u8>, String> {
    match source {
        ResourceSource::File(path) => read_file(&absolute_path(&path)?),
        ResourceSource::Bytes(bytes) => Ok(bytes),
    }
}

fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|e| format!("发布资源序列化失败：{e}"))
}

fn add_file(files: &mut BTreeMap<String, Vec<u8>>, path: String, bytes: Vec<u8>) -> Result<(), String> {
    validate_relative(&path)?;
    if files.insert(path.clone(), bytes).is_some() {
        return Err(format!("发布资源路径重复：{path}"));
    }
    Ok(())
}

fn file_bytes<'a>(files: &'a BTreeMap<&str, Vec<u8>>, name: &str) -> Result<&'a [u8], String> {
    files.get(name).map(Vec::as_slice).ok_or_else(|| format!("发布清单缺少资源 {name}"))
}

fn parse_resource<T: serde::de::DeserializeOwned>(files: &BTreeMap<&str, Vec<u8>>, name: &str) -> Result<T, String> {
    serde_json::from_slice(file_bytes(files, name)?).map_err(|e| format!("发布资源 {name} 无法解析：{e}"))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&read_file(path)?).map_err(|e| format!("发布资源 {} 无法解析：{e}", path.display()))
}

pub fn fnv_hex(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| (hash ^ *byte as u64).wrapping_mul(0x100000001b3));
    format!("{hash:016x}")
}

fn valid_hash(hash: &str) -> Result<(), String> {
    if hash.len() != 16 || !hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err("发布资源 hash 必须是 16 位小写十六进制".into());
    }
    Ok(())
}

fn reserved_component(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT")).is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
}

fn safe_id(id: &str) -> Result<(), String> {
    if !crate::recipe::valid_camera_id(id) || reserved_component(id) {
        return Err("发布配方编号不是安全的目录名".into());
    }
    Ok(())
}

fn validate_relative(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > 240 || path.contains('\\') || path.split('/').any(|part| {
        part.is_empty() || part == "." || part == ".." || part.ends_with('.') || reserved_component(part)
            || !part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    }) {
        return Err(format!("发布资源路径非法或可能逃逸目录：{path:?}"));
    }
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("发布资源路径不能包含上级目录".into());
    }
    if path.is_absolute() { Ok(path.to_path_buf()) } else { std::env::current_dir().map(|dir| dir.join(path)).map_err(|e| e.to_string()) }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    { metadata.file_type().is_symlink() }
}

fn reject_links(path: &Path, allow_missing: bool) -> Result<(), String> {
    let absolute = absolute_path(path)?;
    let mut cursor = PathBuf::new();
    for component in absolute.components() {
        cursor.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) { continue; }
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if is_link(&metadata) => return Err(format!("发布资源不允许符号链接或目录重解析点：{}", cursor.display())),
            Ok(_) => {}
            Err(e) if allow_missing && e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("发布资源不存在或无法读取 {}：{e}", cursor.display())),
        }
    }
    Ok(())
}

fn reject_links_once(path: &Path, checked: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    let absolute = absolute_path(path)?;
    let mut cursor = PathBuf::new();
    for component in absolute.components() {
        cursor.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) || checked.contains(&cursor) { continue; }
        let metadata = fs::symlink_metadata(&cursor).map_err(|e| format!("发布资源不存在或无法读取 {}：{e}", cursor.display()))?;
        if is_link(&metadata) { return Err(format!("发布资源不允许符号链接或目录重解析点：{}", cursor.display())); }
        checked.insert(cursor.clone());
    }
    Ok(())
}

fn read_file_once(path: &Path, checked: &mut BTreeSet<PathBuf>) -> Result<Vec<u8>, String> {
    reject_links_once(path, checked)?;
    let mut file = fs::File::open(path).map_err(|e| format!("读取发布资源 {} 失败：{e}", path.display()))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() { return Err(format!("发布资源不是普通文件：{}", path.display())); }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| format!("读取发布资源 {} 失败：{e}", path.display()))?;
    Ok(bytes)
}

fn inventory_once(root: &Path, dir: &Path, files: &mut BTreeSet<String>, checked: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    reject_links_once(dir, checked)?;
    for entry in fs::read_dir(dir).map_err(|e| format!("读取发布目录 {} 失败：{e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if is_link(&metadata) { return Err(format!("发布目录包含符号链接或目录重解析点：{}", path.display())); }
        if metadata.is_dir() { inventory_once(root, &path, files, checked)?; }
        else if metadata.is_file() {
            let relative = path.strip_prefix(root).map_err(|_| "发布资源逃逸目录")?.to_str().ok_or("发布文件名不是 UTF-8")?.replace('\\', "/");
            validate_relative(&relative)?;
            if !files.insert(relative) { return Err("发布目录包含重复资源".into()); }
        } else { return Err(format!("发布目录包含非普通文件：{}", path.display())); }
    }
    Ok(())
}

fn resource_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    validate_relative(name)?;
    let path = root.join(name);
    reject_links(&path, true)?;
    Ok(path)
}

fn ensure_directory(path: &Path) -> Result<(), String> {
    reject_links(path, true)?;
    fs::create_dir_all(path).map_err(|e| format!("创建发布目录 {} 失败：{e}", path.display()))?;
    reject_links(path, false)
}

fn path_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_link(&metadata) => Err(format!("发布路径不能使用符号链接：{}", path.display())),
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("检查发布路径 {} 失败：{e}", path.display())),
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    reject_links(path, false)?;
    if !fs::metadata(path).map_err(|e| e.to_string())?.is_file() {
        return Err(format!("发布资源不是普通文件：{}", path.display()));
    }
    fs::read(path).map_err(|e| format!("读取发布资源 {} 失败：{e}", path.display()))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    reject_links(path, true)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| format!("创建发布资源 {} 失败：{e}", path.display()))?;
    file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|e| format!("写入发布资源 {} 失败：{e}", path.display()))
}

fn inventory(root: &Path, dir: &Path, files: &mut BTreeSet<String>) -> Result<(), String> {
    reject_links(dir, false)?;
    for entry in fs::read_dir(dir).map_err(|e| format!("读取发布目录 {} 失败：{e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if is_link(&metadata) { return Err(format!("发布目录包含符号链接：{}", path.display())); }
        if metadata.is_dir() { inventory(root, &path, files)?; }
        else if metadata.is_file() {
            let relative = path.strip_prefix(root).map_err(|_| "发布资源逃逸目录")?.to_str().ok_or("发布文件名不是 UTF-8")?.replace('\\', "/");
            validate_relative(&relative)?;
            if !files.insert(relative) { return Err("发布目录包含重复资源".into()); }
        } else { return Err(format!("发布目录包含非普通文件：{}", path.display())); }
    }
    Ok(())
}

fn existing_bundle(target: &Path, recipe_id: &str, hash: &str, manifest: &[u8], files: &BTreeMap<String, Vec<u8>>) -> Result<ReleaseBundle, String> {
    let bundle = load_directory(target, recipe_id, hash)?;
    if read_file(&target.join("manifest.json"))? != manifest {
        return Err("发布清单 hash 冲突，已有发布包不会被改写".into());
    }
    for (name, bytes) in files {
        if read_file(&resource_path(target, name)?)? != *bytes {
            return Err(format!("发布资源 {name} hash 冲突，已有发布包不会被改写"));
        }
    }
    Ok(bundle)
}

struct Staging {
    path: PathBuf,
    parent: PathBuf,
}

impl Staging {
    fn new(parent: &Path) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
        let path = parent.join(format!(".pending-{}-{time}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).map_err(|e| format!("创建临时发布目录失败：{e}"))?;
        Ok(Self { path, parent: parent.to_path_buf() })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if self.path.parent() == Some(self.parent.as_path())
            && self.path.file_name().and_then(|p| p.to_str()).is_some_and(|p| p.starts_with(".pending-"))
            && inventory(&self.path, &self.path, &mut BTreeSet::new()).is_ok()
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{default_detect, default_limits, shot_list, TriggerMode, RECIPE_SCHEMA};

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("gluesight-release-{}-{nanos}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn releases(&self) -> PathBuf { self.0.join("vision/releases") }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            assert!(self.0.file_name().unwrap().to_str().unwrap().starts_with("gluesight-release-"));
            if inventory(&self.0, &self.0, &mut BTreeSet::new()).is_ok() {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }

    fn pgm(width: u32, height: u32, value: u8) -> Vec<u8> {
        let mut bytes = format!("P5\n{width} {height}\n255\n").into_bytes();
        bytes.resize(bytes.len() + (width * height) as usize, value);
        bytes
    }

    fn input() -> PublishInput {
        let mut shots = shot_list("cam1", vec![vec![[2.0, 3.0], [20.0, 3.0]], vec![[4.0, 8.0], [40.0, 8.0]]]);
        for shot in &mut shots { shot.mm_per_px = Some(0.5); }
        shots[1].camera = "cam2".into();
        shots[1].view = 2;
        shots[1].calib = Some("cam2-view2".into());
        PublishInput {
            recipe: RecipeDoc { id: "part-A".into(), name: "多分辨率配方".into(), version: 7, teaching_hash: Some("teaching-1".into()), product_code: 11, trigger_mode: TriggerMode::Fly, schema_version: RECIPE_SCHEMA, spacing: 1.0, filter_window: 3, detect: default_detect(), limits: default_limits(), shots },
            versions: Versions { engine: "lyflow-test-1".into(), graph: "taught-path-v1".into() },
            graph: ResourceSource::Bytes(br#"{"schemaVersion":1,"nodes":[],"edges":[]}"#.to_vec()),
            shots: vec![
                ShotInput { k: 0, image: Some(ResourceSource::Bytes(pgm(32, 24, 50))), calibration: Some(ResourceSource::Bytes(br#"{"mmPerPx":0.5,"source":"manual"}"#.to_vec())) },
                ShotInput { k: 1, image: Some(ResourceSource::Bytes(pgm(64, 40, 80))), calibration: Some(ResourceSource::Bytes(br#"{"mmPerPx":0.5,"source":"manual"}"#.to_vec())) },
            ],
        }
    }

    fn reseal(directory: &Directory, bundle: &ReleaseBundle, mut manifest: ReleaseManifest, replacement: Option<(&str, Vec<u8>)>) -> String {
        let mut files: BTreeMap<String, Vec<u8>> = bundle.manifest.files.iter().map(|f| (f.path.clone(), fs::read(bundle.root.join(&f.path)).unwrap())).collect();
        if let Some((name, bytes)) = replacement {
            let entry = manifest.files.iter_mut().find(|f| f.path == name).unwrap();
            entry.hash = fnv_hex(&bytes);
            entry.bytes = bytes.len() as u64;
            files.insert(name.into(), bytes);
        }
        let bytes = json_bytes(&manifest).unwrap();
        let hash = fnv_hex(&bytes);
        let root = directory.releases().join(&bundle.recipe.id).join(&hash);
        fs::create_dir_all(&root).unwrap();
        for (name, bytes) in files {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        fs::write(root.join("manifest.json"), bytes).unwrap();
        hash
    }

    #[test]
    #[ignore = "requires GLUESIGHT_VERIFY_RELEASE and LYFLOW_CORE_DLL; read-only verification profiling"]
    fn published_bundle_verification_profile() {
        let root = PathBuf::from(std::env::var_os("GLUESIGHT_VERIFY_RELEASE").expect("Set GLUESIGHT_VERIFY_RELEASE"));
        let hash = root.file_name().unwrap().to_str().unwrap();
        let id = root.parent().unwrap().file_name().unwrap().to_str().unwrap();
        let bundle = load_directory(&root, id, hash).unwrap();
        let dll = PathBuf::from(std::env::var_os("LYFLOW_CORE_DLL").expect("Set LYFLOW_CORE_DLL"));
        let expected_dll = fnv_hex(&fs::read(&dll).unwrap());
        let mut samples = Vec::new();
        for _ in 0..20 {
            let start = std::time::Instant::now();
            load_directory(&root, id, hash).unwrap();
            let load_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = std::time::Instant::now();
            bundle.verify().unwrap();
            let verify_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = std::time::Instant::now();
            assert_eq!(fnv_hex(&fs::read(&dll).unwrap()), expected_dll);
            samples.push(serde_json::json!({"loadMs":load_ms,"verifyMs":verify_ms,"dllMs":start.elapsed().as_secs_f64()*1000.0}));
        }
        println!("{}", serde_json::json!({"release":root,"bundleHash":hash,"dllHash":expected_dll,"samples":samples}));
    }

    #[test]
    fn fnv_matches_the_recipe_hash_algorithm() {
        assert_eq!(fnv_hex(b""), "cbf29ce484222325");
        assert_eq!(fnv_hex(b"hello"), "a430d84680aabd0b");
    }

    #[test]
    fn package_resolves_each_shot_with_its_own_image_size_and_geometry() {
        let directory = Directory::new();
        let input = input();
        let bundle = publish(&directory.releases(), input.clone()).unwrap();
        assert_eq!(bundle.recipe, input.recipe);
        assert_eq!(bundle.manifest.recipe_hash, input.recipe.build().unwrap().hash);
        assert_eq!(bundle.manifest.recipe_version, 7);
        assert_eq!(bundle.hash, fnv_hex(&fs::read(bundle.root.join("manifest.json")).unwrap()));
        assert_ne!(bundle.hash, bundle.manifest.recipe_hash);
        let first = bundle.shot(0).unwrap();
        let second = bundle.shot(1).unwrap();
        assert_eq!((first.size, second.size), (Some([32, 24]), Some([64, 40])));
        assert_eq!((first.shot.camera.as_str(), first.shot.view), ("cam1", 1));
        assert_eq!((second.centerline_data.camera.as_str(), second.centerline_data.view), ("cam2", 2));
        assert_eq!(second.centerline_data.shot_id, "P2");
        assert_eq!(second.points_data.size, second.size);
        assert_eq!(first.points_data.indices.first(), Some(&0));
        assert!(second.points_data.indices[0] > *first.points_data.indices.last().unwrap());
        assert_eq!(second.points_data.points[0], [4.0, 8.0]);
        for path in [first.image.unwrap(), first.calibration.unwrap(), first.centerline, first.points, first.graph] {
            assert!(path.starts_with(&bundle.root));
            assert!(path.is_file());
        }
        assert!(bundle.shot(2).is_err());
        load(&directory.releases(), "part-A", &bundle.hash).unwrap().verify().unwrap();
    }

    #[test]
    fn changed_or_missing_resource_refuses_the_existing_package() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let resource = bundle.shot(0).unwrap();
        let calibration = resource.calibration.unwrap();
        let original = fs::read(&calibration).unwrap();
        fs::write(&calibration, br#"{"mmPerPx":9.0}"#).unwrap();
        assert!(bundle.verify().unwrap_err().contains("calibration.json"));
        assert!(load(&directory.releases(), "part-A", &bundle.hash).is_err());
        fs::write(&calibration, original).unwrap();
        fs::remove_file(resource.image.unwrap()).unwrap();
        assert!(bundle.verify().unwrap_err().contains("image.pgm"));
    }

    #[test]
    fn loaded_snapshot_rejects_public_identity_changes() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let mut changed = bundle.clone();
        changed.hash = "0000000000000000".into();
        assert!(changed.verify().unwrap_err().contains("快照"));
        let mut changed = bundle.clone();
        changed.manifest.files[0].bytes += 1;
        assert!(changed.verify().unwrap_err().contains("快照"));
        let mut changed = bundle.clone();
        changed.recipe.shots[0].path[0][0] += 1.0;
        assert!(changed.verify().unwrap_err().contains("快照"));
        bundle.verify().unwrap();
    }

    #[test]
    fn every_verification_reads_all_resource_bytes_and_the_manifest_again() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        for entry in &bundle.manifest.files {
            let path = bundle.root.join(&entry.path);
            let original = fs::read(&path).unwrap();
            let mut changed = original.clone();
            *changed.last_mut().unwrap() ^= 1;
            fs::write(&path, changed).unwrap();
            assert!(bundle.verify().unwrap_err().contains(&entry.path));
            fs::write(&path, original).unwrap();
            bundle.verify().unwrap();
        }
        let path = bundle.root.join("manifest.json");
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed.push(b' ');
        fs::write(&path, changed).unwrap();
        assert!(bundle.verify().unwrap_err().contains("发布清单"));
        fs::write(&path, original).unwrap();
        let extra = bundle.root.join("shots/00/extra.json");
        fs::write(&extra, b"{}").unwrap();
        assert!(bundle.verify().unwrap_err().contains("清单以外"));
        fs::remove_file(extra).unwrap();
        fs::create_dir(bundle.root.join("empty")).unwrap();
        bundle.verify().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn loaded_snapshot_rejects_windows_resource_and_ancestor_junctions() {
        fn junction(link: &Path, target: &Path) {
            let result = std::process::Command::new("pwsh")
                .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:GLUESIGHT_TEST_LINK -Target $env:GLUESIGHT_TEST_TARGET -ErrorAction Stop | Out-Null"])
                .env("GLUESIGHT_TEST_LINK", link).env("GLUESIGHT_TEST_TARGET", target)
                .output().unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
            assert!(is_link(&fs::symlink_metadata(link).unwrap()));
        }
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let link = bundle.root.join("shots/00");
        let target = directory.0.join("resource-original");
        fs::rename(&link, &target).unwrap();
        junction(&link, &target);
        assert!(bundle.verify().unwrap_err().contains("重解析点"));
        fs::remove_dir(&link).unwrap();
        fs::rename(&target, &link).unwrap();
        bundle.verify().unwrap();
        let link = directory.0.join("vision");
        let target = directory.0.join("vision-original");
        fs::rename(&link, &target).unwrap();
        junction(&link, &target);
        assert!(bundle.verify().unwrap_err().contains("重解析点"));
        fs::remove_dir(&link).unwrap();
        fs::rename(&target, &link).unwrap();
        bundle.verify().unwrap();
    }

    #[test]
    fn changing_external_calibration_and_images_does_not_change_the_release() {
        let directory = Directory::new();
        let image_source = directory.0.join("source.pgm");
        let calib_source = directory.0.join("calibration.json");
        fs::write(&image_source, pgm(32, 24, 50)).unwrap();
        let first_calibration = br#"{"mmPerPx":0.5,"source":"station"}"#;
        fs::write(&calib_source, first_calibration).unwrap();
        let mut input = input();
        input.shots[0].image = Some(ResourceSource::File(image_source.clone()));
        input.shots[0].calibration = Some(ResourceSource::File(calib_source.clone()));
        let old = publish(&directory.releases(), input.clone()).unwrap();
        fs::write(&calib_source, br#"{"mmPerPx":0.7,"source":"station"}"#).unwrap();
        fs::write(&image_source, pgm(32, 24, 90)).unwrap();
        let new = publish(&directory.releases(), input).unwrap();
        assert_ne!(new.hash, old.hash);
        assert_eq!(new.manifest.recipe_hash, old.manifest.recipe_hash);
        fs::remove_file(&calib_source).unwrap();
        fs::remove_file(&image_source).unwrap();
        old.verify().unwrap();
        new.verify().unwrap();
        let old_resources = old.shot(0).unwrap();
        assert_eq!(fs::read(old_resources.calibration.unwrap()).unwrap(), first_calibration);
        assert_eq!(image::open(old_resources.image.unwrap()).unwrap().to_luma8().get_pixel(0, 0).0, [50]);
    }

    #[test]
    fn republishing_the_same_bytes_is_idempotent_and_never_repairs_a_corrupt_package() {
        let directory = Directory::new();
        let original = input();
        let first = publish(&directory.releases(), original.clone()).unwrap();
        let manifest_path = first.root.join("manifest.json");
        let modified = fs::metadata(&manifest_path).unwrap().modified().unwrap();
        let second = publish(&directory.releases(), original.clone()).unwrap();
        assert_eq!(first.root, second.root);
        assert_eq!(fs::metadata(&manifest_path).unwrap().modified().unwrap(), modified);
        let entries: Vec<_> = fs::read_dir(first.root.parent().unwrap()).unwrap().collect();
        assert_eq!(entries.len(), 1);
        fs::write(&manifest_path, b"corrupt").unwrap();
        assert!(publish(&directory.releases(), original).unwrap_err().contains("清单 hash"));
        assert_eq!(fs::read(&manifest_path).unwrap(), b"corrupt");
        assert_eq!(fs::read_dir(first.root.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn missing_duplicate_and_out_of_range_shot_inputs_are_rejected() {
        let directory = Directory::new();
        let mut duplicate = input();
        duplicate.shots.push(duplicate.shots[0].clone());
        assert!(publish(&directory.releases(), duplicate).unwrap_err().contains("重复"));
        let mut missing = input();
        missing.shots.pop();
        assert!(publish(&directory.releases(), missing).unwrap_err().contains("未覆盖"));
        let mut unknown = input();
        unknown.shots[0].k = 99;
        assert!(publish(&directory.releases(), unknown).unwrap_err().contains("不存在"));
        for image_missing in [true, false] {
            let mut missing = input();
            if image_missing { missing.shots[0].image = None; } else { missing.shots[0].calibration = None; }
            assert!(publish(&directory.releases(), missing).unwrap_err().contains("缺少"));
        }
        assert!(!directory.releases().exists());
    }

    #[test]
    fn skipped_shots_preserve_geometry_without_required_measurement_resources() {
        let directory = Directory::new();
        let mut input = input();
        input.recipe.shots[1].skip = true;
        input.shots[1].image = None;
        input.shots[1].calibration = None;
        let bundle = publish(&directory.releases(), input).unwrap();
        let shot = bundle.shot(1).unwrap();
        assert!(shot.shot.skip);
        assert!(shot.image.is_none() && shot.calibration.is_none() && shot.size.is_none());
        assert!(shot.points_data.points.is_empty());
        assert_eq!(shot.centerline_data.path, shot.shot.path);
    }

    #[test]
    fn versions_image_decode_and_image_coordinates_are_validated_before_publication() {
        let directory = Directory::new();
        for engine in [true, false] {
            let mut invalid = input();
            if engine { invalid.versions.engine.clear(); } else { invalid.versions.graph = " ".into(); }
            assert!(publish(&directory.releases(), invalid).unwrap_err().contains("版本"));
        }
        let mut invalid = input();
        invalid.shots[0].image = Some(ResourceSource::Bytes(b"P5\n32 24\n255\nshort".to_vec()));
        assert!(publish(&directory.releases(), invalid).unwrap_err().contains("原图"));
        let mut invalid = input();
        invalid.recipe.shots[1].path[1][0] = 64.0;
        assert!(publish(&directory.releases(), invalid).unwrap_err().contains("超出原图 64×40"));
        let mut invalid = input();
        invalid.shots[0].calibration = Some(ResourceSource::Bytes(b"{}".to_vec()));
        assert!(publish(&directory.releases(), invalid).unwrap_err().contains("非空 JSON"));
        let mut invalid = input();
        invalid.graph = ResourceSource::Bytes(b"[]".to_vec());
        assert!(publish(&directory.releases(), invalid).unwrap_err().contains("测量图"));
        assert!(!directory.releases().exists());
    }

    #[test]
    fn paths_cannot_escape_the_release_directory_or_use_windows_device_names() {
        let directory = Directory::new();
        for path in ["../other", "shots/../../other", "/root/file", "C:/outside", "shots\\00\\image.pgm", "shots//image", "shots/NUL.json", "shots/COM1.pgm", "shots/file."] {
            assert!(validate_relative(path).is_err(), "{path}");
        }
        assert!(load(&directory.releases(), "../outside", "0000000000000000").is_err());
        for id in ["CON", "NUL", "LPT9"] {
            let mut invalid = input();
            invalid.recipe.id = id.into();
            assert!(publish(&directory.releases(), invalid).is_err());
        }
        let bundle = publish(&directory.releases(), input()).unwrap();
        let mut manifest = bundle.manifest.clone();
        manifest.files[0].path = "../outside.json".into();
        let hash = reseal(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &hash).unwrap_err().contains("逃逸"));
    }

    #[test]
    fn manifest_duplicate_resources_and_wrong_recipe_identity_are_rejected() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let mut manifest = bundle.manifest.clone();
        manifest.files.push(manifest.files[0].clone());
        let hash = reseal(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &hash).unwrap_err().contains("重复"));
        let mut manifest = bundle.manifest.clone();
        manifest.recipe_id = "another-part".into();
        let hash = reseal(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &hash).unwrap_err().contains("身份"));
        let mut manifest = bundle.manifest.clone();
        manifest.recipe_version += 1;
        let hash = reseal(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &hash).unwrap_err().contains("版本"));
        let mut manifest = bundle.manifest.clone();
        manifest.shots[1].view = 1;
        let hash = reseal(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &hash).unwrap_err().contains("身份"));
    }

    #[test]
    fn rehashed_geometry_cannot_disagree_with_the_recipe_snapshot() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let name = bundle.manifest.shots[1].points.clone();
        let mut points = bundle.shot(1).unwrap().points_data;
        points.camera = "cam1".into();
        let hash = reseal(&directory, &bundle, bundle.manifest.clone(), Some((&name, json_bytes(&points).unwrap())));
        assert!(load(&directory.releases(), "part-A", &hash).unwrap_err().contains("测点与配方快照不一致"));
        let mut manifest = bundle.manifest.clone();
        manifest.shots[1].size = Some([32, 24]);
        let hash = reseal(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &hash).unwrap_err().contains("原图尺寸"));
    }

    #[test]
    fn unsupported_schema_and_unlisted_files_refuse_loading() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let mut manifest = bundle.manifest.clone();
        manifest.schema_version = 0;
        let hash = reseal(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &hash).unwrap_err().contains("需要重新发布"));
        fs::write(bundle.root.join("untracked.txt"), b"extra").unwrap();
        assert!(bundle.verify().unwrap_err().contains("清单以外"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_package_resources_cannot_escape_the_directory() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let path = bundle.shot(0).unwrap().calibration.unwrap();
        let outside = directory.0.join("external-calibration.json");
        fs::write(&outside, fs::read(&path).unwrap()).unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(bundle.verify().unwrap_err().contains("符号链接"));
        fs::remove_file(&path).unwrap();
        assert!(outside.is_file());
    }
}
