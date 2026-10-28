//! 项目整包导出导入：把一个项目目录（硬件配置、标定、配方与发布包、候选与采集轮次，可选检测记录与原图）打成 ZIP，
//! 在本机或另一台机器上导入为新项目。
//!
//! - 包内 `manifest.json` 记格式版本、来源项目、来源项目目录、导出时间、来源软件版本、是否含检测记录与每个文件的字节数和 SHA-256；
//! - 导入先完整解到暂存目录，只接受清单内、路径在白名单结构里的文件并逐个核对，任何不符整包拒绝，不留下半个项目；
//! - 指向来源项目目录的绝对路径改写到新项目目录；生产配方保持可生产，计划版本按本机计数重新分配；候选回到草稿。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use tauri::AppHandle;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::{manage, ProjectMeta};

pub const FORMAT: u32 = 1;
const MANIFEST: &str = "manifest.json";
const MANIFEST_MAX_BYTES: u64 = 64 << 20;
const PREFIX: &str = "project/";
const CONTENT: [&str; 7] = [
    "project.json",
    "config",
    "calib",
    "recipes",
    "workspaces",
    "vision",
    "recipe-capture",
];
const RECORDS: [&str; 3] = ["inspection.db", "records", "records-offline"];
const PENDING_RECORDINGS: &str = ".project-install-pending.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub project: ProjectMeta,
    pub source_root: String,
    pub exported_at: i64,
    pub app_version: String,
    pub includes_records: bool,
    pub notes: Vec<String>,
    pub files: Vec<PackageFile>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectExported {
    pub path: String,
    pub bytes: u64,
    pub files: usize,
    pub includes_records: bool,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectImportPreview {
    pub path: String,
    pub source_id: String,
    pub name: String,
    pub description: String,
    pub exported_at: i64,
    pub app_version: String,
    pub includes_records: bool,
    pub files: usize,
    pub bytes: u64,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectImported {
    pub meta: ProjectMeta,
    pub notes: Vec<String>,
}

fn copy_hashed(mut from: impl Read, mut to: impl Write) -> std::io::Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = from.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        to.write_all(&buffer[..n])?;
        total += n as u64;
    }
    Ok((total, format!("{:x}", hasher.finalize())))
}

fn allowed(path: &str, records: bool) -> bool {
    let Some(rest) = path.strip_prefix(PREFIX) else {
        return false;
    };
    let parts: Vec<&str> = rest.split('/').collect();
    let named = parts.iter().all(|p| {
        !p.is_empty()
            && *p != "."
            && *p != ".."
            && !p.contains(['\\', ':'])
            && !p.chars().any(char::is_control)
    });
    let top = parts[0];
    let single = matches!(top, "project.json" | "inspection.db");
    named
        && single == (parts.len() == 1)
        && (CONTENT.contains(&top) || records && RECORDS.contains(&top))
}

fn walk(
    dir: &Path,
    relative: &str,
    out: &mut Vec<(String, PathBuf)>,
    notes: &mut Vec<String>,
) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("读取 {} 失败：{e}", dir.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let kind = std::fs::symlink_metadata(&path)
            .map_err(|e| e.to_string())?
            .file_type();
        let child = format!("{relative}/{name}");
        if kind.is_symlink() {
            notes.push(format!("{child} 是链接，未导出"));
        } else if kind.is_dir() {
            crate::image_file::checked_source_path(&path)?;
            walk(&path, &child, out, notes)?;
        } else if kind.is_file() && !name.ends_with(".tmp") {
            crate::image_file::checked_source_path(&path)?;
            out.push((child, path));
        }
    }
    Ok(())
}

