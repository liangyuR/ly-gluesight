use std::path::Path;
use std::sync::Mutex;

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::cycle::FrameView;
use crate::judge::{Judgement, PointState, Verdict};
use crate::recipe::Recipe;

/// 一件工件的完整结果，入库一行；测量点按弧长顺序压成一个 BLOB，格式见 POINTS_FORMAT。
pub struct PartRecord<'a> {
    pub ts: i64,
    pub sn: u32,
    pub recipe: Option<&'a Recipe>,
    pub judgement: &'a Judgement,
    pub drain_ms: Option<u64>,
    pub frames: &'a [FrameView],
    pub frames_expected: usize,
    pub frames_received: usize,
    pub triggers: u64,
    pub table: Option<&'a [PointState]>,
    pub software_version: &'a str,
}

/// 测量点 BLOB 格式版本。1：每点 f32 位置 + u8 状态（5 字节）；2：f32 位置 + f32 胶宽 + u8 状态（9 字节）。
const POINTS_FORMAT: i64 = 2;

const ST_MEASURED: u8 = 0;
const ST_GAP: u8 = 1;
const ST_INVALID: u8 = 2;
const ST_PENDING: u8 = 3;

fn encode(table: &[PointState]) -> Vec<u8> {
    let mut out = Vec::with_capacity(table.len() * 9);
    for p in table {
        let (d, w, st) = match *p {
            PointState::Measured { d, w } => (d, w, ST_MEASURED),
            PointState::Gap => (0.0, f32::NAN, ST_GAP),
            PointState::Invalid => (0.0, f32::NAN, ST_INVALID),
            PointState::Pending => (0.0, f32::NAN, ST_PENDING),
        };
        out.extend_from_slice(&d.to_le_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.push(st);
    }
    out
}

/// 每点的 (位置, 胶宽, 状态)。
fn raw_points(format: i64, blob: &[u8]) -> impl Iterator<Item = (f32, f32, u8)> + '_ {
    let size = if format >= 2 { 9 } else { 5 };
    let f = |c: &[u8]| f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
    blob.chunks_exact(size).map(move |c| if size == 9 { (f(c), f(&c[4..]), c[8]) } else { (f(c), f32::NAN, c[4]) })
}

