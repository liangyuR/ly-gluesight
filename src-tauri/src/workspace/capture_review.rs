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
            // 屏蔽不改原始帧组序号，已确认的边界仍有效，只是测量要重做
            invalidate_calibration = before != review.excluded_groups;
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
    }
    w.capture_review
        .rounds
        .insert(round.round_id.clone(), review);
    w.changed();
    Ok(())
}

/// 分段标定工作台的一次保存（D-SB-44）：在候选副本上依次替换段列表（受 `guard_calibration` 约束）、保存并确认
/// 示教件边界、设为模板，全部成功才返回新候选。段列表按示教件上的新旧帧范围逐段对应（`segment_remap`，D-SB-52）：
/// 没动的段连同走廊与示教帧原样保留；示教件还没有可比的旧边界时，段的类型序列变化即清空全部走廊。
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
    if segments.is_empty() {
        return Err("段配方至少需要一段，不能改成拍照点配方".into());
    }
    let mut next = w.clone();
    match super::segment_remap::remap(w, &round.round_id, &segments, &cuts) {
        Some(remapped) => replace_doc(&mut next, remapped.doc, Some(remapped.teaching))?,
        None => {
            burst::update_segments(&mut next, segments)?;
            settle_teaching_frames(&mut next, &round.round_id, &cuts, round.group_count);
        }
    }
    apply_capture_review(&mut next, round, CaptureReviewEdit::Save(cuts))?;
    apply_capture_review(&mut next, round, CaptureReviewEdit::Confirm)?;
    apply_capture_review(&mut next, round, CaptureReviewEdit::SetTemplate)?;
    Ok(next)
}

/// 把指向示教件 `round`、却落在所属段帧范围外的示教帧移到该段中间：换示教件后保留的示教帧来自原件，帧组号在新件上可能
/// 落到别的段（末段被截短、或第一次确认的边界与原件不同）；不移的话下次保存分段按帧组号对应，会把它归到别的段而丢掉走廊。
pub(super) fn settle_teaching_frames(
    w: &mut Workspace,
    round: &str,
    cuts: &[u64],
    group_count: u64,
) {
    let ends: Vec<u64> = cuts.iter().copied().chain([group_count]).collect();
    let span = |index: u32| -> Option<(u64, u64)> {
        let i = w.doc.segments.iter().position(|s| s.index == index)?;
        let last = *ends.get(i)?;
        Some((if i == 0 { 1 } else { ends[i - 1] + 1 }, last))
    };
    let spans: Vec<Option<(u64, u64)>> = w
        .camera_teaching
        .iter()
        .map(|t| t.segment.and_then(span))
        .collect();
    for (t, span) in w.camera_teaching.iter_mut().zip(spans) {
        if let Some((first, last)) = span.filter(|_| t.round == round) {
            if !(first..=last).contains(&t.seq) {
                t.seq = (first + last) / 2;
            }
        }
    }
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

/// 分段保存的影响（保存前确认用）：段结构是否改变、哪些走廊会清掉、保存后哪些段还缺走廊、示教件测量是否失效。
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentSaveImpact {
    pub restructured: bool,
    /// 会清掉的走廊：“原第 j 段（类型）· 相机”
    pub dropped: Vec<String>,
    /// 保存后还没有走廊的（新段, 检测相机）：“第 i 段（类型）· 相机”
    pub untaught: Vec<String>,
    /// 示教件已按当前走廊测量、保存后失效（胶偏参考与起涂收尾建议要重新测量）
    pub measurement_cleared: bool,
    pub coverage: Option<crate::segmentation::InspectionCoverage>,
}

pub(super) fn save_impact(before: &Workspace, after: &Workspace) -> SegmentSaveImpact {
    let kind = |k: crate::plan::SegmentKind| match k {
        crate::plan::SegmentKind::Straight => "直线",
        crate::plan::SegmentKind::Corner => "弯道",
    };
    let carried = |t: &burst::CameraTeaching| {
        after
            .camera_teaching
            .iter()
            .any(|a| a.camera == t.camera && a.round == t.round && a.seq == t.seq)
    };
    let dropped = before
        .camera_teaching
        .iter()
        .filter(|t| !carried(t))
        .filter_map(|t| {
            let segment = before
                .doc
                .segments
                .iter()
                .find(|s| Some(s.index) == t.segment)?;
            before
                .doc
                .inspect
                .iter()
                .find(|i| i.camera == t.camera)?
                .segment_corridor(segment.index)?;
            Some(format!(
                "原第 {} 段（{}）· {}",
                segment.index,
                kind(segment.kind),
                t.camera
            ))
        })
        .collect();
    let cameras = after.doc.plan_cameras();
    let untaught = after
        .doc
        .segments
        .iter()
        .flat_map(|s| {
            crate::plan::segment_cameras(s, &cameras)
                .into_iter()
                .filter(|c| {
                    after
                        .doc
                        .inspect
                        .iter()
                        .find(|i| i.camera == *c)
                        .is_none_or(|i| i.segment_corridor(s.index).is_none())
                })
                .map(move |c| format!("第 {} 段（{}）· {c}", s.index, kind(s.kind)))
        })
        .collect();
    SegmentSaveImpact {
        restructured: navigation::structure_changed(&before.doc, &after.doc),
        dropped,
        untaught,
        measurement_cleared: super::pieces::teaching_measured(before)
            && !super::pieces::teaching_measured(after),
        coverage: after
            .teaching_piece
            .as_ref()
            .and_then(|piece| after.capture_review.rounds.get(&piece.round_id))
            .map(|review| {
                crate::segmentation::proportional_coverage(
                    &after.doc.segments,
                    &cameras,
                    review.group_count,
                    after.doc.boundary_guard,
                    after.doc.boundary_refine,
                    &review
                        .excluded_groups
                        .iter()
                        .map(|group| group.seq)
                        .collect::<Vec<_>>(),
                )
            }),
    }
}

/// 分段标定工作台保存前的预演（只读）：按 `save_template_round` 在副本上保存，返回影响。
#[tauri::command]
pub fn workspace_segment_template_preview(
    app: AppHandle,
    id: String,
    revision: u64,
    segments: Vec<PathSegment>,
    cuts: Vec<u64>,
) -> Result<SegmentSaveImpact, String> {
    let round = teaching_round(&app, &id, revision)?;
    let captured = app
        .state::<CycleHost>()
        .camera
        .recipe_capture()
        .get_burst(&round)?;
    let host = app.state::<WorkspaceHost>();
    let items = host.items.lock().unwrap();
    let current = items.get(&id).ok_or("候选配置不存在")?;
    current.expect(revision)?;
    let next = save_template_round(current, &captured, segments, cuts)?;
    Ok(save_impact(current, &next))
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

/// 屏蔽或恢复示教件的帧组（试测页）：屏蔽的帧组不参与示教件测量。
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