/// 待写进包的文件：（包内路径，磁盘路径）。检测记录库用 `VACUUM INTO` 做含 WAL 的一致性快照，快照放在 `scratch`。
fn collect(
    root: &Path,
    include_records: bool,
    scratch: &Path,
    notes: &mut Vec<String>,
) -> Result<Vec<(String, PathBuf)>, String> {
    let mut out = Vec::new();
    let recording_root = crate::log_storage::recording_project(root)?;
    for top in CONTENT
        .iter()
        .chain(if include_records { &RECORDS[1..] } else { &[] })
    {
        let path = if RECORDS[1..].contains(top) {
            recording_root.join(top)
        } else {
            root.join(top)
        };
        let relative = format!("{PREFIX}{top}");
        if path.is_dir() {
            crate::image_file::checked_source_path(&path)?;
            walk(&path, &relative, &mut out, notes)?;
        } else if path.is_file() {
            out.push((relative, path));
        }
    }
    let db = root.join("inspection.db");
    if include_records && db.is_file() {
        std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
        let snapshot = scratch.join("inspection.db");
        let conn =
            rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("打开检测记录库失败：{e}"))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))
            .map_err(|e| e.to_string())?;
        conn.execute("VACUUM INTO ?1", [snapshot.to_string_lossy()])
            .map_err(|e| format!("检测记录库快照失败：{e}"))?;
        drop(conn);
        if recording_root != root {
            crate::storage::relocate::database(&snapshot, &recording_root, root)?;
        }
        out.push((format!("{PREFIX}inspection.db"), snapshot));
    }
    Ok(out)
}

fn write_package<W: Write + Seek>(
    out: W,
    mut manifest: Manifest,
    entries: Vec<(String, PathBuf)>,
) -> Result<(W, Manifest), String> {
    let mut zip = ZipWriter::new(out);
    for (name, path) in entries {
        if !allowed(&name, manifest.includes_records) {
            return Err(format!("导出路径无效：{name}"));
        }
        let file = File::open(&path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
        let length = file.metadata().map_err(|e| e.to_string())?.len();
        let level = if name.ends_with(".pgm") || name.ends_with(".png") {
            1
        } else {
            6
        };
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(level))
            .large_file(length >= u32::MAX as u64);
        zip.start_file(name.as_str(), options)
            .map_err(|e| e.to_string())?;
        let (bytes, sha256) = copy_hashed(BufReader::new(file), &mut zip)
            .map_err(|e| format!("写入 {name} 失败：{e}"))?;
        manifest.files.push(PackageFile {
            path: name,
            bytes,
            sha256,
        });
    }
    zip.start_file(
        MANIFEST,
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
    )
    .map_err(|e| e.to_string())?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let out = zip.finish().map_err(|e| e.to_string())?;
    Ok((out, manifest))
}

/// 把项目目录 `root` 导出到 `target`（先写 `.tmp` 再改名）。
pub fn export_to(
    root: &Path,
    include_records: bool,
    scratch: &Path,
    target: &Path,
) -> Result<Manifest, String> {
    let project = super::read_meta(root)?;
    let mut notes = Vec::new();
    let entries = collect(root, include_records, scratch, &mut notes)?;
    let manifest = Manifest {
        format: FORMAT,
        project,
        source_root: root.to_string_lossy().into_owned(),
        exported_at: ly_plc::now_ms(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        includes_records: include_records,
        notes,
        files: Vec::new(),
    };
    let mut temporary = target.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let written = File::create(&temporary)
        .map_err(|e| format!("创建导出文件失败：{e}"))
        .and_then(|file| {
            let (out, manifest) = write_package(BufWriter::new(file), manifest, entries)?;
            out.into_inner()
                .map_err(|e| e.to_string())?
                .sync_all()
                .map_err(|e| e.to_string())?;
            Ok(manifest)
        });
    let manifest = written.and_then(|manifest| {
        if target.exists() {
            std::fs::remove_file(target).map_err(|e| format!("覆盖导出文件失败：{e}"))?;
        }
        std::fs::rename(&temporary, target).map_err(|e| e.to_string())?;
        Ok(manifest)
    });
    if manifest.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    manifest
}

fn read_manifest<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<Manifest, String> {
    let mut entry = zip
        .by_name(MANIFEST)
        .map_err(|_| "不是项目导出包：缺少 manifest.json".to_string())?;
    if entry.size() > MANIFEST_MAX_BYTES {
        return Err("项目导出包清单过大".into());
    }
    let mut text = String::new();
    entry
        .by_ref()
        .take(MANIFEST_MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("项目导出包清单无法读取：{e}"))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("项目导出包清单格式无效：{e}"))?;
    let format = value.get("format").and_then(Value::as_u64);
    if format != Some(FORMAT as u64) {
        return Err(format!(
            "项目导出包格式 {} 不受本机软件支持（支持 {FORMAT}）",
            format.map_or("未知".into(), |f| f.to_string())
        ));
    }
    let manifest: Manifest =
        serde_json::from_value(value).map_err(|e| format!("项目导出包清单格式无效：{e}"))?;
    if !super::valid_id(&manifest.project.id) {
        return Err("项目导出包的来源项目编号无效".into());
    }
    let mut seen = HashSet::new();
    for file in &manifest.files {
        if !allowed(&file.path, manifest.includes_records) {
            return Err(format!("项目导出包含不允许的路径：{}", file.path));
        }
        if !seen.insert(file.path.to_ascii_lowercase()) {
            return Err(format!("项目导出包清单重复列出 {}", file.path));
        }
        if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("项目导出包清单的校验和无效：{}", file.path));
        }
    }
    if !seen.contains("project/project.json") {
        return Err("项目导出包缺少 project.json".into());
    }
    Ok(manifest)
}

