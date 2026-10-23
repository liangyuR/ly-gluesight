use serde_json::Value;
use std::{fs, path::Path};

fn remap(value: &mut Value, source: &Path, target: &Path) {
    let Some(text) = value.as_str() else {
        return;
    };
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    let source_text = source.to_string_lossy();
    let source = Path::new(source_text.strip_prefix(r"\\?\").unwrap_or(&source_text));
    let Ok(relative) = Path::new(text).strip_prefix(source) else {
        return;
    };
    if relative
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return;
    }
    *value = Value::String(target.join(relative).to_string_lossy().into_owned());
}

fn at(value: &mut Value, pointer: &str, apply: &mut impl FnMut(&mut Value)) {
    if let Some(value) = value.pointer_mut(pointer) {
        apply(value);
    }
}

fn frame(value: &mut Value, apply: &mut impl FnMut(&mut Value)) {
    at(value, "/image/calibTag/path", apply);
    if let Some(views) = value.get_mut("views").and_then(Value::as_array_mut) {
        for view in views {
            at(view, "/calibTag/path", apply);
        }
    }
    if let Some(states) = value.get_mut("viewStates").and_then(Value::as_array_mut) {
        for state in states {
            at(state, "/calibrationCheck/calibTag/path", apply);
        }
    }
    if let Some(backup) = value.get_mut("backup").filter(|v| v.is_object()) {
        frame(backup, apply);
    }
}

fn workspace(value: &mut Value, apply: &mut impl FnMut(&mut Value)) {
    if let Some(frames) = value.get_mut("frames").and_then(Value::as_array_mut) {
        for item in frames {
            frame(item, apply);
        }
    }
    if let Some(environment) = value
        .pointer_mut("/validation/environmentTag")
        .and_then(Value::as_array_mut)
    {
        for entry in environment {
            if entry.get("value").is_some() {
                at(entry, "/path", apply);
            }
        }
    }
    if let Some(pending) = value.get_mut("pending").filter(|v| v.is_object()) {
        workspace(pending, apply);
    }
}

fn known_document(relative: &Path) -> bool {
    let name = relative
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    let parts: Vec<_> = relative
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    matches!(
        parts.as_slice(),
        ["site-offline.json"] | ["config", "site-offline.json"]
    ) || (parts.first() == Some(&"workspaces")
        && (name == "workspace.json" || parts.get(2) == Some(&"published")))
        || (parts.first() == Some(&"recipe-capture") && name == "round.json")
        || parts.first() == Some(&"audit-spool")
}

fn fields(relative: &Path, value: &mut Value, apply: &mut impl FnMut(&mut Value)) {
    let parts: Vec<_> = relative
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    match parts.first().copied() {
        Some("workspaces") => workspace(value, apply),
        Some("recipe-capture") => {
            if let Some(frames) = value.get_mut("frames").and_then(Value::as_array_mut) {
                for shot in frames {
                    if let Some(cameras) = shot.get_mut("cameras").and_then(Value::as_array_mut) {
                        for camera in cameras {
                            at(camera, "/originalPath", apply);
                            if let Some(views) =
                                camera.get_mut("views").and_then(Value::as_array_mut)
                            {
                                for view in views {
                                    at(view, "/path", apply);
                                }
                            }
                        }
                    }
                }
            }
        }
        Some("audit-spool") => at(value, "/Recording/directory", apply),
        _ => at(value, "/sampleSetDir", apply),
    }
}

pub(super) fn json_file(
    path: &Path,
    relative: &Path,
    source: &Path,
    target: &Path,
) -> Result<(), String> {
    if !known_document(relative) {
        return Ok(());
    }
    let text = crate::fsio::read_text(path).map_err(|e| e.to_string())?;
    let mut value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let original = value.clone();
    fields(relative, &mut value, &mut |v| remap(v, source, target));
    if value != original {
        fs::write(
            path,
            serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) fn database(path: &Path, source: &Path, target: &Path) -> Result<(), String> {
    let mut conn = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if integrity != "ok" {
        return Err(format!("数据库完整性检查失败：{integrity}"));
    }
    let columns = {
        let mut statement = conn
            .prepare("PRAGMA table_info(parts)")
            .map_err(|e| e.to_string())?;
        let result = statement
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        result
    };
    if !columns.iter().any(|c| c == "recording_directory") {
        return Ok(());
    }
    let transaction = conn.transaction().map_err(|e| e.to_string())?;
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT rowid,recording_directory FROM parts WHERE recording_directory IS NOT NULL",
            )
            .map_err(|e| e.to_string())?;
        let result = statement
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        result
    };
    for (id, original) in rows {
        let mut value = Value::String(original.clone());
        remap(&mut value, source, target);
        if value.as_str() != Some(&original) {
            transaction
                .execute(
                    "UPDATE parts SET recording_directory=?1 WHERE rowid=?2",
                    rusqlite::params![value.as_str(), id],
                )
                .map_err(|e| e.to_string())?;
        }
    }
    transaction.commit().map_err(|e| e.to_string())
}

pub(super) fn tree_references(root: &Path, source: &Path, target: &Path) -> Result<(), String> {
    fn visit(root: &Path, directory: &Path, source: &Path, target: &Path) -> Result<(), String> {
        crate::image_file::checked_source_path(directory)?;
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            crate::image_file::checked_source_path(&path)?;
            if path.is_dir() {
                visit(root, &path, source, target)?;
            } else if path.extension().is_some_and(|e| e == "json") {
                json_file(
                    &path,
                    path.strip_prefix(root).map_err(|e| e.to_string())?,
                    source,
                    target,
                )?;
            } else if path.file_name().is_some_and(|n| n == "inspection.db") {
                database(&path, source, target)?;
            }
        }
        Ok(())
    }
    visit(root, root, source, target)
}

