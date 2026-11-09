//! 配方：磁盘上存可编辑的 `RecipeDoc`，加载时生成运行用的 `Recipe`（分段、测量点、修订标识）。
//! 一期按拍照点在图像里检测（P0 D-10）：每个拍照点示教一条胶路中线（图像像素），沿线每隔 spacing 一站；
//! 每个拍照点自成一段，判定在段内做，段与段之间不连。`Recipe` 同时是检测记录里的配方快照。
//!
//! 分段连拍（schema 8，docs/decisions/segment-burst.md D-SB-1）：`segments` 非空的配方是段配方——沿胶路的段列表、
//! 三台检测相机（`inspect`）、整件标定；`segments` 为空是旧拍照点配方，不能开工或发布，遗留类型不再维护。
//! 注意段列表在 `RecipeDoc.segments`，在 `Recipe` 里是 `path_segments`（JSON `pathSegments`）。

use std::collections::BTreeMap;
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
        let ok = [
            self.nominal,
            self.tol_upper,
            self.tol_lower,
            self.abs_min,
            self.abs_max,
            self.max_excursion_len,
        ]
        .iter()
        .all(|v| v.is_finite());
        if !ok || self.tol_upper < 0.0 || self.tol_lower < 0.0 || self.max_excursion_len < 0.0 {
            return Err(format!(
                "{what}：限值必须是有限数，公差与允许超差长度不能为负"
            ));
        }
        if !(self.abs_min <= self.nominal && self.nominal <= self.abs_max) {
            return Err(format!(
                "{what}：名义值 {:.2} 要在绝对限 [{:.2}, {:.2}] 之内",
                self.nominal, self.abs_min, self.abs_max
            ));
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
            let gauge = self.gauge_ref.as_deref().map(str::trim).is_some_and(|g| {
                !g.is_empty() && g.chars().count() <= 64 && !g.chars().any(char::is_control)
            });
            if p.nominal != 0.0 && !gauge {
                return Err(format!(
                    "{what}：位置名义值不为 0 时必须填写量具确认件编号（gaugeRef，最长 64 个字符）"
                ));
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
    /// 第 `segment` 段的检测参数：段设了搜索半宽、胶宽范围就用段的，否则用这份配方默认；段不存在时就是配方默认。
    pub fn for_segment(&self, segments: &[PathSegment], segment: u32) -> DetectParams {
        segments
            .iter()
            .find(|s| s.index == segment)
            .map_or_else(|| self.clone(), |s| s.detect(self))
    }

    pub(crate) fn validate(&self, what: &str) -> Result<(), String> {
        let [lo, hi] = self.width_range;
        if !(self.search_mm.is_finite()
            && self.search_mm > 0.0
            && lo.is_finite()
            && hi.is_finite()
            && lo > 0.0
            && lo < hi)
        {
            return Err(format!(
                "{what}：搜索半宽需为正，胶宽范围需满足 0 < 下限 < 上限"
            ));
        }
        if hi >= 2.0 * self.search_mm {
            return Err(format!(
                "{what}：胶宽上限 {hi:.1} mm 要小于搜索宽度 {:.1} mm",
                2.0 * self.search_mm
            ));
        }
        Ok(())
    }
}

/// 相机、配方编号：字母、数字、- 和 _，最长 32 个字符。
pub fn valid_camera_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 配方文件格式版本。不一致的文件列为加载错误，不迁移。8：增加段模型字段（segments、inspect、segmentDefaults、partCalib）。
pub const RECIPE_SCHEMA: u32 = 8;
/// 没有段的配方文件是已停用的旧拍照点配方。
pub const LEGACY_RECIPE: &str = "旧拍照点配方已停用，请新建段配方";

/// 比例分段边界两侧各留的过渡帧数（D-SB-40）：现场同一程序各轮边界偏差最差约 2 帧。
pub const DEFAULT_BOUNDARY_GUARD: u32 = 2;
/// 过渡帧数的可配范围。
pub const BOUNDARY_GUARD: std::ops::RangeInclusive<u32> = 0..=5;

fn default_boundary_guard() -> u32 {
    DEFAULT_BOUNDARY_GUARD
}

/// 冻结示教原图编号（模板来源）：字母、数字和 -，最长 128 个字符。工作台的冻结图编号都满足。
pub fn valid_image_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// 配方阶段（在线投运）：采集版只定了三台计划相机和曝光增益，供维护模式跟着生产采图；正式版走完整发布门槛。
/// 旧文件没有这一项，按正式版读；正式版不写这一项，旧版本程序照常读取。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipeStage {
    #[default]
    Formal,
    Collect,
}

impl RecipeStage {
    pub fn is_formal(&self) -> bool {
        *self == Self::Formal
    }
}

/// 段配方里一台检测相机的参数（D-SB-45）：像素当量、它在各段的走廊、逐帧定向的测量区间与出胶点。标定一律用设备标定。
/// `RecipeDoc.inspect` 的顺序就是计划相机顺序（PLC 一个触发同时触发这三台）。
/// 不拒绝未知字段：已停用的 `corridor`、`sectorDeg`、`calib`、`detect`、`expert` 读入时丢弃。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraInspect {
    pub camera: String,
    /// 像素当量（mm/px）；为空为尚未设置
    #[serde(default)]
    pub mm_per_px: Option<f64>,
    /// 这台相机在它检测的各段的走廊（D-SB-42/45）：落在该段的帧用它测量，没有相机级的默认走廊
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<SegmentCorridor>,
    /// 逐帧定向的测量区间（D-SB-47）：这台相机各段的默认值，段可在 `HeadingSettings.window` 单独覆盖；为空时用
    /// `HeadingWindow::DEFAULT`，见 `heading::window`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<HeadingWindow>,
    /// 标定的出胶点（图像像素，D-SB-47）；为空时由开启定向的各段走廊起点推出
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle: Option<[f64; 2]>,
}

/// 逐帧定向的测量区间（mm，沿胶路从出胶点量起）：只测 [start, start + length]。界面填“跳过前 start”和“测到 start + length”。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeadingWindow {
    pub start_mm: f64,
    pub length_mm: f64,
}

impl HeadingWindow {
    /// 段与相机都没设测量区间时用它：跳过前 5 mm，测到 20 mm。
    pub const DEFAULT: Self = Self {
        start_mm: 5.0,
        length_mm: 15.0,
    };

