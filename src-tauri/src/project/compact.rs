use std::collections::{BTreeMap, BTreeSet};
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
            let relative = relative.strip_prefix("project/").ok_or("副本资源路径无效")?;
            let destination = target.join(relative);
            std::fs::create_dir_all(destination.parent().ok_or("副本路径无效")?).map_err(|e| e.to_string())?;
            std::fs::copy(source, destination).map_err(|e| e.to_string())?;
        }
        std::fs::remove_dir_all(&scratch).map_err(|e| e.to_string())?;
        deduplicate(target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(target);
    }
    result
}

pub(super) fn prepare(
    entries: Vec<(String, PathBuf)>,
    scratch: &Path,
) -> Result<Vec<(String, PathBuf)>, String> {
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let target = scratch.join("compact");
    std::fs::create_dir(&target).map_err(|e| format!("创建精简暂存目录失败：{e}"))?;
    let mut bundles = BTreeMap::new();
    for (relative, source) in &entries {
        let parts: Vec<_> = relative.split('/').collect();
        if parts.len() == 6 && parts[..3] == ["project", "vision", "releases"] && parts[5] == "manifest.json" {
            let prefix = parts[..5].join("/");
            bundles.insert(prefix, source.parent().ok_or("发布目录无效")?.to_path_buf());
        }
    }
    let mut result = Vec::new();
    for (prefix, source) in &bundles {
        let destination = target.join(prefix);
        let bundle = crate::release::compact_copy(source, &destination)?;
        result.push((format!("{prefix}/manifest.json"), destination.join("manifest.json")));
        for file in bundle.manifest.files {
            result.push((format!("{prefix}/{}", file.path), destination.join(file.path)));
        }
    }
    for (relative, source) in entries {
        if bundles.keys().any(|p| relative.starts_with(&format!("{p}/"))) {
            continue;
        }
        let compress = relative.ends_with(".pgm")
            && ["project/workspaces/", "project/recipe-capture/", "project/calib/"]
                .iter().any(|prefix| relative.starts_with(prefix));
        if compress {
            crate::image_file::checked_source_path(&source)?;
            let bytes = std::fs::read(&source).map_err(|e| e.to_string())?;
            let bytes = crate::image_file::compress_lossless(&bytes)?;
            let destination = target.join(&relative);
            std::fs::create_dir_all(destination.parent().ok_or("示教路径无效")?).map_err(|e| e.to_string())?;
            std::fs::write(&destination, bytes).map_err(|e| e.to_string())?;
            result.push((relative, destination));
        } else {
            result.push((relative, source));
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    let unique: BTreeSet<_> = result.iter().map(|e| &e.0).collect();
    if unique.len() != result.len() {
        return Err("精简资源路径重复".into());
    }
    Ok(result)
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
        if !relative.ends_with(".pgm") && !relative.ends_with(".pgm.gz") && !relative.ends_with(".pgm.zst") {
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
