//! 本机检测历史：一份记录库与审计 spool 放在资料目录 `history/`，记录带所属项目；历史原图仍按项目放在日志根的
//! `vision/<环境>/<实例>/<项目>/`。启动时把各项目的旧记录库并入本机库，旧库改名备份。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::store::{RecordSource, Store};

pub fn history_dir(data: &Path) -> PathBuf {
    data.join("history")
}

/// 本机记录库。
pub fn database(data: &Path) -> PathBuf {
    history_dir(data).join("inspection.db")
}

/// 本机审计 spool（待入库的件证据）。
pub fn spool(data: &Path) -> PathBuf {
    history_dir(data).join("audit-spool")
}

/// 项目 `project` 的真机或离线录制根：记录的原图引用相对于它。
pub fn record_root(data: &Path, project: &str, source: RecordSource) -> Result<PathBuf, String> {
    if !super::valid_id(project) {
        return Err(format!("记录的项目编号 {project} 无效"));
    }
    let base = crate::log_storage::recording_project(&super::dir(data, project))?;
    Ok(base.join(if source.offline() { "records-offline" } else { "records" }))
}

/// 项目还在本机（目录与 `project.json` 可读）；删除后的项目只剩历史记录与原图。
pub fn project_available(data: &Path, project: &str) -> bool {
    super::valid_id(project) && super::read_meta(&super::dir(data, project)).is_ok()
}

/// 记录不属于当前项目时，按候选对照不可用的原因。
pub fn foreign_reason(data: &Path, project: &str) -> String {
    match super::read_meta(&super::dir(data, project)) {
        Ok(meta) => format!("这条记录属于项目“{}”（{project}），切换到该项目后可按候选对照", meta.name),
        Err(_) => format!("这条记录属于已删除的项目 {project}，不能按候选对照"),
    }
}

/// 记录的发布包所在目录；项目已删除时报原因。
pub fn releases_root(data: &Path, project: &str) -> Result<PathBuf, String> {
    if !project_available(data, project) {
        return Err(format!("这条记录所属的项目 {project} 已删除，发布包随项目一起删除，不能按原版本重现或画辅助线"));
    }
    Ok(super::dir(data, project).join("vision").join("releases"))
}

/// 启动时把各项目的旧记录库（`projects/<id>/inspection.db`）并入本机库：当前项目先并、尽量沿用原编号，其余项目重新编号
/// 并改写项目文件里的记录引用；并完的旧库改名为 `inspection.db.merged-<毫秒>.bak`，不删。当前项目未写完的审计 spool
/// 移到本机 spool。返回 (级别, 说明) 供写日志。
pub fn migrate(data: &Path, store: &Store, current: &str) -> Vec<(&'static str, String)> {
    let mut notes = Vec::new();
    let mut ids: Vec<String> = super::list(data)
        .unwrap_or_default()
        .into_iter()
        .map(|entry| entry.id)
        .filter(|id| super::valid_id(id))
        .collect();
    ids.sort_by_key(|id| (id != current, id.clone()));
    for id in &ids {
        let dir = super::dir(data, id);
        if let Err(error) = move_spool(data, &dir, id == current) {
            notes.push(("warn", format!("项目 {id} 的审计 spool：{error}")));
        }
        let db = dir.join("inspection.db");
        if !db.is_file() {
            continue;
        }
        let merged = match store.merged(id) {
            Ok(true) => None,
            Ok(false) => match store.merge_project(&db, id, id == current) {
                Ok(merged) => Some(merged),
                Err(error) => {
                    notes.push(("err", format!("项目 {id} 的检测记录未并入本机记录库，旧库保留：{error}")));
                    continue;
                }
            },
            Err(error) => {
                notes.push(("err", format!("核对项目 {id} 的记录合并状态失败：{error}")));
                continue;
            }
        };
        if let Some(merged) = &merged {
            if merged.renumbered() {
                if let Err(error) = remap_refs(&dir, &merged.ids) {
                    notes.push(("err", format!("项目 {id} 的记录已并入，但候选与采集轮次里的记录引用未改写完：{error}")));
                }
            }
            notes.push((
                "info",
                format!(
                    "项目 {id} 的 {} 件检测记录并入本机记录库{}",
                    merged.inserted,
                    if merged.duplicates > 0 { format!("，{} 件与已有记录是同一工件，未重复保存", merged.duplicates) } else { String::new() }
                ),
            ));
        }
        if let Err(error) = retire(&db) {
            notes.push(("warn", format!("项目 {id} 的旧记录库已并入，改名备份失败：{error}")));
        }
    }
    notes
}

