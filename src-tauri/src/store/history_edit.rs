use std::path::Path;

use rusqlite::{params, OptionalExtension};

use super::{db_err, enum_column, RecordSource, Store};

pub struct DeletedRecording {
    pub project: String,
    pub source: RecordSource,
    pub directory: String,
    pub cycle_id: Option<String>,
}

fn selection(ids: &[i64]) -> Result<String, String> {
    if ids.is_empty() || ids.len() > 500 || ids.iter().any(|id| *id <= 0) {
        return Err("请选择 1–500 件有效历史记录".into());
    }
    let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
    if unique.len() != ids.len() {
        return Err("历史记录编号重复，请刷新后重新选择".into());
    }
    serde_json::to_string(ids).map_err(|e| e.to_string())
}

pub(super) fn normalize_tags(tags: &[String]) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for tag in tags
        .iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
    {
        if tag.chars().count() > 24 || tag.chars().any(char::is_control) {
            return Err("每个 Tag 最多 24 个字符，不能包含控制字符".into());
        }
        if !normalized.iter().any(|existing| existing == tag) {
            normalized.push(tag.to_string());
        }
    }
    if normalized.len() > 8 {
        return Err("每件历史记录最多 8 个 Tag".into());
    }
    Ok(normalized)
}

impl Store {
    pub fn delete_history_with_recordings(
        &self,
        data: &Path,
        ids: &[i64],
    ) -> Result<Vec<String>, String> {
        let recordings = self.delete_history(ids)?;
        let mut warnings = Vec::new();
        for recording in recordings {
            let removed = (|| {
                let directory = Path::new(&recording.directory);
                let path = match directory.canonicalize() {
                    Ok(path) => path,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(error) => return Err(error.to_string()),
                };
                crate::image_file::checked_source_path(directory)?;
                let root = crate::project::records::record_root(
                    data,
                    &recording.project,
                    recording.source,
                )?
                .canonicalize()
                .map_err(|error| format!("无法确认项目录制根：{error}"))?;
                if !path.starts_with(&root) || path == root {
                    return Err("原图目录不在该项目的录制根下".into());
                }
                let cycle = recording
                    .cycle_id
                    .as_deref()
                    .ok_or("记录缺少 cycleId，不能确认原图归属")?;
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("原图目录名称无效")?;
                if !crate::history_package::valid_directory(name, cycle) {
                    return Err("原图目录名与 cycleId 不一致".into());
                }
                if self.recording_referenced(&path)? {
                    return Err("原图目录仍被其他历史记录引用".into());
                }
                crate::project::records::remove_recording(
                    data,
                    &recording.project,
                    recording.source,
                    &path,
                )
            })();
            if let Err(error) = removed {
                warnings.push(format!(
                    "历史记录已删除，原图 {} 未清理完成：{error}",
                    recording.directory
                ));
            }
        }
        Ok(warnings)
    }

    pub fn set_history_tags(&self, ids: &[i64], tags: &[String]) -> Result<Vec<String>, String> {
        let selection = selection(ids)?;
        let normalized = normalize_tags(tags)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM parts WHERE id IN (SELECT value FROM json_each(?1))",
                [&selection],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if count != ids.len() as i64 {
            return Err("选中的历史记录已不存在，请刷新后重试".into());
        }
        tx.execute(
            "UPDATE parts SET tags = ?2 WHERE id IN (SELECT value FROM json_each(?1))",
            params![
                selection,
                serde_json::to_string(&normalized).map_err(|e| e.to_string())?
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(normalized)
    }

    pub fn delete_history(&self, ids: &[i64]) -> Result<Vec<DeletedRecording>, String> {
        let selection = selection(ids)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM parts WHERE id IN (SELECT value FROM json_each(?1))",
                [&selection],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if count != ids.len() as i64 {
            return Err("选中的历史记录已不存在，请刷新后重试".into());
        }
        let pending: Option<i64> = tx.query_row("SELECT id FROM parts WHERE id IN (SELECT value FROM json_each(?1)) AND (delivery_state IN ('pending','submitted','failed') OR recording_state = 'pending') LIMIT 1", [&selection], |row| row.get(0)).optional().map_err(db_err)?;
        if let Some(id) = pending {
            return Err(format!("记录 #{id} 的 PLC 交付或录制尚未完成，不能删除"));
        }
        let recordings = {
            let mut stmt = tx.prepare("SELECT project_id, source, recording_directory, cycle_id FROM parts WHERE id IN (SELECT value FROM json_each(?1)) AND recording_directory IS NOT NULL").map_err(db_err)?;
            let rows = stmt
                .query_map([&selection], |row| {
                    Ok(DeletedRecording {
                        project: row.get(0)?,
                        source: enum_column(row, 1)?,
                        directory: row.get(2)?,
                        cycle_id: row.get(3)?,
                    })
                })
                .map_err(db_err)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?
        };
        tx.execute("INSERT OR IGNORE INTO history_deletions (id, cycle_id) SELECT id, cycle_id FROM parts WHERE id IN (SELECT value FROM json_each(?1))", [&selection]).map_err(db_err)?;
        tx.execute("UPDATE parts SET retest_of = NULL WHERE retest_of IN (SELECT value FROM json_each(?1))", [&selection]).map_err(db_err)?;
        tx.execute(
            "DELETE FROM parts WHERE id IN (SELECT value FROM json_each(?1))",
            [&selection],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(recordings)
    }

    pub fn history_cycle_deleted(&self, cycle_id: &str) -> Result<bool, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM history_deletions WHERE cycle_id = ?1)",
                [cycle_id],
                |row| row.get(0),
            )
            .map_err(db_err)
    }

    pub fn recording_referenced(&self, directory: &Path) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT recording_directory FROM parts WHERE recording_directory IS NOT NULL")
            .map_err(db_err)?;
        for existing in stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_err)?
        {
            let existing = existing.map_err(db_err)?;
            let existing = match Path::new(&existing).canonicalize() {
                Ok(path) => path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("无法核对保留记录的原图目录：{error}")),
            };
            if existing.starts_with(directory) || directory.starts_with(&existing) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
