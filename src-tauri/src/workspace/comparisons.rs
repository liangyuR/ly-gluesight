use std::fs::{self, Metadata, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use super::Comparison;
use crate::store::Store;

const LIMIT: usize = 100;

#[derive(Default)]
pub(super) struct Comparisons(Mutex<()>);

impl Comparisons {
    pub(super) fn save(&self, root: &Path, store: &Store, comparison: &Comparison) -> Result<(), String> {
        let _gate = self.0.lock().unwrap();
        valid_identity(comparison)?;
        let cycle = store.comparison_cycle(comparison.history_id)?.ok_or("历史记录已清理，复测结果未保存")?;
        if cycle != comparison.cycle_id { return Err("历史工件身份已改变，复测结果未保存".into()); }
        let dir = directory(root, comparison.history_id, true)?;
        reconcile(&dir, comparison.history_id, Some(&cycle))?;
        let file = dir.join(format!("{}.json", comparison.id));
        let temporary = dir.join(format!("{}.tmp", comparison.id));
        checked_directory(&dir, false)?;
        if fs::symlink_metadata(&file).is_ok() { return Err("复测记录文件已存在".into()); }
        let bytes = serde_json::to_vec(comparison).map_err(error)?;
        let mut writer = OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(error)?;
        let written = writer.write_all(&bytes).and_then(|_| writer.sync_all());
        drop(writer);
        if let Err(e) = written { let _ = fs::remove_file(&temporary); return Err(error(e)); }
        if let Err(e) = fs::rename(&temporary, &file) { let _ = fs::remove_file(&temporary); return Err(error(e)); }
        if let Err(e) = reconcile(&dir, comparison.history_id, Some(&cycle)) {
            let rollback = remove_checked(&file);
            return Err(format!("复测记录保留清理失败：{e}；新记录回退：{rollback:?}"));
        }
        Ok(())
    }

    pub(super) fn list(&self, root: &Path, store: &Store, candidate: &str, history_id: i64) -> Result<Vec<Comparison>, String> {
        let _gate = self.0.lock().unwrap();
        let Some(cycle) = store.comparison_cycle(history_id)? else { return Ok(Vec::new()); };
        let dir = directory(root, history_id, false)?;
        if !exists(&dir)? { return Ok(Vec::new()); }
        let mut files = inventory(&dir, history_id)?;
        files.retain(|(_, saved)| saved.cycle_id == cycle);
        newest_first(&mut files);
        Ok(files.into_iter().take(LIMIT).map(|(_, saved)| saved)
            .filter(|saved| saved.source == "original" || saved.candidate_id == candidate).collect())
    }

    pub(super) fn purge(&self, root: &Path, store: &Store, before: i64) -> Result<(usize, Vec<String>), String> {
        let _gate = self.0.lock().unwrap();
        let count = store.purge_before(before)?;
        let mut warnings = Vec::new();
        let comparisons = root.join("comparisons");
        let scan = (|| {
            checked_directory(&comparisons, false)?;
            if !exists(&comparisons)? { return Ok(()); }
            for entry in fs::read_dir(&comparisons).map_err(error)? {
                let entry = entry.map_err(error)?;
                let path = entry.path();
                let cleaned = (|| {
                    let name = entry.file_name().into_string().map_err(|_| "复测目录名称无效".to_string())?;
                    let id: i64 = name.parse().map_err(|_| "复测目录不是历史编号".to_string())?;
                    if id <= 0 || name != id.to_string() { return Err("复测目录编号无效".into()); }
                    checked_directory(&path, false)?;
                    let cycle = store.comparison_cycle(id)?;
                    reconcile(&path, id, cycle.as_deref())?;
                    if cycle.is_none() { fs::remove_dir(&path).map_err(error)?; }
                    Ok::<_, String>(())
                })();
                if let Err(e) = cleaned { warnings.push(format!("{}：{e}；未确认文件已保留", path.display())); }
            }
            Ok::<_, String>(())
        })();
        if let Err(e) = scan { warnings.push(format!("复测目录清理未完成：{e}")); }
        Ok((count, warnings))
    }
}

fn error(e: impl std::fmt::Display) -> String { e.to_string() }

fn valid_identity(saved: &Comparison) -> Result<(), String> {
    let prefix = format!("{}-", saved.history_id);
    if saved.history_id <= 0 || saved.cycle_id.is_empty() || !saved.id.starts_with(&prefix)
        || !saved.id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
    { return Err("复测记录身份无效".into()); }
    Ok(())
}

fn directory(root: &Path, history_id: i64, create: bool) -> Result<PathBuf, String> {
    if history_id <= 0 { return Err("历史编号无效".into()); }
    let dir = root.join("comparisons").join(history_id.to_string());
    checked_directory(&dir, create)?;
    Ok(dir)
}

fn exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(error(e)),
    }
}

fn plain(metadata: &Metadata) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 { return Err("拒绝复测目录中的 reparse point".into()); }
    }
    if metadata.file_type().is_symlink() { return Err("拒绝复测目录中的符号链接".into()); }
    Ok(())
}

pub(super) fn checked_directory(path: &Path, create: bool) -> Result<(), String> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) {
        return Err("复测目录必须是绝对路径且不能跳转".into());
    }
    let ancestors: Vec<_> = path.ancestors().collect();
    for ancestor in ancestors.into_iter().rev() {
        let metadata = match fs::symlink_metadata(ancestor) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                fs::create_dir(ancestor).map_err(error)?;
                fs::symlink_metadata(ancestor).map_err(error)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(error(e)),
        };
        plain(&metadata)?;
        if !metadata.is_dir() { return Err("复测路径包含非目录项".into()); }
    }
    Ok(())
}

fn inventory(dir: &Path, history_id: i64) -> Result<Vec<(PathBuf, Comparison)>, String> {
    checked_directory(dir, false)?;
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(error)? {
        let path = entry.map_err(error)?.path();
        let metadata = fs::symlink_metadata(&path).map_err(error)?;
        plain(&metadata)?;
        if !metadata.is_file() || path.extension().and_then(|s| s.to_str()) != Some("json") {
            return Err(format!("未确认的复测目录项：{}", path.display()));
        }
        let saved: Comparison = serde_json::from_slice(&fs::read(&path).map_err(error)?).map_err(error)?;
        valid_identity(&saved)?;
        if saved.history_id != history_id || path.file_stem().and_then(|s| s.to_str()) != Some(saved.id.as_str()) {
            return Err("复测文件与历史编号或文件名不一致".into());
        }
        files.push((path, saved));
    }
    Ok(files)
}

fn newest_first(files: &mut [(PathBuf, Comparison)]) {
    files.sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at).then_with(|| b.1.id.cmp(&a.1.id)));
}

fn reconcile(dir: &Path, history_id: i64, cycle: Option<&str>) -> Result<(), String> {
    let mut files = inventory(dir, history_id)?;
    newest_first(&mut files);
    let mut kept = 0;
    for (file, saved) in files {
        if cycle == Some(saved.cycle_id.as_str()) && kept < LIMIT { kept += 1; }
        else { remove_checked(&file)?; }
    }
    Ok(())
}

fn remove_checked(file: &Path) -> Result<(), String> {
    checked_directory(file.parent().ok_or("复测文件没有父目录")?, false)?;
    match fs::symlink_metadata(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(error(e)),
        Ok(metadata) => {
            plain(&metadata)?;
            if !metadata.is_file() { return Err("拒绝删除非普通复测文件".into()); }
        }
    }
    fs::remove_file(file).map_err(error)
}

#[cfg(test)]
mod tests;
