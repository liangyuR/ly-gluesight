//! 进程在 S7 事务中退出：握手日志留着在途工件，历史却没有它。启动时按握手身份补一条 ERR 记录，
//! 场景与配方取本件 partStart 之后的运行日志；日志缺失时按真机记，原因里写明。

use std::sync::Arc;

use crate::applog::Record;
use crate::audit::RecordedPart;
use crate::judge::{fault, Judgement};
use crate::recipe::Recipe;
use crate::store::{PlcDelivery, PlcDeliveryState, RecordSource};

pub(super) struct Interrupted<'a> {
    pub cycle_id: &'a str,
    pub sn: &'a str,
    pub request_seq: u32,
    pub started_at: Option<i64>,
}

pub(super) fn part(interrupted: &Interrupted, records: &[Record], recipe: impl Fn(&str) -> Option<Arc<Recipe>>, software_version: String, now: i64) -> RecordedPart {
    let own = || records.iter().filter(|r| r.fields.cycle_id.as_deref() == Some(interrupted.cycle_id));
    let start = own().find(|r| r.fields.stage.as_deref() == Some("partStart"));
    let source = start.and_then(|r| r.fields.data.as_ref()?.get("source").cloned())
        .and_then(|value| serde_json::from_value::<RecordSource>(value).ok());
    let recipe = own().find_map(|r| r.fields.recipe_id.as_deref()).and_then(recipe);
    let mut reason = format!("检测进程在事务中中断（请求序号 {}），本件未完成检测，结果未回写 PLC", interrupted.request_seq);
    if source.is_none() { reason.push_str("；未找到本件开工日志，场景按真机记录"); }
    let judgement = Judgement::error(fault::DEVICE_LOST, reason.clone());
    RecordedPart {
        ts: start.map(|r| r.ts).or(interrupted.started_at).unwrap_or(now),
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
        source: source.unwrap_or_default(),
        sample: None,
    }
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

    fn interrupted() -> Interrupted<'static> {
        Interrupted { cycle_id: CYCLE, sn: "OFFLINE-0000000007", request_seq: 7, started_at: Some(900) }
    }

    #[test]
    fn interrupted_part_keeps_the_site_and_recipe_logged_by_its_own_start_and_is_a_failed_delivery_err() {
        let recipe = Arc::new(crate::recipe::samples().remove(1).build().unwrap());
        let lookup = |id: &str| (id == recipe.id).then(|| recipe.clone());
        let interrupted = interrupted();
        let logs = [
            record(1000, CYCLE, Fields::stage("partStart").with_data(serde_json::json!({"source": "offlineSample"}))),
            record(1010, "fedcba9876543210fedcba9876543210", Fields { recipe_id: Some("其他件的配方".into()), ..Fields::default() }),
            record(1020, CYCLE, Fields { recipe_id: Some(recipe.id.clone()), ..Fields::default() }),
        ];
        let part = part(&interrupted, &logs, lookup, "1.0".into(), 5000);
        assert_eq!((part.source, part.ts, part.sn.as_str()), (RecordSource::OfflineSample, 1000, "OFFLINE-0000000007"), "离线件不能计入真机统计");
        assert_eq!((part.judgement.verdict, part.judgement.fault_code), (Verdict::ErrInspect, fault::DEVICE_LOST));
        assert!(part.judgement.reason.contains("进程在事务中中断"), "{}", part.judgement.reason);
        assert_eq!(part.recipe.as_ref().map(|r| r.id.as_str()), Some(recipe.id.as_str()));
        assert_eq!(part.shots.len(), recipe.shot_count());
        assert_eq!(part.delivery.state, PlcDeliveryState::Failed, "补记录的交付不能留在待确认，否则启动 ACK 恢复会把它当未结件");
    }

    #[test]
    fn interrupted_part_without_a_start_log_is_recorded_as_real_and_says_so() {
        let part = part(&interrupted(), &[], |_| None, "1.0".into(), 5000);
        assert_eq!((part.source, part.ts), (RecordSource::Real, 900));
        assert!(part.judgement.reason.contains("场景按真机记录"), "{}", part.judgement.reason);
    }
}
