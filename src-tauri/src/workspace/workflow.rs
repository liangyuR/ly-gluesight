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

pub(super) fn calibration_scale(tag: &Value) -> Option<f32> {
    let value = tag.get("value")?;
    value.get("data").unwrap_or(value).get("mmPerPx").and_then(Value::as_f64).map(|v| v as f32)
}

pub(super) fn updated_calibration_scale(current: Option<f32>, previous: &Value, next: Option<f32>) -> Option<f32> {
    if current.is_none() || current == calibration_scale(previous) { next } else { current }
}

pub(super) fn reusable_calibration(check: &CalibrationCheck, camera: &Value, calib: &Value, scale: Option<f32>, image: &FrozenImage) -> bool {
    check.passed && *camera == check.camera_tag && *calib == check.calib_tag && scale == check.mm_per_px
        && image.camera_tag == *camera && image.calib_tag == *calib
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

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftTeach {
    pub path: Vec<[f32; 2]>,
    pub mm_per_px: Option<f32>,
    pub detect: Option<DetectParams>,
    pub limits: Option<crate::recipe::ShotLimits>,
    /// 必须显式发送（可以是 null），漏发时报错，不会悄悄清掉已保存的 fixture
    #[serde(deserialize_with = "Option::deserialize")]
    pub fixture: Option<crate::recipe::FixtureSpec>,
}

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

    pub fn select_group_image(&mut self, k: usize, selected: u8) -> Result<(), String> {
        let pose = self.doc.shots.get(k).ok_or("拍照点不存在")?.pose_id.clone();
        if !self.frames.get(k).is_some_and(|frame| frame.views.iter().any(|image| image.view == selected)) {
            return Err("本轮没有所选图像".into());
        }
        let mut next = self.clone();
        let group: Vec<_> = next.doc.shots.iter().enumerate().filter(|(_, shot)| shot.pose_id == pose).map(|(index, _)| index).collect();
        for index in group {
            initialize_views(&mut next, index);
            store_active(&mut next, index);
            let shot = &mut next.doc.shots[index];
            shot.skip = index != k;
            for image in &mut shot.views { image.enabled = index == k && image.view == selected; }
        }
        load_active(&mut next, k, selected)?;
        next.last_position = Some(Position { k, view: selected });
        next.changed();
        *self = next;
        Ok(())
    }
}

#[tauri::command]
pub fn workspace_select_group_image(app: AppHandle, id: String, revision: u64, k: usize, view: u8) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone();
    w.expect(revision)?;
    w.select_group_image(k, view)?;
    host.save(&w)?; items.insert(id, w.clone()); super::view(&app, w)
}

#[tauri::command]
pub fn workspace_set_views(app: AppHandle, id: String, revision: u64, k: usize, views: Vec<u8>, skip: bool) -> Result<WorkspaceView, String> {
    let unique: std::collections::HashSet<_> = views.iter().copied().collect();
    if unique.len() != views.len() || views.iter().any(|v| !(1..=3).contains(v)) { return Err("图编号必须为不重复的 1–3".into()); }
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone();
    w.expect(revision)?;
    let frame = w.frames.get(k).ok_or("拍照点不存在")?;
    if views.iter().any(|v| !frame.views.iter().any(|i| i.view == *v)) { return Err("本轮没有所选图像".into()); }
    initialize_views(&mut w, k);
    store_active(&mut w, k);
    for v in &mut w.doc.shots[k].views { v.enabled = views.contains(&v.view); }
    w.doc.shots[k].skip = skip;
    w.changed();
    host.save(&w)?; items.insert(id, w.clone()); view(&app, w)
}

#[tauri::command]
pub fn workspace_save_draft(app: AppHandle, id: String, revision: u64, k: usize, params: DraftTeach) -> Result<WorkspaceView, String> {
    if params.path.len() > 10000 || params.path.iter().flatten().any(|v| !v.is_finite()) || params.mm_per_px.is_some_and(|v| !v.is_finite() || v <= 0.0) {
        return Err("中线和比例必须为有效数值；未标定时请留空".into());
    }
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone();
    w.expect(revision)?;
    apply_draft(&mut w, k, params)?;
    host.save(&w)?; items.insert(id, w.clone()); view(&app, w)
}

