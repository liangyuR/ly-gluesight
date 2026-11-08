//! 历史件导出包的记录库部分：按件导出子集，按件并入本机记录库（docs/architecture/history-package.md）。

use std::collections::BTreeSet;
use std::path::Path;

use rusqlite::Connection;

use super::merge::{has_table, merge_rows, prepare_source, Merged};
use super::{
    db_err, recording_row, summary, PartSummary, RecordingEvidence, Store, DB_VERSION,
    SUMMARY_COLS, SUMMARY_LEN,
};

/// 导出前核对用的一件：摘要、录制证据、是否段配方记录。
pub struct PackageCandidate {
    pub summary: PartSummary,
    pub recording: RecordingEvidence,
    pub burst: bool,
}

impl Store {
    /// 按编号取要导出的记录（顺序同 `ids`）；有编号不存在时报错。
    pub fn package_candidates(&self, ids: &[i64]) -> Result<Vec<PackageCandidate>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SUMMARY_COLS}, recording_state, recording_available, recording_directory, recording_errors, \
                 burst_judgement IS NOT NULL FROM parts WHERE id = ?1"
            ))
            .map_err(db_err)?;
        ids.iter()
            .map(|id| {
                stmt.query_row([id], |row| {
                    Ok(PackageCandidate {
                        summary: summary(row)?,
                        recording: recording_row(row, SUMMARY_LEN)?,
                        burst: row.get(SUMMARY_LEN + 4)?,
                    })
                })
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => format!("记录 {id} 不存在"),
                    e => db_err(e),
                })
            })
            .collect()
    }

    /// 本机已有的 cycleId（`cycles` 的子集）。
    pub fn known_cycles(&self, cycles: &[String]) -> Result<BTreeSet<String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT cycle_id FROM parts WHERE cycle_id IN (SELECT value FROM json_each(?1))",
            )
            .map_err(db_err)?;
        let list = serde_json::to_string(cycles).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([list], |r| r.get(0)).map_err(db_err)?;
        rows.collect::<Result<_, _>>().map_err(db_err)
    }

    /// 把记录 `ids`（调用方保证同属一个项目）连同帧记录与所用配方快照写成独立记录库 `target`（10 版）；
    /// 指向子集外的复测引用置空。
    pub fn export_parts(&self, ids: &[i64], target: &Path) -> Result<(), String> {
        if target.exists() {
            return Err(format!("导出记录库 {} 已存在", target.display()));
        }
        drop(Store::open_inner(target, &self.project, false)?);
        let list = serde_json::to_string(ids).map_err(|e| e.to_string())?;
        let conn = self.conn.lock().unwrap();
        conn.execute("ATTACH DATABASE ?1 AS dst", [target.to_string_lossy()])
            .map_err(db_err)?;
        let result = (|| {
            conn.execute_batch("BEGIN;").map_err(db_err)?;
            let count = conn
                .execute(
                    "INSERT INTO dst.parts SELECT * FROM main.parts WHERE id IN (SELECT value FROM json_each(?1))",
                    [&list],
                )
                .map_err(db_err)?;
            if count != ids.len() {
                return Err("要导出的记录有的已不存在".into());
            }
            conn.execute(
                "INSERT INTO dst.part_frames SELECT * FROM main.part_frames WHERE part_id IN (SELECT value FROM json_each(?1))",
                [&list],
            )
            .map_err(db_err)?;
            conn.execute_batch(
                "INSERT INTO dst.recipe_snapshots SELECT s.* FROM main.recipe_snapshots s \
                     WHERE EXISTS (SELECT 1 FROM dst.parts p WHERE p.project_id = s.project_id AND p.recipe_revision = s.revision_id);
                 UPDATE dst.parts SET retest_of = NULL WHERE retest_of NOT IN (SELECT id FROM dst.parts);
                 COMMIT;",
            )
            .map_err(db_err)
        })();
        if result.is_err() {
            let _ = conn.execute_batch("ROLLBACK;");
        }
        conn.execute_batch("DETACH DATABASE dst;").map_err(db_err)?;
        result?;
        let out = Connection::open(target).map_err(db_err)?;
        let mode: String = out
            .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
            .map_err(db_err)?;
        if mode != "delete" {
            return Err("导出记录库无法退出 WAL 模式".into());
        }
        Ok(())
    }

    /// 把导出包里的记录库 `source`（10 版）并入项目 `project`，一个事务：本机已有同一 cycleId 的件跳过，其余重新编号；
    /// 配方快照同修订号内容不同则整批拒绝。`finalize` 在提交前收到新并入件的（cycleId, 本机编号）（装原图），失败则整批撤回。
    pub fn import_parts(
        &self,
        source: &Path,
        project: &str,
        finalize: impl FnOnce(&[(String, i64)]) -> Result<(), String>,
    ) -> Result<Merged, String> {
        if !crate::project::valid_id(project) {
            return Err(format!("项目编号 {project} 不合法"));
        }
        if prepare_source(source)? != DB_VERSION {
            return Err(format!("导出包的记录库版本不是 {DB_VERSION}"));
        }
        let conn = self.conn.lock().unwrap();
        conn.execute("ATTACH DATABASE ?1 AS src", [source.to_string_lossy()])
            .map_err(db_err)?;
        let result = (|| {
            if !has_table(&conn, "parts")? {
                return Err("导出包里的记录库没有检测记录".to_string());
            }
            conn.execute_batch("BEGIN IMMEDIATE;").map_err(db_err)?;
            let merged = merge_rows(&conn, project, false)?;
            let fresh: Vec<(String, i64)> = {
                let mut stmt = conn
                    .prepare("SELECT s.cycle_id, m.new FROM src.parts s JOIN temp.merge_ids m ON m.old = s.id WHERE m.fresh = 1 ORDER BY s.id")
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(db_err)?;
                rows.collect::<Result<_, _>>().map_err(db_err)?
            };
            conn.execute_batch("DROP TABLE temp.merge_ids;")
                .map_err(db_err)?;
            finalize(&fresh)?;
            conn.execute_batch("COMMIT;").map_err(db_err)?;
            Ok(merged)
        })();
        if result.is_err() {
            let _ = conn.execute_batch("ROLLBACK; DROP TABLE IF EXISTS temp.merge_ids;");
        }
        conn.execute_batch("DETACH DATABASE src;").map_err(db_err)?;
        result
    }

    /// 录制目录正是 `directory` 的记录是否存在（导入中断后核对原图目录归属）。
    pub fn has_recording(&self, directory: &str) -> Result<bool, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM parts WHERE recording_directory = ?1)",
                [directory],
                |r| r.get(0),
            )
            .map_err(db_err)
    }
}
