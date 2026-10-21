//! 连续定位失败告警（定稿 §3、§8 第 5 项）：同一（配方修订、拍照点、图）连续 3 件没有位置基准时告警，只进界面与日志，
//! v1 不上 PLC 故障码。每件定位失败本身已是 ST_NO_REF（判 ERR 92 或让位于 NG），不会被悄悄放行；告警只是把“连续”汇总出来。
//!
//! - 证据只认帧记录 `FrameView`，与帧徽标同一来源：测完（Done）且该图在 `reference_failed` 里算失败（定位失败、两端搬移超限同样计数），
//!   测完且不在里面算定位成功（清零并清除告警）；帧测量出错、该图单独出错、缺帧、超时都没有证据，计数不变也不清除。
//!   没有特征定位的图在生产里不会没有基准，对它判“成功”只是空操作；离线 `LocateFail` 不带 fixture 也能演练告警与恢复。
//! - 不同配方各自计数，别的配方的件不打断连续，换配方也不清除：换到别的配方后旧配方的告警留到它再定位成功、换修订或重启
//!   （检测页横幅写明配方并标“当前未在生产”）。同一配方的修订变了（新发布、候选修订）时，删掉该配方其他修订的条目（含已告警的），
//!   新模板从 0 开始计。
//! - 只在内存：重启清零（与录制告警一致），复位、站点切换不清除；真机与离线 / 候选调试分槽，离线演练不污染真机。

use std::collections::BTreeMap;

use serde::Serialize;

use crate::cycle::{FrameStatus, FrameView};
use crate::store::HistoryScope;

/// 连续多少件没有位置基准时告警（定稿 §8 第 5 项，N = 3）。
pub const LOCATE_ALARM_PARTS: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocateAlarm {
    pub recipe_id: String,
    pub revision_id: String,
    pub shot_id: String,
    pub k: usize,
    /// 该拍照点的相机：日志按条目自己的拍照点记相机，不用触发这次变化的那一帧的（作废时那一帧可能是别的拍照点）
    #[serde(skip)]
    pub camera: String,
    pub view: u8,
    pub consecutive: u32,
    /// 最近一件的原因（帧徽标原因：定位失败 / 两端搬移超限）
    pub reason: String,
    pub cycle_id: String,
    /// 这一串连续失败的第一件的时间
    pub since: i64,
}

impl LocateAlarm {
    pub fn message(&self) -> String {
        format!("特征定位连续失败：配方 {} 拍照点 {} 图 {} 连续 {} 件没有位置基准（最近 {}：{}）；这些件有胶站不判位置。\
            请检查上料、定位特征是否被遮挡或污染、照明与相机；该配方下一件这幅图定位成功后自动消除",
            self.recipe_id, self.shot_id, self.view, self.consecutive, self.cycle_id, self.reason)
    }

    fn raised(&self) -> bool {
        self.consecutive >= LOCATE_ALARM_PARTS
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LocateChange {
    /// 连续失败到达门限（只报一次）
    Raised(LocateAlarm),
    /// 定位成功，告警消除
    Cleared(LocateAlarm),
    /// 同一配方换了修订，旧修订的告警作废（不是定位恢复）
    Superseded(LocateAlarm),
}

/// 测量的是哪个配方修订的哪个拍照点。
pub struct Subject<'a> {
    pub recipe_id: &'a str,
    pub revision_id: &'a str,
    pub shot_id: &'a str,
    pub k: usize,
    pub camera: &'a str,
}

fn slot(scope: HistoryScope) -> u8 {
    match scope {
        HistoryScope::Real => 0,
        HistoryScope::Offline | HistoryScope::Debug => 1,
    }
}

/// (分槽, 配方, 拍照点, 图) → 当前这一串连续失败
#[derive(Default)]
pub struct LocateHealth {
    entries: BTreeMap<(u8, String, String, u8), LocateAlarm>,
}

impl LocateHealth {
    pub fn observe(&mut self, scope: HistoryScope, subject: &Subject, frame: &FrameView, cycle_id: &str, now: i64) -> Vec<LocateChange> {
        let slot = slot(scope);
        let mut changes = Vec::new();
        self.entries.retain(|(s, recipe, _, _), entry| {
            let stale = *s == slot && recipe == subject.recipe_id && entry.revision_id != subject.revision_id;
            if stale && entry.raised() { changes.push(LocateChange::Superseded(entry.clone())); }
            !stale
        });
        if frame.status != FrameStatus::Done { return changes; }
        for result in frame.view_results.iter().filter(|r| r.error.is_none()) {
            let key = (slot, subject.recipe_id.to_string(), subject.shot_id.to_string(), result.view);
            match frame.reference_failed.iter().find(|f| f.view == result.view) {
                Some(failure) => {
                    let entry = self.entries.entry(key).or_insert_with(|| LocateAlarm {
                        recipe_id: subject.recipe_id.into(), revision_id: subject.revision_id.into(), shot_id: subject.shot_id.into(),
                        k: subject.k, camera: subject.camera.into(), view: result.view, consecutive: 0, reason: String::new(), cycle_id: String::new(), since: now,
                    });
                    entry.consecutive = entry.consecutive.saturating_add(1);
                    entry.k = subject.k;
                    entry.camera = subject.camera.into();
                    entry.reason = failure.reason.clone();
                    entry.cycle_id = cycle_id.into();
                    if entry.consecutive == LOCATE_ALARM_PARTS { changes.push(LocateChange::Raised(entry.clone())); }
                }
                None => {
                    if let Some(entry) = self.entries.remove(&key).filter(LocateAlarm::raised) { changes.push(LocateChange::Cleared(entry)); }
                }
            }
        }
        changes
    }