/// 草稿自动保存：先按 `bind_fixture_sources` 定模板来源（没改的框沿用旧来源，不会悄悄改绑到重新冻结的新图），再判断有没有改动。
pub(super) fn apply_draft(w: &mut Workspace, k: usize, mut params: DraftTeach) -> Result<(), String> {
    if params.fixture.is_some() {
        let mut doc = w.doc.clone();
        doc.shots.get_mut(k).ok_or("拍照点不存在")?.fixture = params.fixture.take();
        let selected = doc.shots[k].view;
        let fixture = doc.shots[k].fixture.clone();
        if let Some(v) = doc.shots[k].views.iter_mut().find(|v| v.view == selected) { v.fixture = fixture; }
        super::fixture::reject_unconfirmed_rotation(&w.doc, &doc)?;
        super::fixture::reject_end_param_edit(&w.doc, &doc)?;
        bind_fixture_sources(w, &mut doc, None)?;
        params.fixture = doc.shots[k].fixture.take();
    }
    let shot = w.doc.shots.get_mut(k).ok_or("拍照点不存在")?;
    if shot.path != params.path || shot.mm_per_px != params.mm_per_px || shot.detect != params.detect || shot.limits != params.limits || shot.fixture != params.fixture {
        shot.path = params.path; shot.mm_per_px = params.mm_per_px; shot.detect = params.detect; shot.limits = params.limits; shot.fixture = params.fixture;
        w.frames[k].saved = false; w.frames[k].trial = None;
        store_active(w, k);
        w.doc.build()?;
        w.changed();
    }
    Ok(())
}

#[tauri::command]
pub fn workspace_progress(app: AppHandle, id: String, k: usize, view: u8) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone();
    if k >= w.frames.len() || !(1..=3).contains(&view) { return Err("编辑位置无效".into()); }
    w.last_position = Some(Position { k, view });
    host.save(&w)?; items.insert(id, w.clone()); super::view(&app, w)
}

#[tauri::command]
pub fn workspace_check_calibration(app: AppHandle, id: String, revision: u64, k: usize, reference_mm: f64, measured_mm: f64, reuse_matching: Option<bool>) -> Result<WorkspaceView, String> {
    if !reference_mm.is_finite() || !measured_mm.is_finite() || reference_mm <= 0.0 || measured_mm <= 0.0 { return Err("参考尺寸与实测尺寸必须为正数".into()); }
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone(); w.expect(revision)?;
    let selected = w.doc.shots.get(k).ok_or("拍照点不存在")?.view;
    let r = w.doc.build()?.for_view(k, selected)?;
    let (camera_tag, calib_tag) = tags(&app, &r, k)?;
    if r.shots[k].mm_per_px.is_none() { return Err("请先完成当前图的毫米标定".into()); }
    let image = w.frames[k].views.iter_mut().find(|i| i.view == selected).ok_or("当前图尚未采集")?;
    if image.camera_tag != camera_tag { return Err("采集后设备配置已经改变，请重新采集".into()); }
    let source_image_id = image.id.clone();
    if image.calib_tag != calib_tag {
        image.calib_tag = calib_tag.clone();
        w.frames[k].image = Some(image.clone());
        w.frames[k].saved = false; w.frames[k].trial = None;
    }
    store_active(&mut w, k);
    let error_mm = measured_mm - reference_mm;
    let check = CalibrationCheck {
        verification_id: format!("calibration-{}-{revision}-{k}-{selected}", ly_plc::now_ms()), source_k: k, source_image_id,
        basis: CalibrationBasis::Manual { reference_mm, measured_mm, error_mm }, passed: error_mm.abs() <= 0.1 + 1e-9,
        camera_tag, calib_tag, mm_per_px: r.shots[k].mm_per_px,
    };
    w.frames[k].view_states.iter_mut().find(|s| s.view == selected).unwrap().calibration_check = Some(check.clone());
    if reuse_matching == Some(true) { reuse_calibration(&mut w, k, selected, &check, |recipe, other| tags(&app, recipe, other))?; }
    w.changed(); host.save(&w)?; items.insert(id, w.clone()); view(&app, w)
}

