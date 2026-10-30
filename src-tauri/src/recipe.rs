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
use crate::vision::template::{validate_expert, ExpertParams};

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
    fn validate(&self, what: &str) -> Result<(), String> {
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

/// 配方文件格式版本。不一致的文件列为加载错误，不迁移。8：增加段模型字段（segments、inspect、segmentDefaults、calibrationRounds、partCalib）。
pub const RECIPE_SCHEMA: u32 = 8;
/// 没有段的配方文件是已停用的旧拍照点配方。
pub const LEGACY_RECIPE: &str = "旧拍照点配方已停用，请新建段配方";

/// 段配方连续采集几轮取均值作预期帧数（规格 §2：≥ 10 轮，轮数可配）。
pub const DEFAULT_CALIBRATION_ROUNDS: u32 = 10;
/// 标定轮数的可配范围（D-SB-36）。
pub const CALIBRATION_ROUNDS: std::ops::RangeInclusive<u32> = 10..=100;

fn default_calibration_rounds() -> u32 {
    DEFAULT_CALIBRATION_ROUNDS
}

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

/// 段配方里一台检测相机的参数（D-SB-45）：像素当量、标定引用、检测参数，以及它在各段的走廊。
/// `RecipeDoc.inspect` 的顺序就是计划相机顺序（PLC 一个触发同时触发这三台）。
/// 不拒绝未知字段：旧版的相机走廊 `corridor` 已放弃，读入时丢弃（D-SB-45 不迁移）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraInspect {
    pub camera: String,
    /// 像素当量（mm/px）；为空为尚未设置
    #[serde(default)]
    pub mm_per_px: Option<f64>,
    /// 标定引用；为空时用这台相机的工位标定
    #[serde(default)]
    pub calib: Option<String>,
    /// 为空时用配方的检测参数
    #[serde(default)]
    pub detect: Option<DetectParams>,
    /// 这台相机在它检测的各段的走廊（D-SB-42/45）：落在该段的帧用它测量，没有相机级的默认走廊
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<SegmentCorridor>,
    /// 逐帧定向的检测窗口（D-SB-47）：只测沿胶路距出胶点 [start, start + length] mm 的一段
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<HeadingWindow>,
    /// 标定的出胶点（图像像素，D-SB-47）；为空时由开启定向的各段走廊起点推出
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle: Option<[f64; 2]>,
    /// 这台相机覆盖的专家参数（LF-5，测量图模板参数名 → 值），叠在配方默认之上
    #[serde(default, skip_serializing_if = "ExpertParams::is_empty")]
    pub expert: ExpertParams,
}

/// 逐帧定向的检测窗口（mm，沿胶路从出胶点量起）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeadingWindow {
    pub start_mm: f64,
    pub length_mm: f64,
}

