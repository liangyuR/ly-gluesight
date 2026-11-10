//! 配方回传包：把一个候选（分段、走廊、颜色模型、示教件测量、导航图与示教件轮次记录）打成 ZIP，
//! 导入到本机或另一台机器的当前项目，替换同编号候选。示教件轮次只引用历史件的录制原图，包里不带原图：导入后按示教件那一件
//! 的检测记录读本机录制；以前带原图导出的包照常导入。生产配方与发布包不动，候选回到草稿，在本机发布。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

use super::{
    check_identity, pieces, safe_id, state_value, Discarded, PublishState, Workspace, WorkspaceHost,
};
use crate::burst_judge::FrameState;
use crate::cycle::CycleHost;
use crate::operation::Operation;
use crate::project::transfer::{self, PackageFile};
use crate::recipe::Recipe;
use crate::store::Store;

/// 格式 2 加了 `teachingImages`；格式 1 的包按不带示教件原图读入。
pub const FORMAT: u32 = 2;
const FORMATS: [u32; 2] = [1, 2];
const KIND: &str = "recipe";
const LABEL: &str = "配方回传包";
const CANDIDATE: &str = "candidate";
const ROUNDS: &str = "rounds";
const ROUND_FILES: [&str; 2] = ["round.json", "frames.jsonl"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageManifest {
    pub kind: String,
    pub format: u32,
    pub recipe_id: String,
    pub name: String,
    pub product_code: u16,
    pub source_project: String,
    pub source_project_name: String,
    pub source_root: String,
    #[serde(default, rename = "baseRevision", skip_serializing)]
    pub _base_revision: Discarded,
    pub teaching_piece: Option<pieces::TeachingPiece>,
    pub exported_at: i64,
    pub app_version: String,
    /// 带示教件轮次的原图（以前的导出才有，现在导出恒为 false）
    #[serde(default)]
    pub teaching_images: bool,
    pub files: Vec<PackageFile>,
}

fn named(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && !part.contains(['\\', ':'])
        && !part.chars().any(char::is_control)
}

/// 轮次原图的文件名：`{seq:06}.pgm`。
fn frame_file(file: &str) -> bool {
    file.strip_suffix(".pgm")
        .is_some_and(|seq| seq.len() >= 6 && seq.chars().all(|c| c.is_ascii_digit()))
}

/// 包内路径是否合规；`images` 为带示教件原图的包，才允许 `rounds/<轮次>/<相机>/<seq>.pgm`。
fn allowed(path: &str, images: bool) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    if !parts.iter().all(|p| named(p)) {
        return false;
    }
    match parts.as_slice() {
        [CANDIDATE, "workspace.json"] => true,
        [CANDIDATE, "images", file] => file.ends_with(".pgm"),
        [CANDIDATE, top, _, ..] => matches!(*top, "calibration" | "navigation"),
        [ROUNDS, round, file] => round.starts_with("capture-") && ROUND_FILES.contains(file),
        [ROUNDS, round, camera, file] => {
            images
                && round.starts_with("capture-")
                && crate::recipe::valid_camera_id(camera)
                && frame_file(file)
        }
        _ => false,
    }
}

/// 轮次 `frames.jsonl` 里各帧原图在轮次目录内的相对路径。
fn frame_paths(round_dir: &Path) -> Result<Vec<String>, String> {
    let text =
        crate::fsio::read_text(&round_dir.join("frames.jsonl")).map_err(|e| e.to_string())?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|v| v["path"].as_str().map(str::to_owned))
                .ok_or_else(|| "采集帧记录无法读取".to_string())
        })
        .collect()
}

/// 候选引用的采集轮次：示教件轮次、各段示教帧与示教件测量所在的轮次。
fn referenced_rounds(w: &Workspace) -> BTreeSet<String> {
    w.teaching_piece
        .iter()
        .map(|t| t.round_id.clone())
        .chain(w.camera_teaching.iter().map(|t| t.round.clone()))
        .chain(w.calibration.rounds.iter().map(|r| r.round_id.clone()))
        .collect()
}

