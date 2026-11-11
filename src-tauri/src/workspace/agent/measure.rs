use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::{blocking, burst, changed, diagnostics, open, snapshot, text, Content, ToolError};
use crate::burst_judge::FrameMetrics;
use crate::cycle::CycleHost;
use crate::recipe_capture::CaptureRoundV2;
use crate::workspace::{pieces, Workspace, WorkspaceHost};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Args {
    recipe: String,
    expected_revision: u64,
    expected_instance_id: Option<u64>,
    task_id: Option<String>,
}

pub(super) async fn call(app: &AppHandle, args: Args) -> Result<Vec<Content>, ToolError> {
    let project = crate::project::current_id(app);
    let before = open(app, &args.recipe, args.expected_instance_id)
        .await?
        .workspace;
    before.expect(args.expected_revision)?;
    let instance = args.expected_instance_id.unwrap_or(before.instance_id);
    let saved = pieces::workspace_measure_teaching_at(
        app.clone(),
        args.recipe.clone(),
        args.expected_revision,
        Some(instance),
        args.task_id,
    )
    .await?
    .workspace;
    changed(app, &args.recipe);

    let cycle = app.state::<CycleHost>();
    let _config = cycle.config_gate.lock().await;
    let (task, used) = (app.clone(), saved.clone());
    let result = blocking(move || {
        let host = task.state::<WorkspaceHost>();
        let items = host.items.lock().unwrap();
        let current = items
            .get(&used.doc.id)
            .ok_or("候选已删除，保存结果仍属于原快照")?;
        if current.instance_id != used.instance_id || current.revision != used.revision {
            return Err(
                "测量已保存，但候选随后变化；请重新读取当前状态，不复用其他快照的测量".into(),
            );
        }
        let piece = used.teaching_piece.as_ref().ok_or("保存的快照没有示教件")?;
        let round = task
            .state::<CycleHost>()
            .camera
            .recipe_capture()
            .get_burst(&piece.round_id)?;
        let dir = host
            .dir(&used.doc.id)
            .join(burst::CALIBRATION_DIR)
            .join(&piece.round_id);
        let metrics = burst::read_features(&dir, &used.doc.plan_cameras())?;
        drop(items);
        let measurement = saved_measurements(&used, &round, &metrics)?;
        let suggestion = crate::workspace::burst_skip::pass_skip_suggestion(
            &used.doc,
            Some((&piece.round_id, round.group_count, &piece.passes)),
            &used.capture_review.rounds,
            Ok(metrics),
        );
        let passes = json!({
            "count": piece.passes.len(),
            "skip": used.doc.pass_skip,
            "suggestion": suggestion,
        });
        Ok((measurement, passes))
    })
    .await;

    let mut response = json!({
        "committed": true,
        "baseRevision": before.revision,
        "revision": saved.revision,
        "snapshot": snapshot(app, &project, &saved),
        "acceptance": "notEvaluated",
        "message": "示教件测量已保存；执行测量、测量完整和独立好坏件验收是不同结论，未发布配方",
    });
    match result {
        Ok((measurement, passes)) => {
            response["measurement"] = measurement;
            response["passes"] = passes;
        }
        Err(error) => {
            response["measurement"] = Value::Null;
            response["summaryError"] = json!(error);
        }
    }
    Ok(vec![text(response)])
}

