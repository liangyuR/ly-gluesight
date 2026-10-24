//! 段配方的示教采集轮次（CaptureRoundV2，D1）：不设数量上限、不核对计数，按 SDK 帧号三路配组，增量持久化。
//! 旧拍照点配方仍用父模块的 [`CaptureRound`]，两种轮次永久并存，同一时刻只有一轮在采集。
//!
//! 规则（docs/decisions/segment-burst.md D-SB-8/9/10/33/35 与第六节口径）：
//! - seq = frame_counter − 该相机基线（开始采集时这台相机本会话见到的最高帧计数），帧组数 = 各相机最大 seq 的最大值；
//! - 同一相机重复的 seq 丢弃并告警；帧计数不大于基线或比本轮已见的小（计数回退）、会话变化、手动帧、计数来源变化，整轮失败；
//! - 丢包或无图、写盘队列满、图像内存预算满的帧不写图，记缺帧并告警，整轮不判失败；
//! - 收到 PLC 结束、drain 窗口到期、已入队的帧全部写完即完成；PLC 的 shotCount 与 deviceTriggers 只记录，
//!   各相机帧数只用于丢帧告警（[`loss_alarms`]）；超时只有等待开始超时与写盘超时；
//! - 持久化：`round.json` 只在状态变化时原子重写；每写完一帧向 `frames.jsonl` 追加一行；图像为无压缩 PGM，
//!   一帧一个文件 `{相机}/{seq:06}.pgm`。重启时进行中的轮次按 `frames.jsonl` 恢复统计并判中断。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use super::*;
use crate::frame_group::{loss_alarms, Grouper, LossAlarm, Offer};
use crate::judge::fault;

/// 新轮次记录的格式号（前端按它区分新旧轮次）。
pub const FORMAT: u8 = 2;
/// 每台相机写盘队列里最多排队的帧数；满时该帧记缺帧并告警，不判失败（D-SB-8）。图像内存由 FramePool 字节预算封顶。
pub const MAX_FRAMES_PER_CAMERA: u32 = 8192;
/// 一轮的帧组序号上限：ΣN_exp 不超过 PLC Int；帧号比基线跳得更远视为计数异常，防止异常帧号生成海量帧组。
pub const MAX_SEQ: u64 = crate::plan::MAX_EXPECTED_TOTAL as u64;
/// 轮次记录里最多保留的逐帧告警；缺帧本身另记在 missingFrames，不受此限。
const MAX_ALARMS: usize = 1024;
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
    /// PLC 断连、握手故障、布防失败或记录保存失败
    Other,
}

impl CaptureFailureV2 {
    /// 报给 PLC 的异常码（结果码 90，D-SB-33）：相机断连或会话变化是设备丢失 98，其余是处理异常 99。计数不符不再出现。
    pub fn fault_code(&self) -> u16 {
        match self {
            Self::Camera { .. } | Self::SessionChanged { .. } => fault::DEVICE_LOST,
            Self::Plc | Self::StartTimeout | Self::WriteTimeout { .. } | Self::Interrupted | Self::Other => fault::PROCESS_TIMEOUT,
        }
    }
}

/// 段配方的示教采集轮次。帧元数据在 `frames.jsonl`，这里只有统计（字段与 `src/features/plan/types.ts` 对应）。
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

/// round.json 文本是不是新轮次（format 2）。
pub(super) fn is_v2(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text).ok().and_then(|v| v.get("format").and_then(serde_json::Value::as_u64)) == Some(FORMAT as u64)
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

/// 一台相机在本轮的收帧状态。
struct BurstCam {
    cam: u8,
    /// 开始时本会话还没见过帧则为空，收到本轮第一帧时定为它的帧计数 − 1
    baseline: Option<u64>,
    /// 本轮第一帧的计数来源；之后变化判失败
    source: Option<CounterSource>,
    /// 本轮见到的最大帧计数
    last: Option<u64>,
    /// 本轮因这台相机的帧或设备出错而失败
    faulted: bool,
    /// 已交给写盘线程的帧数
    queued: u32,
    /// 写盘线程已写完（图像与 frames.jsonl）的帧数
    written: u32,
    /// 已写盘的 seq
    saved: BTreeSet<u64>,
}

