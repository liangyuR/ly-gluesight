use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::cycle::{FrameStatus, FrameView};
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
    pub cycle_id: Option<&'a str>,
    pub bundle_hash: Option<&'a str>,
    pub delivery: &'a PlcDelivery,
    pub shots: &'a [PartShot],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlcDeliveryState {
    #[default]
    NotRequired,
    Pending,
    Submitted,
    Acknowledged,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlcDelivery {
    pub state: PlcDeliveryState,
    pub updated_at: i64,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingState {
    #[default]
    Pending,
    Off,
    NotRetained,
    Complete,
    Incomplete,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingEvidence {
    pub state: RecordingState,
    pub available: bool,
    pub directory: Option<String>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotRawFile {
    pub view: u8,
    pub file: String,
    pub hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartShot {
    pub k: usize,
    pub shot_id: String,
    pub camera: String,
    pub view: u8,
    pub session: Option<u64>,
    pub ordinal: Option<u64>,
    pub frame_counter: Option<u64>,
    pub trigger_counter: Option<u64>,
    pub status: FrameStatus,
    pub error: Option<String>,
    pub score: Option<f32>,
    pub ms: Option<u32>,
    pub raw_files: Vec<ShotRawFile>,
}

pub const DB_VERSION: i64 = 2;

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
    pub cycle_id: Option<String>,
    pub bundle_hash: Option<String>,
    pub delivery: PlcDelivery,
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
    pub shots: Vec<PartShot>,
    pub recording: RecordingEvidence,
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
    pub cycle_id: Option<String>,
    pub bundle_hash: Option<String>,
    pub delivery: PlcDelivery,
    pub layout_hash: Option<String>,
}

pub struct Store {
    conn: Mutex<Connection>,
    backup_path: Option<PathBuf>,
    pub interrupted_recordings: usize,
}

fn verdict_str(v: Verdict) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

fn parse_verdict(s: &str) -> Verdict {
    serde_json::from_value(serde_json::Value::String(s.into())).unwrap_or(Verdict::ErrInspect)
}

const SUMMARY_COLS: &str = "id, ts, sn, recipe_id, recipe_version, recipe_hash, trigger_mode, verdict, plc_code, fault_code, \
                            reason, drain_ms, frames_expected, frames_received, retest_of, cycle_id, bundle_hash, \
                            delivery_state, delivery_updated_at, delivery_message";

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
        cycle_id: row.get(15)?,
        bundle_hash: row.get(16)?,
        delivery: delivery_row(row, 17)?,
    })
}

fn db_err(e: rusqlite::Error) -> String {
    format!("检测记录数据库错误：{e}")
}

fn enum_name(value: impl Serialize) -> String {
    serde_json::to_value(value).unwrap().as_str().unwrap().to_string()
}

fn json_column<T: serde::de::DeserializeOwned>(row: &rusqlite::Row, index: usize) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e)))
}

fn recording_row(row: &rusqlite::Row, index: usize) -> rusqlite::Result<RecordingEvidence> {
    Ok(RecordingEvidence {
        state: enum_column(row, index)?,
        available: row.get(index + 1)?,
        directory: row.get(index + 2)?,
        errors: json_column(row, index + 3)?,
    })
}

fn enum_column<T: serde::de::DeserializeOwned>(row: &rusqlite::Row, index: usize) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_value(serde_json::Value::String(text))
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e)))
}

fn count_column(row: &rusqlite::Row, index: usize) -> rusqlite::Result<Option<u64>> {
    row.get::<_, Option<String>>(index)?
        .map(|s| s.parse::<u64>().map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))))
        .transpose()
}

fn delivery_row(row: &rusqlite::Row, first: usize) -> rusqlite::Result<PlcDelivery> {
    Ok(PlcDelivery { state: enum_column(row, first)?, updated_at: row.get(first + 1)?, message: row.get(first + 2)? })
}

fn measurement_layout(recipe: &Recipe) -> serde_json::Value {
    let shots: Vec<_> =
        recipe.shots.iter().map(|s| serde_json::json!([s.id, s.pose_id, s.camera, s.view, s.skip, s.path, s.mm_per_px, s.calib_ref(), s.detect])).collect();
    let segments: Vec<_> = recipe.segments.iter().map(|s| (s.shot, s.first, s.count)).collect();
    serde_json::json!({"unit":"mm", "schema":recipe.schema_version, "spacing":recipe.spacing,
        "shots":shots, "segments":segments, "points":recipe.points, "detect":recipe.detect})
}

