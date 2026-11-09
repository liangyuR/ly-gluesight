//! 项目记录库并入本机记录库，以及按项目导出子集。
//!
//! 来源可以是项目旧库（9 版，每个项目一份）或本机库按项目导出的子集（10 版）。记录按 cycleId 核对：本机已有同一
//! cycleId 的件（同一台机器复制、再导入的同一批件）不再插入，映射到已有记录；其余记录重新编号（`keep_ids` 时尽量沿用
//! 原编号），帧记录跟随，`retest_of` 随之重映射。配方快照与候选实例按项目区分，原样带入。

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use super::{add_part_mode, add_part_tags, db_err, Store, DB_VERSION, PROJECT_DB_VERSION};

/// 两份配方快照按当前格式读入后相同：旧快照里已退役的字段（如 `planVersion`）不算差异。
fn same_snapshot(a: &str, b: &str) -> bool {
    let normalize = |json: &str| {
        serde_json::from_str(json)
            .ok()
            .and_then(|value| crate::recipe::Recipe::from_snapshot(value).ok())
            .and_then(|recipe| serde_json::to_value(recipe).ok())
    };
    matches!((normalize(a), normalize(b)), (Some(a), Some(b)) if a == b)
}

/// parts 中除 id、project_id、retest_of 外的列；9 版与 10 版相同。
const PART_COLUMNS: &str = "ts, sn, recipe_id, recipe_version, recipe_revision, trigger_mode, verdict, plc_code, fault_code, \
    reason, drain_ms, frames_expected, frames_received, triggers, software_version, judgement, frames, cycle_id, bundle_id, \
    delivery_state, delivery_updated_at, delivery_message, recording_state, recording_available, recording_directory, \
    recording_errors, sample, source, expected_total, tolerance_total, device_triggers, camera_frames, flags, burst_judgement, mode, tags";

/// 一次合并的结果。`ids` 是来源记录编号 → 本机记录编号（含按 cycleId 映射到已有记录的）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    pub ids: BTreeMap<i64, i64>,
    pub inserted: usize,
    pub duplicates: usize,
}

/// 打开来源库做合并前的准备：核对版本，9 版补齐旧格式升级（调试来源、维护模式列），合并 WAL。
pub(super) fn prepare_source(path: &Path) -> Result<i64, String> {
    let conn = Connection::open(path).map_err(db_err)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(db_err)?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db_err)?;
    if version != PROJECT_DB_VERSION && version != 10 && version != DB_VERSION {
        return Err(format!(
            "记录库版本 {version} 不能合并（支持 {PROJECT_DB_VERSION}、{DB_VERSION}）"
        ));
    }
    if version == PROJECT_DB_VERSION {
        add_part_mode(&conn)?;
    }
    add_part_tags(&conn)?;
    let mut tags = conn.prepare("SELECT tags FROM parts").map_err(db_err)?;
    for row in tags
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_err)?
    {
        let tags: Vec<String> = serde_json::from_str(&row.map_err(db_err)?)
            .map_err(|e| format!("来源记录的 Tag 格式无效：{e}"))?;
        if super::history_edit::normalize_tags(&tags)? != tags {
            return Err("来源记录的 Tag 未规范化，拒绝导入".into());
        }
    }
    drop(tags);
    let (busy, log, done): (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .map_err(db_err)?;
    if busy != 0 || (log >= 0 && done < log) {
        return Err("来源记录库仍在使用，WAL 未完整合并".into());
    }
    conn.close().map_err(|(_, e)| db_err(e))?;
    Ok(version)
}

pub(super) fn has_table(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM src.sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )
    .map_err(db_err)
}

