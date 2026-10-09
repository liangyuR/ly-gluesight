//! 配方：磁盘上存可编辑的 `RecipeDoc`，加载时生成运行用的 `Recipe`（分段、测量点、哈希）。
//! `Recipe` 同时是检测记录里的配方快照，新增字段都要带默认值，旧快照才能读回来。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SegmentKind {
    Line,
    Corner,
}

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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub name: String,
    pub kind: SegmentKind,
    pub s0: f32,
    pub s1: f32,
    /// 位置：内边→胶中线距离
    pub params: JudgeParams,
    /// 胶宽；为空时不判胶宽
    #[serde(default)]
    pub width: Option<JudgeParams>,
}

/// 名义胶路上的测量点，按弧长等间距排列，第 j 个点的弧长为 j * spacing。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PathPoints {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub seg: Vec<u16>,
    /// 负责该点的拍照点
    pub k: Vec<u8>,
}

/// 名义胶路的几何。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PathSpec {
    RoundedRect { width: f32, height: f32, radius: f32 },
    /// 折线，两条直边之间的拐角按 radius 倒圆（0 为尖角）。
    /// bulges[i] 不为 0 时第 i 条边（点 i → 点 i+1）是圆弧，取 DXF 的约定：tan(圆心角/4)，正值逆时针。
    Polyline {
        points: Vec<[f32; 2]>,
        closed: bool,
        radius: f32,
        #[serde(default)]
        bulges: Vec<f32>,
    },
}

/// 旧文件里相机写的是相机组序号 k，读进来当作编号 "cam{k+1}"（相机组迁移时就是按位置这样编的号）。
pub fn legacy_camera_id(k: u8) -> String {
    format!("cam{}", k as u32 + 1)
}

fn default_camera() -> String {
    legacy_camera_id(0)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CameraRef {
    Index(u8),
    Id(String),
}

impl CameraRef {
    fn id(self) -> String {
        match self {
            CameraRef::Index(k) => legacy_camera_id(k),
            CameraRef::Id(s) => s,
        }
    }
}

fn camera_ref<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(CameraRef::deserialize(d)?.id())
}

