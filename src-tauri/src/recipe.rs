//! 配方：磁盘上存可编辑的 `RecipeDoc`，加载时生成运行用的 `Recipe`（分段、测量点、修订标识）。
//! 一期按拍照点在图像里检测（P0 D-10）：每个拍照点示教一条胶路中线（图像像素），沿线每隔 spacing 一站；
//! 每个拍照点自成一段，判定在段内做，段与段之间不连。`Recipe` 同时是检测记录里的配方快照。

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

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
}

pub fn default_min_present() -> f32 {
    0.8
}

impl ShotLimits {
    fn validate(&self, what: &str) -> Result<(), String> {
        if let Some(p) = &self.position {
            p.validate(&format!("{what} 位置"))?;
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

/// 配方文件格式版本。不一致的文件列为加载错误，不迁移。
pub const RECIPE_SCHEMA: u32 = 5;

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
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "RecipeDoc")]
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
    pub segments: Vec<Segment>,
    pub points: PathPoints,
}

impl Recipe {
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

    /// 本配方要用到的相机（编号），按第一次出现的拍照点排序。
    pub fn cameras(&self) -> Vec<String> {
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
    /// PLC 拍照计划版本：由配方库分配、全局唯一，只在产品代码或拍照点的顺序、编号、Pose、相机变了时换新号。
    /// 只改限值、检测参数、示教中线或"不检"时不变，PLC 侧不用跟着改。0 表示还没分配（不能布防）。
    #[serde(default)]
    pub plan_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teaching_id: Option<String>,
    pub product_code: u16,
    pub trigger_mode: TriggerMode,
    /// 缺省为 0，校验时报格式版本不符
    #[serde(default)]
    pub schema_version: u32,
    pub spacing: f32,
    pub filter_window: usize,
    pub detect: DetectParams,
    pub limits: ShotLimits,
    #[serde(default)]
    pub shots: Vec<ShotSpec>,
}

impl RecipeDoc {
    /// PLC 侧要照着走的那部分：两份配方这几项逐项相同，就是同一份拍照计划（逐字段比较，不算摘要）。
    pub fn same_plan(&self, other: &RecipeDoc) -> bool {
        self.product_code == other.product_code
            && self.shots.len() == other.shots.len()
            && self.shots.iter().zip(&other.shots).all(|(a, b)| a.id == b.id && a.pose_id == b.pose_id && a.camera == b.camera)
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
        if self.shots.len() > 64 {
            return Err("配方最多允许 64 个拍照点".into());
        }
        let mut ids = HashSet::new();
        for shot in &self.shots {
            shot.validate()?;
            if !ids.insert(shot.id.as_str()) {
                return Err(format!("拍照点编号 {} 重复", shot.id));
            }
        }
        Ok(())
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
            segments,
            points,
        };
        Ok(recipe)
    }
}

impl TryFrom<RecipeDoc> for Recipe {
    type Error = String;
    fn try_from(doc: RecipeDoc) -> Result<Self, Self::Error> { doc.build() }
}

