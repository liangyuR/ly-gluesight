use super::workflow::{projected_frame, selected_views, CalibrationBasis};
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepId { Capture, Defaults, Teach, Calibration, Overview, Validation, Publish }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus { NotStarted, InProgress, Done, Blocked, Optional }

/// 阻塞原因与“下一步”可跳转的页面。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "page", rename_all = "camelCase")]
pub enum Target { Capture, Teach { k: usize }, Validation, Camera }

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocker { pub message: String, pub target: Target }

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub id: StepId,
    pub label: &'static str,
    pub optional: bool,
    pub status: StepStatus,
    pub detail: String,
    pub blockers: Vec<Blocker>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShotState { Recapture, Unconfigured, Draft, PendingTrial, TrialFailed, TrialPassed, Taught, Skipped }

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CalibrationState {
    Unverified,
    Verified { error_mm: f64 },
    Failed { error_mm: f64 },
    SampleSet { set_id: String, mm_per_px: f32 },
    Stale,
}

impl CalibrationState {
    fn satisfied(&self) -> bool { matches!(self, Self::Verified { .. } | Self::SampleSet { .. }) }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewProgress {
    pub view: u8,
    pub enabled: bool,
    pub state: ShotState,
    pub trial_stale: bool,
    pub calibration: CalibrationState,
    pub hint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotProgress {
    pub k: usize,
    pub shot_id: String,
    pub state: ShotState,
    pub completed: usize,
    pub total: usize,
    pub views: Vec<ViewProgress>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NextAction { pub step: StepId, pub label: String, pub target: Target }

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub steps: Vec<Step>,
    pub shots: Vec<ShotProgress>,
    pub next: Option<NextAction>,
}

/// 当前设备：按（拍照点，视角）的相机与标定标签，以及当前图像引擎。缺项表示该视角的相机当前不可用。
#[derive(Clone, Debug, Default)]
pub struct Devices {
    pub views: HashMap<(usize, u8), (Value, Value)>,
    pub engine: Option<Value>,
}

#[cfg(test)]
impl Progress {
    pub fn step(&self, id: StepId) -> &Step { self.steps.iter().find(|s| s.id == id).expect("每个步骤都会生成") }
}

fn view_progress(w: &Workspace, recipe: Option<&Recipe>, devices: &Devices, k: usize, view: u8, enabled: bool) -> ViewProgress {
    let skipped = w.doc.shots[k].skip;
    let projected = recipe.and_then(|r| r.for_view(k, view).ok());
    let frame = projected_frame(w, k, view).ok();
    let image = frame.as_ref().and_then(|f| f.image.as_ref());
    let shot = projected.as_ref().map(|r| &r.shots[k]);
    let check = w.frames.get(k).and_then(|f| f.view_states.iter().find(|s| s.view == view)).and_then(|s| s.calibration_check.as_ref());
    let current = devices.views.get(&(k, view));
    let device_changed = image.is_some_and(|image| current.is_none_or(|(camera, _)| *camera != image.camera_tag));
    let calib_changed = image.is_some_and(|image| current.is_some_and(|(_, calib)| *calib != image.calib_tag));
    let calibration = match (check, image, shot) {
        (None, ..) | (_, None, _) | (_, _, None) => CalibrationState::Unverified,
        (Some(c), Some(image), Some(shot)) if c.camera_tag != image.camera_tag || c.calib_tag != image.calib_tag || c.mm_per_px != shot.mm_per_px
            || device_changed || calib_changed => CalibrationState::Stale,
        (Some(c), ..) => match &c.basis {
            CalibrationBasis::SampleSet { set_id } if c.passed => CalibrationState::SampleSet { set_id: set_id.clone(), mm_per_px: c.mm_per_px.unwrap_or_default() },
            CalibrationBasis::SampleSet { .. } => CalibrationState::Unverified,
            CalibrationBasis::Manual { error_mm, .. } if c.passed => CalibrationState::Verified { error_mm: *error_mm },
            CalibrationBasis::Manual { error_mm, .. } => CalibrationState::Failed { error_mm: *error_mm },
        },
    };
    let (state, trial_stale, hint) = 'state: {
        if skipped { break 'state (ShotState::Skipped, false, "这个拍照点设为不检：只要求这一帧到达，不量不判，不需要示教中线。".into()); }
        let (Some(image), Some(shot), Some(r), Some(frame)) = (image, shot, projected.as_ref(), frame.as_ref()) else {
            break 'state (ShotState::Unconfigured, false, "先整圈采集，取得这个拍照点的原图。".into());
        };
        if device_changed {
            break 'state (ShotState::Recapture, false, "当前设备与采集原图时不一致（相机参数、样本集或站点模式已变），需整圈重新采集；换回原设备后状态自动恢复。".into());
        }
        if shot.path.is_empty() && shot.mm_per_px.is_none() {
            break 'state (ShotState::Unconfigured, false, "在原图上从胶嘴一侧往外依次点出胶路中线，填好像素当量后保存参数并试测。".into());
        }
        if shot.path.len() < 2 || shot.path_len_px() < 1.0 || shot.mm_per_px.is_none() {
            break 'state (ShotState::Draft, false, "中线至少两点、长度不为零并填好像素当量后，保存参数并试测。".into());
        }
        let Some(trial) = &frame.trial else { break 'state (ShotState::PendingTrial, false, "参数已保存，可以试测当前帧。".into()) };
        let current = shot_tags(r, k).ok();
        let stale = current.as_ref().is_none_or(|(image_tag, params_tag)| trial.image_id != image.id || &trial.geometry_tag != image_tag
            || &image.geometry_tag != image_tag || &trial.params_tag != params_tag);
        if stale { break 'state (ShotState::PendingTrial, true, "现有试测已过期，请重新保存参数并试测。".into()); }
        if calib_changed { break 'state (ShotState::PendingTrial, true, "工位标定已变化：请重新验证像素当量后重新试测。".into()); }
        if devices.engine.as_ref().is_some_and(|engine| *engine != trial.engine_tag) {
            break 'state (ShotState::PendingTrial, true, "图像引擎已变化，请重新试测。".into());
        }
        if !trial.passed { break 'state (ShotState::TrialFailed, false, trial.reason.clone()); }
        if frame.saved { break 'state (ShotState::Taught, false, "本帧示教已保存；发布前仍需整体验证。".into()); }
        (ShotState::TrialPassed, false, trial.reason.clone())
    };
    ViewProgress { view, enabled, state, trial_stale, calibration, hint }
}

fn shot_progress(w: &Workspace, recipe: Option<&Recipe>, devices: &Devices, k: usize) -> ShotProgress {
    let selected = selected_views(w, k);
    let frame = &w.frames[k];
    let mut listed: Vec<u8> = frame.views.iter().map(|i| i.view).collect();
    if listed.is_empty() { listed = selected.clone(); }
    let views: Vec<_> = listed.into_iter().map(|v| view_progress(w, recipe, devices, k, v, selected.contains(&v))).collect();
    let enabled: Vec<_> = views.iter().filter(|v| v.enabled).collect();
    let completed = enabled.iter().filter(|v| v.state == ShotState::Taught).count();
    let state = if w.doc.shots[k].skip { ShotState::Skipped }
        else { enabled.iter().map(|v| v.state).min().unwrap_or(ShotState::Unconfigured) };
    ShotProgress { k, shot_id: w.doc.shots[k].id.clone(), state, completed, total: enabled.len(), views }
}

fn shot_state_text(state: ShotState) -> &'static str {
    match state {
        ShotState::Recapture => "需重新采集",
        ShotState::Unconfigured => "未配置",
        ShotState::Draft => "编辑草稿",
        ShotState::PendingTrial => "待试测",
        ShotState::TrialFailed => "试测异常",
        ShotState::TrialPassed => "试测通过，待保存示教",
        ShotState::Taught => "已示教",
        ShotState::Skipped => "不检",
    }
}

fn calibration_text(state: &CalibrationState) -> &'static str {
    match state {
        CalibrationState::Unverified => "未验证",
        CalibrationState::Verified { .. } => "人工验证通过",
        CalibrationState::Failed { .. } => "人工验证未通过",
        CalibrationState::SampleSet { .. } => "样本集声明当量",
        CalibrationState::Stale => "当量或设备已变，需重新验证",
    }
}

fn check_target(name: &str, first_open: Option<usize>) -> Target {
    match name {
        "设备与采集" => Target::Camera,
        "完整采集" => Target::Capture,
        "胶路示教" | "示教与标定" => Target::Teach { k: first_open.unwrap_or(0) },
        _ => Target::Validation,
    }
}

/// 候选配方的显式进度：各步骤状态、阻塞原因（带可跳转目标）与全局下一步。只读候选自身记录，不访问设备。
/// 冻结图与 devices（当前设备）不一致时只在结果里标为需重新采集或过期，不改候选记录。
pub fn compute(w: &Workspace, production_version: Option<u32>, devices: &Devices) -> Progress {
    let recipe = w.doc.build().ok();
    let shots: Vec<_> = (0..w.frames.len().min(w.doc.shots.len())).map(|k| shot_progress(w, recipe.as_ref(), devices, k)).collect();
    let captured = w.capture_id.is_some() && !w.frames.is_empty();
    let need_capture = || vec![Blocker { message: "先采用一轮完整的整圈采集".into(), target: Target::Capture }];
    let label = |s: &ShotProgress| format!("拍照点 {} · {}", s.k + 1, s.shot_id);

    let recapture: Vec<_> = shots.iter().filter(|s| s.state == ShotState::Recapture)
        .map(|s| Blocker { message: format!("{}：当前设备与采集时不一致，需重新采集", label(s)), target: Target::Capture }).collect();
    let capture = Step { id: StepId::Capture, label: "整圈采集", optional: false,
        status: if !captured { StepStatus::NotStarted } else if recapture.is_empty() { StepStatus::Done } else { StepStatus::Blocked },
        detail: if !captured { "等待设备就绪后整圈采集并采用".into() }
            else if recapture.is_empty() { format!("已采用完整采集轮次，{} 个拍照点", w.frames.len()) }
            else { "当前设备与采集时不一致，需整圈重新采集".into() },
        blockers: recapture };

    let defaults = Step { id: StepId::Defaults, label: "配方默认参数", optional: true, status: StepStatus::Optional,
        detail: "可选：默认检测与判定参数，单帧示教时可按图覆盖".into(), blockers: Vec::new() };

    let measured: Vec<_> = shots.iter().filter(|s| s.state != ShotState::Skipped).collect();
    let total: usize = measured.iter().map(|s| s.total).sum();
    let completed: usize = measured.iter().map(|s| s.completed).sum();
    let teach = {
        let mut blockers = Vec::new();
        for s in &measured {
            if s.total == 0 { blockers.push(Blocker { message: format!("{}：未选检测图", label(s)), target: Target::Teach { k: s.k } }); }
            else if s.state != ShotState::Taught {
                let target = if s.state == ShotState::Recapture { Target::Capture } else { Target::Teach { k: s.k } };
                blockers.push(Blocker { message: format!("{}：{}", label(s), shot_state_text(s.state)), target });
            }
        }
        let status = if !captured { StepStatus::NotStarted }
            else if measured.is_empty() { StepStatus::Blocked }
            else if blockers.is_empty() { StepStatus::Done }
            else if measured.iter().any(|s| s.state == ShotState::TrialFailed) { StepStatus::Blocked }
            else if completed > 0 || measured.iter().any(|s| s.state > ShotState::Unconfigured) { StepStatus::InProgress }
            else { StepStatus::NotStarted };
        if captured && measured.is_empty() { blockers.push(Blocker { message: "至少需要一个参与检测的拍照点".into(), target: Target::Teach { k: 0 } }); }
        Step { id: StepId::Teach, label: "单帧示教", optional: false, status,
            detail: if captured { format!("{completed} / {total} 幅已示教") } else { "整圈采集后逐点示教".into() },
            blockers: if captured { blockers } else { need_capture() } }
    };

    let calibration = {
        let views: Vec<_> = measured.iter().flat_map(|s| s.views.iter().filter(|v| v.enabled).map(move |v| (*s, v))).collect();
        let satisfied = views.iter().filter(|(_, v)| v.calibration.satisfied()).count();
        let declared: Vec<_> = views.iter().filter_map(|(_, v)| match &v.calibration { CalibrationState::SampleSet { set_id, .. } => Some(set_id.as_str()), _ => None }).collect();
        let blockers: Vec<_> = views.iter().filter(|(_, v)| !v.calibration.satisfied())
            .map(|(s, v)| Blocker { message: format!("{} 图 {}：{}", label(s), v.view, calibration_text(&v.calibration)), target: Target::Teach { k: s.k } }).collect();
        let status = if !captured || views.is_empty() { StepStatus::NotStarted }
            else if blockers.is_empty() { StepStatus::Done }
            else if views.iter().any(|(_, v)| matches!(v.calibration, CalibrationState::Failed { .. })) { StepStatus::Blocked }
            else if satisfied > 0 { StepStatus::InProgress }
            else { StepStatus::NotStarted };
        let detail = if !captured { "整圈采集后逐图确认像素当量".into() }
            else if !declared.is_empty() && declared.len() == views.len() { format!("样本集声明当量（{}）", declared[0]) }
            else if declared.is_empty() { format!("{satisfied} / {} 幅满足 ±0.1 mm", views.len()) }
            else { format!("{satisfied} / {} 幅已满足（其中 {} 幅为样本集声明当量）", views.len(), declared.len()) };
        Step { id: StepId::Calibration, label: "像素当量与标定验证", optional: false, status, detail,
            blockers: if captured { blockers } else { need_capture() } }
    };

    let overview = Step { id: StepId::Overview, label: "工件总览", optional: true,
        status: if w.overview.saved { StepStatus::Done } else { StepStatus::Optional },
        detail: if w.overview.saved { "总览布置已保存".into() } else { "可选：未布置时按默认网格显示".into() }, blockers: Vec::new() };

    let active = match &w.publish { PublishState::Active { version, revision, .. } if *revision == w.revision => Some(*version), _ => None };
    let site = |offline: bool| if offline { "（离线验证）" } else { "" };
    let active_offline = matches!(w.publish, PublishState::Active { validation_site: ValidationSite::Offline, .. });
    let pending_offline = w.pending.as_ref().is_some_and(|p| p.validation_site == ValidationSite::Offline);
    let prerequisites = [&capture, &teach, &calibration];
    let validation = {
        let current = w.validation.as_ref().filter(|v| v.revision == w.revision);
        let first_open = shots.iter().find(|s| s.state != ShotState::Taught && s.state != ShotState::Skipped).map(|s| s.k);
        let (status, detail, blockers) = if let Some(version) = active {
            (StepStatus::Done, format!("已随 v{version} 验证并发布"), Vec::new())
        } else if let Some(v) = current {
            let blockers: Vec<_> = v.checks.iter().filter(|c| !c.passed).map(|c| Blocker { message: format!("{}：{}", c.name, c.detail), target: check_target(&c.name, first_open) }).collect();
            if v.passed { (StepStatus::Done, format!("当前修订验证通过（{} 件样本）", v.samples.len()), Vec::new()) }
            else { (StepStatus::Blocked, "验证未通过".into(), blockers) }
        } else {
            let blockers: Vec<_> = prerequisites.iter().filter(|s| s.status != StepStatus::Done)
                .map(|s| Blocker { message: format!("先完成{}", s.label), target: s.blockers.first().map(|b| b.target.clone()).unwrap_or(Target::Capture) }).collect();
            let detail = if w.validation.is_some() { "候选已修改，需要重新验证" } else { "待独立正常整圈样本验证" };
            (StepStatus::NotStarted, detail.into(), blockers)
        };
        Step { id: StepId::Validation, label: "验证", optional: false, status, detail, blockers }
    };

    let publish = {
        let production = production_version.map_or("未发布".to_string(), |v| format!("生产 v{v}"));
        let pending = w.pending.as_ref().map(|p| p.doc.version);
        let (status, detail, blockers) = match (&w.publish, active) {
            (_, Some(version)) => (StepStatus::Done, format!("v{version} 已生效{}", site(active_offline)), Vec::new()),
            (PublishState::Waiting, _) => (StepStatus::InProgress, format!("v{} 待生效{}：等待当前工件结束", pending.unwrap_or(w.doc.version), site(pending_offline)), Vec::new()),
            (PublishState::Failed { message }, _) => (StepStatus::Blocked, format!("v{} 生效失败，可重试或放弃该待生效版本", pending.unwrap_or(w.doc.version)),
                vec![Blocker { message: message.clone(), target: Target::Validation }]),
            (PublishState::Uncertain { message }, _) => (StepStatus::Blocked, format!("v{} 生效状态不确定", pending.unwrap_or(w.doc.version)),
                vec![Blocker { message: message.clone(), target: Target::Validation }]),
            _ if validation.status == StepStatus::Done => (StepStatus::NotStarted, format!("可发布 v{}（当前{production}）", w.doc.version), Vec::new()),
            _ => (StepStatus::NotStarted, format!("验证通过后发布（当前{production}）"), vec![Blocker { message: "先通过当前修订的验证".into(), target: Target::Validation }]),
        };
        Step { id: StepId::Publish, label: "发布与生效", optional: false, status, detail, blockers }
    };

    let next = if capture.status != StepStatus::Done {
        Some(NextAction { step: StepId::Capture, label: "整圈采集".into(), target: Target::Capture })
    } else if let Some(open) = [&teach, &calibration].into_iter().find(|s| s.status != StepStatus::Done) {
        let target = open.blockers.first().map(|b| b.target.clone()).unwrap_or(Target::Teach { k: 0 });
        let text = match (&target, open.id) {
            (Target::Teach { k }, StepId::Teach) => format!("继续示教拍照点 {}", k + 1),
            (Target::Teach { k }, _) => format!("验证拍照点 {} 的像素当量", k + 1),
            _ => open.label.to_string(),
        };
        Some(NextAction { step: open.id, label: text, target })
    } else if validation.status != StepStatus::Done {
        Some(NextAction { step: StepId::Validation, label: "运行整套验证".into(), target: validation.blockers.first().map(|b| b.target.clone()).unwrap_or(Target::Validation) })
    } else {
        match publish.status {
            StepStatus::Done => None,
            StepStatus::InProgress => Some(NextAction { step: StepId::Publish, label: "等待工件边界生效".into(), target: Target::Validation }),
            StepStatus::Blocked => Some(NextAction { step: StepId::Publish, label: "处理发布失败".into(), target: Target::Validation }),
            _ => Some(NextAction { step: StepId::Publish, label: "发布生产配方".into(), target: Target::Validation }),
        }
    };

    Progress { steps: vec![capture, defaults, teach, calibration, overview, validation, publish], shots, next }
}