/// 把通过的人工标定验证复用到同一相机、同一图号且设备与标定一致的其他拍照点：
/// 未设当量的点带入本次当量，已设成其他当量的点不覆盖。返回复用的拍照点数。
pub(super) fn reuse_calibration(w: &mut Workspace, k: usize, selected: u8, check: &CalibrationCheck, tags_of: impl Fn(&Recipe, usize) -> Result<(Value, Value), String>) -> Result<usize, String> {
    if !check.passed { return Ok(0); }
    let recipe = w.doc.build()?;
    let mut reused = 0;
    for other in 0..w.frames.len() {
        if other == k || recipe.shots[other].camera != recipe.shots[k].camera { continue; }
        let Some(spec) = w.doc.shots[other].views.iter().find(|v| v.view == selected) else { continue };
        if spec.mm_per_px.is_some_and(|mm| Some(mm) != check.mm_per_px) { continue; }
        let Ok(projected) = recipe.for_view(other, selected) else { continue };
        let Ok((camera, calib)) = tags_of(&projected, other) else { continue };
        if !w.frames[other].views.iter().any(|image| image.view == selected && reusable_calibration(check, &camera, &calib, check.mm_per_px, image)) { continue; }
        let shot = &mut w.doc.shots[other];
        if let Some(v) = shot.views.iter_mut().find(|v| v.view == selected) { v.mm_per_px = check.mm_per_px; }
        if shot.view == selected { shot.mm_per_px = check.mm_per_px; }
        let frame = &mut w.frames[other];
        match frame.view_states.iter_mut().find(|state| state.view == selected) {
            Some(state) => state.calibration_check = Some(check.clone()),
            None => frame.view_states.push(ViewTeaching { view: selected, trial: None, saved: false, calibration_check: Some(check.clone()) }),
        }
        reused += 1;
    }
    Ok(reused)
}

#[derive(Serialize)]
pub struct CenterlineResult {
    pub path: Vec<[f32; 2]>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<super::centerline::ExtractionFailure>,
}

#[tauri::command]
pub async fn workspace_extract_centerline(app: AppHandle, id: String, revision: u64, k: usize, image_id: String, roi: [u32; 4], guide: Option<super::centerline::BrushGuide>, point_spacing_px: Option<f32>) -> Result<CenterlineResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let (path, polarity) = {
            let items = host.items.lock().unwrap(); let w = items.get(&id).ok_or("候选不存在")?; w.expect(revision)?;
            let frame = w.frames.get(k).ok_or("拍照点不存在")?;
            if frame.image.as_ref().is_none_or(|i| i.id != image_id) { return Err("图像已变化，请重新框选".into()); }
            let r = w.doc.build()?.for_view(k, w.doc.shots[k].view)?;
            (image_file(&host, &id, &image_id)?, r.shot_detect(k).polarity)
        };
        let image = crate::image_file::load(&path)?;
        let result = super::centerline::extract(&image, roi, polarity, guide.as_ref(), point_spacing_px.unwrap_or(20.0));
        let items = host.items.lock().unwrap(); let current = items.get(&id).ok_or("候选不存在")?; current.expect(revision)?;
        if current.frames[k].image.as_ref().is_none_or(|i| i.id != image_id) { return Err("提取期间图像已变化".into()); }
        match result {
            Ok(extracted) => Ok(CenterlineResult { path: extracted.path, message: extracted.notice.unwrap_or_else(|| "中线已提取，请检查并人工修正后试测".into()), failure: None }),
            Err(super::centerline::ExtractionError::Failed(failure)) => Ok(CenterlineResult { path: Vec::new(), message: failure.message.clone(), failure: Some(failure) }),
            Err(super::centerline::ExtractionError::Invalid(message)) => Err(message),
        }
    }).await.map_err(|e| e.to_string())?
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
pub(crate) fn lock_config_gate<'a>(gate: &'a tokio::sync::Mutex<()>, within: std::time::Duration, what: &str) -> Result<tokio::sync::MutexGuard<'a, ()>, String> {
    tauri::async_runtime::block_on(async { tokio::time::timeout(within, gate.lock()).await })
        .map_err(|_| format!("布防或其他配置操作 3 秒内仍未结束，暂不能{what}；请确认在线检测或离线产线已停止后重试"))
}

/// 等节拍释放上一次握手（busy 清除）后再取配置门；整段最多 3 秒。
pub(crate) fn wait_idle_config_gate<'a>(cycle: &'a CycleHost, what: &str) -> Result<Option<tokio::sync::MutexGuard<'a, ()>>, String> {
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