pub fn decode(format: i64, blob: &[u8]) -> Vec<PointState> {
    raw_points(format, blob)
        .map(|(d, w, st)| match st {
            ST_MEASURED => PointState::Measured { d, w },
            ST_GAP => PointState::Gap,
            ST_INVALID => PointState::Invalid,
            _ => PointState::Pending,
        })
        .collect()
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HistoryQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub verdicts: Vec<Verdict>,
    pub sn: Option<String>,
    pub recipe_id: Option<String>,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartSummary {
    pub id: i64,
    pub ts: i64,
    pub sn: u32,
    pub recipe_id: Option<String>,
    pub recipe_version: Option<u32>,
    pub recipe_hash: Option<String>,
    pub trigger_mode: Option<String>,
    pub verdict: Verdict,
    pub plc_code: u16,
    pub fault_code: u16,
    pub reason: String,
    pub drain_ms: Option<u64>,
    pub frames_expected: usize,
    pub frames_received: usize,
    pub retest_of: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct VerdictCounts {
    pub ok: u64,
    pub excursion: u64,
    pub ng: u64,
    pub err: u64,
}

impl VerdictCounts {
    pub fn add(&mut self, v: Verdict, n: u64) {
        match v {
            Verdict::Ok => self.ok += n,
            Verdict::OkWithExcursion => self.excursion += n,
            Verdict::ErrInspect => self.err += n,
            _ => self.ng += n,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct HistoryPage {
    pub total: u64,
    pub counts: VerdictCounts,
    pub items: Vec<PartSummary>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartPoints {
    pub d: Vec<f32>,
    /// 胶宽；没测胶宽的点为 null
    pub w: Vec<Option<f32>>,
    pub st: Vec<u8>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartDetail {
    pub summary: PartSummary,
    pub judgement: Judgement,
    pub frames: Vec<FrameView>,
    pub triggers: u64,
    pub software_version: String,
    pub points: Option<PartPoints>,
    pub retests: Vec<i64>,
}

/// 重判用：一件工件的判定输入。
pub struct StoredMeasurement {
    pub id: i64,
    pub ts: i64,
    pub sn: u32,
    pub recipe_id: Option<String>,
    pub recipe_hash: Option<String>,
    pub verdict: Verdict,
    pub table: Vec<PointState>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

fn verdict_str(v: Verdict) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

fn parse_verdict(s: &str) -> Verdict {
    serde_json::from_value(serde_json::Value::String(s.into())).unwrap_or(Verdict::ErrInspect)
}

const SUMMARY_COLS: &str = "id, ts, sn, recipe_id, recipe_version, recipe_hash, trigger_mode, verdict, plc_code, fault_code, \
                            reason, drain_ms, frames_expected, frames_received, retest_of";

fn summary(row: &rusqlite::Row) -> rusqlite::Result<PartSummary> {
    Ok(PartSummary {
        id: row.get(0)?,
        ts: row.get(1)?,
        sn: row.get(2)?,
        recipe_id: row.get(3)?,
        recipe_version: row.get(4)?,
        recipe_hash: row.get(5)?,
        trigger_mode: row.get(6)?,
        verdict: parse_verdict(&row.get::<_, String>(7)?),
        plc_code: row.get(8)?,
        fault_code: row.get(9)?,
        reason: row.get(10)?,
        drain_ms: row.get::<_, Option<i64>>(11)?.map(|v| v as u64),
        frames_expected: row.get::<_, i64>(12)? as usize,
        frames_received: row.get::<_, i64>(13)? as usize,
        retest_of: row.get(14)?,
    })
}

fn db_err(e: rusqlite::Error) -> String {
    format!("检测记录数据库错误：{e}")
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(db_err)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS parts (
                 id INTEGER PRIMARY KEY,
                 ts INTEGER NOT NULL,
                 sn INTEGER NOT NULL,
                 recipe_id TEXT,
                 recipe_version INTEGER,
                 recipe_hash TEXT,
                 trigger_mode TEXT,
                 verdict TEXT NOT NULL,
                 plc_code INTEGER NOT NULL,
                 fault_code INTEGER NOT NULL,
                 reason TEXT NOT NULL,
                 drain_ms INTEGER,
                 frames_expected INTEGER NOT NULL,
                 frames_received INTEGER NOT NULL,
                 triggers INTEGER NOT NULL,
                 retest_of INTEGER,
                 software_version TEXT NOT NULL,
                 judgement TEXT NOT NULL,
                 frames TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS parts_ts ON parts(ts);
             CREATE INDEX IF NOT EXISTS parts_sn ON parts(sn);
             CREATE TABLE IF NOT EXISTS part_points (
                 part_id INTEGER PRIMARY KEY REFERENCES parts(id) ON DELETE CASCADE,
                 format INTEGER NOT NULL,
                 data BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS recipe_snapshots (
                 hash TEXT PRIMARY KEY,
                 recipe_id TEXT NOT NULL,
                 version INTEGER NOT NULL,
                 json TEXT NOT NULL
             );",
        )
        .map_err(db_err)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn insert(&self, r: &PartRecord) -> Result<i64, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        if let Some(recipe) = r.recipe {
            let json = serde_json::to_string(recipe).map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT OR IGNORE INTO recipe_snapshots (hash, recipe_id, version, json) VALUES (?1, ?2, ?3, ?4)",
                params![recipe.hash, recipe.id, recipe.version, json],
            )
            .map_err(db_err)?;
        }
        let retest_of: Option<i64> = if r.sn == 0 {
            None
        } else {
            tx.query_row("SELECT id FROM parts WHERE sn = ?1 ORDER BY id DESC LIMIT 1", [r.sn], |row| row.get(0))
                .optional()
                .map_err(db_err)?
        };
        let j = r.judgement;
        tx.execute(
            "INSERT INTO parts (ts, sn, recipe_id, recipe_version, recipe_hash, trigger_mode, verdict, plc_code, fault_code,
                 reason, drain_ms, frames_expected, frames_received, triggers, retest_of, software_version, judgement, frames)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                r.ts,
                r.sn,
                r.recipe.map(|x| x.id.clone()),
                r.recipe.map(|x| x.version),
                r.recipe.map(|x| x.hash.clone()),
                r.recipe.and_then(|x| serde_json::to_value(x.trigger_mode).ok().and_then(|v| v.as_str().map(String::from))),
                verdict_str(j.verdict),
                j.plc_code,
                j.fault_code,
                j.reason,
                r.drain_ms.map(|v| v as i64),
                r.frames_expected as i64,
                r.frames_received as i64,
                r.triggers as i64,
                retest_of,
                r.software_version,
                serde_json::to_string(j).map_err(|e| e.to_string())?,
                serde_json::to_string(r.frames).map_err(|e| e.to_string())?,
            ],
        )
        .map_err(db_err)?;
        let id = tx.last_insert_rowid();
        if let Some(table) = r.table {
            tx.execute("INSERT INTO part_points (part_id, format, data) VALUES (?1, ?2, ?3)", params![id, POINTS_FORMAT, encode(table)])
                .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        Ok(id)
    }

    fn filter(q: &HistoryQuery) -> (String, Vec<SqlValue>) {
        let mut clauses = Vec::new();
        let mut args = Vec::new();
        if let Some(from) = q.from {
            clauses.push("ts >= ?".to_string());
            args.push(SqlValue::Integer(from));
        }
        if let Some(to) = q.to {
            clauses.push("ts < ?".to_string());
            args.push(SqlValue::Integer(to));
        }
        if !q.verdicts.is_empty() {
            clauses.push(format!("verdict IN ({})", vec!["?"; q.verdicts.len()].join(",")));
            args.extend(q.verdicts.iter().map(|v| SqlValue::Text(verdict_str(*v))));
        }
        if let Some(sn) = q.sn.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            clauses.push("CAST(sn AS TEXT) LIKE ?".to_string());
            args.push(SqlValue::Text(format!("%{sn}%")));
        }
        if let Some(id) = q.recipe_id.as_deref().filter(|s| !s.is_empty()) {
            clauses.push("recipe_id = ?".to_string());
            args.push(SqlValue::Text(id.to_string()));
        }
        let sql = if clauses.is_empty() { String::new() } else { format!(" WHERE {}", clauses.join(" AND ")) };
        (sql, args)
    }

    pub fn query(&self, q: &HistoryQuery) -> Result<HistoryPage, String> {
        let conn = self.conn.lock().unwrap();
        let (filter, args) = Self::filter(q);
        let mut counts = VerdictCounts::default();
        let mut total = 0;
        let mut stmt = conn.prepare(&format!("SELECT verdict, COUNT(*) FROM parts{filter} GROUP BY verdict")).map_err(db_err)?;
        let rows = stmt
            .query_map(params_from_iter(args.iter()), |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)))
            .map_err(db_err)?;
        for row in rows {
            let (v, n) = row.map_err(db_err)?;
            counts.add(parse_verdict(&v), n);
            total += n;
        }
        let limit = if q.limit == 0 { 50 } else { q.limit.min(500) };
        let sql = format!("SELECT {SUMMARY_COLS} FROM parts{filter} ORDER BY id DESC LIMIT {limit} OFFSET {}", q.offset);
        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let items = stmt.query_map(params_from_iter(args.iter()), summary).map_err(db_err)?.collect::<Result<_, _>>().map_err(db_err)?;
        Ok(HistoryPage { total, counts, items })
    }

    pub fn detail(&self, id: i64) -> Result<PartDetail, String> {
        let conn = self.conn.lock().unwrap();
        let (summary, judgement, frames, triggers, software_version) = conn
            .query_row(
                &format!("SELECT {SUMMARY_COLS}, judgement, frames, triggers, software_version FROM parts WHERE id = ?1"),
                [id],
                |row| Ok((summary(row)?, row.get::<_, String>(15)?, row.get::<_, String>(16)?, row.get::<_, i64>(17)? as u64, row.get::<_, String>(18)?)),
            )
            .optional()
            .map_err(db_err)?
            .ok_or_else(|| format!("记录 {id} 不存在或已过期清理"))?;
        let points = conn
            .query_row("SELECT format, data FROM part_points WHERE part_id = ?1", [id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)))
            .optional()
            .map_err(db_err)?
            .map(|(format, blob)| {
                let raw: Vec<_> = raw_points(format, &blob).collect();
                PartPoints {
                    d: raw.iter().map(|p| p.0).collect(),
                    w: raw.iter().map(|p| p.1.is_finite().then_some(p.1)).collect(),
                    st: raw.iter().map(|p| p.2).collect(),
                }
            });
        let mut stmt = conn.prepare("SELECT id FROM parts WHERE retest_of = ?1").map_err(db_err)?;
        let retests = stmt.query_map([id], |row| row.get(0)).map_err(db_err)?.collect::<Result<_, _>>().map_err(db_err)?;
        Ok(PartDetail {
            summary,
            judgement: serde_json::from_str(&judgement).map_err(|e| e.to_string())?,
            frames: serde_json::from_str(&frames).unwrap_or_default(),
            triggers,
            software_version,
            points,
            retests,
        })
    }

    pub fn recipe_snapshot(&self, hash: &str) -> Result<Option<Recipe>, String> {
        let conn = self.conn.lock().unwrap();
        let json: Option<String> =
            conn.query_row("SELECT json FROM recipe_snapshots WHERE hash = ?1", [hash], |row| row.get(0)).optional().map_err(db_err)?;
        json.map(|j| serde_json::from_str(&j).map_err(|e| e.to_string())).transpose()
    }

    /// 取出可重判的记录（有测量点数据的），ids 为空时按筛选条件，最多 limit 件。
    pub fn measurements(&self, q: &HistoryQuery, ids: &[i64], limit: usize) -> Result<Vec<StoredMeasurement>, String> {
        let conn = self.conn.lock().unwrap();
        let (filter, args) = if ids.is_empty() {
            Self::filter(q)
        } else {
            (format!(" WHERE id IN ({})", vec!["?"; ids.len()].join(",")), ids.iter().map(|&i| SqlValue::Integer(i)).collect())
        };
        let sql = format!(
            "SELECT p.id, p.ts, p.sn, p.recipe_id, p.recipe_hash, p.verdict, pp.format, pp.data FROM parts p
             JOIN part_points pp ON pp.part_id = p.id{filter} ORDER BY p.id DESC LIMIT {limit}"
        );
        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map(params_from_iter(args.iter()), |row| {
                Ok(StoredMeasurement {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    sn: row.get(2)?,
                    recipe_id: row.get(3)?,
                    recipe_hash: row.get(4)?,
                    verdict: parse_verdict(&row.get::<_, String>(5)?),
                    table: decode(row.get(6)?, &row.get::<_, Vec<u8>>(7)?),
                })
            })
            .map_err(db_err)?;
        rows.collect::<Result<_, _>>().map_err(db_err)
    }

    pub fn counts_since(&self, since: i64) -> Result<VerdictCounts, String> {
        self.query(&HistoryQuery { from: Some(since), limit: 1, ..Default::default() }).map(|p| p.counts)
    }

    pub fn purge_before(&self, ts: i64) -> Result<usize, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM parts WHERE ts < ?1", [ts]).map_err(db_err)
    }
}