    /// 起点 ≥ 0、长度 > 0、终点不超过 500 mm。`what` 是报错主语。
    pub fn validate(&self, what: &str) -> Result<(), String> {
        if self.start_mm.is_finite()
            && self.length_mm.is_finite()
            && self.start_mm >= 0.0
            && self.length_mm > 0.0
            && self.start_mm + self.length_mm <= 500.0
        {
            Ok(())
        } else {
            Err(format!(
                "{what}的测量区间需满足 跳过距离 ≥ 0、测量到的距离大于跳过距离且不超过 500 mm"
            ))
        }
    }
}

/// 一台相机在某一段（`PathSegment.index`）的走廊（图像像素，只决定 ROI 和测站）；少于 2 点为尚未示教。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SegmentCorridor {
    pub segment: u32,
    #[serde(default)]
    pub corridor: Vec<[f32; 2]>,
    /// 这台相机在这一段的逐帧定向搜索扇区中线方向（度，图像坐标，D-SB-47），在这一段的示教帧上拖动扇区得出；
    /// 为空时取背离离出胶点最近的画面边
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sector_deg: Option<f64>,
    /// 这台相机在这一段的颜色模型（`glue.ColorModel` 的 JSON 原文）：彩色示教帧上沿走廊取色得出，按胶色测量时用
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_model: Option<String>,
    /// 操作员选的测量方式；没选时有颜色模型按胶色、否则按灰度（[`SegmentCorridor::measure_mode`]）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<MeasureMode>,
}

/// 一台相机在一段按什么测胶条（color CL-3）：灰度图找暗 / 亮段，或按示教取得的胶色。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeasureMode {
    Gray,
    Color,
}

impl SegmentCorridor {
    pub fn measure_mode(&self) -> MeasureMode {
        self.measure.unwrap_or(if self.color_model.is_some() {
            MeasureMode::Color
        } else {
            MeasureMode::Gray
        })
    }

    /// 测量用的颜色模型：按灰度测为 None；选了按胶色测却还没取色时报错。
    pub fn measuring_color_model(&self) -> Result<Option<&str>, String> {
        match self.measure_mode() {
            MeasureMode::Gray => Ok(None),
            MeasureMode::Color => self.color_model.as_deref().map(Some).ok_or_else(|| {
                "选了按胶色测量，还没取色：在走廊示教页取色，或改按灰度测量".to_string()
            }),
        }
    }
}

impl CameraInspect {
    /// 第 `segment` 段的走廊；还没示教时为 None。
    pub fn segment_corridor(&self, segment: u32) -> Option<&SegmentCorridor> {
        self.segments.iter().find(|s| s.segment == segment)
    }

    /// 第 `segment` 段的走廊已示教：至少 2 点、总长不短于 1 px。
    pub fn segment_taught(&self, segment: u32) -> bool {
        self.segment_corridor(segment).is_some_and(|s| {
            let length: f32 = s
                .corridor
                .windows(2)
                .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
                .sum();
            s.corridor.len() >= 2 && length >= 1.0
        })
    }

    fn validate(&self) -> Result<(), String> {
        let id = &self.camera;
        if !valid_camera_id(id) {
            return Err(format!(
                "检测相机编号 {id:?} 只能用字母、数字、- 和 _，最长 32 个字符"
            ));
        }
        for (i, s) in self.segments.iter().enumerate() {
            if s.corridor.len() > 10_000 || s.corridor.iter().flatten().any(|v| !v.is_finite()) {
                return Err(format!(
                    "相机 {id} 第 {} 段的走廊最多 10000 点，坐标必须是有限数",
                    s.segment
                ));
            }
            if self.segments[..i].iter().any(|o| o.segment == s.segment) {
                return Err(format!("相机 {id} 第 {} 段的走廊重复", s.segment));
            }
            if s.sector_deg
                .is_some_and(|d| !(d.is_finite() && d.abs() <= 360.0))
            {
                return Err(format!(
                    "相机 {id} 第 {} 段的搜索扇区方向需为 −360°–360° 的有限数",
                    s.segment
                ));
            }
        }
        if let Some(m) = self
            .mm_per_px
            .filter(|m| !(m.is_finite() && *m > 0.0 && *m <= 10.0))
        {
            return Err(format!("相机 {id} 的像素当量 {m} 需在 0–10 mm/px 之间"));
        }
        if let Some(w) = &self.window {
            w.validate(&format!("相机 {id} "))?;
        }
        if self.nozzle.is_some_and(|p| {
            p.iter()
                .any(|v| !(v.is_finite() && *v >= 0.0 && *v <= 100_000.0))
        }) {
            return Err(format!("相机 {id} 的出胶点坐标需为 0–100000 px 的有限数"));
        }
        Ok(())
    }
}

/// 每一段、它的每一台检测相机都示教了走廊，每台相机都有像素当量（D-SB-45）；没有时逐条列出缺的。
pub fn untaught_corridors(segments: &[PathSegment], inspect: &[CameraInspect]) -> Vec<String> {
    let cameras: Vec<String> = inspect.iter().map(|i| i.camera.clone()).collect();
    let mut reasons: Vec<String> = inspect
        .iter()
        .filter(|i| i.mm_per_px.is_none())
        .map(|i| format!("相机 {} 缺少像素当量", i.camera))
        .collect();
    for segment in segments {
        for camera in plan::segment_cameras(segment, &cameras) {
            if !inspect
                .iter()
                .any(|i| i.camera == camera && i.segment_taught(segment.index))
            {
                reasons.push(format!(
                    "第 {} 段的相机 {camera} 尚未示教走廊",
                    segment.index
                ));
            } else if let Some(Err(reason)) = inspect
                .iter()
                .find(|i| i.camera == camera)
                .and_then(|i| i.segment_corridor(segment.index))
                .map(|c| c.measuring_color_model())
            {
                reasons.push(format!("第 {} 段的相机 {camera}{reason}", segment.index));
            }
        }
    }
    reasons
}

/// 反序列化把 JSON（快照、界面 layout）里的 `pathSegments` 还原成 `RecipeDoc.segments` 后重新 build（见 `TryFrom<serde_json::Value>`）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "serde_json::Value")]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub revision_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teaching_id: Option<String>,
    pub product_code: u16,
    #[serde(default, skip_serializing_if = "RecipeStage::is_formal")]
    pub stage: RecipeStage,
    pub trigger_mode: TriggerMode,
    pub schema_version: u32,
    /// 站距（mm）
    pub spacing: f32,
    pub filter_window: usize,
    /// 检测参数的配方默认（段可覆盖搜索半宽与胶宽范围，见 `DetectParams::for_segment`）与限值
    pub detect: DetectParams,
    pub limits: ShotLimits,
    /// 段配方的段列表（RecipeDoc.segments）；为空即旧拍照点配方
    pub path_segments: Vec<PathSegment>,
    /// 段配方的检测相机，顺序即计划相机顺序
    pub inspect: Vec<CameraInspect>,
    pub segment_defaults: KindDefaults,
    pub boundary_guard: u32,
    pub pass_skip: crate::passes::PassSkip,
    pub pass_frames: Option<Vec<u32>>,
    pub part_calib: Option<PartCalib>,
    /// 快照里已停用的测量覆盖（配方级 `expert`、各相机的 `detect` 与 `expert`，都为空时为 null）：这些值不再生效，
    /// 按它们测的已存指标与当前口径不同，进测量布局（`store::measurement_layout`），与没有它们的配方比较时要原图复测
    #[serde(skip)]
    pub retired_overrides: serde_json::Value,
}

