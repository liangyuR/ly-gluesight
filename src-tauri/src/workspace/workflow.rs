use super::*;
use crate::recipe::ShotViewSpec;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationCheck {
    pub verification_id: String,
    pub source_k: usize,
    pub source_image_id: String,
    pub basis: CalibrationBasis,
    pub passed: bool,
    pub camera_tag: Value,
    pub calib_tag: Value,
    pub mm_per_px: Option<f32>,
}

/// 标定验证的依据：人工参考件实测，或样本集声明的像素当量（不伪造参考 / 实测值）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CalibrationBasis {
    Manual { reference_mm: f64, measured_mm: f64, error_mm: f64 },
    SampleSet { set_id: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewTeaching {
    pub view: u8,
    pub trial: Option<Trial>,
    pub saved: bool,
    pub calibration_check: Option<CalibrationCheck>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Position { pub k: usize, pub view: u8 }

pub(super) fn store_active(w: &mut Workspace, k: usize) {
    let Some(shot) = w.doc.shots.get_mut(k) else { return };
    if let Some(v) = shot.views.iter_mut().find(|v| v.view == shot.view) {
        v.path = shot.path.clone(); v.mm_per_px = shot.mm_per_px;
        v.detect = shot.detect.clone(); v.limits = shot.limits.clone(); v.calib = shot.calib.clone();
        v.fixture = shot.fixture.clone();
    }
    let Some(frame) = w.frames.get_mut(k) else { return };
    if let Some(state) = frame.view_states.iter_mut().find(|s| s.view == shot.view) {
        state.trial = frame.trial.clone(); state.saved = frame.saved;
    } else {
        frame.view_states.push(ViewTeaching { view: shot.view, trial: frame.trial.clone(), saved: frame.saved, calibration_check: None });
    }
}

pub(super) fn load_active(w: &mut Workspace, k: usize, selected: u8) -> Result<(), String> {
    let shot = w.doc.shots.get_mut(k).ok_or("拍照点不存在")?;
    let v = shot.views.iter().find(|v| v.view == selected).ok_or("本拍照点没有这幅图")?;
    shot.view = selected; shot.path = v.path.clone(); shot.mm_per_px = v.mm_per_px;
    shot.detect = v.detect.clone(); shot.limits = v.limits.clone(); shot.calib = v.calib.clone();
    shot.fixture = v.fixture.clone();
    let frame = w.frames.get_mut(k).ok_or("拍照点不存在")?;
    frame.image = frame.views.iter().find(|i| i.view == selected).cloned();
    let state = frame.view_states.iter().find(|s| s.view == selected);
    frame.trial = state.and_then(|s| s.trial.clone());
    frame.saved = state.is_some_and(|s| s.saved);
    Ok(())
}

pub(super) fn initialize_views(w: &mut Workspace, k: usize) {
    let shot = &mut w.doc.shots[k];
    if !shot.views.is_empty() { return; }
    let available: Vec<_> = w.frames[k].views.iter().map(|i| i.view).collect();
    for view in available {
        let active = view == shot.view;
        shot.views.push(ShotViewSpec { view, enabled: active, path: if active { shot.path.clone() } else { Vec::new() },
            mm_per_px: if active { shot.mm_per_px } else { None }, detect: if active { shot.detect.clone() } else { None },
            limits: if active { shot.limits.clone() } else { None }, calib: if active { shot.calib.clone() } else { None },
            fixture: if active { shot.fixture.clone() } else { None } });
    }
    store_active(w, k);
}

pub(super) fn projected_frame(w: &Workspace, k: usize, selected: u8) -> Result<Teaching, String> {
    let mut frame = w.frames.get(k).ok_or("拍照点不存在")?.clone();
    frame.image = frame.views.iter().find(|i| i.view == selected).cloned();
    if w.doc.shots[k].view != selected || !frame.view_states.is_empty() {
        let state = frame.view_states.iter().find(|s| s.view == selected);
        frame.trial = state.and_then(|s| s.trial.clone());
        frame.saved = state.is_some_and(|s| s.saved);
    }
    Ok(frame)
}

pub(super) fn selected_views(w: &Workspace, k: usize) -> Vec<u8> {
    let shot = &w.doc.shots[k];
    if shot.skip { return Vec::new(); }
    if shot.views.is_empty() { return vec![shot.view]; }
    shot.views.iter().filter(|v| v.enabled).map(|v| v.view).collect()
}

fn checked_view(app: &AppHandle, w: &Workspace, k: usize, selected: u8) -> Result<(Recipe, Teaching), String> {
    let r = w.doc.build()?.for_view(k, selected)?;
    let frame = projected_frame(w, k, selected)?;
    if !frame.saved { return Err(format!("拍照点 {} 图 {selected} 尚未完成示教", k + 1)); }
    let (image, params) = shot_tags(&r, k)?;
    frame.checked(&image, &params, r.shots[k].fixture.is_some())?;
    let engine = app.state::<vision::VisionHost>().engine().ok_or("图像核心库未加载")?;
    frame.checked_engine(&engine_state(&engine))?;
    Ok((r, frame))
}

pub(super) fn check_history_sample_view(app: &AppHandle, w: &Workspace, k: usize, selected: u8) -> Result<(), String> {
    let (recipe, frame) = checked_view(app, w, k, selected)?;
    let current = calibration_state(&vision::shot_calib_path(app, &recipe, k)?)?;
    let frozen = frame.image.as_ref().ok_or("示教原图缺失")?;
    if frozen.calib_tag != current { return Err("标定变化，请重新确认示教后再复测离线样本原图".into()); }
    Ok(())
}

pub(super) fn check_view(app: &AppHandle, w: &Workspace, k: usize, selected: u8, calibration: bool) -> Result<(), String> {
    let (r, frame) = checked_view(app, w, k, selected)?;
    let (ct, cal) = tags(app, &r, k)?;
    let frozen = frame.image.as_ref().ok_or("示教原图缺失")?;
    if frozen.camera_tag != ct || frozen.calib_tag != cal { return Err("设备或标定变化，请重新确认示教".into()); }
    if calibration {
        let checked = w.frames[k].view_states.iter().find(|s| s.view == selected)
            .and_then(|s| s.calibration_check.as_ref()).ok_or("尚未验证 ±0.1 mm 标定误差")?;
        if !checked.passed || checked.camera_tag != ct || checked.calib_tag != cal || checked.mm_per_px != r.shots[k].mm_per_px {
            return Err("标定误差验证未通过或已经失效".into());
        }
    }
    Ok(())
}

fn doc_group_selection_errors(doc: &RecipeDoc) -> Vec<(usize, String)> {
    let mut groups: Vec<(usize, &str, usize)> = Vec::new();
    for (k, shot) in doc.shots.iter().enumerate() {
        let count = shot.enabled_views().len();
        if let Some(group) = groups.iter_mut().find(|(_, pose, _)| *pose == shot.pose_id) {
            group.2 += count;
        } else {
            groups.push((k, &shot.pose_id, count));
        }
    }
    groups.into_iter().filter(|(_, _, count)| *count != 1).map(|(k, pose, count)| {
        (k, if count == 0 { format!("拍照点组 {pose}：请选择一张图示教") }
            else { format!("拍照点组 {pose}：已选 {count} 张图，每组只能选择一张") })
    }).collect()
}

impl Release {
    pub fn check_group_selection(&self) -> Result<(), String> {
        match doc_group_selection_errors(&self.doc).into_iter().next() {
            Some((_, message)) => Err(message),
            None => Ok(()),
        }
    }
}

impl Workspace {
    pub fn group_selection_errors(&self) -> Vec<(usize, String)> {
        doc_group_selection_errors(&self.doc)
    }

}

#[derive(Serialize)]
pub struct CenterlineResult {
    pub path: Vec<[f32; 2]>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<super::centerline::ExtractionFailure>,
}

#[tauri::command]
pub fn workspace_retry_publish(app: AppHandle, id: String, revision: u64) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>(); let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone();
    w.retry_publish(revision)?;
    host.save(&w)?; items.insert(id, w.clone());
    let _ = app.state::<CycleHost>().tx.send(cycle::Input::Refresh);
    view(&app, w)
}

#[tauri::command]
pub fn workspace_abandon_publish(app: AppHandle, id: String, revision: u64) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>(); let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone();
    w.abandon_publish(revision)?;
    host.save(&w)?; items.insert(id, w.clone());
    view(&app, w)
}