pub(super) struct BurstRun {
    round: CaptureRoundV2,
    cams: Vec<BurstCam>,
    grouper: Grouper,
    /// Grouper 接受过的最大 seq，即帧组数（与 `Grouper::groups().len()` 相同，不必每次重建帧组）
    max_seq: u64,
    /// 本进程开始的轮次：内存里的统计可信；启动时从磁盘恢复的轮次读取时仍要核对记录
    verified: bool,
    drain_timeout: Duration,
    deadline: Instant,
    write_deadline: Option<Instant>,
}

impl BurstRun {
    fn restored(round: CaptureRoundV2) -> Self {
        Self { round, cams: Vec::new(), grouper: Grouper::new(Vec::new()), max_seq: 0, verified: false, drain_timeout: Duration::ZERO, deadline: Instant::now(), write_deadline: None }
    }

    fn camera_of(&self, cam: u8) -> Option<usize> {
        self.cams.iter().position(|c| c.cam == cam)
    }

    fn summary(&self) -> Summary {
        let saved: Vec<&BTreeSet<u64>> = self.cams.iter().map(|c| &c.saved).collect();
        summarize(self.max_seq, &saved)
    }

    /// 进行中的轮次带上实时统计；已结束的轮次统计在结束时已定。
    fn snapshot(&self) -> CaptureRoundV2 {
        let mut round = self.round.clone();
        if round.state.active() && !self.cams.is_empty() { apply(&mut round, self.summary()); }
        round
    }

    fn alarm(&mut self, alarm: CaptureAlarm) {
        if self.round.alarms.len() < MAX_ALARMS { self.round.alarms.push(alarm); }
    }
}

/// 写盘线程要写的一帧。
pub(super) struct BurstItem {
    round_id: String,
    camera: usize,
    seq: u64,
    frame_counter: u64,
    trigger_counter: Option<u64>,
    ts: i64,
    image: Arc<FrameImage>,
}

/// 当前轮次的 frames.jsonl 追加句柄。
pub(super) struct Journal {
    round_id: String,
    file: std::fs::File,
}

/// 取图回调交来的帧是正常交付，还是图像内存预算满、没拷图的帧。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Arrival {
    Delivered,
    BudgetDropped,
}

impl RecipeCaptureHost {
    pub(super) fn burst_active(&self) -> bool {
        self.burst.lock().unwrap().as_ref().is_some_and(|r| r.round.state.active())
    }

    /// 最近一轮新轮次（进行中的带实时统计）。
    pub fn current_burst(&self) -> Option<CaptureRoundV2> {
        self.burst.lock().unwrap().as_ref().map(BurstRun::snapshot)
    }

    pub(super) fn burst_round_id(&self) -> Option<String> {
        self.burst.lock().unwrap().as_ref().map(|r| r.round.round_id.clone())
    }

    fn save_burst(&self, round: &CaptureRoundV2) -> Result<(), String> {
        crate::fsio::write_atomic(&self.root.join(&round.round_id).join("round.json"), &serde_json::to_string_pretty(round).map_err(|e| e.to_string())?)
    }

