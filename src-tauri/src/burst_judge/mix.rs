use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::recipe::MixCheck;

pub const MIX_STATION_THRESHOLD: f64 = 0.16;
const BACKGROUND_SHIFT_LIMIT: f64 = 3.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Yellow,
    Blue,
}

impl Direction {
    fn from_axis(value: f64, threshold: f64) -> Option<Self> {
        if value <= -threshold {
            Some(Self::Yellow)
        } else if value >= threshold {
            Some(Self::Blue)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorRegion {
    direction: Direction,
    contours: Vec<Vec<[i32; 2]>>,
    start_px: f64,
    end_px: f64,
    confirmed_length_px: f64,
    span_px: f64,
    max_width_px: u32,
    area_px2: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MixStation {
    s_px: f64,
    center: [f64; 2],
    median_axial: Option<f64>,
    surface_pixels: u32,
    corridor_samples: u32,
    valid_fraction: f64,
    highlight_samples: u32,
    dark_samples: u32,
    off_axis_samples: u32,
    outside_samples: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Background {
    valid_pixels: u64,
    total_pixels: u64,
    delta: Option<[f64; 2]>,
    normalized_shift: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Observation {
    stations: Vec<MixStation>,
    regions: Vec<ColorRegion>,
    background: Background,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShiftRun {
    direction: Direction,
    start_mm: f64,
    end_mm: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixEvidence {
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dataset_kind: Option<String>,
    mm_per_px: Option<f64>,
    pub incomplete: bool,
    pub reason: Option<String>,
    directions: BTreeSet<Direction>,
    shift_runs: Vec<ShiftRun>,
    observation: Option<Observation>,
}

impl MixEvidence {
    pub fn unavailable(check: &MixCheck, reason: impl Into<String>) -> Self {
        Self {
            model_id: check.model_id.clone(),
            dataset_kind: check
                .model
                .as_ref()
                .and_then(|m| m["datasetKind"].as_str())
                .map(str::to_owned),
            mm_per_px: None,
            incomplete: true,
            reason: Some(reason.into()),
            directions: BTreeSet::new(),
            shift_runs: Vec::new(),
            observation: None,
        }
    }

    pub fn alarm(&self) -> bool {
        !self.directions.is_empty()
    }

    pub fn omit_observation(&mut self) {
        self.observation = None;
        self.shift_runs = Vec::new();
    }

    pub(crate) fn summary_heap_bytes(&self) -> usize {
        self.model_id
            .iter()
            .chain(&self.dataset_kind)
            .chain(&self.reason)
            .map(String::capacity)
            .sum::<usize>()
            + if self.directions.is_empty() {
                0
            } else {
                16 * std::mem::size_of::<usize>()
            }
    }

    pub fn from_observation(
        observation: Value,
        check: &MixCheck,
        mm_per_px: f64,
        station_threshold: f64,
    ) -> Result<Self, String> {
        let obs: Observation =
            serde_json::from_value(observation).map_err(|e| format!("混胶观测结构无效：{e}"))?;
        if !(mm_per_px.is_finite()
            && mm_per_px > 0.0
            && 0.0 < station_threshold
            && station_threshold <= 1.0)
        {
            return Err("混胶判定缺少有效像素当量或测站门限".into());
        }
        if obs.stations.windows(2).any(|w| w[1].s_px <= w[0].s_px)
            || obs.stations.iter().any(|s| {
                s.surface_pixels > s.corridor_samples
                    || s.median_axial.is_some() != (s.surface_pixels > 0)
                    || s.median_axial.is_some_and(|v| v.abs() > 1.0)
            })
        {
            return Err("混胶测站位置或可测像素数与轴向偏移不一致".into());
        }
        let runs = shift_runs(&obs.stations, station_threshold, mm_per_px);
        let mut directions: BTreeSet<_> = obs
            .regions
            .iter()
            .filter(|r| {
                r.span_px * mm_per_px >= check.max_length_mm
                    || r.area_px2 as f64 * mm_per_px.powi(2) >= check.max_area_mm2
            })
            .map(|r| r.direction)
            .collect();
        directions.extend(
            runs.iter()
                .filter(|r| r.end_mm - r.start_mm >= check.drift_length_mm)
                .map(|r| r.direction),
        );
        let mut medians: Vec<_> = obs.stations.iter().filter_map(|s| s.median_axial).collect();
        let reason = if medians.is_empty() {
            Some("胶路没有可测胶面")
        } else if obs.background.normalized_shift.is_none()
            || obs.background.delta.is_none()
            || obs.background.valid_pixels == 0
        {
            Some("背景参考不可测，无法排除成像漂移")
        } else if obs
            .background
            .normalized_shift
            .is_some_and(|s| s >= BACKGROUND_SHIFT_LIMIT)
        {
            medians.sort_unstable_by(f64::total_cmp);
            let center = (medians[medians.len() / 2] + medians[(medians.len() - 1) / 2]) * 0.5;
            let direction = Direction::from_axis(center, station_threshold);
            match direction {
                Some(side)
                    if concordant_background(check, side, obs.background.delta.unwrap())? =>
                {
                    Some("成像漂移：胶面和背景色度同向变化")
                }
                _ => None,
            }
        } else {
            None
        };
        if reason.is_some() {
            directions.clear();
        }
        let incomplete = reason.is_some()
            || obs
                .stations
                .iter()
                .any(|s| s.surface_pixels == 0 || s.highlight_samples > 0);
        Ok(Self {
            model_id: check.model_id.clone(),
            dataset_kind: check
                .model
                .as_ref()
                .and_then(|m| m["datasetKind"].as_str())
                .map(str::to_owned),
            mm_per_px: Some(mm_per_px),
            incomplete,
            reason: reason.map(str::to_owned),
            directions,
            shift_runs: runs,
            observation: Some(obs),
        })
    }
}

fn concordant_background(
    check: &MixCheck,
    direction: Direction,
    delta: [f64; 2],
) -> Result<bool, String> {
    let name = match direction {
        Direction::Yellow => "yellow",
        Direction::Blue => "blue",
    };
    let centers = check
        .model
        .as_ref()
        .and_then(|v| v.get("centers"))
        .ok_or("混胶模型缺少参考色度")?;
    let center = |k: &str| -> Result<[f64; 2], String> {
        serde_json::from_value(centers[k].clone()).map_err(|_| format!("混胶模型缺少 {k} 色度"))
    };
    let green = center("green")?;
    let end = center(name)?;
    Ok(delta[0] * (end[0] - green[0]) + delta[1] * (end[1] - green[1]) > 0.0)
}

fn shift_runs(stations: &[MixStation], threshold: f64, mm_per_px: f64) -> Vec<ShiftRun> {
    let mut runs = Vec::<ShiftRun>::new();
    let mut previous = None;
    for (i, s) in stations.iter().enumerate() {
        let direction = s
            .median_axial
            .and_then(|v| Direction::from_axis(v, threshold));
        if let Some(side) = direction {
            let hi = stations
                .get(i + 1)
                .map_or(s.s_px, |n| (n.s_px + s.s_px) * 0.5)
                * mm_per_px;
            if previous == direction {
                runs.last_mut().unwrap().end_mm = hi;
            } else {
                let lo = i
                    .checked_sub(1)
                    .map_or(s.s_px, |j| (stations[j].s_px + s.s_px) * 0.5);
                runs.push(ShiftRun {
                    direction: side,
                    start_mm: lo * mm_per_px,
                    end_mm: hi,
                });
            }
        }
        previous = direction;
    }
    runs
}
