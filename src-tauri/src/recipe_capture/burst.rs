//! 段配方的采集轮次（CaptureRoundV2，D1）：只读取与核对盘上的轮次，新轮次只由历史件导入或手动配置数据（合成轮次直接写入 recipe-capture/）产生。
//!
//! - 目录结构：`round.json` 记统计，`frames.jsonl` 每行一帧，图像为无压缩 PGM，一帧一个文件 `{相机}/{seq:06}.pgm`；
//! - 帧组数 = 各相机最大 seq 的最大值；缺帧只记在 missingFrames 并给丢帧告警（[`loss_alarms`]），整轮不判失败；
//! - 以前示教握手实拍的轮次（format 2，带 `teachingRoundId`、PLC 字段）照常读取；启动时仍在进行中的轮次按 `frames.jsonl`
//!   恢复统计并判中断；
//! - 历史件导入（[`history`]）：维护模式或生产录制的完整件按同一目录结构物化成已完成的轮次，原图为硬链接，`origin` 记来源。

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::frame_group::{loss_alarms, LossAlarm};

/// 新轮次记录的格式号。3 加 `origin`（历史件导入、手动配置数据）；2 没有 `origin`，照常读取。
pub const FORMAT: u8 = 3;
const FORMAT_V2: u8 = 2;
/// 一轮的帧组序号上限：ΣN_exp 不超过 PLC Int，防止异常记录生成海量帧组。
pub const MAX_SEQ: u64 = crate::plan::MAX_EXPECTED_TOTAL as u64;
const FRAMES_FILE: &str = "frames.jsonl";

/// 某台相机（本轮相机下标）缺某个帧组的帧。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MissingFrame {
    pub camera: usize,
    pub seq: u64,
}

/// 逐帧告警：这些帧都不判整轮失败。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CaptureAlarm {
    /// 同一相机重复的帧号：保留先到的那帧
    Duplicate { camera: usize, seq: u64 },
    /// 写盘队列满：不写图，记缺帧
    QueueFull { camera: usize, seq: u64 },
    /// 图像内存预算满，取图回调没拷图：记缺帧
    BudgetDropped { camera: usize, seq: u64 },
    /// 丢包或没有可用图像：不写图，记缺帧
    Corrupt { camera: usize, seq: u64 },
}

/// 新轮次失败原因；计数不符不再是失败。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CaptureFailureV2 {
    /// 相机断连，或帧的归属、计数、来源异常
    Camera { camera_id: String },
    /// 设备会话变化
    SessionChanged { camera_id: String },
    /// PLC 开始 / 结束顺序异常
    Plc,
    StartTimeout,
    /// PLC 已结束、drain 窗口已过，但已入队的帧在时限内没写完（各相机合计）
    WriteTimeout { queued: u32, written: u32 },
    /// 操作员中止或应用退出
    Interrupted,
    /// PLC 断连、握手故障、接件失败或记录保存失败
    Other,
}

/// 轮次的来源：以前示教握手实拍的轮次为空；历史件导入的轮次记下原记录；手动配置数据（合成轮次）记标签。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RoundOrigin {
    History {
        cycle_id: String,
        record_id: i64,
        /// 操作员导入时确认这一件是标准件：加入标定时按标准件轮次
        #[serde(default)]
        standard: bool,
    },
    /// 手动配置数据：不来自现场 PLC，不是标准件
    Prepared { label: String },
}

impl RoundOrigin {
    /// 历史件的 cycleId；手动配置数据没有，返回空串（不会与任何记录的 cycleId 相同）。
    pub fn cycle_id(&self) -> &str {
        match self {
            Self::History { cycle_id, .. } => cycle_id,
            Self::Prepared { .. } => "",
        }
    }

    pub fn standard(&self) -> bool {
        matches!(self, Self::History { standard: true, .. })
    }
}