/// 相机、配方编号：字母、数字、- 和 _，最长 32 个字符。
pub fn valid_camera_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teaching_hash: Option<String>,
    pub product_code: u16,
    pub trigger_mode: TriggerMode,
    /// 工件外形：宽、高、圆角半径（mm）；折线胶路为包围盒宽高、半径 0
    pub part: [f32; 3],
    #[serde(default)]
    pub path: Option<PathSpec>,
    #[serde(default = "yes")]
    pub closed: bool,
    pub fov: [f32; 2],
    pub shots: Vec<[f32; 2]>,
    /// 飞拍用的相机（相机编号）
    #[serde(default = "default_camera", deserialize_with = "camera_ref")]
    pub camera: String,
    pub spacing: f32,
    pub filter_window: usize,
    pub max_gap_len: f32,
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

    /// 胶路全长。闭合胶路的最后一个点到第一个点还有一个间距。
    pub fn length(&self) -> f32 {
        let n = self.point_count() as f32;
        if self.closed { n * self.spacing } else { (n - 1.0).max(0.0) * self.spacing }
    }

    /// 只看胶路几何与拍照点的哈希：示教资料跟它走，改判定限值不用重新示教。
    pub fn geometry_hash(&self) -> String {
        let key = serde_json::json!([self.path, self.part, self.spacing, self.shots, self.fov, self.closed]);
        fnv_hex(&serde_json::to_vec(&key).unwrap_or_default())
    }

    /// 本配方要用到的相机（编号）。
    pub fn cameras(&self) -> Vec<String> {
        vec![self.camera.clone()]
    }

    /// 弧长 s 处的名义位置。闭合胶路按周长取模；开放胶路超出两端时沿端点切向外推。
    pub fn pos(&self, s: f32) -> [f32; 2] {
        let n = self.point_count();
        if n == 0 {
            return [0.0, 0.0];
        }
        if n == 1 {
            return [self.points.x[0], self.points.y[0]];
        }
        let sp = self.spacing;
        let p = |j: usize| [self.points.x[j], self.points.y[j]];
        if self.closed {
            let s = s.rem_euclid(self.length());
            let j = ((s / sp) as usize).min(n - 1);
            let t = s / sp - j as f32;
            let (a, b) = (p(j), p((j + 1) % n));
            return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        }
        let j = ((s / sp).floor().max(0.0) as usize).min(n - 2);
        let t = s / sp - j as f32;
        let (a, b) = (p(j), p(j + 1));
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    }

    /// 弧长 s 处的单位切向（沿涂胶方向）。
    pub fn tangent(&self, s: f32) -> [f32; 2] {
        let h = self.spacing * 0.5;
        let (a, b) = (self.pos(s - h), self.pos(s + h));
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-6);
        [dx / l, dy / l]
    }

    /// 横向偏移的正方向：切向顺时针转 90°（y 向下的坐标里是涂胶方向的左手侧）。
    pub fn normal(&self, s: f32) -> [f32; 2] {
        let [tx, ty] = self.tangent(s);
        [ty, -tx]
    }

    /// normal 乘上它得到指向闭合胶路外侧的法向：绕向与圆角矩形相同（有向面积为正）时 normal 本来就朝外。
    /// 开放胶路没有内外，取 1。
    pub fn outward_sign(&self) -> f32 {
        let (x, y) = (&self.points.x, &self.points.y);
        let n = x.len();
        let area: f32 = (0..n).map(|i| x[i] * y[(i + 1) % n] - x[(i + 1) % n] * y[i]).sum();
        if self.closed && area < 0.0 { -1.0 } else { 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentLimits {
    pub position: JudgeParams,
    #[serde(default)]
    pub width: Option<JudgeParams>,
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
    pub teaching_hash: Option<String>,
    pub product_code: u16,
    pub trigger_mode: TriggerMode,
    #[serde(default = "default_camera", deserialize_with = "camera_ref")]
    pub camera: String,
    pub path: PathSpec,
    pub spacing: f32,
    pub filter_window: usize,
    pub max_gap_len: f32,
    pub line: SegmentLimits,
    pub corner: SegmentLimits,
    /// 按段名覆盖的限值
    #[serde(default)]
    pub segment_overrides: BTreeMap<String, SegmentLimits>,
    #[serde(default)]
    pub fov: [f32; 2],
    #[serde(default)]
    pub shots: Vec<[f32; 2]>,
}

#[derive(Clone, Copy, Debug)]
enum Piece {
    Line { a: [f32; 2], b: [f32; 2] },
    Arc { c: [f32; 2], r: f32, a0: f32, sweep: f32 },
}

impl Piece {
    fn len(&self) -> f32 {
        match *self {
            Piece::Line { a, b } => ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt(),
            Piece::Arc { r, sweep, .. } => r * sweep.abs(),
        }
    }

    fn at(&self, t: f32) -> [f32; 2] {
        match *self {
            Piece::Line { a, b } => {
                let l = self.len().max(1e-9);
                [a[0] + (b[0] - a[0]) * t / l, a[1] + (b[1] - a[1]) * t / l]
            }
            Piece::Arc { c, r, a0, sweep } => {
                let a = a0 + sweep.signum() * t / r;
                [c[0] + r * a.cos(), c[1] + r * a.sin()]
            }
        }
    }
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn unit(v: [f32; 2]) -> [f32; 2] {
    let l = (v[0] * v[0] + v[1] * v[1]).sqrt().max(1e-9);
    [v[0] / l, v[1] / l]
}

/// 两点之间的圆弧边。bulge = tan(圆心角/4)，正值时从 a 到 b 逆时针（角度增大的方向）。
fn bulge_arc(a: [f32; 2], b: [f32; 2], bulge: f32) -> Piece {
    let chord = sub(b, a);
    let l = (chord[0] * chord[0] + chord[1] * chord[1]).sqrt();
    let theta = 4.0 * bulge.atan();
    let r = l / (2.0 * (theta / 2.0).sin().abs());
    // 圆心在弦中点沿弦的左法向偏 d（带符号：逆时针圆弧圆心在左）
    let d = l / (2.0 * (theta / 2.0).tan());
    let c = [(a[0] + b[0]) / 2.0 - chord[1] / l * d, (a[1] + b[1]) / 2.0 + chord[0] / l * d];
    Piece::Arc { c, r, a0: (a[1] - c[1]).atan2(a[0] - c[0]), sweep: theta }
}

/// 折线按半径倒圆（带 bulge 的边画成圆弧），得到依次首尾相接的直线段与圆弧。
fn fillet(points: &[[f32; 2]], closed: bool, radius: f32, bulges: &[f32], names: Option<&[&str]>) -> Result<Vec<(String, SegmentKind, Piece)>, String> {
    let n = points.len();
    if n < 2 || (closed && n < 3) {
        return Err("胶路至少要有 2 个点（闭合至少 3 个）".into());
    }
    let edges = if closed { n } else { n - 1 };
    let bulge = |e: usize| bulges.get(e).copied().filter(|b| b.abs() > 1e-6);
    let dir: Vec<[f32; 2]> = (0..edges).map(|i| unit(sub(points[(i + 1) % n], points[i]))).collect();
    let elen: Vec<f32> = (0..edges).map(|i| Piece::Line { a: points[i], b: points[(i + 1) % n] }.len()).collect();
    if elen.iter().any(|&l| l < 1e-3) {
        return Err("胶路有重合的相邻点".into());
    }
    // 每个顶点：进、出切点与圆弧（没有拐角或半径为 0 时切点就是顶点）
    let corner = |i: usize| -> ([f32; 2], [f32; 2], Option<Piece>) {
        let v = points[i];
        let interior = closed || (i > 0 && i < n - 1);
        // 圆弧边两端不倒角：CAD 里圆弧与直边本来就相切
        if !interior || radius <= 0.0 || bulge((i + edges - 1) % edges).is_some() || bulge(i % edges).is_some() {
            return (v, v, None);
        }
        let (din, dout) = (dir[(i + edges - 1) % edges], dir[i % edges]);
        let phi = (din[0] * dout[1] - din[1] * dout[0]).atan2(din[0] * dout[0] + din[1] * dout[1]);
        if phi.abs() < 1e-4 {
            return (v, v, None);
        }
        let half = (phi.abs() / 2.0).tan();
        let t = (radius * half).min(elen[(i + edges - 1) % edges] / 2.0).min(elen[i % edges] / 2.0);
        let r = t / half;
        let t1 = [v[0] - din[0] * t, v[1] - din[1] * t];
        let t2 = [v[0] + dout[0] * t, v[1] + dout[1] * t];
        let s = phi.signum();
        let c = [t1[0] - din[1] * r * s, t1[1] + din[0] * r * s];
        let a0 = (t1[1] - c[1]).atan2(t1[0] - c[0]);
        (t1, t2, Some(Piece::Arc { c, r, a0, sweep: phi }))
    };
    let corners: Vec<_> = (0..n).map(corner).collect();
    let mut pieces = Vec::new();
    // 第 e 条边与它末端的拐角；给了名字表时按"边、拐角、边、拐角…"的顺序取
    let name = |kind: SegmentKind, e: usize| -> String {
        let i = if kind == SegmentKind::Line { e * 2 } else { e * 2 + 1 };
        match (names.and_then(|n| n.get(i)), kind) {
            (Some(nm), _) => nm.to_string(),
            (None, SegmentKind::Line) => format!("边 {}", e + 1),
            (None, SegmentKind::Corner) => format!("拐角 {}", e + 1),
        }
    };
    for e in 0..edges {
        if let Some(b) = bulge(e) {
            pieces.push((format!("圆弧 {}", e + 1), SegmentKind::Corner, bulge_arc(points[e], points[(e + 1) % n], b)));
            continue;
        }
        let (a, b) = (corners[e].1, corners[(e + 1) % n].0);
        let line = Piece::Line { a, b };
        if line.len() > 1e-4 {
            pieces.push((name(SegmentKind::Line, e), SegmentKind::Line, line));
        }
        if let Some(arc) = corners[(e + 1) % n].2 {
            if closed || e + 1 < n - 1 {
                pieces.push((name(SegmentKind::Corner, e), SegmentKind::Corner, arc));
            }
        }
    }
    Ok(pieces)
}

const RECT_NAMES: [&str; 8] = ["长边 A", "R 角 1", "短边 B", "R 角 2", "长边 C", "R 角 3", "短边 D", "R 角 4"];

impl RecipeDoc {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_camera_id(&self.id) {
            return Err("配方编号只能用字母、数字、- 和 _，最长 32 个字符".into());
        }
        if self.name.trim().is_empty() {
            return Err("配方名称不能为空".into());
        }
        if !(0.1..=5.0).contains(&self.spacing) {
            return Err("测量点间距需在 0.1–5 mm 之间".into());
        }
        if self.filter_window == 0 || self.filter_window > 31 || self.filter_window % 2 == 0 {
            return Err("中值滤波窗口需为 1–31 的奇数".into());
        }
        if !(self.max_gap_len >= 0.0) {
            return Err("允许断胶长度不能为负".into());
        }
        if !valid_camera_id(&self.camera) {
            return Err("飞拍相机编号只能用字母、数字、- 和 _".into());
        }
        for (what, l) in [("直线段", &self.line), ("拐角", &self.corner)].into_iter().chain(self.segment_overrides.iter().map(|(k, v)| (k.as_str(), v))) {
            l.position.validate(&format!("{what} 位置"))?;
            if let Some(w) = &l.width {
                w.validate(&format!("{what} 胶宽"))?;
            }
        }
        match self.path {
            PathSpec::RoundedRect { width, height, radius } => {
                if !(width > 0.0 && height > 0.0 && radius >= 0.0 && 2.0 * radius <= width.min(height)) {
                    return Err("圆角矩形的宽高需为正，圆角半径不超过短边一半".into());
                }
            }
            PathSpec::Polyline { ref points, radius, ref bulges, .. } => {
                if points.iter().flatten().any(|v| !v.is_finite()) || radius < 0.0 || bulges.iter().any(|b| !b.is_finite()) {
                    return Err("胶路点坐标与圆弧参数必须是有限数，倒圆半径不能为负".into());
                }
                if bulges.len() > points.len() {
                    return Err("圆弧参数比胶路边数还多".into());
                }
            }
        }
        if self.shots.is_empty() || self.shots.len() > 64 {
            return Err("飞拍配方需要 1–64 个拍照点".into());
        }
        if !(self.fov[0] > 0.0 && self.fov[1] > 0.0) {
            return Err("视野宽高需为正".into());
        }
        Ok(())
    }

    pub fn build(&self) -> Result<Recipe, String> {
        self.validate()?;
        let (pieces, closed, part) = match self.path {
            PathSpec::RoundedRect { width: w, height: h, radius: r } => {
                let pts = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
                (fillet(&pts, true, r, &[], Some(&RECT_NAMES))?, true, [w, h, r])
            }
            PathSpec::Polyline { ref points, closed, radius, ref bulges } => {
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for p in points {
                    (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
                }
                let (pts, bs) = tidy(points, bulges, closed);
                (fillet(&pts, closed, radius, &bs, None)?, closed, [x1 - x0, y1 - y0, 0.0])
            }
        };
        let limits = |name: &str, kind: SegmentKind| {
            self.segment_overrides.get(name).cloned().unwrap_or_else(|| if kind == SegmentKind::Line { self.line.clone() } else { self.corner.clone() })
        };
        let mut segments = Vec::new();
        let mut s = 0.0;
        for (name, kind, piece) in &pieces {
            let l = limits(name, *kind);
            let len = piece.len();
            segments.push(Segment { name: name.clone(), kind: *kind, s0: s, s1: s + len, params: l.position, width: l.width });
            s += len;
        }
        let total = s;
        if total < 2.0 * self.spacing {
            return Err("胶路太短".into());
        }
        if total / self.spacing > 200_000.0 {
            return Err("测量点超过 20 万个，加大间距".into());
        }
        let mut points = PathPoints::default();
        let mut j = 0usize;
        loop {
            let s = j as f32 * self.spacing;
            if s >= total - if closed { 1e-4 } else { -1e-4 } {
                break;
            }
            let gi = segments.iter().position(|g| s < g.s1).unwrap_or(segments.len() - 1);
            let [x, y] = pieces[gi].2.at((s - segments[gi].s0).min(pieces[gi].2.len()));
            let owner = self
                .shots
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| ((x - a[0]).powi(2) + (y - a[1]).powi(2)).total_cmp(&((x - b[0]).powi(2) + (y - b[1]).powi(2))))
                .map_or(0, |(k, _)| k);
            points.x.push(x);
            points.y.push(y);
            points.seg.push(gi as u16);
            points.k.push(owner as u8);
            j += 1;
        }
        if segments.len() > u16::MAX as usize {
            return Err("胶路分段太多".into());
        }
        let mut recipe = Recipe {
            id: self.id.clone(),
            name: self.name.trim().to_string(),
            version: 0,
            hash: String::new(),
            teaching_hash: self.teaching_hash.clone(),
            product_code: self.product_code,
            trigger_mode: self.trigger_mode,
            part,
            path: Some(self.path.clone()),
            closed,
            fov: self.fov,
            shots: self.shots.clone(),
            camera: self.camera.clone(),
            spacing: self.spacing,
            filter_window: self.filter_window,
            max_gap_len: self.max_gap_len,
            segments,
            points,
        };
        recipe.hash = content_hash(&recipe);
        recipe.version = self.version.max(1);
        Ok(recipe)
    }
}

/// 只看内容的哈希（不含版本号），检测记录按它存配方快照。
fn content_hash(recipe: &Recipe) -> String {
    fnv_hex(&serde_json::to_vec(recipe).unwrap_or_default())
}

/// FNV-1a 64 位，十六进制。
fn fnv_hex(bytes: &[u8]) -> String {
    let h = bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3));
    format!("{:016x}", h)
}

