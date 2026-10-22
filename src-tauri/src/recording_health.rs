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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::RecordedRawFile;

    fn outcome(cycle: &str, state: RecordingState, errors: &[&str]) -> RecordingOutcome {
        RecordingOutcome { cycle_id: cycle.into(), directory: None, files: Vec::new(), errors: errors.iter().map(|e| e.to_string()).collect(),
            retention_errors: Vec::new(), available: state == RecordingState::Complete, state }
    }

    #[test]
    fn consecutive_failures_accumulate_per_recording_directory_keeping_first_time_and_latest_cause() {
        let health = RecordingHealth::default();
        health.observe(HistoryScope::Real, &outcome("c1", RecordingState::Failed, &["k000_P1_cam1_v1.pgm：创建原图失败：拒绝访问"]), 100);
        health.observe(HistoryScope::Real, &outcome("c2", RecordingState::NotRetained, &["k000_P1_cam1_v1.pgm：创建原图失败：拒绝访问", "k001_P2_cam1_v1.pgm：创建原图失败：拒绝访问"]), 200);
        let alarm = health.alarm(HistoryScope::Real).expect("只留 NG 件时合格件写原图失败也算录制失败");
        assert_eq!((alarm.consecutive, alarm.cycle_id.as_str(), alarm.since), (2, "c2", 100));
        assert!(alarm.reason.contains("拒绝访问") && alarm.reason.contains("另有 1 项"), "{}", alarm.reason);
        assert_eq!(health.alarm(HistoryScope::Offline), None, "真机录制失败不能在离线场景报警");
        health.observe(HistoryScope::Debug, &outcome("c3", RecordingState::Incomplete, &[]), 300);
        assert_eq!(health.alarm(HistoryScope::Offline).map(|a| a.consecutive), Some(1), "候选调试与离线共用离线录制目录");
    }

    #[test]
    fn part_without_any_written_original_and_without_write_errors_is_not_a_recording_failure_nor_a_recovery() {
        let aborted = || outcome("aborted", RecordingState::Failed, &["6 个计划拍照点没有录制原图", "本件没有已确认写入的原图"]);
        let health = RecordingHealth::default();
        health.observe(HistoryScope::Real, &aborted(), 100);
        assert_eq!(health.alarm(HistoryScope::Real), None, "采集前中止的件没有帧交给录制，不是写盘失败");
        health.observe(HistoryScope::Real, &outcome("bad", RecordingState::Failed, &["创建原图失败：磁盘已满"]), 200);
        health.observe(HistoryScope::Real, &aborted(), 300);
        assert_eq!(health.alarm(HistoryScope::Real).map(|a| (a.consecutive, a.cycle_id)), Some((1, "bad".to_string())), "中止件不是写盘成功的证据，不能清除告警");
        let mut short = outcome("short", RecordingState::Incomplete, &["1 个计划拍照点没有录制原图"]);
        short.files = vec![RecordedRawFile { k: 0, view: 1, file: "k000.pgm".into(), width: 1, height: 1, frame: None }];
        health.observe(HistoryScope::Real, &short, 400);
        assert_eq!(health.alarm(HistoryScope::Real), None, "少了几帧但其余原图已写成，写盘正常");
    }

    #[test]
    fn capture_validation_error_on_a_part_that_needs_no_originals_is_not_a_recording_failure() {
        let health = RecordingHealth::default();
        let invalid = outcome("ok-part", RecordingState::NotRetained, &["拍照点 P1 原图不完整：实际 1 个视角，所选视角 2"]);
        assert_eq!(health.observe(HistoryScope::Real, &invalid, 100), None);
        assert_eq!(health.alarm(HistoryScope::Real), None, "磁盘与权限正常，按策略也不需要留图，不能提示检查资料盘");
    }

    #[test]
    fn observe_reports_a_change_only_when_a_slot_raises_or_clears_its_alarm() {
        let health = RecordingHealth::default();
        let failed = |cycle| outcome(cycle, RecordingState::Failed, &["创建原图失败：拒绝访问"]);
        let complete = |cycle| outcome(cycle, RecordingState::Complete, &[]);
        let raised = health.observe(HistoryScope::Real, &failed("c1"), 100);
        assert!(matches!(raised, Some(AlarmChange::Raised(ref alarm)) if alarm.cycle_id == "c1"), "{raised:?}");
        assert_eq!(health.observe(HistoryScope::Real, &failed("c2"), 200), None, "连续失败不重复报告出现");
        assert_eq!(health.observe(HistoryScope::Offline, &complete("o1"), 300), None, "离线录制成功不能报真机恢复");
        assert_eq!(health.observe(HistoryScope::Real, &complete("c3"), 400), Some(AlarmChange::Cleared));
        assert_eq!(health.observe(HistoryScope::Real, &complete("c4"), 500), None, "没有告警时不报告恢复");
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
