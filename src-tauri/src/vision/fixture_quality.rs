//! 特征定位的示教检查与验证统计（定稿 docs/discussions/fixture-position/final.md §5“示教”“验证”、§10、§13）。
//! 纯函数：不做 IO、不跑引擎。示教页的保存门（`geometry_checks` + `teach_acceptance`）与验证页的统计
//! （`validation_report`）都在这里，门限的出处写在常量上。

use serde::{Deserialize, Serialize};

use super::taught::PoseRecord;


#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level { Block, Warn }

/// 检查项编号。前端按编号显示，不解析说明文字。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckId {
    // 示教图检查（保存门）
    Inside, Corridor, Texture, Bright, Lever, SelfLocate,
    // 验证统计（发布门）
    LocateAll, Sigma, BrightnessLock, Observable, SearchRange, Nominal, BrightnessReject, Rotation, EndParams, FewSamples,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    pub id: CheckId,
    pub level: Level,
    pub passed: bool,
    pub detail: String,
}

impl CheckItem {
}

/// 杠杆臂 L：示教框中心到最远站的距离。折线上离一点最远的点必在顶点上，所以只看中线顶点。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lever {
    pub from: [f64; 2],
    pub to: [f32; 2],
    pub px: f64,
    pub mm: Option<f64>,
}

/// 示教图检查报告：几何与纹理检查加示教图上的定位验收，存进试测记录（`Trial.fixture`）。
/// 试测有效 = 胶路判定可用且这份报告通过（定稿 §13 的定位验收定义）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureTeachReport {
    pub passed: bool,
    pub checks: Vec<CheckItem>,
    pub pose: Option<PoseRecord>,
    pub lever: Option<Lever>,
    pub texture_std: Option<f64>,
    pub bright_fraction: Option<f64>,
}

/// 示教两端参数的建议（定稿 §10 决定一）：只从验证统计来，用户确认后由后端写进配方。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndSuggestion {
    /// 两端 |Δ| 的 P99（mm）
    pub p99_mm: f64,
    /// maxEndShiftMm = P99 + 1 mm、示教余量 m = P99 + 1 个站距，向上取到 0.1 mm
    pub max_end_shift_mm: f32,
    pub end_margin_mm: f32,
    /// m 会让胶路剩不下 3 站可判时不能采用
    pub applicable: bool,
    pub note: String,
}

/// 开转角的证据（定稿 §10 决定二）：三个条件里前两个自动算，第三个（独立证据）只能人工确认。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationEvidence {
    pub enabled: bool,
    pub lever_mm: f64,
    /// ±3° 下原始验证帧 L·|θ| 的 P95（mm）
    pub lever_theta_p95_mm: Option<f64>,
    /// 公差/5
    pub threshold_mm: f64,
    pub exceeds: bool,
    /// ±3° 下全部验证帧（含亮度 ±40）都定位成功且看得出角度
    pub frames: usize,
    pub observable_frames: usize,
    pub all_observable: bool,
    /// 前两个条件都满足；还要人工给出独立证据才可以开转角
    pub auto_conditions: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureSampleRow {
    pub name: String,
    pub pose: Option<PoseRecord>,
    pub brighter: Option<PoseRecord>,
    pub darker: Option<PoseRecord>,
    pub error: Option<String>,
}

/// 一个特征定位视图在独立正常样本上的验证统计（`Validation.fixture`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureViewReport {
    pub k: usize,
    pub view: u8,
    pub shot_id: String,
    pub passed: bool,
    pub samples: usize,
    pub checks: Vec<CheckItem>,
    /// 全部正常样本、全部有基准站的 d′ 合并算的 σ 与 μ（mm，D2 口径）
    pub sigma_mm: Option<f64>,
    pub mean_mm: Option<f64>,
    pub nominal_mm: f64,
    pub tolerance_mm: f64,
    /// 位姿单轴偏移 / 搜索半径的最大值
    pub search_use: Option<f64>,
    pub suggestion: Option<EndSuggestion>,
    pub rotation: RotationEvidence,
    pub rows: Vec<FixtureSampleRow>,
}

