//! 配方：磁盘上存可编辑的 `RecipeDoc`，加载时生成运行用的 `Recipe`（分段、测量点、修订标识）。
//! 一期按拍照点在图像里检测（P0 D-10）：每个拍照点示教一条胶路中线（图像像素），沿线每隔 spacing 一站；
//! 每个拍照点自成一段，判定在段内做，段与段之间不连。`Recipe` 同时是检测记录里的配方快照。

use std::collections::HashSet;
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
    pub first: usize,
    pub count: usize,
    pub position: Option<JudgeParams>,
    pub width: Option<JudgeParams>,
    pub max_gap_len: f32,
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
pub const RECIPE_SCHEMA: u32 = 4;

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
    /// 标定文件的键：标定引用，缺省为相机编号。
    pub fn calib_ref(&self) -> &str {
        self.calib.as_deref().unwrap_or(&self.camera)
    }

    /// 要量要判：不是"不检"。
    pub fn measured(&self) -> bool {
        !self.skip
    }

    /// 已示教：有中线和像素当量。
    pub fn taught(&self) -> bool {
        self.path.len() >= 2 && self.mm_per_px.is_some()
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
        if self.path.len() == 1 || (self.path.len() >= 2 && self.path_len_px() < 1.0) {
            return Err(format!("拍照点 {id} 的中线至少两个点、长度不能为零"));
        }
        if let Some(m) = self.mm_per_px.filter(|m| !(m.is_finite() && *m > 0.0 && *m <= 10.0)) {
            return Err(format!("拍照点 {id} 的像素当量 {m} 需在 0–10 mm/px 之间"));
        }
        if !self.path.is_empty() && self.mm_per_px.is_none() {
            return Err(format!("拍照点 {id} 有中线却没有像素当量"));
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
        let untaught: Vec<&str> = self.shots.iter().filter(|s| s.measured() && !s.taught()).map(|s| s.id.as_str()).collect();
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
        if self.shots.is_empty() || self.shots.len() > 64 {
            return Err("配方需要 1–64 个拍照点".into());
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
            if !shot.measured() || !shot.taught() {
                continue;
            }
            let step = self.spacing / shot.mm_per_px.unwrap();
            let len = shot.path_len_px();
            let count = (len / step + 1e-3).floor() as usize + 1;
            if count < 3 {
                return Err(format!("拍照点 {} 的中线只有 {:.1} mm，至少要 {:.1} mm", shot.id, len * shot.mm_per_px.unwrap(), 2.0 * self.spacing));
            }
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
                name: format!("{} · {}", shot.id, shot.bead),
                shot: k,
                first,
                count,
                position: limits.position.clone(),
                width: limits.width.clone(),
                max_gap_len: limits.max_gap_len,
            });
        }
        let recipe = Recipe {
            id: self.id.clone(),
            name: self.name.trim().to_string(),
            version: self.version.max(1),
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
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建配方目录失败：{e}"))?;
        let store = Self { dir, inner: RwLock::new(Vec::new()), errors: RwLock::new(Vec::new()) };
        let empty = std::fs::read_dir(&store.dir).map_err(|e| e.to_string())?.flatten().all(|e| e.path().extension().is_none_or(|x| x != "json"));
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

    fn write(&self, doc: &RecipeDoc) -> Result<(), String> {
        crate::fsio::write_atomic(&self.file(&doc.id), &serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?)
    }

    /// 重读目录。坏文件跳过并记下原因，不影响其他配方。
    pub fn reload(&self) {
        let mut list = Vec::new();
        let mut errors = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
            paths.sort();
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
                    match list.iter().find(|(o, _): &&(RecipeDoc, Arc<Recipe>)| o.id.eq_ignore_ascii_case(&d.id) || o.product_code == d.product_code) {
                        Some((o, _)) if o.product_code == d.product_code => Err(format!("产品代码 {} 与配方 {} 重复，没有加载", d.product_code, o.id)),
                        Some((o, _)) => Err(format!("配方编号与 {} 重复，没有加载", o.id)),
                        None => Ok((d, r)),
                    }
                });
                match checked {
                    Ok(pair) => list.push(pair),
                    Err(e) => errors.push(format!("{name}：{e}")),
                }
            }
        }
        *self.inner.write().unwrap() = list;
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
        doc.version = match old {
            Some(version) => version.checked_add(1).ok_or("配方版本已达上限")?,
            None => doc.version.max(1),
        };
        let recipe = Arc::new(doc.build()?);
        self.write(&doc)?;
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
        let expected_version = match current {
            Some((previous, _)) => previous.version.checked_add(1).ok_or("配方版本已达上限")?,
            None => doc.version.max(1),
        };
        if doc.version != expected_version { return Err("发布版本必须是明确分配的下一版本".into()); }
        if inner.iter().any(|(previous, _)| previous.id != doc.id &&
            (previous.id.eq_ignore_ascii_case(&doc.id) || previous.product_code == doc.product_code)) {
            return Err("配方编号或产品代码已被其他配方使用".into());
        }
        if current.is_none() && self.file(&doc.id).exists() {
            return Err("配方目录已有未加载文件，不能覆盖".into());
        }
        let recipe = Arc::new(built);
        self.write(&doc)?;
        inner.retain(|(previous, _)| previous.id != doc.id);
        inner.push((doc, recipe.clone()));
        inner.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        Ok(recipe)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.write().unwrap();
        if !inner.iter().any(|(d, _)| d.id == id) {
            return Err("配方不存在".into());
        }
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
        doc.shots[3].limits = Some(ShotLimits { position: None, width: None, max_gap_len: 9.0 });
        let r = doc.build().unwrap();
        assert_eq!(r.segments[0].max_gap_len, 6.0);
        assert!(r.segments[0].width.is_some());
        assert_eq!((r.segments[3].max_gap_len, r.segments[3].width.is_none()), (9.0, true));
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
        let mut legacy = serde_json::to_value(&*second).unwrap();
        legacy.as_object_mut().unwrap().remove("revisionId");
        legacy["hash"] = serde_json::json!("old-content-value");
        legacy["revisionId"] = serde_json::json!("untrusted-external-revision");
        let restored: Recipe = serde_json::from_value(legacy).unwrap();
        assert_eq!(restored.revision_id, second.revision_id);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_missing_or_zero_version_reloads_as_one_and_next_save_is_two() {
        for missing in [false, true] {
            let dir = std::env::temp_dir().join(format!("gluesight-legacy-version-{}-{}-{missing}", std::process::id(), ly_plc::now_ms()));
            let store = RecipeStore::open(dir.clone()).unwrap();
            let mut doc = three_cameras();
            doc.id = "LEGACY-VERSION".into();
            doc.product_code = 60001;
            doc.version = 0;
            let mut value = serde_json::to_value(&doc).unwrap();
            if missing { value.as_object_mut().unwrap().remove("version"); }
            std::fs::write(store.file(&doc.id), serde_json::to_vec(&value).unwrap()).unwrap();
            store.reload();
            assert!(store.errors().is_empty(), "{:?}", store.errors());
            assert_eq!(store.get(&doc.id).unwrap().revision_id, "LEGACY-VERSION-v1");
            let loaded = store.doc(&doc.id).unwrap();
            assert_eq!(loaded.version, 1);
            assert_eq!(store.save(loaded, Some(&doc.id)).unwrap().revision_id, "LEGACY-VERSION-v2");
            store.reload();
            assert_eq!(store.doc(&doc.id).unwrap().version, 2);
            assert_eq!(store.get(&doc.id).unwrap().version, 2);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn published_versions_are_assigned_once_and_crash_retries_cannot_overwrite_a_revision() {
        let dir = std::env::temp_dir().join(format!("gluesight-published-version-{}-{}", std::process::id(), ly_plc::now_ms()));
        let store = RecipeStore::open(dir.clone()).unwrap();
        let mut doc = three_cameras();
        doc.id = "EXPLICIT-PUBLISHED".into();
        doc.product_code = 60000;
        doc.version = 7;
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
        reject(&|d| d.shots[0].path.truncate(1), "至少两个点");
        reject(&|d| d.shots[0].mm_per_px = None, "像素当量");
        reject(&|d| d.shots[0].mm_per_px = Some(0.0), "像素当量");
        reject(&|d| d.shots[0].path[1] = [975.0, 480.0], "至少要");
        reject(&|d| d.shots[1].detect = Some(DetectParams { search_mm: 3.0, polarity: Polarity::Dark, width_range: [1.0, 8.0] }), "搜索宽度");
        reject(&|d| d.limits.max_gap_len = -1.0, "断胶长度");
        reject(&|d| d.schema_version = 2, "格式版本");
        reject(&|d| d.shots.clear(), "1–64");
        let pose_shared = {
            let mut d = three_cameras();
            d.shots[1].pose_id = "P1".into();
            d
        };
        assert!(pose_shared.build().is_ok(), "同一 Pose 可以触发两台相机");
    }
}
