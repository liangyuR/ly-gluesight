use super::*;
use crate::recipe::ShotViewSpec;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationCheck {
    pub verification_id: String,
    pub source_k: usize,
    pub source_image_id: String,
    pub reference_mm: f64,
    pub measured_mm: f64,
    pub error_mm: f64,
    pub passed: bool,
    pub camera_tag: Value,
    pub calib_tag: Value,
    pub mm_per_px: Option<f32>,
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
}

pub(super) fn store_active(w: &mut Workspace, k: usize) {
    let Some(shot) = w.doc.shots.get_mut(k) else { return };
    if let Some(v) = shot.views.iter_mut().find(|v| v.view == shot.view) {
        v.path = shot.path.clone(); v.mm_per_px = shot.mm_per_px;
        v.detect = shot.detect.clone(); v.limits = shot.limits.clone(); v.calib = shot.calib.clone();
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
            limits: if active { shot.limits.clone() } else { None }, calib: if active { shot.calib.clone() } else { None } });
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

pub(super) fn check_view(app: &AppHandle, w: &Workspace, k: usize, selected: u8, calibration: bool) -> Result<(), String> {
    let r = w.doc.build()?.for_view(k, selected)?;
    let frame = projected_frame(w, k, selected)?;
    if !frame.saved { return Err(format!("拍照点 {} 图 {selected} 尚未完成示教", k + 1)); }
    let (image, params) = shot_tags(&r, k)?;
    frame.checked(&image, &params)?;
    let settings = app.state::<CycleHost>().settings();
    let engine = app.state::<vision::VisionHost>().engine(settings.lyflow_core.as_deref()).ok_or("图像核心库未加载")?;
    frame.checked_engine(&engine_state(&engine))?;
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

pub(super) fn refresh_views(app: &AppHandle, w: &mut Workspace) -> Result<bool, String> {
    let recipe = w.doc.build()?;
    let settings = app.state::<CycleHost>().settings();
    let engine = app.state::<vision::VisionHost>().engine(settings.lyflow_core.as_deref()).map(|e| engine_state(&e));
    let mut changed = false;
    for k in 0..w.frames.len() {
        let selected = w.doc.shots[k].view;
        for state in &mut w.frames[k].view_states {
            let current = recipe.for_view(k, state.view).and_then(|r| tags(app, &r, k).map(|tags| (r, tags)));
            let stale = current.as_ref().map_or(true, |(r, (camera, calib))| {
                state.trial.as_ref().is_some_and(|t| shot_tags(r, k).map_or(true, |p| p.1 != t.params_tag)
                    || engine.as_ref().is_none_or(|e| t.engine_tag != *e))
                    || state.calibration_check.as_ref().is_some_and(|c| c.camera_tag != *camera || c.calib_tag != *calib || c.mm_per_px != r.shots[k].mm_per_px)
            });
            if stale && (state.saved || state.trial.is_some() || state.calibration_check.as_ref().is_some_and(|c| c.passed)) {
                state.saved = false; state.trial = None;
                if let Some(check) = &mut state.calibration_check { check.passed = false; }
                changed = true;
            }
        }
        let frame = &mut w.frames[k];
        for image in &mut frame.views {
            let mut projected = recipe.clone();
            projected.shots[k].view = image.view;
            projected.shots[k].calib = w.doc.shots[k].views.iter().find(|v| v.view == image.view).and_then(|v| v.calib.clone());
            if let Ok((camera, calib)) = tags(app, &projected, k) {
                if image.camera_tag != camera {
                    if let Some(state) = frame.view_states.iter_mut().find(|s| s.view == image.view) {
                        if state.saved || state.trial.is_some() { state.saved = false; state.trial = None; changed = true; }
                    }
                }
                if image.camera_tag == camera && image.calib_tag != calib {
                    let current_scale = vision::calib_info(&vision::shot_calib_path(app, &projected, k)?).and_then(|c| c.mm_per_px).map(|v| v as f32);
                    if let Some(spec) = w.doc.shots[k].views.iter_mut().find(|v| v.view == image.view) {
                        spec.mm_per_px = updated_calibration_scale(spec.mm_per_px, &image.calib_tag, current_scale);
                    }
                    image.calib_tag = calib;
                    if let Some(state) = frame.view_states.iter_mut().find(|s| s.view == image.view) { state.trial = None; state.saved = false; }
                    changed = true;
                }
            }
        }
        if !w.doc.shots[k].views.is_empty() { load_active(w, k, selected)?; }
    }
    if changed { w.changed(); }
    Ok(changed)
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
    let shot = w.doc.shots.get_mut(k).ok_or("拍照点不存在")?;
    if shot.path != params.path || shot.mm_per_px != params.mm_per_px || shot.detect != params.detect || shot.limits != params.limits {
        shot.path = params.path; shot.mm_per_px = params.mm_per_px; shot.detect = params.detect; shot.limits = params.limits;
        w.frames[k].saved = false; w.frames[k].trial = None;
        store_active(&mut w, k);
        w.doc.build()?;
        w.changed();
    }
    host.save(&w)?; items.insert(id, w.clone()); view(&app, w)
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
        reference_mm, measured_mm, error_mm, passed: error_mm.abs() <= 0.1 + 1e-9,
        camera_tag, calib_tag, mm_per_px: r.shots[k].mm_per_px,
    };
    w.frames[k].view_states.iter_mut().find(|s| s.view == selected).unwrap().calibration_check = Some(check.clone());
    if reuse_matching == Some(true) && check.passed {
        let recipe = w.doc.build()?;
        for other in 0..w.frames.len() {
            if other == k || recipe.shots[other].camera != recipe.shots[k].camera { continue; }
            let Ok(projected) = recipe.for_view(other, selected) else { continue };
            let Ok((camera, calib)) = tags(&app, &projected, other) else { continue };
            let frame = &mut w.frames[other];
            if !frame.views.iter().any(|image| image.view == selected && reusable_calibration(&check, &camera, &calib, projected.shots[other].mm_per_px, image)) { continue; }
            if let Some(state) = frame.view_states.iter_mut().find(|state| state.view == selected) { state.calibration_check = Some(check.clone()); }
        }
    }
    w.changed(); host.save(&w)?; items.insert(id, w.clone()); view(&app, w)
}