pub fn measurement_layout_hash(recipe: &Recipe) -> String {
    let bytes = measurement_layout(recipe).to_string();
    format!("{:016x}", bytes.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3)))
}

pub fn same_measurement_layout(original: &Recipe, candidate: &Recipe) -> bool {
    measurement_layout(original) == measurement_layout(candidate)
}

fn validate_raw_files(files: &[ShotRawFile]) -> Result<(), String> {
    let mut views = std::collections::HashSet::new();
    for file in files {
        if !(1..=3).contains(&file.view) || !views.insert(file.view) {
            return Err("原图元数据的视角越界或重复".into());
        }
        if file.file.is_empty()
            || file.file.contains('\\')
            || file.file.contains(':')
            || file.file.contains('\0')
            || Path::new(&file.file).components().any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err("原图文件必须是录制根目录内的相对路径，使用 / 分隔".into());
        }
    }
    Ok(())
}

fn backup_old_database(conn: Connection, path: &Path, version: i64) -> Result<PathBuf, String> {
    conn.pragma_update(None, "locking_mode", "EXCLUSIVE").map_err(db_err)?;
    conn.execute_batch("BEGIN EXCLUSIVE; COMMIT;").map_err(db_err)?;
    let (busy, log, done): (i64, i64, i64) =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(db_err)?;
    if busy != 0 || (log >= 0 && done < log) {
        return Err("旧记录库仍在使用，WAL 未完整合并；保留原库，关闭其他实例后重试".into());
    }
    let mode: String = conn.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0)).map_err(db_err)?;
    if mode != "delete" {
        return Err("旧记录库无法退出 WAL 模式，保留原库".into());
    }
    let token: String = conn.query_row("SELECT lower(hex(randomblob(8)))", [], |r| r.get(0)).map_err(db_err)?;
    conn.close().map_err(|(_, e)| db_err(e))?;
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        if Path::new(&name).exists() {
            return Err("旧记录库仍有 WAL/SHM 文件，保留原库以防丢失数据".into());
        }
    }
    let name = path.file_name().ok_or("记录库路径没有文件名")?.to_string_lossy();
    let backup = path.with_file_name(format!("{name}.v{version}.{}.{token}.bak", ly_plc::now_ms()));
    std::fs::rename(path, &backup).map_err(|e| format!("备份旧记录库失败，原库未重建：{e}"))?;
    Ok(backup)
}