    /// 段配方开始一轮示教采集：相机须为单视角（三台独立相机共用一个触发，D-SB-11），基线取开始时各相机本会话见到的最高帧计数，没见过帧时取本轮首帧计数 − 1。
    pub fn start_burst(&self, recipe_id: String, devices: Vec<CaptureDevice>, drain_timeout_ms: u64, simulated: bool) -> Result<CaptureRoundV2, CaptureStartRejected> {
        use CaptureStartRejected as Rejected;
        if !(200..=30000).contains(&drain_timeout_ms) { return Err(Rejected::DrainTimeout); }
        if devices.is_empty() || devices.len() > MAX_CAMERAS { return Err(Rejected::CameraCount); }
        for (i, d) in devices.iter().enumerate() {
            let id = &d.camera_id;
            if !crate::recipe::valid_camera_id(id) { return Err(Rejected::InvalidCameraId(id.clone())); }
            if devices[..i].iter().any(|o| o.camera_id == *id || o.cam == d.cam) { return Err(Rejected::Duplicate(id.clone())); }
            if d.session == 0 { return Err(Rejected::SessionNotReady(id.clone())); }
            if view_count(&d.camera_config) != 1 || d.camera_config.get("compositeLayout").is_some_and(|l| !l.is_null()) {
                return Err(Rejected::MultiView(id.clone()));
            }
        }
        let legacy = self.current.lock().unwrap();
        let mut guard = self.burst.lock().unwrap();
        if legacy.as_ref().is_some_and(|r| r.round.state.active()) || guard.as_ref().is_some_and(|r| r.round.state.active()) { return Err(Rejected::Busy); }
        let created_at = ly_plc::now_ms();
        let mut suffix = 0u32;
        let round_id = loop {
            let id = format!("capture-{created_at}-{suffix}");
            match std::fs::create_dir(self.root.join(&id)) {
                Ok(()) => break id,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => suffix += 1,
                Err(e) => return Err(Rejected::Io(e.to_string())),
            }
        };
        let dir = self.root.join(&round_id);
        for d in &devices {
            std::fs::create_dir(dir.join(&d.camera_id)).map_err(|e| Rejected::Io(e.to_string()))?;
        }
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(dir.join(FRAMES_FILE))
            .map_err(|e| Rejected::Io(e.to_string()))?;
        let baselines: Vec<u64> = devices
            .iter()
            .map(|d| d.frame_baseline.unwrap_or_default())
            .collect();
        let cams = devices
            .iter()
            .map(|d| BurstCam {
                cam: d.cam,
                baseline: d.frame_baseline,
                source: None,
                last: None,
                faulted: false,
                queued: 0,
                written: 0,
                saved: BTreeSet::new(),
            })
            .collect();
        let n = devices.len();
        let cameras = devices.into_iter().map(|d| CaptureCamera { camera_id: d.camera_id, camera_config: d.camera_config, imaging: d.imaging, device_session: d.session, received_count: 0 }).collect();
        let round = CaptureRoundV2 {
            format: FORMAT,
            round_id: round_id.clone(),
            recipe_id,
            cameras,
            state: CaptureState::WaitingStart,
            plc_shot_count: None,
            plc_plan_version: None,
            teaching_round_id: None,
            device_triggers: None,
            group_count: 0,
            camera_frames: vec![0; n],
            incomplete_groups: Vec::new(),
            missing_frames: Vec::new(),
            loss_alarms: Vec::new(),
            alarms: Vec::new(),
            created_at,
            ended_at: None,
            error: None,
            failure: None,
            simulated,
        };
        self.save_burst(&round).map_err(Rejected::Io)?;
        *self.journal.lock().unwrap() = Some(Journal { round_id, file });
        *guard = Some(BurstRun {
            round: round.clone(), cams, grouper: Grouper::new(baselines), max_seq: 0, verified: true,
            drain_timeout: Duration::from_millis(drain_timeout_ms), deadline: Instant::now() + ROUND_TIMEOUT, write_deadline: None,
        });
        Ok(round)
    }

    /// 结束本轮（失败）。不封锁设备会话：重采的基线取开始时本会话见到的最高帧计数，PLC 开始前到的迟到帧让新一轮判失败。
    fn fail_burst(&self, run: &mut BurstRun, failure: CaptureFailureV2, error: String) {
        if !run.cams.is_empty() { let summary = run.summary(); apply(&mut run.round, summary); }
        run.round.state = CaptureState::Failed;
        run.round.error = Some(error);
        run.round.failure = Some(failure);
        self.journal.lock().unwrap().take();
        if let Err(error) = self.save_burst(&run.round) {
            run.round.error = Some(format!("{}；异常记录保存失败：{error}", run.round.error.as_deref().unwrap_or_default()));
        }
    }

    /// 让进行中的新轮次失败（没有进行中的新轮次时什么也不做）。
    pub(super) fn fail_burst_active(&self, failure: CaptureFailureV2, error: String) {
        if let Some(run) = self.burst.lock().unwrap().as_mut().filter(|r| r.round.state.active()) { self.fail_burst(run, failure, error); }
    }