#[derive(Serialize)]
pub struct CenterlineResult { pub path: Vec<[f32; 2]>, pub message: String }

#[tauri::command]
pub async fn workspace_extract_centerline(app: AppHandle, id: String, revision: u64, k: usize, image_id: String, roi: [u32; 4]) -> Result<CenterlineResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let (path, polarity) = {
            let items = host.items.lock().unwrap(); let w = items.get(&id).ok_or("候选不存在")?; w.expect(revision)?;
            let frame = w.frames.get(k).ok_or("拍照点不存在")?;
            if frame.image.as_ref().is_none_or(|i| i.id != image_id) { return Err("图像已变化，请重新框选".into()); }
            let r = w.doc.build()?.for_view(k, w.doc.shots[k].view)?;
            (image_file(&host, &id, &image_id)?, r.shot_detect(k).polarity)
        };
        let image = crate::replay::load(&path)?;
        let points = super::centerline::extract(&image, roi, polarity)?;
        let items = host.items.lock().unwrap(); let current = items.get(&id).ok_or("候选不存在")?; current.expect(revision)?;
        if current.frames[k].image.as_ref().is_none_or(|i| i.id != image_id) { return Err("提取期间图像已变化".into()); }
        Ok(CenterlineResult { path: points, message: "中线已提取，请检查并人工修正后试测".into() })
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

pub(super) fn require_capture_plan(version: Option<u32>) -> Result<u32, String> {
    version.filter(|version| *version > 0).ok_or_else(|| "采集轮次缺少有效的 PLC 轨迹计划版本，请重新采集".into())
}

pub(super) fn capture_for(app: &AppHandle, id: &str, round_id: &str) -> Result<crate::recipe_capture::CaptureRound, String> {
    let round = app.state::<CycleHost>().camera.recipe_capture().get(round_id)?;
    if round.recipe_id != id || round.state != crate::recipe_capture::CaptureState::Complete || round.frames.is_empty()
        || round.received_count != round.planned_count || round.frames.len() != round.planned_count as usize {
        return Err("采集轮次不属于当前配方或尚未完整完成，不能采用".into());
    }
    require_capture_plan(round.plc_plan_version)?;
    let config = app.state::<CycleHost>().camera.configs().into_iter().find(|c| c.id == round.camera_id).ok_or("采集设备不存在")?;
    if state_value(&config) != round.camera_config { return Err("采集后设备参数或拼接布局已改变，请重新采集".into()); }
    Ok(round)
}

