//! 配方：磁盘上存可编辑的 `RecipeDoc`，加载时生成运行用的 `Recipe`（分段、测量点、修订标识）。
//! 一期按拍照点在图像里检测（P0 D-10）：每个拍照点示教一条胶路中线（图像像素），沿线每隔 spacing 一站；
//! 每个拍照点自成一段，判定在段内做，段与段之间不连。`Recipe` 同时是检测记录里的配方快照。
//!
//! 分段连拍（schema 8，docs/decisions/segment-burst.md D-SB-1）：`segments` 非空的配方是段配方——沿胶路的段列表、
//! 三台检测相机（`inspect`，共用一个触发）、整件标定；`segments` 为空仍是旧拍照点配方，走旧校验、旧节拍、旧发布，两套永久并存。
//! 注意 `Recipe.segments` 是旧模型 build 出的测量段，段配方的段列表在 `Recipe.path_segments`（JSON `pathSegments`）。

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::plan::{self, KindDefaults, PartCalib, PathSegment, SegmentKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TriggerMode {
    Fly,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeParams {
    pub nominal: f32,
    pub tol_upper: f32,
    pub tol_lower: f32,
    pub abs_min: f32,
    pub abs_max: f32,
    pub max_excursion_len: f32,
}

impl JudgeParams {
    pub fn lower(&self) -> f32 {
        self.nominal - self.tol_lower
    }
    pub fn upper(&self) -> f32 {
        self.nominal + self.tol_upper
    }

    fn validate(&self, what: &str) -> Result<(), String> {
        let ok = [self.nominal, self.tol_upper, self.tol_lower, self.abs_min, self.abs_max, self.max_excursion_len].iter().all(|v| v.is_finite());
        if !ok || self.tol_upper < 0.0 || self.tol_lower < 0.0 || self.max_excursion_len < 0.0 {
            return Err(format!("{what}：限值必须是有限数，公差与允许超差长度不能为负"));
        }
        if !(self.abs_min <= self.nominal && self.nominal <= self.abs_max) {
            return Err(format!("{what}：名义值 {:.2} 要在绝对限 [{:.2}, {:.2}] 之内", self.nominal, self.abs_min, self.abs_max));
        }
        Ok(())
    }
}

/// 一个拍照点的判定限值。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotLimits {
    /// 胶条中线相对示教中线的横向偏移（mm）；为空时不判位置
    #[serde(default)]
    pub position: Option<JudgeParams>,
    /// 胶宽（mm）；为空时不判胶宽
    #[serde(default)]
    pub width: Option<JudgeParams>,
    /// 允许的连续缺胶长度（mm）
    pub max_gap_len: f32,
    /// 有胶站至少占这一段的比例（0–1）：短断口零散分布、每段都不超长时，靠它判断续胶
    #[serde(default = "default_min_present")]
    pub min_present: f32,
    /// 量具确认件编号：位置名义值不为 0 时必填（定稿 §3）。只是判定参数的出处，不进测量定义
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gauge_ref: Option<String>,
}

pub fn default_min_present() -> f32 {
    0.8
}

impl ShotLimits {
    fn validate(&self, what: &str) -> Result<(), String> {
        if let Some(p) = &self.position {
            p.validate(&format!("{what} 位置"))?;
            let gauge = self.gauge_ref.as_deref().map(str::trim)
                .is_some_and(|g| !g.is_empty() && g.chars().count() <= 64 && !g.chars().any(char::is_control));
            if p.nominal != 0.0 && !gauge {
                return Err(format!("{what}：位置名义值不为 0 时必须填写量具确认件编号（gaugeRef，最长 64 个字符）"));
            }
        }
        if let Some(w) = &self.width {
            w.validate(&format!("{what} 胶宽"))?;
        }
        if !(self.max_gap_len.is_finite() && self.max_gap_len >= 0.0) {
            return Err(format!("{what}：允许断胶长度不能为负"));
        }
        if !(0.0..=1.0).contains(&self.min_present) {
            return Err(format!("{what}：最低有胶比例需在 0–100% 之间"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Polarity {
    /// 胶条比背景暗
    Dark,
    Light,
}

/// 沿示教中线找胶的参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectParams {
    /// 卡尺沿法向的搜索半宽（mm）：要盖住机器人与工件带来的偏差
    pub search_mm: f32,
    pub polarity: Polarity,
    /// 胶宽的搜索范围（mm）：比它窄或宽的不当作胶
    pub width_range: [f32; 2],
}

impl DetectParams {
    fn validate(&self, what: &str) -> Result<(), String> {
        let [lo, hi] = self.width_range;
        if !(self.search_mm.is_finite() && self.search_mm > 0.0 && lo.is_finite() && hi.is_finite() && lo > 0.0 && lo < hi) {
            return Err(format!("{what}：搜索半宽需为正，胶宽范围需满足 0 < 下限 < 上限"));
        }
        if hi >= 2.0 * self.search_mm {
            return Err(format!("{what}：胶宽上限 {hi:.1} mm 要小于搜索宽度 {:.1} mm", 2.0 * self.search_mm));
        }
        Ok(())
    }
}

/// 一段：一个已示教、要检的拍照点。点 `first..first + count` 属于它，段内弧长 `(j - first) * spacing`。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub name: String,
    /// 拍照点（shots 下标）
    pub shot: usize,
    pub view: u8,
    pub first: usize,
    pub count: usize,
    pub position: Option<JudgeParams>,
    pub width: Option<JudgeParams>,
    pub max_gap_len: f32,
    #[serde(default = "default_min_present")]
    pub min_present: f32,
}

impl Segment {
    /// 段内弧长（mm）。
    pub fn s(&self, j: usize, spacing: f32) -> f32 {
        (j - self.first) as f32 * spacing
    }

    pub fn length(&self, spacing: f32) -> f32 {
        self.count.saturating_sub(1) as f32 * spacing
    }
}

/// 各站：所在拍照点图像里的像素位置、所属段与拍照点。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PathPoints {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub seg: Vec<u16>,
    pub k: Vec<u8>,
}

/// 旧文件里相机写的是相机组序号 k，读进来当作编号 "cam{k+1}"（相机组迁移时就是按位置这样编的号）。
pub fn legacy_camera_id(k: u8) -> String {
    format!("cam{}", k as u32 + 1)
}

/// 相机、配方编号：字母、数字、- 和 _，最长 32 个字符。
pub fn valid_camera_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_label(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= 32 && !s.chars().any(char::is_control)
}

/// 配方文件格式版本。不一致的文件列为加载错误，不迁移。8：增加段模型字段（segments、inspect、segmentDefaults、calibrationRounds、partCalib）。
pub const RECIPE_SCHEMA: u32 = 8;

/// 段配方连续采集几轮取均值作预期帧数（规格 §2：≥ 10 轮，轮数可配）。
pub const DEFAULT_CALIBRATION_ROUNDS: u32 = 10;
/// 标定轮数的可配范围（D-SB-36）。
pub const CALIBRATION_ROUNDS: std::ops::RangeInclusive<u32> = 10..=100;

fn default_calibration_rounds() -> u32 { DEFAULT_CALIBRATION_ROUNDS }

/// 定位模板外圈（glue.locate 的 templatePad，lyFlow#32）：模板文件是示教框四周各多裁这么多，
/// locate 用真实邻域预平滑后再裁回示教框。每个新模板都取 4（平滑核半径）。
pub const FIXTURE_TEMPLATE_PAD_PX: u32 = 4;
/// glue.locate 的角度扫描步长（°），只在开转角时起作用
pub const FIXTURE_ANGLE_STEP_DEG: f32 = 0.5;
/// 带外圈模板离胶检测走廊（示教中线 ± 搜索半宽）至少再留这么多（px）。阶段 0 评测（P0）选框时要求内框离走廊
/// 大于 10 px；外圈让框在角上最多近 4√2 px，所以 P0 选出的框到这里还剩约 10 − 4√2 ≈ 4.3 px（不计 P0 每 2 px
/// 取点的采样误差），取 4。
pub const FIXTURE_CORRIDOR_MARGIN_PX: f64 = 4.0;

fn one() -> f32 { 1.0 }

/// 冻结示教原图编号（模板来源）：字母、数字和 -，最长 128 个字符。工作台的冻结图编号都满足。
pub fn valid_image_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// 特征定位（定稿 §5）：示教框（内框，不含外圈）、模板来源与 glue.locate 门限。模板按 rect 加
/// FIXTURE_TEMPLATE_PAD_PX 外圈从来源冻结示教原图裁出；改 fixture 就是改测量定义，试测与验证都要重做。
///
/// 模板身份是 (source, rect, FIXTURE_TEMPLATE_PAD_PX)，不用内容哈希（D-0）。它成立的前提：冻结图按编号只写一次
/// （`image_file::save_pgm_new`）、发布包复制的是同一份字节、工作台与发布包都用 `template_pgm(decode(字节), rect)` 裁模板。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FixtureSpec {
    /// 示教框 [x, y, w, h]（示教图像素，不含外圈）；anchor 是它的左上角
    pub rect: [u32; 4],
    /// 模板来源：裁模板用的冻结示教原图编号。只由工作台后端写入（`bind_fixture_sources`），前端传来的一律不用
    #[serde(default)]
    pub source: String,
    /// 模板中心最多偏开示教位置多远（px）；峰离搜索窗边 ≤ 1 px 判 at_search_edge
    pub search_radius_px: f32,
    /// 角度扫描范围 ±（°）；v1 默认 0 = 只找平移（定稿 §10 决定二）
    #[serde(default)]
    pub angle_range_deg: f32,
    pub min_score: f32,
    /// 次峰 / 主峰上限；1 = 不判
    #[serde(default = "one")]
    pub max_peak_ratio: f32,
    /// 两端切向搬移 |Δ| 的上限（mm），超过判位置基准无效（ERR 92）；为空时用临时默认公式（见 end_shift_limit_mm）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_end_shift_mm: Option<f32>,
    /// 示教余量 m（mm，定稿 §10 决定一第 3 条）：离胶路首、末弧长不到 m 的站永远不判（超出检测范围、不留胶宽），
    /// 位姿搬移再从 N 端量起排除 max(m, |Δ|) + 1 个站距。只由验证建议写入（`workspace_apply_fixture_suggestion`），默认 0
    #[serde(default, skip_serializing_if = "is_zero")]
    pub end_margin_mm: f32,
}

fn is_zero(v: &f32) -> bool { *v == 0.0 }

impl FixtureSpec {
    /// 只核对配方本身能判的：右、下边要看图像尺寸，由工作台保存示教与生成模板时核对。
    fn validate(&self, what: &str) -> Result<(), String> {
        let [x, y, w, h] = self.rect;
        let pad = FIXTURE_TEMPLATE_PAD_PX;
        if x < pad || y < pad {
            return Err(format!("{what}：定位框离图像左、上边至少 {pad} px（模板要带 {pad} px 外圈）"));
        }
        if !(16..=512).contains(&w) || !(16..=512).contains(&h) {
            return Err(format!("{what}：定位框宽、高需在 16–512 px 之间"));
        }
        if !(self.search_radius_px.is_finite() && (3.0..=200.0).contains(&self.search_radius_px)) {
            return Err(format!("{what}：定位搜索半径需在 3–200 px 之间"));
        }
        if !(self.angle_range_deg.is_finite() && (0.0..=10.0).contains(&self.angle_range_deg)) {
            return Err(format!("{what}：定位角度范围需在 0–10° 之间"));
        }
        if !(self.min_score.is_finite() && self.min_score > 0.0 && self.min_score <= 1.0) {
            return Err(format!("{what}：定位最低分数需在 0–1 之间（不含 0）"));
        }
        if !(self.max_peak_ratio.is_finite() && self.max_peak_ratio > 0.0 && self.max_peak_ratio <= 1.0) {
            return Err(format!("{what}：定位次峰比上限需在 0–1 之间（不含 0）"));
        }
        if self.max_end_shift_mm.is_some_and(|m| !(m.is_finite() && m > 0.0)) {
            return Err(format!("{what}：两端搬移上限必须是正数"));
        }
        if !(self.end_margin_mm.is_finite() && (0.0..=100.0).contains(&self.end_margin_mm)) {
            return Err(format!("{what}：示教余量需在 0–100 mm 之间"));
        }
        if !valid_image_id(&self.source) {
            return Err(format!("{what}：缺少模板来源（示教图编号）。G2 时期的候选需要重建，或在示教页重新保存示教参数"));
        }
        Ok(())
    }

    /// 带外圈的模板放得进 size 大小的示教图：右、下边要看图像尺寸，配方校验查不了，
    /// 由工作台保存示教、发布与加载发布包时核对（lyFlow 在框离图边不到 templatePad 时判 out_of_image）。
    pub fn check_image_size(&self, size: [u32; 2]) -> Result<(), String> {
        crate::vision::fixture::padded_rect(self.rect, size[0], size[1]).map(|_| ()).ok_or_else(|| {
            format!("定位框加 {FIXTURE_TEMPLATE_PAD_PX} px 外圈超出示教原图（{}×{}），请把框从图边往里收", size[0], size[1])
        })
    }

    /// 带外圈模板（按像素中心，[x − pad, x + w + pad − 1] × [y − pad, y + h + pad − 1]）到示教折线及其两端切向延长线的
    /// 精确距离（px），相交为 0。
    ///
    /// 两端各沿末段方向延长到无穷远（对图内的框等价于延到图边，与图像尺寸无关）：枪上相机里同一条胶通常越过示教段两端
    /// 继续留在画面里，只量示教段（端点处是半圆帽）时，框放在胶的直线延长上也能通过，模板里却有胶。
    /// 延长段弯走的胶量不到；亮度 ±40 与 peakRatio 也看不见框里的胶，G4 只靠保存示教时的人工确认（定稿 §13）。
    fn clearance(&self, path: &[[f32; 2]]) -> f64 {
        let pad = f64::from(FIXTURE_TEMPLATE_PAD_PX);
        let [x, y, w, h] = self.rect.map(f64::from);
        let r = [x - pad, y - pad, x + w + pad - 1.0, y + h + pad - 1.0];
        let path: Vec<[f64; 2]> = path.iter().map(|p| [f64::from(p[0]), f64::from(p[1])]).collect();
        let segments = path.windows(2).map(|s| segment_rect_distance(s[0], s[1], r)).fold(f64::INFINITY, f64::min);
        // 端点沿“相邻的另一个不重合点 → 端点”的方向延长；FAR 远大于任何图像，射线截成线段不影响结果
        const FAR: f64 = 1.0e7;
        let ray = |end: [f64; 2], from: Option<&[f64; 2]>| from.map_or(f64::INFINITY, |p| {
            let (dx, dy) = (end[0] - p[0], end[1] - p[1]);
            let len = dx.hypot(dy);
            segment_rect_distance(end, [end[0] + dx / len * FAR, end[1] + dy / len * FAR], r)
        });
        let (Some(first), Some(last)) = (path.first(), path.last()) else { return segments; };
        let head = ray(*first, path.iter().find(|p| *p != first));
        let tail = ray(*last, path.iter().rev().find(|p| *p != last));
        segments.min(head).min(tail)
    }

    /// 模板不得含胶（定稿 §11 跨阶段约束）：带外圈模板离 `lines` 里每条示教中线（含两端切向延长线）都要大于“该线搜索半宽 + 余量”。
    /// 模板框盖住胶时位姿跟着胶走，d′ 恒约等于 0，真实胶偏会被放行。`lines`：（哪个拍照点的哪幅图，示教中线，搜索半宽 px）。
    /// 返回每条线的（哪幅图，实际距离，需要大于的距离）。
    fn corridor_clearances(&self, lines: &[(String, Vec<[f32; 2]>, f64)]) -> Vec<(String, f64, f64)> {
        lines.iter().map(|(owner, path, half)| (owner.clone(), self.clearance(path), half + FIXTURE_CORRIDOR_MARGIN_PX)).collect()
    }

    /// 示教框中心（px），与 glue.locate 的 teachCenter 一致：(x + (w − 1)/2, y + (h − 1)/2)。
    pub fn teach_center(&self) -> [f64; 2] {
        let [x, y, w, h] = self.rect.map(f64::from);
        [x + (w - 1.0) / 2.0, y + (h - 1.0) / 2.0]
    }

    /// 两端切向搬移 |Δ| 的上限（mm）。配方没给时取 locate 接受的位姿能产生的最大 |Δ|（定稿 §10 决定一的临时默认）：
    /// 方形搜索窗对角平移最多 √2·(searchRadius − 2) px，开转角时端点再绕模板中心切向移 L_端·2·sin(angleRange/2)，
    /// L_端 是模板中心到较远端点的距离。`ends_px` 是示教胶路首末站（px）。
    pub fn end_shift_limit_mm(&self, ends_px: [[f64; 2]; 2], mm_per_px: f64) -> f64 {
        if let Some(limit) = self.max_end_shift_mm { return limit as f64; }
        let c = self.teach_center();
        let lever = ends_px.iter().map(|p| (p[0] - c[0]).hypot(p[1] - c[1])).fold(0.0, f64::max) * mm_per_px;
        std::f64::consts::SQRT_2 * (self.search_radius_px as f64 - 2.0) * mm_per_px
            + lever * 2.0 * (self.angle_range_deg as f64 / 2.0).to_radians().sin()
    }
}

/// 定位框到同一场景一条胶检测走廊的余量（`RecipeDoc::fixture_clearances`）。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Clearance {
    /// 框所在的拍照点下标与图号
    pub k: usize,
    pub view: u8,
    /// 走廊属于哪个拍照点的哪幅图（如“拍照点 P2 图 1”）
    pub owner: String,
    /// 带外圈模板到示教中线（含两端切向延长）的距离（px）
    pub clearance: f64,
    /// 需要大于的距离：该线搜索半宽 + FIXTURE_CORRIDOR_MARGIN_PX
    pub need: f64,
}

impl Clearance {
    pub fn passed(&self) -> bool { self.clearance > self.need }

    pub fn message(&self) -> String {
        format!("定位框（含 {FIXTURE_TEMPLATE_PAD_PX} px 外圈）压住了{} 的胶检测走廊——离示教中线（含两端切向延长）{:.1} px，\
            需要大于 {:.1} px（搜索半宽 {:.1} px + 余量 {FIXTURE_CORRIDOR_MARGIN_PX} px）。模板里有胶时位姿跟着胶走、测不出胶偏，请把框移出走廊",
            self.owner, self.clearance, self.need, self.need - FIXTURE_CORRIDOR_MARGIN_PX)
    }
}

/// 线段 ab 到轴对齐矩形 r = [x0, y0, x1, y1] 的精确距离：相交（含端点在矩形内）为 0；否则最近点对一端在线段端点、
/// 或在矩形角上，取端点到矩形、矩形四角到线段的最小值。
fn segment_rect_distance(a: [f64; 2], b: [f64; 2], r: [f64; 4]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    // Liang–Barsky 裁剪：线段与矩形有交即距离 0
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let mut inside = true;
    for (p, q) in [(-d[0], a[0] - r[0]), (d[0], r[2] - a[0]), (-d[1], a[1] - r[1]), (d[1], r[3] - a[1])] {
        if p == 0.0 {
            if q < 0.0 { inside = false; break; }
        } else if p < 0.0 { t0 = t0.max(q / p); } else { t1 = t1.min(q / p); }
        if t0 > t1 { inside = false; break; }
    }
    if inside { return 0.0; }
    let to_rect = |p: [f64; 2]| (r[0] - p[0]).max(p[0] - r[2]).max(0.0).hypot((r[1] - p[1]).max(p[1] - r[3]).max(0.0));
    let len2 = d[0] * d[0] + d[1] * d[1];
    let to_segment = |p: [f64; 2]| {
        let t = if len2 > 0.0 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0) } else { 0.0 };
        (a[0] + d[0] * t - p[0]).hypot(a[1] + d[1] * t - p[1])
    };
    [to_rect(a), to_rect(b), to_segment([r[0], r[1]]), to_segment([r[2], r[1]]), to_segment([r[0], r[3]]), to_segment([r[2], r[3]])]
        .into_iter().fold(f64::INFINITY, f64::min)
}

