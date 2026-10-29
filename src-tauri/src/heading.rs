//! 逐帧定向（D-SB-47）的几何：出胶点、各段方向范围与扇区。出胶点优先用相机上标定的值，其余由示教走廊和段设置推出。
//! 角度为图像坐标的度数：0° 朝右、90° 朝下（与 `glue.bead_path` 一致）。

use serde::Serialize;

use crate::plan::{segment_cameras, HeadingSettings, PathSegment, SegmentKind};
use crate::recipe::CameraInspect;

/// 同一台相机开启定向的各段走廊起点（即出胶点）之间允许的最大距离（mm）。
pub const NOZZLE_TOLERANCE_MM: f64 = 1.0;

/// `glue.bead_path` 要求检测窗口至少 8 px 长。
const MIN_WINDOW_PX: f64 = 8.0;

/// 定向配置上的问题：`segment` 为 None 是相机级（检测窗口、出胶点）。
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub camera: String,
    pub segment: Option<u32>,
    pub message: String,
}

/// 把角度差折到 (−180, 180]。
pub fn wrap(d: f64) -> f64 {
    let r = (d + 180.0).rem_euclid(360.0) - 180.0;
    if r == -180.0 {
        180.0
    } else {
        r
    }
}

/// 走廊方向：起点指向终点；不足两点或首尾重合时为 None。
pub fn corridor_heading(corridor: &[[f32; 2]]) -> Option<f64> {
    let (a, b) = (corridor.first()?, corridor.last()?);
    let (dx, dy) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
    (dx.hypot(dy) > 0.0).then(|| dy.atan2(dx).to_degrees())
}

fn taught(inspect: &CameraInspect, segment: u32) -> Option<&[[f32; 2]]> {
    inspect
        .segment_corridor(segment)
        .map(|s| s.corridor.as_slice())
        .filter(|c| c.len() >= 2)
}

fn detected(segment: &PathSegment, inspect: &CameraInspect, cameras: &[String]) -> bool {
    segment_cameras(segment, cameras).contains(&inspect.camera)
}

/// 这台相机开启定向的段：它检测、且示教了走廊。
fn heading_segments<'a>(
    segments: &'a [PathSegment],
    cameras: &'a [String],
    inspect: &'a CameraInspect,
) -> impl Iterator<Item = &'a PathSegment> {
    segments.iter().filter(move |s| {
        s.heading.is_some() && detected(s, inspect, cameras) && taught(inspect, s.index).is_some()
    })
}

/// 出胶点（px）与来源：`calibrated` 为真是相机上标定的出胶点，否则由走廊起点推出。
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Nozzle {
    pub point: [f64; 2],
    pub calibrated: bool,
}

/// 出胶点：相机标定了就用标定值；否则取这台相机开启定向的各段走廊起点的平均（D-SB-42 走廊从出胶点一侧起画），
/// 没有开启定向的段时为 None，各起点离平均点超过 `NOZZLE_TOLERANCE_MM` 时报错（还没有像素当量时不核对距离）。
pub fn nozzle(
    segments: &[PathSegment],
    cameras: &[String],
    inspect: &CameraInspect,
) -> Result<Option<Nozzle>, String> {
    if let Some(point) = inspect.nozzle {
        return Ok(Some(Nozzle {
            point,
            calibrated: true,
        }));
    }
    let starts: Vec<(u32, [f64; 2])> = heading_segments(segments, cameras, inspect)
        .filter_map(|s| {
            taught(inspect, s.index).map(|c| (s.index, [f64::from(c[0][0]), f64::from(c[0][1])]))
        })
        .collect();
    if starts.is_empty() {
        return Ok(None);
    }
    let n = starts.len() as f64;
    let mean = [
        starts.iter().map(|(_, p)| p[0]).sum::<f64>() / n,
        starts.iter().map(|(_, p)| p[1]).sum::<f64>() / n,
    ];
    if let Some(scale) = inspect.mm_per_px {
        if let Some((segment, p)) = starts
            .iter()
            .find(|(_, p)| (p[0] - mean[0]).hypot(p[1] - mean[1]) * scale > NOZZLE_TOLERANCE_MM)
        {
            return Err(format!(
                "相机 {}：开启逐帧定向的各段走廊起点（出胶点）不一致，第 {segment} 段起点 ({:.0}, {:.0}) 离其余各段超过 {NOZZLE_TOLERANCE_MM} mm，请在示教帧上标定出胶点，或从同一出胶点重画走廊",
                inspect.camera, p[0], p[1]
            ));
        }
    }
    Ok(Some(Nozzle {
        point: mean,
        calibrated: false,
    }))
}

