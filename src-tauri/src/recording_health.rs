//! 原图录制失败告警：录制失败不停产、不改判定，但操作员要能看到原图没存下来。
//! 真机与离线各自计数；下一件录制成功、按策略无需留图或录制关闭时清除。
//! 只有写盘错误算失败：采集前校验不过（少视角、重复拍照点）、队列满、相机少帧都不是资料盘问题，不计入；
//! 没有任何原图写成的件没有写盘证据，既不报警也不清除告警，已有原图写成则视为写盘正常。

use std::sync::Mutex;

use serde::Serialize;

use crate::recorder::{is_write_error, RecordingOutcome, RecordingState};
use crate::store::HistoryScope;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingAlarm {
    pub consecutive: u32,
    pub cycle_id: String,
    pub reason: String,
    pub since: i64,
}

impl RecordingAlarm {
    pub fn message(&self) -> String {
        format!("原图录制失败：连续 {} 件原图未完整保存（最近 {}：{}）；检测与判定照常，请检查资料盘空间与录制目录写权限",
            self.consecutive, self.cycle_id, self.reason)
    }
}

#[derive(Default)]
pub struct RecordingHealth {
    scopes: Mutex<[Option<RecordingAlarm>; 2]>,
}

fn slot(scope: HistoryScope) -> usize {
    match scope {
        HistoryScope::Real => 0,
        HistoryScope::Offline | HistoryScope::Debug => 1,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AlarmChange {
    Raised(RecordingAlarm),
    Cleared,
}

enum Evidence {
    Failed(String),
    Healthy,
    Inconclusive,
}

fn evidence(outcome: &RecordingOutcome) -> Evidence {
    let unsaved = matches!(outcome.state, RecordingState::Failed | RecordingState::Incomplete);
    let write_errors: Vec<&String> = outcome.errors.iter().filter(|error| is_write_error(error)).collect();
    match write_errors.as_slice() {
        [] if outcome.errors.is_empty() && unsaved => Evidence::Failed(format!("录制状态 {:?}", outcome.state)),
        [] if unsaved && outcome.files.is_empty() => Evidence::Inconclusive,
        [] => Evidence::Healthy,
        [only] => Evidence::Failed((*only).clone()),
        [first, rest @ ..] => Evidence::Failed(format!("{first}（另有 {} 项）", rest.len())),
    }
}

impl RecordingHealth {
    pub fn observe(&self, scope: HistoryScope, outcome: &RecordingOutcome, now: i64) -> Option<AlarmChange> {
        let mut scopes = self.scopes.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = &mut scopes[slot(scope)];
        match evidence(outcome) {
            Evidence::Failed(reason) => {
                let (consecutive, since) = current.as_ref().map_or((1, now), |alarm| (alarm.consecutive.saturating_add(1), alarm.since));
                let raised = current.is_none();
                let alarm = RecordingAlarm { consecutive, cycle_id: outcome.cycle_id.clone(), reason, since };
                *current = Some(alarm.clone());
                raised.then_some(AlarmChange::Raised(alarm))
            }
            Evidence::Healthy => current.take().map(|_| AlarmChange::Cleared),
            Evidence::Inconclusive => None,
        }
    }

    pub fn alarm(&self, scope: HistoryScope) -> Option<RecordingAlarm> {
        self.scopes.lock().unwrap_or_else(|poisoned| poisoned.into_inner())[slot(scope)].clone()
    }
}