impl Recipe {
    /// 段配方（分段连拍）：段列表非空。否则是旧拍照点配方。
    pub fn is_burst(&self) -> bool {
        !self.path_segments.is_empty()
    }

    /// 一件按笔判定的规则（D-SB-62）：`part` 是本件的笔结构（不知道为空）。
    pub fn pass_rules<'a>(&'a self, part: &'a [u32]) -> crate::passes::PassRules<'a> {
        crate::passes::PassRules {
            part,
            taught: self.pass_frames.as_deref(),
            skip: self.pass_skip,
        }
    }

    /// 段配方的计划相机（按 inspect 顺序）：PLC 一个触发同时触发它们。旧拍照点配方为空。
    pub fn plan_cameras(&self) -> Vec<String> {
        self.inspect.iter().map(|i| i.camera.clone()).collect()
    }

    /// 采集版：只能在维护模式下采图，不能当正式检测运行。
    pub fn is_collect(&self) -> bool {
        self.stage == RecipeStage::Collect
    }

    /// 整件预期帧组数 ΣN_exp（各段都已标定取其和，否则取整件标定）；未标定为 None。
    pub fn expected_total(&self) -> Option<u32> {
        plan::expected_total(&self.path_segments, self.part_calib.as_ref())
    }

    /// 段配方能不能发布生产（D-SB-20）：每段每台检测相机都示教了走廊、相机有像素当量，且已标定——单段配方凭整件
    /// 预期帧组数，多段配方要各段 N_exp 都已标定（D-SB-45：要按累计 N_exp 预测每帧所属段来选走廊）。不能时逐条列出原因。
    /// `Recipe::ready` 保留给旧拍照点配方，段配方过不了它，旧发布与旧节拍因此不会接受段配方。
    pub fn ready_burst(&self) -> Result<(), Vec<String>> {
        let mut reasons: Vec<String> = Vec::new();
        if !self.is_burst() {
            reasons.push(format!("配方 {} 没有段，是旧拍照点配方", self.id));
        }
        reasons.extend(untaught_corridors(&self.path_segments, &self.inspect));
        reasons.extend(
            crate::heading::issues(&self.path_segments, &self.inspect)
                .into_iter()
                .map(|i| i.message),
        );
        if self.expected_total().is_none() || self.path_segments.len() > 1 {
            reasons.extend(
                self.path_segments
                    .iter()
                    .filter(|s| s.expected_frames.is_none())
                    .map(|s| {
                        let kind = match s.kind {
                            SegmentKind::Straight => "直线段",
                            SegmentKind::Corner => "弯道段",
                        };
                        format!(
                            "第 {} 段（{kind}）没有预期帧数：先在分段步骤确认示教件边界",
                            s.index
                        )
                    }),
            );
        }
        if reasons.is_empty() {
            Ok(())
        } else {
            Err(reasons)
        }
    }

    pub(crate) fn candidate_revision(&self) -> Option<u64> {
        let suffix = self
            .revision_id
            .strip_prefix(&format!("candidate-{}-i", self.id))?;
        let (instance, revision) = suffix.split_once("-r")?;
        let parsed = instance.parse::<u64>().ok()?;
        if parsed == 0 || instance != parsed.to_string() {
            return None;
        }
        revision
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0 && revision == value.to_string())
    }

    pub(crate) fn from_snapshot(value: serde_json::Value) -> Result<Self, String> {
        let revision = value
            .get("revisionId")
            .and_then(serde_json::Value::as_str)
            .ok_or("配方快照缺少修订号")?
            .to_owned();
        let mut recipe: Self = serde_json::from_value(value).map_err(|error| error.to_string())?;
        let production_revision = recipe.revision_id.clone();
        recipe.revision_id = revision;
        if recipe.revision_id != production_revision && recipe.candidate_revision().is_none() {
            return Err("配方快照修订号与身份不一致".into());
        }
        Ok(recipe)
    }
    /// 本配方要用到的相机（编号），即计划相机。
    pub fn cameras(&self) -> Vec<String> {
        self.plan_cameras()
    }

    /// 第 `segment` 段的检测参数（见 `DetectParams::for_segment`）。
    pub fn segment_detect(&self, segment: u32) -> DetectParams {
        self.detect.for_segment(&self.path_segments, segment)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teaching_id: Option<String>,
    pub product_code: u16,
    /// 采集版或正式版；缺省为正式版
    #[serde(default, skip_serializing_if = "RecipeStage::is_formal")]
    pub stage: RecipeStage,
    /// 段配方固定为 Fly（D-SB-23）；Stop（停稳拍）只供旧拍照点配方
    pub trigger_mode: TriggerMode,
    /// 缺省为 0，校验时报格式版本不符
    #[serde(default)]
    pub schema_version: u32,
    pub spacing: f32,
    pub filter_window: usize,
    /// 检测参数的配方默认：极性只在这里设，搜索半宽与胶宽范围段可覆盖（`PathSegment.search_mm` / `width_range`）
    pub detect: DetectParams,
    pub limits: ShotLimits,
    /// 沿胶路的段列表（规格 §1）；为空即旧拍照点配方，下面几项段模型字段不允许设置
    #[serde(default)]
    pub segments: Vec<PathSegment>,
    /// 三台检测相机（有序，计划相机顺序取它）
    #[serde(default)]
    pub inspect: Vec<CameraInspect>,
    /// 新建段时取用的直线 / 弯道默认检测项
    #[serde(default)]
    pub segment_defaults: KindDefaults,
    /// 比例分段边界两侧的过渡帧数，0..=5（D-SB-40）；不进计划身份
    #[serde(default = "default_boundary_guard")]
    pub boundary_guard: u32,
    /// 每笔开头、结尾跳过的帧组数（D-SB-62）；不进计划身份
    #[serde(default, skip_serializing_if = "crate::passes::PassSkip::is_zero")]
    pub pass_skip: crate::passes::PassSkip,
    /// 示教件的笔结构（各笔帧组数，D-SB-62）：随 N_exp 由示教件得出，保存时不采用传入值；None = 不按笔分段
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass_frames: Option<Vec<u32>>,
    /// 整件帧组数标定：各段 N_exp 待分段算法时凭它得到 expected_total 并发布（决策补充第二节）；只由标定写入
    #[serde(default)]
    pub part_calib: Option<PartCalib>,
    /// 示教件来源（RB-7）：正式发布时从候选写入；候选里恒为空，没有它的生产配方即旧流程发布。不进计划身份
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teaching_piece: Option<TeachingPieceRef>,
}

