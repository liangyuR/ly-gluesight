//! 逐帧定向（D-SB-47）的几何：出胶点与扇形搜索范围。出胶点优先用相机上标定的值，没有时由示教走廊推出；扇区按相机，不按段。
//! 角度为图像坐标的度数：0° 朝右、90° 朝下（与 lyFlow 的 `glue.fan_scan`、`glue.track_path` 一致）。

use serde::Serialize;

use crate::plan::{segment_cameras, PathSegment};
use crate::recipe::{CameraInspect, HeadingWindow};

/// 同一台相机开启定向的各段走廊起点（即出胶点）之间允许的最大距离（mm）。
pub const NOZZLE_TOLERANCE_MM: f64 = 1.0;

/// `glue.track_path` 要求检测窗口至少 8 px 长。
const MIN_WINDOW_PX: f64 = 8.0;

/// 定向配置上的问题（按相机与段）。
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub camera: String,
    pub segment: Option<u32>,
    pub message: String,
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

/// 出胶点（px）与来源：`calibrated` 为真是相机上标定的出胶点，否则由走廊起点推出；`warning` 是推算时起点相差过大的提示。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Nozzle {
    pub point: [f64; 2],
    pub calibrated: bool,
    pub warning: Option<String>,
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

/// 出胶点：相机标定了就用标定值；否则取这台相机开启定向的各段走廊起点的中位数（D-SB-42 走廊从出胶点一侧起画，
/// 中位数不被个别画偏的起点拉走），没有开启定向的段时为 None。手画起点总有偏差，不阻断：有起点离推算点超过
/// `NOZZLE_TOLERANCE_MM` 时只在 `warning` 里提示离得最远的那段（还没有像素当量时不核对距离）。
pub fn nozzle(
    segments: &[PathSegment],
    cameras: &[String],
    inspect: &CameraInspect,
) -> Option<Nozzle> {
    if let Some(point) = inspect.nozzle {
        return Some(Nozzle {
            point,
            calibrated: true,
            warning: None,
        });
    }
    let starts: Vec<(u32, [f64; 2])> = heading_segments(segments, cameras, inspect)
        .filter_map(|s| {
            taught(inspect, s.index).map(|c| (s.index, [f64::from(c[0][0]), f64::from(c[0][1])]))
        })
        .collect();
    if starts.is_empty() {
        return None;
    }
    let point = [0, 1].map(|axis| median(starts.iter().map(|(_, p)| p[axis]).collect()));
    let warning = inspect.mm_per_px.and_then(|scale| {
        let off = |p: &[f64; 2]| (p[0] - point[0]).hypot(p[1] - point[1]) * scale;
        starts
            .iter()
            .max_by(|a, b| off(&a.1).total_cmp(&off(&b.1)))
            .filter(|(_, p)| off(p) > NOZZLE_TOLERANCE_MM)
            .map(|(segment, p)| format!(
                "相机 {}：第 {segment} 段走廊起点 ({:.0}, {:.0}) 离推算的出胶点 {:.1} mm，超过 {NOZZLE_TOLERANCE_MM} mm；只提示，不影响试测与发布，确认这段走廊是从出胶点起画的，或在示教帧上标定出胶点",
                inspect.camera, p[0], p[1], off(p)
            ))
    });
    Some(Nozzle {
        point,
        calibrated: false,
        warning,
    })
}

/// 扇形搜索范围（度，lo < hi，跨 180°）：以出胶点为圆心的半圈，中线取相机上拖出的方向 `turned`；没拖过时背离离出胶点
/// 最近的画面边。胶嘴装在画面边上，刚挤出的胶条从出胶点往画面里延伸，胶嘴本体在画面边那一侧；方向不按段示教，
/// 弯道转多少度都在里面（D-SB-47）。
pub fn search_sector(nozzle: [f64; 2], size: [u32; 2], turned: Option<f64>) -> [f64; 2] {
    if let Some(mid) = turned {
        return [mid - 90.0, mid + 90.0];
    }
    let (w, h) = (f64::from(size[0]) - 1.0, f64::from(size[1]) - 1.0);
    let edges = [
        (nozzle[0], 0.0),
        (w - nozzle[0], 180.0),
        (nozzle[1], 90.0),
        (h - nozzle[1], -90.0),
    ];
    let mid = edges
        .iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(0.0, |e| e.1);
    [mid - 90.0, mid + 90.0]
}