/// 一台相机在某一段（`PathSegment.index`）的走廊（图像像素，只决定 ROI 和测站）；少于 2 点为尚未示教。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SegmentCorridor {
    pub segment: u32,
    #[serde(default)]
    pub corridor: Vec<[f32; 2]>,
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
        }
        if let Some(m) = self
            .mm_per_px
            .filter(|m| !(m.is_finite() && *m > 0.0 && *m <= 10.0))
        {
            return Err(format!("相机 {id} 的像素当量 {m} 需在 0–10 mm/px 之间"));
        }
        if self.calib.as_deref().is_some_and(|c| !valid_camera_id(c)) {
            return Err(format!("相机 {id} 的标定引用只能用字母、数字、- 和 _"));
        }
        if let Some(d) = &self.detect {
            d.validate(&format!("相机 {id} 检测参数"))?;
        }
        if let Some(w) = &self.window {
            if !(w.start_mm.is_finite()
                && w.length_mm.is_finite()
                && w.start_mm >= 0.0
                && w.length_mm > 0.0
                && w.start_mm + w.length_mm <= 500.0)
            {
                return Err(format!(
                    "相机 {id} 的测量区间需满足 跳过距离 ≥ 0、测量到的距离大于跳过距离且不超过 500 mm"
                ));
            }
        }
        if self.nozzle.is_some_and(|p| {
            p.iter()
                .any(|v| !(v.is_finite() && *v >= 0.0 && *v <= 100_000.0))
        }) {
            return Err(format!("相机 {id} 的出胶点坐标需为 0–100000 px 的有限数"));
        }
        validate_expert(&self.expert, &format!("相机 {id}"))
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
    /// PLC 拍照计划版本（见 RecipeDoc::plan_version）
    pub plan_version: u32,
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
    /// 拍照点没单独设时用的检测参数与限值
    pub detect: DetectParams,
    pub limits: ShotLimits,
    /// 段配方的段列表（RecipeDoc.segments）；为空即旧拍照点配方
    pub path_segments: Vec<PathSegment>,
    /// 段配方的检测相机，顺序即计划相机顺序
    pub inspect: Vec<CameraInspect>,
    pub segment_defaults: KindDefaults,
    pub calibration_rounds: u32,
    pub boundary_guard: u32,
    pub boundary_refine: bool,
    pub part_calib: Option<PartCalib>,
    pub expert: ExpertParams,
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

    /// 采集版：只能在维护模式下采图，不能当正式检测运行。
    pub fn is_collect(&self) -> bool {
        self.stage == RecipeStage::Collect
    }

    /// 整件预期帧组数 ΣN_exp（各段都已标定取其和，否则取整件标定）；未标定为 None。
    pub fn expected_total(&self) -> Option<u32> {
        plan::expected_total(&self.path_segments, self.part_calib.as_ref())
    }

    /// 整件帧数容差 Σtolerance。
    pub fn tolerance_total(&self) -> u32 {
        plan::tolerance_total(&self.path_segments)
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
                            "第 {} 段（{kind}）待标定：预期帧数要由多轮采集得出",
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

    /// 一台相机的专家参数层，按叠加顺序：配方默认、相机覆盖。
    pub fn expert_layers<'a>(&'a self, inspect: &'a CameraInspect) -> [&'a ExpertParams; 2] {
        [&self.expert, &inspect.expert]
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
    /// 只改限值、检测参数、示教中线、容差、x_ref 或"不检"时不变，PLC 侧不用跟着改。0 表示还没分配（不能接件）。
    #[serde(default)]
    pub plan_version: u32,
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
    pub detect: DetectParams,
    pub limits: ShotLimits,
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
    /// 比例分段边界两侧的过渡帧数，0..=5（D-SB-40）；不进计划身份
    #[serde(default = "default_boundary_guard")]
    pub boundary_guard: u32,
    /// 按逐帧定向的胶条方向序列细化比例边界（D-SB-47）；只作用于开启定向的段，不进计划身份
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub boundary_refine: bool,
    /// 整件帧组数标定：各段 N_exp 待分段算法时凭它得到 expected_total 并发布（决策补充第二节）；只由标定写入
    #[serde(default)]
    pub part_calib: Option<PartCalib>,
    /// 示教件来源（RB-7）：正式发布时从候选写入；候选里恒为空，没有它的生产配方即旧流程发布。不进计划身份
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teaching_piece: Option<TeachingPieceRef>,
    /// 专家参数的配方默认（LF-5，测量图模板参数名 → 值）；没写的用模板默认值
    #[serde(default, skip_serializing_if = "ExpertParams::is_empty")]
    pub expert: ExpertParams,
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

    /// 整件预期帧组数 ΣN_exp（见 `plan::expected_total`）。
    pub fn expected_total(&self) -> Option<u32> {
        plan::expected_total(&self.segments, self.part_calib.as_ref())
    }

    /// 段计划身份（`plan::plan_identity`）；旧拍照点配方没有段和检测相机，身份恒等。
    pub fn plan_identity(&self) -> plan::PlanIdentity {
        plan::plan_identity(
            self.product_code,
            &self.segments,
            &self.plan_cameras(),
            self.expected_total(),
        )
    }

    /// 两份配方的段计划身份相同，就是同一份计划（D-SB-19）。
    pub fn same_plan(&self, other: &RecipeDoc) -> bool {
        self.plan_identity() == other.plan_identity()
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
        validate_expert(&self.expert, "配方")?;
        if !self.is_burst() {
            return Err(LEGACY_RECIPE.into());
        }
        self.validate_burst()
    }

    /// 段配方的模型约束（D-SB-1）：只支持飞拍；标定轮数 10..=100；检测相机各自合法；
    /// 段列表与三台计划相机满足 `plan::validate_segments`；直线 / 弯道默认参数满足 `plan::validate_defaults`（新建段取用它）；
    /// 整件标定的帧组数不超过上限。
    fn validate_burst(&self) -> Result<(), String> {
        if self.trigger_mode != TriggerMode::Fly {
            return Err("段配方只支持飞拍（triggerMode 必须为 fly）".into());
        }
        if !CALIBRATION_ROUNDS.contains(&self.calibration_rounds) {
            return Err(format!(
                "标定轮数 {} 需在 {}–{} 之间",
                self.calibration_rounds,
                CALIBRATION_ROUNDS.start(),
                CALIBRATION_ROUNDS.end()
            ));
        }
        if !BOUNDARY_GUARD.contains(&self.boundary_guard) {
            return Err(format!(
                "边界过渡帧数 {} 需在 {}–{} 之间",
                self.boundary_guard,
                BOUNDARY_GUARD.start(),
                BOUNDARY_GUARD.end()
            ));
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
            plan_version: self.plan_version,
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
            calibration_rounds: self.calibration_rounds,
            boundary_guard: self.boundary_guard,
            boundary_refine: self.boundary_refine,
            part_calib: self.part_calib.clone(),
            expert: self.expert.clone(),
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
        let object = value.as_object_mut().ok_or("配方快照不是 JSON 对象")?;
        let path_segments = object
            .remove("pathSegments")
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
        object.insert("segments".into(), path_segments);
        serde_json::from_value::<RecipeDoc>(value)
            .map_err(|e| e.to_string())?
            .build()
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

/// 段配方的起点（D-SB-24）：飞拍、一个直线段（待标定，容差与检测项取配方默认）、按给定顺序的检测相机（走廊待示教）、
/// 计划版本未分配。相机不足 3 台时照样返回，但过不了段配方校验。
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
        plan_version: 0,
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
                calib: None,
                detect: None,
                segments: Vec::new(),
                window: None,
                nozzle: None,
                expert: ExpertParams::new(),
            })
            .collect(),
        segment_defaults,
        calibration_rounds: DEFAULT_CALIBRATION_ROUNDS,
        boundary_guard: DEFAULT_BOUNDARY_GUARD,
        boundary_refine: false,
        part_calib: None,
        teaching_piece: None,
        expert: ExpertParams::new(),
    }
}

