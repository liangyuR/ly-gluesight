use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::recipe::{CameraInspect, Recipe, RecipeDoc};

/// 段配方的发布包，按计划相机组织（D-SB-30、D-SB-45）：每台相机一份标定，它检测的每一段一张示教帧与一条走廊。
/// 其他版本按版本不符拒绝；按相机一条走廊的 4 已放弃，需要重新发布。
pub const RELEASE_SCHEMA_BURST: u32 = 5;
/// 旧拍照点配方的发布包版本，已停用：加载时给出明确提示。
const LEGACY_RELEASE_SCHEMA: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Versions {
    pub engine: String,
    pub graph: String,
}

/// 发布时所在站点：离线环境验证发布的版本不能在真机开工。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidationSite { Real, Offline }

#[derive(Clone, Debug)]
pub enum ResourceSource {
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// 段配方一台检测相机的发布资源：标定按相机，示教帧按它检测的每一段。
#[derive(Clone, Debug)]
pub struct CameraInput {
    pub camera: String,
    pub calibration: ResourceSource,
    pub corridors: Vec<CorridorInput>,
}

/// 一台相机在一段的示教帧（走廊在它上面示教，预热在它上面量一次）。
#[derive(Clone, Debug)]
pub struct CorridorInput {
    pub segment: u32,
    pub image: ResourceSource,
    pub image_id: String,
}

/// 段配方的发布输入（schema 5）。`graph` 是 `production::burst_graphs` 的结果，生产加载时按配方重算比对。
#[derive(Clone, Debug)]
pub struct BurstPublishInput {
    pub recipe: RecipeDoc,
    pub versions: Versions,
    pub validation_site: ValidationSite,
    pub graph: ResourceSource,
    pub cameras: Vec<CameraInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub bytes: u64,
}

/// 段配方发布包里一台计划相机（schema 5）：资源都在 `cameras/{index:02}/` 下，各段的在 `s{segment:02}/` 子目录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CameraManifest {
    /// 计划相机下标（配方 inspect 的顺序）
    pub index: usize,
    pub camera: String,
    /// 示教帧尺寸：这台相机各段示教帧同尺寸，生产每帧必须同尺寸
    pub size: [u32; 2],
    pub calibration: String,
    /// 它检测的各段，按配方段顺序
    pub corridors: Vec<CorridorManifest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorridorManifest {
    pub segment: u32,
    pub image: String,
    pub image_id: String,
    pub corridor: String,
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
    pub validation_site: ValidationSite,
    pub recipe: String,
    pub graph: String,
    /// 已发布的 schema 4 清单都带这个空数组，保留以兼容读入；必须为空
    #[serde(default)]
    pub shots: Vec<serde_json::Value>,
    pub cameras: Vec<CameraManifest>,
    pub files: Vec<FileEntry>,
}

/// 段配方一台相机在一段的走廊资源：冻结的走廊几何、当量与站距，加载时按配方重算比对。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorridorResource {
    pub schema_version: u32,
    pub camera: String,
    pub segment: u32,
    pub size: [u32; 2],
    pub spacing: f32,
    pub mm_per_px: Option<f64>,
    pub corridor: Vec<[f32; 2]>,
}

/// 段配方一台计划相机的资源（绝对路径）。
#[derive(Clone, Debug)]
pub struct CameraResources {
    pub dir: PathBuf,
    pub camera: String,
    pub size: [u32; 2],
    pub calibration: PathBuf,
    pub corridors: Vec<CorridorResources>,
}