/// 配方文档记下的示教件来源：历史件的工件身份、记录、SN 与时间。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeachingPieceRef {
    pub cycle_id: String,
    pub record_id: i64,
    pub sn: String,
    pub ts: i64,
}

impl RecipeDoc {
    /// 段配方：段列表非空。
    pub fn is_burst(&self) -> bool {
        !self.segments.is_empty()
    }

    /// 段配方的计划相机（按 inspect 顺序）。
    pub fn plan_cameras(&self) -> Vec<String> {
        self.inspect.iter().map(|i| i.camera.clone()).collect()
    }

    /// 一件按笔判定的规则（D-SB-62）：`part` 是这一件的笔结构（不知道为空）。
    pub fn pass_rules<'a>(&'a self, part: &'a [u32]) -> crate::passes::PassRules<'a> {
        crate::passes::PassRules {
            part,
            taught: self.pass_frames.as_deref(),
            skip: self.pass_skip,
        }
    }

    /// 整件预期帧组数 ΣN_exp（见 `plan::expected_total`）。
    pub fn expected_total(&self) -> Option<u32> {
        plan::expected_total(&self.segments, self.part_calib.as_ref())
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != RECIPE_SCHEMA {
            return Err(format!(
                "配方文件格式版本 {}，当前为 {RECIPE_SCHEMA}，需要按新格式重建",
                self.schema_version
            ));
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
        if !self.is_burst() {
            return Err(LEGACY_RECIPE.into());
        }
        self.validate_burst()
    }

    /// 段配方的模型约束（D-SB-1）：只支持飞拍；检测相机各自合法；
    /// 段列表与三台计划相机满足 `plan::validate_segments`；直线 / 弯道默认参数满足 `plan::validate_defaults`（新建段取用它）；
    /// 整件标定的帧组数不超过上限。
    fn validate_burst(&self) -> Result<(), String> {
        if self.trigger_mode != TriggerMode::Fly {
            return Err("段配方只支持飞拍（triggerMode 必须为 fly）".into());
        }
        if !BOUNDARY_GUARD.contains(&self.boundary_guard) {
            return Err(format!(
                "边界过渡帧数 {} 需在 {}–{} 之间",
                self.boundary_guard,
                BOUNDARY_GUARD.start(),
                BOUNDARY_GUARD.end()
            ));
        }
        if self.pass_skip.head > crate::passes::MAX_SKIP
            || self.pass_skip.tail > crate::passes::MAX_SKIP
        {
            return Err(format!(
                "每笔首尾跳过的帧组数需在 0–{} 之间",
                crate::passes::MAX_SKIP
            ));
        }
        if self
            .pass_frames
            .as_ref()
            .is_some_and(|frames| frames.is_empty() || frames.contains(&0))
        {
            return Err("示教件的笔结构无效：每笔至少一个帧组".into());
        }
        for inspect in &self.inspect {
            inspect.validate()?;
            let cameras = self.plan_cameras();
            if let Some(s) = inspect.segments.iter().find(|s| {
                !self.segments.iter().any(|p| {
                    p.index == s.segment
                        && plan::segment_cameras(p, &cameras).contains(&inspect.camera)
                })
            }) {
                return Err(format!(
                    "相机 {} 的走廊指向第 {} 段，但该段不存在或不由这台相机检测",
                    inspect.camera, s.segment
                ));
            }
        }
        plan::validate_segments(&self.segments, &self.plan_cameras())?;
        for segment in &self.segments {
            segment
                .detect(&self.detect)
                .validate(&format!("第 {} 段检测参数", segment.index))?;
        }
        plan::validate_defaults(&self.segment_defaults)?;
        if let Some(part) = &self.part_calib {
            if !(1..=plan::MAX_EXPECTED_TOTAL).contains(&part.expected_total) {
                return Err(format!(
                    "整件标定的预期帧组数 {} 需在 1–{} 之间（PLC shotCount）",
                    part.expected_total,
                    plan::MAX_EXPECTED_TOTAL
                ));
            }
        }
        Ok(())
    }

    pub fn build(&self) -> Result<Recipe, String> {
        self.validate()?;
        let recipe = Recipe {
            id: self.id.clone(),
            name: self.name.trim().to_string(),
            version: self.version.max(1),
            revision_id: format!("{}-v{}", self.id, self.version.max(1)),
            teaching_id: self.teaching_id.clone(),
            product_code: self.product_code,
            stage: self.stage,
            trigger_mode: self.trigger_mode,
            schema_version: self.schema_version,
            spacing: self.spacing,
            filter_window: self.filter_window,
            detect: self.detect.clone(),
            limits: self.limits.clone(),
            path_segments: self.segments.clone(),
            inspect: self.inspect.clone(),
            segment_defaults: self.segment_defaults.clone(),
            boundary_guard: self.boundary_guard,
            pass_skip: self.pass_skip,
            pass_frames: self.pass_frames.clone(),
            part_calib: self.part_calib.clone(),
            retired_overrides: serde_json::Value::Null,
        };
        Ok(recipe)
    }
}

impl TryFrom<RecipeDoc> for Recipe {
    type Error = String;
    fn try_from(doc: RecipeDoc) -> Result<Self, Self::Error> {
        doc.build()
    }
}

impl TryFrom<serde_json::Value> for Recipe {
    type Error = String;
    /// Recipe 的 JSON 还原成 RecipeDoc 再 build：段列表在 `pathSegments`，换回 RecipeDoc 的 `segments` 再解析。
    fn try_from(mut value: serde_json::Value) -> Result<Self, Self::Error> {
        let filled = |v: &serde_json::Value| {
            (!v.is_null() && v.as_object().is_none_or(|o| !o.is_empty())).then(|| v.clone())
        };
        let cameras: Vec<_> = value["inspect"]
            .as_array()
            .map(|cameras| {
                cameras
                    .iter()
                    .map(|i| (filled(&i["detect"]), filled(&i["expert"])))
                    .collect()
            })
            .unwrap_or_default();
        let expert = filled(&value["expert"]);
        let retired_overrides =
            if expert.is_none() && cameras.iter().all(|(d, e)| d.is_none() && e.is_none()) {
                serde_json::Value::Null
            } else {
                serde_json::json!([expert, cameras])
            };
        let object = value.as_object_mut().ok_or("配方快照不是 JSON 对象")?;
        let path_segments = object
            .remove("pathSegments")
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
        object.insert("segments".into(), path_segments);
        let mut recipe = serde_json::from_value::<RecipeDoc>(value)
            .map_err(|e| e.to_string())?
            .build()?;
        recipe.retired_overrides = retired_overrides;
        Ok(recipe)
    }
}

/// 胶宽、位置、断胶的默认限值：名义胶宽 4 mm；断口阈值取自 MX11 现场图（正常帧最长无胶 5.4 mm，断胶帧 10–22 mm）。
pub fn default_limits() -> ShotLimits {
    ShotLimits {
        position: Some(JudgeParams {
            nominal: 0.0,
            tol_upper: 2.0,
            tol_lower: 2.0,
            abs_min: -5.0,
            abs_max: 5.0,
            max_excursion_len: 5.0,
        }),
        width: Some(JudgeParams {
            nominal: 4.0,
            tol_upper: 1.5,
            tol_lower: 1.5,
            abs_min: 1.0,
            abs_max: 8.0,
            max_excursion_len: 5.0,
        }),
        max_gap_len: 6.0,
        min_present: default_min_present(),
        gauge_ref: None,
    }
}

pub fn default_detect() -> DetectParams {
    DetectParams {
        search_mm: 15.0,
        polarity: Polarity::Dark,
        width_range: [1.0, 10.0],
    }
}

/// 模拟相机的像素当量（与模拟出图一致）。
pub const SIM_MM_PER_PX: f32 = 0.112;

/// 模拟相机画面里的示例中线：胶嘴在右侧，胶条往左拖出（直、缓弯、斜向交替）。
#[cfg(test)]
fn sample_path(k: usize) -> Vec<[f32; 2]> {
    match k % 3 {
        0 => vec![[980.0, 480.0], [300.0, 460.0]],
        1 => vec![[980.0, 480.0], [700.0, 420.0], [320.0, 330.0]],
        _ => vec![[980.0, 480.0], [640.0, 560.0], [330.0, 690.0]],
    }
}

/// 段配方的起点（D-SB-24）：飞拍、一个直线段（待标定，检测项取配方默认）、按给定顺序的检测相机（走廊待示教）、
/// 相机不足 3 台时照样返回，但过不了段配方校验。
pub fn burst_template(
    id: String,
    name: String,
    product_code: u16,
    cameras: &[String],
) -> RecipeDoc {
    let segment_defaults = KindDefaults::default();
    RecipeDoc {
        id,
        name,
        version: 1,
        teaching_id: None,
        product_code,
        stage: RecipeStage::Formal,
        trigger_mode: TriggerMode::Fly,
        schema_version: RECIPE_SCHEMA,
        spacing: 1.0,
        filter_window: 5,
        detect: default_detect(),
        limits: default_limits(),
        segments: vec![plan::new_segment(
            1,
            SegmentKind::Straight,
            &segment_defaults,
        )],
        inspect: cameras
            .iter()
            .map(|camera| CameraInspect {
                camera: camera.clone(),
                mm_per_px: None,
                segments: Vec::new(),
                window: None,
                nozzle: None,
            })
            .collect(),
        segment_defaults,
        boundary_guard: DEFAULT_BOUNDARY_GUARD,
        pass_skip: crate::passes::PassSkip::default(),
        pass_frames: None,
        part_calib: None,
        teaching_piece: None,
    }
}

/// 测试用的段配方：模拟相机 cam1–cam3 共用触发、走廊已示教、一个直线段 N_exp = 4（ΣN_exp = 4）、不开逐帧定向。
#[cfg(test)]
pub fn samples_burst() -> RecipeDoc {
    let cameras: Vec<String> = (1..=3).map(|k| format!("cam{k}")).collect();
    let mut doc = burst_template("SEAM-BURST".into(), "分段连拍样例".into(), 14, &cameras);
    for inspect in &mut doc.inspect {
        inspect.segments = vec![SegmentCorridor {
            segment: 1,
            corridor: sample_path(0),
            sector_deg: None,
            color_model: None,
            measure: None,
        }];
        inspect.mm_per_px = Some(f64::from(SIM_MM_PER_PX));
    }
    doc.segments[0].expected_frames = Some(4);
    doc.segments[0].heading = None;
    doc
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
        if serde_json::from_str::<RecipeDoc>(&text)
            .ok()
            .is_some_and(|doc| doc.build().is_ok())
        {
            return None;
        }
        let value = serde_json::from_str::<serde_json::Value>(&text).ok();
        Some(Self {
            file_name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .into(),
            id: value
                .as_ref()
                .and_then(|v| v.get("id")?.as_str())
                .map(str::to_owned),
            name: value
                .as_ref()
                .and_then(|v| v.get("name")?.as_str())
                .map(str::to_owned),
            schema_version: value
                .as_ref()
                .and_then(|v| v.get("schemaVersion")?.as_u64())
                .and_then(|v| u32::try_from(v).ok()),
            current_schema_version: RECIPE_SCHEMA,
            reason,
        })
    }
}