fn line_limits() -> SegmentLimits {
    SegmentLimits {
        position: JudgeParams { nominal: 0.75, tol_upper: 0.75, tol_lower: 0.75, abs_min: 0.0, abs_max: 2.0, max_excursion_len: 2.0 },
        width: None,
    }
}

fn corner_limits() -> SegmentLimits {
    SegmentLimits {
        position: JudgeParams { nominal: 0.75, tol_upper: 1.0, tol_lower: 1.0, abs_min: 0.0, abs_max: 2.2, max_excursion_len: 3.0 },
        width: None,
    }
}

/// 首次启动写入的样例配方。
pub fn samples() -> Vec<RecipeDoc> {
    let fly = |id: &str, name: &str, code: u16, w: f32, h: f32, r: f32, shots: Vec<[f32; 2]>, trigger_mode| RecipeDoc {
        id: id.into(),
        name: name.into(),
        version: 1,
        teaching_hash: None,
        product_code: code,
        trigger_mode,
        camera: default_camera(),
        path: PathSpec::RoundedRect { width: w, height: h, radius: r },
        spacing: 0.5,
        filter_window: 5,
        max_gap_len: 0.5,
        line: line_limits(),
        corner: corner_limits(),
        segment_overrides: BTreeMap::new(),
        fov: [216.0, 145.0],
        shots,
    };
    vec![
        fly(
            "MTR-HSG-A",
            "电机壳体 A",
            12,
            520.0,
            230.0,
            28.0,
            vec![[95.0, 57.5], [260.0, 57.5], [425.0, 57.5], [425.0, 172.5], [260.0, 172.5], [95.0, 172.5]],
            TriggerMode::Fly,
        ),
        fly("MTR-HSG-B", "电机壳体 B", 13, 380.0, 200.0, 24.0, vec![[95.0, 50.0], [285.0, 50.0], [285.0, 150.0], [95.0, 150.0]], TriggerMode::Stop),
    ]
}