/// 一台相机在一段的示教帧（绝对路径）。
#[derive(Clone, Debug)]
pub struct CorridorResources {
    pub segment: u32,
    pub image: PathBuf,
    pub image_id: String,
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
        reject_links(&self.root, false)?;
        let mut actual = BTreeMap::new();
        inventory(&self.root, &self.root, &mut actual)?;
        let listed = actual.remove("manifest.json").is_some();
        for entry in &manifest.files {
            match actual.remove(&entry.path) {
                Some(bytes) if bytes == entry.bytes => {}
                Some(_) => return Err(format!("发布资源 {} 的文件类型或大小不符", entry.path)),
                None => return Err(format!("发布资源 {} 缺失或不是普通文件", entry.path)),
            }
        }
        if !listed || !actual.is_empty() { return Err("发布目录存在清单以外的文件或缺失文件".into()); }
        Ok(())
    }

    /// 段配方第 index 台计划相机的资源（schema 5）。
    pub fn camera(&self, index: usize) -> Result<CameraResources, String> {
        self.verify()?;
        let inspect = self.recipe.inspect.get(index).ok_or("发布包内没有该计划相机")?;
        let entry = self.manifest.cameras.get(index).filter(|c| c.index == index && c.camera == inspect.camera)
            .ok_or("发布清单内缺少该计划相机或与配方顺序不一致")?;
        let corridors = entry
            .corridors
            .iter()
            .map(|c| {
                read_json::<CorridorResource>(&resource_path(&self.root, &c.corridor)?)?;
                Ok(CorridorResources {
                    segment: c.segment,
                    image: resource_path(&self.root, &c.image)?,
                    image_id: c.image_id.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(CameraResources {
            dir: resource_path(&self.root, &camera_prefix(index))?,
            camera: entry.camera.clone(),
            size: entry.size,
            calibration: resource_path(&self.root, &entry.calibration)?,
            corridors,
        })
    }
}

/// 段配方一台计划相机的资源目录（相对发布包根）。
fn camera_prefix(index: usize) -> String {
    format!("cameras/{index:02}")
}

/// 段配方一台相机一段的资源目录（相对发布包根）。
fn corridor_prefix(index: usize, segment: u32) -> String {
    format!("{}/s{segment:02}", camera_prefix(index))
}

/// 这台相机检测的各段（按配方段顺序）：发布包要为每一段冻结一张示教帧和一条走廊。
fn detected_segments(recipe: &Recipe, camera: &str) -> Vec<u32> {
    let cameras = recipe.plan_cameras();
    recipe
        .path_segments
        .iter()
        .filter(|s| {
            crate::plan::segment_cameras(s, &cameras)
                .iter()
                .any(|c| c == camera)
        })
        .map(|s| s.index)
        .collect()
}

fn corridor_data(
    recipe: &Recipe,
    inspect: &CameraInspect,
    segment: u32,
    size: [u32; 2],
) -> CorridorResource {
    CorridorResource {
        schema_version: RELEASE_SCHEMA_BURST,
        camera: inspect.camera.clone(),
        segment,
        size,
        spacing: recipe.spacing,
        mm_per_px: inspect.mm_per_px,
        corridor: inspect
            .segment_corridor(segment)
            .map(|s| s.corridor.clone())
            .unwrap_or_default(),
    }
}

fn check_corridor(inspect: &CameraInspect, segment: u32, size: [u32; 2]) -> Result<(), String> {
    let corridor = inspect
        .segment_corridor(segment)
        .map(|s| s.corridor.as_slice())
        .unwrap_or_default();
    if !crate::vision::corridor_inside(corridor, size) {
        return Err(format!(
            "相机 {} 第 {segment} 段的检测走廊超出示教帧 {}×{}",
            inspect.camera, size[0], size[1]
        ));
    }
    Ok(())
}

/// 段配方的发布（schema 5，D-SB-30、D-SB-45）：按计划相机冻结标定，按它检测的每一段冻结示教帧与走廊。前置
/// `Recipe::ready_burst`（每段每台检测相机都示教了走廊、相机有当量、已标定）；资源必须恰好覆盖每台计划相机一次、
/// 每台相机恰好覆盖它检测的各段；一台相机的各段示教帧同尺寸；走廊必须落在所在段的示教帧内。
/// 测量图由调用方给（`production::burst_graphs`），生产加载时按配方重算比对。
pub fn publish_burst(releases_root: &Path, input: BurstPublishInput) -> Result<ReleaseBundle, String> {
    let recipe = input.recipe.build()?;
    recipe.ready_burst().map_err(|reasons| reasons.join("；"))?;
    safe_id(&recipe.id)?;
    validate_versions(&input.versions)?;
    let mut inputs = BTreeMap::new();
    for camera in input.cameras {
        if !recipe.inspect.iter().any(|i| i.camera == camera.camera) {
            return Err(format!("发布资源引用了不是检测相机的 {}", camera.camera));
        }
        let id = camera.camera.clone();
        if inputs.insert(id.clone(), camera).is_some() {
            return Err(format!("相机 {id} 的发布资源重复"));
        }
    }
    if inputs.len() != recipe.inspect.len() {
        return Err("发布资源未覆盖全部检测相机".into());
    }
    let mut files = BTreeMap::new();
    add_file(&mut files, "recipe.json".into(), json_bytes(&input.recipe)?)?;
    let graph = source_bytes(input.graph)?;
    json_object(&graph, "测量图")?;
    add_file(&mut files, "graph.json".into(), graph)?;
    let mut cameras = Vec::new();
    for (index, inspect) in recipe.inspect.iter().enumerate() {
        let source = inputs.remove(&inspect.camera).ok_or("发布资源缺少检测相机")?;
        let label = format!("相机 {}", inspect.camera);
        let expected = detected_segments(&recipe, &inspect.camera);
        let mut sources: BTreeMap<u32, CorridorInput> = BTreeMap::new();
        for corridor in source.corridors {
            let segment = corridor.segment;
            if !expected.contains(&segment) || sources.insert(segment, corridor).is_some() {
                return Err(format!(
                    "{label} 第 {segment} 段的发布资源重复或该段不由这台相机检测"
                ));
            }
        }
        let prefix = camera_prefix(index);
        let mut size = None;
        let mut corridors = Vec::new();
        for segment in expected {
            let source = sources
                .remove(&segment)
                .ok_or_else(|| format!("{label} 缺少第 {segment} 段的示教帧"))?;
            if !crate::recipe::valid_image_id(&source.image_id) {
                return Err(format!(
                    "{label} 第 {segment} 段的发布示教帧缺少有效的示教图编号"
                ));
            }
            let bytes = source_bytes(source.image)?;
            let (frame, extension) = image_details(&bytes, &label)?;
            if *size.get_or_insert(frame) != frame {
                return Err(format!("{label} 各段示教帧尺寸不同"));
            }
            check_corridor(inspect, segment, frame)?;
            let dir = corridor_prefix(index, segment);
            let image = format!("{dir}/image.{extension}");
            add_file(&mut files, image.clone(), bytes)?;
            let corridor = format!("{dir}/corridor.json");
            add_file(
                &mut files,
                corridor.clone(),
                json_bytes(&corridor_data(&recipe, inspect, segment, frame))?,
            )?;
            corridors.push(CorridorManifest {
                segment,
                image,
                image_id: source.image_id,
                corridor,
            });
        }
        let size = size.ok_or_else(|| format!("{label} 不检测任何一段"))?;
        let calibration_bytes = source_bytes(source.calibration)?;
        json_object(&calibration_bytes, &format!("{label} 标定"))?;
        let calibration = format!("{prefix}/calibration.json");
        add_file(&mut files, calibration.clone(), calibration_bytes)?;
        cameras.push(CameraManifest {
            index,
            camera: inspect.camera.clone(),
            size,
            calibration,
            corridors,
        });
    }
    let bundle_id = new_bundle_id()?;
    let manifest = ReleaseManifest {
        schema_version: RELEASE_SCHEMA_BURST,
        recipe_id: recipe.id.clone(), recipe_revision: recipe.revision_id.clone(), recipe_version: recipe.version, bundle_id: bundle_id.clone(),
        versions: input.versions, validation_site: input.validation_site, recipe: "recipe.json".into(), graph: "graph.json".into(),
        shots: Vec::new(), cameras,
        files: files.iter().map(|(path, bytes)| FileEntry { path: path.clone(), bytes: bytes.len() as u64 }).collect(),
    };
    write_bundle(releases_root, &recipe.id, &bundle_id, &files, &manifest)
}

/// 先写进同目录的临时目录、按加载规则核对一遍，再原子改名成发布包目录；已有的发布包不会被改写。
fn write_bundle(releases_root: &Path, recipe_id: &str, bundle_id: &str, files: &BTreeMap<String, Vec<u8>>, manifest: &ReleaseManifest) -> Result<ReleaseBundle, String> {
    let manifest_bytes = json_bytes(manifest)?;
    let root = absolute_path(releases_root)?;
    ensure_directory(&root)?;
    let recipe_root = root.join(recipe_id);
    ensure_directory(&recipe_root)?;
    let target = recipe_root.join(bundle_id);
    if path_exists(&target)? {
        return Err("发布 ID 已存在，已有发布包不会被改写".into());
    }
    let stage = Staging::new(&recipe_root)?;
    for (name, bytes) in files {
        let path = resource_path(&stage.path, name)?;
        ensure_directory(path.parent().ok_or("发布资源没有父目录")?)?;
        write_new(&path, bytes)?;
    }
    write_new(&stage.path.join("manifest.json"), &manifest_bytes)?;
    load_directory(&stage.path, recipe_id, bundle_id)?;
    if path_exists(&target)? {
        return Err("发布 ID 已存在，已有发布包不会被改写".into());
    }
    match fs::rename(&stage.path, &target) {
        Ok(()) => load(&root, recipe_id, bundle_id),
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

/// 只读清单并按文件元数据核对资源齐全、大小一致，不读取资源内容。
pub fn peek(releases_root: &Path, recipe_id: &str, bundle_id: &str) -> Result<ReleaseManifest, String> {
    safe_id(recipe_id)?;
    valid_bundle_id(bundle_id)?;
    let root = absolute_path(releases_root)?.join(recipe_id).join(bundle_id);
    reject_links(&root, false)?;
    let manifest = read_manifest(&root, bundle_id)?;
    supported_schema(manifest.schema_version)?;
    if manifest.recipe_id != recipe_id { return Err("发布清单的配方身份无效".into()); }
    for entry in &manifest.files {
        validate_relative(&entry.path)?;
        let metadata = fs::symlink_metadata(resource_path(&root, &entry.path)?).map_err(|e| format!("读取发布资源 {} 失败：{e}", entry.path))?;
        if !metadata.is_file() || is_link(&metadata) || metadata.len() != entry.bytes {
            return Err(format!("发布资源 {} 的文件类型或大小不符", entry.path));
        }
    }
    Ok(manifest)
}

fn supported_schema(version: u32) -> Result<(), String> {
    if version == LEGACY_RELEASE_SCHEMA {
        return Err(format!("发布包格式版本 {version} 是旧拍照点配方的发布包，旧拍照点配方已停用，请新建段配方并重新发布"));
    }
    if version != RELEASE_SCHEMA_BURST {
        return Err(format!(
            "发布包格式版本 {version}，当前为 {RELEASE_SCHEMA_BURST}（段配方），需要重新发布"
        ));
    }
    Ok(())
}

fn load_directory(root: &Path, recipe_id: &str, bundle_id: &str) -> Result<ReleaseBundle, String> {
    safe_id(recipe_id)?;
    valid_bundle_id(bundle_id)?;
    reject_links(root, false)?;
    let manifest = read_manifest(root, bundle_id)?;
    supported_schema(manifest.schema_version)?;
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
    let referenced = camera_resources(&manifest, &files, &recipe, &built)?;
    let declared: BTreeSet<_> = manifest.files.iter().map(|f| f.path.clone()).collect();
    if referenced != declared {
        return Err("发布清单存在未声明、未引用或重复资源".into());
    }
    let mut actual = BTreeMap::new();
    inventory(root, root, &mut actual)?;
    let mut expected = declared;
    expected.insert("manifest.json".into());
    if actual.into_keys().collect::<BTreeSet<_>>() != expected {
        return Err("发布目录存在清单以外的文件或缺失文件".into());
    }
    Ok(ReleaseBundle { id: bundle_id.into(), root: root.to_path_buf(), manifest, recipe })
}

/// 段配方（schema 5）的资源核对，返回引用到的资源：清单按配方 inspect 顺序列出每台计划相机，资源在 `cameras/{index:02}/`，
/// 每台相机按配方段顺序列出它检测的各段（`s{segment:02}/`）；各段示教帧解码后的尺寸都与清单一致、走廊落在帧内；
/// 标定是非空 JSON 对象；走廊资源按配方重算比对。
fn camera_resources(manifest: &ReleaseManifest, files: &BTreeMap<&str, Vec<u8>>, recipe: &RecipeDoc, built: &Recipe) -> Result<BTreeSet<String>, String> {
    built.ready_burst().map_err(|reasons| reasons.join("；"))?;
    if recipe.id != manifest.recipe_id || built.revision_id != manifest.recipe_revision || built.version != manifest.recipe_version
        || !manifest.shots.is_empty() || manifest.cameras.len() != built.inspect.len() {
        return Err("发布清单与配方快照的身份、版本或检测相机数量不一致".into());
    }
    json_object(file_bytes(files, &manifest.graph)?, "发布测量图")?;
    let mut referenced = BTreeSet::from([manifest.recipe.clone(), manifest.graph.clone()]);
    for ((index, inspect), entry) in built.inspect.iter().enumerate().zip(&manifest.cameras) {
        let label = format!("相机 {}", inspect.camera);
        if entry.index != index || entry.camera != inspect.camera {
            return Err(format!("发布清单第 {} 台相机的身份与配方不一致", index + 1));
        }
        let prefix = camera_prefix(index);
        if entry.calibration != format!("{prefix}/calibration.json") {
            return Err(format!("{label} 的标定资源引用无效"));
        }
        validate_relative(&entry.calibration)?;
        if !referenced.insert(entry.calibration.clone()) {
            return Err(format!("相机资源被重复引用：{}", entry.calibration));
        }
        json_object(file_bytes(files, &entry.calibration)?, &format!("{label} 标定"))?;
        let segments: Vec<u32> = entry.corridors.iter().map(|c| c.segment).collect();
        if segments != detected_segments(built, &inspect.camera) {
            return Err(format!("{label} 的各段资源与配方的检测段不一致"));
        }
        for corridor in &entry.corridors {
            let (segment, dir) = (corridor.segment, corridor_prefix(index, corridor.segment));
            if corridor.corridor != format!("{dir}/corridor.json") {
                return Err(format!("{label} 第 {segment} 段的走廊资源引用无效"));
            }
            if !crate::recipe::valid_image_id(&corridor.image_id) {
                return Err(format!("{label} 第 {segment} 段的示教图编号格式无效"));
            }
            for name in [&corridor.image, &corridor.corridor] {
                validate_relative(name)?;
                if !referenced.insert(name.clone()) {
                    return Err(format!("相机资源被重复引用：{name}"));
                }
            }
            let (size, extension) = image_details(file_bytes(files, &corridor.image)?, &label)?;
            if entry.size != size || corridor.image != format!("{dir}/image.{extension}") {
                return Err(format!(
                    "{label} 第 {segment} 段的示教帧尺寸或引用与清单不一致"
                ));
            }
            check_corridor(inspect, segment, size)?;
            let resource: CorridorResource = parse_resource(files, &corridor.corridor)?;
            if resource != corridor_data(built, inspect, segment, size) {
                return Err(format!("{label} 第 {segment} 段的走廊与配方快照不一致"));
            }
        }
    }
    Ok(referenced)
}

/// 原图尺寸与扩展名（按 `image_file::decode` 解码核对）。`label` 是报错主语（如“相机 camA”）。
fn image_details(bytes: &[u8], label: &str) -> Result<([u32; 2], &'static str), String> {
    let format = image::guess_format(bytes).map_err(|e| format!("{label} 原图格式无效：{e}"))?;
    let extension = match format {
        image::ImageFormat::Pnm => "pgm",
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::Png => "png",
        image::ImageFormat::Bmp => "bmp",
        image::ImageFormat::Tiff => "tiff",
        _ => return Err(format!("{label} 原图格式不支持")),
    };
    let image = crate::image_file::decode(bytes).map_err(|e| format!("{label} 原图解码失败：{e}"))?;
    if image.width == 0 || image.height == 0 {
        return Err(format!("{label} 原图尺寸无效"));
    }
    Ok(([image.width, image.height], extension))
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
    let value: serde_json::Value = serde_json::from_slice(&read_file(&root.join("manifest.json"))?)
        .map_err(|e| format!("发布清单无法解析：{e}"))?;
    if let Some(version) = value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
    {
        supported_schema(u32::try_from(version).unwrap_or(u32::MAX))?;
    }
    if value.get("bundleId").is_none() { return Err("发布清单缺少明确 bundleId，旧版发布包不再兼容，请重新发布".into()); }
    if value.get("validationSite").is_none() { return Err("发布清单缺少验证现场标记（真机 / 离线），旧版发布包不再兼容，请重新验证并发布".into()); }
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

/// 调用方先核对根目录及其上级不是链接；目录项元数据不跟随链接，Windows 上直接取自目录枚举，不逐个打开文件。
fn inventory(root: &Path, dir: &Path, files: &mut BTreeMap<String, u64>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("读取发布目录 {} 失败：{e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|e| e.to_string())?;
        if is_link(&metadata) { return Err(format!("发布目录包含符号链接或目录重解析点：{}", path.display())); }
        if metadata.is_dir() { inventory(root, &path, files)?; }
        else if metadata.is_file() {
            let relative = path.strip_prefix(root).map_err(|_| "发布资源逃逸目录")?.to_str().ok_or("发布文件名不是 UTF-8")?.replace('\\', "/");
            validate_relative(&relative)?;
            if files.insert(relative, metadata.len()).is_some() { return Err("发布目录包含重复资源".into()); }
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
            && reject_links(&self.path, false).is_ok()
            && inventory(&self.path, &self.path, &mut BTreeMap::new()).is_ok()
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl Drop for TempDir { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

    /// 1000×700 的灰度渐变原图：模板裁到哪一块，字节就不同
    fn image_bytes() -> Vec<u8> {
        let mut image = b"P5\n1000 700\n255\n".to_vec();
        image.extend((0..700u32).flat_map(|y| (0..1000u32).map(move |x| ((x * 7 + y * 3) % 251) as u8)));
        image
    }

    /// 段配方样例（单个直线段，三台相机在第 1 段的走廊 (980, 480)–(300, 460)，0.112 mm/px）按相机发布，三台都用 1000×700 的渐变图作示教帧。
    fn publish_burst_bundle(root: &Path) -> ReleaseBundle {
        let recipe = crate::recipe::samples_burst();
        let cameras = recipe
            .inspect
            .iter()
            .map(|i| CameraInput {
                camera: i.camera.clone(),
                calibration: ResourceSource::Bytes(br#"{"mmPerPx":0.112}"#.to_vec()),
                corridors: vec![CorridorInput {
                    segment: 1,
                    image: ResourceSource::Bytes(image_bytes()),
                    image_id: format!("capture-1-0-{}", i.camera),
                }],
            })
            .collect();
        publish_burst(
            root,
            BurstPublishInput {
                recipe,
                validation_site: ValidationSite::Offline,
                versions: Versions {
                    engine: "1.1.0".into(),
                    graph: crate::production::GRAPH_VERSION_BURST.into(),
                },
                graph: ResourceSource::Bytes(br#"{"graph":1}"#.to_vec()),
                cameras,
            },
        )
        .unwrap()
    }

    /// 改写包内一个资源，并把清单里它的大小同步改掉（只留下按配方重算才查得出的不一致）。

    #[test]
    fn bundle_changed_on_disk_after_publish_is_rejected_by_per_part_verify() {
        let root = TempDir(std::env::temp_dir().join(format!("gluesight-{}", new_bundle_id().unwrap())));
        let tamperings: [(&str, fn(&Path, &Path)); 5] = [
            ("extra file", |root, _| {
                fs::write(root.join("cameras").join("extra.json"), b"{}").unwrap()
            }),
            ("resource resized", |_, resource| {
                fs::write(resource, b"{}").unwrap()
            }),
            ("resource removed", |_, resource| {
                fs::remove_file(resource).unwrap()
            }),
            ("resource replaced by directory", |_, resource| {
                fs::remove_file(resource).unwrap();
                fs::create_dir(resource).unwrap();
            }),
            ("manifest removed", |root, _| {
                fs::remove_file(root.join("manifest.json")).unwrap()
            }),
        ];
        for (case, tamper) in tamperings {
            let bundle = publish_burst_bundle(&root.0);
            bundle.verify().unwrap();
            let resource = bundle
                .manifest
                .files
                .iter()
                .find(|entry| entry.path.starts_with("cameras/"))
                .unwrap();
            tamper(&bundle.root, &bundle.root.join(&resource.path));
            assert!(bundle.verify().is_err(), "{case}");
        }
    }
}
