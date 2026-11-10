use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub fn snapshot(root: &Path, target: &Path) -> Result<(), String> {
    crate::image_file::checked_source_path(root)?;
    std::fs::create_dir(target).map_err(|e| format!("副本目录必须不存在：{e}"))?;
    let scratch = target.join(".compact-work");
    std::fs::create_dir(&scratch).map_err(|e| e.to_string())?;
    let result = (|| {
        let entries = super::transfer::collect(root, None, &scratch, &mut Vec::new())?;
        for (relative, source) in entries {
            let relative = relative
                .strip_prefix("project/")
                .ok_or("副本资源路径无效")?;
            let destination = target.join(relative);
            std::fs::create_dir_all(destination.parent().ok_or("副本路径无效")?)
                .map_err(|e| e.to_string())?;
            super::transfer::copy_original(&source, &destination)?;
        }
        std::fs::remove_dir_all(&scratch).map_err(|e| e.to_string())?;
        deduplicate(target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(target);
    }
    result
}

pub(super) fn deduplicate(root: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    for top in ["workspaces", "vision", "recipe-capture", "calib"] {
        let path = root.join(top);
        if path.is_dir() {
            super::transfer::walk(&path, top, &mut files, &mut Vec::new())?;
        }
    }
    let mut seen = BTreeMap::<String, PathBuf>::new();
    for (relative, path) in files {
        if !relative.ends_with(".pgm") {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let key = format!("{:x}", Sha256::digest(&bytes));
        if let Some(first) = seen.get(&key) {
            let temporary = path.with_extension("dedup-tmp");
            if std::fs::hard_link(first, &temporary).is_err() {
                continue;
            }
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            std::fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
        } else {
            seen.insert(key, path);
        }
    }
    Ok(())
}
