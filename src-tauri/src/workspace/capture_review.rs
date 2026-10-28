use super::*;
use crate::plan::PathSegment;
use crate::recipe_capture::{CaptureRoundV2, CaptureState};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureReview {
    pub rounds: BTreeMap<String, RoundReview>,
    pub template: Option<CaptureTemplate>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewStatus {
    Draft,
    Confirmed,
    Stale,
}

/// 边界由谁确认（D-SB-51）：人工逐个确认，或对齐可信时由软件自动确认。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfirmedBy {
    #[default]
    Manual,
    Aligned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExcludedGroup {
    pub seq: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoundReview {
    pub group_count: u64,
    pub cuts: Vec<u64>,
    pub status: ReviewStatus,
    pub excluded_groups: Vec<ExcludedGroup>,
    #[serde(deserialize_with = "segment_signature_without_imaging")]
    pub segment_signature: Value,
    #[serde(deserialize_with = "capture_signature_without_imaging")]
    pub capture_signature: Value,
    pub template_version: Option<u64>,
    /// 最近一次确认的来源；读旧数据为人工
    #[serde(default)]
    pub confirmed_by: ConfirmedBy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureTemplate {
    pub round_id: String,
    pub group_count: u64,
    pub cuts: Vec<u64>,
    #[serde(deserialize_with = "segment_signature_without_imaging")]
    pub segment_signature: Value,
    #[serde(deserialize_with = "capture_signature_without_imaging")]
    pub capture_signature: Value,
    pub version: u64,
}

pub enum CaptureReviewEdit {
    Save(Vec<u64>),
    Confirm,
    SetTemplate,
    Exclude {
        start: u64,
        end: u64,
        excluded: bool,
        reason: String,
    },
}

fn segment_signature(doc: &RecipeDoc) -> Value {
    json!([
        doc.product_code,
        doc.segments
            .iter()
            .map(|s| (s.index, s.kind))
            .collect::<Vec<_>>(),
        doc.plan_cameras()
    ])
}

fn capture_signature(round: &CaptureRoundV2) -> Value {
    let mut cameras = round.cameras.iter().collect::<Vec<_>>();
    cameras.sort_by_key(|c| &c.camera_id);
    json!(cameras
        .iter()
        .map(|c| (&c.camera_id, &c.camera_config))
        .collect::<Vec<_>>())
}

/// 曝光增益只记录、不比较：旧签名末尾带配方各相机的曝光增益，读入时去掉。
fn segment_signature_without_imaging<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Value, D::Error> {
    let mut value = Value::deserialize(d)?;
    if let Some(items) = value.as_array_mut().filter(|items| items.len() == 4) {
        items.truncate(3);
    }
    Ok(value)
}

/// 旧签名每台相机是（编号, 设备配置, 曝光增益），读入时去掉曝光增益。
fn capture_signature_without_imaging<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Value, D::Error> {
    let mut value = Value::deserialize(d)?;
    for camera in value.as_array_mut().into_iter().flatten() {
        if let Some(fields) = camera.as_array_mut().filter(|fields| fields.len() == 3) {
            fields.truncate(2);
        }
    }
    Ok(value)
}

pub(super) fn refresh(w: &mut Workspace) {
    let signature = segment_signature(&w.doc);
    for review in w.capture_review.rounds.values_mut() {
        if review.segment_signature != signature {
            review.status = ReviewStatus::Stale;
        }
    }
    if w.capture_review
        .template
        .as_ref()
        .is_some_and(|t| t.segment_signature != signature)
    {
        w.capture_review.template = None;
    }
}

/// 本轮已确认、与当前段列表及帧组数一致的手动分段，按段顺序给出各段帧组数；未确认或已失效为 None。
pub(super) fn confirmed_segment_counts(
    w_doc: &RecipeDoc,
    reviews: &BTreeMap<String, RoundReview>,
    round: &str,
    group_count: u64,
) -> Option<Vec<u32>> {
    let review = reviews.get(round)?;
    if review.status != ReviewStatus::Confirmed
        || review.group_count != group_count
        || review.segment_signature != segment_signature(w_doc)
        || validate_cuts(&review.cuts, group_count, w_doc.segments.len()).is_err()
    {
        return None;
    }
    let bounds: Vec<u64> = std::iter::once(0)
        .chain(review.cuts.iter().copied())
        .chain(std::iter::once(group_count))
        .collect();
    bounds
        .windows(2)
        .map(|pair| u32::try_from(pair[1] - pair[0]).ok())
        .collect()
}

pub(super) fn require_available(w: &Workspace, round: &str, seq: u64) -> Result<(), String> {
    if w.capture_review
        .rounds
        .get(round)
        .is_some_and(|r| r.excluded_groups.iter().any(|g| g.seq == seq))
    {
        return Err(format!("第 {seq} 帧组已屏蔽，请恢复后再用于示教或图像标定"));
    }
    Ok(())
}

pub fn measure_calibration_round(
    w: &Workspace,
    round: &CaptureRoundV2,
    mut measure: impl FnMut(usize, &str, u64) -> Result<crate::burst_judge::FrameMetrics, String>,
) -> Vec<crate::burst_judge::FrameMetrics> {
    burst::measure_round(&w.doc.plan_cameras(), round, |camera, camera_id, seq| {
        require_available(w, &round.round_id, seq)?;
        measure(camera, camera_id, seq)
    })
}

fn validate_cuts(cuts: &[u64], groups: u64, segments: usize) -> Result<(), String> {
    if segments == 0
        || cuts.len() != segments - 1
        || groups < segments as u64
        || cuts.iter().any(|&cut| cut == 0 || cut >= groups)
        || cuts.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(format!(
            "{segments} 段需要 {} 个严格递增的内部边界，每段至少一个原始帧组",
            segments.saturating_sub(1)
        ));
    }
    Ok(())
}

pub fn apply_capture_review(
    w: &mut Workspace,
    round: &CaptureRoundV2,
    edit: CaptureReviewEdit,
) -> Result<(), String> {
    if !w.doc.is_burst()
        || round.recipe_id != w.doc.id
        || round.state != CaptureState::Complete
        || round.group_count == 0
        || round.group_count > crate::plan::MAX_EXPECTED_TOTAL as u64
    {
        return Err("只能整理当前配方已完成且具有有效帧组的采集轮次".into());
    }
    let mut actual = round.camera_ids();
    actual.sort();
    let mut planned = w.doc.plan_cameras();
    planned.sort();
    if actual != planned {
        return Err("轮次相机与当前计划不一致".into());
    }
    let signature = segment_signature(&w.doc);
    let capture = capture_signature(round);
    let mut review = w
        .capture_review
        .rounds
        .get(&round.round_id)
        .cloned()
        .unwrap_or_else(|| RoundReview {
            group_count: round.group_count,
            cuts: Vec::new(),
            status: ReviewStatus::Draft,
            excluded_groups: Vec::new(),
            segment_signature: signature.clone(),
            capture_signature: capture.clone(),
            template_version: None,
            confirmed_by: ConfirmedBy::Manual,
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
            review.confirmed_by = ConfirmedBy::Manual;
        }
        CaptureReviewEdit::SetTemplate => {
            if review.status != ReviewStatus::Confirmed || review.segment_signature != signature {
                return Err("只有已确认且与当前段列表一致的轮次可以设为模板".into());
            }
            validate_cuts(&review.cuts, round.group_count, w.doc.segments.len())?;
            w.capture_review.template = Some(CaptureTemplate {
                round_id: round.round_id.clone(),
                group_count: round.group_count,
                cuts: review.cuts.clone(),
                segment_signature: signature,
                capture_signature: capture,
                version: w.revision + 1,
            });
        }
        CaptureReviewEdit::Exclude {
            start,
            end,
            excluded,
            reason,
        } => {
            if start == 0 || start > end || end > round.group_count {
                return Err("屏蔽范围必须在原始帧组范围内".into());
            }
            let reason = reason.trim();
            if excluded && (reason.is_empty() || reason.chars().count() > 200) {
                return Err("请输入 1 至 200 字的屏蔽原因".into());
            }
            if excluded
                && w.camera_teaching
                    .iter()
                    .any(|t| t.round == round.round_id && (start..=end).contains(&t.seq))
            {
                return Err("所选范围包含当前走廊示教帧，请先更换示教帧再屏蔽".into());
            }
            let before = review.excluded_groups.clone();
            review
                .excluded_groups
                .retain(|g| !(start..=end).contains(&g.seq));
            if excluded {
                review
                    .excluded_groups
                    .extend((start..=end).map(|seq| ExcludedGroup {
                        seq,
                        reason: reason.into(),
                    }));
            }
            review.excluded_groups.sort_by_key(|g| g.seq);
            invalidate_calibration = before != review.excluded_groups;
            invalidate_template = invalidate_calibration;
            if invalidate_calibration && review.status == ReviewStatus::Confirmed {
                review.status = ReviewStatus::Draft;
            }
        }
    }
    if invalidate_template
        && w.capture_review
            .template
            .as_ref()
            .is_some_and(|t| t.round_id == round.round_id)
    {
        w.capture_review.template = None;
    }
    if invalidate_calibration
        && w.calibration
            .rounds
            .iter()
            .any(|r| r.round_id == round.round_id)
    {
        w.calibration
            .rounds
            .retain(|r| r.round_id != round.round_id);
        burst::clear_calibration(&mut w.doc);
    }
    w.capture_review
        .rounds
        .insert(round.round_id.clone(), review);
    w.changed();
    Ok(())
}

/// 分段标定工作台的一次保存（D-SB-44）：在候选副本上依次替换段列表（受 `guard_calibration` 约束）、保存并确认
/// 示教件边界、设为模板，全部成功才返回新候选。段的类型序列变化时其余件按 `refresh` 变“需复核”、标定清空。
pub fn save_template_round(
    w: &Workspace,
    round: &CaptureRoundV2,
    segments: Vec<PathSegment>,
    cuts: Vec<u64>,
) -> Result<Workspace, String> {
    if w.teaching_piece
        .as_ref()
        .is_none_or(|t| t.round_id != round.round_id)
    {
        return Err("分段只在示教件上确认".into());
    }
    let mut next = w.clone();
    burst::update_segments(&mut next, segments)?;
    apply_capture_review(&mut next, round, CaptureReviewEdit::Save(cuts))?;
    apply_capture_review(&mut next, round, CaptureReviewEdit::Confirm)?;
    apply_capture_review(&mut next, round, CaptureReviewEdit::SetTemplate)?;
    Ok(next)
}

/// 示教件的轮次：没有示教件时报错。
fn teaching_round(app: &AppHandle, id: &str, revision: u64) -> Result<String, String> {
    let host = app.state::<WorkspaceHost>();
    let items = host.items.lock().unwrap();
    let w = items.get(id).ok_or("候选配置不存在")?;
    w.expect(revision)?;
    Ok(w.teaching_piece
        .as_ref()
        .ok_or("还没有示教件：先选示教件")?
        .round_id
        .clone())
}

/// 分段标定工作台保存：段列表 + 确认示教件边界 + 设为模板（模板即示教件）。
#[tauri::command]
pub fn workspace_segment_template_save(
    app: AppHandle,
    id: String,
    revision: u64,
    segments: Vec<PathSegment>,
    cuts: Vec<u64>,
) -> Result<WorkspaceView, String> {
    let cycle = app.state::<CycleHost>();
    let _gate = cycle
        .config_gate
        .try_lock()
        .map_err(|_| "正在处理采集或设备事务，请稍后保存分段")?;
    crate::camera::check_idle(&cycle)?;
    let round = teaching_round(&app, &id, revision)?;
    let captured = cycle.camera.recipe_capture().get_burst(&round)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let current = items.get_mut(&id).ok_or("候选配置不存在")?;
    current.expect(revision)?;
    let next = save_template_round(current, &captured, segments, cuts)?;
    host.save(&next)?;
    *current = next;
    view(&app, current.clone())
}

/// 对齐复核的建议（只读，不写候选）：示教件、目标件与各边界的对齐结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignSuggestion {
    pub round: String,
    pub template_round: String,
    pub template_cuts: Vec<u64>,
    pub cuts: Vec<crate::frame_align::AlignedCut>,
}

fn align_sequence(
    capture: &crate::recipe_capture::RecipeCaptureHost,
    round: &str,
    camera: &str,
) -> Result<crate::frame_align::Sequence, String> {
    Ok(capture
        .burst_frame_paths(round, camera)?
        .iter()
        .map(|path| {
            path.as_ref()
                .and_then(|path| crate::image_file::load(path).ok())
                .map(|image| {
                    crate::frame_align::downsample(&image, crate::frame_align::ALIGN_FACTOR)
                })
        })
        .collect())
}

/// 对齐用的示教件：已确认的边界、相机采集签名、各计划相机的小图序列（批量对齐时只读一次）与可信容许帧数（配方的 boundary_guard）。
pub(super) struct AlignTemplate {
    round_id: String,
    cuts: Vec<u64>,
    capture_signature: Value,
    cameras: Vec<String>,
    sequences: Vec<crate::frame_align::Sequence>,
    tolerance: u64,
}

impl AlignTemplate {
    pub(super) fn load(
        capture: &crate::recipe_capture::RecipeCaptureHost,
        w: &Workspace,
    ) -> Result<Self, String> {
        let piece = w
            .teaching_piece
            .as_ref()
            .ok_or("还没有示教件：先选示教件")?;
        let review = w
            .capture_review
            .rounds
            .get(&piece.round_id)
            .filter(|_| super::pieces::teaching_counts(w).is_some())
            .ok_or("示教件边界未确认：先在分段步骤确认")?;
        let cameras = w.doc.plan_cameras();
        let sequences = cameras
            .iter()
            .map(|camera| align_sequence(capture, &piece.round_id, camera))
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            round_id: piece.round_id.clone(),
            cuts: review.cuts.clone(),
            capture_signature: review.capture_signature.clone(),
            cameras,
            sequences,
            tolerance: u64::from(w.doc.boundary_guard),
        })
    }

    /// 把示教件边界按图像对齐到 `round`：两件的相机采集配置必须一致。
    pub(super) fn align(
        &self,
        capture: &crate::recipe_capture::RecipeCaptureHost,
        round: &str,
    ) -> Result<Vec<crate::frame_align::AlignedCut>, String> {
        if self.round_id == round {
            return Err("这一件就是示教件".into());
        }
        let target = capture.get_burst(round)?;
        if capture_signature(&target) != self.capture_signature {
            return Err("示教件与这一件的相机采集配置不一致，不能对齐".into());
        }
        let sequences = self
            .cameras
            .iter()
            .map(|camera| align_sequence(capture, round, camera))
            .collect::<Result<Vec<_>, String>>()?;
        Ok(crate::frame_align::align_cuts(
            &self.sequences,
            &sequences,
            &self.cuts,
            self.tolerance,
        ))
    }
}