pub(super) fn saved_measurements(
    workspace: &Workspace,
    round: &CaptureRoundV2,
    metrics: &[FrameMetrics],
) -> Result<Value, String> {
    let cameras = workspace.doc.plan_cameras();
    let excluded: Vec<u64> = workspace
        .capture_review
        .rounds
        .get(&round.round_id)
        .into_iter()
        .flat_map(|review| review.excluded_groups.iter().map(|group| group.seq))
        .collect();
    let coverage = crate::segmentation::proportional_coverage(
        &workspace.doc.segments,
        &cameras,
        round.group_count,
        workspace.doc.boundary_guard,
        &excluded,
        pieces::piece_pass_rules(workspace),
    );
    if !coverage.boundaries_trusted {
        return Err("示教件边界不可信，不能给出生产应测范围的测量摘要".into());
    }
    let scope: Vec<u64> = (1..=round.group_count).collect();
    let camera_measurements: Vec<Value> = cameras
        .iter()
        .enumerate()
        .map(|(index, camera)| {
            let expected: BTreeSet<u64> = coverage
                .segments
                .iter()
                .flat_map(|segment| &segment.cameras)
                .filter(|item| item.camera == *camera)
                .flat_map(|item| &item.ranges)
                .flat_map(|range| range.first_seq..=range.last_seq)
                .collect();
            let frames: Vec<FrameMetrics> = metrics
                .iter()
                .filter(|frame| frame.camera == index)
                .cloned()
                .collect();
            json!({
                "camera": camera,
                "measurement": diagnostics::measurement_evidence(
                    &frames, &scope, &expected.into_iter().collect::<Vec<_>>(), false,
                ),
            })
        })
        .collect();
    Ok(json!({
        "basis": "teachingPieceMeasurementPlan",
        "summary": diagnostics::measurement_summary(&camera_measurements),
        "cameras": camera_measurements,
        "coverage": coverage,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::burst_judge::{FrameState, MeasurementEvidence};
    use crate::plan::SegmentKind;
    use crate::workspace::burst_tests::{burst_workspace, capture_round, measured, DEVICE};
    use crate::workspace::pieces_tests::piece;

    #[test]
    fn planned_pass_skip_and_unknown_stations_keep_saved_measurement_incomplete() {
        let mut workspace = burst_workspace(&[SegmentKind::Straight, SegmentKind::Straight]);
        for segment in &mut workspace.doc.segments {
            segment.cameras = vec!["cam1".into()];
        }
        workspace.doc.boundary_guard = 0;
        workspace.doc.pass_skip = crate::passes::PassSkip { head: 1, tail: 0 };
        workspace.teaching_piece = Some(piece("teach", 1));
        workspace.teaching_piece.as_mut().unwrap().passes = vec![4];
        let round = capture_round("teach", &workspace.doc, 4, DEVICE, &[]);
        workspace = crate::workspace::capture_review::save_template_round(
            &workspace,
            &round,
            workspace.doc.segments.clone(),
            vec![2],
        )
        .unwrap();
        let formed: Vec<FrameMetrics> = (1..=4)
            .map(|seq| {
                if seq == 1 {
                    burst::frame_without_metrics(0, seq, FrameState::Error, None)
                } else {
                    FrameMetrics {
                        seq,
                        ..measured(0.0)
                    }
                }
            })
            .collect();
        assert_eq!(
            saved_measurements(&workspace, &round, &formed).unwrap()["summary"]["complete"],
            true
        );
        let mut unknown = measured(0.0);
        unknown.seq = 3;
        unknown.measurement_evidence = Some(MeasurementEvidence {
            version: 1,
            stations: Vec::new(),
            exact_stations: 0,
            clipped_stations: 0,
            absent_stations: 0,
            unknown_stations: 1,
            max_width_lower_bound_mm: None,
            present_ratio: None,
        });
        let frames = vec![
            burst::frame_without_metrics(0, 1, FrameState::Error, None),
            FrameMetrics {
                seq: 2,
                ..measured(0.0)
            },
            unknown,
        ];
        let result = saved_measurements(&workspace, &round, &frames).unwrap();
        assert_eq!(result["summary"]["expectedFrames"], 3);
        assert_eq!(result["summary"]["measuredFrames"], 2);
        assert_eq!(result["summary"]["completeFrames"], 1);
        assert_eq!(result["summary"]["noEvidenceFrames"], 1);
        assert_eq!(result["summary"]["unmeasuredFrames"], 1);
        assert_eq!(result["summary"]["complete"], false);
        assert_eq!(
            result["cameras"][0]["measurement"]["plannedSkipFrames"],
            json!([1])
        );
        assert_eq!(
            result["cameras"][0]["measurement"]["noEvidenceFrames"],
            json!([3])
        );
        assert_eq!(
            result["cameras"][0]["measurement"]["unmeasuredFrames"],
            json!([4])
        );
    }
}