/// 测试与重判里需要一份现成配方时用。
#[cfg(test)]
pub fn builtin() -> Vec<Arc<Recipe>> {
    samples().iter().map(|d| Arc::new(d.build().unwrap())).collect()
}

fn same_point(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() <= 1e-3 && (a[1] - b[1]).abs() <= 1e-3
}

/// 折线整理：去掉相邻重合点（它后面那条边的 bulge 归到保留下来的点上）；闭合时去掉与起点重合的末点
/// （CAD / CSV 导出的闭合轮廓常把起点再写一遍）；闭合却只剩两个点（整圆、两段半圆）时把圆弧边从中点剖开。
fn tidy(points: &[[f32; 2]], bulges: &[f32], closed: bool) -> (Vec<[f32; 2]>, Vec<f32>) {
    let (mut pts, mut bs): (Vec<[f32; 2]>, Vec<f32>) = (Vec::new(), Vec::new());
    for (i, &p) in points.iter().enumerate() {
        let b = bulges.get(i).copied().unwrap_or(0.0);
        if pts.last().is_some_and(|&q| same_point(q, p)) {
            if let Some(last) = bs.last_mut() {
                *last = b;
            }
            continue;
        }
        pts.push(p);
        bs.push(b);
    }
    if closed && pts.len() > 2 && same_point(pts[0], pts[pts.len() - 1]) {
        pts.pop();
        bs.pop();
    }
    if !(closed && pts.len() == 2 && bs.iter().any(|b| b.abs() > 1e-6)) {
        return (pts, bs);
    }
    let (mut out, mut out_bs) = (Vec::new(), Vec::new());
    for e in 0..2 {
        let (a, b, bulge) = (pts[e], pts[1 - e], bs[e]);
        out.push(a);
        if bulge.abs() <= 1e-6 {
            out_bs.push(0.0);
            continue;
        }
        // 圆弧中点在弦中点的右侧（正 bulge 逆时针，圆心在左），离弦 bulge·弦长/2；两半各转一半的角
        let half = (bulge.atan() / 2.0).tan();
        let c = sub(b, a);
        out.push([(a[0] + b[0]) / 2.0 + c[1] * bulge / 2.0, (a[1] + b[1]) / 2.0 - c[0] * bulge / 2.0]);
        out_bs.extend([half, half]);
    }
    (out, out_bs)
}

