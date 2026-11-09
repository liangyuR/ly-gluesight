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

/// 项目 `project` 在各录制盘上的录制根，按写入优先顺序：每件的录制目录与帧原图引用相对于其中一个。
pub fn record_roots(data: &Path, project: &str) -> Result<Vec<PathBuf>, String> {
    if !super::valid_id(project) {
        return Err(format!("记录的项目编号 {project} 无效"));
    }
    Ok(
        crate::log_storage::recording_projects(&super::dir(data, project))?
            .into_iter()
            .map(|base| base.join("records"))
            .collect(),
    )
}

/// 各录制盘上的录制根，建好目录；离线的盘建不了，照样列出（接上后录制时再建）。
pub fn prepared_roots(projects: &[PathBuf]) -> Vec<PathBuf> {
    projects
        .iter()
        .map(|base| {
            let root = base.join("records");
            let _ = std::fs::create_dir_all(&root);
            root
        })
        .collect()
}

/// 一件记录的录制目录所在的录制根：必须在已配置的某块录制盘上；那块盘离线或移除时报“原图所在盘不可用”。
pub fn record_root_of(
    data: &Path,
    project: &str,
    directory: Option<&str>,
) -> Result<PathBuf, String> {
    let directory = Path::new(directory.ok_or("这条记录没有录制目录，原图未录制或已清理")?);
    let root = record_roots(data, project)?
        .into_iter()
        .find(|root| crate::log_storage::within(root, directory))
        .ok_or_else(|| {
            format!(
                "原图所在盘不可用：录制目录 {} 不在已配置的录制盘上，在设置里加回这块盘后可读",
                directory.display()
            )
        })?;
    if !root.is_dir() {
        return Err(format!(
            "原图所在盘不可用：{} 无法访问（盘离线或已拔出）",
            root.display()
        ));
    }
    Ok(root)
}

/// 本机各项目采集轮次（示教件、标定件）引用的历史件 cycleId：这些记录与录制不参与按天清理与录制滚动清理，也不能手动删除；
/// 换示教件或删除候选会删掉轮次，保护随之解除。
pub fn referenced_cycles(data: &Path) -> Result<std::collections::BTreeSet<String>, String> {
    let mut cycles = std::collections::BTreeSet::new();
    for entry in super::list(data)? {
        if super::valid_id(&entry.id) {
            cycles.extend(crate::recipe_capture::history_cycles(
                &super::dir(data, &entry.id).join("recipe-capture"),
            )?);
        }
    }
    Ok(cycles)
}

/// 项目还在本机（目录与 `project.json` 可读）；删除后的项目只剩历史记录与原图。
pub fn project_available(data: &Path, project: &str) -> bool {
    super::valid_id(project) && super::read_meta(&super::dir(data, project)).is_ok()
}

/// 记录的发布包所在目录；项目已删除时报原因。
pub fn releases_root(data: &Path, project: &str) -> Result<PathBuf, String> {
    if !project_available(data, project) {
        return Err(format!(
            "这条记录所属的项目 {project} 已删除，发布包随项目一起删除，不能按原版本重现或画辅助线"
        ));
    }
    Ok(super::dir(data, project).join("vision").join("releases"))
}

/// 启动时把各项目的旧记录库（`projects/<id>/inspection.db`）并入本机库：当前项目先并、尽量沿用原编号，其余项目重新编号；
/// 并入后按记在本机库里的编号对照改写项目文件里的记录引用（`remap_refs`，中断后下次启动接着做），改写完才把旧库改名为
/// `inspection.db.merged-<毫秒>.bak`（不删）。当前项目未写完的审计 spool 移到本机 spool。返回 (级别, 说明) 供写日志。
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
        let merged = match store.merged(id) {
            Ok(merged) => merged,
            Err(error) => {
                notes.push(("err", format!("核对项目 {id} 的记录合并状态失败：{error}")));
                continue;
            }
        };
        if !merged {
            if !db.is_file() {
                continue;
            }
            match store.merge_project(&db, id, id == current, false) {
                Ok(merged) => notes.push((
                    "info",
                    format!(
                        "项目 {id} 的 {} 件检测记录并入本机记录库{}",
                        merged.inserted,
                        if merged.duplicates > 0 {
                            format!("，{} 件与已有记录是同一工件，未重复保存", merged.duplicates)
                        } else {
                            String::new()
                        }
                    ),
                )),
                Err(error) => {
                    notes.push((
                        "err",
                        format!("项目 {id} 的检测记录未并入本机记录库，旧库保留：{error}"),
                    ));
                    continue;
                }
            }
        }
        if let Err(error) = finish_refs(store, &dir, id) {
            notes.push((
                "err",
                format!("项目 {id} 的记录已并入，候选与采集轮次里的记录引用还没改写完，旧库保留，下次启动继续：{error}"),
            ));
            continue;
        }
        if db.is_file() {
            if let Err(error) = retire(&db) {
                notes.push((
                    "warn",
                    format!("项目 {id} 的旧记录库已并入，改名备份失败：{error}"),
                ));
            }
        }
    }
    notes
}

