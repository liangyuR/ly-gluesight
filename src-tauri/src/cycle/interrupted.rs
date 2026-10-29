//! 进程在 S7 事务中退出：握手日志留着在途工件，历史却没有它。启动时按握手日志里的身份与开工场景补一条 ERR 记录，
//! 配方取本件的运行日志（运行日志不落盘同步，缺失时本件不带配方）。补记录带预期帧组数 ΣN_exp 与容差、没有帧，
//! 区域判定同为 ERR 98。

use std::sync::Arc;

use crate::applog::Record;
use crate::audit::RecordedPart;
use crate::judge::{fault, Judgement};
use crate::plc_session::PartSite;
use crate::recipe::Recipe;
use crate::recorder::{RecordingOutcome, RecordingState};
use crate::settings::RecordMode;
use crate::store::{HistoryScope, PartMode, PlcDelivery, PlcDeliveryState, RecordSource};

pub(super) struct Interrupted<'a> {
    pub cycle_id: &'a str,
    pub sn: &'a str,
    pub request_seq: u32,
    pub started_at: Option<i64>,
    pub site: PartSite,
    pub maintenance: bool,
}

pub(super) fn part(interrupted: &Interrupted, records: &[Record], recipe: impl Fn(&str) -> Option<Arc<Recipe>>, software_version: String, now: i64) -> RecordedPart {
    let recipe = records.iter()
        .filter(|r| r.fields.cycle_id.as_deref() == Some(interrupted.cycle_id))
        .find_map(|r| r.fields.recipe_id.as_deref())
        .and_then(recipe);
    let reason = format!("检测进程在事务中中断（请求序号 {}），本件未完成检测，结果未回写 PLC", interrupted.request_seq);
    let judgement = Judgement::error(fault::DEVICE_LOST, reason);
    let burst = recipe
        .as_deref()
        .map(|recipe| super::burst::empty_record(recipe, &judgement));
    let mode = match (
        interrupted.maintenance,
        recipe.as_deref().is_some_and(Recipe::is_collect),
    ) {
        (false, _) => PartMode::Production,
        (true, true) => PartMode::MaintenanceCollect,
        (true, false) => PartMode::MaintenanceShadow,
    };
    RecordedPart {
        mode,
        ts: interrupted.started_at.unwrap_or(now),
        sn: interrupted.sn.into(),
        frames_expected: burst
            .as_ref()
            .map_or(0, |burst| burst.expected_total.unwrap_or(0) as usize),
        shots: Vec::new(),
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