/// 从文件导入的胶路：折线点、各边的 bulge（直边为 0）、是否闭合。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPath {
    pub points: Vec<[f32; 2]>,
    pub bulges: Vec<f32>,
    pub closed: bool,
    /// 给操作员看的说明（例如文件里有几条路径、取了哪条）
    pub note: Option<String>,
}

impl ImportedPath {
    /// 首尾重合视为闭合；再按 tidy 整理。
    fn normalized(mut self) -> Result<Self, String> {
        let n = self.points.len();
        self.closed |= n > 2 && same_point(self.points[0], self.points[n - 1]);
        let (pts, mut bs) = tidy(&self.points, &self.bulges, self.closed);
        if pts.len() < 2 {
            return Err("文件里没读到至少 2 个点".into());
        }
        if !self.closed {
            bs.truncate(pts.len() - 1);
        }
        if bs.iter().all(|b| b.abs() < 1e-9) {
            bs.clear();
        }
        self.points = pts;
        self.bulges = bs;
        Ok(self)
    }

    /// 长度（圆弧按弧长）。
    fn length(&self) -> f32 {
        let n = self.points.len();
        let edges = if self.closed { n } else { n.saturating_sub(1) };
        (0..edges)
            .map(|e| {
                let (a, b) = (self.points[e], self.points[(e + 1) % n]);
                match self.bulges.get(e).copied().filter(|b| b.abs() > 1e-6) {
                    Some(bulge) => bulge_arc(a, b, bulge).len(),
                    None => Piece::Line { a, b }.len(),
                }
            })
            .sum()
    }
}

