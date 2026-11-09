use super::{db_err, Store};
use rusqlite::{params, Connection};
use std::path::Path;

pub(super) fn migrate(conn: &mut Connection) -> Result<(), String> {
    conn.execute_batch("BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS retired_recordings (
            project_id TEXT NOT NULL, directory TEXT PRIMARY KEY, cycle_id TEXT
        );
        INSERT OR IGNORE INTO retired_recordings
            SELECT project_id, recording_directory, cycle_id FROM parts
            WHERE source != 'real' AND recording_directory IS NOT NULL;
        INSERT OR IGNORE INTO history_deletions (id, cycle_id)
            SELECT id, cycle_id FROM parts WHERE source != 'real';
        UPDATE parts SET retest_of = NULL WHERE retest_of IN (SELECT id FROM parts WHERE source != 'real');
        CREATE TEMP TABLE retired_revisions AS SELECT DISTINCT project_id, recipe_revision FROM parts WHERE source != 'real';
        DELETE FROM parts WHERE source != 'real';
        DELETE FROM recipe_snapshots WHERE EXISTS (SELECT 1 FROM retired_revisions r WHERE r.project_id = recipe_snapshots.project_id AND r.recipe_revision = recipe_snapshots.revision_id)
            AND NOT EXISTS (SELECT 1 FROM parts WHERE parts.project_id = recipe_snapshots.project_id AND parts.recipe_revision = recipe_snapshots.revision_id);
        DROP TABLE retired_revisions;
        CREATE TRIGGER IF NOT EXISTS production_source_insert BEFORE INSERT ON parts
            WHEN NEW.source != 'real' BEGIN SELECT RAISE(ABORT, 'offline records are retired'); END;
        CREATE TRIGGER IF NOT EXISTS production_source_update BEFORE UPDATE OF source ON parts
            WHEN NEW.source != 'real' BEGIN SELECT RAISE(ABORT, 'offline records are retired'); END;
        PRAGMA user_version = 11;
        COMMIT;").map_err(|error| {
            let _ = conn.execute_batch("ROLLBACK;");
            format!("删除停用离线记录失败：{error}")
        })
}

impl Store {
    pub fn cleanup_retired_recordings(&self, data: &Path) -> Vec<String> {
        let entries = (|| {
            let conn = self.conn.lock().unwrap();
            let mut query = conn
                .prepare("SELECT project_id, directory, cycle_id FROM retired_recordings")
                .map_err(db_err)?;
            let rows = query
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(db_err)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
        })();
        let entries = match entries {
            Ok(entries) => entries,
            Err(error) => return vec![error],
        };
        let mut warnings = Vec::new();
        for (project, directory, cycle) in entries {
            let removed = (|| {
                if !crate::project::valid_id(&project) {
                    return Err("项目编号非法".to_string());
                }
                let path = Path::new(&directory);
                if !path.exists() {
                    return Ok(());
                }
                crate::image_file::checked_source_path(path)?;
                let path = path.canonicalize().map_err(|e| e.to_string())?;
                if !crate::log_storage::recording_projects(&crate::project::dir(data, &project))?
                    .iter()
                    .filter_map(|base| base.join("records-offline").canonicalize().ok())
                    .any(|root| path != root && path.starts_with(&root))
                {
                    return Err("原图目录不在该项目已停用的离线录制根下".into());
                }
                let cycle = cycle.as_deref().ok_or("原图记录没有 cycleId")?;
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or("目录名称无效")?;
                if !crate::history_package::valid_directory(name, cycle) {
                    return Err("原图目录与 cycleId 不一致".into());
                }
                if self.recording_referenced(&path)? {
                    return Err("原图仍被生产记录引用".into());
                }
                std::fs::remove_dir_all(path).map_err(|e| e.to_string())
            })();
            match removed {
                Ok(()) => {
                    if let Err(error) = self.conn.lock().unwrap().execute(
                        "DELETE FROM retired_recordings WHERE directory = ?1",
                        params![directory],
                    ) {
                        warnings.push(error.to_string());
                    }
                }
                Err(error) => warnings.push(format!(
                    "旧离线记录已删除，原图 {directory} 清理待重试：{error}"
                )),
            }
        }
        warnings
    }
}