/// 一台相机在这个配方下的曝光与增益：示教采集和生产布防前下发到相机。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CameraImaging {
    pub exposure_us: f32,
    pub gain_db: f32,
}

pub const DEFAULT_IMAGING: CameraImaging = CameraImaging { exposure_us: 60.0, gain_db: 6.0 };

impl Default for CameraImaging {
    fn default() -> Self { DEFAULT_IMAGING }
}

impl CameraImaging {
    pub fn validate(&self, camera: &str) -> Result<(), String> {
        if !(1.0..=1_000_000.0).contains(&self.exposure_us) {
            return Err(format!("相机 {camera} 的曝光时间需在 1–1000000 µs 之间"));
        }
        if !(self.gain_db.is_finite() && self.gain_db >= 0.0) {
            return Err(format!("相机 {camera} 的增益需为不小于 0 的有限数"));
        }
        Ok(())
    }
}

/// 段配方里一台检测相机：按直线段几何示教的一条走廊（图像像素，所有帧共用，只决定 ROI 和测站）与像素当量（D-SB-17）。
/// `RecipeDoc.inspect` 的顺序就是计划相机顺序（PLC 一个触发同时触发这三台）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CameraInspect {
    pub camera: String,
    /// 走廊中线（图像像素）；少于 2 点为尚未示教
    #[serde(default)]
    pub corridor: Vec<[f32; 2]>,
    /// 示教时的像素当量（mm/px）；为空为尚未示教
    #[serde(default)]
    pub mm_per_px: Option<f64>,
    /// 标定引用；为空时用这台相机的工位标定
    #[serde(default)]
    pub calib: Option<String>,
    /// 为空时用配方的检测参数
    #[serde(default)]
    pub detect: Option<DetectParams>,
}

impl CameraInspect {
    fn validate(&self) -> Result<(), String> {
        let id = &self.camera;
        if !valid_camera_id(id) {
            return Err(format!("检测相机编号 {id:?} 只能用字母、数字、- 和 _，最长 32 个字符"));
        }
        if self.corridor.iter().flatten().any(|v| !v.is_finite()) {
            return Err(format!("相机 {id} 的走廊坐标必须是有限数"));
        }
        if let Some(m) = self.mm_per_px.filter(|m| !(m.is_finite() && *m > 0.0 && *m <= 10.0)) {
            return Err(format!("相机 {id} 的像素当量 {m} 需在 0–10 mm/px 之间"));
        }
        if self.calib.as_deref().is_some_and(|c| !valid_camera_id(c)) {
            return Err(format!("相机 {id} 的标定引用只能用字母、数字、- 和 _"));
        }
        if let Some(d) = &self.detect {
            d.validate(&format!("相机 {id} 检测参数"))?;
        }
        Ok(())
    }

    /// 已示教：走廊至少 2 点、总长不短于 1 px，且有像素当量。没示教时返回缺的是什么。
    fn untaught(&self) -> Option<String> {
        let length: f32 = self.corridor.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
        let id = &self.camera;
        if self.corridor.len() < 2 || length < 1.0 {
            Some(format!("相机 {id} 尚未示教检测走廊"))
        } else if self.mm_per_px.is_none() {
            Some(format!("相机 {id} 缺少像素当量"))
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShotViewSpec {
    pub view: u8,
    pub enabled: bool,
    #[serde(default)]
    pub path: Vec<[f32; 2]>,
    pub mm_per_px: Option<f32>,
    pub detect: Option<DetectParams>,
    pub limits: Option<ShotLimits>,
    pub calib: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture: Option<FixtureSpec>,
}

/// 一个拍照点：机器人走到 Pose 时 PLC 触发这台相机拍一帧，在这帧里沿示教中线量胶。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotSpec {
    /// 配方内唯一，如 P1
    pub id: String,
    /// 现场机器人 / PLC 程序里的 Pose 标识；同一 Pose 可同时触发几台相机，不要求唯一
    pub pose_id: String,
    /// 相机编号
    pub camera: String,
    pub view: u8,
    #[serde(default)]
    pub views: Vec<ShotViewSpec>,
    /// 标定引用；为空时用这台相机的工位标定
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calib: Option<String>,
    /// 胶条名：同一条胶上的拍照点同名，结果按胶条汇总
    pub bead: String,
    /// 不检：要求这一帧到达，但不量不判（盘圈、大胶堆等示教不出的点）
    #[serde(default)]
    pub skip: bool,
    /// 示教的胶路中线（图像像素，从胶嘴一侧往外）；为空表示尚未示教
    #[serde(default)]
    pub path: Vec<[f32; 2]>,
    /// 示教时的像素当量（mm/px），把站距、搜索宽换成像素
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mm_per_px: Option<f32>,
    /// 为空时用配方的检测参数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detect: Option<DetectParams>,
    /// 为空时用配方的限值
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ShotLimits>,
    /// 特征定位：为空时胶偏按静态示教线量（d 相对示教中线）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture: Option<FixtureSpec>,
}

impl ShotSpec {
    pub fn enabled_views(&self) -> Vec<u8> {
        if self.skip { return Vec::new(); }
        if self.views.is_empty() { return vec![self.view]; }
        self.views.iter().filter(|v| v.enabled).map(|v| v.view).collect()
    }

    pub fn for_view(&self, view: u8) -> Result<Self, String> {
        let mut shot = self.clone();
        if let Some(v) = self.views.iter().find(|v| v.view == view) {
            shot.view = v.view;
            shot.path = v.path.clone();
            shot.mm_per_px = v.mm_per_px;
            shot.detect = v.detect.clone();
            shot.limits = v.limits.clone();
            shot.calib = v.calib.clone();
            shot.fixture = v.fixture.clone();
        } else if !self.views.is_empty() || view != self.view {
            return Err(format!("拍照点 {} 没有图 {view}", self.id));
        }
        shot.views.clear();
        Ok(shot)
    }
    /// 标定文件的键：标定引用，缺省为相机编号。
    pub fn calib_ref(&self) -> String {
        self.calib.clone().unwrap_or_else(|| format!("{}-v{}", self.camera, self.view))
    }

    /// 要量要判：不是"不检"。
    pub fn measured(&self) -> bool {
        !self.skip
    }

    /// 要检，且启用的视图里有用特征定位的（停用视图上残留的 fixture 不算）。
    pub fn has_fixture(&self) -> bool {
        self.measured() && self.enabled_views().into_iter().any(|v| self.for_view(v).is_ok_and(|s| s.fixture.is_some()))
    }

    /// 已示教：有中线和像素当量。
    pub fn taught(&self) -> bool {
        if self.views.is_empty() { return self.path.len() >= 2 && self.path_len_px() >= 1.0 && self.mm_per_px.is_some(); }
        let views = self.enabled_views();
        !views.is_empty() && views.into_iter().all(|v| self.for_view(v).is_ok_and(|s| s.taught()))
    }

    /// 中线总长（px）。
    pub fn path_len_px(&self) -> f32 {
        self.path.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum()
    }

    /// 中线上弧长 t（px）处的点。
    pub fn path_at(&self, t: f32) -> [f32; 2] {
        let mut left = t.max(0.0);
        for w in self.path.windows(2) {
            let l = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
            if left <= l && l > 0.0 {
                let f = left / l;
                return [w[0][0] + (w[1][0] - w[0][0]) * f, w[0][1] + (w[1][1] - w[0][1]) * f];
            }
            left -= l;
        }
        self.path.last().copied().unwrap_or([0.0, 0.0])
    }

    fn validate(&self) -> Result<(), String> {
        let mut selected = HashSet::new();
        for v in &self.views {
            if !(1..=3).contains(&v.view) || !selected.insert(v.view) {
                return Err(format!("拍照点 {} 的图编号必须在 1–3 内且不能重复", self.id));
            }
            self.for_view(v.view)?.validate()?;
        }
        let id = &self.id;
        if !valid_camera_id(id) {
            return Err(format!("拍照点编号 {id:?} 只能用字母、数字、- 和 _，最长 32 个字符"));
        }
        if !valid_label(&self.pose_id) {
            return Err(format!("拍照点 {id} 的 Pose 标识不能为空，最长 32 个字符"));
        }
        if !valid_label(&self.bead) {
            return Err(format!("拍照点 {id} 的胶条名不能为空，最长 32 个字符"));
        }
        if !valid_camera_id(&self.camera) {
            return Err(format!("拍照点 {id} 的相机编号只能用字母、数字、- 和 _"));
        }
        if !(1..=3).contains(&self.view) {
            return Err(format!("拍照点 {id} 的视角必须在 1–3 之间"));
        }
        if self.calib.as_deref().is_some_and(|c| !valid_camera_id(c)) {
            return Err(format!("拍照点 {id} 的标定引用只能用字母、数字、- 和 _"));
        }
        if self.path.iter().flatten().any(|v| !v.is_finite()) {
            return Err(format!("拍照点 {id} 的中线坐标必须是有限数"));
        }
        if let Some(m) = self.mm_per_px.filter(|m| !(m.is_finite() && *m > 0.0 && *m <= 10.0)) {
            return Err(format!("拍照点 {id} 的像素当量 {m} 需在 0–10 mm/px 之间"));
        }
        if let Some(d) = &self.detect {
            d.validate(&format!("拍照点 {id} 检测参数"))?;
        }
        if let Some(l) = &self.limits {
            l.validate(&format!("拍照点 {id}"))?;
        }
        if let Some(f) = &self.fixture {
            f.validate(&format!("拍照点 {id} 特征定位"))?;
        }
        Ok(())
    }
}

/// JSON（快照、界面 layout）里的 `segments` / `points` 是 build 生成的旧模型测量段与测点；反序列化不读它们，
/// 把 `pathSegments` 还原成 `RecipeDoc.segments` 后重新 build（见 `TryFrom<serde_json::Value>`）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "serde_json::Value")]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub version: u32,
    /// PLC 拍照计划版本（见 RecipeDoc::plan_version）
    pub plan_version: u32,
    pub revision_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teaching_id: Option<String>,
    pub product_code: u16,
    pub trigger_mode: TriggerMode,
    pub schema_version: u32,
    /// 站距（mm）
    pub spacing: f32,
    pub filter_window: usize,
    /// 拍照点没单独设时用的检测参数与限值
    pub detect: DetectParams,
    pub limits: ShotLimits,
    pub shots: Vec<ShotSpec>,
    #[serde(rename = "cameras")]
    pub camera_imaging: BTreeMap<String, CameraImaging>,
    /// 旧拍照点模型的测量段（段配方为空）
    pub segments: Vec<Segment>,
    /// 旧拍照点模型的测点（段配方为空）
    pub points: PathPoints,
    /// 段配方的段列表（RecipeDoc.segments）；为空即旧拍照点配方
    pub path_segments: Vec<PathSegment>,
    /// 段配方的检测相机，顺序即计划相机顺序
    pub inspect: Vec<CameraInspect>,
    pub segment_defaults: KindDefaults,
    pub calibration_rounds: u32,
    pub part_calib: Option<PartCalib>,
}

impl Recipe {
    /// 段配方（分段连拍）：段列表非空。否则是旧拍照点配方。
    pub fn is_burst(&self) -> bool {
        !self.path_segments.is_empty()
    }