#[tauri::command]
pub fn workspace_adopt_capture(app: AppHandle, id: String, revision: u64, round_id: String, reuse_teaching: bool, correspondence_confirmed: bool) -> Result<WorkspaceView, String> {
    let cycle = app.state::<CycleHost>();
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "设备配置正在变化，请稍后重试")?;
    if cycle.busy() { return Err("请等待当前工件和 PLC 事务结束后采用采集图像".into()); }
    let round = capture_for(&app, &id, &round_id)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone(); w.expect(revision)?;
    if w.capture_id.as_deref() == Some(&round_id) { return view(&app, w); }
    if reuse_teaching && (!correspondence_confirmed || w.doc.shots.len() != round.frames.len()) {
        return Err("继承示教前必须确认点数、轨迹及触发顺序对应一致".into());
    }
    let camera = app.state::<CycleHost>().camera.configs().into_iter().find(|c| c.id == round.camera_id).ok_or("采集设备不存在")?;
    let old = w.clone();
    w.doc.shots.clear(); w.frames.clear();
    for (k, captured) in round.frames.iter().enumerate() {
        let id = format!("S{:03}", k + 1);
        let mut shot = if reuse_teaching { old.doc.shots[k].clone() } else {
            crate::recipe::ShotSpec { id: id.clone(), pose_id: id, camera: round.camera_id.clone(), view: 1, calib: None,
                bead: "J1".into(), skip: false, path: Vec::new(), mm_per_px: None, detect: None, limits: None, views: Vec::new() }
        };
        if reuse_teaching && shot.camera != round.camera_id { return Err("采集设备已改变，不能按原对应关系继承示教".into()); }
        shot.camera = round.camera_id.clone();
        if !reuse_teaching {
            shot.views = captured.views.iter().map(|v| ShotViewSpec { view: v.view, enabled: false, path: Vec::new(), mm_per_px: None,
                detect: None, limits: None, calib: None }).collect();
        }
        let mut frame = Teaching::empty(k);
        w.doc.shots.push(shot);
        for raw in &captured.views {
            let image = crate::replay::load(Path::new(&raw.path))?;
            if [image.width, image.height] != [raw.width, raw.height] { return Err("采集图尺寸与记录不一致".into()); }
            let recipe = w.doc.build()?;
            let mut projection = recipe.clone();
            projection.shots[k].view = raw.view;
            projection.shots[k].calib = w.doc.shots[k].views.iter().find(|v| v.view == raw.view).and_then(|v| v.calib.clone());
            let (camera_tag, calib_tag) = tags(&app, &projection, k)?;
            let scale = vision::calib_info(&vision::shot_calib_path(&app, &projection, k)?).and_then(|c| c.mm_per_px).map(|scale| scale as f32);
            let previous_calib = old.frames.get(k).and_then(|f| f.views.iter().find(|i| i.view == raw.view)).map(|i| &i.calib_tag).unwrap_or(&Value::Null);
            let inherited_scale = if let Some(v) = w.doc.shots[k].views.iter_mut().find(|v| v.view == raw.view) {
                v.mm_per_px = if reuse_teaching { updated_calibration_scale(v.mm_per_px, previous_calib, scale) } else { scale };
                v.mm_per_px
            } else { scale };
            if w.doc.shots[k].view == raw.view {
                w.doc.shots[k].mm_per_px = inherited_scale;
            }
            let image_id = format!("{}-{k}-v{}", round.round_id, raw.view);
            let path = image_file(&host, &w.doc.id, &image_id)?;
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            crate::replay::save_pgm(&path, &image)?;
            frame.views.push(FrozenImage { id: image_id, view: raw.view, source: format!("capture:{}", round.round_id),
                captured_at: round.created_at, size: [raw.width, raw.height], camera: round.camera_id.clone(), camera_tag, calib_tag,
                geometry_tag: shot_tags(&projection, k)?.0, exposure_us: Some(camera.exposure_us), gain_db: Some(camera.gain_db), history_id: None });
            frame.view_states.push(ViewTeaching { view: raw.view, trial: None, saved: false, calibration_check: None });
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
pub fn workspace_capture_sample(app: AppHandle, id: String, revision: u64, round_id: String, expected: Verdict, correspondence_confirmed: bool) -> Result<WorkspaceView, String> {
    if !correspondence_confirmed { return Err("请确认验证采集与示教的轨迹、触发顺序对应一致".into()); }
    let cycle = app.state::<CycleHost>();
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "设备配置正在变化，请稍后重试")?;
    if cycle.busy() { return Err("请等待当前工件和 PLC 事务结束后加入验证样本".into()); }
    let round = capture_for(&app, &id, &round_id)?;
    let host = app.state::<WorkspaceHost>(); let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone(); w.expect(revision)?;
    if w.capture_id.as_deref() == Some(&round_id) { return Err("示教采集不能同时作为独立验证样本，请重新采集一圈".into()); }
    let teaching_round = capture_for(&app, &id, w.capture_id.as_deref().ok_or("尚未采用示教采集")?)?;
    if teaching_round.plc_plan_version != round.plc_plan_version { return Err("验证采集与示教的 PLC 轨迹计划版本不同".into()); }
    if w.doc.shots.len() != round.frames.len() || w.doc.shots.iter().any(|s| s.camera != round.camera_id) { return Err("验证采集的点数或设备与候选不同".into()); }
    if w.sample_bank.iter().any(|b| b.id == round_id) { return view(&app, w); }
    if w.sample_bank.len() >= 100 { return Err("最多保留 100 组验证样本".into()); }
    let recipe = w.doc.build()?;
    let dir = host.dir(&id).join("samples").join(&round_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for (k, frame) in round.frames.iter().enumerate() {
        for raw in &frame.views {
            let image = crate::replay::load(Path::new(&raw.path))?;
            let frozen = w.frames[k].views.iter().find(|i| i.view == raw.view).ok_or("样本的图编号与示教不一致")?;
            if [image.width, image.height] != frozen.size { return Err("验证样本与示教图的尺寸不一致".into()); }
            crate::replay::save_pgm(&dir.join(format!("k{k}-v{}.pgm", raw.view)), &image)?;
        }
    }
    w.sample_bank.push(BankSample { id: round_id.clone(), name: format!("独立采集 {}", round.created_at), geometry_tag: images_tag(&recipe), expected, created_at: round.created_at, capture_id: Some(round_id.clone()), camera_state: round.camera_config });
    w.samples.push(Sample { history_id: None, sample_id: Some(round_id), expected });
    w.changed(); host.save(&w)?; items.insert(id, w.clone()); view(&app, w)
}