fn open_archive(path: &Path) -> Result<ZipArchive<BufReader<File>>, String> {
    let file = File::open(path).map_err(|e| format!("打开项目导出包失败：{e}"))?;
    ZipArchive::new(BufReader::new(file)).map_err(|e| format!("项目导出包不是有效的 ZIP 文件：{e}"))
}

pub fn inspect(path: &Path) -> Result<Manifest, String> {
    read_manifest(&mut open_archive(path)?)
}

/// 完整解到空的暂存目录：只写清单列出、路径合规的普通文件，逐个核对字节数与 SHA-256。
fn extract(path: &Path, staging: &Path) -> Result<Manifest, String> {
    let mut zip = open_archive(path)?;
    let manifest = read_manifest(&mut zip)?;
    let listed: BTreeMap<&str, &PackageFile> = manifest
        .files
        .iter()
        .map(|f| (f.path.as_str(), f))
        .collect();
    let mut seen = HashSet::new();
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|e| format!("项目导出包条目无法读取：{e}"))?;
        let name = entry
            .name()
            .map_err(|e| format!("项目导出包条目名称无效：{e}"))?
            .into_owned();
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(format!("项目导出包含重复条目：{name}"));
        }
        if name == MANIFEST {
            continue;
        }
        let expected = listed
            .get(name.as_str())
            .filter(|_| allowed(&name, manifest.includes_records))
            .ok_or_else(|| format!("项目导出包含清单之外或不允许的条目：{name}"))?;
        if !entry.is_file() || entry.encrypted() || entry.size() != expected.bytes {
            return Err(format!(
                "项目导出包条目 {name} 不是普通文件或大小与清单不符"
            ));
        }
        let target = name
            .split('/')
            .fold(staging.to_path_buf(), |path, part| path.join(part));
        std::fs::create_dir_all(target.parent().ok_or("暂存路径无效")?)
            .map_err(|e| e.to_string())?;
        let mut out = BufWriter::new(
            File::create_new(&target).map_err(|e| format!("写入暂存文件 {name} 失败：{e}"))?,
        );
        let (bytes, sha256) = copy_hashed(entry.take(expected.bytes + 1), &mut out)
            .map_err(|e| format!("解出 {name} 失败：{e}"))?;
        out.flush().map_err(|e| e.to_string())?;
        if bytes != expected.bytes || sha256 != expected.sha256.to_ascii_lowercase() {
            return Err(format!(
                "项目导出包文件 {name} 校验和不符，文件已损坏或被改动"
            ));
        }
    }
    if seen.len() != manifest.files.len() + 1 {
        return Err("项目导出包缺少清单列出的文件".into());
    }
    Ok(manifest)
}

fn plan_of(value: &Value) -> Option<u32> {
    value
        .get("planVersion")
        .and_then(Value::as_u64)
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p != 0)
}

fn set_plan(value: &mut Value, plans: &BTreeMap<u32, u32>) -> bool {
    match plan_of(value).and_then(|p| plans.get(&p)) {
        Some(new) => {
            value["planVersion"] = (*new).into();
            true
        }
        None => false,
    }
}