    /// 段配方的计划相机（按 inspect 顺序）：PLC 一个触发同时触发它们。旧拍照点配方为空。
    pub fn plan_cameras(&self) -> Vec<String> {
        self.inspect.iter().map(|i| i.camera.clone()).collect()
    }

    /// 整件预期帧组数 ΣN_exp（各段都已标定取其和，否则取整件标定）；未标定为 None。
    pub fn expected_total(&self) -> Option<u32> {
        plan::expected_total(&self.path_segments, self.part_calib.as_ref())
    }

    /// 整件帧数容差 Σtolerance。
    pub fn tolerance_total(&self) -> u32 {
        plan::tolerance_total(&self.path_segments)
    }

    /// 段配方能不能发布生产（D-SB-20）：三台相机都示教了走廊和当量，且整件预期帧组数已标定
    /// （各段 N_exp 可以为空，凭整件标定发布，决策补充第二节）。不能时逐条列出原因。
    /// `Recipe::ready` 保留给旧拍照点配方，段配方过不了它，旧发布与旧节拍因此不会接受段配方。
    #[cfg_attr(not(test), allow(dead_code))] // D3 发布前检查接入后去掉
    pub fn ready_burst(&self) -> Result<(), Vec<String>> {
        let mut reasons: Vec<String> = Vec::new();
        if !self.is_burst() {
            reasons.push(format!("配方 {} 没有段，是旧拍照点配方", self.id));
        }
        reasons.extend(self.inspect.iter().filter_map(CameraInspect::untaught));
        if self.expected_total().is_none() {
            reasons.extend(self.path_segments.iter().filter(|s| s.expected_frames.is_none()).map(|s| {
                let kind = match s.kind { SegmentKind::Straight => "直线段", SegmentKind::Corner => "弯道段" };
                format!("第 {} 段（{kind}）待标定：预期帧数要由多轮采集得出", s.index)
            }));
        }
        if reasons.is_empty() { Ok(()) } else { Err(reasons) }
    }

    pub(crate) fn candidate_revision(&self) -> Option<u64> {
        let suffix = self.revision_id.strip_prefix(&format!("candidate-{}-", self.id))?;
        let revision = if let Some(legacy) = suffix.strip_prefix('r') { legacy } else {
            let (instance, revision) = suffix.strip_prefix('i')?.split_once("-r")?;
            let parsed = instance.parse::<u64>().ok()?;
            if parsed == 0 || instance != parsed.to_string() { return None; }
            revision
        };
        revision.parse::<u64>().ok().filter(|value| *value > 0 && revision == value.to_string())
    }

    pub(crate) fn from_snapshot(value: serde_json::Value) -> Result<Self, String> {
        let revision = value.get("revisionId").and_then(serde_json::Value::as_str).ok_or("配方快照缺少修订号")?.to_owned();
        let mut recipe: Self = serde_json::from_value(value).map_err(|error| error.to_string())?;
        let production_revision = recipe.revision_id.clone();
        recipe.revision_id = revision;
        if recipe.revision_id != production_revision && recipe.candidate_revision().is_none() {
            return Err("配方快照修订号与身份不一致".into());
        }
        Ok(recipe)
    }
    pub fn imaging(&self, camera: &str) -> Option<CameraImaging> {
        self.camera_imaging.get(camera).copied()
    }

    pub fn shot_count(&self) -> usize {
        self.shots.len()
    }

    pub fn point_count(&self) -> usize {
        self.points.k.len()
    }

    pub fn owned_points(&self, k: usize) -> impl Iterator<Item = usize> + '_ {
        self.points.k.iter().enumerate().filter(move |(_, &o)| o as usize == k).map(|(j, _)| j)
    }

    pub fn for_view(&self, k: usize, view: u8) -> Result<Self, String> {
        let selected = self.shots.get(k).ok_or("拍照点不存在")?.for_view(view)?;
        let mut recipe = self.clone();
        for (index, shot) in recipe.shots.iter_mut().enumerate() { if index != k { shot.skip = true; } }
        recipe.shots[k] = selected;
        recipe.segments.retain(|s| s.shot == k && s.view == view);
        Ok(recipe)
    }

    pub fn view_points(&self, k: usize, view: u8) -> impl Iterator<Item = usize> + '_ {
        self.segments.iter().filter(move |s| s.shot == k && s.view == view)
            .flat_map(|s| s.first..s.first + s.count)
    }

    /// 拍照点 k 的检测参数。
    pub fn shot_detect(&self, k: usize) -> &DetectParams {
        self.shots[k].detect.as_ref().unwrap_or(&self.detect)
    }

    /// 拍照点 k 的段（不检、未示教的拍照点没有）。
    pub fn shot_segment(&self, k: usize) -> Option<&Segment> {
        self.segments.iter().find(|g| g.shot == k)
    }

    /// 能不能开工：要检的拍照点都示教过。
    pub fn ready(&self) -> Result<(), String> {
        if !self.shots.iter().any(|s| s.measured() && !s.enabled_views().is_empty()) {
            return Err("至少需要一个参与检测的拍照点".into());
        }
        let untaught: Vec<&str> = self.shots.iter().enumerate().filter(|(k, s)| s.measured() && (!s.taught() || s.enabled_views().into_iter().any(|v| self.view_points(*k, v).count() < 3))).map(|(_, s)| s.id.as_str()).collect();
        if untaught.is_empty() {
            Ok(())
        } else {
            Err(format!("配方 {} 的拍照点 {} 尚未示教胶路", self.id, untaught.join("、")))
        }
    }

    /// 本配方要用到的相机（编号）：旧拍照点配方按第一次出现的拍照点排序；段配方是计划相机。
    pub fn cameras(&self) -> Vec<String> {
        if self.is_burst() { return self.plan_cameras(); }
        let mut out: Vec<String> = Vec::new();
        for s in &self.shots {
            if !out.contains(&s.camera) {
                out.push(s.camera.clone());
            }
        }
        out
    }
}

/// 配方文件的内容，也是配方页编辑的对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeDoc {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: u32,
    /// PLC 拍照计划版本：由配方库分配、全局唯一，只在计划身份变了时换新号（见 `same_plan`）：旧拍照点配方是产品代码或
    /// 拍照点的顺序、编号、Pose、相机；段配方是产品代码、各段序号 / 类型 / N_exp、相机顺序与 ΣN_exp。
    /// 只改限值、检测参数、示教中线、容差、x_ref 或"不检"时不变，PLC 侧不用跟着改。0 表示还没分配（不能布防）。
    #[serde(default)]
    pub plan_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teaching_id: Option<String>,
    pub product_code: u16,
    /// 段配方固定为 Fly（D-SB-23）；Stop（停稳拍）只供旧拍照点配方
    pub trigger_mode: TriggerMode,
    /// 缺省为 0，校验时报格式版本不符
    #[serde(default)]
    pub schema_version: u32,
    pub spacing: f32,
    pub filter_window: usize,
    pub detect: DetectParams,
    pub limits: ShotLimits,
    /// 旧拍照点模型的拍照点；段配方必须为空
    #[serde(default)]
    pub shots: Vec<ShotSpec>,
    /// 各相机的曝光与增益：拍照点与检测相机用到的相机各一组，不多不少
    #[serde(default)]
    pub cameras: BTreeMap<String, CameraImaging>,
    /// 沿胶路的段列表（规格 §1）；为空即旧拍照点配方，下面几项段模型字段不允许设置
    #[serde(default)]
    pub segments: Vec<PathSegment>,
    /// 三台检测相机（有序，计划相机顺序取它）
    #[serde(default)]
    pub inspect: Vec<CameraInspect>,
    /// 新建段时取用的直线 / 弯道默认容差与检测项
    #[serde(default)]
    pub segment_defaults: KindDefaults,
    /// 标定轮数，10..=100
    #[serde(default = "default_calibration_rounds")]
    pub calibration_rounds: u32,
    /// 整件帧组数标定：各段 N_exp 待分段算法时凭它得到 expected_total 并发布（决策补充第二节）；只由标定写入
    #[serde(default)]
    pub part_calib: Option<PartCalib>,
}

impl RecipeDoc {
    pub fn shot_cameras(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in &self.shots {
            if !out.contains(&s.camera) { out.push(s.camera.clone()); }
        }
        out
    }

    /// 段配方：段列表非空。
    pub fn is_burst(&self) -> bool {
        !self.segments.is_empty()
    }

    /// 段配方的计划相机（按 inspect 顺序）。
    pub fn plan_cameras(&self) -> Vec<String> {
        self.inspect.iter().map(|i| i.camera.clone()).collect()
    }

    /// 要成像参数的相机：拍照点用到的相机与检测相机的并集（拍照点在前）。
    fn imaging_cameras(&self) -> Vec<String> {
        let mut out = self.shot_cameras();
        for camera in self.plan_cameras() {
            if !out.contains(&camera) { out.push(camera); }
        }
        out
    }

    /// 整件预期帧组数 ΣN_exp（见 `plan::expected_total`）。
    pub fn expected_total(&self) -> Option<u32> {
        plan::expected_total(&self.segments, self.part_calib.as_ref())
    }

    /// 段计划身份（`plan::plan_identity`）；旧拍照点配方没有段和检测相机，身份恒等。
    pub fn plan_identity(&self) -> plan::PlanIdentity {
        plan::plan_identity(self.product_code, &self.segments, &self.plan_cameras(), self.expected_total())
    }

    /// 拍照点与检测相机新用到的相机补默认成像参数，不再用到的相机去掉，返回去掉的那些。
    pub fn sync_imaging(&mut self) -> BTreeMap<String, CameraImaging> {
        let used = self.imaging_cameras();
        let (kept, dropped): (BTreeMap<_, _>, BTreeMap<_, _>) = std::mem::take(&mut self.cameras).into_iter().partition(|(c, _)| used.contains(c));
        self.cameras = kept;
        for c in used { self.cameras.entry(c).or_default(); }
        dropped
    }

    /// PLC 侧要照着走的那部分：两份配方这几项逐项相同，就是同一份拍照计划（逐字段比较，不算摘要）。
    /// 旧拍照点身份与段计划身份都相等才算同一计划（D-SB-19）：旧配方段身份恒等，段配方没有拍照点、旧身份恒等。
    pub fn same_plan(&self, other: &RecipeDoc) -> bool {
        self.product_code == other.product_code
            && self.shots.len() == other.shots.len()
            && self.shots.iter().zip(&other.shots).all(|(a, b)| a.id == b.id && a.pose_id == b.pose_id && a.camera == b.camera)
            && self.plan_identity() == other.plan_identity()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != RECIPE_SCHEMA {
            return Err(format!("配方文件格式版本 {}，当前为 {RECIPE_SCHEMA}，需要按新格式重建", self.schema_version));
        }
        if !valid_camera_id(&self.id) {
            return Err("配方编号只能用字母、数字、- 和 _，最长 32 个字符".into());
        }
        if self.name.trim().is_empty() {
            return Err("配方名称不能为空".into());
        }
        if !(0.1..=10.0).contains(&self.spacing) {
            return Err("站距需在 0.1–10 mm 之间".into());
        }
        if self.filter_window == 0 || self.filter_window > 31 || self.filter_window % 2 == 0 {
            return Err("中值滤波窗口需为 1–31 的奇数".into());
        }
        self.detect.validate("检测参数")?;
        self.limits.validate("限值")?;
        if self.is_burst() {
            self.validate_burst()?;
        } else if !self.inspect.is_empty() || self.part_calib.is_some() {
            return Err("没有段的配方是旧拍照点配方，不能配置检测相机（inspect）或整件标定（partCalib）".into());
        }
        if self.shots.len() > 64 {
            return Err("配方最多允许 64 个拍照点".into());
        }
        let mut ids = HashSet::new();
        for shot in &self.shots {
            shot.validate()?;
            if !ids.insert(shot.id.as_str()) {
                return Err(format!("拍照点编号 {} 重复", shot.id));
            }
            // 用了特征定位却不判位置：定位失败、两端搬移超限都报不出 ERR，两端还会按位姿悄悄排除断胶判定
            for v in shot.enabled_views() {
                let projected = shot.for_view(v)?;
                if projected.fixture.is_some() && projected.limits.as_ref().unwrap_or(&self.limits).position.is_none() {
                    return Err(format!("拍照点 {} 图 {v} 使用了特征定位，必须设置位置判定限值", shot.id));
                }
            }
        }
        self.check_fixture_corridors()?;
        let used = self.imaging_cameras();
        let user = if self.is_burst() { "检测相机" } else { "拍照点" };
        if let Some(missing) = used.iter().find(|c| !self.cameras.contains_key(*c)) {
            return Err(format!("相机 {missing} 被{user}使用，但配方没有它的曝光与增益"));
        }
        for (camera, imaging) in &self.cameras {
            if !used.contains(camera) {
                return Err(format!("配方给了相机 {camera} 的曝光与增益，但没有{user}使用它"));
            }
            imaging.validate(camera)?;
        }
        Ok(())
    }

