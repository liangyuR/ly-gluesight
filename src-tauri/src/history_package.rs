//! 历史件导出包（docs/architecture/history-package.md）：在历史页选一件或多件真机段配方记录，连同帧记录、所用配方快照与
//! （可选的）录制原图打成 ZIP；在本机或另一台机器上导入到当前项目的历史里，用于复测、示教与对比。
//!
//! - 包内 `manifest.json` 记包种类与格式版本、来源项目、导出时间、来源软件版本、各件身份与每个文件的字节数和 SHA-256；
//!   记录库 `records.db` 里的原图引用都是包内相对路径，不含本机绝对路径。
//! - 导入先把要装的文件解到当前项目录制根下的暂存目录并逐个核对，本机已有同一 cycleId 的件跳过；原图目录在记录库事务提交前
//!   移入 `records/imported/`，提交失败即移回删除。中断时目录带未提交标记，下次启动按记录库核对后删掉或保留。

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};
use zip::ZipArchive;

use crate::burst_judge::FrameState;
use crate::operation::Operation;
use crate::project::transfer::{self, PackageFile};
use crate::store::{RecordSource, RecordingState, Store, DB_VERSION};

pub const FORMAT: u32 = 1;
const FORMATS: [u32; 1] = [1];
const KIND: &str = "history";
const LABEL: &str = "历史件导出包";
const DATABASE: &str = "records.db";
const PARTS: &str = "parts";
/// 导入件原图所在的子目录（录制根下）；录制的滚动清理只扫日期目录，不会动它。
pub const IMPORTED: &str = "imported";
const PENDING: &str = ".history-import-pending";
const STAGING_PREFIX: &str = ".staging-";
/// 一次最多导出的件数：每件约四千个帧文件，清单要在导入方的清单大小上限（64 MB）以内。
pub const EXPORT_LIMIT: usize = 20;
const NO_IMAGES: &str = "导出包未带原图";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackagePart {
    pub cycle_id: String,
    pub sn: String,
    pub ts: i64,
    /// 原图目录名（包内 `parts/<目录>/`）；没带原图为 None
    pub directory: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageManifest {
    pub kind: String,
    pub format: u32,
    pub source_project: String,
    pub source_project_name: String,
    pub exported_at: i64,
    pub app_version: String,
    pub parts: Vec<PackagePart>,
    pub notes: Vec<String>,
    pub files: Vec<PackageFile>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub sn: String,
    pub ts: i64,
    pub cycle_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub imported: usize,
    /// 新并入件的本机记录编号
    pub ids: Vec<i64>,
    /// 本机已有同一 cycleId、没有导入的件
    pub skipped: Vec<Skipped>,
    pub notes: Vec<String>,
}

fn named(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && !part.contains(['\\', ':'])
        && !part.chars().any(char::is_control)
}

fn valid_cycle(cycle: &str) -> bool {
    !cycle.is_empty()
        && cycle.len() <= 128
        && cycle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// 录制目录名：`*_cycle_<cycleId>`，只含字母、数字、`-` 与 `_`（与读原图时的核对一致）。
fn valid_directory(name: &str, cycle: &str) -> bool {
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        && name
            .split_once("_cycle_")
            .is_some_and(|(_, id)| id == cycle)
}

/// 录制目录是否为导入件的原图目录（录制根下的 `imported/<目录>`）。
pub fn imported_recording(directory: &Path) -> bool {
    directory
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == IMPORTED)
}

/// 包内路径属于哪个原图目录（`parts/<目录>/...`）；不是原图返回 None。
fn part_directory(path: &str) -> Option<&str> {
    let rest = path.strip_prefix(PARTS)?.strip_prefix('/')?;
    let (directory, file) = rest.split_once('/')?;
    (!file.is_empty()).then_some(directory)
}

fn allowed(path: &str, directories: &BTreeSet<&str>) -> bool {
    if path == DATABASE {
        return true;
    }
    path.split('/').all(named)
        && part_directory(path).is_some_and(|directory| directories.contains(directory))
}

fn read_manifest<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<PackageManifest, String> {
    let value = transfer::manifest_value(zip, LABEL)?;
    if value.get("kind").and_then(Value::as_str) != Some(KIND) {
        return Err(format!("不是{LABEL}（可能是项目导出包或配方回传包）"));
    }
    let format = value.get("format").and_then(Value::as_u64);
    if !format.is_some_and(|f| FORMATS.iter().any(|known| u64::from(*known) == f)) {
        return Err(format!(
            "{LABEL}格式 {} 不受本机软件支持（支持 {FORMAT}），请升级软件后再导入",
            format.map_or("未知".into(), |f| f.to_string())
        ));
    }
    let manifest: PackageManifest =
        serde_json::from_value(value).map_err(|e| format!("{LABEL}清单格式无效：{e}"))?;
    if manifest.parts.is_empty() {
        return Err(format!("{LABEL}里没有历史件"));
    }
    let mut cycles = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for part in &manifest.parts {
        if !valid_cycle(&part.cycle_id) || !cycles.insert(part.cycle_id.as_str()) {
            return Err(format!(
                "{LABEL}清单的 cycleId 无效或重复：{}",
                part.cycle_id
            ));
        }
        if let Some(directory) = &part.directory {
            if !valid_directory(directory, &part.cycle_id)
                || !directories.insert(directory.as_str())
            {
                return Err(format!("{LABEL}清单的原图目录无效：{directory}"));
            }
        }
    }
    let seen = transfer::check_listed(&manifest.files, |path| allowed(path, &directories), LABEL)?;
    if !seen.contains(DATABASE) {
        return Err(format!("{LABEL}缺少记录库 {DATABASE}"));
    }
    for directory in &directories {
        if !seen.contains(&format!("{PARTS}/{directory}/part.json").to_ascii_lowercase()) {
            return Err(format!("{LABEL}的原图目录 {directory} 缺少 part.json"));
        }
    }
    Ok(manifest)
}

/// 读包清单（不解包）。
pub fn inspect(path: &Path) -> Result<PackageManifest, String> {
    read_manifest(&mut transfer::open_zip(path, LABEL)?)
}

/// 导出的来源：记录所属项目的真机录制根与项目名称。
pub struct ExportSource<'a> {
    pub project: &'a str,
    pub project_name: &'a str,
    pub root: &'a Path,
}