/// 把示教件边界按图像对齐到 `round`（D-SB-44 P2，可信判据见 D-SB-51）。
/// 读两件全部原图并计算，有在途件或采集时拒绝；结果只作复核草稿，确认走 `workspace_capture_review_confirm_cuts`。
#[tauri::command]
pub async fn workspace_capture_review_suggest(
    app: AppHandle,
    id: String,
    round: String,
) -> Result<AlignSuggestion, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cycle = app.state::<CycleHost>();
        crate::camera::check_idle(&cycle)?;
        let w = {
            let host = app.state::<WorkspaceHost>();
            let items = host.items.lock().unwrap();
            items.get(&id).ok_or("候选配置不存在")?.clone()
        };
        let capture = cycle.camera.recipe_capture();
        let template = AlignTemplate::load(capture, &w)?;
        let cuts = template.align(capture, &round)?;
        Ok(AlignSuggestion {
            round,
            template_round: template.round_id,
            template_cuts: template.cuts,
            cuts,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 需重点复核的边界说明（第 k 个边界在段 k 与段 k + 1 之间）；全部可信时为 None。
pub(super) fn review_issue(
    cuts: &[crate::frame_align::AlignedCut],
    tolerance: u64,
) -> Option<String> {
    use crate::frame_align::ReviewReason;
    let issues: Vec<String> = cuts
        .iter()
        .enumerate()
        .filter_map(|(k, c)| {
            let what = match c.reason? {
                ReviewReason::Moved => "为保持递增被挪动".to_string(),
                ReviewReason::Deviates => format!(
                    "与比例换算差 {} 帧（容许 {tolerance}）",
                    c.cut.abs_diff(c.proportional)
                ),
                ReviewReason::Unclear => format!("对齐不清晰（{:.2}）", c.sharpness),
            };
            Some(format!("段 {} → 段 {}：{what}", k + 1, k + 2))
        })
        .collect();
    (!issues.is_empty()).then(|| issues.join("；"))
}

/// 按可信的对齐结果保存并确认一件的边界（D-SB-51，记 `ConfirmedBy::Aligned`）。
pub(super) fn confirm_aligned(
    w: &mut Workspace,
    round: &CaptureRoundV2,
    cuts: Vec<u64>,
) -> Result<(), String> {
    apply_capture_review(w, round, CaptureReviewEdit::Save(cuts))?;
    apply_capture_review(w, round, CaptureReviewEdit::Confirm)?;
    if let Some(review) = w.capture_review.rounds.get_mut(&round.round_id) {
        review.confirmed_by = ConfirmedBy::Aligned;
    }
    Ok(())
}

/// 人工复核确认一件标定件的边界：同一候选副本上保存并确认，任一步失败不写入。示教件的边界在分段步骤确认。
#[tauri::command]
pub fn workspace_capture_review_confirm_cuts(
    app: AppHandle,
    id: String,
    revision: u64,
    round: String,
    cuts: Vec<u64>,
) -> Result<WorkspaceView, String> {
    if teaching_round(&app, &id, revision).is_ok_and(|teaching| teaching == round) {
        return Err("示教件的边界在分段步骤确认".into());
    }
    edit_round(app, id, revision, round, |next, captured| {
        apply_capture_review(next, captured, CaptureReviewEdit::Save(cuts))?;
        apply_capture_review(next, captured, CaptureReviewEdit::Confirm)
    })
}

/// 在同一候选副本上整理一件，任一步失败不写入。
fn edit_round(
    app: AppHandle,
    id: String,
    revision: u64,
    round: String,
    edit: impl FnOnce(&mut Workspace, &CaptureRoundV2) -> Result<(), String>,
) -> Result<WorkspaceView, String> {
    let cycle = app.state::<CycleHost>();
    let _gate = cycle
        .config_gate
        .try_lock()
        .map_err(|_| "正在处理采集或设备事务，请稍后整理轮次")?;
    crate::camera::check_idle(&cycle)?;
    let captured = cycle.camera.recipe_capture().get_burst(&round)?;
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let current = items.get_mut(&id).ok_or("候选配置不存在")?;
    current.expect(revision)?;
    let mut next = current.clone();
    edit(&mut next, &captured)?;
    host.save(&next)?;
    *current = next;
    view(&app, current.clone())
}

/// 屏蔽或恢复一件的帧组（标定页）。
#[tauri::command]
pub fn workspace_capture_review_exclude(
    app: AppHandle,
    id: String,
    revision: u64,
    round: String,
    start: u64,
    end: u64,
    excluded: bool,
    reason: String,
) -> Result<WorkspaceView, String> {
    edit_round(app, id, revision, round, |next, captured| {
        apply_capture_review(
            next,
            captured,
            CaptureReviewEdit::Exclude {
                start,
                end,
                excluded,
                reason,
            },
        )
    })
}

#[cfg(test)]
mod tests;