    /// 当前分槽里已告警的条目，按配方、拍照点、图排序。
    pub fn alarms(&self, scope: HistoryScope) -> Vec<LocateAlarm> {
        let slot = slot(scope);
        self.entries.iter().filter(|((s, ..), entry)| *s == slot && entry.raised()).map(|(_, entry)| entry.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycle::ViewResult;
    use crate::judge::Verdict;
    use crate::measure::ReferenceFailure;

    const P1: Subject = Subject { recipe_id: "A", revision_id: "rev-1", shot_id: "P1", k: 0, camera: "cam1" };
    const P2: Subject = Subject { recipe_id: "A", revision_id: "rev-1", shot_id: "P2", k: 1, camera: "cam2" };

    /// 测完的帧：views 各图都有结果，failed 里的图没有位置基准
    fn done(views: &[u8], failed: &[(u8, &str)]) -> FrameView {
        let mut frame = FrameView::waiting();
        frame.status = FrameStatus::Done;
        frame.view_results = views.iter().map(|&view| ViewResult { view, verdict: Verdict::Ok, error: None, score: Some(1.0), ms: Some(5), pose: None }).collect();
        frame.reference_failed = failed.iter().map(|&(view, reason)| ReferenceFailure { view, reason: reason.into() }).collect();
        frame
    }
    fn fail(reason: &str) -> FrameView { done(&[1], &[(1, reason)]) }
    fn raised(changes: &[LocateChange]) -> usize { changes.iter().filter(|c| matches!(c, LocateChange::Raised(_))).count() }

    #[test]
    fn three_consecutive_parts_without_reference_then_alarm_raised_once_on_the_third() {
        for reason in ["定位失败", "两端搬移超限"] {
            let mut health = LocateHealth::default();
            assert_eq!(raised(&health.observe(HistoryScope::Real, &P1, &fail(reason), "c1", 100)), 0);
            assert_eq!(raised(&health.observe(HistoryScope::Real, &P1, &fail(reason), "c2", 200)), 0);
            assert!(health.alarms(HistoryScope::Real).is_empty(), "{reason}：两件还不到门限");
            assert_eq!(raised(&health.observe(HistoryScope::Real, &P1, &fail(reason), "c3", 300)), 1, "{reason}：第 3 件告警");
            assert_eq!(raised(&health.observe(HistoryScope::Real, &P1, &fail(reason), "c4", 400)), 0, "{reason}：继续失败不重复报出现");
            let alarm = &health.alarms(HistoryScope::Real)[0];
            assert_eq!((alarm.consecutive, alarm.cycle_id.as_str(), alarm.since, alarm.reason.as_str()), (4, "c4", 100, reason));
        }
    }

    #[test]
    fn located_part_after_alarm_then_cleared_while_other_views_and_shots_keep_counting() {
        let mut health = LocateHealth::default();
        for cycle in ["c1", "c2", "c3"] {
            health.observe(HistoryScope::Real, &P1, &done(&[1, 2], &[(1, "定位失败"), (2, "定位失败")]), cycle, 100);
            health.observe(HistoryScope::Real, &P2, &fail("定位失败"), cycle, 100);
        }
        assert_eq!(health.alarms(HistoryScope::Real).len(), 3);
        let changes = health.observe(HistoryScope::Real, &P1, &done(&[1, 2], &[(2, "定位失败")]), "c4", 400);
        assert!(matches!(changes.as_slice(), [LocateChange::Cleared(a)] if a.shot_id == "P1" && a.view == 1), "{changes:?}");
        let left: Vec<_> = health.alarms(HistoryScope::Real).into_iter().map(|a| (a.shot_id, a.view, a.consecutive)).collect();
        assert_eq!(left, [("P1".to_string(), 2, 4), ("P2".to_string(), 1, 3)], "同件其他图、其他拍照点不受影响");
    }

    #[test]
    fn measurement_without_evidence_between_failures_then_count_neither_resets_nor_clears() {
        let mut errored = done(&[1], &[]);
        errored.status = FrameStatus::Error;
        let mut missing = FrameView::waiting();
        missing.status = FrameStatus::Missing;
        let mut view_error = done(&[1], &[]);
        view_error.view_results[0].error = Some("lyFlow 运行失败".into());
        for (name, gap) in [("测量出错", errored), ("缺帧", missing), ("该图单独出错", view_error)] {
            let mut health = LocateHealth::default();
            health.observe(HistoryScope::Real, &P1, &fail("定位失败"), "c1", 100);
            health.observe(HistoryScope::Real, &P1, &fail("定位失败"), "c2", 200);
            assert!(health.observe(HistoryScope::Real, &P1, &gap, "c3", 300).is_empty(), "{name}");
            assert_eq!(raised(&health.observe(HistoryScope::Real, &P1, &fail("定位失败"), "c4", 400)), 1, "{name}不能证明定位恢复");
        }
    }

    #[test]
    fn parts_of_another_recipe_between_failures_then_streak_is_not_interrupted() {
        let other = Subject { recipe_id: "B", revision_id: "rev-9", shot_id: "P1", k: 0, camera: "cam1" };
        let mut health = LocateHealth::default();
        health.observe(HistoryScope::Real, &P1, &fail("定位失败"), "c1", 100);
        health.observe(HistoryScope::Real, &P1, &fail("定位失败"), "c2", 200);
        health.observe(HistoryScope::Real, &other, &done(&[1], &[]), "b1", 250);
        assert_eq!(raised(&health.observe(HistoryScope::Real, &P1, &fail("定位失败"), "c3", 300)), 1);
    }

    #[test]
    fn same_recipe_new_revision_then_old_streak_and_alarm_dropped_and_new_template_counts_from_zero() {
        let next = Subject { revision_id: "rev-2", ..P1 };
        let mut health = LocateHealth::default();
        for cycle in ["c1", "c2", "c3"] { health.observe(HistoryScope::Real, &P1, &fail("定位失败"), cycle, 100); }
        let changes = health.observe(HistoryScope::Real, &next, &fail("定位失败"), "n1", 400);
        assert!(matches!(changes.as_slice(), [LocateChange::Superseded(a)] if a.revision_id == "rev-1"), "{changes:?}");
        assert!(health.alarms(HistoryScope::Real).is_empty());
        assert_eq!(raised(&health.observe(HistoryScope::Real, &next, &fail("定位失败"), "n2", 500)), 0, "新修订从 0 计：第 2 件不告警");
        assert_eq!(raised(&health.observe(HistoryScope::Real, &next, &fail("定位失败"), "n3", 600)), 1);
    }

    #[test]
    fn new_revision_seen_on_another_shot_then_superseded_alarm_keeps_its_own_shot_and_camera() {
        // 回归：作废的日志曾用触发这一帧（P1 / cam1）的相机记 P2 的告警
        let mut health = LocateHealth::default();
        for cycle in ["c1", "c2", "c3"] { health.observe(HistoryScope::Real, &P2, &fail("定位失败"), cycle, 100); }
        let changes = health.observe(HistoryScope::Real, &Subject { revision_id: "rev-2", ..P1 }, &done(&[1], &[]), "n1", 400);
        assert!(matches!(changes.as_slice(), [LocateChange::Superseded(a)] if (a.shot_id.as_str(), a.k, a.camera.as_str()) == ("P2", 1, "cam2")), "{changes:?}");
    }

    #[test]
    fn offline_parts_fail_three_times_then_real_slot_has_no_alarm() {
        let mut health = LocateHealth::default();
        for cycle in ["o1", "o2", "o3"] { health.observe(HistoryScope::Offline, &P1, &fail("定位失败"), cycle, 100); }
        assert_eq!(health.alarms(HistoryScope::Offline).len(), 1);
        assert!(health.alarms(HistoryScope::Real).is_empty(), "离线演练不能在真机报警");
        assert_eq!(raised(&health.observe(HistoryScope::Real, &P1, &fail("定位失败"), "c1", 200)), 0, "离线的失败不计入真机连续数");
    }
}