    pub(super) fn disconnected_burst(&self, cam: u8) {
        let mut guard = self.burst.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.state.active()) else { return };
        let Some(j) = run.camera_of(cam) else { return };
        run.cams[j].faulted = true;
        let camera_id = run.round.cameras[j].camera_id.clone();
        let error = format!("相机 {camera_id} 断连，请整圈重采");
        self.fail_burst(run, CaptureFailureV2::Camera { camera_id }, error);
    }

    pub fn burst_ready(&self, teaching_round_id: u16) -> Result<(), String> {
        let mut guard = self.burst.lock().unwrap();
        let run = guard
            .as_mut()
            .filter(|r| r.round.state.active())
            .ok_or("没有等待开始的示教采集")?;
        if run.round.state != CaptureState::WaitingStart
            || !(1..=32767).contains(&teaching_round_id)
        {
            let error = "示教准备状态或轮次编号无效".to_string();
            self.fail_burst(run, CaptureFailureV2::Plc, error.clone());
            return Err(error);
        }
        run.round.teaching_round_id = Some(teaching_round_id);
        run.round.state = CaptureState::Receiving;
        run.deadline = Instant::now() + Duration::from_secs(130);
        if let Err(e) = self.save_burst(&run.round) {
            self.fail_burst(run, CaptureFailureV2::Other, e.clone());
            return Err(e);
        }
        Ok(())
    }

    /// PLC 开始：shotCount 只记录（可为估算值，不限 1–64、不核对）；计划版本须为正数（协议 4.1）。
    pub fn burst_plc_started(&self, shot_count: u16, plan_version: u32) -> Result<(), String> {
        let mut guard = self.burst.lock().unwrap();
        let run = guard.as_mut().filter(|r| r.round.state.active()).ok_or("没有等待开始的示教采集")?;
        let error = if run.round.state != CaptureState::WaitingStart { Some("PLC 开始信号重复或顺序异常") }
            else if plan_version == 0 { Some("PLC 计划版本须为正数") }
            else { None };
        if let Some(error) = error {
            self.fail_burst(run, CaptureFailureV2::Plc, error.into());
            return Err(error.into());
        }
        run.round.plc_shot_count = Some(shot_count);
        run.round.plc_plan_version = Some(plan_version);
        run.round.state = CaptureState::Receiving;
        if let Err(e) = self.save_burst(&run.round) { self.fail_burst(run, CaptureFailureV2::Other, e.clone()); return Err(e); }
        Ok(())
    }

    /// PLC 结束：deviceTriggers 为保留字段，不参与丢帧告警；之后等 drain 窗口到期、已入队的帧写完即完成。
    pub fn burst_plc_ended(&self, device_triggers: Option<u16>) -> Result<(), String> {
        let mut guard = self.burst.lock().unwrap();
        let run = guard.as_mut().filter(|r| r.round.state.active()).ok_or("没有正在进行的示教采集")?;
        if run.round.state != CaptureState::Receiving {
            let error = "PLC 结束信号顺序异常".to_string();
            self.fail_burst(run, CaptureFailureV2::Plc, error.clone());
            return Err(error);
        }
        run.round.device_triggers = device_triggers;
        run.round.state = CaptureState::Draining;
        run.round.ended_at = Some(ly_plc::now_ms());
        run.deadline = Instant::now() + run.drain_timeout;
        run.write_deadline = Some(Instant::now() + ROUND_TIMEOUT);
        if let Err(e) = self.save_burst(&run.round) { self.fail_burst(run, CaptureFailureV2::Other, e.clone()); return Err(e); }
        Ok(())
    }

    /// 取图回调交来的帧：有进行中的新轮次时返回 true（本轮相机以外的帧也不交给检测节拍）。
    pub(super) fn offer_burst(&self, frame: &Frame) -> bool {
        self.arrive(frame, Arrival::Delivered)
    }

    /// 图像内存预算满、没拷图的帧（camera.rs）：照常配组，记缺帧并告警。
    pub fn offer_dropped(&self, frame: &Frame) {
        self.arrive(frame, Arrival::BudgetDropped);
    }

    fn arrive(&self, f: &Frame, arrival: Arrival) -> bool {
        let mut guard = self.burst.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.state.active()) else { return false };
        let Some(j) = run.camera_of(f.cam) else { return true };
        let camera_id = run.round.cameras[j].camera_id.clone();
        let cam = &run.cams[j];
        let baseline = cam
            .baseline
            .unwrap_or_else(|| f.frame_counter.saturating_sub(1));
        let rejected = if run.round.state == CaptureState::WaitingStart {
            Some((false, "PLC 开始前收到帧，无法确定本轮归属".to_string()))
        } else if f.session != run.round.cameras[j].device_session {
            Some((true, "设备会话已变化，不能混用不同会话的图像".into()))
        } else if f.manual {
            Some((false, "本轮收到手动触发图像".into()))
        } else if f.counter == CounterSource::Synthetic && !run.round.simulated {
            Some((false, "设备实拍轮次收到模拟计数的帧".into()))
        } else if cam.source.is_some_and(|source| source != f.counter) {
            Some((false, "帧计数来源发生变化".into()))
        } else {
            match f.frame_counter.checked_sub(baseline).filter(|seq| *seq > 0) {
                None => Some((
                    false,
                    format!(
                        "帧计数 {} 不大于开始采集时的基线 {baseline}（计数回退或上一轮的迟到帧）",
                        f.frame_counter
                    ),
                )),
                Some(seq) if seq > MAX_SEQ => Some((
                    false,
                    format!("帧计数比基线大 {seq}，超过一轮上限 {MAX_SEQ}，计数异常"),
                )),
                Some(_) => None,
            }
        };
        if let Some((session, error)) = rejected {
            run.cams[j].faulted = true;
            let failure = if session { CaptureFailureV2::SessionChanged { camera_id: camera_id.clone() } } else { CaptureFailureV2::Camera { camera_id: camera_id.clone() } };
            self.fail_burst(run, failure, format!("相机 {camera_id}：{error}"));
            return true;
        }
        if run.cams[j].baseline.is_none() {
            run.cams[j].baseline = Some(baseline);
            run.grouper.set_baseline(j, baseline);
        }
        let seq = match run.grouper.offer(j, f.frame_counter) {
            Offer::Accepted { seq } => seq,
            Offer::Duplicate => {
                let seq = f.frame_counter - baseline;
                run.alarm(CaptureAlarm::Duplicate { camera: j, seq });
                return true;
            }
            // seq 已按 MAX_SEQ 先行核对（上面判整轮失败），默认上限与它相同，OutOfRange 在这里到不了
            Offer::Stale | Offer::OutOfRange { .. } => return true,
        };
        if run.cams[j].last.is_some_and(|last| f.frame_counter < last) {
            run.cams[j].faulted = true;
            let error = format!("相机 {camera_id}：帧计数从 {} 回退到 {}，不能确定帧组归属", run.cams[j].last.unwrap_or_default(), f.frame_counter);
            self.fail_burst(run, CaptureFailureV2::Camera { camera_id }, error);
            return true;
        }
        let cam = &mut run.cams[j];
        cam.last = Some(f.frame_counter);
        cam.source = Some(f.counter);
        run.max_seq = run.max_seq.max(seq);
        if arrival == Arrival::BudgetDropped {
            run.alarm(CaptureAlarm::BudgetDropped { camera: j, seq });
            return true;
        }
        let image = f.images.first().filter(|i| f.images.len() == 1 && i.width != 0 && i.height != 0
            && (i.width as usize).checked_mul(i.height as usize) == Some(i.pixels.len()));
        let Some(image) = image.filter(|_| f.lost_packets == 0).cloned() else {
            run.alarm(CaptureAlarm::Corrupt { camera: j, seq });
            return true;
        };
        if run.cams[j].queued - run.cams[j].written >= MAX_FRAMES_PER_CAMERA {
            run.alarm(CaptureAlarm::QueueFull { camera: j, seq });
            return true;
        }
        let trigger_counter = matches!(f.counter, CounterSource::ChunkTrigger | CounterSource::Synthetic).then_some(f.trigger_counter);
        let item = BurstItem { round_id: run.round.round_id.clone(), camera: j, seq, frame_counter: f.frame_counter, trigger_counter, ts: f.ts, image };
        if self.burst_writers[j].send(item).is_err() {
            run.cams[j].faulted = true;
            self.fail_burst(run, CaptureFailureV2::Other, format!("相机 {camera_id}：示教采集写盘线程已关闭，请整圈重采"));
        } else {
            run.cams[j].queued += 1;
        }
        true
    }

    fn append_line(&self, round_id: &str, line: &FrameLine) -> Result<(), String> {
        let mut text = serde_json::to_string(line).map_err(|e| e.to_string())?;
        text.push('\n');
        let mut journal = self.journal.lock().unwrap();
        let journal = journal.as_mut().filter(|j| j.round_id == round_id).ok_or("采集记录已关闭")?;
        journal.file.write_all(text.as_bytes()).map_err(|e| e.to_string())
    }

    /// 写盘线程：写 PGM，再向 frames.jsonl 追加一行，最后记为已写盘。两次取锁之间不持有任何锁，写盘停顿不挡取图回调。
    pub(super) fn write_burst(&self, item: BurstItem) {
        let BurstItem { round_id, camera, seq, frame_counter, trigger_counter, ts, image } = item;
        let camera_id = {
            let guard = self.burst.lock().unwrap();
            let Some(run) = guard.as_ref().filter(|r| r.round.round_id == round_id && r.round.state.active()) else { return };
            run.round.cameras[camera].camera_id.clone()
        };
        let path = self.root.join(&round_id).join(&camera_id).join(frame_file(seq));
        let line = FrameLine { camera, seq, frame_counter, trigger_counter, path: frame_rel_path(&camera_id, seq), width: image.width, height: image.height, ts };
        let saved = crate::image_file::save_pgm(&path, &image).and_then(|()| self.append_line(&round_id, &line));
        drop(image);
        let mut guard = self.burst.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.round_id == round_id && r.round.state.active()) else { return };
        match saved {
            Ok(()) => {
                let cam = &mut run.cams[camera];
                cam.written += 1;
                cam.saved.insert(seq);
            }
            Err(e) => self.fail_burst(run, CaptureFailureV2::Other, format!("相机 {camera_id}：保存采集图像失败：{e}")),
        }
    }

    pub(super) fn tick_burst(&self) {
        let mut guard = self.burst.lock().unwrap();
        let Some(run) = guard.as_mut().filter(|r| r.round.state.active() && Instant::now() >= r.deadline) else { return };
        match run.round.state {
            CaptureState::WaitingStart => self.fail_burst(run, CaptureFailureV2::StartTimeout, "采集超时：仍在等待 PLC 开始请求，请整圈重采".into()),
            CaptureState::Draining => {
                let queued: u32 = run.cams.iter().map(|c| c.queued).sum();
                let written: u32 = run.cams.iter().map(|c| c.written).sum();
                if run.cams.iter().all(|c| c.written == c.queued) {
                    let summary = run.summary();
                    apply(&mut run.round, summary);
                    run.round.state = CaptureState::Complete;
                    self.journal.lock().unwrap().take();
                    if let Err(e) = self.save_burst(&run.round) { self.fail_burst(run, CaptureFailureV2::Other, e); }
                } else if run.write_deadline.is_none_or(|d| Instant::now() >= d) {
                    let error = format!("采集超时：PLC 已结束，但 {} s 内只写完 {written}/{queued} 帧；请检查磁盘后整圈重采", ROUND_TIMEOUT.as_secs());
                    self.fail_burst(run, CaptureFailureV2::WriteTimeout { queued, written }, error);
                }
            }
            // 接收中不设超时（D-SB-9）：等 PLC 结束，或操作员中止、PLC 断连
            _ => {}
        }
    }

    /// 读一轮新轮次：本进程开始的轮次直接用内存里的统计；其余从磁盘读并核对 round.json 与 frames.jsonl。
    pub fn get_burst(&self, round_id: &str) -> Result<CaptureRoundV2, String> {
        self.safe_round_path(round_id)?;
        if let Some(round) = self.verified_burst(round_id) { return Ok(round); }
        self.load_burst(round_id, true).map(|(round, _)| round)
    }

    fn verified_burst(&self, round_id: &str) -> Option<CaptureRoundV2> {
        self.burst.lock().unwrap().as_ref().filter(|r| r.verified && r.round.round_id == round_id).map(BurstRun::snapshot)
    }

    /// 历史列表用：只核对 round.json（不逐帧读 frames.jsonl）。
    pub(super) fn burst_summary(&self, round_id: &str) -> Result<CaptureRoundV2, String> {
        self.safe_round_path(round_id)?;
        if let Some(round) = self.verified_burst(round_id) { return Ok(round); }
        self.load_burst(round_id, false).map(|(round, _)| round)
    }

    /// 某台相机第 seq 帧（帧组序号，从 1 起）的原图路径；这台相机缺这一帧时报错。路径由相机与 seq 推出，不取记录里写的路径。
    pub fn burst_frame_path(&self, round_id: &str, camera_id: &str, seq: u64) -> Result<PathBuf, String> {
        let dir = self.safe_round_path(round_id)?;
        let live = self.burst.lock().unwrap().as_ref().filter(|r| r.verified && r.round.round_id == round_id)
            .map(|r| (r.round.camera_ids(), r.cams.iter().map(|c| c.saved.clone()).collect::<Vec<_>>()));
        let (cameras, saved) = match live {
            Some(live) => live,
            None => {
                let (round, lines) = self.load_burst(round_id, true)?;
                let mut saved = vec![BTreeSet::new(); round.cameras.len()];
                for line in lines.unwrap_or_default() { saved[line.camera].insert(line.seq); }
                (round.camera_ids(), saved)
            }
        };
        let camera = cameras.iter().position(|id| id == camera_id).ok_or_else(|| format!("采集轮次没有相机 {camera_id}"))?;
        if !saved[camera].contains(&seq) { return Err(format!("相机 {camera_id} 缺第 {seq} 帧组的图像")); }
        Ok(dir.join(camera_id).join(frame_file(seq)))
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
    if round.format != FORMAT || round.round_id != expected_id || round.recipe_id.is_empty() || n == 0 || n > MAX_CAMERAS
        || cameras.iter().enumerate().any(|(i, c)| !crate::recipe::valid_camera_id(&c.camera_id) || c.device_session == 0
            || cameras[..i].iter().any(|o| o.camera_id == c.camera_id)) {
        return Err("采集记录身份或相机无效".into());
    }
    let missing_ok = round.missing_frames.windows(2).all(|w| (w[0].seq, w[0].camera) < (w[1].seq, w[1].camera))
        && round.missing_frames.iter().all(|m| m.camera < n && (1..=round.group_count).contains(&m.seq));
    let mut incomplete: Vec<u64> = round.missing_frames.iter().map(|m| m.seq).collect();
    incomplete.dedup();
    if round.group_count > MAX_SEQ || round.camera_frames.len() != n || !missing_ok || incomplete != round.incomplete_groups
        || cameras.iter().zip(&round.camera_frames).any(|(c, &frames)| c.received_count != frames)
        || round.camera_frames.iter().enumerate().any(|(i, &frames)| frames as u64 + round.missing_frames.iter().filter(|m| m.camera == i).count() as u64 != round.group_count)
        || round.loss_alarms != loss_alarms(&round.camera_frames) {
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
    let started = teaching
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
    round.error = Some("应用退出导致本轮采集中断，请整圈重采".into());
    round.failure = Some(CaptureFailureV2::Interrupted);
    crate::fsio::write_atomic(path, &serde_json::to_string_pretty(&round).map_err(|e| e.to_string())?)?;
    Ok(round)
}

pub(super) fn restored_run(round: CaptureRoundV2) -> BurstRun {
    BurstRun::restored(round)
}

#[cfg(test)]
mod tests;