/// 段配方的采集轮次。帧元数据在 `frames.jsonl`，这里只有统计（字段与 `src/features/plan/types.ts` 对应）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureRoundV2 {
    pub format: u8,
    pub round_id: String,
    pub recipe_id: String,
    /// received_count 为这台相机已写盘的帧数
    pub cameras: Vec<CaptureCamera>,
    pub state: CaptureState,
    /// PLC 写来的 shotCount：示教轮次可为估算值，只记录不核对
    pub plc_shot_count: Option<u16>,
    pub plc_plan_version: Option<u32>,
    #[serde(default)]
    pub teaching_round_id: Option<u16>,
    /// partEnd 时的 deviceTriggers，只作参考
    pub device_triggers: Option<u16>,
    /// 帧组数 = 各相机最大 seq 的最大值
    pub group_count: u64,
    /// 各相机已写盘的帧数，按本轮相机顺序；只用于丢帧告警
    pub camera_frames: Vec<u32>,
    /// 至少一台相机缺帧的帧组序号，升序
    pub incomplete_groups: Vec<u64>,
    /// 按 (seq, camera) 升序
    pub missing_frames: Vec<MissingFrame>,
    pub loss_alarms: Vec<LossAlarm>,
    /// 逐帧告警（最多 MAX_ALARMS 条）
    #[serde(default)]
    pub alarms: Vec<CaptureAlarm>,
    pub created_at: i64,
    pub ended_at: Option<i64>,
    pub error: Option<String>,
    pub failure: Option<CaptureFailureV2>,
    pub simulated: bool,
    #[serde(default)]
    pub origin: Option<RoundOrigin>,
}

impl CaptureRoundV2 {
    pub fn camera_ids(&self) -> Vec<String> {
        self.cameras.iter().map(|c| c.camera_id.clone()).collect()
    }
}

/// `frames.jsonl` 的一行：一台相机已写盘的一帧。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameLine {
    /// 本轮相机下标
    pub camera: usize,
    pub seq: u64,
    pub frame_counter: u64,
    /// 触发计数可信（Chunk 触发计数、模拟计数）时记下，只作诊断
    pub trigger_counter: Option<u64>,
    /// 轮次目录内的相对路径，必须是 `{相机}/{seq:06}.pgm`
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub ts: i64,
}

fn frame_file(seq: u64) -> String {
    format!("{seq:06}.pgm")
}

fn frame_rel_path(camera_id: &str, seq: u64) -> String {
    format!("{camera_id}/{}", frame_file(seq))
}

/// round.json 文本是不是段配方轮次（format 2 或 3）。
pub(super) fn is_v2(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("format").and_then(serde_json::Value::as_u64))
        .is_some_and(|format| format == FORMAT as u64 || format == FORMAT_V2 as u64)
}

/// 由帧组数与各相机已写盘的 seq 得出的统计。
struct Summary {
    group_count: u64,
    camera_frames: Vec<u32>,
    incomplete_groups: Vec<u64>,
    missing_frames: Vec<MissingFrame>,
    loss_alarms: Vec<LossAlarm>,
}

/// 只沿各相机已写盘的 seq 找空缺（O(帧数 + 缺帧数)）：进行中的轮次每次节拍轮询都要算一次实时统计。
fn summarize<S: std::borrow::Borrow<BTreeSet<u64>>>(group_count: u64, saved: &[S]) -> Summary {
    let camera_frames: Vec<u32> = saved.iter().map(|s| u32::try_from(s.borrow().len()).unwrap_or(u32::MAX)).collect();
    let mut missing_frames = Vec::new();
    for (camera, seqs) in saved.iter().enumerate() {
        let mut next = 1;
        for &seq in seqs.borrow().range(..=group_count) {
            missing_frames.extend((next..seq).map(|seq| MissingFrame { camera, seq }));
            next = seq + 1;
        }
        missing_frames.extend((next..=group_count).map(|seq| MissingFrame { camera, seq }));
    }
    missing_frames.sort_unstable_by_key(|m| (m.seq, m.camera));
    let mut incomplete_groups: Vec<u64> = missing_frames.iter().map(|m| m.seq).collect();
    incomplete_groups.dedup();
    let loss_alarms = loss_alarms(&camera_frames);
    Summary { group_count, camera_frames, incomplete_groups, missing_frames, loss_alarms }
}