fn fail_interrupted_recordings(conn: &mut Connection) -> Result<usize, String> {
    const REASON: &str = "检测服务在录制完成前退出，原图完整性未确认";
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_err)?;
    let pending = {
        let mut stmt = tx.prepare("SELECT id,recording_errors FROM parts WHERE recording_state='pending' ORDER BY id").map_err(db_err)?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, json_column::<Vec<String>>(row, 1)?))).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?
    };
    let mut interrupted = 0;
    for (id, mut errors) in pending {
        if !errors.iter().any(|error| error == REASON) {
            errors.push(REASON.into());
        }
        interrupted += tx.execute(
            "UPDATE parts SET recording_state='failed',recording_available=0,recording_errors=?2 WHERE id=?1 AND recording_state='pending'",
            params![id, serde_json::to_string(&errors).map_err(|e| e.to_string())?],
        ).map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(interrupted)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let existing = path.metadata().map(|m| m.len() > 0).unwrap_or(false);
        let mut conn = Connection::open(path).map_err(db_err)?;
        conn.busy_timeout(Duration::from_secs(2)).map_err(db_err)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(db_err)?;
        let backup_path = if existing && version != DB_VERSION {
            let backup = backup_old_database(conn, path, version)?;
            conn = Connection::open(path).map_err(|e| format!("旧记录库已备份到 {}；新库打开失败：{e}", backup.display()))?;
            conn.busy_timeout(Duration::from_secs(2)).map_err(db_err)?;
            Some(backup)
        } else {
            None
        };
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS cycle_ids (
                 id TEXT PRIMARY KEY NOT NULL DEFAULT (lower(hex(randomblob(16)))),
                 created_at INTEGER NOT NULL
             );
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
                 frames TEXT NOT NULL,
                 cycle_id TEXT,
                 bundle_hash TEXT,
                 delivery_state TEXT NOT NULL,
                 delivery_updated_at INTEGER NOT NULL,
                 delivery_message TEXT,
                 layout_hash TEXT,
                 recording_state TEXT NOT NULL DEFAULT 'pending',
                 recording_available INTEGER NOT NULL DEFAULT 0,
                 recording_directory TEXT,
                 recording_errors TEXT NOT NULL DEFAULT '[]'
             );
             CREATE INDEX IF NOT EXISTS parts_ts ON parts(ts);
             CREATE INDEX IF NOT EXISTS parts_sn ON parts(sn);
             CREATE UNIQUE INDEX IF NOT EXISTS parts_cycle ON parts(cycle_id) WHERE cycle_id IS NOT NULL;
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
             );
             CREATE TABLE IF NOT EXISTS part_shots (
                 part_id INTEGER NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
                 k INTEGER NOT NULL,
                 shot_id TEXT NOT NULL,
                 camera TEXT NOT NULL,
                 view INTEGER NOT NULL CHECK(view BETWEEN 1 AND 3),
                 session TEXT,
                 ordinal TEXT,
                 frame_counter TEXT,
                 trigger_counter TEXT,
                 status TEXT NOT NULL,
                 error TEXT,
                 score REAL,
                 ms INTEGER,
                 raw_files TEXT NOT NULL,
                 PRIMARY KEY (part_id, k),
                 UNIQUE (part_id, shot_id)
             );
             PRAGMA user_version = 2;
             COMMIT;",
        )
        .map_err(db_err)?;
        let interrupted_recordings = fail_interrupted_recordings(&mut conn)?;
        Ok(Self { conn: Mutex::new(conn), backup_path, interrupted_recordings })
    }

    pub fn backup_path(&self) -> Option<&Path> {
        self.backup_path.as_deref()
    }

    pub fn reserve_cycle_id(&self) -> Result<String, String> {
        Ok(self.reserve_cycle_ids(1)?.remove(0))
    }

    pub fn reserve_cycle_ids(&self, count: usize) -> Result<Vec<String>, String> {
        if !(1..=32).contains(&count) { return Err("工件身份批量分配数量必须为 1–32".into()); }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let ids = (0..count).map(|_| tx.query_row(
            "INSERT INTO cycle_ids (created_at) VALUES (?1) RETURNING id",
            [ly_plc::now_ms()], |row| row.get(0),
        ).map_err(db_err)).collect::<Result<Vec<String>, String>>()?;
        tx.commit().map_err(db_err)?;
        Ok(ids)
    }

    pub fn insert(&self, r: &PartRecord) -> Result<i64, String> {
        if r.cycle_id.is_some_and(|s| s.is_empty()) || r.bundle_hash.is_some_and(|s| s.is_empty()) {
            return Err("检测记录的 cycleId 或发布包哈希为空".into());
        }
        if let Some(recipe) = r.recipe {
            if r.shots.len() != recipe.shot_count() || r.frames_expected != recipe.shot_count() {
                return Err("检测记录必须包含每个计划拍照点，包括缺帧拍照点".into());
            }
            let mut seen = std::collections::HashSet::new();
            for shot in r.shots {
                let expected = recipe.shots.get(shot.k).ok_or("记录中的拍照点序号越界")?;
                if !seen.insert(shot.k) || shot.shot_id != expected.id || shot.camera != expected.camera || shot.view != expected.view {
                    return Err("记录中的拍照点身份、相机或视角与原配方不一致".into());
                }
            }
            if r.table.is_some_and(|t| t.len() != recipe.point_count()) {
                return Err("测量点数据长度与原配方不一致".into());
            }
        } else if !r.shots.is_empty() || r.table.is_some() {
            return Err("缺少原配方，不能保存无身份的拍照点或测量点".into());
        }
        for shot in r.shots {
            if shot.score.is_some_and(|x| !x.is_finite()) {
                return Err("拍照点得分不是有限数".into());
            }
            validate_raw_files(&shot.raw_files)?;
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        if let Some(recipe) = r.recipe {
            let json = serde_json::to_string(recipe).map_err(|e| e.to_string())?;
            let previous: Option<String> =
                tx.query_row("SELECT json FROM recipe_snapshots WHERE hash = ?1", [&recipe.hash], |row| row.get(0)).optional().map_err(db_err)?;
            if let Some(previous) = previous {
                let mut previous: serde_json::Value = serde_json::from_str(&previous).map_err(|e| e.to_string())?;
                let mut candidate: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
                previous.as_object_mut().ok_or("原配方快照格式损坏")?.remove("version");
                candidate.as_object_mut().ok_or("配方快照格式错误")?.remove("version");
                if previous != candidate {
                    return Err("相同配方哈希对应了不同快照，拒绝覆盖原记录".into());
                }
            }
            tx.execute(
                "INSERT OR IGNORE INTO recipe_snapshots (hash, recipe_id, version, json) VALUES (?1, ?2, ?3, ?4)",
                params![recipe.hash, recipe.id, recipe.version, json],
            )
            .map_err(db_err)?;
        }
        let retest_of: Option<i64> = if r.sn == 0 {
            None
        } else {
            tx.query_row("SELECT id FROM parts WHERE sn = ?1 ORDER BY id DESC LIMIT 1", [r.sn], |row| row.get(0)).optional().map_err(db_err)?
        };
        let j = r.judgement;
        tx.execute(
            "INSERT INTO parts (ts, sn, recipe_id, recipe_version, recipe_hash, trigger_mode, verdict, plc_code, fault_code,
                 reason, drain_ms, frames_expected, frames_received, triggers, retest_of, software_version, judgement, frames,
                 cycle_id, bundle_hash, delivery_state, delivery_updated_at, delivery_message, layout_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                 ?19, ?20, ?21, ?22, ?23, ?24)",
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
                r.cycle_id,
                r.bundle_hash,
                enum_name(r.delivery.state),
                r.delivery.updated_at,
                r.delivery.message,
                r.recipe.map(measurement_layout_hash),
            ],
        )
        .map_err(db_err)?;
        let id = tx.last_insert_rowid();
        for shot in r.shots {
            tx.execute(
                "INSERT INTO part_shots (part_id,k,shot_id,camera,view,session,ordinal,frame_counter,trigger_counter,status,error,score,ms,raw_files)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                params![
                    id,
                    shot.k as i64,
                    shot.shot_id,
                    shot.camera,
                    shot.view,
                    shot.session.map(|n| n.to_string()),
                    shot.ordinal.map(|n| n.to_string()),
                    shot.frame_counter.map(|n| n.to_string()),
                    shot.trigger_counter.map(|n| n.to_string()),
                    enum_name(shot.status),
                    shot.error,
                    shot.score,
                    shot.ms,
                    serde_json::to_string(&shot.raw_files).map_err(|e| e.to_string())?
                ],
            )
            .map_err(db_err)?;
        }
        if let Some(table) = r.table {
            tx.execute("INSERT INTO part_points (part_id, format, data) VALUES (?1, ?2, ?3)", params![id, POINTS_FORMAT, encode(table)]).map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        Ok(id)
    }

    pub fn update_delivery(&self, cycle_id: &str, delivery: &PlcDelivery) -> Result<bool, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let previous = tx
            .query_row("SELECT delivery_state,delivery_updated_at,delivery_message FROM parts WHERE cycle_id=?1", [cycle_id], |r| delivery_row(r, 0))
            .optional()
            .map_err(db_err)?;
        let Some(previous) = previous else { return Ok(false) };
        if delivery.updated_at < previous.updated_at {
            return Ok(false);
        }
        if previous.state == PlcDeliveryState::Acknowledged && delivery.state != PlcDeliveryState::Acknowledged {
            return Err("PLC 已确认交付，不能回退交付状态".into());
        }
        if previous.state == PlcDeliveryState::NotRequired && delivery.state != PlcDeliveryState::NotRequired {
            return Err("该记录无需 PLC 交付，不能追加到其他事务".into());
        }
        if previous.state != PlcDeliveryState::NotRequired && delivery.state == PlcDeliveryState::NotRequired {
            return Err("已有 PLC 交付事务，不能改为无需交付".into());
        }
        if matches!(previous.state, PlcDeliveryState::Submitted | PlcDeliveryState::Failed) && delivery.state == PlcDeliveryState::Pending {
            return Err("PLC 交付已尝试，不能回退为待提交".into());
        }
        tx.execute(
            "UPDATE parts SET delivery_state=?2,delivery_updated_at=?3,delivery_message=?4 WHERE cycle_id=?1",
            params![cycle_id, enum_name(delivery.state), delivery.updated_at, delivery.message],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(true)
    }

    pub fn recover_acknowledgement(&self, cycle_id: &str, sn: u32, request_seq: u32, acknowledged_at: i64) -> Result<bool, String> {
        if cycle_id.len() != 32 || !cycle_id.bytes().all(|b| b.is_ascii_hexdigit()) || request_seq == 0 || acknowledged_at < 0 {
            return Err("PLC 确认收据的身份无效".into());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_err)?;
        let previous = tx.query_row(
            "SELECT sn,delivery_state,delivery_updated_at,delivery_message FROM parts WHERE cycle_id=?1",
            [cycle_id], |r| Ok((r.get::<_, u32>(0)?, delivery_row(r, 1)?)),
        ).optional().map_err(db_err)?;
        let Some((recorded_sn, previous)) = previous else { return Err("PLC 已确认，但对应 cycleId 的检测记录不存在；保留握手审计供核查".into()) };
        if recorded_sn != sn { return Err("PLC 确认收据与检测记录的 SN 不一致".into()); }
        if previous.state == PlcDeliveryState::NotRequired { return Err("检测记录无需 PLC 交付，不能追加确认收据".into()); }
        if previous.state == PlcDeliveryState::Acknowledged { return Ok(false); }
        tx.execute("UPDATE parts SET delivery_state='acknowledged',delivery_updated_at=?2,delivery_message=?3 WHERE cycle_id=?1",
            params![cycle_id, previous.updated_at.max(acknowledged_at), format!("从持久握手记录恢复 PLC 确认：请求序号 {request_seq}，收据时间 {acknowledged_at}")])
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(true)
    }

    pub fn update_shot_raw_files(&self, cycle_id: &str, k: usize, files: &[ShotRawFile]) -> Result<bool, String> {
        validate_raw_files(files)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let previous: Option<(i64, Vec<ShotRawFile>)> = tx
            .query_row(
                "SELECT s.part_id,s.raw_files FROM part_shots s JOIN parts p ON p.id=s.part_id WHERE p.cycle_id=?1 AND s.k=?2",
                params![cycle_id, k as i64],
                |row| Ok((row.get(0)?, json_column(row, 1)?)),
            )
            .optional()
            .map_err(db_err)?;
        let Some((id, previous)) = previous else { return Ok(false) };
        if previous.iter().any(|old| !files.iter().any(|new| old.view == new.view && old.file == new.file && (old.hash.is_none() || old.hash == new.hash))) {
            return Err("原图身份已保存，不能替换为另一组文件".into());
        }
        tx.execute(
            "UPDATE part_shots SET raw_files=?3 WHERE part_id=?1 AND k=?2",
            params![id, k as i64, serde_json::to_string(files).map_err(|e| e.to_string())?],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(true)
    }

    pub fn update_recording(&self, cycle_id: &str, evidence: &RecordingEvidence) -> Result<bool, String> {
        if evidence.available != (evidence.state == RecordingState::Complete)
            || (evidence.available && (!evidence.errors.is_empty() || evidence.directory.as_deref().is_none_or(str::is_empty)))
        {
            return Err("只有带实际目录且无错误的完整录制才能标记原图可用".into());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let previous: Option<(i64, RecordingEvidence)> = tx
            .query_row("SELECT id,recording_state,recording_available,recording_directory,recording_errors FROM parts WHERE cycle_id=?1", [cycle_id], |row| {
                Ok((row.get(0)?, recording_row(row, 1)?))
            })
            .optional()
            .map_err(db_err)?;
        let Some((id, previous)) = previous else { return Ok(false) };
        if previous.state != RecordingState::Pending && previous != *evidence {
            return Err("原录制收尾证据已保存，不能替换或回退".into());
        }
        if evidence.available {
            let mut stmt = tx.prepare("SELECT raw_files FROM part_shots WHERE part_id=?1").map_err(db_err)?;
            let refs = stmt.query_map([id], |row| json_column::<Vec<ShotRawFile>>(row, 0)).map_err(db_err)?;
            let mut has_file = false;
            for files in refs {
                has_file |= files.map_err(db_err)?.iter().any(|file| file.hash.as_deref().is_some_and(|hash| !hash.is_empty()));
            }
            if !has_file {
                return Err("没有成功保存的带哈希原图引用，不能标记录制可用".into());
            }
        }
        tx.execute(
            "UPDATE parts SET recording_state=?2,recording_available=?3,recording_directory=?4,recording_errors=?5 WHERE id=?1",
            params![id, enum_name(evidence.state), evidence.available, evidence.directory, serde_json::to_string(&evidence.errors).map_err(|e| e.to_string())?],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(true)
    }

    fn filter(q: &HistoryQuery, prefix: &str) -> (String, Vec<SqlValue>) {
        let mut clauses = Vec::new();
        let mut args = Vec::new();
        if let Some(from) = q.from {
            clauses.push(format!("{prefix}ts >= ?"));
            args.push(SqlValue::Integer(from));
        }
        if let Some(to) = q.to {
            clauses.push(format!("{prefix}ts < ?"));
            args.push(SqlValue::Integer(to));
        }
        if !q.verdicts.is_empty() {
            clauses.push(format!("{prefix}verdict IN ({})", vec!["?"; q.verdicts.len()].join(",")));
            args.extend(q.verdicts.iter().map(|v| SqlValue::Text(verdict_str(*v))));
        }
        if let Some(sn) = q.sn.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            clauses.push(format!("CAST({prefix}sn AS TEXT) LIKE ?"));
            args.push(SqlValue::Text(format!("%{sn}%")));
        }
        if let Some(id) = q.recipe_id.as_deref().filter(|s| !s.is_empty()) {
            clauses.push(format!("{prefix}recipe_id = ?"));
            args.push(SqlValue::Text(id.to_string()));
        }
        let sql = if clauses.is_empty() { String::new() } else { format!(" WHERE {}", clauses.join(" AND ")) };
        (sql, args)
    }

    pub fn pending_recording_directories(&self, root: &Path) -> Result<Vec<PathBuf>, String> {
        let root_text = root.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_string();
        let prefix = format!("{root_text}/");
        let mut paths = Vec::new();
        let mut protect = |value: &str| -> Result<(), String> {
            let normalized = value.replace('\\', "/");
            let relative = if normalized.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(&prefix)) {
                &normalized[prefix.len()..]
            } else if !Path::new(value).is_absolute() && !normalized.contains(':') {
                normalized.as_str()
            } else {
                if normalized.to_ascii_lowercase().contains("/_pending/") {
                    return Err("历史 pending 引用不在当前 records 根目录，暂停清理".into());
                }
                return Ok(());
            };
            let components: Vec<_> = relative.split('/').collect();
            if !components.first().is_some_and(|p| p.eq_ignore_ascii_case("_pending")) { return Ok(()); }
            if components.len() < 2 || components.iter().any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains(':')) {
                return Err("历史 pending 录制引用路径无效，暂停清理".into());
            }
            paths.push(root.join("_pending").join(components[1]));
            Ok(())
        };
        let conn = self.conn.lock().map_err(|_| "数据库锁异常，暂停孤立录制清理".to_string())?;
        let mut stmt = conn.prepare("SELECT recording_directory FROM parts WHERE instr(lower(recording_directory),'_pending')>0").map_err(db_err)?;
        for row in stmt.query_map([], |row| row.get::<_, String>(0)).map_err(db_err)? {
            protect(&row.map_err(db_err)?)?;
        }
        let mut stmt = conn.prepare("SELECT raw_files FROM part_shots WHERE NOT json_valid(raw_files) OR instr(lower(raw_files),'_pending')>0").map_err(db_err)?;
        for row in stmt.query_map([], |row| row.get::<_, String>(0)).map_err(db_err)? {
            let files: Vec<ShotRawFile> = serde_json::from_str(&row.map_err(db_err)?).map_err(|e| format!("历史原图引用损坏，暂停清理：{e}"))?;
            validate_raw_files(&files)?;
            for file in files { protect(&file.file)?; }
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    pub fn query(&self, q: &HistoryQuery) -> Result<HistoryPage, String> {
        let conn = self.conn.lock().unwrap();
        let (filter, args) = Self::filter(q, "");
        let mut counts = VerdictCounts::default();
        let mut total = 0;
        let mut stmt = conn.prepare(&format!("SELECT verdict, COUNT(*) FROM parts{filter} GROUP BY verdict")).map_err(db_err)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))).map_err(db_err)?;
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
        let (summary, judgement, frames, triggers, software_version, recording) = conn
            .query_row(
                &format!("SELECT {SUMMARY_COLS}, judgement, frames, triggers, software_version,recording_state,recording_available,recording_directory,recording_errors FROM parts WHERE id = ?1"),
                [id],
                |row| Ok((summary(row)?, row.get::<_, String>(20)?, row.get::<_, String>(21)?, row.get::<_, i64>(22)? as u64, row.get::<_, String>(23)?, recording_row(row, 24)?)),
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
        let mut stmt = conn.prepare("SELECT id FROM parts WHERE retest_of = ?1 ORDER BY id").map_err(db_err)?;
        let retests = stmt.query_map([id], |row| row.get(0)).map_err(db_err)?.collect::<Result<_, _>>().map_err(db_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT k,shot_id,camera,view,session,ordinal,frame_counter,trigger_counter,status,error,score,ms,raw_files
            FROM part_shots WHERE part_id=?1 ORDER BY k",
            )
            .map_err(db_err)?;
        let shots = stmt
            .query_map([id], |row| {
                Ok(PartShot {
                    k: row.get::<_, i64>(0)? as usize,
                    shot_id: row.get(1)?,
                    camera: row.get(2)?,
                    view: row.get(3)?,
                    session: count_column(row, 4)?,
                    ordinal: count_column(row, 5)?,
                    frame_counter: count_column(row, 6)?,
                    trigger_counter: count_column(row, 7)?,
                    status: enum_column(row, 8)?,
                    error: row.get(9)?,
                    score: row.get(10)?,
                    ms: row.get(11)?,
                    raw_files: json_column(row, 12)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<_, _>>()
            .map_err(db_err)?;
        Ok(PartDetail {
            summary,
            judgement: serde_json::from_str(&judgement).map_err(|e| e.to_string())?,
            frames: serde_json::from_str(&frames).map_err(|e| format!("记录 {id} 的帧信息损坏：{e}"))?,
            triggers,
            software_version,
            points,
            retests,
            shots,
            recording,
        })
    }

    pub fn detail_by_cycle(&self, cycle_id: &str) -> Result<Option<PartDetail>, String> {
        let id =
            self.conn.lock().unwrap().query_row("SELECT id FROM parts WHERE cycle_id=?1", [cycle_id], |row| row.get::<_, i64>(0)).optional().map_err(db_err)?;
        id.map(|id| self.detail(id)).transpose()
    }

    pub fn recipe_snapshot(&self, hash: &str) -> Result<Option<Recipe>, String> {
        let conn = self.conn.lock().unwrap();
        let json: Option<String> = conn.query_row("SELECT json FROM recipe_snapshots WHERE hash = ?1", [hash], |row| row.get(0)).optional().map_err(db_err)?;
        json.map(|j| {
            let recipe: Recipe = serde_json::from_str(&j).map_err(|e| e.to_string())?;
            if recipe.hash != hash {
                return Err("原配方快照身份损坏".into());
            }
            Ok(recipe)
        })
        .transpose()
    }

    /// 取出重判候选，缺少测量数据的记录返回空表；ids 为空时按筛选条件。
    pub fn measurements(&self, q: &HistoryQuery, ids: &[i64], limit: usize) -> Result<Vec<StoredMeasurement>, String> {
        let conn = self.conn.lock().unwrap();
        let (filter, args) = if ids.is_empty() {
            Self::filter(q, "p.")
        } else {
            (format!(" WHERE p.id IN ({})", vec!["?"; ids.len()].join(",")), ids.iter().map(|&i| SqlValue::Integer(i)).collect())
        };
        let sql = format!(
            "SELECT p.id, p.ts, p.sn, p.recipe_id, p.recipe_hash, p.verdict, COALESCE(pp.format, {POINTS_FORMAT}), COALESCE(pp.data, x''),
                 p.cycle_id, p.bundle_hash, p.delivery_state, p.delivery_updated_at, p.delivery_message, p.layout_hash FROM parts p
             LEFT JOIN part_points pp ON pp.part_id = p.id{filter} ORDER BY p.id DESC LIMIT {limit}"
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
                    cycle_id: row.get(8)?,
                    bundle_hash: row.get(9)?,
                    delivery: delivery_row(row, 10)?,
                    layout_hash: row.get(13)?,
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

#[cfg(test)]
#[path = "store/tests.rs"]
mod tests;