pub const COLLECT_AFTER_FORMAL: &str =
    "已有正式版，不能再发布采集版：正式版不降级，请继续示教、标定后发布新的正式版";

/// 磁盘上的配方库：每个配方一个 `<id>.json`。
pub struct RecipeStore {
    dir: PathBuf,
    inner: RwLock<Vec<(RecipeDoc, Arc<Recipe>)>>,
    errors: RwLock<Vec<String>>,
    unavailable: RwLock<Vec<UnavailableRecipe>>,
}

impl RecipeStore {
    #[cfg(test)]
    pub fn open(root: PathBuf) -> Result<Self, String> {
        Self::open_with_floors(root.join("recipes"), &BTreeMap::new())
    }

    pub fn open_with_floors(dir: PathBuf, floors: &BTreeMap<String, u32>) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建配方目录失败：{e}"))?;
        let store = Self {
            dir,
            inner: RwLock::new(Vec::new()),
            errors: RwLock::new(Vec::new()),
            unavailable: RwLock::new(Vec::new()),
        };
        store.seed_version_floor(floors)?;
        store.retire_outdated_sources()?;
        store.revision_sources()?;
        store.reload();
        Ok(store)
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn retired_versions(&self) -> Result<BTreeMap<String, u32>, String> {
        match crate::fsio::read_text(&self.dir.join(".revision-versions")) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("配方版本记录损坏，拒绝复用修订号：{e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(format!("读取配方版本记录失败：{e}")),
        }
    }