impl Store {
    /// 项目的旧记录是否已经并入。
    pub fn merged(&self, project: &str) -> Result<bool, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM project_merges WHERE project_id = ?1)",
                [project],
                |r| r.get(0),
            )
            .map_err(db_err)
    }

    /// 把 `source` 的记录作为项目 `project` 的记录并入本机库（一个事务；同一项目只能合并一次）。
    /// `installing` 为导入中的新项目：项目目录装好后调 `finish_install`；之前中断的由 `pending_installs` 在启动时找出处理。
    pub fn merge_project(
        &self,
        source: &Path,
        project: &str,
        keep_ids: bool,
        installing: bool,
    ) -> Result<Merged, String> {
        if !crate::project::valid_id(project) {
            return Err(format!("项目编号 {project} 不合法"));
        }
        prepare_source(source)?;
        let conn = self.conn.lock().unwrap();
        conn.execute("ATTACH DATABASE ?1 AS src", [source.to_string_lossy()])
            .map_err(db_err)?;
        let result = merge_attached(&conn, project, keep_ids, installing);
        if result.is_err() {
            let _ = conn.execute_batch("ROLLBACK;");
        }
        let _ = conn.execute_batch("DROP TABLE IF EXISTS temp.merge_ids;");
        conn.execute_batch("DETACH DATABASE src;").map_err(db_err)?;
        result
    }

    /// 并入后还没改写完项目文件里记录引用的编号对照（来源编号 → 本机编号）；没合并过或已改写完为 None。
    pub fn pending_refs(&self, project: &str) -> Result<Option<BTreeMap<i64, i64>>, String> {
        let conn = self.conn.lock().unwrap();
        let done: Option<bool> = conn
            .query_row(
                "SELECT refs_done FROM project_merges WHERE project_id = ?1",
                [project],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        if done != Some(false) {
            return Ok(None);
        }
        let mut stmt = conn
            .prepare("SELECT old, new FROM project_merge_ids WHERE project_id = ?1")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([project], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(db_err)?;
        rows.collect::<Result<_, _>>().map_err(db_err).map(Some)
    }

    /// 记下项目文件里的记录引用已改写完，编号对照不再需要。
    pub fn finish_refs(&self, project: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        tx.execute(
            "UPDATE project_merges SET refs_done = 1 WHERE project_id = ?1",
            [project],
        )
        .map_err(db_err)?;
        tx.execute(
            "DELETE FROM project_merge_ids WHERE project_id = ?1",
            [project],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    /// 导入的项目目录已装好：它并入的记录从此算数。
    pub fn finish_install(&self, project: &str) -> Result<(), String> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE project_merges SET installed = 1 WHERE project_id = ?1",
                [project],
            )
            .map(|_| ())
            .map_err(db_err)
    }

    /// 记录已并入、项目目录还没装好的导入（上次导入中断）。
    pub fn pending_installs(&self) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT project_id FROM project_merges WHERE installed = 0 ORDER BY project_id",
            )
            .map_err(db_err)?;
        let rows = stmt.query_map([], |r| r.get(0)).map_err(db_err)?;
        rows.collect::<Result<_, _>>().map_err(db_err)
    }

    /// 撤回一个项目的全部记录（导入在合并后失败时用；项目是新建的，记录只来自这次合并）。
    pub fn forget_project(&self, project: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        for sql in [
            "DELETE FROM parts WHERE project_id = ?1",
            "DELETE FROM recipe_snapshots WHERE project_id = ?1",
            "DELETE FROM workspace_instances WHERE project_id = ?1",
            "DELETE FROM project_merges WHERE project_id = ?1",
            "DELETE FROM project_merge_ids WHERE project_id = ?1",
        ] {
            tx.execute(sql, [project]).map_err(db_err)?;
        }
        tx.commit().map_err(db_err)
    }

    /// 把项目 `project` 的记录、配方快照与候选实例写成独立的记录库 `target`（10 版，导出项目用）。
    pub fn export_project(&self, project: &str, target: &Path) -> Result<usize, String> {
        if target.exists() {
            return Err(format!("导出记录库 {} 已存在", target.display()));
        }
        drop(Store::open_inner(target, project, false)?);
        let conn = self.conn.lock().unwrap();
        conn.execute("ATTACH DATABASE ?1 AS dst", [target.to_string_lossy()])
            .map_err(db_err)?;
        let result = (|| {
            conn.execute_batch("BEGIN;").map_err(db_err)?;
            let count = conn
                .execute(
                    "INSERT INTO dst.parts SELECT * FROM main.parts WHERE project_id = ?1",
                    [project],
                )
                .map_err(db_err)?;
            for sql in [
                "INSERT INTO dst.part_frames SELECT f.* FROM main.part_frames f JOIN main.parts p ON p.id = f.part_id WHERE p.project_id = ?1",
                "INSERT INTO dst.part_points SELECT f.* FROM main.part_points f JOIN main.parts p ON p.id = f.part_id WHERE p.project_id = ?1",
                "INSERT INTO dst.part_shots SELECT f.* FROM main.part_shots f JOIN main.parts p ON p.id = f.part_id WHERE p.project_id = ?1",
                "INSERT INTO dst.recipe_snapshots SELECT * FROM main.recipe_snapshots WHERE project_id = ?1",
                "INSERT INTO dst.workspace_instances SELECT * FROM main.workspace_instances WHERE project_id = ?1",
            ] {
                conn.execute(sql, [project]).map_err(db_err)?;
            }
            // 导出子集里的跨项目复测引用没有目标，置空
            conn.execute("UPDATE dst.parts SET retest_of = NULL WHERE retest_of NOT IN (SELECT id FROM dst.parts)", []).map_err(db_err)?;
            conn.execute_batch("COMMIT;").map_err(db_err)?;
            Ok::<usize, String>(count)
        })();
        if result.is_err() {
            let _ = conn.execute_batch("ROLLBACK;");
        }
        conn.execute_batch("DETACH DATABASE dst;").map_err(db_err)?;
        let count = result?;
        let out = Connection::open(target).map_err(db_err)?;
        let mode: String = out
            .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
            .map_err(db_err)?;
        if mode != "delete" {
            return Err("导出记录库无法退出 WAL 模式".into());
        }
        Ok(count)
    }
}

