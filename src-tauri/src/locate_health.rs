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