/// 一次预留 `count` 个计划版本号并返回第一个（项目导入用）；与配方库发号共用本机同一份记录。
pub fn reserve_plan_versions(plan_file: &std::path::Path, count: u32) -> Result<u32, String> {
    let issued: u32 = match crate::fsio::read_text(plan_file) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| format!("计划版本记录损坏，拒绝分配计划版本：{e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(format!("读取计划版本记录失败：{e}")),
    };
    let last = issued.checked_add(count).ok_or("计划版本已达上限")?;
    crate::fsio::write_atomic(plan_file, &last.to_string())?;
    Ok(issued + 1)
}

/// 测试用的段配方：模拟相机 cam1–cam3 共用触发、走廊已示教、一个直线段 N_exp = 4（ΣN_exp = 4），计划版本已分配。
#[cfg(test)]
pub fn samples_burst() -> RecipeDoc {
    let cameras: Vec<String> = (1..=3).map(|k| format!("cam{k}")).collect();
    let mut doc = burst_template("SEAM-BURST".into(), "分段连拍样例".into(), 14, &cameras);
    doc.plan_version = 14;
    for inspect in &mut doc.inspect {
        inspect.segments = vec![SegmentCorridor {
            segment: 1,
            corridor: sample_path(0),
        }];
        inspect.mm_per_px = Some(f64::from(SIM_MM_PER_PX));
    }
    doc.segments[0].expected_frames = Some(4);
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
    plan_file: PathBuf,
    inner: RwLock<Vec<(RecipeDoc, Arc<Recipe>)>>,
    errors: RwLock<Vec<String>>,
    unavailable: RwLock<Vec<UnavailableRecipe>>,
}