/// 从 CSV（每行 x, y，可有表头）或 DXF 的文本读出胶路。
/// DXF 读 ENTITIES 段里的 LWPOLYLINE / POLYLINE（含圆弧与闭合标记）、CIRCLE，以及首尾相连的 LINE / ARC；
/// 有多条路径时取最长的一条。
pub fn parse_path(text: &str, ext: &str) -> Result<ImportedPath, String> {
    if ext.eq_ignore_ascii_case("dxf") {
        dxf_path(text)
    } else {
        csv_path(text).normalized()
    }
}

fn csv_cells(line: &str) -> Vec<&str> {
    line.split([',', ';', '\t', ' ']).map(str::trim).filter(|s| !s.is_empty()).collect()
}

/// 第 3 列起的数默认不认（常见的是高度 z、序号）；表头里写明 bulge 的那一列才当圆弧参数（tan(圆心角/4)，正值逆时针）。
fn csv_path(text: &str) -> ImportedPath {
    let head = text.lines().map(csv_cells).find(|c| !c.is_empty()).filter(|c| c.iter().any(|s| s.parse::<f32>().is_err()));
    let bulge_col = head.and_then(|c| c.iter().position(|s| s.eq_ignore_ascii_case("bulge")));
    let mut path = ImportedPath::default();
    for l in text.lines() {
        let v: Vec<f32> = csv_cells(l).into_iter().map_while(|s| s.parse::<f32>().ok()).collect();
        if v.len() >= 2 && v[0].is_finite() && v[1].is_finite() {
            path.points.push([v[0], v[1]]);
            path.bulges.push(bulge_col.and_then(|c| v.get(c).copied()).filter(|b| b.is_finite()).unwrap_or(0.0));
        }
    }
    path
}

/// DXF 实体：组码 → 值（同一组码可能出现多次，按出现顺序）。
struct Entity<'a> {
    kind: &'a str,
    pairs: Vec<(i32, &'a str)>,
}

impl Entity<'_> {
    fn f(&self, code: i32) -> Option<f32> {
        self.pairs.iter().find(|(c, _)| *c == code).and_then(|(_, v)| v.parse().ok())
    }
}

fn dxf_entities(text: &str) -> Vec<Entity<'_>> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut out: Vec<Entity> = Vec::new();
    let mut section = "";
    let mut in_section_header = false;
    let mut i = 0;
    while i + 1 < lines.len() {
        let (Ok(code), value) = (lines[i].parse::<i32>(), lines[i + 1]) else {
            i += 1;
            continue;
        };
        i += 2;
        if code == 0 {
            match value {
                "SECTION" => in_section_header = true,
                "ENDSEC" => section = "",
                _ if section == "ENTITIES" => out.push(Entity { kind: value, pairs: Vec::new() }),
                _ => {}
            }
            continue;
        }
        if in_section_header && code == 2 {
            section = value;
            in_section_header = false;
            continue;
        }
        if section == "ENTITIES" {
            if let Some(e) = out.last_mut() {
                e.pairs.push((code, value));
            }
        }
    }
    out
}

/// 一条直边或圆弧边（ARC 实体转成两端点 + bulge）。
struct Edge {
    a: [f32; 2],
    b: [f32; 2],
    bulge: f32,
}