    fn revision_sources(&self) -> Result<BTreeMap<String, RecipeDoc>, String> {
        let sources: BTreeMap<String, RecipeDoc> =
            match crate::fsio::read_text(&self.dir.join(".revision-sources")) {
                Ok(text) => serde_json::from_str(&text)
                    .map_err(|e| format!("配方修订来源记录损坏，拒绝复用修订号：{e}"))?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
                Err(e) => return Err(format!("读取配方修订来源记录失败：{e}")),
            };
        for (id, doc) in &sources {
            if id != &doc.id.to_ascii_lowercase() || doc.version == 0 {
                return Err("配方修订来源记录身份不一致，拒绝复用修订号".into());
            }
            doc.build()
                .map_err(|e| format!("配方修订来源记录无效：{e}"))?;
        }
        Ok(sources)
    }

    /// 旧格式的修订来源无法再证明配方内容（D-0）：原文件备份后移除，其修订号并入版本记录，仍不复用。
    fn retire_outdated_sources(&self) -> Result<(), String> {
        let path = self.dir.join(".revision-sources");
        let text = match crate::fsio::read_text(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("读取配方修订来源记录失败：{e}")),
        };
        let mut sources: BTreeMap<String, RecipeDoc> = serde_json::from_str(&text)
            .map_err(|e| format!("配方修订来源记录损坏，拒绝复用修订号：{e}"))?;
        let outdated: Vec<RecipeDoc> = sources
            .values()
            .filter(|d| d.schema_version != RECIPE_SCHEMA)
            .cloned()
            .collect();
        if outdated.is_empty() {
            return Ok(());
        }
        std::fs::copy(
            &path,
            self.dir.join(format!(
                ".revision-sources.outdated-{}",
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            )),
        )
        .map_err(|e| format!("备份旧格式配方修订来源记录失败：{e}"))?;
        for doc in &outdated {
            self.retire_version(&doc.id, doc.version)?;
            sources.remove(&doc.id.to_ascii_lowercase());
        }
        crate::fsio::write_atomic(
            &path,
            &serde_json::to_string(&sources).map_err(|e| e.to_string())?,
        )
    }

    fn remember_source(&self, doc: &RecipeDoc) -> Result<(), String> {
        let mut sources = self.revision_sources()?;
        let id = doc.id.to_ascii_lowercase();
        if let Some(previous) = sources.get(&id) {
            if previous == doc {
                return Ok(());
            }
            if previous.version >= doc.version {
                return Err("同一修订号不能记录不同的实际配方来源".into());
            }
        }
        sources.insert(id, doc.clone());
        crate::fsio::write_atomic(
            &self.dir.join(".revision-sources"),
            &serde_json::to_string(&sources).map_err(|e| e.to_string())?,
        )
    }

    fn retire_version(&self, id: &str, version: u32) -> Result<(), String> {
        let mut versions = self.retired_versions()?;
        let recorded = versions.entry(id.to_ascii_lowercase()).or_default();
        *recorded = (*recorded).max(version);
        crate::fsio::write_atomic(
            &self.dir.join(".revision-versions"),
            &serde_json::to_string(&versions).map_err(|e| e.to_string())?,
        )
    }

    fn version_after(&self, id: &str, active: Option<u32>) -> Result<u32, String> {
        let retired = self
            .retired_versions()?
            .get(&id.to_ascii_lowercase())
            .copied()
            .unwrap_or(0);
        let source = self
            .revision_sources()?
            .get(&id.to_ascii_lowercase())
            .map_or(0, |doc| doc.version);
        retired
            .max(source)
            .max(active.unwrap_or(0))
            .checked_add(1)
            .ok_or_else(|| "配方版本已达上限".into())
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
            crate::fsio::write_atomic(
                &self.dir.join(".revision-versions"),
                &serde_json::to_string(&versions).map_err(|e| e.to_string())?,
            )?;
        }
        Ok(())
    }

    pub fn next_version(&self, id: &str) -> Result<u32, String> {
        let inner = self.inner.read().unwrap();
        self.version_after(
            id,
            inner
                .iter()
                .find(|(doc, _)| doc.id.eq_ignore_ascii_case(id))
                .map(|(doc, _)| doc.version),
        )
    }

    fn write(&self, doc: &RecipeDoc) -> Result<(), String> {
        crate::fsio::write_atomic(
            &self.file(&doc.id),
            &serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?,
        )
    }

    fn rollback_new_file(&self, doc: &RecipeDoc) -> Result<(), String> {
        let path = self.file(&doc.id);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("新配方回滚无法检查文件，已保留：{error}")),
        };
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("新配方文件已变为 reparse point，回滚拒绝删除".into());
            }
        }
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("新配方文件类型已变化，回滚拒绝删除".into());
        }
        let actual = crate::fsio::read_text(&path)
            .map_err(|error| format!("新配方回滚无法读取文件，已保留：{error}"))
            .and_then(|text| {
                serde_json::from_str::<RecipeDoc>(&text)
                    .map_err(|error| format!("新配方回滚无法确认实际内容，已保留：{error}"))
            })?;
        if &actual != doc {
            return Err("新配方实际内容已被外部修改，回滚拒绝删除".into());
        }
        std::fs::remove_file(&path)
            .map_err(|error| format!("新配方回滚删除失败，文件已保留，请核查后重试：{error}"))
    }

    fn write_with_source(&self, doc: &RecipeDoc, new_file: bool) -> Result<(), String> {
        if new_file {
            match std::fs::symlink_metadata(self.file(&doc.id)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("无法确认新配方文件是否存在，拒绝覆盖：{error}")),
                Ok(_) => return Err("新配方目标已有文件，拒绝覆盖或删除".into()),
            }
        }
        self.write(doc)?;
        if let Err(error) = self.remember_source(doc) {
            if new_file {
                if let Err(rollback) = self.rollback_new_file(doc) {
                    return Err(format!("{error}；{rollback}"));
                }
                return Err(format!("{error}；本次新建配方文件已回滚，可直接重试"));
            }
            return Err(error);
        }
        Ok(())
    }

    /// 重读目录。坏文件跳过并记下原因，不影响其他配方。
    pub fn reload(&self) {
        let mut inner = self.inner.write().unwrap();
        let provenance = self
            .retired_versions()
            .and_then(|versions| self.revision_sources().map(|sources| (versions, sources)));
        let (versions, sources) = match provenance {
            Ok(provenance) => provenance,
            Err(error) => {
                *self.errors.write().unwrap() = vec![error];
                return;
            }
        };
        let mut list = Vec::new();
        let mut errors = Vec::new();
        let mut unavailable = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            let mut paths: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect();
            paths.sort();
            for p in paths {
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                let stem = p
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
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
                    match list.iter().find(|(o, _): &&(RecipeDoc, Arc<Recipe>)| {
                        o.id.eq_ignore_ascii_case(&d.id) || o.product_code == d.product_code
                    }) {
                        Some((o, _)) if o.product_code == d.product_code => Err(format!(
                            "产品代码 {} 与配方 {} 重复，没有加载",
                            d.product_code, o.id
                        )),
                        Some((o, _)) => Err(format!("配方编号与 {} 重复，没有加载", o.id)),
                        None => Ok((d, r)),
                    }
                });
                match checked {
                    Ok((mut doc, mut recipe)) => {
                        let id = doc.id.to_ascii_lowercase();
                        let source = sources.get(&id);
                        let floor = versions
                            .get(&id)
                            .copied()
                            .unwrap_or(0)
                            .max(source.map_or(0, |previous| previous.version));
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
                            Err(error) => errors
                                .push(format!("{name}：修订来源未能持久保存，没有加载：{error}")),
                        }
                    }
                    Err(e) => {
                        errors.push(format!("{name}：{e}"));
                        if let Some(item) = UnavailableRecipe::from_file(&p, e) {
                            unavailable.push(item);
                        }
                    }
                }
            }
        }
        *inner = list;
        *self.errors.write().unwrap() = errors;
        *self.unavailable.write().unwrap() = unavailable;
    }

    fn read_regular_file(path: &std::path::Path) -> Result<String, String> {
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("配方文件为 reparse point，拒绝读取或删除".into());
            }
        }
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("配方文件类型不安全，拒绝读取或删除".into());
        }
        crate::fsio::read_text(path).map_err(|e| e.to_string())
    }

    pub fn unavailable(&self) -> Vec<UnavailableRecipe> {
        self.unavailable.read().unwrap().clone()
    }

    pub fn delete_unavailable(&self, file_name: &str) -> Result<(), String> {
        if file_name.is_empty()
            || file_name.starts_with('.')
            || !file_name.ends_with(".json")
            || file_name.contains(['/', '\\', ':'])
            || file_name.trim() != file_name
        {
            return Err("无效的配方文件名".into());
        }
        self.reload();
        {
            let inner = self.inner.write().unwrap();
            if !self
                .unavailable
                .read()
                .unwrap()
                .iter()
                .any(|item| item.file_name == file_name)
            {
                return Err("该文件不在不可用配方列表中，请刷新后重试".into());
            }
            let path = self.dir.join(file_name);
            let text = Self::read_regular_file(&path)?;
            if serde_json::from_str::<RecipeDoc>(&text)
                .ok()
                .is_some_and(|doc| doc.build().is_ok())
            {
                return Err("配方内容已可用，请刷新后通过正常配方操作处理".into());
            }
            let stem = path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or("无效的配方文件名")?;
            if inner
                .iter()
                .any(|(doc, _)| doc.id.eq_ignore_ascii_case(stem))
            {
                return Err("该文件对应已加载的生产配方，拒绝删除".into());
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(version) = value
                    .get("version")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok())
                {
                    self.retire_version(stem, version)?;
                    if let Some(id) = value.get("id").and_then(|v| v.as_str()).filter(|id| {
                        !id.trim().is_empty()
                            && !inner.iter().any(|(doc, _)| doc.id.eq_ignore_ascii_case(id))
                    }) {
                        self.retire_version(id, version)?;
                    }
                }
            }
            std::fs::remove_file(&path).map_err(|e| format!("删除不可用配方失败：{e}"))?;
        }
        self.reload();
        Ok(())
    }

    pub fn list(&self) -> Vec<Arc<Recipe>> {
        self.inner
            .read()
            .unwrap()
            .iter()
            .map(|(_, r)| r.clone())
            .collect()
    }

    pub fn errors(&self) -> Vec<String> {
        self.errors.read().unwrap().clone()
    }

    pub fn get(&self, id: &str) -> Option<Arc<Recipe>> {
        self.inner
            .read()
            .unwrap()
            .iter()
            .find(|(d, _)| d.id == id)
            .map(|(_, r)| r.clone())
    }

    pub fn doc(&self, id: &str) -> Option<RecipeDoc> {
        self.inner
            .read()
            .unwrap()
            .iter()
            .find(|(d, _)| d.id == id)
            .map(|(d, _)| d.clone())
    }

    /// 保存配方时版本号 +1；编号（不分大小写：Windows 上文件名不分）与产品代码都不能和别的配方重复。
    #[cfg(test)]
    pub fn save(
        &self,
        mut doc: RecipeDoc,
        original_id: Option<&str>,
    ) -> Result<Arc<Recipe>, String> {
        doc.name = doc.name.trim().to_string();
        doc.build()?;
        let mut inner = self.inner.write().unwrap();
        let replacing = original_id.unwrap_or(&doc.id).to_string();
        if let Some((d, _)) = inner.iter().find(|(d, _)| {
            d.id != replacing
                && (d.id.eq_ignore_ascii_case(&doc.id) || d.product_code == doc.product_code)
        }) {
            return Err(if d.id.eq_ignore_ascii_case(&doc.id) {
                format!("配方编号 {} 已存在", d.id)
            } else {
                format!("产品代码 {} 已被配方 {} 使用", doc.product_code, d.id)
            });
        }
        // 目录里已有这个文件却不是正在保存的这个配方：启动时没能加载，或是之后手工放进来的，不能覆盖
        let ours = inner
            .iter()
            .any(|(d, _)| d.id == replacing && d.id.eq_ignore_ascii_case(&doc.id));
        if !ours && self.file(&doc.id).exists() {
            return Err(format!(
                "配方目录里已有 {}.json 但没有加载（见配方页的提示）：移走它，或修好后重启程序",
                doc.id
            ));
        }
        let old = inner
            .iter()
            .find(|(d, _)| d.id == replacing)
            .map(|(d, _)| d.version);
        doc.version = self.version_after(&doc.id, old)?.max(if old.is_none() {
            doc.version.max(1)
        } else {
            1
        });
        if !replacing.eq_ignore_ascii_case(&doc.id) {
            if let Some(version) = old {
                self.retire_version(&replacing, version)?;
            }
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

    /// 发布这一配方应分配的版本（`save_published` 按它核对）：已有生产版本时为下一版本；还没有时取下一版本与文档版本的较大者。
    /// 候选里记的版本可能已过时（候选建好后，启动时按历史记录抬高了版本下限），发布时按它重新分配。
    pub fn release_version(&self, doc: &RecipeDoc) -> Result<u32, String> {
        let inner = self.inner.read().unwrap();
        let current = inner
            .iter()
            .find(|(previous, _)| previous.id == doc.id)
            .map(|(previous, _)| previous.version);
        self.expected_version(doc, current)
    }

    fn expected_version(&self, doc: &RecipeDoc, current: Option<u32>) -> Result<u32, String> {
        let floor = self.version_after(&doc.id, current)?;
        Ok(if current.is_some() {
            floor
        } else {
            floor.max(doc.version.max(1))
        })
    }

    pub fn save_published(&self, mut doc: RecipeDoc) -> Result<Arc<Recipe>, String> {
        doc.name = doc.name.trim().to_string();
        let built = doc.build()?;
        let mut inner = self.inner.write().unwrap();
        let current = inner.iter().find(|(previous, _)| previous.id == doc.id);
        if let Some((previous, recipe)) =
            current.filter(|(_, recipe)| recipe.revision_id == built.revision_id)
        {
            if previous == &doc {
                return Ok(recipe.clone());
            }
            return Err("同一发布修订的实际配方不同，不能覆盖".into());
        }
        if doc.stage == RecipeStage::Collect
            && current.is_some_and(|(previous, _)| previous.stage.is_formal())
        {
            return Err(COLLECT_AFTER_FORMAL.into());
        }
        let expected_version =
            self.expected_version(&doc, current.map(|(previous, _)| previous.version))?;
        if doc.version != expected_version {
            return Err("发布版本必须是明确分配的下一版本".into());
        }
        if inner.iter().any(|(previous, _)| {
            previous.id != doc.id
                && (previous.id.eq_ignore_ascii_case(&doc.id)
                    || previous.product_code == doc.product_code)
        }) {
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
        let version = inner
            .iter()
            .find(|(d, _)| d.id == id)
            .ok_or("配方不存在")?
            .0
            .version;
        self.retire_version(id, version)?;
        std::fs::remove_file(self.file(id)).map_err(|e| format!("删除配方文件失败：{e}"))?;
        inner.retain(|(d, _)| d.id != id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// samples_burst（直线 N_exp = 4）后面接一个弯道段。

    #[test]
    fn published_versions_are_assigned_once_and_crash_retries_cannot_overwrite_a_revision() {
        let dir = std::env::temp_dir().join(format!(
            "gluesight-published-version-{}-{}",
            std::process::id(),
            ly_plc::now_ms()
        ));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = samples_burst();
        doc.id = "EXPLICIT-PUBLISHED".into();
        doc.product_code = 60000;
        doc.version = 7;
        let first = store.save_published(doc.clone()).unwrap();
        assert_eq!(first.revision_id, "EXPLICIT-PUBLISHED-v7");
        assert_eq!(store.save_published(doc.clone()).unwrap().version, 7);
        let mut different = doc.clone();
        different.inspect[0].segments[0].corridor[0][0] -= 1.0;
        assert!(store.save_published(different).is_err());
        doc.version = 9;
        assert!(store.save_published(doc.clone()).is_err());
        doc.version = 8;
        let second = store.save_published(doc.clone()).unwrap();
        assert_eq!(second.revision_id, "EXPLICIT-PUBLISHED-v8");
        assert_eq!(store.save(doc, None).unwrap().version, 9);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 防的回归：现场已有的配方文件与修订来源都带已停用的键（`planVersion`、每段与默认参数的 `tolerance`、`boundaryRefine`、
    /// 配方级 `expert`、相机级 `calib` / `detect` / `expert`），读入时不能报错，也不能因与来源记录不一致而被当成手改文件升版本。
    #[test]
    fn recipe_and_revision_source_with_retired_keys_load_at_their_revision() {
        let dir = std::env::temp_dir().join(format!(
            "gluesight-legacy-plan-version-{}-{}",
            std::process::id(),
            ly_plc::now_ms()
        ));
        let recipes = dir.join("recipes");
        std::fs::create_dir_all(&recipes).unwrap();
        let doc = samples_burst();
        let mut legacy = serde_json::to_value(&doc).unwrap();
        legacy["planVersion"] = 14.into();
        legacy["boundaryRefine"] = true.into();
        legacy["segmentDefaults"]["straight"]["tolerance"] = 2.into();
        legacy["segmentDefaults"]["corner"]["tolerance"] = 2.into();
        for segment in legacy["segments"].as_array_mut().unwrap() {
            segment["tolerance"] = 1.into();
        }
        legacy["expert"] = serde_json::json!({ "presentRatio": 0.4 });
        for (k, inspect) in legacy["inspect"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            inspect["calib"] = format!("cam{}-v1", k + 1).into();
            inspect["detect"] = serde_json::json!({ "searchMm": 6.0, "polarity": "dark", "widthRange": [1.0, 5.0] });
            inspect["expert"] = serde_json::json!({ "trackSharpMin": 0.6 });
        }
        std::fs::write(recipes.join(format!("{}.json", doc.id)), legacy.to_string()).unwrap();
        let sources = serde_json::json!({ doc.id.to_ascii_lowercase(): legacy });
        std::fs::write(recipes.join(".revision-sources"), sources.to_string()).unwrap();
        let store = RecipeStore::open(dir.clone()).unwrap();
        assert!(store.errors().is_empty(), "{:?}", store.errors());
        assert_eq!(
            store.get(&doc.id).unwrap().revision_id,
            format!("{}-v1", doc.id)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 在线投运：采集版之后照常发布正式版；正式版生效后不能再发布采集版（不降级成只采图的配方）。
    #[test]
    fn collect_then_formal_publishes_but_formal_never_downgrades_to_collect() {
        let dir = std::env::temp_dir().join(format!(
            "gluesight-collect-stage-{}-{}",
            std::process::id(),
            ly_plc::now_ms()
        ));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let publish = |stage: RecipeStage| {
            let mut doc = samples_burst();
            doc.stage = stage;
            doc.version = store.next_version(&doc.id).unwrap();
            store.save_published(doc)
        };
        let collect = publish(RecipeStage::Collect).unwrap();
        assert!(collect.is_collect());
        let formal = publish(RecipeStage::Formal).unwrap();
        assert!(!formal.is_collect());
        assert_eq!(
            publish(RecipeStage::Collect).unwrap_err(),
            COLLECT_AFTER_FORMAL
        );
        assert!(!store.get(&formal.id).unwrap().is_collect());
        let _ = std::fs::remove_dir_all(dir);
    }
}
