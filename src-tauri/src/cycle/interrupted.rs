//! 进程在 S7 事务中退出：握手日志留着在途工件，历史却没有它。启动时按握手日志里的身份与开工场景补一条 ERR 记录，
//! 配方取本件的运行日志（运行日志不落盘同步，缺失时本件不带配方）。段配方的补记录带预期帧组数 ΣN_exp 与容差、没有帧，
//! 区域判定同为 ERR 98；旧拍照点配方仍按计划拍照点逐个记缺帧。

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
    let burst = recipe.as_deref().and_then(|recipe| super::burst::empty_record(recipe, &judgement));
    RecordedPart {
        ts: interrupted.started_at.unwrap_or(now),
        sn: interrupted.sn.into(),
        frames_expected: match &burst { Some(burst) => burst.expected_total.unwrap_or(0) as usize, None => recipe.as_ref().map_or(0, |r| r.shot_count()) },
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
        burst,
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

    /// 中断件补记录（段配方）：带 ΣN_exp 与容差、没有帧、区域判定与摘要同为 ERR 98，按 C5 的记录约束能落库
    #[test]
    fn interrupted_part_record_carries_plan_totals_and_err_98() {
        let recipe = Arc::new(crate::recipe::samples_burst().build().unwrap());
        let lookup = |id: &str| (id == recipe.id).then(|| recipe.clone());
        let logs = [record(1020, CYCLE, Fields { recipe_id: Some(recipe.id.clone()), ..Fields::default() })];
        let part = part(&interrupted(RecordSource::Real), &logs, lookup, "1.0".into(), 5000);
        let burst = part.burst.as_ref().expect("段配方补记录带帧记录");
        assert_eq!((part.frames_expected, burst.expected_total, burst.tolerance_total), (4, Some(4), 1));
        assert!(burst.frames.is_empty());
        assert_eq!((burst.judgement.verdict, burst.judgement.fault_code), (Verdict::ErrInspect, fault::DEVICE_LOST));
        let directory = crate::offline::test_dir::TestDir::new("cycle-burst-tests", "interrupted");
        let store = crate::store::Store::open(&directory.join("inspection.db")).unwrap();
        store.insert(&crate::store::PartRecord { ts: part.ts, sn: &part.sn, recipe: part.recipe.as_deref(), judgement: &part.judgement,
            drain_ms: None, frames: &part.frames, frames_expected: part.frames_expected, frames_received: part.frames_received, triggers: part.triggers,
            table: None, software_version: &part.software_version, cycle_id: Some(&part.cycle_id), bundle_id: None, delivery: &part.delivery,
            shots: &part.shots, source: part.source, sample: None, burst: part.burst.as_ref() }).unwrap();
    }

}