fn chain(mut edges: Vec<Edge>) -> Vec<ImportedPath> {
    let mut paths = Vec::new();
    while let Some(first) = edges.pop() {
        let mut pts = vec![first.a, first.b];
        let mut bs = vec![first.bulge];
        loop {
            let end = *pts.last().unwrap();
            let Some(i) = edges.iter().position(|e| same_point(e.a, end) || same_point(e.b, end)) else { break };
            let e = edges.swap_remove(i);
            if same_point(e.a, end) {
                pts.push(e.b);
                bs.push(e.bulge);
            } else {
                pts.push(e.a);
                bs.push(-e.bulge);
            }
        }
        loop {
            let start = pts[0];
            let Some(i) = edges.iter().position(|e| same_point(e.a, start) || same_point(e.b, start)) else { break };
            let e = edges.swap_remove(i);
            if same_point(e.b, start) {
                pts.insert(0, e.a);
                bs.insert(0, e.bulge);
            } else {
                pts.insert(0, e.b);
                bs.insert(0, -e.bulge);
            }
        }
        bs.push(0.0);
        paths.push(ImportedPath { points: pts, bulges: bs, closed: false, note: None });
    }
    paths
}

fn dxf_path(text: &str) -> Result<ImportedPath, String> {
    let entities = dxf_entities(text);
    let mut paths: Vec<ImportedPath> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut i = 0;
    while i < entities.len() {
        let e = &entities[i];
        match e.kind {
            "LWPOLYLINE" => {
                let mut p = ImportedPath { closed: e.f(70).is_some_and(|f| (f as i32) & 1 == 1), ..Default::default() };
                let mut x = None;
                for &(code, v) in &e.pairs {
                    match code {
                        10 => x = v.parse::<f32>().ok(),
                        20 => {
                            if let (Some(px), Ok(py)) = (x.take(), v.parse::<f32>()) {
                                p.points.push([px, py]);
                                p.bulges.push(0.0);
                            }
                        }
                        // bulge 跟在它所属的顶点之后，作用于从这个顶点出发的那条边
                        42 => {
                            if let (Some(b), Ok(v)) = (p.bulges.last_mut(), v.parse::<f32>()) {
                                *b = v;
                            }
                        }
                        _ => {}
                    }
                }
                paths.push(p);
            }
            "POLYLINE" => {
                let mut p = ImportedPath { closed: e.f(70).is_some_and(|f| (f as i32) & 1 == 1), ..Default::default() };
                while i + 1 < entities.len() && entities[i + 1].kind == "VERTEX" {
                    i += 1;
                    let v = &entities[i];
                    if let (Some(x), Some(y)) = (v.f(10), v.f(20)) {
                        p.points.push([x, y]);
                        p.bulges.push(v.f(42).unwrap_or(0.0));
                    }
                }
                paths.push(p);
            }
            "LINE" => {
                if let (Some(x0), Some(y0), Some(x1), Some(y1)) = (e.f(10), e.f(20), e.f(11), e.f(21)) {
                    edges.push(Edge { a: [x0, y0], b: [x1, y1], bulge: 0.0 });
                }
            }
            "ARC" => {
                if let (Some(cx), Some(cy), Some(r), Some(a0), Some(a1)) = (e.f(10), e.f(20), e.f(40), e.f(50), e.f(51)) {
                    let mut sweep = (a1 - a0).rem_euclid(360.0);
                    if sweep < 1e-6 {
                        sweep = 360.0;
                    }
                    let at = |deg: f32| [cx + r * deg.to_radians().cos(), cy + r * deg.to_radians().sin()];
                    edges.push(Edge { a: at(a0), b: at(a1), bulge: (sweep.to_radians() / 4.0).tan() });
                }
            }
            "CIRCLE" => {
                if let (Some(cx), Some(cy), Some(r)) = (e.f(10), e.f(20), e.f(40)) {
                    paths.push(ImportedPath { points: vec![[cx + r, cy], [cx - r, cy]], bulges: vec![1.0, 1.0], closed: true, note: None });
                }
            }
            _ => {}
        }
        i += 1;
    }
    paths.extend(chain(edges));
    let mut paths: Vec<ImportedPath> = paths.into_iter().filter_map(|p| p.normalized().ok()).collect();
    if paths.is_empty() {
        return Err("DXF 的 ENTITIES 段里没找到 LWPOLYLINE / POLYLINE / LINE / ARC / CIRCLE 组成的路径".into());
    }
    paths.sort_by(|a, b| b.length().total_cmp(&a.length()));
    let n = paths.len();
    let mut best = paths.swap_remove(0);
    if n > 1 {
        best.note = Some(format!("文件里有 {n} 条路径，取了最长的一条（约 {:.1} mm）", best.length()));
    }
    Ok(best)
}