/// 第 `segment` 段的示教方向：走廊起点指向终点；标定了出胶点时取背离出胶点的方向（终点比起点离出胶点近就反过来），
/// 所以走廊从哪头画起都一样。没有示教走廊时为 None。
pub fn segment_heading(inspect: &CameraInspect, segment: u32) -> Option<f64> {
    let corridor = taught(inspect, segment)?;
    let h = corridor_heading(corridor)?;
    let Some(n) = inspect.nozzle else {
        return Some(h);
    };
    let dist = |p: &[f32; 2]| (f64::from(p[0]) - n[0]).hypot(f64::from(p[1]) - n[1]);
    Some(if dist(corridor.last()?) < dist(&corridor[0]) {
        wrap(h + 180.0)
    } else {
        h
    })
}

fn tolerance(segment: &PathSegment) -> f64 {
    segment
        .heading
        .as_ref()
        .map_or(HeadingSettings::default().tolerance_deg, |h| {
            h.tolerance_deg
        })
}

/// 第 k 段（下标）这台相机的方向范围 [lo, hi]（度，lo ≤ hi，不折回）：直线段为示教方向 ± 容差；
/// 弯道取它自己和相邻直线段（这台相机示教了走廊的）的示教方向所跨的范围，再各放宽容差。本段没有示教走廊时为 None。
pub fn direction_range(
    segments: &[PathSegment],
    cameras: &[String],
    inspect: &CameraInspect,
    k: usize,
) -> Option<[f64; 2]> {
    let segment = segments.get(k)?;
    let own = segment_heading(inspect, segment.index)?;
    let tol = tolerance(segment);
    let mut rel = vec![0.0];
    if segment.kind == SegmentKind::Corner {
        for j in [k.checked_sub(1), Some(k + 1)].into_iter().flatten() {
            let Some(neighbour) = segments
                .get(j)
                .filter(|s| s.kind == SegmentKind::Straight && detected(s, inspect, cameras))
            else {
                continue;
            };
            if let Some(h) = segment_heading(inspect, neighbour.index) {
                rel.push(wrap(h - own));
            }
        }
    }
    let lo = rel.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = rel.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some([own + lo - tol, own + hi + tol])
}

/// 帧按预测落在第 k 段时的扇形搜索范围：第 k 段与相邻两段（这台相机检测且示教了走廊的）方向范围的并集，
/// 预测错一两帧时相邻段的方向仍在扇区内。跨度不超过 360°。
pub fn sector(
    segments: &[PathSegment],
    cameras: &[String],
    inspect: &CameraInspect,
    k: usize,
) -> Option<[f64; 2]> {
    let own = direction_range(segments, cameras, inspect, k)?;
    let base = (own[0] + own[1]) / 2.0;
    let (mut lo, mut hi) = (own[0], own[1]);
    for j in [k.checked_sub(1), Some(k + 1)].into_iter().flatten() {
        if !segments
            .get(j)
            .is_some_and(|s| detected(s, inspect, cameras))
        {
            continue;
        }
        if let Some([a, b]) = direction_range(segments, cameras, inspect, j) {
            let shift = base + wrap((a + b) / 2.0 - base) - (a + b) / 2.0;
            lo = lo.min(a + shift);
            hi = hi.max(b + shift);
        }
    }
    if hi - lo > 360.0 {
        hi = lo + 360.0;
    }
    Some([lo, hi])
}

/// 定向配置是否齐全（D-SB-47）：开启定向的段，它的每台检测相机要有检测窗口（换算后不短于 8 px）、出胶点一致。
/// 走廊与当量缺失由 `untaught_corridors` 报，这里不重复。
pub fn issues(segments: &[PathSegment], inspect: &[CameraInspect]) -> Vec<Issue> {
    let cameras: Vec<String> = inspect.iter().map(|i| i.camera.clone()).collect();
    let mut out = Vec::new();
    for i in inspect {
        let used: Vec<u32> = segments
            .iter()
            .filter(|s| s.heading.is_some() && detected(s, i, &cameras))
            .map(|s| s.index)
            .collect();
        if used.is_empty() {
            continue;
        }
        let camera_issue = |message: String| Issue {
            camera: i.camera.clone(),
            segment: None,
            message,
        };
        match (&i.window, i.mm_per_px) {
            (None, _) => out.push(camera_issue(format!(
                "相机 {}：第 {used:?} 段开启了逐帧定向，需要设置检测窗口",
                i.camera
            ))),
            (Some(w), Some(scale)) if w.length_mm / scale < MIN_WINDOW_PX => {
                out.push(camera_issue(format!(
                    "相机 {}：检测窗口长 {} mm 换算后不足 {MIN_WINDOW_PX} px",
                    i.camera, w.length_mm
                )))
            }
            _ => {}
        }
        if let Err(message) = nozzle(segments, &cameras, i) {
            out.push(camera_issue(message));
        }
    }
    out
}
