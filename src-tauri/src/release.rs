use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::recipe::{Recipe, RecipeDoc, ShotSpec};

/// 3：启用 fixture 的视图带定位模板 template.pgm，清单记示教图编号（imageId）与模板引用。旧包按版本不符拒绝（D-0）。
pub const RELEASE_SCHEMA: u32 = 3;

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

#[derive(Clone, Debug)]
pub struct ShotInput {
    pub k: usize,
    pub view: u8,
    pub image: Option<ResourceSource>,
    /// 示教原图的冻结图编号（有图则必有）：特征定位的模板来源要等于它
    pub image_id: Option<String>,
    pub calibration: Option<ResourceSource>,
}

#[derive(Clone, Debug)]
pub struct PublishInput {
    pub recipe: RecipeDoc,
    pub versions: Versions,
    pub validation_site: ValidationSite,
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
    /// 示教原图的冻结图编号（有图则必有），追溯用；fixture 视图上必须等于配方里的模板来源
    pub image_id: Option<String>,
    /// 定位模板（只在启用 fixture 的视图上）：shots/kk/vN/template.pgm
    pub template: Option<String>,
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
    pub validation_site: ValidationSite,
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
    /// 这个（拍照点，视图）的资源目录（绝对路径）：特征定位的 base_dir
    pub dir: PathBuf,
    pub size: Option<[u32; 2]>,
    pub image: Option<PathBuf>,
    pub template: Option<PathBuf>,
    pub calibration: Option<PathBuf>,
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

    pub fn view(&self, k: usize, view: u8) -> Result<ShotResources, String> {
        self.verify()?;
        self.recipe.shots.get(k).ok_or("发布包内没有该拍照点")?.for_view(view)?;
        let entry = self.manifest.shots.iter().find(|s| s.k == k && s.view == view).ok_or("发布清单内缺少拍照点图像")?;
        let centerline = resource_path(&self.root, &entry.centerline)?;
        let points = resource_path(&self.root, &entry.points)?;
        let image = entry.image.as_ref().map(|p| resource_path(&self.root, p)).transpose()?;
        let calibration = entry.calibration.as_ref().map(|p| resource_path(&self.root, p)).transpose()?;
        let template = entry.template.as_ref().map(|p| resource_path(&self.root, p)).transpose()?;
        read_json::<Centerline>(&centerline)?;
        read_json::<MeasurePoints>(&points)?;
        resource_path(&self.root, &self.manifest.graph)?;
        let dir = resource_path(&self.root, &view_prefix(k, view))?;
        Ok(ShotResources {
            dir, size: entry.size, image, template, calibration,
        })
    }
}

/// 一个（拍照点，视图）的资源目录（相对发布包根）。
fn view_prefix(k: usize, view: u8) -> String {
    format!("shots/{k:02}/v{view}")
}