    /// 段配方的模型约束（D-SB-1）：不能再有拍照点；只支持飞拍；标定轮数 10..=100；检测相机各自合法；
    /// 段列表与三台计划相机满足 `plan::validate_segments`；直线 / 弯道默认参数满足 `plan::validate_defaults`（新建段取用它）；
    /// 整件标定的帧组数放得进 PLC shotCount。
    fn validate_burst(&self) -> Result<(), String> {
        if !self.shots.is_empty() {
            return Err("段配方不能同时有拍照点：segments 非空时 shots 必须为空，两套模型不能混用".into());
        }
        if self.trigger_mode != TriggerMode::Fly {
            return Err("段配方只支持飞拍（triggerMode 必须为 fly）".into());
        }
        if !CALIBRATION_ROUNDS.contains(&self.calibration_rounds) {
            return Err(format!("标定轮数 {} 需在 {}–{} 之间", self.calibration_rounds, CALIBRATION_ROUNDS.start(), CALIBRATION_ROUNDS.end()));
        }
        for inspect in &self.inspect {
            inspect.validate()?;
        }
        plan::validate_segments(&self.segments, &self.plan_cameras())?;
        plan::validate_defaults(&self.segment_defaults)?;
        if let Some(part) = &self.part_calib {
            if !(1..=plan::MAX_EXPECTED_TOTAL).contains(&part.expected_total) {
                return Err(format!("整件标定的预期帧组数 {} 需在 1–{} 之间（PLC shotCount）", part.expected_total, plan::MAX_EXPECTED_TOTAL));
            }
        }
        Ok(())
    }

    /// 模板与胶检测走廊不重叠（替代 G2 的“含特征定位不能发布”，定稿 §11 跨阶段约束），只查与图像无关的部分；
    /// 带外圈模板是否在图内要看图像尺寸，由工作台保存示教、发布与加载发布包时查（`FixtureSpec::check_image_size`）。
    ///
    /// 同一 Pose、同一相机、同一幅图的几个拍照点拍的是同一场景、同一坐标系的不同帧（同一 Pose 可多次触发）：
    /// 拍照点 A 的模板可能盖住拍照点 B 的胶，所以每个框要避开组内全部已示教中线的走廊。
    /// 停用视图、“不检”拍照点上的 fixture 不查（与 `has_fixture` 同口径），它们的中线也不算走廊。
    fn check_fixture_corridors(&self) -> Result<(), String> {
        match self.fixture_clearances()?.into_iter().find(|c| !c.passed()) {
            Some(c) => Err(format!("拍照点 {} 图 {} 特征定位：{}", self.shots[c.k].id, c.view, c.message())),
            None => Ok(()),
        }
    }

    /// 每个启用的定位框到同组（同 Pose、同相机、同一幅图）每条已示教中线走廊的余量，见 `check_fixture_corridors`。
    /// 配方校验与示教检查（G4 的 `workspace_fixture_check`、试测报告）共用这一份几何。
    pub(crate) fn fixture_clearances(&self) -> Result<Vec<Clearance>, String> {
        type Group = (Vec<(String, Vec<[f32; 2]>, f64)>, Vec<(usize, u8, FixtureSpec)>);
        let mut groups: BTreeMap<(&str, &str, u8), Group> = BTreeMap::new();
        for (k, shot) in self.shots.iter().enumerate().filter(|(_, s)| s.measured()) {
            for v in shot.enabled_views() {
                let projected = shot.for_view(v)?;
                let label = format!("拍照点 {} 图 {v}", shot.id);
                let group = groups.entry((shot.pose_id.as_str(), shot.camera.as_str(), v)).or_default();
                if let (true, Some(mm)) = (projected.path.len() >= 2, projected.mm_per_px) {
                    let detect = projected.detect.as_ref().unwrap_or(&self.detect);
                    group.0.push((label, projected.path.clone(), f64::from(detect.search_mm) / f64::from(mm)));
                }
                if let Some(f) = projected.fixture { group.1.push((k, v, f)); }
            }
        }
        let mut out = Vec::new();
        for (lines, fixtures) in groups.values() {
            for (k, view, f) in fixtures {
                out.extend(f.corridor_clearances(lines).into_iter().map(|(owner, clearance, need)| Clearance { k: *k, view: *view, owner, clearance, need }));
            }
        }
        Ok(out)
    }

    pub fn build(&self) -> Result<Recipe, String> {
        self.validate()?;
        let mut points = PathPoints::default();
        let mut segments = Vec::new();
        for (k, shot) in self.shots.iter().enumerate() {
            for view in shot.enabled_views() {
            let shot = shot.for_view(view)?;
            if !shot.taught() { continue; }
            let step = self.spacing / shot.mm_per_px.unwrap();
            let len = shot.path_len_px();
            let count = (len / step + 1e-3).floor() as usize + 1;
            if count < 3 { continue; }
            if points.k.len() + count > 200_000 {
                return Err("测量点超过 20 万个，加大站距".into());
            }
            let first = points.k.len();
            for i in 0..count {
                let [x, y] = shot.path_at(i as f32 * step);
                points.x.push(x);
                points.y.push(y);
                points.seg.push(segments.len() as u16);
                points.k.push(k as u8);
            }
            let limits = shot.limits.as_ref().unwrap_or(&self.limits);
            segments.push(Segment {
                name: if self.shots[k].views.is_empty() { format!("{} · {}", shot.id, shot.bead) } else { format!("{} · {} · 图 {view}", shot.id, shot.bead) },
                shot: k,
                view,
                first,
                count,
                position: limits.position.clone(),
                width: limits.width.clone(),
                max_gap_len: limits.max_gap_len,
                min_present: limits.min_present,
            });
            }
        }
        let recipe = Recipe {
            id: self.id.clone(),
            name: self.name.trim().to_string(),
            version: self.version.max(1),
            plan_version: self.plan_version,
            revision_id: format!("{}-v{}", self.id, self.version.max(1)),
            teaching_id: self.teaching_id.clone(),
            product_code: self.product_code,
            trigger_mode: self.trigger_mode,
            schema_version: self.schema_version,
            spacing: self.spacing,
            filter_window: self.filter_window,
            detect: self.detect.clone(),
            limits: self.limits.clone(),
            shots: self.shots.clone(),
            camera_imaging: self.cameras.clone(),
            segments,
            points,
            path_segments: self.segments.clone(),
            inspect: self.inspect.clone(),
            segment_defaults: self.segment_defaults.clone(),
            calibration_rounds: self.calibration_rounds,
            part_calib: self.part_calib.clone(),
        };
        Ok(recipe)
    }
}

impl TryFrom<RecipeDoc> for Recipe {
    type Error = String;
    fn try_from(doc: RecipeDoc) -> Result<Self, Self::Error> { doc.build() }
}

impl TryFrom<serde_json::Value> for Recipe {
    type Error = String;
    /// Recipe 的 JSON 还原成 RecipeDoc 再 build：`segments` / `points` 是 build 生成的旧模型测量段与测点（不读），
    /// 段配方的段列表在 `pathSegments`（与 RecipeDoc 的 `segments` 同名不同义，不能直接按 RecipeDoc 解析）。
    fn try_from(mut value: serde_json::Value) -> Result<Self, Self::Error> {
        let object = value.as_object_mut().ok_or("配方快照不是 JSON 对象")?;
        object.remove("points");
        let path_segments = object.remove("pathSegments").unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
        object.insert("segments".into(), path_segments);
        serde_json::from_value::<RecipeDoc>(value).map_err(|e| e.to_string())?.build()
    }
}

/// 胶宽、位置、断胶的默认限值：名义胶宽 4 mm；断口阈值取自 MX11 现场图（正常帧最长无胶 5.4 mm，断胶帧 10–22 mm）。
pub fn default_limits() -> ShotLimits {
    ShotLimits {
        position: Some(JudgeParams { nominal: 0.0, tol_upper: 2.0, tol_lower: 2.0, abs_min: -5.0, abs_max: 5.0, max_excursion_len: 5.0 }),
        width: Some(JudgeParams { nominal: 4.0, tol_upper: 1.5, tol_lower: 1.5, abs_min: 1.0, abs_max: 8.0, max_excursion_len: 5.0 }),
        max_gap_len: 6.0,
        min_present: default_min_present(),
        gauge_ref: None,
    }
}

pub fn default_detect() -> DetectParams {
    DetectParams { search_mm: 15.0, polarity: Polarity::Dark, width_range: [1.0, 10.0] }
}

/// 模拟相机的像素当量（与模拟出图一致）。
pub const SIM_MM_PER_PX: f32 = 0.112;

/// 一台相机按顺序拍几个拍照点：编号 P1、P2…，Pose 同编号，胶条 J1；中线由调用方给（空为未示教）。
pub fn shot_list(camera: &str, paths: Vec<Vec<[f32; 2]>>) -> Vec<ShotSpec> {
    paths
        .into_iter()
        .enumerate()
        .map(|(k, path)| ShotSpec {
            id: format!("P{}", k + 1),
            pose_id: format!("P{}", k + 1),
            camera: camera.into(),
            view: 1,
            views: Vec::new(),
            calib: None,
            bead: "J1".into(),
            skip: false,
            mm_per_px: (!path.is_empty()).then_some(SIM_MM_PER_PX),
            path,
            detect: None,
            limits: None,
            fixture: None,
        })
        .collect()
}

/// 模拟相机画面里的示例中线：胶嘴在右侧，胶条往左拖出（直、缓弯、斜向交替）。
fn sample_path(k: usize) -> Vec<[f32; 2]> {
    match k % 3 {
        0 => vec![[980.0, 480.0], [300.0, 460.0]],
        1 => vec![[980.0, 480.0], [700.0, 420.0], [320.0, 330.0]],
        _ => vec![[980.0, 480.0], [640.0, 560.0], [330.0, 690.0]],
    }
}

/// 首次启动写入的样例配方：模拟相机按示例中线就能跑；换真实相机要重新示教。
pub fn samples() -> Vec<RecipeDoc> {
    let doc = |id: &str, name: &str, code: u16, n: usize, trigger_mode| { let mut doc = RecipeDoc {
        id: id.into(),
        name: name.into(),
        version: 1,
        plan_version: u32::from(code),
        teaching_id: None,
        product_code: code,
        trigger_mode,
        schema_version: RECIPE_SCHEMA,
        spacing: 1.0,
        filter_window: 5,
        detect: default_detect(),
        limits: default_limits(),
        shots: shot_list(&legacy_camera_id(0), (0..n).map(sample_path).collect()),
        cameras: BTreeMap::new(),
        segments: Vec::new(),
        inspect: Vec::new(),
        segment_defaults: KindDefaults::default(),
        calibration_rounds: DEFAULT_CALIBRATION_ROUNDS,
        part_calib: None,
    }; doc.sync_imaging(); doc };
    vec![doc("MTR-HSG-A", "电机壳体 A", 12, 6, TriggerMode::Fly), doc("MTR-HSG-B", "电机壳体 B", 13, 4, TriggerMode::Stop)]
}

/// 段配方的起点（D-SB-24）：飞拍、一个直线段（待标定，容差与检测项取配方默认）、按给定顺序的检测相机（走廊待示教）、
/// 计划版本未分配。相机不足 3 台时照样返回，但过不了段配方校验。
pub fn burst_template(id: String, name: String, product_code: u16, cameras: &[String]) -> RecipeDoc {
    let segment_defaults = KindDefaults::default();
    let mut doc = RecipeDoc {
        id,
        name,
        version: 1,
        plan_version: 0,
        teaching_id: None,
        product_code,
        trigger_mode: TriggerMode::Fly,
        schema_version: RECIPE_SCHEMA,
        spacing: 1.0,
        filter_window: 5,
        detect: default_detect(),
        limits: default_limits(),
        shots: Vec::new(),
        cameras: BTreeMap::new(),
        segments: vec![plan::new_segment(1, SegmentKind::Straight, &segment_defaults)],
        inspect: cameras.iter().map(|camera| CameraInspect { camera: camera.clone(), corridor: Vec::new(), mm_per_px: None, calib: None, detect: None }).collect(),
        segment_defaults,
        calibration_rounds: DEFAULT_CALIBRATION_ROUNDS,
        part_calib: None,
    };
    doc.sync_imaging();
    doc
}

/// 测试用的段配方：模拟相机 cam1–cam3 共用触发、走廊已示教、一个直线段 N_exp = 4（ΣN_exp = 4），计划版本已分配。
#[cfg(test)]
pub fn samples_burst() -> RecipeDoc {
    let cameras: Vec<String> = (0..3).map(legacy_camera_id).collect();
    let mut doc = burst_template("SEAM-BURST".into(), "分段连拍样例".into(), 14, &cameras);
    doc.plan_version = 14;
    for inspect in &mut doc.inspect {
        inspect.corridor = sample_path(0);
        inspect.mm_per_px = Some(f64::from(SIM_MM_PER_PX));
    }
    doc.segments[0].expected_frames = Some(4);
    doc
}