fn round_id_ok(id: &str) -> bool {
    id.starts_with("capture-") && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// 把项目 `project` 里的候选 `w` 导出为回传包 `target`：带走廊示教帧、示教件测量与导航资料、示教件轮次的
/// `round.json` 与 `frames.jsonl`，不带示教件原图、未被引用的旧示教帧与发布快照。
pub(crate) fn export_candidate(
    project: &Path,
    project_meta: &crate::project::ProjectMeta,
    w: &Workspace,
    target: &Path,
    operation: &Operation,
) -> Result<PackageManifest, String> {
    if w.legacy() {
        return Err(pieces::LEGACY.into());
    }
    if w.pending.is_some() {
        return Err("候选正在等待生效，生效或处理发布失败后再导出".into());
    }
    safe_id(&w.doc.id)?;
    let dir = project.join("workspaces").join(&w.doc.id);
    let file = dir.join("workspace.json");
    let saved: Workspace = serde_json::from_str(
        &crate::fsio::read_text(&file).map_err(|e| format!("读取候选文件失败：{e}"))?,
    )
    .map_err(|e| format!("候选文件无法读取：{e}"))?;
    if saved.revision != w.revision || saved.instance_id != w.instance_id {
        return Err("候选文件与界面上的候选不一致，请重新加载后导出".into());
    }
    let mut entries = vec![(format!("{CANDIDATE}/workspace.json"), file)];
    let frozen: BTreeSet<&str> = super::pass_align::frozen_teaching(w)
        .map(|t| t.frame.id.as_str())
        .collect();
    for id in frozen {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!("示教帧编号无效：{id}"));
        }
        let path = dir.join("images").join(format!("{id}.pgm"));
        if !path.is_file() {
            return Err(format!("示教帧 {id} 的图像文件缺失，不能导出"));
        }
        entries.push((format!("{CANDIDATE}/images/{id}.pgm"), path));
    }
    let rounds = referenced_rounds(w);
    let mut notes = Vec::new();
    for top in [super::burst::CALIBRATION_DIR, "navigation"] {
        let path = dir.join(top);
        if !path.is_dir() {
            continue;
        }
        let mut found = Vec::new();
        transfer::walk(&path, &format!("{CANDIDATE}/{top}"), &mut found, &mut notes)?;
        entries.extend(found.into_iter().filter(|(name, _)| {
            top != super::burst::CALIBRATION_DIR
                || name
                    .split('/')
                    .nth(2)
                    .is_some_and(|round| rounds.contains(round))
        }));
    }
    let capture = project.join("recipe-capture");
    for round in &rounds {
        if !round_id_ok(round) {
            return Err(format!("采集轮次编号无效：{round}"));
        }
        for name in ROUND_FILES {
            let path = capture.join(round).join(name);
            if !path.is_file() {
                return Err(format!("示教件轮次 {round} 缺少 {name}，不能导出"));
            }
            entries.push((format!("{ROUNDS}/{round}/{name}"), path));
        }
    }
    if let Some((name, _)) = entries.iter().find(|(name, _)| !allowed(name, false)) {
        return Err(format!("导出路径无效：{name}"));
    }
    if !notes.is_empty() {
        return Err(format!("候选目录里有链接，不能导出：{}", notes.join("；")));
    }
    let manifest = PackageManifest {
        kind: KIND.into(),
        format: FORMAT,
        recipe_id: w.doc.id.clone(),
        name: w.doc.name.clone(),
        product_code: w.doc.product_code,
        source_project: project_meta.id.clone(),
        source_project_name: project_meta.name.clone(),
        source_root: project.to_string_lossy().into_owned(),
        _base_revision: Discarded,
        teaching_piece: w.teaching_piece.clone(),
        exported_at: ly_plc::now_ms(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        teaching_images: false,
        files: Vec::new(),
    };
    operation.measuring(entries.len())?;
    transfer::write_zip_with(
        target,
        entries,
        |files| PackageManifest { files, ..manifest },
        || operation.advance(FrameState::Measured),
        || operation.saving(),
    )
}