/// 把记录 `ids`（同一项目的真机段配方记录，带 cycleId）导出为 `target`；`include_images` 时带上原图完整的件的录制目录，
/// 其余件只带记录与每帧指标。`scratch` 放导出用的记录库子集，由调用方删除。
pub fn export_package(
    store: &Store,
    ids: &[i64],
    include_images: bool,
    source: &ExportSource,
    scratch: &Path,
    target: &Path,
    operation: &Operation,
) -> Result<PackageManifest, String> {
    if ids.is_empty() || ids.len() > EXPORT_LIMIT {
        return Err(format!("每次导出 1–{EXPORT_LIMIT} 件"));
    }
    if ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err("不能重复选择同一件".into());
    }
    let candidates = store.package_candidates(ids)?;
    let mut reasons = Vec::new();
    for c in &candidates {
        let s = &c.summary;
        let why = if s.project_id != source.project {
            Some("不属于同一个项目（一次只能导出同一项目的记录）")
        } else if s.source != RecordSource::Real {
            Some("不是真机记录")
        } else if !c.burst {
            Some("不是段配方记录")
        } else if !s.cycle_id.as_deref().is_some_and(valid_cycle) {
            Some("没有有效的 cycleId")
        } else {
            None
        };
        if let Some(why) = why {
            reasons.push(format!("SN {}（#{}）{why}", s.sn, s.id));
        }
    }
    if !reasons.is_empty() {
        return Err(format!("不能导出：{}", reasons.join("；")));
    }
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let database = scratch.join(DATABASE);
    store.export_parts(ids, &database)?;
    let root = source.root.canonicalize().ok();
    let mut notes = Vec::new();
    let mut entries = vec![(DATABASE.to_string(), database.clone())];
    let mut parts = Vec::with_capacity(candidates.len());
    let mut without_images = Vec::new();
    for c in &candidates {
        let s = &c.summary;
        let cycle = s.cycle_id.clone().unwrap_or_default();
        let directory = if include_images {
            match recording_directory(root.as_deref(), &c.recording, &cycle) {
                Ok((name, path)) => {
                    let mut walked = Vec::new();
                    let mut links = Vec::new();
                    transfer::walk(&path, &format!("{PARTS}/{name}"), &mut walked, &mut links)?;
                    if !links.is_empty() {
                        return Err(format!("SN {} 的录制目录包含链接，拒绝导出", s.sn));
                    }
                    entries.extend(walked);
                    Some(name)
                }
                Err(reason) => {
                    without_images.push(format!("SN {}：{reason}", s.sn));
                    None
                }
            }
        } else {
            None
        };
        parts.push(PackagePart {
            cycle_id: cycle,
            sn: s.sn.clone(),
            ts: s.ts,
            directory,
        });
    }
    if !without_images.is_empty() {
        notes.push(format!(
            "{} 件原图不可用，只导出记录与每帧指标：{}",
            without_images.len(),
            without_images.join("；")
        ));
    }
    if !include_images {
        notes.push("未带原图：导入后只能按已存指标重判，不能原图复测或选作示教件".into());
    }
    relativize(&database, &parts)?;
    let manifest = PackageManifest {
        kind: KIND.into(),
        format: FORMAT,
        source_project: source.project.into(),
        source_project_name: source.project_name.into(),
        exported_at: ly_plc::now_ms(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        parts,
        notes,
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

/// 记录的原图目录（名称，规范化路径）：录制完整、目录在来源录制根下、目录名与 cycleId 一致；否则给出原因。
fn recording_directory(
    root: Option<&Path>,
    recording: &crate::store::RecordingEvidence,
    cycle: &str,
) -> Result<(String, PathBuf), String> {
    if recording.state != RecordingState::Complete || !recording.available {
        return Err("录制不完整或原图已清理".into());
    }
    let root = root.ok_or("录制根目录不存在")?;
    let path = Path::new(recording.directory.as_deref().ok_or("记录没有录制目录")?)
        .canonicalize()
        .map_err(|_| "录制目录不存在或已清理".to_string())?;
    if !path.starts_with(root) || path == root {
        return Err("录制目录不在项目的真机录制根下".into());
    }
    crate::image_file::checked_source_path(&path)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|name| valid_directory(name, cycle))
        .ok_or("录制目录名与 cycleId 不一致")?
        .to_string();
    if !path.join("part.json").is_file() {
        return Err("录制元数据 part.json 缺失".into());
    }
    Ok((name, path))
}

/// 导出记录库里的原图引用改为包内相对路径：带原图的件录制目录记为 `parts/<目录>`、帧原图记为 `<目录>/<文件>`；
/// 没带原图的件去掉录制目录与帧原图引用并注明原因。
fn relativize(database: &Path, parts: &[PackagePart]) -> Result<(), String> {
    let mut conn = rusqlite::Connection::open(database).map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for part in parts {
        let id: i64 = tx
            .query_row(
                "SELECT id FROM parts WHERE cycle_id = ?1",
                [&part.cycle_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        match &part.directory {
            Some(name) => {
                tx.execute(
                    "UPDATE parts SET recording_directory = ?2 WHERE id = ?1",
                    rusqlite::params![id, format!("{PARTS}/{name}")],
                )
                .map_err(|e| e.to_string())?;
                let prefix = format!("/{name}/");
                tx.execute(
                    "UPDATE part_frames SET raw_file = substr(raw_file, instr('/' || raw_file, ?2)) \
                     WHERE part_id = ?1 AND raw_file IS NOT NULL AND instr('/' || raw_file, ?2) > 0",
                    rusqlite::params![id, prefix],
                )
                .map_err(|e| e.to_string())?;
                let stray: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM part_frames WHERE part_id = ?1 AND raw_file IS NOT NULL \
                         AND (substr(raw_file, 1, length(?2)) != ?2 OR instr(substr(raw_file, length(?2) + 1), '/') > 0)",
                        rusqlite::params![id, format!("{name}/")],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if stray > 0 {
                    return Err(format!(
                        "SN {} 有 {stray} 帧的原图引用不在本件录制目录里，拒绝导出",
                        part.sn
                    ));
                }
            }
            None => {
                let errors: String = tx
                    .query_row(
                        "SELECT recording_errors FROM parts WHERE id = ?1",
                        [id],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                let mut errors: Vec<String> = serde_json::from_str(&errors).unwrap_or_default();
                errors.push(NO_IMAGES.into());
                tx.execute(
                    "UPDATE parts SET recording_available = 0, recording_directory = NULL, recording_errors = ?2 WHERE id = ?1",
                    rusqlite::params![id, serde_json::to_string(&errors).map_err(|e| e.to_string())?],
                )
                .map_err(|e| e.to_string())?;
                tx.execute(
                    "UPDATE part_frames SET raw_file = NULL WHERE part_id = ?1",
                    [id],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

fn staging_dir(parent: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(parent).map_err(|e| format!("创建导入原图目录失败：{e}"))?;
    crate::image_file::checked_source_path(parent)?;
    let stamp = ly_plc::now_ms();
    for suffix in 0..1000 {
        let dir = parent.join(format!("{STAGING_PREFIX}{stamp}-{suffix}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("无法创建导入暂存目录".into())
}

/// 暂存记录库与清单逐件核对：库版本 10，记录正好是清单里的各件、都是真机段配方记录，原图引用都是包内相对路径；
/// 带原图的新件的 part.json 属于本件。`fresh` 为要装的件的 cycleId。
fn verify_staged(
    database: &Path,
    staging: &Path,
    manifest: &PackageManifest,
    fresh: &BTreeSet<&str>,
) -> Result<(), String> {
    let conn =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("{LABEL}的记录库无法打开：{e}"))?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version != DB_VERSION {
        return Err(format!(
            "{LABEL}的记录库版本 {version} 不受支持（支持 {DB_VERSION}）"
        ));
    }
    let mut stmt = conn
        .prepare("SELECT id, cycle_id, source, burst_judgement IS NOT NULL, recording_directory FROM parts")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(i64, Option<String>, String, bool, Option<String>)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{LABEL}的记录库损坏：{e}"))?;
    let listed: BTreeMap<&str, &PackagePart> = manifest
        .parts
        .iter()
        .map(|p| (p.cycle_id.as_str(), p))
        .collect();
    if rows.len() != listed.len() {
        return Err(format!("{LABEL}的记录库与清单件数不一致"));
    }
    for (id, cycle, source, burst, directory) in rows {
        let part = cycle
            .as_deref()
            .and_then(|c| listed.get(c))
            .ok_or_else(|| format!("{LABEL}的记录库含清单之外的记录"))?;
        if source != "real" || !burst {
            return Err(format!("{LABEL}里 SN {} 不是真机段配方记录", part.sn));
        }
        let expected = part.directory.as_ref().map(|d| format!("{PARTS}/{d}"));
        if directory != expected {
            return Err(format!("{LABEL}里 SN {} 的原图目录与清单不一致", part.sn));
        }
        let prefix = part.directory.as_ref().map(|d| format!("{d}/"));
        let stray: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM part_frames WHERE part_id = ?1 AND raw_file IS NOT NULL \
                 AND (?2 IS NULL OR substr(raw_file, 1, length(?2)) != ?2 OR instr(substr(raw_file, length(?2) + 1), '/') > 0 \
                      OR instr(raw_file, '\\') > 0 OR instr(raw_file, ':') > 0 OR instr(raw_file, '..') > 0)",
                rusqlite::params![id, prefix],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if stray > 0 {
            return Err(format!("{LABEL}里 SN {} 的帧原图引用无效", part.sn));
        }
        if let (Some(directory), true) = (&part.directory, fresh.contains(part.cycle_id.as_str())) {
            let meta = staging.join(PARTS).join(directory).join("part.json");
            let value: Value =
                serde_json::from_slice(&std::fs::read(&meta).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("{LABEL}里 SN {} 的录制元数据损坏：{e}", part.sn))?;
            if value["cycleId"].as_str() != Some(part.cycle_id.as_str()) {
                return Err(format!(
                    "{LABEL}里 SN {} 的录制元数据与记录不是同一件",
                    part.sn
                ));
            }
        }
    }
    Ok(())
}

/// 暂存记录库里的原图引用改到当前项目：录制目录为 `<录制根>/imported/<目录>`，帧原图为 `imported/<目录>/<文件>`。
fn localize(database: &Path, root: &Path) -> Result<(), String> {
    let mut conn = rusqlite::Connection::open(database).map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let directories: Vec<(i64, String)> = {
        let mut stmt = tx
            .prepare(
                "SELECT id, recording_directory FROM parts WHERE recording_directory IS NOT NULL",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    for (id, directory) in directories {
        let name = directory
            .strip_prefix(&format!("{PARTS}/"))
            .ok_or("原图目录无效")?;
        tx.execute(
            "UPDATE parts SET recording_directory = ?2 WHERE id = ?1",
            rusqlite::params![id, root.join(IMPORTED).join(name).to_string_lossy()],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute(
        &format!("UPDATE part_frames SET raw_file = '{IMPORTED}/' || raw_file WHERE raw_file IS NOT NULL"),
        [],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

/// 把导出包 `package` 导入为项目 `project` 的记录，原图装进 `root`（当前项目的真机录制根）下的 `imported/`。
/// 本机已有同一 cycleId 的件跳过；包损坏、版本不支持、配方快照冲突时整包拒绝，不留下记录或原图。
pub fn import_package(
    store: &Store,
    project: &str,
    root: &Path,
    package: &Path,
    operation: &Operation,
) -> Result<Imported, String> {
    let mut zip = transfer::open_zip(package, LABEL)?;
    let manifest = read_manifest(&mut zip)?;
    let cycles: Vec<String> = manifest.parts.iter().map(|p| p.cycle_id.clone()).collect();
    let known = store.known_cycles(&cycles)?;
    let skipped: Vec<Skipped> = manifest
        .parts
        .iter()
        .filter(|p| known.contains(&p.cycle_id))
        .map(|p| Skipped {
            sn: p.sn.clone(),
            ts: p.ts,
            cycle_id: p.cycle_id.clone(),
        })
        .collect();
    let mut notes = Vec::new();
    if skipped.len() == manifest.parts.len() {
        return Ok(Imported {
            imported: 0,
            ids: Vec::new(),
            skipped,
            notes,
        });
    }
    let fresh: BTreeSet<&str> = manifest
        .parts
        .iter()
        .filter(|p| !known.contains(&p.cycle_id))
        .map(|p| p.cycle_id.as_str())
        .collect();
    let wanted_directories: BTreeSet<&str> = manifest
        .parts
        .iter()
        .filter(|p| fresh.contains(p.cycle_id.as_str()))
        .filter_map(|p| p.directory.as_deref())
        .collect();
    let directories: BTreeSet<&str> = manifest
        .parts
        .iter()
        .filter_map(|p| p.directory.as_deref())
        .collect();
    let imported_root = root.join(IMPORTED);
    let staging = staging_dir(&imported_root)?;
    let mut moved: Vec<PathBuf> = Vec::new();
    let mut inserted_cycles: BTreeSet<String> = BTreeSet::new();
    let mut ids: Vec<i64> = Vec::new();
    let result = (|| {
        operation.measuring(manifest.files.len())?;
        transfer::extract_listed_with(
            &mut zip,
            &manifest.files,
            &staging,
            |path| allowed(path, &directories),
            LABEL,
            |path| part_directory(path).is_none_or(|d| wanted_directories.contains(d)),
            || operation.advance(FrameState::Measured),
        )?;
        let database = staging.join(DATABASE);
        verify_staged(&database, &staging, &manifest, &fresh)?;
        localize(&database, root)?;
        for directory in &wanted_directories {
            crate::fsio::write_atomic(&staging.join(PARTS).join(directory).join(PENDING), project)?;
        }
        operation.saving()?;
        store.import_parts(&database, project, |inserted| {
            let by_cycle: BTreeMap<&str, &PackagePart> = manifest
                .parts
                .iter()
                .map(|p| (p.cycle_id.as_str(), p))
                .collect();
            for (cycle, id) in inserted {
                inserted_cycles.insert(cycle.clone());
                ids.push(*id);
            }
            for (cycle, _) in inserted {
                let Some(directory) = by_cycle
                    .get(cycle.as_str())
                    .and_then(|p| p.directory.as_deref())
                else {
                    continue;
                };
                let target = imported_root.join(directory);
                if target.exists() {
                    return Err(format!(
                        "导入原图目录已存在，拒绝覆盖：{}",
                        target.display()
                    ));
                }
                std::fs::rename(staging.join(PARTS).join(directory), &target)
                    .map_err(|e| format!("移入导入原图目录失败：{e}"))?;
                moved.push(target);
            }
            Ok(())
        })
    })();
    let merged = match result {
        Ok(merged) => merged,
        Err(error) => {
            for path in &moved {
                let _ = std::fs::remove_dir_all(path);
            }
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    let _ = std::fs::remove_dir_all(&staging);
    for path in &moved {
        if let Err(error) = std::fs::remove_file(path.join(PENDING)) {
            crate::applog::write(
                "warn",
                "历史件导入标记待清理",
                &format!("{}：{error}", path.display()),
            );
        }
    }
    let skipped: Vec<Skipped> = manifest
        .parts
        .iter()
        .filter(|p| !inserted_cycles.contains(&p.cycle_id))
        .map(|p| Skipped {
            sn: p.sn.clone(),
            ts: p.ts,
            cycle_id: p.cycle_id.clone(),
        })
        .collect();
    let without = manifest
        .parts
        .iter()
        .filter(|p| inserted_cycles.contains(&p.cycle_id) && p.directory.is_none())
        .count();
    if without > 0 {
        notes.push(format!(
            "{without} 件没带原图：只能按已存指标重判，不能原图复测或选作示教件"
        ));
    }
    Ok(Imported {
        imported: merged.inserted,
        ids,
        skipped,
        notes,
    })
}

/// 上次导入中断留下的原图目录：删掉暂存目录；带未提交标记的目录，记录库里有记录引用就去掉标记，没有就删除。
pub fn recover(store: &Store, root: &Path) -> Vec<String> {
    let dir = root.join(IMPORTED);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let result = (|| {
            if !path.is_dir() || crate::image_file::checked_source_path(&path).is_err() {
                return Ok(());
            }
            if name.starts_with(STAGING_PREFIX) {
                return std::fs::remove_dir_all(&path).map_err(|e| e.to_string());
            }
            if !path.join(PENDING).is_file() {
                return Ok(());
            }
            if store.has_recording(&path.to_string_lossy())? {
                std::fs::remove_file(path.join(PENDING)).map_err(|e| e.to_string())
            } else {
                std::fs::remove_dir_all(&path).map_err(|e| e.to_string())
            }
        })();
        if let Err(error) = result {
            warnings.push(format!("{}：{error}", path.display()));
        }
    }
    warnings
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPackageExported {
    pub path: String,
    pub bytes: u64,
    pub parts: usize,
    pub with_images: usize,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPackagePreview {
    pub path: String,
    pub bytes: u64,
    pub source_project: String,
    pub source_project_name: String,
    pub exported_at: i64,
    pub app_version: String,
    pub parts: usize,
    pub with_images: usize,
    pub duplicates: usize,
    pub notes: Vec<String>,
}

fn exports_dir(app: &AppHandle) -> PathBuf {
    crate::storage::root(app).join("exports")
}

/// 导出前选保存位置。
#[tauri::command]
pub async fn history_package_target(
    window: tauri::WebviewWindow,
    app: AppHandle,
    count: usize,
) -> Result<Option<String>, String> {
    let exports = exports_dir(&app);
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&exports).map_err(|e| e.to_string())?;
        let name = format!(
            "历史件_{count}件_{}.zip",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        Ok(rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("导出历史件")
            .set_directory(&exports)
            .set_file_name(name)
            .add_filter(LABEL, &["zip"])
            .save_file()
            .map(|p| p.display().to_string()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn history_package_export(
    app: AppHandle,
    ids: Vec<i64>,
    include_images: bool,
    path: String,
    task_id: Option<String>,
) -> Result<HistoryPackageExported, String> {
    let operation = Operation::start(&app, task_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        operation.run(|operation| {
            let store = app.state::<Store>();
            let data = crate::storage::root(&app);
            let project = store
                .package_candidates(&ids[..ids.len().min(1)])?
                .into_iter()
                .next()
                .map(|c| c.summary.project_id)
                .ok_or_else(|| format!("每次导出 1–{EXPORT_LIMIT} 件"))?;
            let name = crate::project::read_meta(&crate::project::dir(&data, &project))
                .map(|meta| meta.name)
                .unwrap_or_else(|_| project.clone());
            let root = crate::project::records::record_root(&data, &project, RecordSource::Real)?;
            let scratch = transfer::staging_dir(&data.join("cache"), "history-export")?;
            let target = PathBuf::from(&path);
            let manifest = export_package(
                &store,
                &ids,
                include_images,
                &ExportSource {
                    project: &project,
                    project_name: &name,
                    root: &root,
                },
                &scratch,
                &target,
                operation,
            );
            let _ = std::fs::remove_dir_all(&scratch);
            let manifest = manifest?;
            let bytes = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
            let with_images = manifest
                .parts
                .iter()
                .filter(|p| p.directory.is_some())
                .count();
            crate::applog::write(
                "info",
                "历史件导出",
                &format!(
                    "已导出 {} 件（{with_images} 件带原图）到 {}（{:.1} MB）",
                    manifest.parts.len(),
                    target.display(),
                    bytes as f64 / 1048576.0
                ),
            );
            Ok(HistoryPackageExported {
                path: target.display().to_string(),
                bytes,
                parts: manifest.parts.len(),
                with_images,
                notes: manifest.notes,
            })
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 选导出包并读清单（不导入）。
#[tauri::command]
pub async fn history_package_pick(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<Option<HistoryPackagePreview>, String> {
    let exports = exports_dir(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("选择历史件导出包")
            .add_filter(LABEL, &["zip"]);
        if exports.is_dir() {
            dialog = dialog.set_directory(exports);
        }
        let Some(path) = dialog.pick_file() else {
            return Ok(None);
        };
        let manifest = inspect(&path)?;
        let cycles: Vec<String> = manifest.parts.iter().map(|p| p.cycle_id.clone()).collect();
        let duplicates = app.state::<Store>().known_cycles(&cycles)?.len();
        Ok(Some(HistoryPackagePreview {
            bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
            path: path.display().to_string(),
            source_project: manifest.source_project,
            source_project_name: manifest.source_project_name,
            exported_at: manifest.exported_at,
            app_version: manifest.app_version,
            parts: manifest.parts.len(),
            with_images: manifest
                .parts
                .iter()
                .filter(|p| p.directory.is_some())
                .count(),
            duplicates,
            notes: manifest.notes,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn history_package_import(
    app: AppHandle,
    path: String,
    task_id: Option<String>,
) -> Result<Imported, String> {
    let operation = Operation::start(&app, task_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        operation.run(|operation| {
            let store = app.state::<Store>();
            let data = crate::storage::root(&app);
            let project = store.project().to_string();
            let root = crate::project::records::record_root(&data, &project, RecordSource::Real)?;
            let mut imported = import_package(&store, &project, &root, Path::new(&path), operation)?;
            let days = app.state::<crate::cycle::CycleHost>().settings().history_days.max(1) as i64;
            let cutoff = ly_plc::now_ms() - days * 86_400_000;
            let stale = inspect(Path::new(&path))
                .map(|m| m.parts.iter().filter(|p| p.ts < cutoff && !imported.skipped.iter().any(|s| s.cycle_id == p.cycle_id)).count())
                .unwrap_or(0);
            if stale > 0 {
                imported.notes.push(format!(
                    "{stale} 件早于历史保留期（{days} 天），会在下次记录清理时连同原图删除；需要保留请先在设置里调大保留天数"
                ));
            }
            crate::applog::write(
                "info",
                "历史件导入",
                &format!(
                    "从 {path} 导入 {} 件到项目 {project}，{} 件本机已有、跳过",
                    imported.imported,
                    imported.skipped.len()
                ),
            );
            Ok(imported)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "history_package/tests.rs"]
mod tests;