/// 胶宽、位置、断胶的默认限值：名义胶宽 4 mm；断口阈值取自 MX11 现场图（正常帧最长无胶 5.4 mm，断胶帧 10–22 mm）。
pub fn default_limits() -> ShotLimits {
    ShotLimits {
        position: Some(JudgeParams { nominal: 0.0, tol_upper: 2.0, tol_lower: 2.0, abs_min: -5.0, abs_max: 5.0, max_excursion_len: 5.0 }),
        width: Some(JudgeParams { nominal: 4.0, tol_upper: 1.5, tol_lower: 1.5, abs_min: 1.0, abs_max: 8.0, max_excursion_len: 5.0 }),
        max_gap_len: 6.0,
        min_present: default_min_present(),
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
    let doc = |id: &str, name: &str, code: u16, n: usize, trigger_mode| RecipeDoc {
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
    };
    vec![doc("MTR-HSG-A", "电机壳体 A", 12, 6, TriggerMode::Fly), doc("MTR-HSG-B", "电机壳体 B", 13, 4, TriggerMode::Stop)]
}

/// 测试与重判里需要一份现成配方时用。
#[cfg(test)]
pub fn builtin() -> Vec<Arc<Recipe>> {
    samples().iter().map(|d| Arc::new(d.build().unwrap())).collect()
}

/// 磁盘上的配方库：每个配方一个 `<id>.json`。
pub struct RecipeStore {
    dir: PathBuf,
    inner: RwLock<Vec<(RecipeDoc, Arc<Recipe>)>>,
    errors: RwLock<Vec<String>>,
}

impl RecipeStore {
    pub fn open(dir: PathBuf) -> Result<Self, String> {
        Self::open_with_floors(dir, &BTreeMap::new())
    }

    pub fn open_with_floors(dir: PathBuf, floors: &BTreeMap<String, u32>) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建配方目录失败：{e}"))?;
        let store = Self { dir, inner: RwLock::new(Vec::new()), errors: RwLock::new(Vec::new()) };
        store.seed_version_floor(floors)?;
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
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
            paths.sort();
            // 计划版本重复（多半是复制了配方文件）：只留修订来源能证明这个号属于它的那一份，与文件名先后无关
            let mut plan_users: BTreeMap<u32, Vec<String>> = BTreeMap::new();
            for p in &paths {
                if let Some(d) = crate::fsio::read_text(p).ok().and_then(|s| serde_json::from_str::<RecipeDoc>(&s).ok()).filter(|d| d.plan_version != 0) {
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
                let parsed = crate::fsio::read_text(&p)
                    .map_err(|e| e.to_string())
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
                    Err(e) => errors.push(format!("{name}：{e}")),
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

    /// 电机壳体 B 的四个拍照点改成 cam1 → cam2 → cam3 → cam1。
    fn three_cameras() -> RecipeDoc {
        let mut doc = samples().remove(1);
        for (shot, camera) in doc.shots.iter_mut().zip(["cam1", "cam2", "cam3", "cam1"]) {
            shot.camera = camera.into();
        }
        doc
    }

    #[test]
    fn multiview_keeps_one_shot_and_distinct_segments() {
        let mut doc = samples().remove(0);
        doc.shots.truncate(1);
        let shot = &mut doc.shots[0];
        shot.views = [1, 2, 3].into_iter().map(|view| ShotViewSpec { view, enabled: view != 2,
            path: shot.path.clone(), mm_per_px: shot.mm_per_px, detect: None, limits: None, calib: None }).collect();
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
    fn incomplete_drafts_save_but_cannot_run() {
        let mut doc = samples().remove(0);
        doc.shots.clear();
        assert!(doc.build().unwrap().ready().is_err());
        doc.shots = shot_list("cam1", vec![vec![[1.0, 2.0]]]);
        doc.shots[0].mm_per_px = None;
        assert!(doc.build().unwrap().ready().is_err());
        doc.schema_version = 4;
        assert!(doc.build().is_err());
    }

    #[test]
    fn stations_follow_each_taught_line() {
        let r = samples().remove(1).build().unwrap();
        assert_eq!(r.segments.len(), 4);
        for (k, g) in r.segments.iter().enumerate() {
            let shot = &r.shots[k];
            assert_eq!((g.shot, g.name.as_str()), (k, format!("{} · J1", shot.id).as_str()));
            // 首站在中线起点，站与站沿线相隔 spacing（换成像素）
            assert_eq!([r.points.x[g.first], r.points.y[g.first]], shot.path[0]);
            let step = r.spacing / shot.mm_per_px.unwrap();
            let expect = (shot.path_len_px() / step + 1e-3).floor() as usize + 1;
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
        assert_eq!(r.ready().unwrap_err(), "配方 MTR-HSG-B 的拍照点 P3 尚未示教胶路");
        doc.shots[2].skip = true;
        assert!(doc.build().unwrap().ready().is_ok());
    }

    #[test]
    fn shot_limits_override_recipe_limits() {
        let mut doc = samples().remove(1);
        doc.shots[3].limits = Some(ShotLimits { position: None, width: None, max_gap_len: 9.0, min_present: 0.5 });
        let r = doc.build().unwrap();
        assert_eq!(r.segments[0].max_gap_len, 6.0);
        assert!(r.segments[0].width.is_some());
        assert_eq!((r.segments[3].max_gap_len, r.segments[3].width.is_none(), r.segments[3].min_present), (9.0, true, 0.5));
        assert_eq!(r.segments[0].min_present, 0.8);
    }

    #[test]
    fn all_skipped_recipe_is_rejected() {
        let mut doc = samples().remove(1);
        doc.shots.iter_mut().for_each(|s| s.skip = true);
        assert!(doc.build().unwrap().ready().unwrap_err().contains("至少需要一个参与检测"));
        doc.shots[2].skip = false;
        assert!(doc.build().is_ok());
    }

    #[test]
    fn three_camera_recipe_saves_and_reloads() {
        let dir = std::env::temp_dir().join(format!("gluesight-recipe-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let saved = store.save(three_cameras(), None).unwrap();
        assert_eq!(saved.cameras(), ["cam1", "cam2", "cam3"]);
        store.reload();
        assert!(store.errors().is_empty(), "{:?}", store.errors());
        let loaded = store.get(&saved.id).unwrap();
        assert_eq!(loaded.revision_id, saved.revision_id);
        assert_eq!(loaded.shots, saved.shots);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn every_save_advances_the_explicit_recipe_revision() {
        let dir = std::env::temp_dir().join(format!("gluesight-revision-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let doc = three_cameras();
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
        let mut doc = three_cameras(); doc.id = "REUSED".into(); doc.product_code = 60002; doc.version = 1;
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
    fn historical_floors_are_loaded_before_automatic_samples_are_seeded() {
        let dir = std::env::temp_dir().join(format!("gluesight-history-first-{}-{}", std::process::id(), ly_plc::now_ms()));
        let sample = samples().remove(0);
        let floors = BTreeMap::from([(sample.id.clone(), 7)]);
        let store = RecipeStore::open_with_floors(dir.clone(), &floors).unwrap();
        assert!(store.list().is_empty());
        assert_eq!(store.next_version(&sample.id).unwrap(), 8);
        let mut candidate = sample; candidate.version = 1;
        assert_eq!(store.save(candidate, None).unwrap().version, 8);
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
        assert_eq!(store.save(doc.clone(), None).unwrap().plan_version, plan);
        // 改了 Pose、相机、拍照点顺序或产品代码：发新号，且和别的配方都不重
        let mut seen = vec![plan, store.doc("MTR-HSG-A").unwrap().plan_version];
        let edits: [fn(&mut RecipeDoc); 4] = [
            |d| d.shots[0].pose_id = "P9".into(),
            |d| d.shots[1].camera = "cam2".into(),
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

    #[test]
    fn copied_recipe_file_with_the_same_plan_version_is_not_loaded() {
        let dir = std::env::temp_dir().join(format!("gluesight-plan-copy-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut copy = store.doc("MTR-HSG-B").unwrap();
        copy.id = "COPIED".into();
        copy.product_code = 63000;
        std::fs::write(store.file(&copy.id), serde_json::to_string(&copy).unwrap()).unwrap();
        store.reload();
        // 复制件文件名排在原件前面，也只拒复制件，原配方照常加载；重启后仍然如此
        for _ in 0..2 {
            assert!(store.get("COPIED").is_none());
            assert!(store.get("MTR-HSG-B").is_some());
            assert!(store.errors().iter().any(|e| e.contains("计划版本") && e.contains("重复")), "{:?}", store.errors());
            store.reload();
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn publish_requires_a_plan_version_assigned_by_the_store() {
        let dir = std::env::temp_dir().join(format!("gluesight-plan-publish-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = three_cameras(); doc.id = "PLAN-PUBLISH".into(); doc.product_code = 60007;
        doc.version = store.next_version(&doc.id).unwrap();
        // 没分配、借用别的配方的号、自己编一个没发过的号：都不收
        for forged in [0, store.doc("MTR-HSG-A").unwrap().plan_version, 900_000] {
            doc.plan_version = forged;
            assert!(store.save_published(doc.clone(), None).unwrap_err().contains("计划版本"), "{forged}");
        }
        doc.plan_version = store.plan_version_for(&doc).unwrap();
        let first = store.save_published(doc.clone(), None).unwrap();
        // 下一版只改限值：必须沿用原号
        doc.version = store.next_version(&doc.id).unwrap();
        doc.limits.max_gap_len = 2.0;
        assert_eq!(store.plan_version_for(&doc).unwrap(), first.plan_version);
        let mut renumbered = doc.clone();
        renumbered.plan_version = store.plan_version_for(&{ let mut d = doc.clone(); d.product_code = 60008; d }).unwrap();
        assert!(store.save_published(renumbered, Some(&first.revision_id)).is_err());
        doc.plan_version = first.plan_version;
        assert_eq!(store.save_published(doc, Some(&first.revision_id)).unwrap().plan_version, first.plan_version);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_publish_write_does_not_consume_the_frozen_target_version() {
        let dir = std::env::temp_dir().join(format!("gluesight-publish-retry-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = three_cameras(); doc.id = "WRITE-RETRY".into(); doc.product_code = 60005;
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
    fn damaged_retired_version_ledger_never_resets_to_one() {
        let dir = std::env::temp_dir().join(format!("gluesight-damaged-versions-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let doc = three_cameras();
        let original = store.get(&doc.id).unwrap();
        std::fs::write(dir.join(".revision-versions"), "invalid").unwrap();
        assert!(store.next_version(&doc.id).unwrap_err().contains("版本记录损坏"));
        assert!(store.save(doc.clone(), None).unwrap_err().contains("版本记录损坏"));
        assert!(store.delete(&doc.id).unwrap_err().contains("版本记录损坏"));
        assert_eq!(store.get(&doc.id).unwrap().revision_id, original.revision_id);
        drop(store);
        assert!(RecipeStore::open(dir.clone()).err().unwrap().contains("版本记录损坏"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn external_recipe_edits_receive_persistent_new_versions_without_content_fingerprints() {
        let dir = std::env::temp_dir().join(format!("gluesight-external-revision-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = store.doc("MTR-HSG-B").unwrap();
        let original = store.get(&doc.id).unwrap();
        doc.limits.max_gap_len += 1.0;
        std::fs::write(store.file(&doc.id), serde_json::to_vec(&doc).unwrap()).unwrap();
        store.reload();
        assert!(store.errors().is_empty(), "{:?}", store.errors());
        let edited = store.get(&doc.id).unwrap();
        assert_eq!(edited.version, original.version + 1);
        assert_ne!(edited.revision_id, original.revision_id);
        assert_eq!(original.version, 1);
        assert_eq!(store.doc(&doc.id).unwrap().limits.max_gap_len, doc.limits.max_gap_len);
        drop(store);
        let store = RecipeStore::open(dir.clone()).unwrap();
        assert_eq!(store.get(&doc.id).unwrap().version, edited.version);
        let formatted = serde_json::to_value(store.doc(&doc.id).unwrap()).unwrap();
        std::fs::write(store.file(&doc.id), format!("\n{}\n", serde_json::to_string_pretty(&formatted).unwrap())).unwrap();
        store.reload();
        assert_eq!(store.get(&doc.id).unwrap().version, edited.version);
        store.delete(&doc.id).unwrap();
        std::fs::write(store.file(&doc.id), serde_json::to_vec(&doc).unwrap()).unwrap();
        drop(store);
        let store = RecipeStore::open(dir.clone()).unwrap();
        assert_eq!(store.get(&doc.id).unwrap().version, edited.version + 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_without_proven_source_and_rolled_back_files_never_reuse_versions() {
        let dir = std::env::temp_dir().join(format!("gluesight-unproven-revision-{}-{}", std::process::id(), ly_plc::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = three_cameras();
        std::fs::write(dir.join(format!("{}.json", doc.id)), serde_json::to_vec(&doc).unwrap()).unwrap();
        let store = RecipeStore::open_with_floors(dir.clone(), &BTreeMap::from([(doc.id.clone(), 7)])).unwrap();
        assert_eq!(store.get(&doc.id).unwrap().version, 8);
        std::fs::write(store.file(&doc.id), serde_json::to_vec(&doc).unwrap()).unwrap();
        store.reload();
        assert_eq!(store.get(&doc.id).unwrap().version, 9);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_source_write_does_not_admit_an_external_revision_and_can_be_recovered() {
        let dir = std::env::temp_dir().join(format!("gluesight-source-write-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = store.doc("MTR-HSG-B").unwrap(); doc.name.push_str(" externally edited");
        std::fs::write(store.file(&doc.id), serde_json::to_vec(&doc).unwrap()).unwrap();
        let blocking = dir.join(".revision-sources").with_extension("tmp");
        std::fs::create_dir(&blocking).unwrap();
        store.reload();
        assert!(store.get(&doc.id).is_none());
        assert!(store.errors().iter().any(|error| error.contains("来源未能持久保存")));
        assert_eq!(store.revision_sources().unwrap()[&doc.id.to_ascii_lowercase()].version, 1);
        std::fs::remove_dir(&blocking).unwrap();
        store.reload();
        assert_eq!(store.get(&doc.id).unwrap().version, 2);
        drop(store);
        let store = RecipeStore::open(dir.clone()).unwrap();
        assert_eq!(store.get(&doc.id).unwrap().version, 2);
        let mut published = store.doc(&doc.id).unwrap(); published.version = 3; published.name.push_str(" published");
        std::fs::create_dir(&blocking).unwrap();
        assert!(store.save_published(published.clone(), Some(&format!("{}-v2", doc.id))).is_err());
        assert_eq!(store.get(&doc.id).unwrap().version, 2);
        std::fs::remove_dir(&blocking).unwrap();
        assert_eq!(store.save_published(published, Some(&format!("{}-v2", doc.id))).unwrap().version, 3);
        std::fs::write(dir.join(".revision-sources"), "invalid").unwrap();
        assert!(RecipeStore::open(dir.clone()).err().unwrap().contains("来源记录损坏"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn newly_created_recipe_source_failure_can_retry_both_save_paths_without_reload() {
        for published in [false, true] {
            let dir = std::env::temp_dir().join(format!("gluesight-new-source-retry-{published}-{}-{}", std::process::id(), ly_plc::now_ms()));
            let store = RecipeStore::open(dir.clone()).unwrap();
            let mut doc = three_cameras(); doc.id = "NEW-SOURCE-RETRY".into(); doc.product_code = 60006;
            doc.plan_version = store.plan_version_for(&doc).unwrap();
            let blocking = dir.join(".revision-sources").with_extension("tmp"); std::fs::create_dir(&blocking).unwrap();
            let failed = if published { store.save_published(doc.clone(), None) } else { store.save(doc.clone(), None) };
            assert!(failed.unwrap_err().contains("本次新建配方文件已回滚"));
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
    fn failed_source_save_keeps_preexisting_recipe_files_and_new_targets_are_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("gluesight-existing-source-failure-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = store.doc("MTR-HSG-B").unwrap(); doc.name.push_str(" saved update");
        let blocking = dir.join(".revision-sources").with_extension("tmp"); std::fs::create_dir(&blocking).unwrap();
        assert!(store.save(doc.clone(), None).is_err());
        assert!(store.file(&doc.id).is_file());
        assert_eq!(store.get(&doc.id).unwrap().version, 1);
        assert_eq!(serde_json::from_str::<RecipeDoc>(&crate::fsio::read_text(&store.file(&doc.id)).unwrap()).unwrap().version, 2);
        doc.id = "EXTERNAL-TARGET".into(); doc.product_code = 60007;
        let text = serde_json::to_string(&doc).unwrap(); std::fs::write(store.file(&doc.id), &text).unwrap();
        assert!(store.write_with_source(&doc, true).unwrap_err().contains("拒绝覆盖"));
        assert!(store.save(doc.clone(), None).is_err()); assert!(store.save_published(doc.clone(), None).is_err());
        assert_eq!(crate::fsio::read_text(&store.file(&doc.id)).unwrap(), text);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn new_recipe_rollback_preserves_external_changes_and_reports_delete_failures() {
        let dir = std::env::temp_dir().join(format!("gluesight-rollback-source-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = three_cameras(); doc.id = "ROLLBACK-NEW".into(); doc.product_code = 60008;
        let mut changed = doc.clone(); changed.limits.max_gap_len += 1.0;
        store.write(&changed).unwrap();
        assert!(store.rollback_new_file(&doc).unwrap_err().contains("外部修改"));
        assert_eq!(serde_json::from_str::<RecipeDoc>(&crate::fsio::read_text(&store.file(&doc.id)).unwrap()).unwrap(), changed);
        std::fs::remove_file(store.file(&doc.id)).unwrap(); std::fs::create_dir(store.file(&doc.id)).unwrap();
        assert!(store.rollback_new_file(&doc).unwrap_err().contains("类型已变化"));
        std::fs::remove_dir(store.file(&doc.id)).unwrap(); store.write(&doc).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let held = std::fs::OpenOptions::new().read(true).share_mode(1 | 2).open(store.file(&doc.id)).unwrap();
            assert!(store.rollback_new_file(&doc).unwrap_err().contains("回滚删除失败"));
            assert!(store.file(&doc.id).is_file()); drop(held);
        }
        store.rollback_new_file(&doc).unwrap(); assert!(!store.file(&doc.id).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn published_versions_are_assigned_once_and_crash_retries_cannot_overwrite_a_revision() {
        let dir = std::env::temp_dir().join(format!("gluesight-published-version-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = three_cameras();
        doc.id = "EXPLICIT-PUBLISHED".into();
        doc.product_code = 60000;
        doc.version = 7;
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
    fn tricam_recipe_requires_an_explicit_valid_view_and_keeps_one_device() {
        let mut doc = samples().remove(1);
        for (shot, view) in doc.shots.iter_mut().zip([1, 2, 3, 1]) { shot.view = view; }
        let encoded = serde_json::to_string(&doc).unwrap();
        let decoded: RecipeDoc = serde_json::from_str(&encoded).unwrap();
        let recipe = decoded.build().unwrap();
        assert_eq!(recipe.cameras(), ["cam1"]);
        assert_eq!(recipe.shots.iter().map(|s| s.view).collect::<Vec<_>>(), [1, 2, 3, 1]);
        for invalid in [0, 4, 255] {
            let mut bad = doc.clone();
            bad.shots[0].view = invalid;
            assert!(bad.build().unwrap_err().contains("视角"));
        }
        let mut missing = serde_json::to_value(&doc).unwrap();
        missing["shots"][0].as_object_mut().unwrap().remove("view");
        assert!(serde_json::from_value::<RecipeDoc>(missing).is_err());
        doc.schema_version = 3;
        assert!(doc.build().unwrap_err().contains("格式版本"));
    }

    #[test]
    fn invalid_recipes_are_rejected() {
        let reject = |f: &dyn Fn(&mut RecipeDoc), what: &str| {
            let mut doc = three_cameras();
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
        let pose_shared = {
            let mut d = three_cameras();
            d.shots[1].pose_id = "P1".into();
            d
        };
        assert!(pose_shared.build().is_ok(), "同一 Pose 可以触发两台相机");
    }
}
