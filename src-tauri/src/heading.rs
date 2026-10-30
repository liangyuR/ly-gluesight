//! 逐帧定向（D-SB-47）的几何：出胶点与扇形搜索范围。出胶点优先用相机上标定的值，没有时由示教走廊推出；扇区按相机，不按段。
//! 角度为图像坐标的度数：0° 朝右、90° 朝下（与 lyFlow 的 `glue.fan_scan`、`glue.track_path` 一致）。

use serde::Serialize;

use crate::plan::{segment_cameras, PathSegment};
use crate::recipe::CameraInspect;

/// 同一台相机开启定向的各段走廊起点（即出胶点）之间允许的最大距离（mm）。
pub const NOZZLE_TOLERANCE_MM: f64 = 1.0;

/// `glue.track_path` 要求检测窗口至少 8 px 长。
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

/// 扇形搜索范围（度，lo < hi，跨 180°）：以出胶点为圆心、背离离它最近的画面边的半圈。胶嘴装在画面边上，刚挤出的胶条
/// 从出胶点往画面里延伸，胶嘴本体在画面边那一侧；方向不按段示教，弯道转多少度都在里面（D-SB-47）。
pub fn search_sector(nozzle: [f64; 2], size: [u32; 2]) -> [f64; 2] {
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
                "相机 {}：第 {used:?} 段开启了逐帧定向，需要设置测量区间（跳过胶嘴附近与测量到的距离）",
                i.camera
            ))),
            (Some(w), Some(scale)) if w.length_mm / scale < MIN_WINDOW_PX => {
                out.push(camera_issue(format!(
                    "相机 {}：测量区间长 {} mm 换算后不足 {MIN_WINDOW_PX} px",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_sector_is_the_half_facing_away_from_the_nearest_image_edge() {
        // 防的回归（D-SB-47）：扇区朝向胶嘴本体所在的画面边，或随出胶点偏离画面中线而斜过去，贴着画面边走的胶条落到扇区外
        let inside = |s: [f64; 2], deg: f64| {
            s[0] <= deg && deg <= s[1] || s[0] <= deg - 360.0 || deg + 360.0 <= s[1]
        };
        for (nozzle, toward, away) in [
            ([548.0, 888.0], -175.0, 90.0),
            ([618.0, 34.0], 90.0, -90.0),
            ([20.0, 512.0], 0.0, 180.0),
        ] {
            let s = search_sector(nozzle, [1280, 1024]);
            assert!((s[1] - s[0] - 180.0).abs() < 1e-9, "{nozzle:?}");
            assert!(inside(s, toward) && !inside(s, away), "{nozzle:?} → {s:?}");
        }
    }
}