/// 删一件过期记录的录制目录（非当前项目，没有在运行的录制滚动清理）：目录须在该项目对应的录制根下。
pub fn remove_recording(data: &Path, project: &str, source: RecordSource, directory: &Path) -> Result<(), String> {
    let root = record_root(data, project, source)?;
    let (Ok(root), Ok(checked)) = (root.canonicalize(), directory.canonicalize()) else {
        return Ok(());
    };
    if !checked.starts_with(&root) || checked == root {
        return Err("录制目录不在该项目的录制根下".into());
    }
    crate::image_file::checked_source_path(&checked)?;
    std::fs::remove_dir_all(&checked).map_err(|e| e.to_string())
}

fn retire(db: &Path) -> Result<(), String> {
    let stamp = ly_plc::now_ms();
    for suffix in ["", "-wal", "-shm"] {
        let mut from = db.as_os_str().to_os_string();
        from.push(suffix);
        let from = PathBuf::from(from);
        if from.exists() {
            let mut to = db.as_os_str().to_os_string();
            to.push(format!(".merged-{stamp}.bak{suffix}"));
            std::fs::rename(&from, PathBuf::from(to)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn non_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

fn move_spool(data: &Path, project: &Path, current: bool) -> Result<(), String> {
    let old = project.join("audit-spool");
    if !old.is_dir() {
        return Ok(());
    }
    if !non_empty(&old) {
        return std::fs::remove_dir(&old).map_err(|e| e.to_string());
    }
    if !current {
        return Err("还有未入库的件证据，项目不是当前项目，未移入本机 spool；切换到该项目时一并处理".into());
    }
    let target = spool(data);
    if non_empty(&target) {
        return Err("本机 spool 也有未入库的件证据，两边都保留，请人工核对".into());
    }
    if target.exists() {
        std::fs::remove_dir(&target).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(history_dir(data)).map_err(|e| e.to_string())?;
    std::fs::rename(&old, &target).map_err(|e| format!("移入本机 spool 失败：{e}"))
}

/// 项目文件里的记录引用按 `ids`（旧编号 → 本机编号）改写；不在 `ids` 里的编号（旧库里已清理的记录）改为 0，
/// 免得指到别的项目的记录。改写：候选 `workspace.json` 与示教帧元数据里的 `historyId`、候选的示教件 `recordId`
/// （冻结的配方文档 `doc`、待生效版本 `pending` 不改，它们只按 SN 与时间显示示教件，并与发布辅助快照逐字核对）、
/// 采集轮次 `round.json` 的来源 `recordId`，以及 `workspaces/comparisons/<编号>/` 的目录、文件名与内容。
pub(crate) fn remap_refs(project: &Path, ids: &BTreeMap<i64, i64>) -> Result<(), String> {
    let map = |old: i64| if old <= 0 { old } else { ids.get(&old).copied().unwrap_or(0) };
    let workspaces = project.join("workspaces");
    for entry in read_dirs(&workspaces)? {
        if entry.file_name().is_some_and(|n| n == "comparisons") {
            continue;
        }
        for file in json_files(&entry)? {
            rewrite(&file, |value| {
                let mut changed = history_ids(value, &map, true);
                if file.file_name().is_some_and(|n| n == "workspace.json") {
                    if let Some(record) = value.pointer_mut("/teachingPiece/recordId") {
                        changed |= remap_number(record, &map);
                    }
                }
                changed
            })?;
        }
    }
    for entry in read_dirs(&project.join("recipe-capture"))? {
        let file = entry.join("round.json");
        if file.is_file() {
            rewrite(&file, |value| {
                if value.pointer("/origin/kind").and_then(Value::as_str) != Some("history") {
                    return false;
                }
                value.pointer_mut("/origin/recordId").is_some_and(|record| remap_number(record, &map))
            })?;
        }
    }
    remap_comparisons(&workspaces.join("comparisons"), &map)
}

fn read_dirs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    crate::image_file::checked_source_path(dir)?;
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn json_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        crate::image_file::checked_source_path(&dir)?;
        for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let path = entry.path();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "json") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn rewrite(path: &Path, edit: impl FnOnce(&mut Value) -> bool) -> Result<(), String> {
    let text = crate::fsio::read_text(path).map_err(|e| format!("{}：{e}", path.display()))?;
    let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
        return Ok(());
    };
    if edit(&mut value) {
        crate::fsio::write_atomic(path, &serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?)?;
    }
    Ok(())
}

fn remap_number(value: &mut Value, map: &impl Fn(i64) -> i64) -> bool {
    match value.as_i64() {
        Some(old) if map(old) != old => {
            *value = map(old).into();
            true
        }
        _ => false,
    }
}

/// 改写 `historyId`；`top` 层跳过冻结的配方文档 `doc` 与待生效版本 `pending`。
fn history_ids(value: &mut Value, map: &impl Fn(i64) -> i64, top: bool) -> bool {
    match value {
        Value::Object(object) => {
            let mut changed = false;
            for (key, child) in object.iter_mut() {
                if top && (key == "doc" || key == "pending") {
                    continue;
                }
                changed |= if key == "historyId" { remap_number(child, map) } else { history_ids(child, map, false) };
            }
            changed
        }
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| history_ids(item, map, false) | changed),
        _ => false,
    }
}

/// 复测对照目录按新编号改名（先全部移到暂存目录再放回，免得新旧编号互相覆盖）；旧库里已清理的记录的对照删除，
/// 与历史清理时的处理相同。
fn remap_comparisons(root: &Path, map: &impl Fn(i64) -> i64) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    let staging = root.join(format!(".remap-{}", ly_plc::now_ms()));
    let mut moved = Vec::new();
    for dir in read_dirs(root)? {
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let Ok(old) = name.parse::<i64>() else { continue };
        if old <= 0 || name != old.to_string() {
            continue;
        }
        let new = map(old);
        if new == old {
            continue;
        }
        if new == 0 {
            crate::image_file::checked_source_path(&dir)?;
            std::fs::remove_dir_all(&dir).map_err(|e| format!("清理复测目录 {} 失败：{e}", dir.display()))?;
            continue;
        }
        std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        let target = staging.join(new.to_string());
        std::fs::rename(&dir, &target).map_err(|e| format!("复测目录 {} 改名失败：{e}", dir.display()))?;
        moved.push((old, new));
    }
    for (old, new) in moved {
        let dir = staging.join(new.to_string());
        for file in json_files(&dir)? {
            let stem = file.file_stem().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            let prefix = format!("{old}-");
            rewrite(&file, |value| {
                let mut changed = value.get_mut("historyId").is_some_and(|id| remap_number(id, &|_| new));
                if let Some(Value::String(id)) = value.get_mut("id") {
                    if let Some(rest) = id.strip_prefix(&prefix) {
                        *id = format!("{new}-{rest}");
                        changed = true;
                    }
                }
                changed
            })?;
            if let Some(rest) = stem.strip_prefix(&prefix) {
                std::fs::rename(&file, dir.join(format!("{new}-{rest}.json"))).map_err(|e| e.to_string())?;
            }
        }
        let target = root.join(new.to_string());
        if target.exists() {
            return Err(format!("复测目录 {} 已存在，未覆盖", target.display()));
        }
        std::fs::rename(&dir, &target).map_err(|e| e.to_string())?;
    }
    if staging.exists() {
        std::fs::remove_dir(&staging).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "records_tests.rs"]
mod tests;
