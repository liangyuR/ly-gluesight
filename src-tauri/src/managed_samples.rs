use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::offline::sample_set::SampleSet;

fn inventory(root: &Path, dir: &Path, files: &mut Vec<(PathBuf, u64)>) -> Result<(), String> {
    crate::image_file::checked_source_path(dir)?;
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        crate::image_file::checked_source_path(&path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.is_dir() {
            inventory(root, &path, files)?;
        } else if metadata.is_file() {
            files.push((path.strip_prefix(root).map_err(|e| e.to_string())?.to_path_buf(), metadata.len()));
        } else {
            return Err(format!("样本集包含非普通文件：{}", path.display()));
        }
    }
    Ok(())
}

fn complete_set(path: &Path) -> Result<SampleSet, String> {
    let set = SampleSet::load(path)?;
    if !set.missing().is_empty() {
        return Err(format!("样本集有 {} 幅缺失或无效图片，未导入：{}", set.missing().len(), set.missing().iter().take(3).cloned().collect::<Vec<_>>().join("、")));
    }
    Ok(set)
}

fn find_id(samples: &Path, id: &str) -> Result<Option<PathBuf>, String> {
    if !samples.exists() { return Ok(None); }
    crate::image_file::checked_source_path(samples)?;
    let mut found = None;
    for entry in fs::read_dir(samples).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        crate::image_file::checked_source_path(&path)?;
        if !path.is_dir() || path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) { continue; }
        let existing = SampleSet::load(&path).map_err(|e| format!("受管样本清单无法检查：{}：{e}", path.display()))?;
        if existing.manifest().id == id {
            if found.is_some() { return Err(format!("受管样本集 ID {id} 重复，无法确定原图来源")); }
            found = Some(path.canonicalize().map_err(|e| e.to_string())?);
        }
    }
    Ok(found)
}

pub fn load_by_id(root: &Path, id: &str) -> Result<SampleSet, String> {
    let path = find_id(&root.join("samples"), id)?.ok_or_else(|| format!("受管样本集 {id} 不存在，离线样本原图不可用"))?;
    SampleSet::load(&path)
}

pub fn existing_copy(root: &Path, source: &Path, id: &str) -> Result<Option<PathBuf>, String> {
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    Ok(find_id(&root.join("samples"), id)?.filter(|path| *path != source))
}

pub fn import(root: &Path, source: &Path) -> Result<PathBuf, String> {
    crate::image_file::checked_source_path(source)?;
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    let original = complete_set(&source)?;
    crate::image_file::checked_source_path(root)?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let samples = root.join("samples");
    if samples.starts_with(&source) {
        return Err("样本源不能包含受管样本目录，请选择外部样本集或已导入的具体样本目录".into());
    }
    if samples.exists() { crate::image_file::checked_source_path(&samples)?; }
    fs::create_dir_all(&samples).map_err(|e| format!("无法创建受管样本目录：{e}"))?;
    crate::image_file::checked_source_path(&samples)?;
    let samples = samples.canonicalize().map_err(|e| e.to_string())?;
    let lock_path = samples.join(".import.lock");
    if lock_path.exists() { crate::image_file::checked_source_path(&lock_path)?; }
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&lock_path).map_err(|e| e.to_string())?;
    lock.try_lock().map_err(|_| "另一个样本集正在导入，请稍后再试".to_string())?;
    if let Some(path) = find_id(&samples, &original.manifest().id)? {
        if path == source { return Ok(path); }
        return Err(format!("样本集 ID {} 已导入，不能覆盖。请使用已有受管目录：{}", original.manifest().id, path.display()));
    }
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let token = format!("{}-{}-{}", std::process::id(), ly_plc::now_ms(), SEQUENCE.fetch_add(1, Ordering::Relaxed));
    let stage = samples.join(format!(".import-{token}"));
    let destination = samples.join(format!("set-{token}"));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut files = Vec::new();
        inventory(&source, &source, &mut files)?;
        for (relative, expected_length) in &files {
            let from = source.join(relative);
            crate::image_file::checked_source_path(&from)?;
            let to = stage.join(relative);
            fs::create_dir_all(to.parent().ok_or("无效样本路径")?).map_err(|e| e.to_string())?;
            let copied = fs::copy(&from, &to).map_err(|e| format!("复制 {} 失败：{e}", from.display()))?;
            if copied != *expected_length { return Err(format!("导入时源文件长度变化：{}", from.display())); }
        }
        let copied = complete_set(&stage)?;
        if serde_json::to_value(copied.manifest()).map_err(|e| e.to_string())? != serde_json::to_value(original.manifest()).map_err(|e| e.to_string())? {
            return Err("导入时样本清单发生变化，请重新导入".into());
        }
        let mut actual = Vec::new();
        inventory(&stage, &stage, &mut actual)?;
        files.sort();
        actual.sort();
        if files != actual { return Err("导入样本的文件清单或长度不一致".into()); }
        fs::rename(&stage, &destination).map_err(|e| format!("启用受管样本失败：{e}"))?;
        Ok(destination)
    })();
    if result.is_err() && stage.parent() == Some(samples.as_path()) {
        if crate::image_file::checked_source_path(&stage).is_ok() {
            let _ = fs::remove_dir_all(&stage);
        }
    }
    result
}
