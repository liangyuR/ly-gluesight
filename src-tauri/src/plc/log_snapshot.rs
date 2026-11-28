use std::io;
use std::path::Path;

use ly_plc::LogEntry;
use rusqlite::{params, Connection, OpenFlags, Row};

pub(super) const LOG_FIELDS: &str =
    "ts, level, category, point_id, point_name, message, old_value, new_value, context";

pub(super) fn open(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
}

pub(super) fn row_to_entry(row: &Row<'_>) -> rusqlite::Result<LogEntry> {
    Ok(LogEntry {
        id: row.get(0)?,
        ts: row.get(1)?,
        level: row.get(2)?,
        category: row.get(3)?,
        point_id: row.get(4)?,
        point_name: row.get(5)?,
        message: row.get(6)?,
        old_value: row.get(7)?,
        new_value: row.get(8)?,
        context: row.get(9)?,
    })
}

#[derive(Debug)]
pub(crate) enum ScanError {
    Read(rusqlite::Error),
    Write(io::Error),
}

pub(crate) fn visit_recent(
    path: &Path,
    from: i64,
    to: i64,
    limit: u32,
    mut visit: impl FnMut(LogEntry) -> io::Result<()>,
) -> Result<bool, ScanError> {
    if limit == 0 {
        return Ok(false);
    }
    let mut conn = open(path).map_err(ScanError::Read)?;
    let snapshot = conn.transaction().map_err(ScanError::Read)?;
    let (oldest, truncated) = {
        let mut stmt = snapshot
            .prepare(
                "SELECT ts, id FROM plc_log WHERE ts BETWEEN ?1 AND ?2
                 ORDER BY ts DESC, id DESC LIMIT 2 OFFSET ?3",
            )
            .map_err(ScanError::Read)?;
        let mut rows = stmt
            .query_map(params![from, to, limit - 1], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(ScanError::Read)?;
        let oldest = rows.next().transpose().map_err(ScanError::Read)?;
        let truncated = rows.next().transpose().map_err(ScanError::Read)?.is_some();
        (oldest.unwrap_or((from, i64::MIN)), truncated)
    };
    {
        let mut stmt = snapshot
            .prepare(&format!(
                "SELECT id, {LOG_FIELDS} FROM plc_log
                 WHERE (ts, id) >= (?1, ?2) AND ts <= ?3 ORDER BY ts, id"
            ))
            .map_err(ScanError::Read)?;
        let rows = stmt
            .query_map(params![oldest.0, oldest.1, to], row_to_entry)
            .map_err(ScanError::Read)?;
        for row in rows {
            visit(row.map_err(ScanError::Read)?).map_err(ScanError::Write)?;
        }
    }
    snapshot.commit().map_err(ScanError::Read)?;
    Ok(truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offline::test_dir::TestDir;

    #[test]
    fn recent_logs_keep_the_window_and_tie_order_in_a_single_snapshot() {
        let dir = TestDir::new("plc-log-tests", "snapshot");
        let path = dir.join("plc.db");
        let _book = ly_plc::Logbook::open(&path, std::sync::Arc::new(|_| {}), 0).unwrap();
        let writer = Connection::open(&path).unwrap();
        let insert = |ts| {
            writer
                .execute(
                    "INSERT INTO plc_log(ts,level,category,point_id,point_name,message,old_value,new_value,context)
                     VALUES(?1,'vendor-level','vendor-category','point','点位','信号','0','1','partSeq=4')",
                    [ts],
                )
                .unwrap();
        };
        for ts in [30, 10, 20, 30, 31, 30, 9] {
            insert(ts);
        }

        let mut exact = Vec::new();
        assert!(!visit_recent(&path, 20, 30, 4, |entry| {
            exact.push((entry.ts, entry.id));
            Ok(())
        })
        .unwrap());
        assert_eq!(exact, [(20, 3), (30, 1), (30, 4), (30, 6)]);

        let mut capped = Vec::new();
        assert!(visit_recent(&path, 10, 30, 2, |entry| {
            if capped.is_empty() {
                insert(30);
                assert_eq!(entry.level, "vendor-level");
                assert_eq!(entry.category, "vendor-category");
                assert_eq!(entry.context.as_deref(), Some("partSeq=4"));
            }
            capped.push((entry.ts, entry.id));
            Ok(())
        })
        .unwrap());
        assert_eq!(capped, [(30, 4), (30, 6)]);

        let mut later = Vec::new();
        assert!(visit_recent(&path, 10, 30, 2, |entry| {
            later.push(entry.id);
            Ok(())
        })
        .unwrap());
        assert_eq!(later, [6, 8]);
    }
}
