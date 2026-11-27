use std::collections::HashMap;
use std::path::Path;

use ly_plc::{HistorySample, LogEntry, LogPage, LogQuery};
use rusqlite::{named_params, params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use super::PlcHost;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupQuery {
    start: i64,
    end: Option<i64>,
    keyword: Option<String>,
    #[serde(default)]
    abnormal_only: bool,
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct GroupSelection {
    ranges: Vec<[i64; 2]>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    #[serde(flatten)]
    selection: GroupSelection,
    first_id: i64,
    last_id: i64,
    first_ts: i64,
    last_ts: i64,
    entry_count: u32,
    warning_count: u32,
    error_count: u32,
    context: Option<String>,
    part_seq: Option<i32>,
    sn: Option<String>,
    event: Option<String>,
    product_code: Option<u16>,
    result_code: Option<u16>,
    fault_code: Option<u16>,
    #[serde(skip)]
    received_at: Option<i64>,
}

impl Group {
    fn include(&mut self, id: i64, ts: i64, level: &str) {
        if self.entry_count == 0 {
            self.first_id = id;
            self.first_ts = ts;
            self.last_ts = ts;
        }
        self.last_id = id;
        self.first_ts = self.first_ts.min(ts);
        self.last_ts = self.last_ts.max(ts);
        self.entry_count += 1;
        self.warning_count += u32::from(level == "warn");
        self.error_count += u32::from(level == "error");
        match self.selection.ranges.last_mut() {
            Some(range) if range[1].checked_add(1) == Some(id) => range[1] = id,
            _ => self.selection.ranges.push([id, id]),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartEvent {
    event: String,
    received_at: i64,
    observed_at: Option<i64>,
    sn: Option<String>,
    product_code: Option<u16>,
    result_code: Option<u16>,
    fault_code: Option<u16>,
}

struct Arrival {
    id: i64,
    observed_at: i64,
    received_at: i64,
}

fn arrivals(
    conn: &Connection,
    start: i64,
    end: i64,
) -> rusqlite::Result<HashMap<String, Vec<Arrival>>> {
    let mut stmt = conn.prepare(
        "SELECT id, context, new_value FROM plc_log
        WHERE ts BETWEEN ?1 AND ?2 AND category = 'part' AND context IS NOT NULL ORDER BY id DESC",
    )?;
    let mut rows = stmt.query(params![start, end])?;
    let mut result = HashMap::<String, Vec<Arrival>>::new();
    while let Some(row) = rows.next()? {
        let value: Option<String> = row.get(2)?;
        let Some(event) = value
            .as_deref()
            .and_then(|v| serde_json::from_str::<PartEvent>(v).ok())
        else {
            continue;
        };
        if matches!(event.event.as_str(), "received" | "skipped") {
            if let Some(observed_at) = event.observed_at {
                result.entry(row.get(1)?).or_default().push(Arrival {
                    id: row.get(0)?,
                    observed_at,
                    received_at: event.received_at,
                });
            }
        }
    }
    Ok(result)
}

#[derive(Default)]
struct Groups {
    items: Vec<Group>,
    active: Option<usize>,
    unassigned: Option<usize>,
    latest: HashMap<String, usize>,
    instances: HashMap<(String, i64), usize>,
}

impl Groups {
    fn create(&mut self, context: Option<&str>, part_seq: Option<i32>) -> usize {
        let index = self.items.len();
        self.items.push(Group {
            context: context.map(str::to_owned),
            part_seq,
            ..Group::default()
        });
        index
    }

    fn select(
        &mut self,
        context: Option<&str>,
        received_at: Option<i64>,
        starts_piece: bool,
    ) -> usize {
        let seq = context
            .and_then(|s| {
                s.strip_prefix("partSeq=")
                    .or_else(|| s.strip_prefix("requestSeq="))
            })
            .and_then(|s| s.parse::<i32>().ok());
        let (Some(context), Some(seq)) = (context, seq.filter(|&s| s > 0)) else {
            if seq == Some(0) {
                self.active = None;
            }
            let index = self.unassigned.unwrap_or_else(|| self.create(None, None));
            self.unassigned = Some(index);
            return index;
        };
        if let Some(received_at) = received_at {
            let key = (context.to_owned(), received_at);
            if let Some(index) = self.instances.get(&key) {
                return *index;
            }
            let latest = self.latest.get(context).copied();
            let index = latest
                .filter(|&index| {
                    self.items[index].received_at.is_none()
                        && (!starts_piece || self.active == Some(index))
                })
                .unwrap_or_else(|| self.create(Some(context), Some(seq)));
            self.items[index].received_at = Some(received_at);
            self.instances.insert(key, index);
            if starts_piece || self.active.is_none() {
                self.active = Some(index);
            }
            if starts_piece || latest.is_none() {
                self.latest.insert(context.into(), index);
            }
            return index;
        }
        let index = self
            .active
            .filter(|&index| self.items[index].context.as_deref() == Some(context))
            .unwrap_or_else(|| self.create(Some(context), Some(seq)));
        self.active = Some(index);
        self.latest.insert(context.into(), index);
        index
    }
}

#[derive(Serialize)]
pub struct GroupPage {
    total: usize,
    items: Vec<Group>,
}

fn open(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())
}

fn groups(conn: &Connection, query: GroupQuery) -> rusqlite::Result<GroupPage> {
    let _snapshot = conn.unchecked_transaction()?;
    let end = query.end.unwrap_or(i64::MAX);
    let mut arrivals = arrivals(conn, query.start, end)?;
    let mut stmt = conn.prepare(
        "SELECT id, ts, level, context, category, point_id, new_value
         FROM plc_log INDEXED BY idx_plc_log_ts WHERE ts BETWEEN ?1 AND ?2 ORDER BY id",
    )?;
    let mut rows = stmt.query(params![query.start, end])?;
    let mut groups = Groups::default();
    while let Some(row) = rows.next()? {
        let context: Option<String> = row.get(3)?;
        let category: String = row.get(4)?;
        let value: Option<String> = row.get(6)?;
        let event = if category == "part" {
            value
                .as_deref()
                .and_then(|v| serde_json::from_str::<PartEvent>(v).ok())
        } else {
            None
        };
        let id: i64 = row.get(0)?;
        let ts: i64 = row.get(1)?;
        let arrival = context
            .as_ref()
            .and_then(|c| arrivals.get_mut(c))
            .and_then(|list| {
                while list.last().is_some_and(|arrival| arrival.id < id) {
                    list.pop();
                }
                list.last()
                    .filter(|arrival| arrival.observed_at <= ts)
                    .map(|arrival| arrival.received_at)
            });
        let (received_at, starts_piece) = match &event {
            Some(event) => (
                Some(event.received_at),
                matches!(event.event.as_str(), "received" | "skipped"),
            ),
            None => (arrival, arrival.is_some()),
        };
        let index = groups.select(context.as_deref(), received_at, starts_piece);
        let group = &mut groups.items[index];
        group.include(id, ts, &row.get::<_, String>(2)?);
        if let Some(event) = event {
            group.sn = event.sn.or_else(|| group.sn.take());
            group.product_code = event.product_code.or(group.product_code);
            group.result_code = event.result_code;
            group.fault_code = event.fault_code;
            group.event = Some(event.event);
        } else if group.part_seq.is_some()
            && group.event.is_none()
            && category == "value"
            && row.get::<_, Option<String>>(5)?.as_deref() == Some("p_sn")
        {
            group.sn = value;
        }
    }
    let keyword = query.keyword.as_deref().unwrap_or("").trim().to_lowercase();
    groups.items.retain(|g| {
        (!query.abnormal_only || g.warning_count + g.error_count > 0)
            && (keyword.is_empty()
                || g.part_seq.is_some_and(|seq| seq.to_string() == keyword)
                || g.sn
                    .as_ref()
                    .is_some_and(|s| s.to_lowercase().contains(&keyword)))
    });
    groups
        .items
        .sort_unstable_by_key(|g| std::cmp::Reverse((g.last_ts, g.last_id)));
    Ok(GroupPage {
        total: groups.items.len(),
        items: groups
            .items
            .into_iter()
            .skip(query.offset.unwrap_or(0) as usize)
            .take(query.limit.unwrap_or(100).clamp(1, 1000) as usize)
            .collect(),
    })
}

#[tauri::command]
pub async fn plc_query_log_groups(plc: &PlcHost, query: GroupQuery) -> Result<GroupPage, String> {
    let path = plc.log_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        groups(&open(&path)?, query).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

const GROUP_ROWS: &str = "FROM json_each(:ranges) AS span
    CROSS JOIN plc_log AS log ON log.id BETWEEN json_extract(span.value, '$[0]') AND json_extract(span.value, '$[1]')";

pub(super) struct Reader {
    conn: Connection,
    ranges: String,
}

impl Reader {
    pub(super) fn open(path: &Path, selection: GroupSelection) -> Result<Self, String> {
        if selection.ranges.iter().any(|r| r[0] < 1 || r[0] > r[1])
            || selection.ranges.windows(2).any(|r| r[0][1] >= r[1][0])
        {
            return Err("日志范围无效".into());
        }
        let conn = open(path)?;
        conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;
        Ok(Self {
            conn,
            ranges: serde_json::to_string(&selection.ranges).map_err(|e| e.to_string())?,
        })
    }

    pub(super) fn logs(&self, q: &LogQuery) -> Result<LogPage, String> {
        self.query(q).map_err(|e| e.to_string())
    }

    fn query(&self, q: &LogQuery) -> rusqlite::Result<LogPage> {
        let filter = format!("{GROUP_ROWS}
            WHERE ts BETWEEN :start AND :end AND (:point IS NULL OR point_id = :point)
              AND (:context IS NULL OR context = :context)
              AND (:levels = '[]' OR level IN (SELECT value FROM json_each(:levels)))
              AND (:categories = '[]' OR category IN (SELECT value FROM json_each(:categories)))
              AND (:keyword IS NULL OR message LIKE :keyword OR point_name LIKE :keyword OR context LIKE :keyword)");
        let args = named_params! {
            ":ranges": self.ranges, ":start": q.start.unwrap_or(i64::MIN), ":end": q.end.unwrap_or(i64::MAX),
            ":point": q.point_id.as_deref().filter(|s| !s.is_empty()),
            ":context": q.context.as_deref().filter(|s| !s.is_empty()),
            ":levels": serde_json::json!(q.levels).to_string(),
            ":categories": serde_json::json!(q.categories).to_string(),
            ":keyword": q.keyword.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| format!("%{s}%"))
        };
        let total = self
            .conn
            .query_row(&format!("SELECT count(*) {filter}"), args, |r| r.get(0))?;
        let limit = q.limit.unwrap_or(100).clamp(1, 1000);
        let offset = q.offset.unwrap_or(0);
        let mut stmt = self.conn.prepare(&format!("SELECT log.id, ts, level, category, point_id,
            point_name, message, old_value, new_value, context {filter} ORDER BY ts, log.id LIMIT {limit} OFFSET {offset}"))?;
        let items = stmt
            .query_map(args, |r| {
                Ok(LogEntry {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    level: r.get(2)?,
                    category: r.get(3)?,
                    point_id: r.get(4)?,
                    point_name: r.get(5)?,
                    message: r.get(6)?,
                    old_value: r.get(7)?,
                    new_value: r.get(8)?,
                    context: r.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(LogPage { total, items })
    }

    pub(super) fn history(
        &self,
        point: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<HistorySample>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT ts, new_value {GROUP_ROWS} WHERE point_id = :point AND category = 'value'
             AND ts BETWEEN :start AND :end ORDER BY ts, log.id LIMIT 20000"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map(named_params! {":ranges": self.ranges, ":point": point, ":start": start, ":end": end},
            |r| Ok(HistorySample {ts: r.get(0)?, value: r.get(1)?})).map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub async fn plc_query_group_logs(
    plc: &PlcHost,
    group: GroupSelection,
    query: LogQuery,
) -> Result<LogPage, String> {
    let path = plc.log_path.clone();
    tauri::async_runtime::spawn_blocking(move || Reader::open(&path, group)?.logs(&query))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offline::test_dir::TestDir;
    use serde_json::{json, Value};
    use std::sync::Arc;

    fn fixture(entries: &[(i64, Option<&str>, &str, &str, &str)]) -> (TestDir, PlcHost) {
        let dir = TestDir::new("plc-log-tests", "groups");
        let path = dir.join("plc.db");
        let sink: ly_plc::EventSink = Arc::new(|_| {});
        let book = ly_plc::Logbook::open(&path, sink.clone(), 0).unwrap();
        let conn = Connection::open(&path).unwrap();
        for &(ts, context, category, level, value) in entries {
            conn.execute(
                "INSERT INTO plc_log(ts,context,category,level,new_value,point_id,message)
                VALUES(?1,?2,?3,?4,?5,CASE WHEN ?3='value' THEN 'test-point' END,'')",
                params![ts, context, category, level, value],
            )
            .unwrap();
        }
        let plc = PlcHost {
            engine: ly_plc::PlcEngine::new(
                crate::inspection::default_plc_config(),
                Arc::new(book),
                sink,
            ),
            config_path: dir.join("plc.json"),
            log_path: path,
        };
        (dir, plc)
    }

    async fn query_groups(plc: &PlcHost, query: Value) -> Value {
        let page = plc_query_log_groups(plc, serde_json::from_value(query).unwrap())
            .await
            .unwrap();
        serde_json::to_value(page).unwrap()
    }

    async fn detail_ids(plc: &PlcHost, group: &Value) -> Vec<i64> {
        let selection = serde_json::from_value(group.clone()).unwrap();
        plc_query_group_logs(plc, selection, LogQuery::default())
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|row| row.id)
            .collect()
    }

    #[tokio::test]
    async fn reused_sequence_keeps_early_signals_and_delayed_failure_with_their_own_piece() {
        let seq = Some("partSeq=12");
        let (_dir, plc) = fixture(&[
            (
                10,
                seq,
                "part",
                "info",
                r#"{"event":"ended","receivedAt":3}"#,
            ),
            (11, seq, "value", "info", "old"),
            (12, seq, "value", "info", "new"),
            (12, None, "connection", "warn", ""),
            (
                13,
                seq,
                "part",
                "info",
                r#"{"event":"received","receivedAt":13,"observedAt":12,"sn":"B123"}"#,
            ),
            (
                14,
                seq,
                "part",
                "error",
                r#"{"event":"resultFailed","receivedAt":3,"sn":"A123"}"#,
            ),
            (
                15,
                seq,
                "part",
                "info",
                r#"{"event":"resultWritten","receivedAt":13,"sn":"B123","resultCode":1,"faultCode":0}"#,
            ),
        ]);
        let result = query_groups(&plc, json!({"start":10,"end":15,"keyword":"b12"})).await;
        assert_eq!(result["total"], 1);
        let piece = &result["items"][0];
        assert_eq!(piece["event"], "resultWritten");
        assert_eq!(piece["resultCode"], 1);
        assert_eq!(piece["entryCount"], 3);
        assert_eq!(detail_ids(&plc, piece).await, [3, 5, 7]);
        let trend = crate::plc::plc_point_history(
            &plc,
            "test-point".into(),
            0,
            20,
            Some(serde_json::from_value(piece.clone()).unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(
            trend.iter().map(|v| v.value.as_deref()).collect::<Vec<_>>(),
            [Some("new")]
        );
        let result = query_groups(
            &plc,
            json!({"start":10,"end":15,"abnormalOnly":true,"limit":1}),
        )
        .await;
        assert_eq!(result["total"], 2);
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        let old = &result["items"][0];
        assert_eq!(old["sn"], "A123");
        assert_eq!(old["event"], "resultFailed");
        assert_eq!(detail_ids(&plc, old).await, [1, 2, 6]);
    }

    #[tokio::test]
    async fn legacy_contexts_and_unassociated_records_remain_browsable_within_window() {
        let old = Some("requestSeq=34");
        let (_dir, plc) = fixture(&[
            (1, old, "value", "info", "outside"),
            (2, old, "value", "info", "first"),
            (2, None, "connection", "info", ""),
            (3, old, "value", "info", "second"),
            (4, Some("requestSeq=0"), "value", "info", "zero"),
            (5, old, "value", "info", "reused"),
        ]);
        let result = query_groups(&plc, json!({"start":2,"end":5})).await;
        assert_eq!(result["total"], 3);
        let mut details = Vec::new();
        for group in result["items"].as_array().unwrap() {
            assert!(group["event"].is_null());
            details.push((group["partSeq"].as_i64(), detail_ids(&plc, group).await));
        }
        assert_eq!(
            details,
            [
                (Some(34), vec![6]),
                (None, vec![3, 5]),
                (Some(34), vec![2, 4]),
            ]
        );
    }
}