fn apply(round: &mut CaptureRoundV2, summary: Summary) {
    for (camera, &frames) in round.cameras.iter_mut().zip(&summary.camera_frames) { camera.received_count = frames; }
    round.group_count = summary.group_count;
    round.camera_frames = summary.camera_frames;
    round.incomplete_groups = summary.incomplete_groups;
    round.missing_frames = summary.missing_frames;
    round.loss_alarms = summary.loss_alarms;
}

impl RecipeCaptureHost {
    /// 新建一轮的目录 `capture-{创建时间}-{序号}`。
    fn new_round_dir(&self) -> Result<(String, i64), String> {
        let created_at = ly_plc::now_ms();
        let mut suffix = 0u32;
        loop {
            let id = format!("capture-{created_at}-{suffix}");
            match std::fs::create_dir(self.root.join(&id)) {
                Ok(()) => return Ok((id, created_at)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => suffix += 1,
                Err(e) => return Err(e.to_string()),
            }
        }
    }

    fn save_burst(&self, round: &CaptureRoundV2) -> Result<(), String> {
        crate::fsio::write_atomic(&self.root.join(&round.round_id).join("round.json"), &serde_json::to_string_pretty(round).map_err(|e| e.to_string())?)
    }

    /// 读一轮新轮次：本进程开始的轮次直接用内存里的统计；其余从磁盘读并核对 round.json 与 frames.jsonl。
    pub fn get_burst(&self, round_id: &str) -> Result<CaptureRoundV2, String> {
        self.load_burst(round_id, true).map(|(round, _)| round)
    }

    /// 历史列表用：只核对 round.json（不逐帧读 frames.jsonl）。
    pub(super) fn burst_summary(&self, round_id: &str) -> Result<CaptureRoundV2, String> {
        self.load_burst(round_id, false).map(|(round, _)| round)
    }

    /// 某台相机第 seq 帧（帧组序号，从 1 起）的原图路径；这台相机缺这一帧时报错。路径由相机与 seq 推出，不取记录里写的路径。
    pub fn burst_frame_path(&self, round_id: &str, camera_id: &str, seq: u64) -> Result<PathBuf, String> {
        let dir = self.safe_round_path(round_id)?;
        let (round, lines) = self.load_burst(round_id, true)?;
        let mut saved = vec![BTreeSet::new(); round.cameras.len()];
        for line in lines.unwrap_or_default() {
            saved[line.camera].insert(line.seq);
        }
        let cameras = round.camera_ids();
        let camera = cameras.iter().position(|id| id == camera_id).ok_or_else(|| format!("采集轮次没有相机 {camera_id}"))?;
        if !saved[camera].contains(&seq) { return Err(format!("相机 {camera_id} 缺第 {seq} 帧组的图像")); }
        Ok(dir.join(camera_id).join(frame_file(seq)))
    }

    /// 某台相机一轮全部帧组（1..=帧组数）的原图路径，缺帧为 None；轮次与帧记录只读一次（缩略图条带用）。
    pub fn burst_frame_paths(
        &self,
        round_id: &str,
        camera_id: &str,
    ) -> Result<Vec<Option<PathBuf>>, String> {
        let dir = self.safe_round_path(round_id)?;
        let (round, lines) = self.load_burst(round_id, true)?;
        let camera = round
            .camera_ids()
            .iter()
            .position(|id| id == camera_id)
            .ok_or_else(|| format!("采集轮次没有相机 {camera_id}"))?;
        let saved: BTreeSet<u64> = lines
            .unwrap_or_default()
            .into_iter()
            .filter(|line| line.camera == camera)
            .map(|line| line.seq)
            .collect();
        Ok((1..=round.group_count)
            .map(|seq| {
                saved
                    .contains(&seq)
                    .then(|| dir.join(camera_id).join(frame_file(seq)))
            })
            .collect())
    }

    fn load_burst(&self, round_id: &str, deep: bool) -> Result<(CaptureRoundV2, Option<Vec<FrameLine>>), String> {
        let dir = self.safe_round_path(round_id)?;
        let path = std::fs::canonicalize(dir.join("round.json")).map_err(|e| e.to_string())?;
        if !path.starts_with(&dir) { return Err("采集记录路径越出本轮目录".into()); }
        let round: CaptureRoundV2 = serde_json::from_str(&crate::fsio::read_text(&path).map_err(|e| e.to_string())?).map_err(|e| format!("采集记录无法读取：{e}"))?;
        let lines = if deep {
            let path = std::fs::canonicalize(dir.join(FRAMES_FILE)).map_err(|e| format!("采集帧记录不存在：{e}"))?;
            if !path.starts_with(&dir) { return Err("采集帧记录路径越出本轮目录".into()); }
            let text = crate::fsio::read_text(&path).map_err(|e| e.to_string())?;
            Some(text.lines().map(|line| serde_json::from_str::<FrameLine>(line).map_err(|e| format!("采集帧记录无法读取：{e}"))).collect::<Result<Vec<_>, _>>()?)
        } else { None };
        validate_record(round_id, &round, lines.as_deref())?;
        Ok((round, lines))
    }
}

/// 核对新轮次记录：身份、相机、统计自洽、各状态应有的字段；给了 frames.jsonl 时逐行核对路径、相机、seq 与帧计数，
/// 完整轮次还要求统计与 frames.jsonl 一致（防止改写记录伪造帧或把图像指到别处）。
fn validate_record(expected_id: &str, round: &CaptureRoundV2, lines: Option<&[FrameLine]>) -> Result<(), String> {
    let cameras = &round.cameras;
    let n = cameras.len();
    let format_ok = round.format == FORMAT || (round.format == FORMAT_V2 && round.origin.is_none());
    if !format_ok
        || round.round_id != expected_id
        || round.recipe_id.is_empty()
        || n == 0
        || n > MAX_CAMERAS
        || cameras.iter().enumerate().any(|(i, c)| {
            !crate::recipe::valid_camera_id(&c.camera_id)
                || c.device_session == 0
                || cameras[..i].iter().any(|o| o.camera_id == c.camera_id)
        })
    {
        return Err("采集记录身份或相机无效".into());
    }
    let missing_ok = round
        .missing_frames
        .windows(2)
        .all(|w| (w[0].seq, w[0].camera) < (w[1].seq, w[1].camera))
        && round
            .missing_frames
            .iter()
            .all(|m| m.camera < n && (1..=round.group_count).contains(&m.seq));
    let mut incomplete: Vec<u64> = round.missing_frames.iter().map(|m| m.seq).collect();
    incomplete.dedup();
    if round.group_count > MAX_SEQ
        || round.camera_frames.len() != n
        || !missing_ok
        || incomplete != round.incomplete_groups
        || cameras
            .iter()
            .zip(&round.camera_frames)
            .any(|(c, &frames)| c.received_count != frames)
        || round.camera_frames.iter().enumerate().any(|(i, &frames)| {
            frames as u64
                + round
                    .missing_frames
                    .iter()
                    .filter(|m| m.camera == i)
                    .count() as u64
                != round.group_count
        })
        || round.loss_alarms != loss_alarms(&round.camera_frames)
    {
        return Err("采集记录的帧组、缺帧或丢帧告警统计不自洽".into());
    }
    let teaching = round
        .teaching_round_id
        .is_some_and(|id| (1..=32767).contains(&id))
        && round.plc_shot_count.is_none()
        && round.plc_plan_version.is_none()
        && round.device_triggers.is_none();
    if round.teaching_round_id.is_some() && !teaching {
        return Err("示教 V1 轮次编号或元数据不一致".into());
    }
    let history = round.origin.is_some();
    if history
        && (round.teaching_round_id.is_some()
            || round.plc_shot_count.is_some()
            || round.plc_plan_version.is_some()
            || round.device_triggers.is_some()
            || round.state != CaptureState::Complete
            || round.origin.as_ref().is_some_and(|o| match o {
                RoundOrigin::History { cycle_id, .. } => !history::valid_cycle_id(cycle_id),
                RoundOrigin::Prepared { label } => label.trim().is_empty(),
            }))
    {
        return Err("历史件导入或手动配置数据的轮次来源或状态无效".into());
    }
    let started = teaching
        || history
        || (round.plc_plan_version.is_some_and(|v| v > 0) && round.plc_shot_count.is_some());
    let terminal_ok = match round.state {
        CaptureState::WaitingStart => {
            round.teaching_round_id.is_none()
                && round.plc_shot_count.is_none()
                && round.plc_plan_version.is_none()
                && round.device_triggers.is_none()
                && round.group_count == 0
                && round.ended_at.is_none()
                && round.error.is_none()
                && round.failure.is_none()
        }
        CaptureState::Receiving => {
            started
                && round.device_triggers.is_none()
                && round.ended_at.is_none()
                && round.error.is_none()
                && round.failure.is_none()
        }
        CaptureState::Draining | CaptureState::Complete => {
            started && round.ended_at.is_some() && round.error.is_none() && round.failure.is_none()
        }
        CaptureState::Failed => round.error.is_some() && round.failure.is_some(),
    };
    if !terminal_ok { return Err("采集记录的状态与 PLC 开始、结束或失败原因不一致".into()); }
    let Some(lines) = lines else { return Ok(()) };
    let mut saved = vec![BTreeSet::new(); n];
    let mut baselines: BTreeMap<usize, u64> = BTreeMap::new();
    for line in lines {
        let camera = cameras.get(line.camera).ok_or("采集帧记录引用了本轮之外的相机")?;
        if !(1..=MAX_SEQ).contains(&line.seq) || line.width == 0 || line.height == 0 { return Err("采集帧记录的帧组序号或尺寸无效".into()); }
        if line.path != frame_rel_path(&camera.camera_id, line.seq) { return Err("采集帧记录的图像路径与轮次目录不一致".into()); }
        if !saved[line.camera].insert(line.seq) { return Err("采集帧记录重复".into()); }
        let baseline = line.frame_counter.checked_sub(line.seq).ok_or("采集帧记录的帧计数小于帧组序号")?;
        if *baselines.entry(line.camera).or_insert(baseline) != baseline { return Err(format!("相机 {} 的帧计数与帧组序号对不上", camera.camera_id)); }
    }
    if round.state == CaptureState::Complete {
        let expected = summarize(round.group_count, &saved);
        if saved.iter().any(|s| s.last().is_some_and(|&seq| seq > round.group_count))
            || expected.camera_frames != round.camera_frames || expected.missing_frames != round.missing_frames {
            return Err("完整采集的统计与已写盘的帧不一致".into());
        }
    }
    Ok(())
}

/// 启动时恢复：进行中的轮次按 frames.jsonl 重算统计（容忍末行写了一半），判为应用退出中断。
pub(super) fn restore(path: &Path, mut round: CaptureRoundV2) -> Result<CaptureRoundV2, String> {
    if !round.state.active() { return Ok(round); }
    let text = path.parent().map(|dir| crate::fsio::read_text(&dir.join(FRAMES_FILE)).unwrap_or_default()).unwrap_or_default();
    let mut saved = vec![BTreeSet::new(); round.cameras.len()];
    for line in text.lines().filter_map(|line| serde_json::from_str::<FrameLine>(line).ok()) {
        let valid = round.cameras.get(line.camera).is_some_and(|c| line.path == frame_rel_path(&c.camera_id, line.seq)) && (1..=MAX_SEQ).contains(&line.seq);
        if valid { saved[line.camera].insert(line.seq); }
    }
    let group_count = saved.iter().filter_map(|s| s.last().copied()).max().unwrap_or(0);
    let summary = summarize(group_count, &saved);
    apply(&mut round, summary);
    round.state = CaptureState::Failed;
    round.error = Some("应用退出导致本轮采集中断".into());
    round.failure = Some(CaptureFailureV2::Interrupted);
    crate::fsio::write_atomic(path, &serde_json::to_string_pretty(&round).map_err(|e| e.to_string())?)?;
    Ok(round)
}

mod history;

pub use history::{HistoryFrame, HistoryRound};

#[cfg(test)]
mod tests;