pub(super) fn validate_references(
    staging: &Path,
    source: &Path,
    target: &Path,
) -> Result<Vec<String>, String> {
    fn collect(root: &Path, dir: &Path, references: &mut Vec<String>) -> Result<(), String> {
        crate::image_file::checked_source_path(dir)?;
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            crate::image_file::checked_source_path(&path)?;
            if path.is_dir() {
                collect(root, &path, references)?;
                continue;
            }
            let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
            if path.extension().is_some_and(|e| e == "json") && known_document(relative) {
                if let Ok(mut value) = serde_json::from_str::<Value>(
                    &crate::fsio::read_text(&path).map_err(|e| e.to_string())?,
                ) {
                    fields(relative, &mut value, &mut |v| {
                        if let Some(text) = v.as_str() {
                            references.push(text.to_owned());
                        }
                    });
                }
            } else if path.file_name().is_some_and(|n| n == "inspection.db") {
                let conn = rusqlite::Connection::open_with_flags(
                    &path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .map_err(|e| e.to_string())?;
                let columns: Vec<String> = conn
                    .prepare("PRAGMA table_info(parts)")
                    .map_err(|e| e.to_string())?
                    .query_map([], |r| r.get(1))
                    .map_err(|e| e.to_string())?
                    .collect::<Result<_, _>>()
                    .map_err(|e| e.to_string())?;
                if columns.iter().any(|c| c == "recording_directory") {
                    let mut statement = conn.prepare("SELECT recording_directory FROM parts WHERE recording_directory IS NOT NULL").map_err(|e| e.to_string())?;
                    let paths = statement
                        .query_map([], |r| r.get::<_, String>(0))
                        .map_err(|e| e.to_string())?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| e.to_string())?;
                    references.extend(paths);
                }
            }
        }
        Ok(())
    }
    let mut references = Vec::new();
    collect(staging, staging, &mut references)?;
    references.sort();
    references.dedup();
    let mut warnings = Vec::new();
    for text in references {
        let path = Path::new(text.strip_prefix(r"\\?\").unwrap_or(&text));
        if let Ok(relative) = path.strip_prefix(target) {
            if relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err(format!("资料引用越界：{text}"));
            }
            let copied = staging.join(relative);
            if copied.exists() {
                crate::image_file::checked_source_path(&copied)?;
            } else if source.join(relative).exists() {
                return Err(format!("迁移后资料引用丢失：{text}"));
            } else {
                warnings.push(format!("原资料引用已缺失，保留原状态：{text}"));
            }
        } else if path.starts_with(source) {
            return Err(format!("迁移后仍引用旧资料目录：{text}"));
        }
    }
    Ok(warnings)
}