/// 有界等待配置门：布防、整圈采集开始或离线切换正在进行时排队等它释放，而不是立即拒绝。
/// 只能在阻塞线程（spawn_blocking 或普通线程）里调用，不能在 async 任务里调用。
pub(crate) fn lock_config_gate<'a>(
    gate: &'a tokio::sync::Mutex<()>,
    within: std::time::Duration,
    what: &str,
) -> Result<tokio::sync::MutexGuard<'a, ()>, String> {
    tauri::async_runtime::block_on(async { tokio::time::timeout(within, gate.lock()).await })
        .map_err(|_| format!("布防或其他配置操作 3 秒内仍未结束，暂不能{what}；请确认在线检测或离线产线已停止后重试"))
}

/// 等节拍释放上一次握手（busy 清除）后再取配置门；整段最多 3 秒。
pub(crate) fn wait_idle_config_gate<'a>(
    cycle: &'a CycleHost,
    what: &str,
) -> Result<Option<tokio::sync::MutexGuard<'a, ()>>, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        while cycle.busy() && std::time::Instant::now() < deadline { std::thread::sleep(std::time::Duration::from_millis(50)); }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let gate = lock_config_gate(&cycle.config_gate, left, what)?;
        if !cycle.busy() { return Ok(Some(gate)); }
        drop(gate);
        if std::time::Instant::now() >= deadline { return Ok(None); }
    }
}