/// 按采集轮次重建候选的拍照点与相机曝光增益。全部拍照点加完才校验：多相机时只加了第一台相机的点就校验会误判其余相机未被使用。
pub(super) fn adopt_shots(w: &mut Workspace, round: &crate::recipe_capture::CaptureRound, reuse_teaching: bool) -> Result<(), String> {
    if w.sample_bank.iter().any(|sample| sample.capture_id.as_deref() == Some(round.round_id.as_str())) {
        return Err("本轮已作为独立验证样本加入样本库，不能再采用为示教图像：示教与验证样本必须来自不同的采集轮次，请重新采集一圈用于示教".into());
    }
    let expanded = round.recipe_shots()?;
    let old = std::mem::take(&mut w.doc.shots);
    if reuse_teaching && old.len() != expanded.len() { return Err("继承示教前必须确认点数、相机、轨迹及触发顺序对应一致".into()); }
    let used = round.camera_ids();
    let imaging: Vec<_> = round.cameras.iter().map(|c| (c.camera_id.clone(), w.doc.cameras.get(&c.camera_id).or(w.unbound_imaging.get(&c.camera_id)).copied().unwrap_or(c.imaging))).collect();
    let others: Vec<_> = std::mem::take(&mut w.doc.cameras).into_iter().filter(|(c, _)| !used.contains(c)).collect();
    w.unbound_imaging.extend(others);
    for (camera, imaging) in imaging {
        w.unbound_imaging.remove(&camera);
        w.doc.cameras.insert(camera, imaging);
    }
    for (k, captured) in expanded.iter().enumerate() {
        let camera_id = &captured.camera.camera_id;
        let mut shot = if reuse_teaching { old[k].clone() } else {
            crate::recipe::ShotSpec { id: captured.shot_id.clone(), pose_id: captured.pose_id.clone(), camera: camera_id.clone(), view: 1, calib: None,
                bead: "J1".into(), skip: false, path: Vec::new(), mm_per_px: None, detect: None, limits: None, views: Vec::new(), fixture: None }
        };
        if reuse_teaching && shot.camera != *camera_id { return Err("采集相机或相机顺序已改变，不能按原对应关系继承示教".into()); }
        if !reuse_teaching {
            shot.views = captured.frame.views.iter().map(|v| ShotViewSpec { view: v.view, enabled: false, path: Vec::new(), mm_per_px: None,
                detect: None, limits: None, calib: None, fixture: None }).collect();
        }
        w.doc.shots.push(shot);
    }
    w.doc.validate()
}

#[tauri::command]
pub async fn workspace_adopt_capture(app: AppHandle, id: String, revision: u64, round_id: String, reuse_teaching: bool, correspondence_confirmed: bool) -> Result<WorkspaceView, String> {
    tauri::async_runtime::spawn_blocking(move || adopt_capture(app, id, revision, round_id, reuse_teaching, correspondence_confirmed)).await.map_err(|e| e.to_string())?
}