/// 上次导入中断的项目：记录已并入本机库，但项目目录没装上。目录在（装好后还没来得及记下）就记下装好；不在就撤回这次并入的
/// 记录，编号可以再用（它的外部原图暂存已由 `transfer::recover_pending_recordings` 清掉）。返回 (级别, 说明) 供写日志。
pub fn recover_installs(data: &Path, store: &Store) -> Vec<(&'static str, String)> {
    let pending = match store.pending_installs() {
        Ok(pending) => pending,
        Err(error) => return vec![("err", format!("核对中断的项目导入失败：{error}"))],
    };
    pending
        .into_iter()
        .map(|id| {
            let result = if project_available(data, &id) {
                store
                    .finish_install(&id)
                    .map(|_| ("info", format!("项目 {id} 的导入已完成，记下其检测记录")))
            } else {
                store.forget_project(&id).map(|_| {
                    (
                        "warn",
                        format!("项目 {id} 的导入中断，已撤回并入的检测记录，可用同一编号重新导入"),
                    )
                })
            };
            result.unwrap_or_else(|error| ("err", format!("处理中断的项目导入 {id} 失败：{error}")))
        })
        .collect()
}

/// 并入后还没改写完的记录引用：按本机库记下的编号对照改写，完成后在库里记下。没有待改写的什么也不做。
pub(crate) fn finish_refs(store: &Store, project: &Path, id: &str) -> Result<(), String> {
    match store.pending_refs(id)? {
        Some(ids) => {
            remap_refs(project, &ids)?;
            store.finish_refs(id)?;
            remove_manifest(project)
        }
        None => remove_manifest(project),
    }
}