impl RecipeStore {
    #[cfg(test)]
    pub fn open(root: PathBuf) -> Result<Self, String> {
        Self::open_with_floors(
            root.join("recipes"),
            root.join("state").join("plan-version"),
            &BTreeMap::new(),
        )
    }

    /// `plan_file` 是本机共用的计划版本记录：PLC 是同一台，换项目也不能发出用过的计划版本。
    pub fn open_with_floors(
        dir: PathBuf,
        plan_file: PathBuf,
        floors: &BTreeMap<String, u32>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建配方目录失败：{e}"))?;
        let store = Self {
            dir,
            plan_file,
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

    /// 旧格式的修订来源无法再证明配方内容（D-0）：原文件备份后移除，其修订号与计划版本并入版本记录，仍不复用。
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
        let plan = outdated
            .iter()
            .map(|d| d.plan_version)
            .max()
            .unwrap_or(0)
            .max(self.issued_plan_version()?);
        crate::fsio::write_atomic(&self.plan_file, &plan.to_string())?;
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

    /// 已发出的最大计划版本。文件丢了就从现有配方里取最大值，不回退到已用过的号。
    fn issued_plan_version(&self) -> Result<u32, String> {
        match crate::fsio::read_text(&self.plan_file) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("计划版本记录损坏，拒绝分配计划版本：{e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(format!("读取计划版本记录失败：{e}")),
        }
    }

    /// 发一个新的计划版本号：比已发出的、现有配方和修订来源里用过的都大。
    fn issue_plan_version(&self, list: &[(RecipeDoc, Arc<Recipe>)]) -> Result<u32, String> {
        let used = list
            .iter()
            .map(|(d, _)| d.plan_version)
            .chain(self.revision_sources()?.values().map(|d| d.plan_version))
            .max()
            .unwrap_or(0);
        let next = self
            .issued_plan_version()?
            .max(used)
            .checked_add(1)
            .ok_or("计划版本已达上限")?;
        crate::fsio::write_atomic(&self.plan_file, &next.to_string())?;
        Ok(next)
    }

    /// 同一配方拍照计划没变就沿用原号，否则（新配方、改了拍照点或产品代码）发新号。
    fn plan_version_in(
        &self,
        list: &[(RecipeDoc, Arc<Recipe>)],
        doc: &RecipeDoc,
        current_id: &str,
    ) -> Result<u32, String> {
        match list.iter().find(|(d, _)| d.id == current_id) {
            Some((current, _)) if current.plan_version != 0 && current.same_plan(doc) => {
                Ok(current.plan_version)
            }
            _ => self.issue_plan_version(list),
        }
    }

    /// 发布前冻结配方时调用：拍照计划没变沿用生产配方的计划版本，变了就发新号（没生效的号作废即可）。
    pub fn plan_version_for(&self, doc: &RecipeDoc) -> Result<u32, String> {
        let inner = self.inner.write().unwrap();
        self.plan_version_in(&inner, doc, &doc.id)
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
            // 计划版本重复（多半是复制了配方文件）：只留修订来源能证明这个号属于它的那一份，与文件名先后无关
            let mut plan_users: BTreeMap<u32, Vec<String>> = BTreeMap::new();
            for p in &paths {
                if let Some(d) = Self::read_regular_file(p)
                    .ok()
                    .and_then(|s| serde_json::from_str::<RecipeDoc>(&s).ok())
                    .filter(|d| d.plan_version != 0)
                {
                    plan_users
                        .entry(d.plan_version)
                        .or_default()
                        .push(d.id.to_ascii_lowercase());
                }
            }
            let plan_owned = |d: &RecipeDoc| -> Result<(), String> {
                let Some(users) = plan_users
                    .get(&d.plan_version)
                    .filter(|users| users.len() > 1)
                else {
                    return Ok(());
                };
                let owners: Vec<&String> = users
                    .iter()
                    .filter(|id| {
                        sources
                            .get(*id)
                            .is_some_and(|s| s.plan_version == d.plan_version)
                    })
                    .collect();
                if owners.len() == 1 && *owners[0] == d.id.to_ascii_lowercase() {
                    return Ok(());
                }
                Err(format!("计划版本 {} 与别的配方文件重复（复制过的配方文件？），没有加载：删掉复制件或在配方页新建", d.plan_version))
            };
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
                    plan_owned(&d)?;
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
        // 手工放进来的配方可能带着比记录还大的计划版本：记录跟上，以后发的号不和它撞
        let max_plan = list.iter().map(|(d, _)| d.plan_version).max().unwrap_or(0);
        if self
            .issued_plan_version()
            .is_ok_and(|issued| issued < max_plan)
        {
            if let Err(error) = crate::fsio::write_atomic(&self.plan_file, &max_plan.to_string()) {
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
                if let Some(plan) = value
                    .get("planVersion")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok())
                {
                    let plan = plan.max(self.issued_plan_version()?);
                    crate::fsio::write_atomic(&self.plan_file, &plan.to_string())?;
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
        doc.plan_version = self.plan_version_in(&inner, &doc, &replacing)?;
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

    pub fn save_published(
        &self,
        mut doc: RecipeDoc,
        expected_base: Option<&str>,
    ) -> Result<Arc<Recipe>, String> {
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
        if current.map(|(_, recipe)| recipe.revision_id.as_str()) != expected_base {
            return Err("待发布版本与当前生产配方冲突".into());
        }
        if doc.stage == RecipeStage::Collect
            && current.is_some_and(|(previous, _)| previous.stage.is_formal())
        {
            return Err(COLLECT_AFTER_FORMAL.into());
        }
        let floor = self.version_after(&doc.id, current.map(|(previous, _)| previous.version))?;
        let expected_version = if current.is_some() {
            floor
        } else {
            floor.max(doc.version.max(1))
        };
        if doc.version != expected_version {
            return Err("发布版本必须是明确分配的下一版本".into());
        }
        let kept = current
            .is_some_and(|(previous, _)| previous.plan_version != 0 && previous.same_plan(&doc));
        let plan_ok = if kept {
            current.is_some_and(|(previous, _)| previous.plan_version == doc.plan_version)
        } else {
            doc.plan_version != 0
                && doc.plan_version <= self.issued_plan_version()?
                && !inner
                    .iter()
                    .any(|(previous, _)| previous.plan_version == doc.plan_version)
        };
        if !plan_ok {
            return Err("发布的拍照计划版本必须由配方库分配：计划没变沿用原号，变了用新号".into());
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
        assert!(store
            .save_published(doc.clone(), None)
            .unwrap_err()
            .contains("计划版本"));
        doc.plan_version = store.plan_version_for(&doc).unwrap();
        let first = store.save_published(doc.clone(), None).unwrap();
        assert_eq!(first.revision_id, "EXPLICIT-PUBLISHED-v7");
        assert_eq!(store.save_published(doc.clone(), None).unwrap().version, 7);
        let mut different = doc.clone();
        different.inspect[0].segments[0].corridor[0][0] -= 1.0;
        assert!(store.save_published(different, None).is_err());
        doc.version = 8;
        let second = store
            .save_published(doc.clone(), Some(&first.revision_id))
            .unwrap();
        assert_eq!(second.revision_id, "EXPLICIT-PUBLISHED-v8");
        doc.version = 9;
        assert!(store
            .save_published(doc.clone(), Some(&first.revision_id))
            .is_err());
        assert_eq!(store.get(&doc.id).unwrap().version, 8);
        assert_eq!(store.save(doc, None).unwrap().version, 9);
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
        let publish = |stage: RecipeStage, base: Option<&str>| {
            let mut doc = samples_burst();
            doc.stage = stage;
            doc.version = store.next_version(&doc.id).unwrap();
            doc.plan_version = store.plan_version_for(&doc).unwrap();
            store.save_published(doc, base)
        };
        let collect = publish(RecipeStage::Collect, None).unwrap();
        assert!(collect.is_collect());
        let formal = publish(RecipeStage::Formal, Some(&collect.revision_id)).unwrap();
        assert!(!formal.is_collect());
        assert_eq!(
            publish(RecipeStage::Collect, Some(&formal.revision_id)).unwrap_err(),
            COLLECT_AFTER_FORMAL
        );
        assert!(!store.get(&formal.id).unwrap().is_collect());
        let _ = std::fs::remove_dir_all(dir);
    }
}
