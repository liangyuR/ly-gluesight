//! 进程在 S7 事务中退出：握手日志留着在途工件，历史却没有它。启动时按握手日志里的身份与开工场景补一条 ERR 记录，
//! 配方取本件的运行日志（运行日志不落盘同步，缺失时本件不带配方）。

use std::sync::Arc;

use crate::applog::Record;
use crate::audit::RecordedPart;
use crate::judge::{fault, Judgement};
use crate::plc_session::PartSite;
use crate::recipe::Recipe;
use crate::recorder::{RecordingOutcome, RecordingState};
use crate::settings::RecordMode;
use crate::store::{HistoryScope, PlcDelivery, PlcDeliveryState, RecordSource};

pub(super) struct Interrupted<'a> {
    pub cycle_id: &'a str,
    pub sn: &'a str,
    pub request_seq: u32,
    pub started_at: Option<i64>,
    pub site: PartSite,
}

pub(super) fn part(interrupted: &Interrupted, records: &[Record], recipe: impl Fn(&str) -> Option<Arc<Recipe>>, software_version: String, now: i64) -> RecordedPart {
    let recipe = records.iter()
        .filter(|r| r.fields.cycle_id.as_deref() == Some(interrupted.cycle_id))
        .find_map(|r| r.fields.recipe_id.as_deref())
        .and_then(recipe);
    let reason = format!("检测进程在事务中中断（请求序号 {}），本件未完成检测，结果未回写 PLC", interrupted.request_seq);
    let judgement = Judgement::error(fault::DEVICE_LOST, reason.clone());
    RecordedPart {
        ts: interrupted.started_at.unwrap_or(now),
        sn: interrupted.sn.into(),
        frames_expected: recipe.as_ref().map_or(0, |r| r.shot_count()),
        shots: super::recorded_shots(recipe.as_deref(), &[], &reason),
        recipe,
        judgement,
        drain_ms: None,
        frames: Vec::new(),
        frames_received: 0,
        triggers: 0,
        table: None,
        software_version,
        cycle_id: interrupted.cycle_id.into(),
        bundle_id: None,
        delivery: PlcDelivery { state: PlcDeliveryState::Failed, updated_at: now, message: Some("检测进程中断，结果未回写 PLC".into()) },
        source: interrupted.site.source,
        sample: None,
    }
}

pub(super) fn recording(cycle_id: &str, record: RecordMode) -> RecordingOutcome {
    let off = record == RecordMode::Off;
    RecordingOutcome {
        cycle_id: cycle_id.into(),
        directory: None,
        files: Vec::new(),
        errors: if off { Vec::new() } else { vec!["检测进程在事务中中断，本件原图未收尾".into()] },
        retention_errors: Vec::new(),
        available: false,
        state: if off { RecordingState::Off } else { RecordingState::Failed },
    }
}

pub(super) fn counts_in_stats(source: RecordSource, ts: i64, scope: HistoryScope, midnight: i64) -> bool {
    HistoryScope::of(source) == scope && ts >= midnight
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::applog::{Fields, Level};
    use crate::judge::Verdict;

    const CYCLE: &str = "0123456789abcdef0123456789abcdef";

    fn record(ts: i64, cycle: &str, fields: Fields) -> Record {
        Record { ts, level: Level::Info, ev: "事件".into(), msg: String::new(), fields: Fields { cycle_id: Some(cycle.into()), ..fields } }
    }

    fn interrupted(source: RecordSource) -> Interrupted<'static> {
        Interrupted { cycle_id: CYCLE, sn: "OFFLINE-0000000007", request_seq: 7, started_at: Some(900), site: PartSite { source, record: RecordMode::All } }
    }

    #[test]
    fn interrupted_part_keeps_the_journal_site_and_the_recipe_logged_by_its_own_cycle_and_is_a_failed_delivery_err() {
        let recipe = Arc::new(crate::recipe::samples().remove(1).build().unwrap());
        let lookup = |id: &str| (id == recipe.id).then(|| recipe.clone());
        let logs = [
            record(1010, "fedcba9876543210fedcba9876543210", Fields { recipe_id: Some("其他件的配方".into()), ..Fields::default() }),
            record(1020, CYCLE, Fields { recipe_id: Some(recipe.id.clone()), ..Fields::default() }),
        ];
        let part = part(&interrupted(RecordSource::OfflineSample), &logs, lookup, "1.0".into(), 5000);
        assert_eq!((part.source, part.ts, part.sn.as_str()), (RecordSource::OfflineSample, 900, "OFFLINE-0000000007"), "离线件不能计入真机统计");
        assert_eq!((part.judgement.verdict, part.judgement.fault_code), (Verdict::ErrInspect, fault::DEVICE_LOST));
        assert!(part.judgement.reason.contains("进程在事务中中断"), "{}", part.judgement.reason);
        assert_eq!(part.recipe.as_ref().map(|r| r.id.as_str()), Some(recipe.id.as_str()));
        assert_eq!(part.shots.len(), recipe.shot_count());
        assert_eq!(part.delivery.state, PlcDeliveryState::Failed, "补记录的交付不能留在待确认，否则启动 ACK 恢复会把它当未结件");
    }

    #[test]
    fn recording_is_off_without_errors_when_the_site_was_not_recording_and_failed_otherwise() {
        for (record, state, has_errors) in [
            (RecordMode::Off, RecordingState::Off, false),
            (RecordMode::Failed, RecordingState::Failed, true),
            (RecordMode::All, RecordingState::Failed, true),
        ] {
            let outcome = recording(CYCLE, record);
            assert_eq!((outcome.state, !outcome.errors.is_empty()), (state, has_errors), "{record:?}");
        }
    }

    #[test]
    fn interrupted_err_joins_today_stats_only_for_the_stats_scope_and_after_midnight() {
        for (source, ts, counted) in [
            (RecordSource::Real, 1500, true),
            (RecordSource::OfflineSynthetic, 1500, false),
            (RecordSource::Real, 900, false),
        ] {
            assert_eq!(counts_in_stats(source, ts, HistoryScope::Real, 1000), counted, "{source:?} {ts}");
        }
    }
}
