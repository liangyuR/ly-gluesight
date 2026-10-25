//! 采集版发布（在线投运）：新产线还没有可用配方时，先把未完成的候选发布成采集版，维护模式按它的型号、三台计划相机与
//! 曝光增益跟着生产采图；攒够历史件再示教、标定、验证，发布正式版（`workspace_publish`）。
//!
//! 采集版只检查相机与成像、型号、段与生产版本，不要求走廊示教、ΣN_exp 标定与独立验证，也不生成发布包；版本号照常递增，
//! 同样在工件边界生效。已有正式版时不能发布采集版（正式版不降级，`RecipeStore::save_published` 同样拒绝）。

use super::*;
use crate::camera::CameraConfig;
use crate::plan::PLAN_CAMERAS;
use crate::recipe::{valid_camera_id, RecipeStage, COLLECT_AFTER_FORMAL};

/// 采集版检查项的输入：设备与配方库由调用方读出，其余只看候选。
pub(super) struct CollectInputs<'a> {
    /// 当前相机组
    pub rig: &'a [CameraConfig],
    /// 同编号的当前生产配方
    pub production: Option<&'a Recipe>,
    /// 用了同一型号的其他生产配方
    pub conflict: Option<String>,
    /// 候选基于当前生产版本
    pub base_matches: bool,
}

/// 采集版的全部检查项，按页面展示顺序。
pub(super) fn collect_checks(doc: &RecipeDoc, inputs: CollectInputs) -> Vec<Check> {
    let cameras = doc.plan_cameras();
    let distinct = cameras.len() == PLAN_CAMERAS && cameras.iter().enumerate().all(|(i, c)| valid_camera_id(c) && !cameras[..i].contains(c));
    let mut checks = vec![Check {
        name: "计划相机".into(),
        passed: distinct,
        detail: if distinct { format!("{} 共用一个触发", cameras.join("、")) }
            else { format!("需要恰好 {PLAN_CAMERAS} 台互不相同的计划相机，当前 {} 台：{}", cameras.len(), cameras.join("、")) },
    }];
    let rig: Vec<String> = cameras.iter().filter_map(|camera| match inputs.rig.iter().find(|c| &c.id == camera) {
        None => Some(format!("相机组里没有 {camera}")),
        Some(config) => config.require_single_view().err(),
    }).collect();
    checks.push(Check {
        name: "相机组".into(),
        passed: !cameras.is_empty() && rig.is_empty(),
        detail: if rig.is_empty() { "计划相机都在当前相机组里，且是单视角独立相机".into() } else { rig.join("；") },
    });
    let imaging: Vec<Result<String, String>> = cameras.iter().map(|camera| {
        let imaging = doc.cameras.get(camera).ok_or_else(|| format!("相机 {camera} 缺少曝光与增益"))?;
        imaging.validate(camera)?;
        Ok(format!("{camera} {} µs / {} dB", imaging.exposure_us, imaging.gain_db))
    }).collect();
    let missing: Vec<&String> = imaging.iter().filter_map(|r| r.as_ref().err()).collect();
    checks.push(Check {
        name: "曝光与增益".into(),
        passed: !cameras.is_empty() && missing.is_empty(),
        detail: if missing.is_empty() { imaging.iter().flatten().cloned().collect::<Vec<_>>().join("；") }
            else { missing.into_iter().cloned().collect::<Vec<_>>().join("；") },
    });
    checks.push(Check {
        name: "PLC 型号".into(),
        passed: inputs.conflict.is_none(),
        detail: match &inputs.conflict {
            None => format!("PLC 下发型号 {}", doc.product_code),
            Some(other) => format!("型号 {} 已被配方 {other} 使用", doc.product_code),
        },
    });
    checks.push(Check {
        name: "段".into(),
        passed: !doc.segments.is_empty(),
        detail: if doc.segments.is_empty() { "至少需要一段，可先放一个直线段占位".into() }
            else { format!("{} 段；采集版不要求示教与标定", doc.segments.len()) },
    });
    let formal = inputs.production.filter(|r| !r.is_collect());
    checks.push(Check {
        name: "生产版本".into(),
        passed: formal.is_none() && inputs.base_matches,
        detail: match (formal, inputs.production) {
            (Some(r), _) => format!("已有正式版 v{}。{COLLECT_AFTER_FORMAL}", r.version),
            _ if !inputs.base_matches => "生产配方已被其他操作更新，请重新建立候选".into(),
            (None, Some(r)) => format!("替换当前采集版 v{}", r.version),
            (None, None) => "尚无生产版本".into(),
        },
    });
    checks
}

fn current_checks(app: &AppHandle, w: &Workspace) -> Vec<Check> {
    let cycle = app.state::<CycleHost>();
    let production = cycle.recipe(&w.doc.id);
    let conflict = cycle.recipes.list().iter()
        .find(|r| !r.id.eq_ignore_ascii_case(&w.doc.id) && r.product_code == w.doc.product_code)
        .map(|r| r.id.clone());
    let base = production.as_ref().map(|r| r.revision_id.clone());
    collect_checks(&w.doc, CollectInputs { rig: &cycle.camera.configs(), production: production.as_deref(), conflict, base_matches: base == w.base_revision })
}

/// 采集版检查结果（发布确认框展示，不改候选）。
#[tauri::command]
pub fn workspace_collect_checks(app: AppHandle, id: String, revision: u64) -> Result<Vec<Check>, String> {
    let host = app.state::<WorkspaceHost>();
    let items = host.items.lock().unwrap();
    let w = items.get(&id).ok_or("候选配置不存在")?;
    if w.revision != revision { return Err("候选配置已更新，请重新加载后操作".into()); }
    Ok(current_checks(&app, w))
}

/// 把候选冻结成待生效的采集版：检查全部通过才发布，计划版本照常由配方库分配，在工件边界生效（`apply_pending`）。
#[tauri::command]
pub fn workspace_publish_collect(app: AppHandle, id: String, revision: u64) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let w = items.get_mut(&id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    let checks = current_checks(&app, w);
    if let Some(failed) = checks.iter().find(|c| !c.passed) {
        return Err(format!("不能发布采集版：{}：{}", failed.name, failed.detail));
    }
    let mut doc = w.doc.clone();
    doc.stage = RecipeStage::Collect;
    doc.teaching_id = None;
    doc.plan_version = app.state::<CycleHost>().recipes.plan_version_for(&doc)?;
    doc.build()?;
    let mut next = w.clone();
    next.doc = doc.clone();
    next.pending = Some(Box::new(Release {
        doc,
        bundle_id: String::new(),
        base_revision: w.base_revision.clone(),
        revision,
        frames: w.frames.clone(),
        overview: w.overview.clone(),
        validation: Validation { revision, passed: true, checked_at: ly_plc::now_ms(), checks, samples: Vec::new(), environment_tag: Value::Null, fixture: Vec::new() },
        validation_site: if crate::offline::enabled(&app) { ValidationSite::Offline } else { ValidationSite::Real },
    }));
    next.publish = PublishState::Waiting;
    host.save(&next)?;
    *w = next;
    let _ = app.state::<CycleHost>().tx.send(cycle::Input::Refresh);
    view(&app, w.clone())
}