/// 测试与重判里需要一份现成配方时用。
#[cfg(test)]
pub fn builtin() -> Vec<Arc<Recipe>> {
    samples().iter().map(|d| Arc::new(d.build().unwrap())).collect()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnavailableRecipe {
    pub file_name: String,
    pub id: Option<String>,
    pub name: Option<String>,
    pub schema_version: Option<u32>,
    pub current_schema_version: u32,
    pub reason: String,
}

impl UnavailableRecipe {
    fn from_file(path: &std::path::Path, reason: String) -> Option<Self> {
        let text = RecipeStore::read_regular_file(path).ok()?;
        if serde_json::from_str::<RecipeDoc>(&text).ok().is_some_and(|doc| doc.build().is_ok()) {
            return None;
        }
        let value = serde_json::from_str::<serde_json::Value>(&text).ok();
        Some(Self {
            file_name: path.file_name().and_then(|name| name.to_str()).unwrap_or_default().into(),
            id: value.as_ref().and_then(|v| v.get("id")?.as_str()).map(str::to_owned),
            name: value.as_ref().and_then(|v| v.get("name")?.as_str()).map(str::to_owned),
            schema_version: value.as_ref().and_then(|v| v.get("schemaVersion")?.as_u64()).and_then(|v| u32::try_from(v).ok()),
            current_schema_version: RECIPE_SCHEMA,
            reason,
        })
    }
}

/// 磁盘上的配方库：每个配方一个 `<id>.json`。
pub struct RecipeStore {
    dir: PathBuf,
    inner: RwLock<Vec<(RecipeDoc, Arc<Recipe>)>>,
    errors: RwLock<Vec<String>>,
    unavailable: RwLock<Vec<UnavailableRecipe>>,
}

impl RecipeStore {
    #[cfg(test)]
    pub fn open(dir: PathBuf) -> Result<Self, String> {
        Self::open_with_floors(dir, &BTreeMap::new())
    }

    pub fn open_with_floors(dir: PathBuf, floors: &BTreeMap<String, u32>) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建配方目录失败：{e}"))?;
        let store = Self { dir, inner: RwLock::new(Vec::new()), errors: RwLock::new(Vec::new()), unavailable: RwLock::new(Vec::new()) };
        store.seed_version_floor(floors)?;
        store.retire_outdated_sources()?;
        let retired = store.retired_versions()?;
        let sources = store.revision_sources()?;
        let empty = retired.is_empty() && sources.is_empty() && std::fs::read_dir(&store.dir).map_err(|e| e.to_string())?.flatten().all(|e| e.path().extension().is_none_or(|x| x != "json"));
        if empty {
            for doc in samples() {
                store.write(&doc)?;
            }
        }
        store.reload();
        Ok(store)
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn retired_versions(&self) -> Result<BTreeMap<String, u32>, String> {
        match crate::fsio::read_text(&self.dir.join(".revision-versions")) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("配方版本记录损坏，拒绝复用修订号：{e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(format!("读取配方版本记录失败：{e}")),
        }
    }

    fn revision_sources(&self) -> Result<BTreeMap<String, RecipeDoc>, String> {
        let sources: BTreeMap<String, RecipeDoc> = match crate::fsio::read_text(&self.dir.join(".revision-sources")) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("配方修订来源记录损坏，拒绝复用修订号：{e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("读取配方修订来源记录失败：{e}")),
        };
        for (id, doc) in &sources {
            if id != &doc.id.to_ascii_lowercase() || doc.version == 0 {
                return Err("配方修订来源记录身份不一致，拒绝复用修订号".into());
            }
            doc.build().map_err(|e| format!("配方修订来源记录无效：{e}"))?;
        }
        Ok(sources)
    }

    /// 旧格式的修订来源无法再证明配方内容（D-0）：原文件备份后移除，其修订号与计划版本并入版本记录，仍不复用。
    fn retire_outdated_sources(&self) -> Result<(), String> {
        let path = self.dir.join(".revision-sources");
        let text = match crate::fsio::read_text(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("读取配方修订来源记录失败：{e}")),
        };
        let mut sources: BTreeMap<String, RecipeDoc> = serde_json::from_str(&text).map_err(|e| format!("配方修订来源记录损坏，拒绝复用修订号：{e}"))?;
        let outdated: Vec<RecipeDoc> = sources.values().filter(|d| d.schema_version != RECIPE_SCHEMA).cloned().collect();
        if outdated.is_empty() { return Ok(()); }
        std::fs::copy(&path, self.dir.join(format!(".revision-sources.outdated-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"))))
            .map_err(|e| format!("备份旧格式配方修订来源记录失败：{e}"))?;
        let plan = outdated.iter().map(|d| d.plan_version).max().unwrap_or(0).max(self.issued_plan_version()?);
        crate::fsio::write_atomic(&self.dir.join(".plan-version"), &plan.to_string())?;
        for doc in &outdated {
            self.retire_version(&doc.id, doc.version)?;
            sources.remove(&doc.id.to_ascii_lowercase());
        }
        crate::fsio::write_atomic(&path, &serde_json::to_string(&sources).map_err(|e| e.to_string())?)
    }

    fn remember_source(&self, doc: &RecipeDoc) -> Result<(), String> {
        let mut sources = self.revision_sources()?;
        let id = doc.id.to_ascii_lowercase();
        if let Some(previous) = sources.get(&id) {
            if previous == doc { return Ok(()); }
            if previous.version >= doc.version { return Err("同一修订号不能记录不同的实际配方来源".into()); }
        }
        sources.insert(id, doc.clone());
        crate::fsio::write_atomic(&self.dir.join(".revision-sources"), &serde_json::to_string(&sources).map_err(|e| e.to_string())?)
    }

    fn retire_version(&self, id: &str, version: u32) -> Result<(), String> {
        let mut versions = self.retired_versions()?;
        let recorded = versions.entry(id.to_ascii_lowercase()).or_default();
        *recorded = (*recorded).max(version);
        crate::fsio::write_atomic(&self.dir.join(".revision-versions"), &serde_json::to_string(&versions).map_err(|e| e.to_string())?)
    }

    /// 已发出的最大计划版本。文件丢了就从现有配方里取最大值，不回退到已用过的号。
    fn issued_plan_version(&self) -> Result<u32, String> {
        match crate::fsio::read_text(&self.dir.join(".plan-version")) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("计划版本记录损坏，拒绝分配计划版本：{e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(format!("读取计划版本记录失败：{e}")),
        }
    }

    /// 发一个新的计划版本号：比已发出的、现有配方和修订来源里用过的都大。
    fn issue_plan_version(&self, list: &[(RecipeDoc, Arc<Recipe>)]) -> Result<u32, String> {
        let used = list.iter().map(|(d, _)| d.plan_version)
            .chain(self.revision_sources()?.values().map(|d| d.plan_version))
            .max().unwrap_or(0);
        let next = self.issued_plan_version()?.max(used).checked_add(1).ok_or("计划版本已达上限")?;
        crate::fsio::write_atomic(&self.dir.join(".plan-version"), &next.to_string())?;
        Ok(next)
    }

    /// 同一配方拍照计划没变就沿用原号，否则（新配方、改了拍照点或产品代码）发新号。
    fn plan_version_in(&self, list: &[(RecipeDoc, Arc<Recipe>)], doc: &RecipeDoc, current_id: &str) -> Result<u32, String> {
        match list.iter().find(|(d, _)| d.id == current_id) {
            Some((current, _)) if current.plan_version != 0 && current.same_plan(doc) => Ok(current.plan_version),
            _ => self.issue_plan_version(list),
        }
    }

    /// 发布前冻结配方时调用：拍照计划没变沿用生产配方的计划版本，变了就发新号（没生效的号作废即可）。
    pub fn plan_version_for(&self, doc: &RecipeDoc) -> Result<u32, String> {
        let inner = self.inner.write().unwrap();
        self.plan_version_in(&inner, doc, &doc.id)
    }

    fn version_after(&self, id: &str, active: Option<u32>) -> Result<u32, String> {
        let retired = self.retired_versions()?.get(&id.to_ascii_lowercase()).copied().unwrap_or(0);
        let source = self.revision_sources()?.get(&id.to_ascii_lowercase()).map_or(0, |doc| doc.version);
        retired.max(source).max(active.unwrap_or(0)).checked_add(1).ok_or_else(|| "配方版本已达上限".into())
    }

    pub fn seed_version_floor(&self, floors: &BTreeMap<String, u32>) -> Result<(), String> {
        let _inner = self.inner.write().unwrap();
        let mut versions = self.retired_versions()?;
        let before = versions.clone();
        for (id, version) in floors {
            let previous = versions.entry(id.to_ascii_lowercase()).or_default();
            *previous = (*previous).max(*version);
        }
        if versions != before {
            crate::fsio::write_atomic(&self.dir.join(".revision-versions"), &serde_json::to_string(&versions).map_err(|e| e.to_string())?)?;
        }
        Ok(())
    }

    pub fn next_version(&self, id: &str) -> Result<u32, String> {
        let inner = self.inner.read().unwrap();
        self.version_after(id, inner.iter().find(|(doc, _)| doc.id.eq_ignore_ascii_case(id)).map(|(doc, _)| doc.version))
    }

    fn write(&self, doc: &RecipeDoc) -> Result<(), String> {
        crate::fsio::write_atomic(&self.file(&doc.id), &serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?)
    }

    fn rollback_new_file(&self, doc: &RecipeDoc) -> Result<(), String> {
        let path = self.file(&doc.id);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("新配方回滚无法检查文件，已保留：{error}")),
        };
        #[cfg(windows)]
        { use std::os::windows::fs::MetadataExt;
          if metadata.file_attributes() & 0x400 != 0 { return Err("新配方文件已变为 reparse point，回滚拒绝删除".into()); } }
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("新配方文件类型已变化，回滚拒绝删除".into());
        }
        let actual = crate::fsio::read_text(&path).map_err(|error| format!("新配方回滚无法读取文件，已保留：{error}"))
            .and_then(|text| serde_json::from_str::<RecipeDoc>(&text).map_err(|error| format!("新配方回滚无法确认实际内容，已保留：{error}")))?;
        if &actual != doc { return Err("新配方实际内容已被外部修改，回滚拒绝删除".into()); }
        std::fs::remove_file(&path).map_err(|error| format!("新配方回滚删除失败，文件已保留，请核查后重试：{error}"))
    }

    fn write_with_source(&self, doc: &RecipeDoc, new_file: bool) -> Result<(), String> {
        if new_file {
            match std::fs::symlink_metadata(self.file(&doc.id)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(format!("无法确认新配方文件是否存在，拒绝覆盖：{error}")),
                Ok(_) => return Err("新配方目标已有文件，拒绝覆盖或删除".into()),
            }
        }
        self.write(doc)?;
        if let Err(error) = self.remember_source(doc) {
            if new_file {
                if let Err(rollback) = self.rollback_new_file(doc) { return Err(format!("{error}；{rollback}")); }
                return Err(format!("{error}；本次新建配方文件已回滚，可直接重试"));
            }
            return Err(error);
        }
        Ok(())
    }

    /// 重读目录。坏文件跳过并记下原因，不影响其他配方。
    pub fn reload(&self) {
        let mut inner = self.inner.write().unwrap();
        let provenance = self.retired_versions().and_then(|versions| self.revision_sources().map(|sources| (versions, sources)));
        let (versions, sources) = match provenance {
            Ok(provenance) => provenance,
            Err(error) => { *self.errors.write().unwrap() = vec![error]; return; }
        };
        let mut list = Vec::new();
        let mut errors = Vec::new();
        let mut unavailable = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
            paths.sort();
            // 计划版本重复（多半是复制了配方文件）：只留修订来源能证明这个号属于它的那一份，与文件名先后无关
            let mut plan_users: BTreeMap<u32, Vec<String>> = BTreeMap::new();
            for p in &paths {
                if let Some(d) = Self::read_regular_file(p).ok().and_then(|s| serde_json::from_str::<RecipeDoc>(&s).ok()).filter(|d| d.plan_version != 0) {
                    plan_users.entry(d.plan_version).or_default().push(d.id.to_ascii_lowercase());
                }
            }
            let plan_owned = |d: &RecipeDoc| -> Result<(), String> {
                let Some(users) = plan_users.get(&d.plan_version).filter(|users| users.len() > 1) else { return Ok(()) };
                let owners: Vec<&String> = users.iter().filter(|id| sources.get(*id).is_some_and(|s| s.plan_version == d.plan_version)).collect();
                if owners.len() == 1 && *owners[0] == d.id.to_ascii_lowercase() { return Ok(()); }
                Err(format!("计划版本 {} 与别的配方文件重复（复制过的配方文件？），没有加载：删掉复制件或在配方页新建", d.plan_version))
            };
            for p in paths {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                let stem = p.file_stem().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                let parsed = Self::read_regular_file(&p)
                    .and_then(|s| serde_json::from_str::<RecipeDoc>(&s).map_err(|e| e.to_string()))
                    .and_then(|mut d| {
                        let r = d.build()?;
                        d.version = r.version;
                        Ok((d, Arc::new(r)))
                    });
                // 手工复制、改过的文件也要守住保存时的规矩：删除按编号找文件，PLC 按产品代码找配方
                let checked = parsed.and_then(|(d, r)| {
                    if !d.id.eq_ignore_ascii_case(&stem) {
                        return Err(format!("文件名与配方编号 {} 不一致，没有加载", d.id));
                    }
                    plan_owned(&d)?;
                    match list.iter().find(|(o, _): &&(RecipeDoc, Arc<Recipe>)| o.id.eq_ignore_ascii_case(&d.id) || o.product_code == d.product_code) {
                        Some((o, _)) if o.product_code == d.product_code => Err(format!("产品代码 {} 与配方 {} 重复，没有加载", d.product_code, o.id)),
                        Some((o, _)) => Err(format!("配方编号与 {} 重复，没有加载", o.id)),
                        None => Ok((d, r)),
                    }
                });
                match checked {
                    Ok((mut doc, mut recipe)) => {
                        let id = doc.id.to_ascii_lowercase();
                        let source = sources.get(&id);
                        let floor = versions.get(&id).copied().unwrap_or(0).max(source.map_or(0, |previous| previous.version));
                        let proven = source.is_some_and(|previous| previous == &doc);
                        let accepted = (|| -> Result<(), String> {
                            if doc.version < floor || doc.version == floor && !proven {
                                doc.version = floor.checked_add(1).ok_or("配方版本已达上限")?;
                                recipe = Arc::new(doc.build()?);
                                self.write(&doc)?;
                            }
                            self.remember_source(&doc)
                        })();
                        match accepted {
                            Ok(()) => list.push((doc, recipe)),
                            Err(error) => errors.push(format!("{name}：修订来源未能持久保存，没有加载：{error}")),
                        }
                    },
                    Err(e) => {
                        errors.push(format!("{name}：{e}"));
                        if let Some(item) = UnavailableRecipe::from_file(&p, e) {
                            unavailable.push(item);
                        }
                    },
                }
            }
        }
        // 手工放进来的配方可能带着比记录还大的计划版本：记录跟上，以后发的号不和它撞
        let max_plan = list.iter().map(|(d, _)| d.plan_version).max().unwrap_or(0);
        if self.issued_plan_version().is_ok_and(|issued| issued < max_plan) {
            if let Err(error) = crate::fsio::write_atomic(&self.dir.join(".plan-version"), &max_plan.to_string()) {
                errors.push(format!("计划版本记录未能更新：{error}"));
            }
        }
        *inner = list;
        *self.errors.write().unwrap() = errors;
        *self.unavailable.write().unwrap() = unavailable;
    }

    fn read_regular_file(path: &std::path::Path) -> Result<String, String> {
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        { use std::os::windows::fs::MetadataExt;
          if metadata.file_attributes() & 0x400 != 0 { return Err("配方文件为 reparse point，拒绝读取或删除".into()); } }
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("配方文件类型不安全，拒绝读取或删除".into());
        }
        crate::fsio::read_text(path).map_err(|e| e.to_string())
    }

    pub fn unavailable(&self) -> Vec<UnavailableRecipe> {
        self.unavailable.read().unwrap().clone()
    }

    pub fn delete_unavailable(&self, file_name: &str) -> Result<(), String> {
        if file_name.is_empty() || file_name.starts_with('.') || !file_name.ends_with(".json")
            || file_name.contains(['/', '\\', ':']) || file_name.trim() != file_name {
            return Err("无效的配方文件名".into());
        }
        self.reload();
        {
            let inner = self.inner.write().unwrap();
            if !self.unavailable.read().unwrap().iter().any(|item| item.file_name == file_name) {
                return Err("该文件不在不可用配方列表中，请刷新后重试".into());
            }
            let path = self.dir.join(file_name);
            let text = Self::read_regular_file(&path)?;
            if serde_json::from_str::<RecipeDoc>(&text).ok().is_some_and(|doc| doc.build().is_ok()) {
                return Err("配方内容已可用，请刷新后通过正常配方操作处理".into());
            }
            let stem = path.file_stem().and_then(|name| name.to_str()).ok_or("无效的配方文件名")?;
            if inner.iter().any(|(doc, _)| doc.id.eq_ignore_ascii_case(stem)) {
                return Err("该文件对应已加载的生产配方，拒绝删除".into());
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(version) = value.get("version").and_then(|v| v.as_u64()).and_then(|v| u32::try_from(v).ok()) {
                    self.retire_version(stem, version)?;
                    if let Some(id) = value.get("id").and_then(|v| v.as_str())
                        .filter(|id| !id.trim().is_empty() && !inner.iter().any(|(doc, _)| doc.id.eq_ignore_ascii_case(id))) {
                        self.retire_version(id, version)?;
                    }
                }
                if let Some(plan) = value.get("planVersion").and_then(|v| v.as_u64()).and_then(|v| u32::try_from(v).ok()) {
                    let plan = plan.max(self.issued_plan_version()?);
                    crate::fsio::write_atomic(&self.dir.join(".plan-version"), &plan.to_string())?;
                }
            }
            std::fs::remove_file(&path).map_err(|e| format!("删除不可用配方失败：{e}"))?;
        }
        self.reload();
        Ok(())
    }

    pub fn list(&self) -> Vec<Arc<Recipe>> {
        self.inner.read().unwrap().iter().map(|(_, r)| r.clone()).collect()
    }

    pub fn errors(&self) -> Vec<String> {
        self.errors.read().unwrap().clone()
    }

    pub fn get(&self, id: &str) -> Option<Arc<Recipe>> {
        self.inner.read().unwrap().iter().find(|(d, _)| d.id == id).map(|(_, r)| r.clone())
    }

    pub fn doc(&self, id: &str) -> Option<RecipeDoc> {
        self.inner.read().unwrap().iter().find(|(d, _)| d.id == id).map(|(d, _)| d.clone())
    }

    /// 保存配方时版本号 +1；编号（不分大小写：Windows 上文件名不分）与产品代码都不能和别的配方重复。
    #[cfg(test)]
    pub fn save(&self, mut doc: RecipeDoc, original_id: Option<&str>) -> Result<Arc<Recipe>, String> {
        doc.name = doc.name.trim().to_string();
        doc.build()?;
        let mut inner = self.inner.write().unwrap();
        let replacing = original_id.unwrap_or(&doc.id).to_string();
        if let Some((d, _)) = inner.iter().find(|(d, _)| d.id != replacing && (d.id.eq_ignore_ascii_case(&doc.id) || d.product_code == doc.product_code)) {
            return Err(if d.id.eq_ignore_ascii_case(&doc.id) {
                format!("配方编号 {} 已存在", d.id)
            } else {
                format!("产品代码 {} 已被配方 {} 使用", doc.product_code, d.id)
            });
        }
        // 目录里已有这个文件却不是正在保存的这个配方：启动时没能加载，或是之后手工放进来的，不能覆盖
        let ours = inner.iter().any(|(d, _)| d.id == replacing && d.id.eq_ignore_ascii_case(&doc.id));
        if !ours && self.file(&doc.id).exists() {
            return Err(format!("配方目录里已有 {}.json 但没有加载（见配方页的提示）：移走它，或修好后重启程序", doc.id));
        }
        let old = inner.iter().find(|(d, _)| d.id == replacing).map(|(d, _)| d.version);
        doc.version = self.version_after(&doc.id, old)?.max(if old.is_none() { doc.version.max(1) } else { 1 });
        doc.plan_version = self.plan_version_in(&inner, &doc, &replacing)?;
        if !replacing.eq_ignore_ascii_case(&doc.id) {
            if let Some(version) = old { self.retire_version(&replacing, version)?; }
        }
        let recipe = Arc::new(doc.build()?);
        self.write_with_source(&doc, !ours)?;
        // 只改了大小写时新旧是同一个文件，不能删
        if !replacing.eq_ignore_ascii_case(&doc.id) {
            let _ = std::fs::remove_file(self.file(&replacing));
        }
        inner.retain(|(d, _)| d.id != replacing && d.id != doc.id);
        inner.push((doc, recipe.clone()));
        inner.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        Ok(recipe)
    }

    pub fn save_published(&self, mut doc: RecipeDoc, expected_base: Option<&str>) -> Result<Arc<Recipe>, String> {
        doc.name = doc.name.trim().to_string();
        let built = doc.build()?;
        let mut inner = self.inner.write().unwrap();
        let current = inner.iter().find(|(previous, _)| previous.id == doc.id);
        if let Some((previous, recipe)) = current.filter(|(_, recipe)| recipe.revision_id == built.revision_id) {
            if previous == &doc { return Ok(recipe.clone()); }
            return Err("同一发布修订的实际配方不同，不能覆盖".into());
        }
        if current.map(|(_, recipe)| recipe.revision_id.as_str()) != expected_base {
            return Err("待发布版本与当前生产配方冲突".into());
        }
        let floor = self.version_after(&doc.id, current.map(|(previous, _)| previous.version))?;
        let expected_version = if current.is_some() { floor } else { floor.max(doc.version.max(1)) };
        if doc.version != expected_version { return Err("发布版本必须是明确分配的下一版本".into()); }
        let kept = current.is_some_and(|(previous, _)| previous.plan_version != 0 && previous.same_plan(&doc));
        let plan_ok = if kept {
            current.is_some_and(|(previous, _)| previous.plan_version == doc.plan_version)
        } else {
            doc.plan_version != 0 && doc.plan_version <= self.issued_plan_version()?
                && !inner.iter().any(|(previous, _)| previous.plan_version == doc.plan_version)
        };
        if !plan_ok { return Err("发布的拍照计划版本必须由配方库分配：计划没变沿用原号，变了用新号".into()); }
        if inner.iter().any(|(previous, _)| previous.id != doc.id &&
            (previous.id.eq_ignore_ascii_case(&doc.id) || previous.product_code == doc.product_code)) {
            return Err("配方编号或产品代码已被其他配方使用".into());
        }
        if current.is_none() && self.file(&doc.id).exists() {
            return Err("配方目录已有未加载文件，不能覆盖".into());
        }
        let recipe = Arc::new(built);
        self.write_with_source(&doc, current.is_none())?;
        inner.retain(|(previous, _)| previous.id != doc.id);
        inner.push((doc, recipe.clone()));
        inner.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        Ok(recipe)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.write().unwrap();
        let version = inner.iter().find(|(d, _)| d.id == id).ok_or("配方不存在")?.0.version;
        self.retire_version(id, version)?;
        std::fs::remove_file(self.file(id)).map_err(|e| format!("删除配方文件失败：{e}"))?;
        inner.retain(|(d, _)| d.id != id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::plan::SegmentCalib;

    /// 目录里只有一份 schema 7 配方文件（没有段模型字段；修订版本 7、计划版本 40）时打开的配方库；返回（目录，配方库，旧配方，文件名）。
    fn obsolete_store(tag: &str) -> (PathBuf, RecipeStore, RecipeDoc, String) {
        let dir = std::env::temp_dir().join(format!("gluesight-unavailable-{tag}-{}-{}", std::process::id(), ly_plc::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut old = samples().remove(0);
        old.schema_version = 7;
        old.version = 7;
        old.plan_version = 40;
        let file_name = format!("{}.json", old.id);
        let mut file = serde_json::to_value(&old).unwrap();
        for key in ["segments", "inspect", "segmentDefaults", "calibrationRounds", "partCalib"] {
            file.as_object_mut().unwrap().remove(key);
        }
        std::fs::write(dir.join(&file_name), file.to_string()).unwrap();
        let store = RecipeStore::open(dir.clone()).unwrap();
        (dir, store, old, file_name)
    }

    /// 段配方的整件标定（各段待分段算法时凭它发布）。
    fn part_calib(total: u32) -> PartCalib {
        PartCalib { expected_total: total, calib: SegmentCalib { rounds: 10, min: total - 1, max: total + 1, mean: total as f64 } }
    }

    /// samples_burst（直线 N_exp = 4）后面接一个弯道段。
    fn two_segment_burst(corner_frames: Option<u32>) -> RecipeDoc {
        let mut doc = samples_burst();
        doc.segments.push(plan::new_segment(2, SegmentKind::Corner, &doc.segment_defaults));
        doc.segments[1].expected_frames = corner_frames;
        doc
    }

    /// 旧格式文件删掉后，同编号配方按当前格式重新保存并已加载。
    fn recreated_after_obsolete_deleted(tag: &str) -> (PathBuf, RecipeStore, RecipeDoc, String) {
        let (dir, store, mut old, file_name) = obsolete_store(tag);
        store.delete_unavailable(&file_name).unwrap();
        old.schema_version = RECIPE_SCHEMA;
        store.save(old.clone(), None).unwrap();
        (dir, store, old, file_name)
    }

    /// 迁移 D-0：schema 7（升级到段模型前）的配方文件不迁移，列为不可用，也不加载来运行。
    #[test]
    fn schema_7_recipe_file_is_listed_unavailable() {
        let (dir, store, old, file_name) = obsolete_store("listed");
        assert!(store.get(&old.id).is_none());
        assert!(store.doc(&old.id).is_none());
        assert!(store.list().is_empty());
        let unavailable = store.unavailable();
        assert_eq!(unavailable.len(), 1);
        assert_eq!(unavailable[0].file_name, file_name);
        assert_eq!(unavailable[0].id.as_deref(), Some(old.id.as_str()));
        assert_eq!(unavailable[0].schema_version, Some(7));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_an_unavailable_recipe_removes_its_file_and_clears_the_list() {
        let (dir, store, _, file_name) = obsolete_store("delete");
        store.delete_unavailable(&file_name).unwrap();
        assert!(!dir.join(&file_name).exists());
        assert!(store.errors().is_empty());
        assert!(store.unavailable().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// delete_unavailable 路径上唯一防止修订号复用的保护：删掉的旧文件版本必须记下，重启后同编号重建也不回退。
    #[test]
    fn recipe_recreated_after_deleting_its_unavailable_file_and_restarting_never_reuses_versions() {
        let (dir, store, mut old, file_name) = obsolete_store("versions");
        store.delete_unavailable(&file_name).unwrap();
        drop(store);
        let store = RecipeStore::open(dir.clone()).unwrap();
        assert!(store.next_version(&old.id).unwrap() > old.version);
        old.schema_version = RECIPE_SCHEMA;
        let recreated = store.save(old.clone(), None).unwrap();
        assert!(recreated.version > old.version);
        assert!(recreated.plan_version > old.plan_version);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn loaded_recipe_file_cannot_be_deleted_as_unavailable() {
        let (dir, store, old, file_name) = recreated_after_obsolete_deleted("loaded");
        assert!(store.delete_unavailable(&file_name).is_err());
        assert!(store.get(&old.id).is_some());
        assert!(dir.join(&file_name).is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_an_obsolete_copy_leaves_the_loaded_recipe_with_the_same_id_unchanged() {
        let (dir, store, mut old, _) = recreated_after_obsolete_deleted("copy");
        let original = store.doc(&old.id).unwrap();
        old.schema_version = RECIPE_SCHEMA - 1;
        old.version = 100;
        old.plan_version = 100;
        std::fs::write(dir.join("obsolete-copy.json"), serde_json::to_string(&old).unwrap()).unwrap();
        store.reload();
        store.delete_unavailable("obsolete-copy.json").unwrap();
        assert_eq!(store.doc(&old.id).unwrap(), original);
        assert!(store.unavailable().is_empty());
        assert!(store.errors().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn composite_recipe() -> RecipeDoc {
        let mut doc = samples().remove(1);
        for (shot, view) in doc.shots.iter_mut().zip([1, 2, 3, 1]) { shot.view = view; }
        doc
    }

    #[test]
    fn multiview_keeps_one_shot_and_distinct_segments() {
        let mut doc = samples().remove(0);
        doc.shots.truncate(1);
        let shot = &mut doc.shots[0];
        shot.views = [1, 2, 3].into_iter().map(|view| ShotViewSpec { view, enabled: view != 2,
            path: shot.path.clone(), mm_per_px: shot.mm_per_px, detect: None, limits: None, calib: None, fixture: None }).collect();
        let recipe = doc.build().unwrap();
        assert_eq!(recipe.shot_count(), 1);
        assert_eq!(recipe.segments.iter().map(|s| (s.shot, s.view)).collect::<Vec<_>>(), [(0, 1), (0, 3)]);
        assert!(recipe.ready().is_ok());
        let view = recipe.for_view(0, 3).unwrap();
        assert_eq!(view.shots[0].view, 3);
        assert_eq!(view.shots[0].calib_ref(), "cam1-v3");
        assert_eq!(view.segments.len(), 1);
        assert!(view.segments[0].first > 0);
    }

    #[test]
    fn stations_follow_each_taught_line() {
        let r = samples().remove(1).build().unwrap();
        assert_eq!(r.segments.len(), 4);
        // 站距 1 mm、0.112 mm/px：中线长约 680.3 / 676.9 / 685.4 / 680.3 px，每隔约 8.93 px 一站，含起点
        let expected_counts = [77, 76, 77, 77];
        for (k, g) in r.segments.iter().enumerate() {
            let shot = &r.shots[k];
            assert_eq!((g.shot, g.name.as_str()), (k, format!("{} · J1", shot.id).as_str()));
            // 首站在中线起点，站与站沿线相隔 spacing（换成像素）
            assert_eq!([r.points.x[g.first], r.points.y[g.first]], shot.path[0]);
            let expect = expected_counts[k];
            assert_eq!(g.count, expect);
            assert!((g.length(r.spacing) - (expect - 1) as f32 * r.spacing).abs() < 1e-4);
            assert!(r.owned_points(k).all(|j| r.points.seg[j] as usize == k));
        }
        assert_eq!(r.segments.last().map(|g| g.first + g.count), Some(r.point_count()));
    }

    #[test]
    fn skipped_and_untaught_shots_have_no_points() {
        let mut doc = samples().remove(1);
        doc.shots[1].skip = true;
        doc.shots[2].path.clear();
        doc.shots[2].mm_per_px = None;
        let r = doc.build().unwrap();
        assert_eq!(r.segments.iter().map(|g| g.shot).collect::<Vec<_>>(), [0, 3]);
        assert_eq!(r.owned_points(1).count() + r.owned_points(2).count(), 0);
        let untaught = r.ready().unwrap_err();
        assert!(untaught.contains("MTR-HSG-B") && untaught.contains("P3"), "{untaught}");
        doc.shots[2].skip = true;
        assert!(doc.build().unwrap().ready().is_ok());
        doc.shots.iter_mut().for_each(|shot| shot.skip = true);
        assert!(doc.build().unwrap().ready().unwrap_err().contains("至少需要一个参与检测"));
    }

    #[test]
    fn every_save_advances_the_explicit_recipe_revision() {
        let dir = std::env::temp_dir().join(format!("gluesight-revision-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let doc = composite_recipe();
        let first = store.save(doc.clone(), None).unwrap();
        let second = store.save(doc, Some(&first.id)).unwrap();
        assert_eq!(second.version, first.version + 1);
        assert_eq!(second.revision_id, format!("{}-v{}", second.id, second.version));
        store.reload();
        assert_eq!(store.get(&second.id).unwrap().revision_id, second.revision_id);
        let mut forged = serde_json::to_value(&*second).unwrap();
        forged["revisionId"] = serde_json::json!("untrusted-external-revision");
        let restored: Recipe = serde_json::from_value(forged).unwrap();
        assert_eq!(restored.revision_id, second.revision_id);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleted_and_renamed_ids_keep_monotonic_versions_across_restart() {
        let dir = std::env::temp_dir().join(format!("gluesight-retired-version-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = composite_recipe(); doc.id = "REUSED".into(); doc.product_code = 60002; doc.version = 1;
        let original = store.save(doc.clone(), None).unwrap();
        store.delete(&doc.id).unwrap();
        drop(store);
        let store = RecipeStore::open(dir.clone()).unwrap();
        assert_eq!(store.next_version("REUSED").unwrap(), 2);
        let recreated = store.save(doc.clone(), None).unwrap();
        assert_eq!(recreated.version, 2);
        assert_ne!(recreated.revision_id, original.revision_id);
        let mut renamed = store.doc("REUSED").unwrap(); renamed.id = "RENAMED".into();
        assert_eq!(store.save(renamed, Some("REUSED")).unwrap().version, 3);
        drop(store);
        let store = RecipeStore::open(dir.clone()).unwrap();
        assert_eq!(store.next_version("REUSED").unwrap(), 3);
        doc.version = store.next_version("REUSED").unwrap(); doc.product_code = 60003;
        doc.plan_version = store.plan_version_for(&doc).unwrap();
        let assigned = doc.build().unwrap();
        let published = store.save_published(doc.clone(), None).unwrap();
        assert_eq!(published.revision_id, assigned.revision_id);
        assert_eq!(published.version, 3);
        assert_eq!(store.save_published(doc, None).unwrap().version, 3);
        store.delete("RENAMED").unwrap();
        let mut moved = store.doc("REUSED").unwrap(); moved.id = "RENAMED".into();
        assert_eq!(store.save(moved, Some("REUSED")).unwrap().version, 4);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plan_version_changes_only_when_the_plc_plan_changes() {
        let dir = std::env::temp_dir().join(format!("gluesight-plan-version-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let b = store.doc("MTR-HSG-B").unwrap();
        let plan = b.plan_version;
        assert_ne!(plan, 0);
        // 只改限值、检测参数、示教中线、"不检"：PLC 不用改
        let mut doc = b.clone();
        doc.limits.max_gap_len = 3.0;
        doc.detect.search_mm = 9.0;
        doc.shots[0].path[1][0] += 5.0;
        doc.shots[1].skip = true;
        doc.cameras.get_mut("cam1").unwrap().exposure_us = 90.0;
        assert_eq!(store.save(doc.clone(), None).unwrap().plan_version, plan);
        // 改了 Pose、相机、拍照点顺序或产品代码：发新号，且和别的配方都不重
        let mut seen = vec![plan, store.doc("MTR-HSG-A").unwrap().plan_version];
        let edits: [fn(&mut RecipeDoc); 4] = [
            |d| d.shots[0].pose_id = "P9".into(),
            |d| { d.shots.iter_mut().for_each(|shot| shot.camera = "another-device".into()); d.sync_imaging(); },
            |d| d.shots.swap(0, 1),
            |d| d.product_code = 61000,
        ];
        for edit in edits {
            let mut doc = store.doc("MTR-HSG-B").unwrap();
            edit(&mut doc);
            let saved = store.save(doc, None).unwrap().plan_version;
            assert!(!seen.contains(&saved), "{saved} reused");
            seen.push(saved);
        }
        // 删掉再建同名配方、计划版本记录丢失：都不回到用过的号
        store.delete("MTR-HSG-B").unwrap();
        std::fs::remove_file(dir.join(".plan-version")).unwrap();
        drop(store);
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut recreated = b.clone();
        recreated.product_code = 62000;
        assert!(store.save(recreated, None).unwrap().plan_version > store.doc("MTR-HSG-A").unwrap().plan_version);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 规格 §1：计划版本由配方库分配——段计划身份不变（只改容差、限值）沿用原号；N_exp 变了发新号，且不与用过的号重复。
    /// 身份由哪些字段组成见 plan::tests 的参数化用例，这里防的是配方库分配没有接上段计划身份。
    #[test]
    fn publish_reuses_plan_version_for_same_identity_and_issues_new_otherwise() {
        let dir = std::env::temp_dir().join(format!("gluesight-burst-plan-version-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let saved = store.save(samples_burst(), None).unwrap();
        let used: Vec<u32> = store.list().iter().map(|r| r.plan_version).collect();
        let cases: [(&str, fn(&mut RecipeDoc), bool); 2] = [
            ("容差与胶宽限值", |d| { d.segments[0].tolerance = 2; d.segments[0].checks.width.tol_upper = 2.0; }, true),
            ("重新标定后 N_exp 变了", |d| d.segments[0].expected_frames = Some(5), false),
        ];
        for (case, edit, reused) in cases {
            let mut doc = store.doc(&saved.id).unwrap();
            edit(&mut doc);
            let assigned = store.plan_version_for(&doc).unwrap();
            if reused {
                assert_eq!(assigned, saved.plan_version, "{case}");
            } else {
                assert!(!used.contains(&assigned), "{case}：{assigned} 复用了 {used:?} 中的号");
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 规格 §1 + 决策补充第二节：整件预期帧组数没标定时不能发布，逐条点名待标定的段；有整件标定时各段 N_exp 可以为空。
    #[test]
    fn uncalibrated_segments_block_ready_and_name_each_segment() {
        let mut doc = two_segment_burst(None);
        doc.segments.push(plan::new_segment(3, SegmentKind::Straight, &doc.segment_defaults));
        let reasons = doc.build().unwrap().ready_burst().unwrap_err();
        assert!(reasons.len() == 2 && reasons[0].contains("第 2 段") && reasons[1].contains("第 3 段"), "{reasons:?}");
        doc.part_calib = Some(part_calib(9));
        assert_eq!(doc.build().unwrap().ready_burst(), Ok(()));
    }

    /// D-SB-20：三台相机都示教了走廊（至少 2 点、不短于 1 px）和像素当量才能发布，缺哪台点名哪台。
    #[test]
    fn untaught_camera_blocks_ready_and_names_the_camera() {
        assert_eq!(samples_burst().build().unwrap().ready_burst(), Ok(()));
        let cases: [(&str, fn(&mut CameraInspect)); 3] = [
            ("走廊不到 2 点", |i| i.corridor.truncate(1)),
            ("走廊两点重合", |i| i.corridor = vec![[10.0, 10.0], [10.0, 10.0]]),
            ("没有像素当量", |i| i.mm_per_px = None),
        ];
        for (case, edit) in cases {
            let mut doc = samples_burst();
            edit(&mut doc.inspect[1]);
            let reasons = doc.build().unwrap().ready_burst().unwrap_err();
            assert!(reasons.len() == 1 && reasons[0].contains("cam2"), "{case}：{reasons:?}");
        }
    }

    /// D-SB-1：段配方不能再有拍照点、只支持飞拍、标定轮数 10..=100、成像参数恰好覆盖检测相机、整件标定放得进 PLC shotCount；
    /// 没有段的旧配方不能带检测相机或整件标定。
    #[test]
    fn recipe_mixing_shot_and_segment_models_or_out_of_range_burst_settings_is_rejected() {
        let cases: [(&str, fn(&mut RecipeDoc), Option<&str>); 11] = [
            ("段配方带拍照点", |d| d.shots = samples().remove(1).shots, Some("shots")),
            ("段配方停稳拍", |d| d.trigger_mode = TriggerMode::Stop, Some("飞拍")),
            ("标定轮数 9", |d| d.calibration_rounds = 9, Some("标定轮数")),
            ("标定轮数 101", |d| d.calibration_rounds = 101, Some("标定轮数")),
            ("标定轮数 100 在上限", |d| d.calibration_rounds = 100, None),
            ("检测相机缺成像参数", |d| { d.cameras.remove("cam3"); }, Some("曝光与增益")),
            ("成像参数多给一台不用的相机", |d| { d.cameras.insert("cam9".into(), DEFAULT_IMAGING); }, Some("曝光与增益")),
            ("整件标定帧组数为 0", |d| d.part_calib = Some(PartCalib { expected_total: 0, calib: part_calib(4).calib }), Some("整件标定")),
            ("整件标定帧组数 32767 在上限", |d| d.part_calib = Some(part_calib(plan::MAX_EXPECTED_TOTAL)), None),
            ("旧配方带检测相机", |d| { let inspect = std::mem::take(&mut d.inspect); *d = samples().remove(0); d.inspect = inspect; }, Some("检测相机")),
            ("旧配方带整件标定", |d| { *d = samples().remove(0); d.part_calib = Some(part_calib(4)); }, Some("整件标定")),
        ];
        for (case, edit, error) in cases {
            let mut doc = samples_burst();
            edit(&mut doc);
            match (doc.build(), error) {
                (Ok(_), None) => {}
                (Err(e), Some(what)) => assert!(e.contains(what), "{case}：{e}"),
                (result, _) => panic!("{case}：{:?}", result.map(|_| ())),
            }
        }
    }

    /// 新建段从 segment_defaults 取容差与检测项：弯道默认带胶偏、默认容差超上限或默认限值非法时，validate_burst 拒绝，
    /// 否则 new_segment 会生成过不了段校验的段。
    #[test]
    fn corner_default_with_offset_or_default_tolerance_or_limits_out_of_range_is_rejected() {
        assert_eq!(samples_burst().validate(), Ok(()));
        let cases: [(&str, fn(&mut KindDefaults)); 3] = [
            ("弯道默认带胶偏检测", |d| d.corner.checks.offset = d.straight.checks.offset.clone()),
            ("弯道默认容差超过 1000", |d| d.corner.tolerance = plan::MAX_TOLERANCE + 1),
            ("直线默认有胶比例超过 1", |d| d.straight.checks.gap.min_present = 1.2),
        ];
        for (case, edit) in cases {
            let mut doc = samples_burst();
            edit(&mut doc.segment_defaults);
            assert!(doc.validate().is_err(), "{case}");
        }
    }

    /// 契约：Recipe JSON 里 `segments` 是旧模型的测量段，段配方的段列表在 `pathSegments`；
    /// 检测记录、历史按快照还原配方时不能把两者混淆。
    #[test]
    fn burst_recipe_snapshot_restores_its_path_segments() {
        let doc = two_segment_burst(Some(3));
        let restored = Recipe::from_snapshot(serde_json::to_value(doc.build().unwrap()).unwrap()).unwrap();
        assert_eq!(restored.path_segments, doc.segments);
        assert_eq!(restored.plan_cameras(), doc.plan_cameras());
    }

    #[test]
    fn outdated_revision_source_is_retired_without_blocking_startup_or_reusing_numbers() {
        let dir = std::env::temp_dir().join(format!("gluesight-outdated-source-{}-{}", std::process::id(), ly_plc::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut old = samples().remove(0);
        old.schema_version = RECIPE_SCHEMA - 1;
        old.version = 7;
        old.plan_version = 40;
        let sources = BTreeMap::from([(old.id.to_ascii_lowercase(), old.clone())]);
        std::fs::write(dir.join(".revision-sources"), serde_json::to_string(&sources).unwrap()).unwrap();
        let store = RecipeStore::open(dir.clone()).unwrap();
        assert!(store.next_version(&old.id).unwrap() > old.version);
        let mut doc = composite_recipe(); doc.id = "AFTER-OUTDATED".into(); doc.product_code = 60007;
        assert!(store.plan_version_for(&doc).unwrap() > old.plan_version);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_publish_write_does_not_consume_the_frozen_target_version() {
        let dir = std::env::temp_dir().join(format!("gluesight-publish-retry-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = composite_recipe(); doc.id = "WRITE-RETRY".into(); doc.product_code = 60005;
        doc.version = store.next_version(&doc.id).unwrap();
        doc.plan_version = store.plan_version_for(&doc).unwrap();
        let blocking = store.file(&doc.id).with_extension("tmp");
        std::fs::create_dir(&blocking).unwrap();
        assert!(store.save_published(doc.clone(), None).is_err());
        assert_eq!(store.next_version(&doc.id).unwrap(), doc.version);
        std::fs::remove_dir(blocking).unwrap();
        assert_eq!(store.save_published(doc.clone(), None).unwrap().version, doc.version);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn newly_created_recipe_source_failure_can_retry_both_save_paths_without_reload() {
        for published in [false, true] {
            let dir = std::env::temp_dir().join(format!("gluesight-new-source-retry-{published}-{}-{}", std::process::id(), ly_plc::now_ms()));
            let store = RecipeStore::open(dir.clone()).unwrap();
            let mut doc = composite_recipe(); doc.id = "NEW-SOURCE-RETRY".into(); doc.product_code = 60006;
            doc.plan_version = store.plan_version_for(&doc).unwrap();
            let blocking = dir.join(".revision-sources").with_extension("tmp"); std::fs::create_dir(&blocking).unwrap();
            let failed = if published { store.save_published(doc.clone(), None) } else { store.save(doc.clone(), None) };
            assert!(failed.is_err());
            assert!(!store.file(&doc.id).exists()); assert!(store.get(&doc.id).is_none());
            assert_eq!(store.next_version(&doc.id).unwrap(), 1);
            std::fs::remove_dir(&blocking).unwrap();
            let saved = if published { store.save_published(doc.clone(), None) } else { store.save(doc.clone(), None) }.unwrap();
            assert_eq!(saved.version, 1); assert!(store.file(&doc.id).is_file());
            assert_eq!(store.revision_sources().unwrap()[&doc.id.to_ascii_lowercase()], store.doc(&doc.id).unwrap());
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn published_versions_are_assigned_once_and_crash_retries_cannot_overwrite_a_revision() {
        let dir = std::env::temp_dir().join(format!("gluesight-published-version-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = composite_recipe();
        doc.id = "EXPLICIT-PUBLISHED".into();
        doc.product_code = 60000;
        doc.version = 7;
        assert!(store.save_published(doc.clone(), None).unwrap_err().contains("计划版本"));
        doc.plan_version = store.plan_version_for(&doc).unwrap();
        let first = store.save_published(doc.clone(), None).unwrap();
        assert_eq!(first.revision_id, "EXPLICIT-PUBLISHED-v7");
        assert_eq!(store.save_published(doc.clone(), None).unwrap().version, 7);
        let mut different = doc.clone();
        different.shots[0].view = 2;
        assert!(store.save_published(different, None).is_err());
        doc.version = 8;
        let second = store.save_published(doc.clone(), Some(&first.revision_id)).unwrap();
        assert_eq!(second.revision_id, "EXPLICIT-PUBLISHED-v8");
        doc.version = 9;
        assert!(store.save_published(doc.clone(), Some(&first.revision_id)).is_err());
        assert_eq!(store.get(&doc.id).unwrap().version, 8);
        assert_eq!(store.save(doc, None).unwrap().version, 9);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tricam_shots_on_different_views_stay_one_device() {
        let recipe = composite_recipe().build().unwrap();
        assert_eq!(recipe.cameras(), ["cam1"]);
        assert_eq!(recipe.shots.iter().map(|s| s.view).collect::<Vec<_>>(), [1, 2, 3, 1]);
    }

    #[test]
    fn shot_view_outside_1_to_3_is_rejected() {
        for invalid in [0, 4, 255] {
            let mut bad = composite_recipe();
            bad.shots[0].view = invalid;
            assert!(bad.build().is_err(), "{invalid}");
        }
    }

    #[test]
    fn recipe_file_without_an_explicit_shot_view_is_rejected() {
        let mut missing = serde_json::to_value(composite_recipe()).unwrap();
        missing["shots"][0].as_object_mut().unwrap().remove("view");
        assert!(serde_json::from_value::<RecipeDoc>(missing).is_err());
    }

    #[test]
    fn invalid_recipes_are_rejected() {
        let reject = |f: &dyn Fn(&mut RecipeDoc), what: &str| {
            let mut doc = composite_recipe();
            f(&mut doc);
            let e = doc.build().unwrap_err();
            assert!(e.contains(what), "{e}");
        };
        reject(&|d| d.shots[1].id = "P1".into(), "重复");
        reject(&|d| d.shots[2].camera = "cam 3".into(), "相机编号");
        reject(&|d| d.shots[0].pose_id = " ".into(), "Pose");
        reject(&|d| d.shots[0].bead = String::new(), "胶条名");
        reject(&|d| d.shots[3].calib = Some("a/b".into()), "标定引用");
        reject(&|d| d.shots[0].mm_per_px = Some(0.0), "像素当量");
        reject(&|d| d.shots[1].detect = Some(DetectParams { search_mm: 3.0, polarity: Polarity::Dark, width_range: [1.0, 8.0] }), "搜索宽度");
        reject(&|d| d.limits.max_gap_len = -1.0, "断胶长度");
        reject(&|d| d.schema_version = 2, "格式版本");
        // 特征定位：外圈出左、上边，框太小，lyFlow 门限越界，护栏被关掉
        let fixture = |f: fn(&mut FixtureSpec)| move |d: &mut RecipeDoc| { let mut spec = fixture_spec(); f(&mut spec); d.shots[0].fixture = Some(spec); };
        reject(&fixture(|f| f.rect[0] = 3), "左、上边");
        reject(&fixture(|f| f.rect[2] = 8), "16–512");
        reject(&fixture(|f| f.search_radius_px = 2.0), "搜索半径");
        reject(&fixture(|f| f.angle_range_deg = -1.0), "角度范围");
        reject(&fixture(|f| f.min_score = 0.0), "最低分数");
        reject(&fixture(|f| f.max_peak_ratio = 1.5), "次峰比");
        reject(&fixture(|f| f.max_end_shift_mm = Some(0.0)), "两端搬移上限");
        // 模板身份靠来源图编号：缺了就说不清模板是哪张图裁的（G2 时期的配方）
        reject(&fixture(|f| f.source.clear()), "模板来源");
        // 不判位置时定位失败报不出 ERR、两端排除没有上限
        reject(&|d| { fixture(|_| {})(d); d.limits.position = None; }, "必须设置位置判定限值");
        // 名义值不为 0 要有量具确认件
        let nominal = |d: &mut RecipeDoc| d.limits.position.as_mut().unwrap().nominal = 0.3;
        reject(&nominal, "gaugeRef");
        reject(&|d| { nominal(d); d.limits.gauge_ref = Some("  ".into()); }, "gaugeRef");
        let mut accepted = composite_recipe();
        fixture(|_| {})(&mut accepted);
        nominal(&mut accepted);
        accepted.limits.gauge_ref = Some("GA-0315".into());
        assert!(accepted.build().is_ok(), "带位置限值的特征定位、名义值附量具确认件");
        let pose_shared = {
            let mut d = composite_recipe();
            d.shots[1].pose_id = "P1".into();
            d
        };
        assert!(pose_shared.build().is_ok(), "同一设备允许同一 Pose 多次触发");
    }

    fn fixture_spec() -> FixtureSpec {
        FixtureSpec { rect: [100, 100, 32, 32], source: "capture-1-0-v1".into(), search_radius_px: 20.0, angle_range_deg: 0.0,
            min_score: 0.6, max_peak_ratio: 1.0, max_end_shift_mm: None, end_margin_mm: 0.0 }
    }

    #[test]
    fn fixture_template_overlapping_any_bead_corridor_of_the_same_scene_is_rejected() {
        // 防的回归（定稿 §11 跨阶段约束）：模板框盖住胶时位姿跟着胶走、d′ 恒约等于 0，真实胶偏被放行。
        // 走廊 = 示教中线 ± 搜索半宽，外加 4 px 余量；同一 Pose、同一相机、同一幅图的其他拍照点的胶也在这一帧的场景里
        let half = 15.0 / f64::from(SIM_MM_PER_PX); // 默认搜索半宽 15 mm
        let need = half + FIXTURE_CORRIDOR_MARGIN_PX;
        // 框 [600, 280, 32, 32] 的外圈下沿在 y = 280 + 32 + 4 − 1 = 315（像素中心）；中线水平，横跨框的 x 范围
        let bottom = 315.0;
        let line = |y: f64| vec![[300.0, y as f32], [980.0, y as f32]];
        let at = |d: &mut RecipeDoc, k: usize, path: Vec<[f32; 2]>| { d.shots[k].path = path; d.shots[k].mm_per_px = Some(SIM_MM_PER_PX); };
        let with_box = |d: &mut RecipeDoc, k: usize| d.shots[k].fixture = Some(FixtureSpec { rect: [600, 280, 32, 32], ..fixture_spec() });
        let cases: [(&str, fn(&mut RecipeDoc, f64, f64), Option<&str>); 7] = [
            ("离自己的中线差 0.5 px 不到走廊 + 余量", |d, need, bottom| { d.shots[0].path = vec![[300.0, (bottom + need - 0.5) as f32], [980.0, (bottom + need - 0.5) as f32]]; },
                Some("拍照点 P1 图 1 的胶检测走廊")),
            ("离自己的中线比走廊 + 余量多 0.5 px", |d, need, bottom| { d.shots[0].path = vec![[300.0, (bottom + need + 0.5) as f32], [980.0, (bottom + need + 0.5) as f32]]; }, None),
            // 枪上相机里胶越过示教段终点继续在画面里：框在终点外的直线延长上（外圈左沿 x = 596，终点离它比走廊 + 余量还远 20 px）
            ("框在中线终点之外的切向延长上", |d, need, _| { let x = (596.0 - need - 20.0) as f32; d.shots[0].path = vec![[300.0, 295.5], [x, 295.5]]; },
                Some("拍照点 P1 图 1 的胶检测走廊")),
            ("框在终点之外、侧向离延长线比走廊 + 余量多 0.5 px", |d, need, bottom| { let y = (bottom + need + 0.5) as f32; d.shots[0].path = vec![[300.0, y], [560.0, y]]; }, None),
            ("压住同 Pose、同相机、同一幅图的另一拍照点的走廊", |d, _, _| { d.shots[1].pose_id = "P1".into(); d.shots[1].view = 1;
                d.shots[1].path = vec![[300.0, 330.0], [980.0, 330.0]]; }, Some("拍照点 P2 图 1 的胶检测走廊")),
            ("另一拍照点同一位置但 Pose 不同：不在同一场景", |d, _, _| { d.shots[1].view = 1; d.shots[1].path = vec![[300.0, 330.0], [980.0, 330.0]]; }, None),
            ("压走廊的框在停用视图上：不用，不查", |d, _, _| {
                let shot = &mut d.shots[0];
                shot.path = vec![[300.0, 330.0], [980.0, 330.0]];
                let spec = |view: u8, enabled: bool| ShotViewSpec { view, enabled, path: shot.path.clone(), mm_per_px: shot.mm_per_px, detect: None,
                    limits: None, calib: None, fixture: (!enabled).then(|| FixtureSpec { rect: [600, 280, 32, 32], ..fixture_spec() }) };
                shot.views = vec![spec(1, true), spec(2, false)];
                shot.fixture = None;
            }, None),
        ];
        for (name, edit, error) in cases {
            let mut doc = composite_recipe();
            at(&mut doc, 0, line(bottom + 2.0 * need));
            at(&mut doc, 1, line(bottom + 2.0 * need));
            with_box(&mut doc, 0);
            edit(&mut doc, need, bottom);
            match (doc.build(), error) {
                (Ok(_), None) => {}
                (Err(e), Some(what)) => assert!(e.contains("走廊") && e.contains(what), "{name}：{e}"),
                (result, _) => panic!("{name}：{:?}", result.map(|_| ())),
            }
        }
    }

    #[test]
    fn default_end_shift_limit_matches_the_final_design_example_numbers() {
        // 防的回归：临时默认上限漏了方形搜索窗的 √2、贴边门的 2 px 或转角的杠杆臂项，护栏在正常件上触发或放得过宽。
        // 数值取自定稿 §10 决定一：searchRadius 60 px、0.117 mm/px 只找平移约 9.6 mm；±3°、L_端 87 mm 再加约 4.6 mm
        let mut f = fixture_spec();
        f.search_radius_px = 60.0;
        let c = f.teach_center();
        let far = [c[0] + 87.0 / 0.117, c[1]];
        let ends = [[c[0] - 10.0, c[1]], far];
        let still = f.end_shift_limit_mm(ends, 0.117);
        assert!((still - 9.597).abs() < 0.005, "{still}");
        f.angle_range_deg = 3.0;
        let turned = f.end_shift_limit_mm(ends, 0.117);
        assert!((turned - still - 4.555).abs() < 0.005, "{turned}");
    }

    #[test]
    fn camera_imaging_must_cover_exactly_the_shot_cameras_within_limits() {
        fn imaging(d: &mut RecipeDoc, exposure_us: f32, gain_db: f32) { d.cameras.insert("cam1".into(), CameraImaging { exposure_us, gain_db }); }
        let cases: [(&str, fn(&mut RecipeDoc), Option<&str>); 8] = [
            ("拍照点相机都有成像参数", |_| {}, None),
            ("拍照点相机缺成像参数", |d| { d.cameras.clear(); }, Some("没有它的曝光与增益")),
            ("给了没被使用的相机", |d| { d.cameras.insert("cam9".into(), DEFAULT_IMAGING); }, Some("没有拍照点使用它")),
            ("曝光低于 1 µs", |d| imaging(d, 0.5, 6.0), Some("曝光时间")),
            ("曝光超过 1 s", |d| imaging(d, 1_000_001.0, 6.0), Some("曝光时间")),
            ("增益为负", |d| imaging(d, 60.0, -0.5), Some("增益")),
            ("增益不是有限数", |d| imaging(d, 60.0, f32::NAN), Some("增益")),
            ("曝光与增益在边界上", |d| imaging(d, 1_000_000.0, 0.0), None),
        ];
        for (name, edit, error) in cases {
            let mut doc = composite_recipe();
            edit(&mut doc);
            match (doc.build(), error) {
                (Ok(recipe), None) => assert_eq!(recipe.imaging("cam1"), doc.cameras.get("cam1").copied(), "{name}"),
                (Err(e), Some(what)) => assert!(e.contains(what), "{name}: {e}"),
                (result, _) => panic!("{name}: {:?}", result.map(|_| ())),
            }
        }
    }
}
