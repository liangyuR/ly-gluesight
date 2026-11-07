use std::collections::{BTreeMap, BTreeSet};

use crate::burst_judge::{FrameMetrics, FrameState};
use crate::plan::{OffsetMeasure, PathSegment};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Diagnostic {
    pub code: &'static str,
    pub message: String,
}

pub(crate) use crate::vision::color_model_status;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FrameIssue {
    pub seq: u64,
    pub state: FrameState,
    pub expected: bool,
    pub reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EdgeEvidence {
    pub expected_frames: usize,
    pub valid_frames: usize,
    pub valid_rate: Option<f64>,
    pub missing_frames: Vec<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MeasurementEvidence {
    pub expected_frames: usize,
    pub measured_frames: usize,
    pub planned_skip_frames: Vec<u64>,
    pub states: BTreeMap<String, usize>,
    pub non_measured_frames: Vec<FrameIssue>,
    pub unmeasured_frames: Vec<u64>,
    pub edge: Option<EdgeEvidence>,
}

pub(crate) fn measurement_evidence(
    frames: &[FrameMetrics],
    scope: &[u64],
    expected: &[u64],
    to_edge: bool,
) -> MeasurementEvidence {
    let scope: BTreeSet<u64> = scope.iter().copied().collect();
    let expected: BTreeSet<u64> = expected
        .iter()
        .copied()
        .filter(|s| scope.contains(s))
        .collect();
    let frames: BTreeMap<u64, &FrameMetrics> = frames.iter().map(|f| (f.seq, f)).collect();
    let mut evidence = MeasurementEvidence {
        expected_frames: expected.len(),
        measured_frames: 0,
        planned_skip_frames: scope.difference(&expected).copied().collect(),
        states: BTreeMap::new(),
        non_measured_frames: Vec::new(),
        unmeasured_frames: Vec::new(),
        edge: to_edge.then_some(EdgeEvidence {
            expected_frames: expected.len(),
            valid_frames: 0,
            valid_rate: None,
            missing_frames: Vec::new(),
        }),
    };
    for seq in scope {
        let frame = frames.get(&seq);
        let state = frame.map_or(FrameState::Missing, |f| f.state);
        let key = serde_json::to_value(state).unwrap_or_default();
        *evidence
            .states
            .entry(key.as_str().unwrap_or("unknown").into())
            .or_default() += 1;
        let should_measure = expected.contains(&seq);
        if should_measure {
            if state == FrameState::Measured {
                evidence.measured_frames += 1;
            } else {
                evidence.unmeasured_frames.push(seq);
            }
            if let Some(edge) = evidence.edge.as_mut() {
                if state == FrameState::Measured
                    && frame.is_some_and(|f| f.edge_mm.is_some_and(f64::is_finite))
                {
                    edge.valid_frames += 1;
                } else {
                    edge.missing_frames.push(seq);
                }
            }
        }
        if state != FrameState::Measured {
            let reason = if !should_measure {
                "按计划不检测".into()
            } else {
                frame
                    .and_then(|f| f.error.as_ref())
                    .cloned()
                    .unwrap_or_else(|| match state {
                        FrameState::Missing => "缺少图像或帧结果".into(),
                        FrameState::Error => "测量失败".into(),
                        FrameState::Unmeasured => "未执行测量".into(),
                        FrameState::OutOfCorridor => "胶条走出走廊".into(),
                        FrameState::NotInspected => "应检测帧未执行测量".into(),
                        FrameState::Measured => String::new(),
                    })
            };
            evidence.non_measured_frames.push(FrameIssue {
                seq,
                state,
                expected: should_measure,
                reason,
            });
        }
    }
    if let Some(edge) = evidence.edge.as_mut() {
        edge.valid_rate = (edge.expected_frames > 0)
            .then(|| edge.valid_frames as f64 / edge.expected_frames as f64);
    }
    evidence
}

pub(crate) fn edge_requested(segment: &PathSegment, camera: &str) -> bool {
    segment
        .offset_ref
        .as_ref()
        .is_some_and(|r| r.measure == OffsetMeasure::ToEdge && r.camera == camera)
}

pub(crate) fn parameter_warnings(segment: &PathSegment) -> Vec<Diagnostic> {
    let checks = &segment.checks;
    let mut warnings = Vec::new();
    if checks.width.enabled && checks.overflow.enabled {
        let upper = checks.width.nominal_mm + checks.width.tol_upper;
        let nominal = checks.width.nominal_mm;
        let limit = checks.overflow.max_width_mm;
        let overflow = if checks.width.wide_warn_only { "报溢胶预警" } else { "判溢胶" };
        if nominal > limit {
            warnings.push(Diagnostic {
                code: "nominalAboveOverflow",
                message: format!("胶宽标称 {nominal} mm 高于独立溢胶上限 {limit} mm；落在胶宽公差带内也可能{overflow}"),
            });
        } else if upper > limit {
            warnings.push(Diagnostic {
                code: "widthBandAboveOverflow",
                message: format!(
                    "胶宽公差上限 {upper} mm 高于独立溢胶上限 {limit} mm；两项规则分别生效"
                ),
            });
        }
    }
    if segment.checks.offset.as_ref().is_some_and(|o| o.enabled) {
        match segment.offset_ref.as_ref() {
            None => warnings.push(Diagnostic {
                code: "offsetReferenceMissing",
                message: "胶偏已启用但没有参考配置".into(),
            }),
            Some(reference)
                if reference.measure == OffsetMeasure::ToEdge && reference.edge.is_none() =>
            {
                warnings.push(Diagnostic {
                    code: "edgeParametersMissing",
                    message: "ToEdge 没有零件边参数，胶偏无法判定".into(),
                })
            }
            _ => {}
        }
    }
    warnings
}

pub(crate) fn replay_measurements(
    burst: &super::super::burst_history::ComparisonBurst,
    guard: u32,
    refine: bool,
    passes: crate::passes::PassRules,
) -> Value {
    use crate::segmentation::{frame_corridors, resolve_regions, segment_part, SegInput};
    let groups = super::super::burst_history::frame_groups(&burst.frames, burst.cameras.len());
    let total = groups.len() as u64;
    let outcome = segment_part(
        &SegInput {
            groups: &groups,
            segments: &burst.segments,
        },
        &[],
        false,
        passes,
    );
    let regions = resolve_regions(&burst.segments, &outcome, total, 0.5, guard, passes);
    let choice = frame_corridors(&burst.segments, &burst.cameras, &regions, &outcome, total);
    let coverage = crate::segmentation::proportional_coverage(
        &burst.segments,
        &burst.cameras,
        total,
        guard,
        refine,
        &[],
        passes,
    );
    let scope: Vec<u64> = (1..=total).collect();
    let cameras: Vec<Value> = burst.cameras.iter().enumerate().map(|(index, camera)| {
        let frames: Vec<FrameMetrics> = burst.frames.iter().filter(|f| f.camera == index).cloned().collect();
        let expected: Vec<u64> = scope.iter().copied().filter(|seq| choice[index][(*seq - 1) as usize].is_some()).collect();
        let segments: Vec<Value> = burst.segments.iter().map(|segment| {
            let expected: Vec<u64> = scope.iter().copied().filter(|seq| choice[index][(*seq - 1) as usize] == Some(segment.index)).collect();
            let range = coverage.segments.iter().find(|s| s.segment == segment.index).and_then(|s| s.range.as_ref());
            let scope: BTreeSet<u64> = range.into_iter().flat_map(|r| r.first_seq..=r.last_seq).chain(expected.iter().copied()).collect();
            let scope: Vec<u64> = scope.into_iter().collect();
            json!({
                "segment": segment.index,
                "range": range,
                "expectedSeqs": expected,
                "measurement": measurement_evidence(&frames, &scope, &expected, edge_requested(segment, camera)),
            })
        }).collect();
        json!({ "camera": camera, "measurement": measurement_evidence(&frames, &scope, &expected, false), "segments": segments })
    }).collect();
    json!({ "basis": "measurementPlan", "coverage": coverage, "cameras": cameras })
}

pub(crate) fn offset_evidence(
    burst: &super::super::burst_history::ComparisonBurst,
    result: &crate::burst_judge::RegionResult,
) -> Value {
    let crate::segmentation::RegionKind::Straight { segment } = result.region.kind else {
        return Value::Null;
    };
    let Some(segment) = burst.segments.iter().find(|s| s.index == segment) else {
        return Value::Null;
    };
    match crate::burst_judge::offset_window(
        segment,
        &segment.checks,
        &result.region,
        &burst.cameras,
    ) {
        Ok(window) => {
            let expected: Vec<u64> = (window.first_seq..=window.last_seq).collect();
            let frames: BTreeMap<u64, &FrameMetrics> = burst
                .frames
                .iter()
                .filter(|f| f.camera == window.camera)
                .map(|f| (f.seq, f))
                .collect();
            let to_edge = edge_requested(segment, &burst.cameras[window.camera]);
            let missing: Vec<u64> = expected
                .iter()
                .copied()
                .filter(|seq| {
                    frames.get(seq).is_none_or(|f| {
                        f.state != FrameState::Measured
                            || (if to_edge { f.edge_mm } else { f.x_mm })
                                .is_none_or(|v| !v.is_finite())
                    })
                })
                .collect();
            let valid = expected.len() - missing.len();
            json!({
                "camera": burst.cameras[window.camera],
                "measure": segment.offset_ref.as_ref().map(|r| r.measure),
                "window": [window.first_seq, window.last_seq],
                "expectedFrames": expected.len(),
                "validFrames": valid,
                "validRate": valid as f64 / expected.len() as f64,
                "missingFrames": missing,
                "unjudged": result.offset_unjudged,
            })
        }
        Err(reason) => json!({ "window": null, "expectedFrames": 0, "unjudged": reason }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(seq: u64, state: FrameState, edge_mm: Option<f64>) -> FrameMetrics {
        FrameMetrics {
            camera: 0,
            seq,
            state,
            error: None,
            width_min: None,
            width_max: None,
            width_mean: None,
            widths: Vec::new(),
            spacing_mm: 1.0,
            gap_max_mm: 0.0,
            present_ratio: 1.0,
            x_mm: None,
            corridor: Some(1),
            heading_deg: None,
            heading_fallback: false,
            fallback_reason: None,
            edge_mm,
        }
    }

    #[test]
    fn failed_and_absent_frames_remain_unmeasured_while_planned_skip_is_excluded() {
        let frames = vec![
            frame(1, FrameState::Measured, None),
            frame(2, FrameState::Error, None),
            frame(3, FrameState::OutOfCorridor, None),
            frame(4, FrameState::NotInspected, None),
        ];
        let result = measurement_evidence(&frames, &[1, 2, 3, 4, 5], &[1, 2, 3, 5], false);
        assert_eq!((result.expected_frames, result.measured_frames), (4, 1));
        assert_eq!(result.unmeasured_frames, [2, 3, 5]);
        assert_eq!(result.planned_skip_frames, [4]);
        assert_eq!(
            result
                .non_measured_frames
                .iter()
                .map(|f| (f.seq, f.expected))
                .collect::<Vec<_>>(),
            [(2, true), (3, true), (4, false), (5, true)]
        );
    }

    #[test]
    fn edge_missing_and_failed_frames_reduce_valid_rate_without_counting_planned_skip() {
        let frames = vec![
            frame(1, FrameState::Measured, Some(5.0)),
            frame(2, FrameState::Measured, None),
            frame(3, FrameState::Error, Some(6.0)),
            frame(4, FrameState::NotInspected, None),
        ];
        let edge = measurement_evidence(&frames, &[1, 2, 3, 4], &[1, 2, 3], true)
            .edge
            .unwrap();
        assert_eq!((edge.expected_frames, edge.valid_frames), (3, 1));
        assert_eq!(edge.valid_rate, Some(1.0 / 3.0));
        assert_eq!(edge.missing_frames, [2, 3]);
    }

    #[test]
    fn present_model_with_no_background_is_unusable_with_a_recoverable_reason() {
        let status = color_model_status(Some(
            r#"{"quality":{"usable":false,"glueCount":200,"bgCount":0},"background":[]}"#,
        ));
        assert!(status.model_present);
        assert!(!status.model_usable);
        assert!(status
            .issues
            .iter()
            .any(|i| i.code == "backgroundSamplesMissing"));
    }

    #[test]
    fn width_band_crossing_overflow_explains_why_width_tolerance_is_not_enough() {
        let mut segment = crate::plan::new_segment(
            1,
            crate::plan::SegmentKind::Straight,
            &crate::plan::KindDefaults::default(),
        );
        segment.checks.offset = None;
        segment.checks.width.enabled = true;
        segment.checks.width.nominal_mm = 8.0;
        segment.checks.width.tol_upper = 1.5;
        segment.checks.overflow.enabled = true;
        segment.checks.overflow.max_width_mm = 7.0;
        assert!(parameter_warnings(&segment)
            .iter()
            .any(|w| w.code == "nominalAboveOverflow"));
        segment.checks.overflow.max_width_mm = 9.0;
        assert!(parameter_warnings(&segment)
            .iter()
            .any(|w| w.code == "widthBandAboveOverflow"));
        segment.checks.overflow.max_width_mm = 10.0;
        assert!(parameter_warnings(&segment).is_empty());
    }
}