fn adopt_capture(app: AppHandle, id: String, revision: u64, round_id: String, reuse_teaching: bool, correspondence_confirmed: bool) -> Result<WorkspaceView, String> {
    let cycle = app.state::<CycleHost>();
    let Some(_gate) = wait_idle_config_gate(&cycle, "采用采集图像")? else { return Err("请等待当前工件和 PLC 事务结束后采用采集图像".into()) };
    let round = capture_for(&app, &id, &round_id)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone(); w.expect(revision)?;
    if w.capture_id.as_deref() == Some(&round_id) { return view(&app, w); }
    let expanded = round.recipe_shots()?;
    if reuse_teaching && !correspondence_confirmed {
        return Err("继承示教前必须确认点数、相机、轨迹及触发顺序对应一致".into());
    }
    let configs = app.state::<CycleHost>().camera.configs();
    let offline_set = crate::offline::sample_set(&app);
    let mut sample_sets = std::collections::BTreeMap::new();
    for camera in &round.cameras {
        let config = configs.iter().find(|c| c.id == camera.camera_id).ok_or_else(|| format!("采集相机 {} 不存在", camera.camera_id))?;
        let set = offline_set.clone().filter(|set| config.source == CameraSource::SampleSet && set.has_camera(&camera.camera_id));
        sample_sets.insert(camera.camera_id.clone(), set);
    }
    let old = w.clone();
    w.frames.clear();
    adopt_shots(&mut w, &round, reuse_teaching)?;
    for (k, captured) in expanded.iter().enumerate() {
        let camera_id = &captured.camera.camera_id;
        let mut frame = Teaching::empty(k);
        let declared = sample_sets.get(camera_id).cloned().flatten().and_then(|set| {
            let shot = set.manifest().shots.iter().filter(|s| s.camera_id == *camera_id).nth(captured.k)?;
            set.shot_mm_per_px(&shot.shot_id).map(|mm| (set.manifest().id.clone(), mm))
        });
        for raw in &captured.frame.views {
            let image = crate::image_file::load(Path::new(&raw.path))?;
            if [image.width, image.height] != [raw.width, raw.height] { return Err("采集图尺寸与记录不一致".into()); }
            let recipe = w.doc.build()?;
            let mut projection = recipe.clone();
            projection.shots[k].view = raw.view;
            projection.shots[k].calib = w.doc.shots[k].views.iter().find(|v| v.view == raw.view).and_then(|v| v.calib.clone());
            let (camera_tag, calib_tag) = tags(&app, &projection, k)?;
            let calibrated = vision::calib_info(&vision::shot_calib_path(&app, &projection, k)?).and_then(|c| c.mm_per_px).map(|scale| scale as f32);
            let scale = declared.as_ref().map(|(_, mm)| *mm).or(calibrated);
            let previous_calib = old.frames.get(k).and_then(|f| f.views.iter().find(|i| i.view == raw.view)).map(|i| &i.calib_tag).unwrap_or(&Value::Null);
            let inherited_scale = if let Some(v) = w.doc.shots[k].views.iter_mut().find(|v| v.view == raw.view) {
                v.mm_per_px = if reuse_teaching && declared.is_none() { updated_calibration_scale(v.mm_per_px, previous_calib, scale) } else { scale };
                v.mm_per_px
            } else { scale };
            if w.doc.shots[k].view == raw.view {
                w.doc.shots[k].mm_per_px = inherited_scale;
            }
            let image_id = format!("{}-{k}-v{}", round.round_id, raw.view);
            let path = image_file(&host, &w.doc.id, &image_id)?;
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            crate::image_file::save_pgm_new(&path, &image)?;
            let calibration_check = declared.as_ref().map(|(set_id, mm)| CalibrationCheck {
                verification_id: format!("sample-set-{}-{k}-v{}", round.round_id, raw.view), source_k: k, source_image_id: image_id.clone(),
                basis: CalibrationBasis::SampleSet { set_id: set_id.clone() }, passed: true,
                camera_tag: camera_tag.clone(), calib_tag: calib_tag.clone(), mm_per_px: Some(*mm),
            });
            frame.views.push(FrozenImage { id: image_id, view: raw.view, source: format!("capture:{}", round.round_id),
                captured_at: round.created_at, size: [raw.width, raw.height], camera: camera_id.clone(), camera_tag, calib_tag,
                geometry_tag: shot_tags(&projection, k)?.0, exposure_us: Some(captured.camera.imaging.exposure_us), gain_db: Some(captured.camera.imaging.gain_db), history_id: None });
            frame.view_states.push(ViewTeaching { view: raw.view, trial: None, saved: false, calibration_check });
        }
        frame.image = frame.views.iter().find(|i| i.view == w.doc.shots[k].view).cloned();
        if frame.image.is_none() { return Err("采集图缺少继承示教使用的图编号".into()); }
        w.frames.push(frame);
    }
    w.capture_id = Some(round.round_id);
    w.last_position = Some(Position { k: 0, view: w.doc.shots[0].view });
    w.samples.clear();
    w.overview = Overview { background: None, positions: (0..w.frames.len()).map(|k| [((k % 5) as f32 + 0.5) / 5.0, ((k / 5) as f32 + 0.5) / w.frames.len().div_ceil(5) as f32]).collect(), saved: true };
    w.changed(); host.save(&w)?; items.insert(id, w.clone()); view(&app, w)
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
    let Some(_gate) = wait_idle_config_gate(&cycle, "加入验证样本")? else { return Err("请等待当前工件和 PLC 事务结束后加入验证样本".into()) };
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