fn read_manifest<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Result<PackageManifest, String> {
    let value = transfer::manifest_value(zip, LABEL)?;
    if value.get("kind").and_then(Value::as_str) != Some(KIND) {
        return Err("不是配方回传包（项目导出包请在项目页导入）".into());
    }
    let format = value.get("format").and_then(Value::as_u64);
    if !format.is_some_and(|f| FORMATS.iter().any(|&known| known as u64 == f)) {
        return Err(format!(
            "配方回传包格式 {} 不受本机软件支持（支持 1–{FORMAT}），请升级软件",
            format.map_or("未知".into(), |f| f.to_string())
        ));
    }
    let manifest: PackageManifest =
        serde_json::from_value(value).map_err(|e| format!("配方回传包清单格式无效：{e}"))?;
    safe_id(&manifest.recipe_id)?;
    let images = manifest.teaching_images;
    let seen = transfer::check_listed(&manifest.files, |path| allowed(path, images), LABEL)?;
    if !seen.contains("candidate/workspace.json") {
        return Err("配方回传包缺少候选文件".into());
    }
    Ok(manifest)
}

pub(crate) fn inspect(path: &Path) -> Result<PackageManifest, String> {
    read_manifest(&mut transfer::open_zip(path, LABEL)?)
}

const ROUND_JSON_MAX: u64 = 1 << 20;

/// 预览用：清单与包里各采集轮次的 `round.json`（这里不核对校验和，导入解包时核对）。
fn inspect_rounds(path: &Path) -> Result<(PackageManifest, Vec<Value>), String> {
    let mut zip = transfer::open_zip(path, LABEL)?;
    let manifest = read_manifest(&mut zip)?;
    let mut rounds = Vec::new();
    for file in &manifest.files {
        if !(file.path.starts_with("rounds/") && file.path.ends_with("/round.json")) {
            continue;
        }
        let mut text = String::new();
        zip.by_name(&file.path)
            .map_err(|e| format!("{LABEL}条目无法读取：{e}"))?
            .take(ROUND_JSON_MAX)
            .read_to_string(&mut text)
            .map_err(|e| format!("{LABEL}条目无法读取：{e}"))?;
        rounds.push(serde_json::from_str(&text).map_err(|e| format!("采集轮次无法读取：{e}"))?);
    }
    Ok((manifest, rounds))
}

/// 示教件轮次各相机采集时的设备配置与本机当前（`current` 按相机编号，`state_value` 口径）对不上时的提示，不拦导入：
/// 候选的分段、走廊与取色照样导入，示教帧、示教件测量与发布按现有规则核对相机。
pub(crate) fn camera_notes(rounds: &[Value], current: &BTreeMap<String, Value>) -> Vec<String> {
    let mut absent = BTreeSet::new();
    let mut changed = BTreeSet::new();
    for camera in rounds
        .iter()
        .flat_map(|r| r["cameras"].as_array().into_iter().flatten())
    {
        let id = camera["cameraId"].as_str().unwrap_or_default().to_string();
        match current.get(&id) {
            None => {
                absent.insert(id);
            }
            Some(config) if config != &camera["cameraConfig"] => {
                changed.insert(id);
            }
            Some(_) => {}
        }
    }
    let join = |ids: BTreeSet<String>| ids.into_iter().collect::<Vec<_>>().join("、");
    let mut reasons = Vec::new();
    if !absent.is_empty() {
        reasons.push(format!("相机 {} 不在本机设备配置里", join(absent)));
    }
    if !changed.is_empty() {
        reasons.push(format!(
            "相机 {} 的设备配置与示教件采集时不一致",
            join(changed)
        ));
    }
    if reasons.is_empty() {
        return reasons;
    }
    vec![format!(
        "{}：导入后换示教帧、测量示教件和发布前，需把这些相机恢复为示教件采集时的配置，或在本机重新选用示教件",
        reasons.join("；")
    )]
}