#[cfg(test)]
mod import_tests {
    use super::*;

    #[test]
    fn closed_csv_repeating_first_point() {
        let p = parse_path("x,y,z\n0,0,5\n100,0,5\n100,50,5\n0,50,5\n0,0,5\n", "csv").unwrap();
        assert!(p.closed);
        assert_eq!(p.points.len(), 4);
        assert!(p.bulges.is_empty(), "z 列不能当 bulge：{:?}", p.bulges);
    }

    #[test]
    fn dxf_circle_builds_full_circle() {
        let p = parse_path("0\nSECTION\n2\nENTITIES\n0\nCIRCLE\n10\n50\n20\n50\n40\n20\n0\nENDSEC\n0\nEOF\n", "dxf").unwrap();
        let doc = RecipeDoc { path: PathSpec::Polyline { points: p.points, closed: p.closed, radius: 0.0, bulges: p.bulges }, ..samples()[0].clone() };
        let r = doc.build().unwrap();
        let total = r.segments.last().unwrap().s1;
        assert!((total - 2.0 * std::f32::consts::PI * 20.0).abs() < 0.05, "周长 {total}");
        assert!((r.pos(total / 4.0)[1] - 50.0).abs() > 15.0, "四分之一处应在圆的上下两端");
    }

    #[test]
    fn dxf_lines_and_arc_chain_into_rounded_path() {
        // 两条直线 + 一段 90° 圆弧（半径 10），故意打乱顺序、反向书写
        let dxf = "0\nSECTION\n2\nENTITIES\n\
                   0\nLINE\n10\n110\n20\n10\n11\n110\n21\n60\n\
                   0\nARC\n10\n100\n20\n10\n40\n10\n50\n270\n51\n0\n\
                   0\nLINE\n10\n0\n20\n0\n11\n100\n21\n0\n\
                   0\nENDSEC\n0\nEOF\n";
        let p = parse_path(dxf, "dxf").unwrap();
        assert!(!p.closed);
        assert_eq!(p.points.len(), 4);
        let doc = RecipeDoc { path: PathSpec::Polyline { points: p.points.clone(), closed: false, radius: 0.0, bulges: p.bulges.clone() }, ..samples()[0].clone() };
        let r = doc.build().unwrap();
        let total = r.segments.last().unwrap().s1;
        assert!((total - (100.0 + 50.0 + std::f32::consts::PI * 5.0)).abs() < 0.05, "全长 {total}");
        assert!(r.segments.iter().any(|g| g.kind == SegmentKind::Corner));
    }
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
                    .and_then(|d| d.build().map(|r| (d, Arc::new(r))));
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

    /// 保存配方。内容变了版本号 +1；编号（不分大小写：Windows 上文件名不分）与产品代码都不能和别的配方重复。
    pub fn save(&self, mut doc: RecipeDoc, original_id: Option<&str>) -> Result<Arc<Recipe>, String> {
        doc.name = doc.name.trim().to_string();
        let built = doc.build()?;
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
        let old = inner.iter().find(|(d, _)| d.id == replacing).map(|(d, r)| (d.version, r.hash.clone()));
        doc.version = match old {
            Some((v, h)) if h == built.hash => v,
            Some((v, _)) => v + 1,
            None => doc.version.max(1),
        };
        let recipe = Arc::new(Recipe { version: doc.version, ..built });
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
    use std::f32::consts::PI;

    #[test]
    fn rounded_rect_matches_perimeter() {
        let r = samples()[0].build().unwrap();
        let [w, h, rad] = r.part;
        let perimeter = 2.0 * (w + h - 4.0 * rad) + 2.0 * PI * rad;
        assert!((r.segments.last().unwrap().s1 - perimeter).abs() < 1e-2);
        assert_eq!(r.segments[1].name, "R 角 1");
        let p = r.pos(r.segments[1].s1);
        assert!((p[0] - w).abs() < 1e-2 && (p[1] - rad).abs() < 1e-2);
    }

    #[test]
    fn open_polyline_fillet() {
        let doc = RecipeDoc {
            path: PathSpec::Polyline { points: vec![[0.0, 0.0], [100.0, 0.0], [100.0, 50.0]], closed: false, radius: 10.0, bulges: Vec::new() },
            ..samples()[0].clone()
        };
        let r = doc.build().unwrap();
        assert!(!r.closed);
        assert_eq!(r.segments.len(), 3);
        let total = r.segments[2].s1;
        assert!((total - (90.0 + 40.0 + PI * 5.0)).abs() < 1e-2);
    }
}