fn read_value(path: &Path) -> Result<Value, String> {
    serde_json::from_str(&crate::fsio::read_text(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{} 无法解析：{e}", path.display()))
}

/// 生产配方、修订来源与检测记录里的配方快照：计划版本按本机计数整体换成新号（同一旧号换成同一新号），
/// 修订来源与配方文件同步改，加载时仍能证明修订号属于它，不会被当成手改文件而升版本。
fn remap_plans(root: &Path, plan_file: &Path) -> Result<BTreeMap<u32, u32>, String> {
    let recipes = root.join("recipes");
    let docs: Vec<PathBuf> = std::fs::read_dir(&recipes)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json"))
        .filter(|p| {
            !p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with('.')
        })
        .collect();
    let sources_path = recipes.join(".revision-sources");
    let mut sources = if sources_path.is_file() {
        Some(read_value(&sources_path)?)
    } else {
        None
    };
    let mut used = BTreeSet::new();
    for doc in &docs {
        used.extend(plan_of(&read_value(doc)?));
    }
    if let Some(map) = sources.as_ref().and_then(Value::as_object) {
        used.extend(map.values().filter_map(plan_of));
    }
    if used.is_empty() {
        return Ok(BTreeMap::new());
    }
    let first = crate::recipe::reserve_plan_versions(plan_file, used.len() as u32)?;
    let plans: BTreeMap<u32, u32> = used.into_iter().zip(first..).collect();
    for doc in &docs {
        let mut value = read_value(doc)?;
        if set_plan(&mut value, &plans) {
            crate::fsio::write_atomic(
                doc,
                &serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?,
            )?;
        }
    }
    if let Some(Value::Object(map)) = sources.as_mut() {
        map.values_mut().for_each(|value| {
            set_plan(value, &plans);
        });
        crate::fsio::write_atomic(
            &sources_path,
            &serde_json::to_string(&map).map_err(|e| e.to_string())?,
        )?;
    }
    let db = root.join("inspection.db");
    if db.is_file() {
        let mut conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
        let has: bool = conn
            .query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='recipe_snapshots')", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if has {
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            let rows: Vec<(String, String)> = tx
                .prepare("SELECT revision_id, json FROM recipe_snapshots")
                .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
                .map_err(|e| e.to_string())?;
            for (revision, json) in rows {
                let mut value: Value = serde_json::from_str(&json)
                    .map_err(|e| format!("配方快照 {revision} 无法解析：{e}"))?;
                if set_plan(&mut value, &plans) {
                    tx.execute(
                        "UPDATE recipe_snapshots SET json=?1 WHERE revision_id=?2",
                        rusqlite::params![value.to_string(), revision],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            tx.commit().map_err(|e| e.to_string())?;
        }
    }
    Ok(plans)
}

fn staging_dir(parent: &Path, kind: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stamp = ly_plc::now_ms();
    for suffix in 0..1000 {
        let dir = parent.join(format!("project-{kind}-{stamp}-{suffix}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("无法创建暂存目录".into())
}

/// 把导出包导入为资料目录 `data` 下的新项目 `id`；`plan_file` 是本机计划版本记录，`cache` 放暂存目录（用后删除）。
pub fn import_from(
    data: &Path,
    package: &Path,
    id: &str,
    name: &str,
    plan_file: &Path,
    cache: &Path,
) -> Result<ProjectImported, String> {
    let staging = staging_dir(cache, "import")?;
    let result = install(data, package, id, name, plan_file, &staging);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn install(
    data: &Path,
    package: &Path,
    id: &str,
    name: &str,
    plan_file: &Path,
    staging: &Path,
) -> Result<ProjectImported, String> {
    let manifest = extract(package, staging)?;
    let mut imported = finish(
        data,
        &staging.join("project"),
        &Source {
            id: &manifest.project.id,
            root: Path::new(&manifest.source_root),
            includes_records: manifest.includes_records,
        },
        &New {
            id,
            name,
            description: &manifest.project.description,
        },
        plan_file,
    )?;
    imported
        .notes
        .push("相机组、PLC 配置与标定来自来源项目，切换到该项目前请核对".into());
    Ok(imported)
}

struct Source<'a> {
    id: &'a str,
    root: &'a Path,
    includes_records: bool,
}

pub struct New<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub description: &'a str,
}

/// 暂存目录 `root` 里的项目内容落成新项目：改写来源目录路径、重新分配计划版本、候选回到草稿，再原子移入 `projects/<id>`。
fn finish(
    data: &Path,
    root: &Path,
    source: &Source,
    new: &New,
    plan_file: &Path,
) -> Result<ProjectImported, String> {
    let _guard = manage::LOCK.lock().unwrap_or_else(|e| e.into_inner());
    manage::check_new_id(data, new.id)?;
    let name = manage::check_name(data, new.name, None)?;
    let target = super::dir(data, new.id);
    let meta = ProjectMeta::new(
        new.id,
        &name,
        new.description.trim(),
        Some(source.id.to_string()),
    );
    super::write_meta(root, &meta)?;
    crate::storage::relocate::tree_references(root, source.root, &target)?;
    let mut notes = crate::storage::relocate::validate_references(root, source.root, &target)?;
    let plans = remap_plans(root, plan_file)?;
    for entry in std::fs::read_dir(root.join("workspaces"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let candidate = entry.path().join("workspace.json");
        if candidate.is_file() {
            notes.extend(crate::workspace::reset_for_project_import(
                &candidate,
                &plans,
                source.includes_records,
            )?);
        }
    }
    if !plans.is_empty() {
        notes.push(format!(
            "{} 个计划版本已按本机重新分配，生产配方的拍照计划不变",
            plans.len()
        ));
    }
    std::fs::create_dir_all(super::projects_root(data)).map_err(|e| e.to_string())?;
    let external = install_recordings(root, &target)?;
    if let Err(error) = std::fs::rename(root, &target) {
        if let Some(external) = external {
            let _ = remove_owned_tree(&external);
        }
        return Err(format!("移入项目目录失败：{error}"));
    }
    if let Some(external) = external {
        if let Err(error) = std::fs::remove_file(external.join(PENDING_RECORDINGS)) {
            crate::applog::write(
                "warn",
                "项目原图提交标记待清理",
                &format!("{}：{error}", external.display()),
            );
        }
    }
    Ok(ProjectImported { meta, notes })
}

fn remove_owned_tree(path: &Path) -> Result<(), String> {
    crate::image_file::checked_source_path(path)?;
    std::fs::remove_dir_all(path).map_err(|e| format!("清理暂存原图 {} 失败：{e}", path.display()))
}

fn install_recordings(root: &Path, project: &Path) -> Result<Option<PathBuf>, String> {
    let external = crate::log_storage::recording_project(project)?;
    if external == project {
        return Ok(None);
    }
    if external.exists() {
        return Err(format!(
            "项目原图目录已存在，拒绝覆盖：{}",
            external.display()
        ));
    }
    let db = root.join("inspection.db");
    let has_recordings = RECORDS[1..].iter().any(|top| root.join(top).exists());
    if !has_recordings {
        if db.is_file() {
            crate::storage::relocate::database(&db, project, &external)?;
        }
        return Ok(None);
    }
    let id = project
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|id| super::valid_id(id))
        .ok_or("项目编号无效")?;
    let parent = external.parent().ok_or("项目原图目录无效")?;
    let staging = staging_dir(parent, &format!("recordings-{id}"))?;
    crate::image_file::checked_source_path(&staging)?;
    let result = (|| {
        crate::fsio::write_atomic(
            &staging.join(PENDING_RECORDINGS),
            &serde_json::json!({"projectId": id}).to_string(),
        )?;
        for top in &RECORDS[1..] {
            let source = root.join(top);
            if !source.exists() {
                continue;
            }
            crate::image_file::checked_source_path(&source)?;
            let mut entries = Vec::new();
            let mut notes = Vec::new();
            walk(&source, top, &mut entries, &mut notes)?;
            if !notes.is_empty() {
                return Err("项目原图包含链接，拒绝导入".into());
            }
            std::fs::create_dir_all(staging.join(top)).map_err(|e| e.to_string())?;
            for (relative, source) in entries {
                crate::image_file::checked_source_path(&source)?;
                let target = relative
                    .split('/')
                    .fold(staging.clone(), |p, part| p.join(part));
                std::fs::create_dir_all(target.parent().ok_or("原图暂存路径无效")?)
                    .map_err(|e| e.to_string())?;
                let mut output =
                    BufWriter::new(File::create_new(&target).map_err(|e| e.to_string())?);
                let expected = copy_hashed(
                    BufReader::new(File::open(&source).map_err(|e| e.to_string())?),
                    &mut output,
                )
                .map_err(|e| e.to_string())?;
                output.flush().map_err(|e| e.to_string())?;
                drop(output);
                let actual = copy_hashed(
                    BufReader::new(File::open(&target).map_err(|e| e.to_string())?),
                    &mut std::io::sink(),
                )
                .map_err(|e| e.to_string())?;
                if actual != expected {
                    return Err(format!("原图复制校验失败：{}", source.display()));
                }
            }
        }
        if db.is_file() {
            crate::storage::relocate::database(&db, project, &external)?;
        }
        if external.exists() {
            return Err(format!(
                "项目原图目录已存在，拒绝覆盖：{}",
                external.display()
            ));
        }
        std::fs::rename(&staging, &external).map_err(|e| format!("移入项目原图目录失败：{e}"))?;
        Ok(())
    })();
    if let Err(error) = result {
        if staging.exists() {
            let _ = remove_owned_tree(&staging);
        }
        return Err(error);
    }
    for top in &RECORDS[1..] {
        let local = root.join(top);
        if local.exists() {
            if let Err(error) = remove_owned_tree(&local) {
                let _ = remove_owned_tree(&external);
                return Err(error);
            }
        }
    }
    Ok(Some(external))
}

pub(crate) fn recover_pending_recordings(data: &Path) -> Result<(), String> {
    let Some(config) = crate::log_storage::read(data)? else {
        return Ok(());
    };
    let namespace = config
        .vision("_")
        .parent()
        .ok_or("视觉日志命名空间无效")?
        .to_path_buf();
    if !namespace.exists() {
        return Ok(());
    }
    crate::image_file::checked_source_path(&namespace)?;
    for entry in std::fs::read_dir(&namespace).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let marker = path.join(PENDING_RECORDINGS);
        if !path.is_dir() || !marker.is_file() {
            continue;
        }
        crate::image_file::checked_source_path(&marker)?;
        let value: Value =
            serde_json::from_str(&crate::fsio::read_text(&marker).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let id = value["projectId"]
            .as_str()
            .filter(|id| super::valid_id(id))
            .ok_or("项目原图提交标记的项目编号无效")?;
        let expected = config.vision(id);
        if path == expected {
            if super::dir(data, id).is_dir() {
                std::fs::remove_file(&marker).map_err(|e| e.to_string())?;
            } else {
                remove_owned_tree(&path)?;
            }
        } else {
            let prefix = format!("project-recordings-{id}-");
            let suffix = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix(&prefix));
            let owned_staging =
                suffix
                    .and_then(|s| s.split_once('-'))
                    .is_some_and(|(stamp, index)| {
                        !stamp.is_empty()
                            && !index.is_empty()
                            && stamp.bytes().all(|b| b.is_ascii_digit())
                            && index.bytes().all(|b| b.is_ascii_digit())
                    });
            if !owned_staging {
                return Err("项目原图提交标记与目录归属不符，保留待核对资料".into());
            }
            remove_owned_tree(&path)?;
        }
    }
    Ok(())
}

/// 把本机项目 `source_id` 复制为新项目：按导出的取舍收集文件（记录库做一致性快照），拷进暂存目录后按导入规则落成新项目。
pub fn copy_from(
    data: &Path,
    source_id: &str,
    include_records: bool,
    new: &New,
    plan_file: &Path,
    cache: &Path,
) -> Result<ProjectImported, String> {
    let staging = staging_dir(cache, "copy")?;
    let result = copy_into(data, source_id, include_records, new, plan_file, &staging);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn copy_into(
    data: &Path,
    source_id: &str,
    include_records: bool,
    new: &New,
    plan_file: &Path,
    staging: &Path,
) -> Result<ProjectImported, String> {
    let source_root = super::dir(data, source_id);
    let mut notes = Vec::new();
    let entries = collect(
        &source_root,
        include_records,
        &staging.join("snapshot"),
        &mut notes,
    )?;
    for (name, path) in entries {
        let target = name
            .split('/')
            .fold(staging.to_path_buf(), |path, part| path.join(part));
        std::fs::create_dir_all(target.parent().ok_or("暂存路径无效")?)
            .map_err(|e| e.to_string())?;
        std::fs::copy(&path, &target).map_err(|e| format!("复制 {} 失败：{e}", path.display()))?;
    }
    let mut copied = finish(
        data,
        &staging.join("project"),
        &Source {
            id: source_id,
            root: &source_root,
            includes_records: include_records,
        },
        new,
        plan_file,
    )?;
    notes.append(&mut copied.notes);
    copied.notes = notes;
    Ok(copied)
}

fn file_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    format!(
        "项目_{}_{}.zip",
        safe.trim(),
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    )
}

#[tauri::command]
pub async fn project_export(
    window: tauri::WebviewWindow,
    app: AppHandle,
    id: String,
    include_records: bool,
) -> Result<Option<ProjectExported>, String> {
    let data = crate::storage::root(&app);
    let meta = super::open_target(&data, &id)?;
    if id == super::current_id(&app) {
        crate::site::check_project_switch(&super::switch_state(&app)?)
            .map_err(|e| e.replace("切换项目", "导出当前项目"))?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let exports = data.join("exports");
        std::fs::create_dir_all(&exports).map_err(|e| e.to_string())?;
        let Some(path) = rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("导出项目")
            .set_directory(&exports)
            .set_file_name(file_name(&meta.name))
            .add_filter("项目导出包", &["zip"])
            .save_file()
        else {
            return Ok(None);
        };
        let scratch = staging_dir(&data.join("cache"), "export")?;
        let exported = export_to(&super::dir(&data, &id), include_records, &scratch, &path);
        let _ = std::fs::remove_dir_all(&scratch);
        let manifest = exported?;
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        crate::applog::write(
            "info",
            "项目导出",
            &format!(
                "已导出项目 {id} 到 {}（{} 个文件，{:.1} MB）",
                path.display(),
                manifest.files.len(),
                bytes as f64 / 1048576.0
            ),
        );
        Ok(Some(ProjectExported {
            path: path.display().to_string(),
            bytes,
            files: manifest.files.len(),
            includes_records: manifest.includes_records,
            notes: manifest.notes,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn project_import_pick(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<Option<ProjectImportPreview>, String> {
    let exports = crate::storage::root(&app).join("exports");
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("选择项目导出包")
            .add_filter("项目导出包", &["zip"]);
        if exports.is_dir() {
            dialog = dialog.set_directory(exports);
        }
        let Some(path) = dialog.pick_file() else {
            return Ok(None);
        };
        let manifest = inspect(&path)?;
        Ok(Some(ProjectImportPreview {
            bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
            path: path.display().to_string(),
            source_id: manifest.project.id,
            name: manifest.project.name,
            description: manifest.project.description,
            exported_at: manifest.exported_at,
            app_version: manifest.app_version,
            includes_records: manifest.includes_records,
            files: manifest.files.len(),
            notes: manifest.notes,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn project_import(
    app: AppHandle,
    path: String,
    id: String,
    name: String,
) -> Result<ProjectImported, String> {
    let data = crate::storage::root(&app);
    let plan_file = crate::storage::state(&app).join("plan-version");
    tauri::async_runtime::spawn_blocking(move || {
        let imported = import_from(
            &data,
            Path::new(&path),
            &id,
            &name,
            &plan_file,
            &data.join("cache"),
        )?;
        crate::applog::write(
            "info",
            "项目导入",
            &format!(
                "从 {path} 导入项目 {}（{}）",
                imported.meta.id, imported.meta.name
            ),
        );
        Ok(imported)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyProject {
    pub source_id: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub include_records: bool,
}

#[tauri::command]
pub async fn project_copy(app: AppHandle, request: CopyProject) -> Result<ProjectImported, String> {
    let data = crate::storage::root(&app);
    super::open_target(&data, &request.source_id)?;
    if request.source_id == super::current_id(&app) {
        crate::site::check_project_switch(&super::switch_state(&app)?)
            .map_err(|e| e.replace("切换项目", "复制当前项目"))?;
    }
    let plan_file = crate::storage::state(&app).join("plan-version");
    tauri::async_runtime::spawn_blocking(move || {
        let copied = copy_from(
            &data,
            &request.source_id,
            request.include_records,
            &New {
                id: &request.id,
                name: &request.name,
                description: &request.description,
            },
            &plan_file,
            &data.join("cache"),
        )?;
        crate::applog::write(
            "info",
            "项目复制",
            &format!(
                "已把项目 {} 复制为 {}（{}）{}",
                request.source_id,
                copied.meta.id,
                copied.meta.name,
                if request.include_records {
                    "，含检测记录"
                } else {
                    ""
                }
            ),
        );
        Ok(copied)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