/// 含特征定位的配方可以发布：模板与胶检测走廊不重叠由配方校验保证（`RecipeDoc::validate`，定稿 §11 跨阶段约束），
/// 带外圈模板在示教原图内由这里与加载时核对；亮度、纹理、peakRatio 等质量门归 G4。运行库是否支持带外圈模板由调用方
/// 用 `vision::require_locate` 核对（发布前、生产加载时）。
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
        if inputs.insert((k, shot.view), shot).is_some() {
            return Err(format!("拍照点 {k} 的发布资源重复"));
        }
    }
    let expected = resource_views(&recipe);
    if inputs.keys().copied().collect::<BTreeSet<_>>() != expected.iter().copied().collect() {
        return Err("发布资源未覆盖全部拍照点".into());
    }
    let mut files = BTreeMap::new();
    add_file(&mut files, "recipe.json".into(), json_bytes(&input.recipe)?)?;
    let graph = source_bytes(input.graph)?;
    json_object(&graph, "测量图")?;
    add_file(&mut files, "graph.json".into(), graph)?;
    let mut shots = Vec::new();
    for (k, view) in expected {
        let projected = recipe.for_view(k, view)?;
        let shot = &projected.shots[k];
        let source = inputs.remove(&(k, view)).ok_or("发布资源缺少拍照点图像")?;
        if shot.measured() && (source.image.is_none() || source.calibration.is_none()) {
            return Err(format!("拍照点 {} 缺少发布原图或标定资源", shot.id));
        }
        let prefix = view_prefix(k, shot.view);
        let fixture = shot.measured().then_some(shot.fixture.as_ref()).flatten();
        let (image, size, template) = if let Some(image_source) = source.image {
            let image_id = source.image_id.as_deref().filter(|id| crate::recipe::valid_image_id(id))
                .ok_or_else(|| format!("拍照点 {} 图 {} 的发布原图缺少有效的示教图编号", shot.id, shot.view))?;
            let bytes = source_bytes(image_source)?;
            let (size, extension, decoded) = image_details(&bytes, &shot.id)?;
            check_coordinates(shot, size)?;
            let template = match fixture {
                Some(f) => {
                    if f.source != image_id {
                        return Err(format!("拍照点 {} 图 {} 的定位框来源是示教图 {}，发布原图却是 {image_id}：请确认定位框后重新保存示教参数", shot.id, shot.view, f.source));
                    }
                    f.check_image_size(size).map_err(|e| format!("拍照点 {} 图 {}：{e}", shot.id, shot.view))?;
                    let name = format!("{prefix}/{}", crate::vision::fixture::TEMPLATE_FILE);
                    add_file(&mut files, name.clone(), crate::vision::fixture::template_pgm(&decoded, f.rect)?)?;
                    Some(name)
                }
                None => None,
            };
            let name = format!("{prefix}/image.{extension}");
            add_file(&mut files, name.clone(), bytes)?;
            (Some((name, image_id.to_string())), Some(size), template)
        } else {
            if source.image_id.is_some() { return Err(format!("拍照点 {} 没有发布原图却给了示教图编号", shot.id)); }
            if fixture.is_some() { return Err(format!("拍照点 {} 使用了特征定位，缺少发布原图", shot.id)); }
            (None, None, None)
        };
        let (image, image_id) = image.unzip();
        let calibration = if let Some(source) = source.calibration {
            let bytes = source_bytes(source)?;
            json_object(&bytes, &format!("拍照点 {} 标定", shot.id))?;
            let name = format!("{prefix}/calibration.json");
            add_file(&mut files, name.clone(), bytes)?;
            Some(name)
        } else { None };
        let centerline = format!("{prefix}/centerline.json");
        let points = format!("{prefix}/points.json");
        add_file(&mut files, centerline.clone(), json_bytes(&centerline_data(&projected, k, size))?)?;
        add_file(&mut files, points.clone(), json_bytes(&points_data(&projected, k, size))?)?;
        shots.push(ShotManifest { k, shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view, skip: shot.skip, size, image, image_id, template, calibration, centerline, points });
    }
    let bundle_id = new_bundle_id()?;
    let manifest = ReleaseManifest {
        schema_version: RELEASE_SCHEMA,
        recipe_id: recipe.id.clone(), recipe_revision: recipe.revision_id.clone(), recipe_version: recipe.version, bundle_id: bundle_id.clone(),
        versions: input.versions, validation_site: input.validation_site, recipe: "recipe.json".into(), graph: "graph.json".into(), shots,
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

/// 只读清单并按文件元数据核对资源齐全、大小一致，不读取资源内容。
pub fn peek(releases_root: &Path, recipe_id: &str, bundle_id: &str) -> Result<ReleaseManifest, String> {
    safe_id(recipe_id)?;
    valid_bundle_id(bundle_id)?;
    let root = absolute_path(releases_root)?.join(recipe_id).join(bundle_id);
    reject_links(&root, false)?;
    let manifest = read_manifest(&root, bundle_id)?;
    if manifest.schema_version != RELEASE_SCHEMA {
        return Err(format!("发布包格式版本 {}，当前为 {RELEASE_SCHEMA}，需要重新发布", manifest.schema_version));
    }
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
    if recipe.id != manifest.recipe_id || built.revision_id != manifest.recipe_revision || built.version != manifest.recipe_version || manifest.shots.len() != resource_views(&built).len() {
        return Err("发布清单与配方快照的身份、版本或拍照点数量不一致".into());
    }
    json_object(file_bytes(&files, &manifest.graph)?, "发布测量图")?;
    let mut referenced = BTreeSet::from([manifest.recipe.clone(), manifest.graph.clone()]);
    for ((k, view), entry) in resource_views(&built).into_iter().zip(&manifest.shots) {
        let projected = built.for_view(k, view)?;
        let shot = &projected.shots[k];
        if entry.k != k || entry.shot_id != shot.id || entry.camera != shot.camera || entry.view != shot.view || entry.skip != shot.skip {
            return Err(format!("发布清单拍照点 {k} 的身份与配方不一致"));
        }
        let prefix = view_prefix(k, shot.view);
        if entry.centerline != format!("{prefix}/centerline.json") || entry.points != format!("{prefix}/points.json") {
            return Err(format!("拍照点 {} 的几何资源引用无效", shot.id));
        }
        if shot.measured() && (entry.image.is_none() || entry.calibration.is_none()) {
            return Err(format!("拍照点 {} 缺少发布原图或标定资源", shot.id));
        }
        if entry.image.is_some() != entry.image_id.is_some() || entry.image_id.as_deref().is_some_and(|id| !crate::recipe::valid_image_id(id)) {
            return Err(format!("拍照点 {} 的示教图编号与原图不对应或格式无效", shot.id));
        }
        // 定位模板只在启用 fixture 的视图上，且来源就是这张发布原图
        let fixture = shot.measured().then_some(shot.fixture.as_ref()).flatten();
        let template_name = format!("{prefix}/{}", crate::vision::fixture::TEMPLATE_FILE);
        match fixture {
            Some(f) if entry.template.as_deref() == Some(template_name.as_str()) && entry.image_id.as_deref() == Some(f.source.as_str()) => {}
            None if entry.template.is_none() => {}
            _ => return Err(format!("拍照点 {} 图 {} 的定位模板引用或示教图编号与配方不一致", shot.id, shot.view)),
        }
        for name in [Some(&entry.centerline), Some(&entry.points), entry.image.as_ref(), entry.calibration.as_ref(), entry.template.as_ref()].into_iter().flatten() {
            validate_relative(name)?;
            if !referenced.insert(name.clone()) {
                return Err(format!("拍照点资源被重复引用：{name}"));
            }
        }
        if let Some(image) = &entry.image {
            let (size, extension, decoded) = image_details(file_bytes(&files, image)?, &shot.id)?;
            if entry.size != Some(size) || image != &format!("{prefix}/image.{extension}") {
                return Err(format!("拍照点 {} 的原图尺寸或引用与清单不一致", shot.id));
            }
            check_coordinates(shot, size)?;
            // 模板按包内示教原图与配方重算比对（与中线、测点同一机制，不用内容哈希）
            if let (Some(f), Some(template)) = (fixture, &entry.template) {
                f.check_image_size(size).map_err(|e| format!("拍照点 {} 图 {}：{e}", shot.id, shot.view))?;
                if crate::vision::fixture::template_pgm(&decoded, f.rect)?.as_slice() != file_bytes(&files, template)? {
                    return Err(format!("拍照点 {} 图 {} 的定位模板与示教原图和配方不一致", shot.id, shot.view));
                }
            }
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
        if centerline != centerline_data(&projected, k, entry.size) || points != points_data(&projected, k, entry.size) {
            return Err(format!("拍照点 {} 的中线或测点与配方快照不一致", shot.id));
        }
    }
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

fn resource_views(recipe: &Recipe) -> Vec<(usize, u8)> {
    recipe.shots.iter().enumerate().flat_map(|(k, shot)| {
        let views = if shot.skip { vec![shot.view] } else { shot.enabled_views() };
        views.into_iter().map(move |view| (k, view))
    }).collect()
}

fn centerline_data(recipe: &Recipe, k: usize, size: Option<[u32; 2]>) -> Centerline {
    let shot = &recipe.shots[k];
    Centerline { schema_version: RELEASE_SCHEMA, shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view, size, spacing: recipe.spacing, mm_per_px: shot.mm_per_px, path: shot.path.clone() }
}

fn points_data(recipe: &Recipe, k: usize, size: Option<[u32; 2]>) -> MeasurePoints {
    let shot = &recipe.shots[k];
    let indices: Vec<_> = recipe.view_points(k, shot.view).collect();
    let points = indices.iter().map(|&j| [recipe.points.x[j], recipe.points.y[j]]).collect();
    MeasurePoints { schema_version: RELEASE_SCHEMA, shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view, size, spacing: recipe.spacing, indices, points }
}

fn check_coordinates(shot: &ShotSpec, size: [u32; 2]) -> Result<(), String> {
    if shot.path.iter().any(|p| p[0] < 0.0 || p[1] < 0.0 || p[0] >= size[0] as f32 || p[1] >= size[1] as f32) {
        return Err(format!("拍照点 {} 的中线超出原图 {}×{}", shot.id, size[0], size[1]));
    }
    Ok(())
}

/// 原图尺寸、扩展名与解码后的灰度像素（`image_file::decode`，与工作台裁模板同一条路径）。
fn image_details(bytes: &[u8], shot: &str) -> Result<([u32; 2], &'static str, crate::frame::FrameImage), String> {
    let format = image::guess_format(bytes).map_err(|e| format!("拍照点 {shot} 原图格式无效：{e}"))?;
    let extension = match format {
        image::ImageFormat::Pnm => "pgm",
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::Png => "png",
        image::ImageFormat::Bmp => "bmp",
        image::ImageFormat::Tiff => "tiff",
        _ => return Err(format!("拍照点 {shot} 原图格式不支持")),
    };
    let image = crate::image_file::decode(bytes).map_err(|e| format!("拍照点 {shot} 原图解码失败：{e}"))?;
    if image.width == 0 || image.height == 0 {
        return Err(format!("拍照点 {shot} 原图尺寸无效"));
    }
    Ok(([image.width, image.height], extension, image))
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

    fn one_shot_recipe() -> crate::recipe::RecipeDoc {
        let mut recipe = crate::recipe::samples().remove(1);
        recipe.shots.truncate(1);
        recipe
    }

    const IMAGE_ID: &str = "capture-1-0-v1";

    /// 1000×700 的灰度渐变原图：模板裁到哪一块，字节就不同
    fn image_bytes() -> Vec<u8> {
        let mut image = b"P5\n1000 700\n255\n".to_vec();
        image.extend((0..700u32).flat_map(|y| (0..1000u32).map(move |x| ((x * 7 + y * 3) % 251) as u8)));
        image
    }

    fn input(view: u8, image_id: &str) -> ShotInput {
        ShotInput { k: 0, view, image: Some(ResourceSource::Bytes(image_bytes())), image_id: Some(image_id.into()),
            calibration: Some(ResourceSource::Bytes(br#"{"mmPerPx":0.1}"#.to_vec())) }
    }

    fn publish_shots(root: &Path, recipe: crate::recipe::RecipeDoc, validation_site: ValidationSite, shots: Vec<ShotInput>) -> Result<ReleaseBundle, String> {
        publish(root, PublishInput {
            recipe, validation_site,
            versions: Versions { engine: "1.1.0".into(), graph: "graph-1".into() },
            graph: ResourceSource::Bytes(br#"{"graph":1}"#.to_vec()),
            shots,
        })
    }

    fn publish_with(root: &Path, validation_site: ValidationSite) -> ReleaseBundle {
        publish_shots(root, one_shot_recipe(), validation_site, vec![input(1, IMAGE_ID)]).unwrap()
    }

    /// 一个拍照点三幅图：图 1 用特征定位（框 rect），图 2 不用，图 3 停用但残留定位框
    fn fixture_recipe(rect: [u32; 4]) -> crate::recipe::RecipeDoc {
        let fixture = crate::recipe::FixtureSpec { rect, source: IMAGE_ID.into(), search_radius_px: 20.0, angle_range_deg: 0.0,
            min_score: 0.6, max_peak_ratio: 1.0, max_end_shift_mm: None, end_margin_mm: 0.0 };
        let mut recipe = one_shot_recipe();
        let shot = &mut recipe.shots[0];
        shot.views = [1, 2, 3].into_iter().map(|v| crate::recipe::ShotViewSpec { view: v, enabled: v != 3, path: shot.path.clone(),
            mm_per_px: shot.mm_per_px, detect: None, limits: None, calib: None, fixture: (v != 2).then(|| fixture.clone()) }).collect();
        recipe
    }

    fn publish_fixture(root: &Path, rect: [u32; 4], image_id: &str) -> Result<ReleaseBundle, String> {
        publish_shots(root, fixture_recipe(rect), ValidationSite::Real, vec![input(1, image_id), input(2, "capture-1-0-v2")])
    }

    #[test]
    fn fixture_view_carries_a_padded_template_in_its_own_view_directory() {
        // 防的回归（发布包文件契约）：生产 base_dir 指向 shots/kk/vN，glue.locate 在那里找 template.pgm、按 templatePad = 4
        // 裁回示教框；模板没带外圈、放错目录，或不用定位的视图（含停用视图上残留的框）也带了模板
        let root = TempDir(std::env::temp_dir().join(format!("gluesight-{}", new_bundle_id().unwrap())));
        let bundle = publish_fixture(&root.0, [100, 100, 32, 32], IMAGE_ID).unwrap();
        let loaded = load(&root.0, &bundle.manifest.recipe_id, &bundle.id).unwrap();
        let located = loaded.view(0, 1).unwrap();
        let template = located.template.unwrap();
        assert_eq!(template.parent(), Some(located.dir.as_path()));
        assert_eq!(template.file_name().and_then(|n| n.to_str()), Some(crate::vision::fixture::TEMPLATE_FILE));
        let bytes = fs::read(&template).unwrap();
        let head = b"P5\n40 40\n255\n";
        assert!(bytes.starts_with(head) && bytes.len() == head.len() + 40 * 40, "模板应为 (32 + 8) × (32 + 8)");
        assert!(loaded.view(0, 2).unwrap().template.is_none());
        assert_eq!(loaded.manifest.files.iter().filter(|f| f.path.ends_with("template.pgm")).count(), 1);
        assert_eq!(loaded.manifest.shots.iter().map(|s| s.image_id.as_deref()).collect::<Vec<_>>(), [Some(IMAGE_ID), Some("capture-1-0-v2")]);
    }

    #[test]
    fn template_tampered_with_same_size_or_dropped_with_its_reference_is_rejected_on_load() {
        // 防的回归：加载只核对文件大小（同尺寸改像素查不出），或只核对清单声明的集合（模板连同引用一起删掉后仍能加载、
        // 生产时 glue.locate 找不到模板）。模板按包内示教原图与配方重算比对，fixture 视图必须引用模板
        let root = TempDir(std::env::temp_dir().join(format!("gluesight-{}", new_bundle_id().unwrap())));
        let tamperings: [(&str, fn(&Path)); 2] = [
            ("同尺寸改像素", |dir| {
                let path = dir.join("shots/00/v1/template.pgm");
                let mut bytes = fs::read(&path).unwrap();
                let last = bytes.len() - 1;
                bytes[last] = !bytes[last];
                fs::write(path, bytes).unwrap();
            }),
            ("模板连同清单引用一起删掉", |dir| {
                fs::remove_file(dir.join("shots/00/v1/template.pgm")).unwrap();
                let path = dir.join("manifest.json");
                let mut manifest: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                manifest["files"].as_array_mut().unwrap().retain(|f| !f["path"].as_str().unwrap().ends_with("template.pgm"));
                for shot in manifest["shots"].as_array_mut().unwrap() { shot["template"] = serde_json::Value::Null; }
                fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            }),
        ];
        for (case, tamper) in tamperings {
            let bundle = publish_fixture(&root.0, [100, 100, 32, 32], IMAGE_ID).unwrap();
            tamper(&bundle.root);
            assert!(load(&root.0, &bundle.manifest.recipe_id, &bundle.id).is_err(), "{case}");
        }
    }

    #[test]
    fn fixture_whose_source_is_not_the_bundled_image_or_whose_padded_box_leaves_it_is_not_published() {
        // 防的回归：模板从别的图裁（来源与发布原图不是同一张，测量定义里的模板身份失真）；
        // 外圈出了原图右、下边（配方校验只查得了左、上边），生产时 glue.locate 每帧都报 out_of_image
        let root = TempDir(std::env::temp_dir().join(format!("gluesight-{}", new_bundle_id().unwrap())));
        for (case, rect, image_id, error) in [
            ("发布原图不是模板来源", [100, 100, 32, 32], "capture-9-0-v1", "来源"),
            ("外圈出右边", [966, 100, 32, 32], IMAGE_ID, "外圈"),
            ("外圈出下边", [100, 666, 32, 32], IMAGE_ID, "外圈"),
        ] {
            let e = publish_fixture(&root.0, rect, image_id).unwrap_err();
            assert!(e.contains(error), "{case}：{e}");
        }
    }

    #[test]
    fn validation_site_is_frozen_at_publish_and_bundles_without_it_are_rejected() {
        let root = TempDir(std::env::temp_dir().join(format!("gluesight-{}", new_bundle_id().unwrap())));
        for site in [ValidationSite::Real, ValidationSite::Offline] {
            let bundle = publish_with(&root.0, site);
            assert_eq!(load(&root.0, &bundle.manifest.recipe_id, &bundle.id).unwrap().manifest.validation_site, site);
        }
        let bundle = publish_with(&root.0, ValidationSite::Offline);
        let path = bundle.root.join("manifest.json");
        let mut manifest: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest.as_object_mut().unwrap().remove("validationSite");
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(load(&root.0, &bundle.manifest.recipe_id, &bundle.id).is_err());
    }

    #[test]
    fn bundle_changed_on_disk_after_publish_is_rejected_by_per_part_verify() {
        let root = TempDir(std::env::temp_dir().join(format!("gluesight-{}", new_bundle_id().unwrap())));
        let tamperings: [(&str, fn(&Path, &Path)); 5] = [
            ("extra file", |root, _| fs::write(root.join("shots").join("extra.json"), b"{}").unwrap()),
            ("resource resized", |_, resource| fs::write(resource, b"{}").unwrap()),
            ("resource removed", |_, resource| fs::remove_file(resource).unwrap()),
            ("resource replaced by directory", |_, resource| { fs::remove_file(resource).unwrap(); fs::create_dir(resource).unwrap(); }),
            ("manifest removed", |root, _| fs::remove_file(root.join("manifest.json")).unwrap()),
        ];
        for (case, tamper) in tamperings {
            let bundle = publish_with(&root.0, ValidationSite::Offline);
            bundle.verify().unwrap();
            let resource = bundle.manifest.files.iter().find(|entry| entry.path.starts_with("shots/")).unwrap();
            tamper(&bundle.root, &bundle.root.join(&resource.path));
            assert!(bundle.verify().is_err(), "{case}");
        }
    }
}