/// 解到暂存目录 `staging` 并核对：候选编号与清单一致、是示教件流程的候选、配方能构建，
/// 带来的轮次正是候选引用的轮次且都属于这个配方；带示教件原图的包，各轮次帧记录里的原图须一张不缺、不多。
pub(crate) fn stage(
    package: &Path,
    staging: &Path,
    operation: &Operation,
) -> Result<(PackageManifest, Workspace), String> {
    let mut zip = transfer::open_zip(package, LABEL)?;
    let manifest = read_manifest(&mut zip)?;
    let images = manifest.teaching_images;
    operation.measuring(manifest.files.len())?;
    transfer::extract_listed_with(
        &mut zip,
        &manifest.files,
        staging,
        |path| allowed(path, images),
        LABEL,
        |_| true,
        || operation.advance(FrameState::Measured),
    )?;
    let text = crate::fsio::read_text(&staging.join(CANDIDATE).join("workspace.json"))
        .map_err(|e| e.to_string())?;
    let mut w: Workspace =
        serde_json::from_str(&text).map_err(|e| format!("回传包里的候选无法读取：{e}"))?;
    if w.doc.id != manifest.recipe_id {
        return Err("回传包里的候选编号与清单不一致".into());
    }
    if w.legacy() {
        return Err(pieces::LEGACY.into());
    }
    w.doc.build()?;
    let rounds = referenced_rounds(&w);
    let carried: BTreeSet<String> = std::fs::read_dir(staging.join(ROUNDS))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    if carried != rounds {
        return Err("回传包带的采集轮次与候选引用的不一致".into());
    }
    for round in &rounds {
        if !round_id_ok(round) {
            return Err(format!("采集轮次编号无效：{round}"));
        }
        let dir = staging.join(ROUNDS).join(round);
        if ROUND_FILES.iter().any(|f| !dir.join(f).is_file()) {
            return Err(format!("回传包缺少采集轮次 {round} 的记录"));
        }
        let value: Value = serde_json::from_str(
            &crate::fsio::read_text(&dir.join("round.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("采集轮次 {round} 无法读取：{e}"))?;
        if value["roundId"].as_str() != Some(round) || value["recipeId"].as_str() != Some(&w.doc.id)
        {
            return Err(format!("采集轮次 {round} 不属于配方 {}", w.doc.id));
        }
        if images {
            let frames = frame_paths(&dir)?;
            let prefix = format!("{ROUNDS}/{round}/");
            let carried = manifest
                .files
                .iter()
                .filter(|f| {
                    f.path
                        .strip_prefix(&prefix)
                        .is_some_and(|rest| !ROUND_FILES.contains(&rest))
                })
                .count();
            if let Some(frame) = frames
                .iter()
                .find(|f| !allowed(&format!("{prefix}{f}"), true) || !dir.join(f).is_file())
            {
                return Err(format!("回传包缺少示教件轮次 {round} 的原图 {frame}"));
            }
            if carried != frames.len() {
                return Err(format!("回传包里示教件轮次 {round} 的原图与帧记录不一致"));
            }
        }
    }
    for t in super::pass_align::frozen_teaching(&w) {
        if !staging
            .join(CANDIDATE)
            .join("images")
            .join(format!("{}.pgm", t.frame.id))
            .is_file()
        {
            return Err(format!("回传包缺少示教帧 {}", t.frame.id));
        }
    }
    w.instance_id = 0;
    Ok((manifest, w))
}

/// 本机这一配方的现状：有没有生产配方与下一版本号。
pub(crate) struct Local {
    pub has_production: bool,
    pub next_version: u32,
}

/// 改到本机：版本号按本机配方库取，候选回到草稿；指向来源项目目录的绝对路径改到本机项目目录。
pub(crate) fn rebase(
    w: &mut Workspace,
    local: &Local,
    source_root: &Path,
    project: &Path,
) -> Result<(), String> {
    let mut value = serde_json::to_value(&*w).map_err(|e| e.to_string())?;
    crate::storage::relocate::all_strings(&mut value, source_root, project);
    *w = serde_json::from_value(value).map_err(|e| e.to_string())?;
    w.doc.version = if local.has_production {
        local.next_version
    } else {
        local.next_version.max(w.doc.version.max(1))
    };
    w.doc.teaching_piece = None;
    w.pending = None;
    w.publish = PublishState::Draft;
    w.changed();
    Ok(())
}

/// 已执行的改名，失败时倒序撤回。
#[derive(Default)]
struct Moves(Vec<(PathBuf, PathBuf)>);

impl Moves {
    fn rename(&mut self, from: &Path, to: &Path) -> Result<(), String> {
        std::fs::create_dir_all(to.parent().ok_or("目标路径无效")?).map_err(|e| e.to_string())?;
        std::fs::rename(from, to).map_err(|e| format!("移动 {} 失败：{e}", from.display()))?;
        self.0.push((from.to_path_buf(), to.to_path_buf()));
        Ok(())
    }

    fn undo(self) {
        for (from, to) in self.0.into_iter().rev() {
            if let Err(error) = std::fs::rename(&to, &from) {
                crate::applog::write(
                    "error",
                    "配方回传包",
                    &format!("撤回 {} 失败：{error}", to.display()),
                );
            }
        }
    }
}

fn owned_rounds(capture: &Path, recipe_id: &str) -> Vec<String> {
    let mut rounds: Vec<String> = std::fs::read_dir(capture)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|id| {
            crate::fsio::read_text(&capture.join(id).join("round.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .is_some_and(|v| v["recipeId"].as_str() == Some(recipe_id))
        })
        .collect();
    rounds.sort();
    rounds
}

/// 把暂存目录里改好的候选 `w` 落进项目 `project`：同编号的旧候选目录与它的采集轮次移到 `staging/previous`，
/// 旧候选的 `published/` 发布快照留给新候选；任一步失败都撤回已做的移动，项目保持原样。
pub(crate) fn install(project: &Path, staging: &Path, w: &Workspace) -> Result<(), String> {
    let id = &w.doc.id;
    let candidate = staging.join(CANDIDATE);
    crate::fsio::write_atomic(
        &candidate.join("workspace.json"),
        &serde_json::to_string_pretty(w).map_err(|e| e.to_string())?,
    )?;
    let dir = project.join("workspaces").join(id);
    let capture = project.join("recipe-capture");
    let previous = staging.join("previous");
    let old_rounds = owned_rounds(&capture, id);
    let incoming = referenced_rounds(w);
    if let Some(round) = incoming
        .iter()
        .find(|r| capture.join(r).exists() && !old_rounds.contains(r))
    {
        return Err(format!("采集轮次 {round} 已被本机别的配方占用，不能导入"));
    }
    let mut moves = Moves::default();
    let result = (|| {
        if dir.exists() {
            moves.rename(&dir, &previous.join(CANDIDATE))?;
        }
        moves.rename(&candidate, &dir)?;
        let published = previous.join(CANDIDATE).join("published");
        if published.is_dir() {
            moves.rename(&published, &dir.join("published"))?;
        }
        for round in &old_rounds {
            moves.rename(&capture.join(round), &previous.join(ROUNDS).join(round))?;
        }
        for round in &incoming {
            moves.rename(&staging.join(ROUNDS).join(round), &capture.join(round))?;
        }
        Ok(())
    })();
    if result.is_err() {
        moves.undo();
    }
    result
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageExported {
    pub path: String,
    pub bytes: u64,
    pub files: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalCandidate {
    pub revision: u64,
    pub updated_at: i64,
    pub pending: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackagePreview {
    pub path: String,
    pub bytes: u64,
    pub recipe_id: String,
    pub name: String,
    pub product_code: u16,
    pub source_project_name: String,
    pub exported_at: i64,
    pub app_version: String,
    pub teaching_piece: Option<pieces::TeachingPiece>,
    /// 包里带示教件原图（以前导出的包）
    pub teaching_images: bool,
    /// 本机生产配方的版本；没有为空
    pub production_version: Option<u32>,
    /// 本机同编号的候选，导入后被替换
    pub candidate: Option<LocalCandidate>,
    /// 相机与本机对不上等不拦导入的提示
    pub warnings: Vec<String>,
    /// 不能导入的原因
    pub blocker: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageImported {
    pub id: String,
    /// 导入后示教件原图读得到（本机有示教件那一件的录制，或随以前的包带入），能直接试测
    pub teaching_images: bool,
    pub notes: Vec<String>,
}

fn file_name(id: &str) -> String {
    format!(
        "配方_{id}_{}.zip",
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    )
}

/// 本机能否导入这一配方：同编号候选正在等待生效、编号或型号与别的配方冲突都不能导入。
fn blocker_in(
    items: &HashMap<String, Workspace>,
    production: &[Arc<Recipe>],
    recipe_id: &str,
    product_code: u16,
) -> Option<String> {
    if items.get(recipe_id).is_some_and(|w| w.pending.is_some()) {
        return Some("本机这一配方的候选正在等待生效，生效或处理发布失败后再导入".into());
    }
    check_identity(
        recipe_id,
        product_code,
        items
            .values()
            .map(|w| (w.doc.id.as_str(), w.doc.product_code))
            .chain(production.iter().map(|r| (r.id.as_str(), r.product_code)))
            .filter(|(id, _)| *id != recipe_id),
    )
    .err()
}

fn blocker(app: &AppHandle, manifest: &PackageManifest) -> Option<String> {
    let production = app.state::<CycleHost>().recipes.list();
    let host = app.state::<WorkspaceHost>();
    let items = host.items.lock().unwrap();
    blocker_in(
        &items,
        &production,
        &manifest.recipe_id,
        manifest.product_code,
    )
}

/// 本机当前各相机的设备配置（按相机编号，`state_value` 口径）。
fn current_cameras(app: &AppHandle) -> BTreeMap<String, Value> {
    app.state::<CycleHost>()
        .camera
        .configs()
        .iter()
        .map(|c| (c.id.clone(), state_value(c)))
        .collect()
}

/// 导出前选保存位置。
#[tauri::command]
pub async fn workspace_package_target(
    window: tauri::WebviewWindow,
    app: AppHandle,
    id: String,
) -> Result<Option<String>, String> {
    safe_id(&id)?;
    let exports = crate::storage::root(&app).join("exports");
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&exports).map_err(|e| e.to_string())?;
        Ok(rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("导出工件配方")
            .set_directory(&exports)
            .set_file_name(file_name(&id))
            .add_filter(LABEL, &["zip"])
            .save_file()
            .map(|p| p.display().to_string()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn workspace_package_export(
    app: AppHandle,
    id: String,
    path: String,
    task_id: Option<String>,
) -> Result<PackageExported, String> {
    safe_id(&id)?;
    let operation = Operation::start(&app, task_id)?;
    let w = {
        let host = app.state::<WorkspaceHost>();
        let items = host.items.lock().unwrap();
        items.get(&id).ok_or("候选配置不存在")?.clone()
    };
    let project = crate::storage::project(&app);
    tauri::async_runtime::spawn_blocking(move || {
        operation.run(|operation| {
            let target = PathBuf::from(&path);
            let result = crate::project::read_meta(&project)
                .and_then(|meta| export_candidate(&project, &meta, &w, &target, operation));
            crate::cycle::log_op(
                &app,
                &crate::applog::codes::OP_RECIPE_PACKAGE,
                json!({"action": "export", "recipeId": id, "path": path}),
                &result,
            );
            let manifest = result?;
            Ok(PackageExported {
                bytes: std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0),
                path,
                files: manifest.files.len(),
            })
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn workspace_package_pick(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<Option<PackagePreview>, String> {
    let exports = crate::storage::root(&app).join("exports");
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("选择工件配方包")
            .add_filter(LABEL, &["zip"]);
        if exports.is_dir() {
            dialog = dialog.set_directory(exports);
        }
        let Some(path) = dialog.pick_file() else {
            return Ok(None);
        };
        inspect_rounds(&path).map(|inspected| Some((path, inspected)))
    })
    .await
    .map_err(|e| e.to_string())??;
    let Some((path, (manifest, rounds))) = picked else {
        return Ok(None);
    };
    let candidate = {
        let host = app.state::<WorkspaceHost>();
        let items = host.items.lock().unwrap();
        items.get(&manifest.recipe_id).map(|w| LocalCandidate {
            revision: w.revision,
            updated_at: w.updated_at,
            pending: w.pending.is_some(),
        })
    };
    Ok(Some(PackagePreview {
        bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        path: path.display().to_string(),
        production_version: app
            .state::<CycleHost>()
            .recipe(&manifest.recipe_id)
            .map(|r| r.version),
        candidate,
        warnings: camera_notes(&rounds, &current_cameras(&app)),
        blocker: blocker(&app, &manifest),
        recipe_id: manifest.recipe_id,
        name: manifest.name,
        product_code: manifest.product_code,
        source_project_name: manifest.source_project_name,
        exported_at: manifest.exported_at,
        app_version: manifest.app_version,
        teaching_piece: manifest.teaching_piece,
        teaching_images: manifest.teaching_images,
    }))
}

#[tauri::command]
pub async fn workspace_package_import(
    app: AppHandle,
    path: String,
    task_id: Option<String>,
) -> Result<PackageImported, String> {
    let operation = Operation::start(&app, task_id)?;
    let cycle = app.state::<CycleHost>();
    let _config = cycle.config_gate.lock().await;
    let task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        operation.run(|operation| {
            let staging = transfer::staging_dir(
                &crate::storage::root(&task).join("cache"),
                "recipe-import",
            )?;
            let result = import(&task, Path::new(&path), &staging, operation);
            let _ = std::fs::remove_dir_all(&staging);
            crate::cycle::log_op(
                &task,
                &crate::applog::codes::OP_RECIPE_PACKAGE,
                json!({"action": "import", "path": path, "recipeId": result.as_ref().ok().map(|r| r.id.clone())}),
                &result,
            );
            result
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

fn import(
    app: &AppHandle,
    package: &Path,
    staging: &Path,
    operation: &Operation,
) -> Result<PackageImported, String> {
    let project = crate::storage::project(app);
    let (manifest, mut w) = stage(package, staging, operation)?;
    let id = w.doc.id.clone();
    let rounds: Vec<Value> = referenced_rounds(&w)
        .iter()
        .filter_map(|round| {
            crate::fsio::read_text(&staging.join(ROUNDS).join(round).join("round.json"))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
        })
        .collect();
    let mut notes = camera_notes(&rounds, &current_cameras(app));
    operation.saving()?;
    let installed = {
        let cycle = app.state::<CycleHost>();
        let production = cycle.recipes.list();
        let host = app.state::<WorkspaceHost>();
        let mut items = host.items.lock().unwrap();
        if let Some(reason) = blocker_in(&items, &production, &id, w.doc.product_code) {
            return Err(reason);
        }
        let local = Local {
            has_production: cycle.recipe(&id).is_some(),
            next_version: cycle.recipes.next_version(&id)?,
        };
        rebase(&mut w, &local, Path::new(&manifest.source_root), &project)?;
        w.ensure_instance(&app.state::<Store>())?;
        {
            // 替换轮次时旧轮次先移出、新轮次后移入，中间不让清理按缺了引用的快照删掉示教件的来源历史件
            let _references = crate::recipe_capture::lock_references();
            install(&project, staging, &w)?;
        }
        items.insert(id.clone(), w.clone());
        w
    };
    let mut teaching_images = false;
    if let Some(piece) = &installed.teaching_piece {
        let cycle = app.state::<CycleHost>();
        match cycle.camera.recipe_capture().images_unavailable(&piece.round_id) {
            Ok(None) => teaching_images = true,
            Ok(Some(reason)) | Err(reason) => notes.push(format!(
                "示教件（SN {}）的原图读不到：{reason}。分段、走廊、取色与示教件测量结果都已导入，候选仍可发布；看示教图、示教走廊、测量示教件或试测需要本机有这件历史件的检测记录与录制，否则重新选用示教件",
                piece.sn
            )),
        }
    }
    Ok(PackageImported {
        id,
        teaching_images,
        notes,
    })
}

#[cfg(test)]
#[path = "package_tests.rs"]
mod tests;
