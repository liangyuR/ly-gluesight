//! 原图录制失败告警：录制失败不停产、不改判定，但操作员要能看到原图没存下来。
//! 真机与离线各自计数；下一件录制成功、按策略无需留图且无错误、或录制关闭时清除。

use std::sync::Mutex;

use serde::Serialize;

use crate::recorder::{RecordingOutcome, RecordingState};
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

fn failure(outcome: &RecordingOutcome) -> Option<String> {
    if outcome.errors.is_empty() && !matches!(outcome.state, RecordingState::Failed | RecordingState::Incomplete) {
        return None;
    }
    Some(match outcome.errors.as_slice() {
        [] => format!("录制状态 {:?}", outcome.state),
        [only] => only.clone(),
        [first, rest @ ..] => format!("{first}（另有 {} 项）", rest.len()),
    })
}

impl RecordingHealth {
    pub fn observe(&self, scope: HistoryScope, outcome: &RecordingOutcome, now: i64) {
        let mut scopes = self.scopes.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = &mut scopes[slot(scope)];
        *current = failure(outcome).map(|reason| {
            let (consecutive, since) = current.as_ref().map_or((1, now), |alarm| (alarm.consecutive.saturating_add(1), alarm.since));
            RecordingAlarm { consecutive, cycle_id: outcome.cycle_id.clone(), reason, since }
        });
    }

    pub fn alarm(&self, scope: HistoryScope) -> Option<RecordingAlarm> {
        self.scopes.lock().unwrap_or_else(|poisoned| poisoned.into_inner())[slot(scope)].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(cycle: &str, state: RecordingState, errors: &[&str]) -> RecordingOutcome {
        RecordingOutcome { cycle_id: cycle.into(), directory: None, files: Vec::new(), errors: errors.iter().map(|e| e.to_string()).collect(),
            retention_errors: Vec::new(), available: state == RecordingState::Complete, state }
    }

    #[test]
    fn consecutive_failures_accumulate_per_recording_directory_keeping_first_time_and_latest_cause() {
        let health = RecordingHealth::default();
        health.observe(HistoryScope::Real, &outcome("c1", RecordingState::Failed, &["k000_P1_cam1_v1.pgm：创建原图失败：拒绝访问"]), 100);
        health.observe(HistoryScope::Real, &outcome("c2", RecordingState::NotRetained, &["k000_P1_cam1_v1.pgm：创建原图失败：拒绝访问", "本件没有已确认写入的原图"]), 200);
        let alarm = health.alarm(HistoryScope::Real).expect("只留 NG 件时合格件写原图失败也算录制失败");
        assert_eq!((alarm.consecutive, alarm.cycle_id.as_str(), alarm.since), (2, "c2", 100));
        assert!(alarm.reason.contains("拒绝访问") && alarm.reason.contains("另有 1 项"), "{}", alarm.reason);
        assert_eq!(health.alarm(HistoryScope::Offline), None, "真机录制失败不能在离线场景报警");
        health.observe(HistoryScope::Debug, &outcome("c3", RecordingState::Incomplete, &[]), 300);
        assert_eq!(health.alarm(HistoryScope::Offline).map(|a| a.consecutive), Some(1), "候选调试与离线共用离线录制目录");
    }

    #[test]
    fn alarm_clears_once_a_part_is_recorded_unneeded_or_recording_is_off() {
        for state in [RecordingState::Complete, RecordingState::NotRetained, RecordingState::Off] {
            let health = RecordingHealth::default();
            health.observe(HistoryScope::Real, &outcome("bad", RecordingState::Failed, &["创建原图失败：磁盘已满"]), 100);
            assert!(health.alarm(HistoryScope::Real).is_some());
            health.observe(HistoryScope::Real, &outcome("next", state, &[]), 200);
            assert_eq!(health.alarm(HistoryScope::Real), None, "{state:?}");
        }
    }
}
