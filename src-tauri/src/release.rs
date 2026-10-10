use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
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
pub struct FileEntry {
    pub path: String,
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
    pub recipe_revision: String,
    pub bundle_id: String,
    pub recipe_version: u32,
    pub versions: Versions,
    pub recipe: String,
    pub graph: String,
    pub shots: Vec<ShotManifest>,
    pub files: Vec<FileEntry>,
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
    pub id: String,
    pub root: PathBuf,
    pub manifest: ReleaseManifest,
    pub recipe: RecipeDoc,
}

impl ReleaseBundle {
    pub fn verify(&self) -> Result<(), String> {
        valid_bundle_id(&self.id)?;
        let manifest = read_manifest(&self.root, &self.id)?;
        if manifest != self.manifest || manifest.bundle_id != self.id {
            return Err("发布清单的身份、版本或结构不一致".into());
        }
        let recipe: RecipeDoc = read_json(&self.root.join(&manifest.recipe))?;
        if recipe != self.recipe { return Err("发布配方的版本或实际布局不一致".into()); }
        let mut checked = BTreeSet::new();
        for entry in &manifest.files {
            let path = self.root.join(&entry.path);
            reject_links_once(&path, &mut checked)?;
            let metadata = fs::metadata(&path).map_err(|e| format!("读取发布资源 {} 失败：{e}", entry.path))?;
            if !metadata.is_file() || metadata.len() != entry.bytes {
                return Err(format!("发布资源 {} 的文件类型或大小不符", entry.path));
            }
        }
        let mut actual = BTreeSet::new();
        inventory_once(&self.root, &self.root, &mut actual, &mut checked)?;
        let mut expected: BTreeSet<_> = manifest.files.iter().map(|entry| entry.path.clone()).collect();
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
    let bundle_id = new_bundle_id()?;
    let manifest = ReleaseManifest {
        schema_version: RELEASE_SCHEMA,
        recipe_id: recipe.id.clone(), recipe_revision: recipe.revision_id.clone(), recipe_version: recipe.version, bundle_id: bundle_id.clone(),
        versions: input.versions, recipe: "recipe.json".into(), graph: "graph.json".into(), shots,
        files: files.iter().map(|(path, bytes)| FileEntry { path: path.clone(), bytes: bytes.len() as u64 }).collect(),
    };
    let manifest_bytes = json_bytes(&manifest)?;
    let root = absolute_path(releases_root)?;
    ensure_directory(&root)?;
    let recipe_root = root.join(&recipe.id);
    ensure_directory(&recipe_root)?;
    let target = recipe_root.join(&bundle_id);
    if path_exists(&target)? {
        return Err("发布 ID 已存在，已有发布包不会被改写".into());
    }
    let stage = Staging::new(&recipe_root)?;
    for (name, bytes) in &files {
        let path = resource_path(&stage.path, name)?;
        ensure_directory(path.parent().ok_or("发布资源没有父目录")?)?;
        write_new(&path, bytes)?;
    }
    write_new(&stage.path.join("manifest.json"), &manifest_bytes)?;
    load_directory(&stage.path, &recipe.id, &bundle_id)?;
    if path_exists(&target)? {
        return Err("发布 ID 已存在，已有发布包不会被改写".into());
    }
    match fs::rename(&stage.path, &target) {
        Ok(()) => load(&root, &recipe.id, &bundle_id),
        Err(_) if path_exists(&target)? => Err("发布 ID 已存在，已有发布包不会被改写".into()),
        Err(e) => Err(format!("原子发布目录 {} 失败：{e}", target.display())),
    }
}

pub fn load(releases_root: &Path, recipe_id: &str, bundle_id: &str) -> Result<ReleaseBundle, String> {
    safe_id(recipe_id)?;
    valid_bundle_id(bundle_id)?;
    let root = absolute_path(releases_root)?.join(recipe_id).join(bundle_id);
    load_directory(&root, recipe_id, bundle_id)
}

fn load_directory(root: &Path, recipe_id: &str, bundle_id: &str) -> Result<ReleaseBundle, String> {
    safe_id(recipe_id)?;
    valid_bundle_id(bundle_id)?;
    reject_links(root, false)?;
    let manifest = read_manifest(root, bundle_id)?;
    if manifest.schema_version != RELEASE_SCHEMA {
        return Err(format!("发布包格式版本 {}，当前为 {RELEASE_SCHEMA}，需要重新发布", manifest.schema_version));
    }
    if manifest.recipe_id != recipe_id || manifest.recipe != "recipe.json" || manifest.graph != "graph.json" {
        return Err("发布清单的配方身份或资源引用无效".into());
    }
    validate_versions(&manifest.versions)?;
    let mut files = BTreeMap::new();
    let mut names = BTreeSet::new();
    for entry in &manifest.files {
        validate_relative(&entry.path)?;
        if entry.path == "manifest.json" || !names.insert(entry.path.to_ascii_lowercase()) {
            return Err(format!("发布清单资源重复或自引用：{}", entry.path));
        }
        let bytes = read_file(&resource_path(root, &entry.path)?)?;
        if bytes.len() as u64 != entry.bytes {
            return Err(format!("发布资源 {} 的大小不符", entry.path));
        }
        files.insert(entry.path.as_str(), bytes);
    }
    let recipe: RecipeDoc = parse_resource(&files, &manifest.recipe)?;
    let built = recipe.build()?;
    built.ready()?;
    if recipe.id != manifest.recipe_id || built.revision_id != manifest.recipe_revision || built.version != manifest.recipe_version || manifest.shots.len() != built.shot_count() {
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
    Ok(ReleaseBundle { id: bundle_id.into(), root: root.to_path_buf(), manifest, recipe })
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

fn read_manifest(root: &Path, bundle_id: &str) -> Result<ReleaseManifest, String> {
    let value: serde_json::Value = serde_json::from_slice(&read_file(&root.join("manifest.json"))?).map_err(|e| format!("发布清单无法解析：{e}"))?;
    if value.get("bundleId").is_none() { return Err("发布清单缺少明确 bundleId，旧版发布包不再兼容，请重新发布".into()); }
    let manifest: ReleaseManifest = serde_json::from_value(value).map_err(|e| format!("发布清单格式不兼容，请重新发布：{e}"))?;
    if manifest.bundle_id != bundle_id { return Err("发布清单与包的显式 ID 身份不一致".into()); }
    Ok(manifest)
}

fn new_bundle_id() -> Result<String, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    Ok(format!("release-{time}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

fn valid_bundle_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 96 || reserved_component(id) || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
        return Err("发布包 ID 不是安全的目录名".into());
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
            recipe: RecipeDoc { id: "part-A".into(), name: "多分辨率配方".into(), version: 7, plan_version: 0, teaching_id: Some("teaching-1".into()), product_code: 11, trigger_mode: TriggerMode::Fly, schema_version: RECIPE_SCHEMA, spacing: 1.0, filter_window: 3, detect: default_detect(), limits: default_limits(), shots },
            versions: Versions { engine: "lyflow-test-1".into(), graph: "taught-path-v1".into() },
            graph: ResourceSource::Bytes(br#"{"schemaVersion":1,"nodes":[],"edges":[]}"#.to_vec()),
            shots: vec![
                ShotInput { k: 0, image: Some(ResourceSource::Bytes(pgm(32, 24, 50))), calibration: Some(ResourceSource::Bytes(br#"{"mmPerPx":0.5,"source":"manual"}"#.to_vec())) },
                ShotInput { k: 1, image: Some(ResourceSource::Bytes(pgm(64, 40, 80))), calibration: Some(ResourceSource::Bytes(br#"{"mmPerPx":0.5,"source":"manual"}"#.to_vec())) },
            ],
        }
    }

    fn copy_with_resources(directory: &Directory, bundle: &ReleaseBundle, mut manifest: ReleaseManifest, replacement: Option<(&str, Vec<u8>)>) -> String {
        let mut files: BTreeMap<String, Vec<u8>> = bundle.manifest.files.iter().map(|f| (f.path.clone(), fs::read(bundle.root.join(&f.path)).unwrap())).collect();
        if let Some((name, bytes)) = replacement {
            let entry = manifest.files.iter_mut().find(|f| f.path == name).unwrap();
            entry.bytes = bytes.len() as u64;
            files.insert(name.into(), bytes);
        }
        let id = new_bundle_id().unwrap();
        manifest.bundle_id = id.clone();
        let bytes = json_bytes(&manifest).unwrap();
        let root = directory.releases().join(&bundle.recipe.id).join(&id);
        fs::create_dir_all(&root).unwrap();
        for (name, bytes) in files {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        fs::write(root.join("manifest.json"), bytes).unwrap();
        id
    }

    #[test]
    fn old_format_release_is_rejected_instead_of_guessing_identity() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let old_directory = "0123456789abcdef";
        let root = directory.releases().join(&bundle.recipe.id).join(old_directory);
        fs::rename(&bundle.root, &root).unwrap();
        let mut manifest = serde_json::to_value(&bundle.manifest).unwrap();
        manifest.as_object_mut().unwrap().remove("bundleId");
        manifest.as_object_mut().unwrap().remove("recipeRevision");
        manifest["recipeHash"] = serde_json::json!("old-value");
        for entry in manifest["files"].as_array_mut().unwrap() { entry["hash"] = serde_json::json!("old-value"); }
        fs::write(root.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(load(&directory.releases(), "part-A", old_directory).unwrap_err().contains("旧版发布包"));
        manifest["bundleId"] = serde_json::json!(old_directory);
        fs::write(root.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(load(&directory.releases(), "part-A", old_directory).unwrap_err().contains("不兼容"));
        manifest["recipeRevision"] = serde_json::json!(bundle.manifest.recipe_revision);
        fs::write(root.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(load(&directory.releases(), "part-A", old_directory).unwrap_err().contains("hash"));
    }

    #[test]
    fn new_release_requires_explicit_id_on_load_and_after_bundle_has_been_loaded() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let path = bundle.root.join("manifest.json");
        let mut value = serde_json::to_value(&bundle.manifest).unwrap();
        value.as_object_mut().unwrap().remove("bundleId");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(load(&directory.releases(), "part-A", &bundle.id).unwrap_err().contains("bundleId"));
        assert!(bundle.verify().unwrap_err().contains("bundleId"));
        value["bundleId"] = serde_json::json!("another-explicit-id");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(load(&directory.releases(), "part-A", &bundle.id).unwrap_err().contains("显式 ID"));
        assert!(bundle.verify().unwrap_err().contains("显式 ID"));
    }

    #[test]
    #[ignore = "requires LYFLOW_CORE_DLL; actual Prepared load and per-arm identity verification"]
    fn native_prepared_rejects_missing_explicit_bundle_identity_on_later_verification() {
        let dll = std::env::var_os("LYFLOW_CORE_DLL").expect("Set LYFLOW_CORE_DLL to a taught-path core");
        let engine = std::sync::Arc::new(crate::vision::Engine::load(Path::new(&dll)).unwrap());
        let directory = Directory::new();
        let mut source = input();
        let recipe = source.recipe.build().unwrap();
        source.versions = Versions { engine: engine.identity.clone(), graph: crate::production::GRAPH_VERSION.into() };
        source.graph = ResourceSource::Bytes(serde_json::to_vec(&crate::production::graphs(&recipe).unwrap()).unwrap());
        let bundle = publish(&directory.releases(), source).unwrap();
        let prepared = crate::production::Prepared::load(bundle.clone(), engine, &recipe).unwrap();
        assert!(prepared.engine_note.is_none());
        prepared.verify().unwrap();
        let mut value = serde_json::to_value(&bundle.manifest).unwrap();
        value.as_object_mut().unwrap().remove("bundleId");
        fs::write(bundle.root.join("manifest.json"), serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(prepared.verify().unwrap_err().contains("bundleId"));
    }

    #[test]
    fn recreated_recipe_freezes_and_publishes_the_same_unconsumed_next_version() {
        let directory = Directory::new();
        let recipes_dir = directory.0.join("recipes");
        let recipes = crate::recipe::RecipeStore::open(recipes_dir.clone()).unwrap();
        let mut first = input().recipe;
        first.plan_version = recipes.plan_version_for(&first).unwrap();
        let original = recipes.save_published(first, None).unwrap();
        recipes.delete(&original.id).unwrap();
        drop(recipes);
        let recipes = crate::recipe::RecipeStore::open(recipes_dir).unwrap();
        let mut candidate = input();
        candidate.recipe.version = recipes.next_version(&original.id).unwrap();
        candidate.recipe.plan_version = recipes.plan_version_for(&candidate.recipe).unwrap();
        assert_eq!(candidate.recipe.version, 8);
        // 删掉重建的配方拿新的计划版本，不复用删除前的号
        assert!(candidate.recipe.plan_version > original.plan_version);
        let frozen = publish(&directory.releases(), candidate).unwrap();
        let saved = recipes.save_published(frozen.recipe.clone(), None).unwrap();
        assert_eq!(saved.revision_id, frozen.manifest.recipe_revision);
        assert_eq!(saved.version, frozen.manifest.recipe_version);
        assert_eq!(recipes.save_published(frozen.recipe, None).unwrap().version, 8);
    }

    #[test]
    fn package_resolves_each_shot_with_its_own_image_size_and_geometry() {
        let directory = Directory::new();
        let input = input();
        let bundle = publish(&directory.releases(), input.clone()).unwrap();
        assert_eq!(bundle.recipe, input.recipe);
        assert_eq!(bundle.manifest.recipe_revision, input.recipe.build().unwrap().revision_id);
        assert_eq!(bundle.manifest.recipe_version, 7);
        assert_eq!(bundle.id, bundle.manifest.bundle_id);
        assert_ne!(bundle.id, bundle.manifest.recipe_revision);
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
        load(&directory.releases(), "part-A", &bundle.id).unwrap().verify().unwrap();
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
        assert!(load(&directory.releases(), "part-A", &bundle.id).is_err());
        fs::write(&calibration, original).unwrap();
        fs::remove_file(resource.image.unwrap()).unwrap();
        assert!(bundle.verify().unwrap_err().contains("image.pgm"));
    }

    #[test]
    fn loaded_bundle_rejects_public_identity_changes() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let mut changed = bundle.clone();
        changed.id = "0000000000000000".into();
        assert!(changed.verify().unwrap_err().contains("身份"));
        let mut changed = bundle.clone();
        changed.manifest.files[0].bytes += 1;
        assert!(changed.verify().is_err());
        let mut changed = bundle.clone();
        changed.recipe.shots[0].path[0][0] += 1.0;
        assert!(changed.verify().is_err());
        bundle.verify().unwrap();
    }

    #[test]
    fn resource_validation_uses_paths_sizes_and_structure() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let path = bundle.shot(0).unwrap().image.unwrap();
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, &bytes).unwrap();
        bundle.verify().unwrap();
        load(&directory.releases(), "part-A", &bundle.id).unwrap();
        bytes.push(0);
        fs::write(&path, &bytes).unwrap();
        assert!(bundle.verify().unwrap_err().contains("大小"));
        bytes.pop();
        fs::write(&path, &bytes).unwrap();
        let extra = bundle.root.join("shots/00/extra.json");
        fs::write(&extra, b"{}").unwrap();
        assert!(bundle.verify().unwrap_err().contains("清单以外"));
        fs::remove_file(extra).unwrap();
        fs::create_dir(bundle.root.join("empty")).unwrap();
        bundle.verify().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn loaded_bundle_rejects_windows_resource_and_ancestor_junctions() {
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
        assert_ne!(new.id, old.id);
        assert_eq!(new.manifest.recipe_revision, old.manifest.recipe_revision);
        fs::remove_file(&calib_source).unwrap();
        fs::remove_file(&image_source).unwrap();
        old.verify().unwrap();
        new.verify().unwrap();
        let old_resources = old.shot(0).unwrap();
        assert_eq!(fs::read(old_resources.calibration.unwrap()).unwrap(), first_calibration);
        assert_eq!(image::open(old_resources.image.unwrap()).unwrap().to_luma8().get_pixel(0, 0).0, [50]);
    }

    #[test]
    fn each_publication_has_an_explicit_id_without_overwriting_previous_packages() {
        let directory = Directory::new();
        let original = input();
        let first = publish(&directory.releases(), original.clone()).unwrap();
        let second = publish(&directory.releases(), original.clone()).unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(first.root, second.root);
        let manifest_path = first.root.join("manifest.json");
        fs::write(&manifest_path, b"corrupt").unwrap();
        let third = publish(&directory.releases(), original).unwrap();
        third.verify().unwrap();
        assert_eq!(fs::read(&manifest_path).unwrap(), b"corrupt");
        assert_eq!(fs::read_dir(first.root.parent().unwrap()).unwrap().count(), 3);
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
        let id = copy_with_resources(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &id).unwrap_err().contains("逃逸"));
    }

    #[test]
    fn manifest_duplicate_resources_and_wrong_recipe_identity_are_rejected() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let mut manifest = bundle.manifest.clone();
        manifest.files.push(manifest.files[0].clone());
        let id = copy_with_resources(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &id).unwrap_err().contains("重复"));
        let mut manifest = bundle.manifest.clone();
        manifest.recipe_id = "another-part".into();
        let id = copy_with_resources(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &id).unwrap_err().contains("身份"));
        let mut manifest = bundle.manifest.clone();
        manifest.recipe_version += 1;
        let id = copy_with_resources(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &id).unwrap_err().contains("版本"));
        let mut manifest = bundle.manifest.clone();
        manifest.shots[1].view = 1;
        let id = copy_with_resources(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &id).unwrap_err().contains("身份"));
    }

    #[test]
    fn geometry_resources_match_the_recipe_coordinates() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let name = bundle.manifest.shots[1].points.clone();
        let mut points = bundle.shot(1).unwrap().points_data;
        points.camera = "cam1".into();
        let id = copy_with_resources(&directory, &bundle, bundle.manifest.clone(), Some((&name, json_bytes(&points).unwrap())));
        assert!(load(&directory.releases(), "part-A", &id).unwrap_err().contains("测点与配方快照不一致"));
        let mut manifest = bundle.manifest.clone();
        manifest.shots[1].size = Some([32, 24]);
        let id = copy_with_resources(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &id).unwrap_err().contains("原图尺寸"));
    }

    #[test]
    fn unsupported_schema_and_unlisted_files_refuse_loading() {
        let directory = Directory::new();
        let bundle = publish(&directory.releases(), input()).unwrap();
        let mut manifest = bundle.manifest.clone();
        manifest.schema_version = 0;
        let id = copy_with_resources(&directory, &bundle, manifest, None);
        assert!(load(&directory.releases(), "part-A", &id).unwrap_err().contains("需要重新发布"));
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