fn merge_attached(
    conn: &Connection,
    project: &str,
    keep_ids: bool,
    installing: bool,
) -> Result<Merged, String> {
    if !has_table(conn, "parts")? {
        return Err("来源不是检测记录库".into());
    }
    let already: bool = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM project_merges WHERE project_id = ?1)",
            [project],
            |r| r.get(0),
        )
        .map_err(db_err)?;
    if already {
        return Err(format!("项目 {project} 的记录已经合并过"));
    }
    conn.execute_batch("BEGIN IMMEDIATE;").map_err(db_err)?;
    let merged = merge_rows(conn, project, keep_ids)?;
    conn.execute(
        "INSERT INTO project_merges (project_id, merged_at, inserted, duplicates, installed) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![project, ly_plc::now_ms(), merged.inserted as i64, merged.duplicates as i64, !installing],
    ).map_err(db_err)?;
    conn.execute(
        "INSERT INTO project_merge_ids (project_id, old, new) SELECT ?1, old, new FROM temp.merge_ids",
        [project],
    )
    .map_err(db_err)?;
    conn.execute_batch("DROP TABLE temp.merge_ids; COMMIT;")
        .map_err(db_err)?;
    Ok(merged)
}

/// 在调用方已开的事务里把附加库 `src` 的记录并入项目 `project`：按 cycleId 去重、分配本机编号（编号对照留在
/// `temp.merge_ids`，调用方用完删除），带入配方快照（同修订号内容不同则拒绝）、候选实例与帧记录。
pub(super) fn merge_rows(
    conn: &Connection,
    project: &str,
    keep_ids: bool,
) -> Result<Merged, String> {
    let sources: Vec<(i64, Option<String>)> = {
        let mut stmt = conn
            .prepare("SELECT id, cycle_id FROM src.parts WHERE source = 'real' ORDER BY id")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(db_err)?;
        rows.collect::<Result<_, _>>().map_err(db_err)?
    };
    let main_max: i64 = conn
        .query_row("SELECT MAX(COALESCE((SELECT MAX(id) FROM main.parts), 0), COALESCE((SELECT MAX(id) FROM main.history_deletions), 0))", [], |r| {
            r.get(0)
        })
        .map_err(db_err)?;
    let source_max = sources.last().map_or(0, |(id, _)| *id);
    let mut next = if keep_ids {
        main_max.max(source_max) + 1
    } else {
        main_max + 1
    };
    let mut assigned = HashSet::new();
    let mut merged = Merged::default();
    conn.execute_batch("CREATE TEMP TABLE merge_ids (old INTEGER PRIMARY KEY, new INTEGER NOT NULL, fresh INTEGER NOT NULL);")
        .map_err(db_err)?;
    {
        let mut existing_cycle = conn
            .prepare("SELECT id FROM main.parts WHERE cycle_id = ?1")
            .map_err(db_err)?;
        let mut existing_id = conn
            .prepare("SELECT EXISTS (SELECT 1 FROM main.parts WHERE id = ?1 UNION ALL SELECT 1 FROM main.history_deletions WHERE id = ?1)")
            .map_err(db_err)?;
        let mut record = conn
            .prepare("INSERT INTO temp.merge_ids (old, new, fresh) VALUES (?1, ?2, ?3)")
            .map_err(db_err)?;
        for (old, cycle) in &sources {
            let duplicate = match cycle {
                Some(cycle) => existing_cycle
                    .query_row([cycle], |r| r.get::<_, i64>(0))
                    .optional()
                    .map_err(db_err)?,
                None => None,
            };
            let (new, fresh) = match duplicate {
                Some(existing) => {
                    merged.duplicates += 1;
                    (existing, false)
                }
                None => {
                    let keep = keep_ids
                        && !assigned.contains(old)
                        && !existing_id
                            .query_row([old], |r| r.get::<_, bool>(0))
                            .map_err(db_err)?;
                    let new = if keep {
                        *old
                    } else {
                        next += 1;
                        next - 1
                    };
                    assigned.insert(new);
                    merged.inserted += 1;
                    (new, true)
                }
            };
            record.execute(params![old, new, fresh]).map_err(db_err)?;
            merged.ids.insert(*old, new);
        }
    }
    conn.execute(
        "INSERT INTO recipe_snapshots (project_id, revision_id, recipe_id, version, json) \
         SELECT ?1, revision_id, recipe_id, version, json FROM src.recipe_snapshots s WHERE NOT EXISTS (SELECT 1 FROM src.parts p WHERE p.source != 'real' AND p.recipe_revision = s.revision_id) OR EXISTS (SELECT 1 FROM src.parts p WHERE p.source = 'real' AND p.recipe_revision = s.revision_id) \
         ON CONFLICT (project_id, revision_id) DO NOTHING",
        [project],
    )
    .map_err(db_err)?;
    let differing: Vec<(String, bool, String, String)> = conn
        .prepare(
            "SELECT s.revision_id, m.recipe_id = s.recipe_id AND m.version = s.version, m.json, s.json \
             FROM src.recipe_snapshots s JOIN main.recipe_snapshots m ON m.project_id = ?1 AND m.revision_id = s.revision_id \
             WHERE (m.json != s.json OR m.recipe_id != s.recipe_id OR m.version != s.version) AND (NOT EXISTS (SELECT 1 FROM src.parts p WHERE p.source != 'real' AND p.recipe_revision = s.revision_id) OR EXISTS (SELECT 1 FROM src.parts p WHERE p.source = 'real' AND p.recipe_revision = s.revision_id))",
        )
        .and_then(|mut s| s.query_map([project], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect())
        .map_err(db_err)?;
    let conflict = differing
        .into_iter()
        .find(|(_, same_identity, ours, theirs)| !same_identity || !same_snapshot(ours, theirs))
        .map(|(revision, ..)| revision);
    if let Some(revision) = conflict {
        return Err(format!(
            "项目 {project} 的配方快照 {revision} 与本机已有的不同，拒绝合并"
        ));
    }
    conn.execute(
        "INSERT INTO workspace_instances (project_id, id, recipe_id) SELECT ?1, id, recipe_id FROM src.workspace_instances WHERE true \
         ON CONFLICT (project_id, id) DO NOTHING",
        [project],
    ).map_err(db_err)?;
    let conflict: Option<i64> = conn.query_row(
        "SELECT s.id FROM src.workspace_instances s JOIN main.workspace_instances m ON m.project_id = ?1 AND m.id = s.id \
         WHERE m.recipe_id != s.recipe_id LIMIT 1",
        [project], |r| r.get(0),
    ).optional().map_err(db_err)?;
    if let Some(id) = conflict {
        return Err(format!(
            "项目 {project} 的候选实例 {id} 已属于其他配方，拒绝合并"
        ));
    }
    if has_table(conn, "cycle_ids")? {
        conn.execute_batch("INSERT OR IGNORE INTO cycle_ids (id, created_at) SELECT id, created_at FROM src.cycle_ids;").map_err(db_err)?;
    }
    let source_columns = PART_COLUMNS
        .split(", ")
        .map(|c| format!("s.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    conn.execute(
        &format!(
            "INSERT INTO main.parts (id, project_id, retest_of, {PART_COLUMNS}) \
             SELECT m.new, ?1, (SELECT r.new FROM temp.merge_ids r WHERE r.old = s.retest_of), {source_columns} \
             FROM src.parts s JOIN temp.merge_ids m ON m.old = s.id WHERE m.fresh = 1 ORDER BY s.id"
        ),
        [project],
    ).map_err(db_err)?;
    for (table, columns) in [
        ("part_frames", "camera, seq, status, frame_counter, trigger_counter, region, metrics, raw_file"),
        ("part_points", "format, data"),
        ("part_shots", "k, shot_id, camera, view, session, ordinal, frame_counter, trigger_counter, status, error, score, ms, raw_files"),
    ] {
        if !has_table(conn, table)? {
            continue;
        }
        let selected = columns.split(", ").map(|c| format!("f.{c}")).collect::<Vec<_>>().join(", ");
        conn.execute_batch(&format!(
            "INSERT INTO main.{table} (part_id, {columns}) SELECT m.new, {selected} \
             FROM src.{table} f JOIN temp.merge_ids m ON m.old = f.part_id WHERE m.fresh = 1;"
        )).map_err(db_err)?;
    }
    Ok(merged)
}