pub(super) fn require_capture_plan(version: Option<u32>) -> Result<u32, String> {
    version.filter(|version| *version > 0).ok_or_else(|| "采集轮次缺少有效的 PLC 轨迹计划版本，请重新采集".into())
}

pub(super) fn capture_for(app: &AppHandle, id: &str, round_id: &str) -> Result<crate::recipe_capture::CaptureRound, String> {
    let round = app.state::<CycleHost>().camera.recipe_capture().get(round_id)?;
    if round.recipe_id != id || round.state != crate::recipe_capture::CaptureState::Complete || round.frames.is_empty()
        || !round.planned_count.is_some_and(|n| (1..=64).contains(&n) && round.cameras.iter().all(|c| c.received_count == n)
            && round.frames.len() == n as usize && round.complete_shots() == n) {
        return Err("采集轮次不属于当前配方或尚未完整完成，不能采用".into());
    }
    require_capture_plan(round.plc_plan_version)?;
    let configs = app.state::<CycleHost>().camera.configs();
    for camera in &round.cameras {
        let config = configs.iter().find(|c| c.id == camera.camera_id).ok_or_else(|| format!("采集相机 {} 已不在设备配置里", camera.camera_id))?;
        if state_value(config) != camera.camera_config { return Err(format!("采集后相机 {} 的参数或拼接布局已改变，请重新采集", camera.camera_id)); }
    }
    Ok(round)
}

#[tauri::command]
pub async fn workspace_capture_sample(app: AppHandle, id: String, revision: u64, round_id: String, expected: Verdict, correspondence_confirmed: bool) -> Result<WorkspaceView, String> {
    if !correspondence_confirmed { return Err("请确认验证采集与示教的轨迹、触发顺序对应一致".into()); }
    tauri::async_runtime::spawn_blocking(move || capture_sample(app, id, revision, round_id, expected)).await.map_err(|e| e.to_string())?
}

fn capture_sample(app: AppHandle, id: String, revision: u64, round_id: String, expected: Verdict) -> Result<WorkspaceView, String> {
    // 段配方的样本是新采集轮次（CaptureRoundV2），规则见 burst_release（D3）；以下是旧拍照点配方，行为不变
    let burst = app.state::<WorkspaceHost>().items.lock().unwrap().get(&id).is_some_and(|w| w.doc.is_burst());
    if burst { return super::burst_release::capture_sample(app, id, revision, round_id, expected); }
    let cycle = app.state::<CycleHost>();
    let Some(_gate) = wait_idle_config_gate(&cycle, "加入验证样本")? else {
        return Err("请等待当前工件和 PLC 事务结束后加入验证样本".into());
    };
    let round = capture_for(&app, &id, &round_id)?;
    let host = app.state::<WorkspaceHost>(); let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone(); w.expect(revision)?;
    if w.capture_id.as_deref() == Some(&round_id) { return Err("示教采集不能同时作为独立验证样本，请重新采集一圈".into()); }
    let teaching_round = capture_for(&app, &id, w.capture_id.as_deref().ok_or("尚未采用示教采集")?)?;
    if teaching_round.plc_plan_version != round.plc_plan_version { return Err("验证采集与示教的 PLC 轨迹计划版本不同".into()); }
    let expanded = round.recipe_shots()?;
    if w.doc.shots.len() != expanded.len() || w.doc.shots.iter().zip(&expanded).any(|(s, e)| s.camera != e.camera.camera_id) { return Err("验证采集的点数或相机与候选不同".into()); }
    if w.sample_bank.iter().any(|b| b.id == round_id) { return view(&app, w); }
    if w.sample_bank.len() >= 100 { return Err("最多保留 100 组验证样本".into()); }
    let recipe = w.doc.build()?;
    let dir = host.dir(&id).join("samples").join(&round_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for (k, captured) in expanded.iter().enumerate() {
        for raw in &captured.frame.views {
            let image = crate::image_file::load(Path::new(&raw.path))?;
            let frozen = w.frames[k].views.iter().find(|i| i.view == raw.view).ok_or("样本的图编号与示教不一致")?;
            if [image.width, image.height] != frozen.size { return Err("验证样本与示教图的尺寸不一致".into()); }
            crate::image_file::save_pgm(&dir.join(format!("k{k}-v{}.pgm", raw.view)), &image)?;
        }
    }
    w.sample_bank.push(BankSample { id: round_id.clone(), name: format!("独立采集 {}", round.created_at), geometry_tag: images_tag(&recipe), expected, created_at: round.created_at, capture_id: Some(round_id.clone()), camera_state: state_value(&round.cameras.iter().map(|c| &c.camera_config).collect::<Vec<_>>()) });
    w.samples.push(Sample { history_id: None, sample_id: Some(round_id), expected });
    w.changed(); host.save(&w)?; items.insert(id, w.clone()); view(&app, w)
}
