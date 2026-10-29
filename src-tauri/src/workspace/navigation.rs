use super::*;
use sha2::{Digest, Sha256};
use std::io::Write;

const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_MAP_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NavigationImage {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub mime: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NavigationBackground {
    #[default]
    Slate,
    Light,
    Checker,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct NavigationPresentation {
    #[serde(deserialize_with = "deserialize_rotation")]
    pub rotation: f64,
    pub flip_x: bool,
    pub flip_y: bool,
    pub background: NavigationBackground,
}

fn normalized_rotation(rotation: f64) -> Result<f64, String> {
    if !rotation.is_finite() || !(-180.0..=180.0).contains(&rotation) {
        return Err("地图旋转角度需在 -180° 到 180° 之间".into());
    }
    Ok(if rotation == 180.0 { -180.0 } else { rotation })
}

fn deserialize_rotation<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<f64, D::Error> {
    normalized_rotation(f64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NavigationMap {
    pub format: u32,
    pub image: NavigationImage,
    pub points: Vec<[f64; 2]>,
    pub closed: bool,
    pub capture_start: f64,
    pub capture_end: f64,
    pub cuts: Vec<f64>,
    pub confirmed: bool,
    pub structure_revision: u64,
    #[serde(default)]
    pub view: NavigationPresentation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NavigationArchiveStatus {
    pub state: String,
    pub bundle_id: Option<String>,
    pub detail: String,
}

pub(super) fn deserialize_archive_status<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<NavigationArchiveStatus>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value::<NavigationArchiveStatus>(value)
        .ok()
        .filter(|status| {
            matches!(
                status.state.as_str(),
                "none" | "ready" | "unconfirmed" | "error"
            ) && status
                .bundle_id
                .as_deref()
                .is_some_and(|id| component(id).is_ok())
        }))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigationView {
    pub map: Option<NavigationMap>,
    pub error: Option<String>,
    pub image_available: bool,
    pub structure_revision: u64,
    pub published: Option<NavigationArchiveStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NavigationSegment {
    index: u32,
    kind: crate::plan::SegmentKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NavigationSnapshot {
    format: u32,
    recipe_id: String,
    recipe_revision: String,
    bundle_id: String,
    workspace_revision: u64,
    teaching_piece: crate::recipe::TeachingPieceRef,
    group_count: u64,
    cuts: Vec<u64>,
    segments: Vec<NavigationSegment>,
    map: NavigationMap,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NavigationManifest {
    format: u32,
    recipe_id: String,
    recipe_revision: String,
    bundle_id: String,
    snapshot_sha256: String,
    image_sha256: String,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hash_id(id: &str) -> Result<(), String> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err("导航资源编号无效".into());
    }
    Ok(())
}

fn component(id: &str) -> Result<(), String> {
    let upper = id.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'));
    if id.is_empty()
        || id.len() > 160
        || reserved
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("导航归档身份无效".into());
    }
    Ok(())
}

fn read_bytes(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    crate::image_file::checked_source_path(path)?;
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > limit as u64 {
        return Err("导航资源不是普通文件或超过大小限制".into());
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

fn make_dir(path: &Path) -> Result<(), String> {
    let parent = path
        .ancestors()
        .find(|p| p.exists())
        .ok_or("导航资料父目录不存在")?;
    crate::image_file::checked_source_path(parent)?;
    std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    crate::image_file::checked_source_path(path)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    make_dir(path.parent().ok_or("导航资源路径无效")?)?;
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
    {
        Ok(mut f) => match f.write_all(bytes).and_then(|_| f.sync_all()) {
            Ok(()) => Ok(()),
            Err(error) => {
                drop(f);
                let _ = std::fs::remove_file(path);
                Err(error.to_string())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if read_bytes(path, bytes.len())? == bytes {
                Ok(())
            } else {
                Err("不可变导航资源内容不一致".into())
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

fn candidate_dir(project: &Path, id: &str) -> Result<PathBuf, String> {
    safe_id(id)?;
    Ok(project.join("workspaces").join(id).join("navigation"))
}

fn image_path(dir: &Path, id: &str) -> Result<PathBuf, String> {
    hash_id(id)?;
    Ok(dir.join("images").join(format!("{id}.png")))
}

fn decode_image(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err("工件图不得为空或超过 32 MiB".into());
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(160 * 1024 * 1024);
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| format!("工件图解码失败：{e}"))?;
    if u64::from(image.width()) * u64::from(image.height()) > 32_000_000 {
        return Err("工件图不得超过 3200 万像素".into());
    }
    Ok(image)
}

fn import_image(dir: &Path, bytes: &[u8]) -> Result<NavigationMap, String> {
    let image = decode_image(bytes)?.into_rgba8();
    let (width, height) = image.dimensions();
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let bytes = encoded.into_inner();
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("转换后的 PNG 超过 32 MiB，请缩小工件图".into());
    }
    let id = digest(&bytes);
    write_new(&image_path(dir, &id)?, &bytes)?;
    Ok(NavigationMap {
        format: 1,
        image: NavigationImage {
            id,
            width,
            height,
            mime: "image/png".into(),
        },
        points: Vec::new(),
        closed: false,
        capture_start: 0.0,
        capture_end: 1.0,
        cuts: Vec::new(),
        confirmed: false,
        structure_revision: 0,
        view: NavigationPresentation::default(),
    })
}

fn validate_map(map: &NavigationMap, segments: Option<usize>) -> Result<(), String> {
    hash_id(&map.image.id)?;
    normalized_rotation(map.view.rotation)?;
    if map.format != 1
        || map.image.mime != "image/png"
        || map.image.width == 0
        || map.image.height == 0
    {
        return Err("导航地图格式或图片尺寸无效".into());
    }
    if map.points.len() > 4096
        || map
            .points
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err("胶路点需在图片内且不得超过 4096 个".into());
    }
    if !map.capture_start.is_finite()
        || !map.capture_end.is_finite()
        || map.capture_start < 0.0
        || map.capture_start >= 1.0
        || map.capture_end <= map.capture_start
        || map.capture_end > map.capture_start + 1.0
        || (!map.closed && map.capture_end > 1.0)
    {
        return Err("采集范围需沿胶路正向连续且不超过一圈".into());
    }
    if map.cuts.len() > 4095
        || map.cuts.iter().enumerate().any(|(i, &v)| {
            !v.is_finite()
                || v <= if i == 0 {
                    map.capture_start
                } else {
                    map.cuts[i - 1]
                }
                || v >= map.capture_end
        })
    {
        return Err("空间分界点必须在采集范围内严格递增".into());
    }
    if let Some(segments) = segments {
        if map.points.len() < if map.closed { 3 } else { 2 }
            || segments == 0
            || map.cuts.len() + 1 != segments
        {
            return Err("胶路或空间分界点尚未完成，与段列表不对应".into());
        }
        let mut pairs = map
            .points
            .windows(2)
            .map(|p| (p[0], p[1]))
            .collect::<Vec<_>>();
        if map.closed {
            pairs.push((*map.points.last().unwrap(), map.points[0]));
        }
        if pairs.iter().any(|(a, b)| a == b) {
            return Err("胶路不能包含零长度折线".into());
        }
    }
    Ok(())
}

fn verify_image(path: &Path, expected: &NavigationImage) -> Result<Vec<u8>, String> {
    let bytes = read_bytes(path, MAX_IMAGE_BYTES)?;
    if digest(&bytes) != expected.id {
        return Err("工件图内容校验失败".into());
    }
    let image = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    if image.format() != Some(image::ImageFormat::Png)
        || image.into_dimensions().map_err(|e| e.to_string())? != (expected.width, expected.height)
    {
        return Err("工件图格式或尺寸与地图不一致".into());
    }
    Ok(bytes)
}

fn save_map(dir: &Path, map: &NavigationMap) -> Result<String, String> {
    validate_map(map, None)?;
    let bytes = serde_json::to_vec(map).map_err(|e| e.to_string())?;
    let id = digest(&bytes);
    write_new(&dir.join("maps").join(format!("{id}.json")), &bytes)?;
    Ok(id)
}

fn read_map(project: &Path, w: &Workspace) -> Result<Option<NavigationMap>, String> {
    let Some(id) = &w.navigation_id else {
        return Ok(None);
    };
    hash_id(id)?;
    let dir = candidate_dir(project, &w.doc.id)?;
    let bytes = read_bytes(&dir.join("maps").join(format!("{id}.json")), MAX_MAP_BYTES)?;
    if digest(&bytes) != *id {
        return Err("地图内容校验失败".into());
    }
    let mut map: NavigationMap =
        serde_json::from_slice(&bytes).map_err(|e| format!("地图格式无效：{e}"))?;
    validate_map(&map, None)?;
    map.confirmed = map.confirmed
        && map.structure_revision == w.navigation_structure_revision
        && validate_map(&map, Some(w.doc.segments.len())).is_ok();
    Ok(Some(map))
}

pub(super) fn structure_changed(before: &RecipeDoc, after: &RecipeDoc) -> bool {
    before
        .segments
        .iter()
        .map(|s| (s.index, s.kind))
        .ne(after.segments.iter().map(|s| (s.index, s.kind)))
}

fn auxiliary_changed(w: &mut Workspace) {
    let previous_revision = w.revision;
    w.revision += 1;
    w.updated_at = ly_plc::now_ms();
    if let Some(validation) = w
        .validation
        .as_mut()
        .filter(|v| v.revision == previous_revision)
    {
        validation.revision = w.revision;
    }
}

pub(super) enum NavigationEdit {
    Import(Vec<u8>),
    Save { map: NavigationMap, confirm: bool },
    Delete,
}

pub(super) fn edit_navigation(
    project: &Path,
    w: &Workspace,
    revision: u64,
    edit: NavigationEdit,
) -> Result<Workspace, String> {
    w.expect(revision)?;
    let dir = candidate_dir(project, &w.doc.id)?;
    let map = match edit {
        NavigationEdit::Import(bytes) => {
            let mut map = import_image(&dir, &bytes)?;
            map.structure_revision = w.navigation_structure_revision;
            Some(map)
        }
        NavigationEdit::Save { mut map, confirm } => {
            map.view.rotation = normalized_rotation(map.view.rotation)?;
            let previous = read_map(project, w)?.ok_or("请先导入工件图")?;
            if map.image != previous.image
                || map.structure_revision != w.navigation_structure_revision
            {
                return Err("地图图片或段结构已改变，请重新加载后确认".into());
            }
            validate_map(
                &map,
                if confirm {
                    Some(w.doc.segments.len())
                } else {
                    None
                },
            )?;
            verify_image(&image_path(&dir, &map.image.id)?, &map.image)?;
            map.confirmed = confirm;
            Some(map)
        }
        NavigationEdit::Delete => None,
    };
    let mut next = w.clone();
    next.navigation_id = map.as_ref().map(|map| save_map(&dir, map)).transpose()?;
    auxiliary_changed(&mut next);
    Ok(next)
}

fn edit_command(
    app: AppHandle,
    id: String,
    revision: u64,
    edit: NavigationEdit,
) -> Result<WorkspaceView, String> {
    safe_id(&id)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let current = items.get_mut(&id).ok_or("候选配置不存在")?;
    let next = edit_navigation(&crate::storage::project(&app), current, revision, edit)?;
    host.save(&next)?;
    *current = next;
    view(&app, current.clone())
}

#[tauri::command]
pub fn workspace_navigation_import(
    app: AppHandle,
    id: String,
    revision: u64,
    bytes: Vec<u8>,
) -> Result<WorkspaceView, String> {
    edit_command(app, id, revision, NavigationEdit::Import(bytes))
}

#[tauri::command]
pub fn workspace_navigation_save(
    app: AppHandle,
    id: String,
    revision: u64,
    map: NavigationMap,
    confirm: bool,
) -> Result<WorkspaceView, String> {
    edit_command(app, id, revision, NavigationEdit::Save { map, confirm })
}

#[tauri::command]
pub fn workspace_navigation_delete(
    app: AppHandle,
    id: String,
    revision: u64,
) -> Result<WorkspaceView, String> {
    edit_command(app, id, revision, NavigationEdit::Delete)
}

#[tauri::command]
pub fn workspace_navigation_image(
    host: State<'_, WorkspaceHost>,
    id: String,
    image_id: String,
) -> Result<tauri::ipc::Response, String> {
    safe_id(&id)?;
    let items = host.items.lock().unwrap();
    let w = items.get(&id).ok_or("候选配置不存在")?;
    let project = host.root.parent().ok_or("项目目录不存在")?;
    let map = read_map(project, w)?.ok_or("候选没有工件图")?;
    if map.image.id != image_id {
        return Err("工件图已更新，请重新加载".into());
    }
    Ok(tauri::ipc::Response::new(verify_image(
        &image_path(&candidate_dir(project, &id)?, &image_id)?,
        &map.image,
    )?))
}

fn status(state: &str, bundle: Option<&str>, detail: impl Into<String>) -> NavigationArchiveStatus {
    NavigationArchiveStatus {
        state: state.into(),
        bundle_id: bundle.map(str::to_string),
        detail: detail.into(),
    }
}

fn release_dir(project: &Path, id: &str, bundle: &str, pending: bool) -> Result<PathBuf, String> {
    safe_id(id)?;
    component(bundle)?;
    Ok(project.join("recipe-support").join(id).join(if pending {
        format!(".pending-{bundle}")
    } else {
        bundle.to_string()
    }))
}

fn verified_snapshot(
    dir: &Path,
    id: &str,
    bundle: &str,
    revision: &str,
) -> Result<NavigationSnapshot, String> {
    let manifest: NavigationManifest =
        serde_json::from_slice(&read_bytes(&dir.join("manifest.json"), MAX_MAP_BYTES)?)
            .map_err(|e| format!("导航清单格式无效：{e}"))?;
    let bytes = read_bytes(&dir.join("snapshot.json"), MAX_MAP_BYTES)?;
    if manifest.format != 1
        || manifest.recipe_id != id
        || manifest.recipe_revision != revision
        || manifest.bundle_id != bundle
        || digest(&bytes) != manifest.snapshot_sha256
    {
        return Err("导航清单身份或快照摘要不一致".into());
    }
    let snapshot: NavigationSnapshot =
        serde_json::from_slice(&bytes).map_err(|e| format!("导航快照格式无效：{e}"))?;
    if snapshot.format != 1
        || snapshot.recipe_id != id
        || snapshot.bundle_id != bundle
        || snapshot.recipe_revision != revision
        || !snapshot.map.confirmed
    {
        return Err("导航快照与发布身份不一致或未确认".into());
    }
    validate_map(&snapshot.map, Some(snapshot.segments.len()))?;
    if snapshot.group_count == 0
        || snapshot.cuts.len() + 1 != snapshot.segments.len()
        || snapshot.cuts.iter().enumerate().any(|(i, &c)| {
            c == 0 || c >= snapshot.group_count || i > 0 && c <= snapshot.cuts[i - 1]
        })
        || snapshot
            .segments
            .iter()
            .enumerate()
            .any(|(i, s)| s.index != i as u32 + 1)
    {
        return Err("导航快照的示教件范围无效".into());
    }
    if manifest.image_sha256 != snapshot.map.image.id {
        return Err("导航清单的图片摘要不一致".into());
    }
    verify_image(&dir.join("image.png"), &snapshot.map.image)?;
    let mut names = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    names.sort();
    if names != ["image.png", "manifest.json", "snapshot.json"] {
        return Err("导航归档含有未声明的资源".into());
    }
    Ok(snapshot)
}

fn verify_snapshot_doc(snapshot: &NavigationSnapshot, doc: &RecipeDoc) -> Result<(), String> {
    if snapshot
        .segments
        .iter()
        .map(|s| (s.index, s.kind))
        .ne(doc.segments.iter().map(|s| (s.index, s.kind)))
        || doc.teaching_piece.as_ref().is_none_or(|piece| {
            serde_json::to_value(piece).ok() != serde_json::to_value(&snapshot.teaching_piece).ok()
        })
    {
        return Err("导航快照的段结构或示教件与发布配方不一致".into());
    }
    Ok(())
}

fn source_range(project: &Path, w: &Workspace) -> Result<(u64, Vec<u64>), String> {
    let piece = w
        .teaching_piece
        .as_ref()
        .ok_or("没有示教件，辅助资料未归档")?;
    if let Some(review) = w
        .capture_review
        .rounds
        .get(&piece.round_id)
        .filter(|r| r.status == capture_review::ReviewStatus::Confirmed)
    {
        return Ok((review.group_count, review.cuts.clone()));
    }
    if w.doc.segments.len() != 1 {
        return Err("示教件分段边界未确认，辅助资料未归档".into());
    }
    component(&piece.round_id)?;
    let bytes = read_bytes(
        &project
            .join("recipe-capture")
            .join(&piece.round_id)
            .join("round.json"),
        4 * MAX_MAP_BYTES,
    )?;
    let round: crate::recipe_capture::CaptureRoundV2 =
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if round.recipe_id != w.doc.id || round.round_id != piece.round_id || round.group_count == 0 {
        return Err("示教件身份或帧组数无效".into());
    }
    Ok((round.group_count, Vec::new()))
}

fn freeze(
    project: &Path,
    w: &Workspace,
    doc: &RecipeDoc,
    bundle: &str,
    map: NavigationMap,
) -> Result<(), String> {
    let (group_count, cuts) = source_range(project, w)?;
    let snapshot = NavigationSnapshot {
        format: 1,
        recipe_id: doc.id.clone(),
        recipe_revision: doc.build()?.revision_id,
        bundle_id: bundle.into(),
        workspace_revision: w.revision,
        teaching_piece: w.teaching_piece.as_ref().ok_or("没有示教件")?.recipe_ref(),
        group_count,
        cuts,
        segments: doc
            .segments
            .iter()
            .map(|s| NavigationSegment {
                index: s.index,
                kind: s.kind,
            })
            .collect(),
        map,
    };
    let pending = release_dir(project, &doc.id, bundle, true)?;
    let parent = pending.parent().ok_or("辅助资料目录无效")?;
    make_dir(parent)?;
    let scratch = parent.join(format!(".tmp-{}", vision::unique_run_id("navigation")));
    make_dir(&scratch)?;
    let result = (|| {
        let image = verify_image(
            &image_path(&candidate_dir(project, &doc.id)?, &snapshot.map.image.id)?,
            &snapshot.map.image,
        )?;
        write_new(&scratch.join("image.png"), &image)?;
        let snapshot_bytes = serde_json::to_vec(&snapshot).map_err(|e| e.to_string())?;
        let manifest = NavigationManifest {
            format: 1,
            recipe_id: doc.id.clone(),
            recipe_revision: snapshot.recipe_revision.clone(),
            bundle_id: bundle.into(),
            snapshot_sha256: digest(&snapshot_bytes),
            image_sha256: snapshot.map.image.id.clone(),
        };
        write_new(&scratch.join("snapshot.json"), &snapshot_bytes)?;
        write_new(
            &scratch.join("manifest.json"),
            &serde_json::to_vec(&manifest).map_err(|e| e.to_string())?,
        )?;
        verified_snapshot(&scratch, &doc.id, bundle, &snapshot.recipe_revision)?;
        if pending.exists() {
            let existing = verified_snapshot(&pending, &doc.id, bundle, &snapshot.recipe_revision)?;
            if serde_json::to_value(&existing).ok() != serde_json::to_value(&snapshot).ok() {
                return Err("已冻结的辅助资料内容不同".into());
            }
        } else {
            std::fs::rename(&scratch, &pending).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if scratch.exists() {
        let _ = std::fs::remove_dir_all(&scratch);
    }
    result
}

pub(super) fn prepare_navigation_release(
    project: &Path,
    w: &Workspace,
    doc: &RecipeDoc,
    bundle: &str,
) -> NavigationArchiveStatus {
    match read_map(project, w) {
        Ok(None) => status("none", Some(bundle), "未配置导航地图"),
        Ok(Some(map)) if !map.confirmed => status(
            "unconfirmed",
            Some(bundle),
            "地图对应尚未确认，辅助资料未归档",
        ),
        Ok(Some(map)) => match freeze(project, w, doc, bundle, map) {
            Ok(()) => status("ready", Some(bundle), "辅助资料已冻结，等待配方生效"),
            Err(e) => status("error", Some(bundle), format!("辅助资料未归档：{e}")),
        },
        Err(e) => status("error", Some(bundle), format!("辅助资料未归档：{e}")),
    }
}

pub(super) fn activate_navigation_release(project: &Path, release: &Release) -> Result<(), String> {
    let Some(record) = &release.navigation else {
        return Ok(());
    };
    if record.state == "error" || record.state == "unconfirmed" {
        return Err(record.detail.clone());
    }
    if record.state != "ready" {
        return Ok(());
    }
    let revision = release.doc.build()?.revision_id;
    let dir = release_dir(project, &release.doc.id, &release.bundle_id, false)?;
    if dir.exists() {
        let snapshot = verified_snapshot(&dir, &release.doc.id, &release.bundle_id, &revision)
            .map_err(|e| format!("辅助资料未归档：{e}"))?;
        verify_snapshot_doc(&snapshot, &release.doc)?;
        return Ok(());
    }
    let pending = release_dir(project, &release.doc.id, &release.bundle_id, true)?;
    let snapshot = verified_snapshot(&pending, &release.doc.id, &release.bundle_id, &revision)
        .map_err(|e| format!("辅助资料未归档：{e}"))?;
    verify_snapshot_doc(&snapshot, &release.doc)?;
    std::fs::rename(pending, dir).map_err(|e| format!("辅助资料未归档：{e}"))
}

fn archived_navigation(project: &Path, w: &Workspace) -> Option<NavigationArchiveStatus> {
    let release = match &w.pending {
        Some(release) => return release.navigation.clone(),
        None => {
            let revision = w.base_revision.as_deref()?;
            component(revision).ok()?;
            read_release(
                &project
                    .join("workspaces")
                    .join(&w.doc.id)
                    .join("published")
                    .join(format!("{revision}.json")),
            )
            .ok()?
        }
    };
    let record = release.navigation?;
    if record.state != "ready" {
        return Some(record);
    }
    let result = release_dir(project, &w.doc.id, &release.bundle_id, false).and_then(|dir| {
        let snapshot = verified_snapshot(
            &dir,
            &w.doc.id,
            &release.bundle_id,
            w.base_revision.as_deref().unwrap_or(""),
        )?;
        verify_snapshot_doc(&snapshot, &release.doc)
    });
    Some(match result {
        Ok(_) => status(
            "frozen",
            Some(&release.bundle_id),
            "当前生产版本的辅助资料已冻结",
        ),
        Err(e) => status(
            "error",
            Some(&release.bundle_id),
            format!("辅助资料不可用：{e}"),
        ),
    })
}

pub(super) fn navigation_view(project: &Path, w: &Workspace) -> NavigationView {
    let mut result = NavigationView {
        map: None,
        error: None,
        image_available: false,
        structure_revision: w.navigation_structure_revision,
        published: archived_navigation(project, w),
    };
    match read_map(project, w) {
        Ok(Some(map)) => {
            let image = candidate_dir(project, &w.doc.id)
                .and_then(|dir| image_path(&dir, &map.image.id))
                .and_then(|path| verify_image(&path, &map.image));
            result.image_available = image.is_ok();
            result.error = image.err();
            result.map = Some(map);
        }
        Ok(None) => {}
        Err(e) => result.error = Some(format!("导航地图不可用：{e}")),
    }
    result
}

pub(super) fn inherit_navigation(project: &Path, w: &mut Workspace) -> Option<String> {
    let result = (|| -> Result<(), String> {
        let Some(revision) = &w.base_revision else {
            return Ok(());
        };
        component(revision)?;
        let release = read_release(
            &project
                .join("workspaces")
                .join(&w.doc.id)
                .join("published")
                .join(format!("{revision}.json")),
        )?;
        if release
            .navigation
            .as_ref()
            .is_none_or(|record| record.state != "ready")
        {
            return Ok(());
        }
        let dir = release_dir(project, &w.doc.id, &release.bundle_id, false)?;
        let mut snapshot = verified_snapshot(&dir, &w.doc.id, &release.bundle_id, revision)?;
        verify_snapshot_doc(&snapshot, &release.doc)?;
        snapshot.map.structure_revision = w.navigation_structure_revision;
        let target = candidate_dir(project, &w.doc.id)?;
        write_new(
            &image_path(&target, &snapshot.map.image.id)?,
            &verify_image(&dir.join("image.png"), &snapshot.map.image)?,
        )?;
        w.navigation_id = Some(save_map(&target, &snapshot.map)?);
        Ok(())
    })();
    result.err().map(|e| format!("导航资料未继承：{e}"))
}

pub(crate) fn navigation_import_notes(project: &Path) -> Vec<String> {
    let mut notes = Vec::new();
    if let Ok(recipes) = std::fs::read_dir(project.join("recipe-support")) {
        for recipe in recipes.flatten().filter(|e| e.path().is_dir()) {
            let id = recipe.file_name().to_string_lossy().into_owned();
            if let Ok(bundles) = std::fs::read_dir(recipe.path()) {
                for entry in bundles
                    .flatten()
                    .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                {
                    let bundle = entry.file_name().to_string_lossy().into_owned();
                    let check = (|| -> Result<(), String> {
                        safe_id(&id)?;
                        component(&bundle)?;
                        let bytes = read_bytes(&entry.path().join("snapshot.json"), MAX_MAP_BYTES)?;
                        let snapshot: NavigationSnapshot =
                            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                        component(&snapshot.recipe_revision)?;
                        let release = read_release(
                            &project
                                .join("workspaces")
                                .join(&id)
                                .join("published")
                                .join(format!("{}.json", snapshot.recipe_revision)),
                        )?;
                        if release.bundle_id != bundle || release.doc.id != id {
                            return Err("辅助资料没有对应的发布归档".into());
                        }
                        let snapshot = verified_snapshot(
                            &entry.path(),
                            &id,
                            &bundle,
                            &release.doc.build()?.revision_id,
                        )?;
                        verify_snapshot_doc(&snapshot, &release.doc)?;
                        Ok(())
                    })();
                    if let Err(e) = check {
                        notes.push(format!(
                            "导航辅助资料 {id}/{bundle} 不可用：{e}；生产配方不受影响"
                        ));
                    }
                }
            }
        }
    }
    if let Ok(workspaces) = std::fs::read_dir(project.join("workspaces")) {
        for entry in workspaces.flatten() {
            if let Ok(bytes) = std::fs::read(entry.path().join("workspace.json")) {
                if let Ok(w) = serde_json::from_slice::<Workspace>(&bytes) {
                    if let Some(e) = navigation_view(project, &w).error {
                        notes.push(format!("候选 {} 的{e}；生产配方不受影响", w.doc.id));
                    }
                }
            }
        }
    }
    notes
}

pub(super) fn remove_navigation_archives(project: &Path, id: &str) -> Result<(), String> {
    safe_id(id)?;
    let dir = project.join("recipe-support").join(id);
    if dir.exists() {
        crate::image_file::checked_source_path(&dir)?;
        std::fs::remove_dir_all(dir).map_err(|e| format!("清理导航辅助归档失败：{e}"))?;
    }
    Ok(())
}
