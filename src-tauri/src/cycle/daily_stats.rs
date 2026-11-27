use chrono::{Local, NaiveDate, TimeZone};

use crate::judge::Verdict;
use crate::store::PartMode;

use super::Stats;

pub(super) struct DailyStats {
    day: NaiveDate,
    counts: Stats,
}

impl DailyStats {
    pub(super) fn new(day: NaiveDate, counts: Stats) -> Self {
        Self { day, counts }
    }

    pub(super) fn advance(&mut self, day: NaiveDate) -> bool {
        if self.day == day {
            return false;
        }
        self.day = day;
        self.counts = Stats::default();
        true
    }

    pub(super) fn record(&mut self, mode: PartMode, ts: i64, verdict: Verdict) -> bool {
        if mode != PartMode::Production
            || !Local
                .timestamp_millis_opt(ts)
                .single()
                .is_some_and(|at| at.date_naive() == self.day)
        {
            return false;
        }
        self.counts.total += 1;
        match verdict {
            Verdict::Ok | Verdict::OkWithExcursion => self.counts.ok += 1,
            Verdict::ErrInspect => self.counts.err += 1,
            _ => self.counts.ng += 1,
        }
        true
    }

    pub(super) fn snapshot(&self) -> Stats {
        self.counts.clone()
    }
}