/// 测量区间从哪来：本段设的，或旧配方这台相机的相机级区间。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowSource {
    Segment,
    Camera,
}

/// 这台相机在这一段用的测量区间（D-SB-47）：本段设了就用本段的（本段各检测相机共用），否则沿用旧配方的相机级区间；
/// 都没有为 None。不看本段是否开启定向。
pub fn window<'a>(
    segment: &'a PathSegment,
    inspect: &'a CameraInspect,
) -> Option<(&'a HeadingWindow, WindowSource)> {
    match segment.heading.as_ref().and_then(|h| h.window.as_ref()) {
        Some(w) => Some((w, WindowSource::Segment)),
        None => inspect.window.as_ref().map(|w| (w, WindowSource::Camera)),
    }
}

/// 定向配置是否齐全（D-SB-47）：开启定向的段，它的每台检测相机要有测量区间（换算后不短于 8 px）。出胶点起点相差过大只提示，见 `nozzle`。
/// 走廊与当量缺失由 `untaught_corridors` 报，这里不重复。
pub fn issues(segments: &[PathSegment], inspect: &[CameraInspect]) -> Vec<Issue> {
    let cameras: Vec<String> = inspect.iter().map(|i| i.camera.clone()).collect();
    let mut out = Vec::new();
    for i in inspect {
        for s in segments
            .iter()
            .filter(|s| s.heading.is_some() && detected(s, i, &cameras))
        {
            let message = match (window(s, i), i.mm_per_px) {
                (None, _) => format!(
                    "第 {} 段相机 {}：开启了逐帧定向，需要设置本段的测量区间（跳过胶嘴附近与测量到的距离）",
                    s.index, i.camera
                ),
                (Some((w, _)), Some(scale)) if w.length_mm / scale < MIN_WINDOW_PX => format!(
                    "第 {} 段相机 {}：测量区间长 {} mm 换算后不足 {MIN_WINDOW_PX} px",
                    s.index, i.camera, w.length_mm
                ),
                _ => continue,
            };
            out.push(Issue {
                camera: i.camera.clone(),
                segment: Some(s.index),
                message,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{new_segment, KindDefaults, SegmentKind};

    #[test]
    fn one_far_corridor_start_warns_without_blocking_or_pulling_the_nozzle() {
        let starts = [
            (4, [570.0, 980.0]),
            (5, [570.0, 980.0]),
            (11, [581.0, 990.0]),
            (61, [619.0, 937.0]),
        ];
        let segments: Vec<PathSegment> = starts
            .iter()
            .map(|&(index, _)| new_segment(index, SegmentKind::Straight, &KindDefaults::default()))
            .collect();
        let inspect: CameraInspect = serde_json::from_value(serde_json::json!({
            "camera": "cam3",
            "mmPerPx": 0.0757,
            "window": { "startMm": 5.0, "lengthMm": 25.0 },
            "segments": starts.iter().map(|&(segment, p)| serde_json::json!({
                "segment": segment,
                "corridor": [p, [p[0] + 200.0, p[1] - 200.0]],
            })).collect::<Vec<_>>(),
        }))
        .unwrap();
        let nozzle = nozzle(&segments, &["cam3".into()], &inspect).unwrap();
        assert_eq!(nozzle.point, [575.5, 980.0]);
        assert!(nozzle.warning.unwrap().contains("第 61 段"));
        assert!(issues(&segments, std::slice::from_ref(&inspect)).is_empty());
    }
}
