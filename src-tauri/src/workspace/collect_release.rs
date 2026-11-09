//! 采集版发布（在线投运）：新产线还没有可用配方时，先把未完成的候选发布成采集版，维护模式按它的型号与三台计划相机
//! 跟着生产采图（曝光增益取相机页的设定）；攒够历史件再示教、标定、验证，发布正式版（`workspace_publish`）。
//!
//! 采集版只检查计划相机、段与生产版本，不要求走廊示教、ΣN_exp 标定与独立验证，也不生成发布包；相机组与型号在
//! 接件和写入配方库时照常核对。版本号照常递增，同样在工件边界生效。已有正式版时不能发布采集版
//! （正式版不降级，`RecipeStore::save_published` 同样拒绝）。

use super::*;
use crate::plan::PLAN_CAMERAS;
use crate::recipe::{valid_camera_id, RecipeStage, COLLECT_AFTER_FORMAL};

/// 采集版的全部检查项，按页面展示顺序；`production` 是同编号的当前生产配方。
pub(super) fn collect_checks(doc: &RecipeDoc, production: Option<&Recipe>) -> Vec<Check> {
    let cameras = doc.plan_cameras();
    let distinct = cameras.len() == PLAN_CAMERAS
        && cameras
            .iter()
            .enumerate()
            .all(|(i, c)| valid_camera_id(c) && !cameras[..i].contains(c));
    let mut checks = vec![Check {
        advisory: false,
        name: "计划相机".into(),
        passed: distinct,
        detail: if distinct {
            format!("{} 共用一个触发", cameras.join("、"))
        } else {
            format!(
                "需要恰好 {PLAN_CAMERAS} 台互不相同的计划相机，当前 {} 台：{}",
                cameras.len(),
                cameras.join("、")
            )
        },
    }];
    checks.push(Check {
        advisory: false,
        name: "段".into(),
        passed: !doc.segments.is_empty(),
        detail: if doc.segments.is_empty() {
            "至少需要一段，可先放一个直线段占位".into()
        } else {
            format!("{} 段；采集版不要求示教与标定", doc.segments.len())
        },
    });
    let formal = production.filter(|r| !r.is_collect());
    checks.push(Check {
        advisory: false,
        name: "生产版本".into(),
        passed: formal.is_none(),
        detail: match (formal, production) {
            (Some(r), _) => format!("已有正式版 v{}。{COLLECT_AFTER_FORMAL}", r.version),
            (None, Some(r)) => format!("替换当前采集版 v{}", r.version),
            (None, None) => "尚无生产版本".into(),
        },
    });
    checks
}

fn current_checks(app: &AppHandle, w: &Workspace) -> Vec<Check> {
    let production = app.state::<CycleHost>().recipe(&w.doc.id);
    collect_checks(&w.doc, production.as_deref())
}

/// 采集版检查结果（发布确认框展示，不改候选）。
#[tauri::command]
pub fn workspace_collect_checks(
    app: AppHandle,
    id: String,
    revision: u64,
) -> Result<Vec<Check>, String> {
    let host = app.state::<WorkspaceHost>();
    let items = host.items.lock().unwrap();
    let w = items.get(&id).ok_or("候选配置不存在")?;
    if w.revision != revision {
        return Err("候选配置已更新，请重新加载后操作".into());
    }
    Ok(current_checks(&app, w))
}

/// 把候选冻结成待生效的采集版：检查全部通过才发布，在工件边界生效（`apply_pending`）。
#[tauri::command]
pub fn workspace_publish_collect(
    app: AppHandle,
    id: String,
    revision: u64,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let w = items.get_mut(&id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    let checks = current_checks(&app, w);
    if let Some(failed) = checks.iter().find(|c| !c.passed) {
        return Err(format!(
            "不能发布采集版：{}：{}",
            failed.name, failed.detail
        ));
    }
    let recipes = &app.state::<CycleHost>().recipes;
    let mut doc = w.doc.clone();
    doc.version = recipes.release_version(&w.doc)?;
    doc.stage = RecipeStage::Collect;
    doc.teaching_id = None;
    doc.build()?;
    // 段 ID 留在候选里（逐笔对照的沿用状态按它记），只从发布的配方去掉（RB-11）
    let mut released = doc.clone();
    for segment in &mut released.segments {
        segment.id = None;
    }
    let mut next = w.clone();
    next.doc = doc;
    next.pending = Some(Box::new(Release {
        doc: released,
        bundle_id: String::new(),
        navigation: None,
        revision,
        _frames: Discarded,
        _overview: Discarded,
        _base_revision: Discarded,
        validation: Validation {
            revision,
            passed: true,
            checked_at: ly_plc::now_ms(),
            checks,
            samples: Vec::new(),
            environment_tag: Value::Null,
        },
        validation_site: ValidationSite::Real,
    }));
    next.publish = PublishState::Waiting;
    host.save(&next)?;
    *w = next;
    let _ = app.state::<CycleHost>().tx.send(cycle::Input::Refresh);
    view(&app, w.clone())
}
