//! 高频重复事件的聚合（docs/architecture/agent-logging.md 第 6 节）：按 (摘要编码, key) 计数，
//! 窗口内首次出现由调用方照写原事件，之后只计数；窗口结束（`flush`）或调用方声明状态结束（`end`）时给出一条摘要。
//! 时间（毫秒）由调用方传入，节拍每拍调用一次 `flush`。

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Map, Value};

use super::codes::Event;
use super::Fields;

pub const WINDOW: Duration = Duration::from_secs(10);

/// 摘要里随计数一起给出的样本量。
#[derive(Clone, Copy, Debug)]
pub enum Tally {
    None,
    /// 样本范围：摘要写 `{名}Min`、`{名}Max`
    Range(&'static str, i64),
    /// 累加量：摘要写 `{名}` 为计入的合计
    Sum(&'static str, u64),
}

#[derive(Clone, Copy, Debug)]
enum Acc {
    Range(&'static str, i64, i64),
    Sum(&'static str, u64),
}

struct Entry {
    summary: &'static Event,
    opened: i64,
    count: u64,
    first_ts: i64,
    last_ts: i64,
    acc: Option<Acc>,
    fields: Fields,
}

impl Entry {
    fn take(&mut self, key: &str) -> Option<Summary> {
        if self.count == 0 {
            return None;
        }
        let mut data = match self.fields.data.take() {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        data.insert("count".into(), json!(self.count));
        data.insert("firstTs".into(), json!(self.first_ts));
        data.insert("lastTs".into(), json!(self.last_ts));
        match self.acc.take() {
            Some(Acc::Range(name, min, max)) => {
                data.remove(name);
                data.insert(format!("{name}Min"), json!(min));
                data.insert(format!("{name}Max"), json!(max));
            }
            Some(Acc::Sum(name, sum)) => {
                data.insert(name.into(), json!(sum));
            }
            None => {}
        }
        let span = (self.last_ts - self.first_ts) as f64 / 1000.0;
        let who = if key.is_empty() {
            String::new()
        } else {
            format!("{key}：")
        };
        let summary = Summary {
            event: self.summary,
            msg: format!("{who}{span:.1} s 内又出现 {} 次", self.count),
            fields: std::mem::take(&mut self.fields).with_data(Value::Object(data)),
        };
        self.count = 0;
        Some(summary)
    }
}

/// 一条待写的摘要：`fields` 沿用最近一次计入的关联字段，`data` 为最近一次的 data 加上计数与样本量。
#[derive(Debug)]
pub struct Summary {
    pub event: &'static Event,
    pub msg: String,
    pub fields: Fields,
}

pub struct Throttle {
    window: i64,
    entries: HashMap<(&'static str, String), Entry>,
}

impl Throttle {
    pub fn new(window: Duration) -> Self {
        Self {
            window: window.as_millis() as i64,
            entries: HashMap::new(),
        }
    }

    /// 记一次出现。返回 true 表示窗口内首次出现，调用方照写原事件；否则已计入，等摘要。
    /// `fields.data` 应与原事件的 data 相同，摘要沿用最近一次的值。
    pub fn observe(
        &mut self,
        summary: &'static Event,
        key: &str,
        now: i64,
        tally: Tally,
        fields: Fields,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(&(summary.code, key.to_owned())) else {
            let entry = Entry {
                summary,
                opened: now,
                count: 0,
                first_ts: now,
                last_ts: now,
                acc: None,
                fields: Fields::default(),
            };
            self.entries.insert((summary.code, key.to_owned()), entry);
            return true;
        };
        if entry.count == 0 {
            entry.first_ts = now;
        }
        entry.count += 1;
        entry.last_ts = now;
        entry.fields = fields;
        entry.acc = match (entry.acc, tally) {
            (Some(Acc::Range(_, min, max)), Tally::Range(name, v)) => {
                Some(Acc::Range(name, min.min(v), max.max(v)))
            }
            (_, Tally::Range(name, v)) => Some(Acc::Range(name, v, v)),
            (Some(Acc::Sum(_, sum)), Tally::Sum(name, v)) => Some(Acc::Sum(name, sum + v)),
            (_, Tally::Sum(name, v)) => Some(Acc::Sum(name, v)),
            (_, Tally::None) => None,
        };
        false
    }

    /// 到期的窗口：有计数的给出摘要并开始下一窗口（持续刷屏时只出摘要）；窗口内无新出现的结束，之后再出现重新照写原事件。
    pub fn flush(&mut self, now: i64) -> Vec<Summary> {
        let window = self.window;
        let mut out = Vec::new();
        self.entries.retain(|(_, key), entry| {
            if now - entry.opened < window {
                return true;
            }
            match entry.take(key) {
                Some(summary) => {
                    out.push(summary);
                    entry.opened = now;
                    true
                }
                None => false,
            }
        });
        out
    }

    /// 调用方声明这一路状态结束：立即给出未写的摘要，之后再出现重新照写原事件。
    pub fn end(&mut self, summary: &'static Event, key: &str) -> Option<Summary> {
        self.entries
            .remove(&(summary.code, key.to_owned()))
            .and_then(|mut entry| entry.take(key))
    }

    /// 同一摘要编码下所有 key 的状态结束。
    pub fn end_all(&mut self, summary: &'static Event) -> Vec<Summary> {
        let keys: Vec<String> = self
            .entries
            .keys()
            .filter(|(code, _)| *code == summary.code)
            .map(|(_, key)| key.clone())
            .collect();
        keys.iter()
            .filter_map(|key| self.end(summary, key))
            .collect()
    }

    /// 全部状态结束（退出前尽力写出）。
    pub fn drain(&mut self) -> Vec<Summary> {
        self.entries
            .drain()
            .filter_map(|((_, key), mut entry)| entry.take(&key))
            .collect()
    }
}