/// 删一件过期记录的录制目录（非当前项目，没有在运行的录制滚动清理）：目录须在该项目某块录制盘的录制根下；
/// 目录已不在或所在盘离线时什么也不做。
pub fn remove_recording(
    data: &Path,
    project: &str,
    _source: RecordSource,
    directory: &Path,
) -> Result<(), String> {
    let roots = record_roots(data, project)?;
    let Ok(checked) = directory.canonicalize() else {
        return Ok(());
    };
    if !roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| checked.starts_with(&root) && checked != root)
    {
        return Err("录制目录不在该项目的录制盘上".into());
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
        return Err(
            "还有未入库的件证据，项目不是当前项目，未移入本机 spool；切换到该项目时一并处理".into(),
        );
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

const MANIFEST: &str = ".record-remap.json";
const STAGED: &str = ".remap-new";
const COMPARISONS_NEW: &str = ".comparisons-remap";
const COMPARISONS_OLD: &str = ".comparisons-old";

fn staged(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(STAGED);
    PathBuf::from(name)
}

fn remove_manifest(project: &Path) -> Result<(), String> {
    let manifest = project.join(MANIFEST);
    if manifest.exists() {
        std::fs::remove_file(&manifest).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 项目文件里的记录引用按 `ids`（旧编号 → 本机编号）改写；不在 `ids` 里的编号（旧库里已清理的记录）改为 0，
/// 免得指到别的项目的记录，编号没变时也照样清理。改写：候选 `workspace.json` 与示教帧元数据里的 `historyId`、候选的示教件
/// `recordId`（冻结的配方文档 `doc`、待生效版本 `pending` 不改，它们只按 SN 与时间显示示教件，并与发布辅助快照逐字核对）、
/// 采集轮次 `round.json` 的来源 `recordId`，以及 `workspaces/comparisons/<编号>/` 的目录、文件名与内容（旧库里已清理的
/// 记录的对照删除，与历史清理一致）。
///
/// 先把改写结果全部写成暂存（文件旁的 `.remap-new`、整个新的 `workspaces/.comparisons-remap/`），再写清单
/// `.record-remap.json`，最后按清单逐项换上：清单写好之前中断，下次从头再算一遍（原文件没动）；之后中断，下次按清单接着换，
/// 每一步都可重复，不会把编号映射两次。
pub(crate) fn remap_refs(project: &Path, ids: &BTreeMap<i64, i64>) -> Result<(), String> {
    if !project.join(MANIFEST).is_file() {
        stage_refs(project, ids)?;
    }
    apply_staged(project)
}

fn stage_refs(project: &Path, ids: &BTreeMap<i64, i64>) -> Result<(), String> {
    let map = |old: i64| {
        if old <= 0 {
            old
        } else {
            ids.get(&old).copied().unwrap_or(0)
        }
    };
    let mut files = Vec::new();
    let workspaces = project.join("workspaces");
    for entry in read_dirs(&workspaces)? {
        if entry
            .file_name()
            .is_some_and(|n| n == "comparisons" || n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        for file in json_files(&entry)? {
            let workspace = file.file_name().is_some_and(|n| n == "workspace.json");
            stage(&file, &mut files, |value| {
                let mut changed = history_ids(value, &map, true);
                if workspace {
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
            stage(&file, &mut files, |value| {
                if value.pointer("/origin/kind").and_then(Value::as_str) != Some("history") {
                    return false;
                }
                value
                    .pointer_mut("/origin/recordId")
                    .is_some_and(|record| remap_number(record, &map))
            })?;
        }
    }
    let comparisons = stage_comparisons(&workspaces, &map)?;
    crate::fsio::write_atomic(
        &project.join(MANIFEST),
        &serde_json::json!({"files": files, "comparisons": comparisons}).to_string(),
    )
}

/// 改写一个 JSON 文件到它旁边的暂存文件；有改动时记下相对项目根的路径。
fn stage(
    path: &Path,
    files: &mut Vec<PathBuf>,
    edit: impl FnOnce(&mut Value) -> bool,
) -> Result<(), String> {
    let text = crate::fsio::read_text(path).map_err(|e| format!("{}：{e}", path.display()))?;
    let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
        return Ok(());
    };
    if edit(&mut value) {
        crate::fsio::write_atomic(
            &staged(path),
            &serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?,
        )?;
        files.push(path.to_path_buf());
    }
    Ok(())
}

/// 按新编号把复测对照整份复制到 `workspaces/.comparisons-remap/`；没有对照目录时为 false。
fn stage_comparisons(workspaces: &Path, map: &impl Fn(i64) -> i64) -> Result<bool, String> {
    let root = workspaces.join("comparisons");
    if !root.is_dir() {
        return Ok(false);
    }
    let target = workspaces.join(COMPARISONS_NEW);
    if target.exists() {
        crate::image_file::checked_source_path(&target)?;
        std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    for dir in read_dirs(&root)? {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let parsed = name
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0 && name == id.to_string());
        let (old, new) = match parsed {
            Some(old) => (old, map(old)),
            None => continue,
        };
        if new == 0 {
            continue;
        }
        let into = target.join(new.to_string());
        std::fs::create_dir_all(&into).map_err(|e| e.to_string())?;
        let prefix = format!("{old}-");
        for file in json_files(&dir)? {
            let mut value: Value = serde_json::from_str(
                &crate::fsio::read_text(&file).map_err(|e| format!("{}：{e}", file.display()))?,
            )
            .map_err(|e| format!("{}：{e}", file.display()))?;
            if let Some(id) = value.get_mut("historyId") {
                *id = new.into();
            }
            if let Some(Value::String(id)) = value.get_mut("id") {
                if let Some(rest) = id.strip_prefix(&prefix) {
                    *id = format!("{new}-{rest}");
                }
            }
            let stem = file
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let name = stem
                .strip_prefix(&prefix)
                .map_or(stem.clone(), |rest| format!("{new}-{rest}"));
            crate::fsio::write_atomic(
                &into.join(format!("{name}.json")),
                &serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?,
            )?;
        }
    }
    Ok(true)
}

/// 按清单换上暂存的改写：文件逐个改名覆盖（已换上的跳过），复测对照整份换目录；每一步都可重复。
fn apply_staged(project: &Path) -> Result<(), String> {
    let manifest = project.join(MANIFEST);
    let value: Value = serde_json::from_str(
        &crate::fsio::read_text(&manifest).map_err(|e| format!("记录引用改写清单：{e}"))?,
    )
    .map_err(|e| format!("记录引用改写清单损坏：{e}"))?;
    for file in value["files"].as_array().into_iter().flatten() {
        let target = PathBuf::from(file.as_str().ok_or("记录引用改写清单损坏")?);
        if !target.starts_with(project) {
            return Err("记录引用改写清单里的文件不在项目目录".into());
        }
        let source = staged(&target);
        if source.exists() {
            std::fs::rename(&source, &target)
                .map_err(|e| format!("换上 {} 失败：{e}", target.display()))?;
        }
    }
    if value["comparisons"].as_bool() == Some(true) {
        let workspaces = project.join("workspaces");
        let (current, new, old) = (
            workspaces.join("comparisons"),
            workspaces.join(COMPARISONS_NEW),
            workspaces.join(COMPARISONS_OLD),
        );
        if new.exists() {
            if current.exists() && !old.exists() {
                std::fs::rename(&current, &old).map_err(|e| e.to_string())?;
            }
            if !current.exists() {
                std::fs::rename(&new, &current).map_err(|e| e.to_string())?;
            }
        }
        if old.exists() && !new.exists() {
            crate::image_file::checked_source_path(&old)?;
            std::fs::remove_dir_all(&old).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
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
                changed |= if key == "historyId" {
                    remap_number(child, map)
                } else {
                    history_ids(child, map, false)
                };
            }
            changed
        }
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| {
            history_ids(item, map, false) | changed
        }),
        _ => false,
    }
}
