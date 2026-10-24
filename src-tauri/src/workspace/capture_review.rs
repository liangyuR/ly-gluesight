use super::*;
use crate::recipe_capture::{CaptureRoundV2, CaptureState};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureReview {
    pub rounds: BTreeMap<String, RoundReview>,
    pub template: Option<CaptureTemplate>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewStatus { Draft, Confirmed, Stale }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExcludedGroup { pub seq: u64, pub reason: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoundReview {
    pub group_count: u64,
    pub cuts: Vec<u64>,
    pub status: ReviewStatus,
    pub excluded_groups: Vec<ExcludedGroup>,
    pub segment_signature: Value,
    pub capture_signature: Value,
    pub template_version: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureTemplate {
    pub round_id: String,
    pub group_count: u64,
    pub cuts: Vec<u64>,
    pub segment_signature: Value,
    pub capture_signature: Value,
    pub version: u64,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TemplateMode { Proportional, Original }

pub enum CaptureReviewEdit {
    Save(Vec<u64>),
    Confirm,
    SetTemplate,
    ApplyTemplate(TemplateMode),
    Exclude { start: u64, end: u64, excluded: bool, reason: String },
}

fn segment_signature(doc: &RecipeDoc) -> Value {
    json!([doc.product_code, doc.segments.iter().map(|s| (s.index, s.kind)).collect::<Vec<_>>(), doc.plan_cameras(), doc.cameras])
}

fn capture_signature(round: &CaptureRoundV2) -> Value {
    let mut cameras = round.cameras.iter().collect::<Vec<_>>();
    cameras.sort_by_key(|c| &c.camera_id);
    json!(cameras.iter().map(|c| (&c.camera_id, &c.camera_config, c.imaging)).collect::<Vec<_>>())
}

pub(super) fn refresh(w: &mut Workspace) {
    let signature = segment_signature(&w.doc);
    for review in w.capture_review.rounds.values_mut() {
        if review.segment_signature != signature { review.status = ReviewStatus::Stale; }
    }
    if w.capture_review.template.as_ref().is_some_and(|t| t.segment_signature != signature) {
        w.capture_review.template = None;
    }
}

pub(super) fn require_available(w: &Workspace, round: &str, seq: u64) -> Result<(), String> {
    if w.capture_review.rounds.get(round).is_some_and(|r| r.excluded_groups.iter().any(|g| g.seq == seq)) {
        return Err(format!("第 {seq} 帧组已屏蔽，请恢复后再用于示教或图像标定"));
    }
    Ok(())
}

pub fn measure_calibration_round(w: &Workspace, round: &CaptureRoundV2,
    mut measure: impl FnMut(usize, &str, u64) -> Result<crate::burst_judge::FrameMetrics, String>) -> Vec<crate::burst_judge::FrameMetrics> {
    burst::measure_round(&w.doc.plan_cameras(), round, |camera, camera_id, seq| {
        require_available(w, &round.round_id, seq)?;
        measure(camera, camera_id, seq)
    })
}

fn validate_cuts(cuts: &[u64], groups: u64, segments: usize) -> Result<(), String> {
    if segments == 0 || cuts.len() != segments - 1 || groups < segments as u64
        || cuts.iter().any(|&cut| cut == 0 || cut >= groups)
        || cuts.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(format!("{segments} 段需要 {} 个严格递增的内部边界，每段至少一个原始帧组", segments.saturating_sub(1)));
    }
    Ok(())
}

pub fn apply_capture_review(w: &mut Workspace, round: &CaptureRoundV2, edit: CaptureReviewEdit) -> Result<(), String> {
    if !w.doc.is_burst() || round.recipe_id != w.doc.id || round.state != CaptureState::Complete
        || round.group_count == 0 || round.group_count > crate::plan::MAX_EXPECTED_TOTAL as u64 {
        return Err("只能整理当前配方已完成且具有有效帧组的采集轮次".into());
    }
    let mut actual = round.camera_ids();
    actual.sort();
    let mut planned = w.doc.plan_cameras();
    planned.sort();
    if actual != planned { return Err("轮次相机与当前计划不一致".into()); }
    let signature = segment_signature(&w.doc);
    let capture = capture_signature(round);
    let mut review = w.capture_review.rounds.get(&round.round_id).cloned().unwrap_or_else(|| RoundReview {
        group_count: round.group_count, cuts: Vec::new(), status: ReviewStatus::Draft,
        excluded_groups: Vec::new(), segment_signature: signature.clone(), capture_signature: capture.clone(), template_version: None,
    });
    if review.group_count != round.group_count || review.capture_signature != capture {
        return Err("轮次原始记录已变化，请重新加载采集记录".into());
    }
    let mut invalidate_calibration = false;
    let mut invalidate_template = false;
    match edit {
        CaptureReviewEdit::Save(cuts) => {
            validate_cuts(&cuts, round.group_count, w.doc.segments.len())?;
            invalidate_calibration = review.cuts != cuts || review.segment_signature != signature;
            invalidate_template = true;
            review.cuts = cuts;
            review.segment_signature = signature;
            review.template_version = None;
            review.status = ReviewStatus::Draft;
        }
        CaptureReviewEdit::Confirm => {
            if review.segment_signature != signature || review.status == ReviewStatus::Stale {
                return Err("段列表或采集参数已变化，请重新保存边界后复核".into());
            }
            validate_cuts(&review.cuts, round.group_count, w.doc.segments.len())?;
            review.status = ReviewStatus::Confirmed;
        }
        CaptureReviewEdit::SetTemplate => {
            if review.status != ReviewStatus::Confirmed || review.segment_signature != signature {
                return Err("只有已确认且与当前段列表一致的轮次可以设为模板".into());
            }
            validate_cuts(&review.cuts, round.group_count, w.doc.segments.len())?;
            if round.cameras.iter().any(|c| w.doc.cameras.get(&c.camera_id) != Some(&c.imaging)) {
                return Err("模板轮次的成像参数与当前配方不一致，请重新采集".into());
            }
            w.capture_review.template = Some(CaptureTemplate { round_id: round.round_id.clone(), group_count: round.group_count,
                cuts: review.cuts.clone(), segment_signature: signature, capture_signature: capture, version: w.revision + 1 });
        }
        CaptureReviewEdit::ApplyTemplate(mode) => {
            let template = w.capture_review.template.as_ref().ok_or("请先将一轮已确认分段设为模板")?;
            if template.segment_signature != signature || template.capture_signature != capture {
                return Err("模板的段列表或相机采集配置与目标轮次不一致，不能套用".into());
            }
            let cuts: Vec<u64> = template.cuts.iter().map(|&cut| match mode {
                TemplateMode::Original => cut,
                TemplateMode::Proportional => (cut * round.group_count + template.group_count / 2) / template.group_count,
            }).collect();
            validate_cuts(&cuts, round.group_count, w.doc.segments.len())?;
            invalidate_calibration = review.cuts != cuts || review.segment_signature != signature;
            invalidate_template = template.round_id == round.round_id;
            review.cuts = cuts;
            review.segment_signature = signature;
            review.template_version = Some(template.version);
            review.status = ReviewStatus::Draft;
        }
        CaptureReviewEdit::Exclude { start, end, excluded, reason } => {
            if start == 0 || start > end || end > round.group_count { return Err("屏蔽范围必须在原始帧组范围内".into()); }
            let reason = reason.trim();
            if excluded && (reason.is_empty() || reason.chars().count() > 200) { return Err("请输入 1 至 200 字的屏蔽原因".into()); }
            if excluded && w.camera_teaching.iter().any(|t| t.round == round.round_id && (start..=end).contains(&t.seq)) {
                return Err("所选范围包含当前走廊示教帧，请先更换示教帧再屏蔽".into());
            }
            let before = review.excluded_groups.clone();
            review.excluded_groups.retain(|g| !(start..=end).contains(&g.seq));
            if excluded { review.excluded_groups.extend((start..=end).map(|seq| ExcludedGroup { seq, reason: reason.into() })); }
            review.excluded_groups.sort_by_key(|g| g.seq);
            invalidate_calibration = before != review.excluded_groups;
            invalidate_template = invalidate_calibration;
            if invalidate_calibration && review.status == ReviewStatus::Confirmed { review.status = ReviewStatus::Draft; }
        }
    }
    if invalidate_template && w.capture_review.template.as_ref().is_some_and(|t| t.round_id == round.round_id) {
        w.capture_review.template = None;
    }
    if invalidate_calibration && w.calibration.rounds.iter().any(|r| r.round_id == round.round_id) {
        w.calibration.rounds.retain(|r| r.round_id != round.round_id);
        burst::clear_calibration(&mut w.doc);
    }
    w.capture_review.rounds.insert(round.round_id.clone(), review);
    w.changed();
    Ok(())
}

fn edit_round(app: AppHandle, id: String, revision: u64, round: String, edit: CaptureReviewEdit) -> Result<WorkspaceView, String> {
    let cycle = app.state::<CycleHost>();
    let _gate = cycle.plc_gate.try_lock().map_err(|_| "正在处理采集或设备事务，请稍后整理轮次")?;
    crate::camera::check_idle(&cycle)?;
    let captured = cycle.camera.recipe_capture().get_burst(&round)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let current = items.get_mut(&id).ok_or("候选配置不存在")?;
    current.expect(revision)?;
    let mut next = current.clone();
    apply_capture_review(&mut next, &captured, edit)?;
    host.save(&next)?;
    *current = next;
    view(&app, current.clone())
}

#[tauri::command]
pub fn workspace_capture_review_save(app: AppHandle, id: String, revision: u64, round: String, cuts: Vec<u64>) -> Result<WorkspaceView, String> {
    edit_round(app, id, revision, round, CaptureReviewEdit::Save(cuts))
}

#[tauri::command]
pub fn workspace_capture_review_confirm(app: AppHandle, id: String, revision: u64, round: String) -> Result<WorkspaceView, String> {
    edit_round(app, id, revision, round, CaptureReviewEdit::Confirm)
}

#[tauri::command]
pub fn workspace_capture_review_set_template(app: AppHandle, id: String, revision: u64, round: String) -> Result<WorkspaceView, String> {
    edit_round(app, id, revision, round, CaptureReviewEdit::SetTemplate)
}

#[tauri::command]
pub fn workspace_capture_review_apply_template(app: AppHandle, id: String, revision: u64, round: String, mode: TemplateMode) -> Result<WorkspaceView, String> {
    edit_round(app, id, revision, round, CaptureReviewEdit::ApplyTemplate(mode))
}

#[tauri::command]
pub fn workspace_capture_review_exclude(app: AppHandle, id: String, revision: u64, round: String, start: u64, end: u64, excluded: bool, reason: String) -> Result<WorkspaceView, String> {
    edit_round(app, id, revision, round, CaptureReviewEdit::Exclude { start, end, excluded, reason })
}

#[cfg(test)]
mod tests;
