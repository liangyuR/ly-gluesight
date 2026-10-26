use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::fixture::{self, Pose};
use super::{Engine, FrameImage, RunResult};
use crate::burst_judge::{FrameMetrics, FrameState};
use crate::measure::{drop_reference, Job, Measured, ReferenceFailure, REFERENCE_END_SHIFT, REFERENCE_LOCATE_FAILED, ST_GAP, ST_NO_REF, ST_OK, ST_OUT_OF_RANGE};
use crate::recipe::{CameraInspect, DetectParams, FixtureSpec, Polarity, Recipe, FIXTURE_ANGLE_STEP_DEG, FIXTURE_TEMPLATE_PAD_PX};

/// 特征定位的结果（定稿 §10）：位姿是否采用、两端切向搬移与排除长度。随单图测量、试测记录与帧记录（`ViewResult.pose`）序列化；
/// 叠图时 T 由 offset、angle 与配方快照里的 `rect` 示教中心还原。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoseRecord {
    pub accepted: bool,
    /// 不采用的原因：glue.locate 的 reason，或开了转角却看不出角度时的 "angle_unobservable"
    pub reason: Option<String>,
    pub score: Option<f32>,
    pub peak_ratio: Option<f32>,
    pub angle: Option<f32>,
    /// 模板中心相对示教位置的平移 center − teachCenter（px）
    pub offset: Option<[f32; 2]>,
    /// 带符号的两端切向搬移 [Δ_起, Δ_止]（mm），沿胶路方向为正
    pub end_shift_mm: Option<[f32; 2]>,
    /// 两端各排除的长度（mm，从示教线 N 的端点量起，含示教余量 m）：max(m, |Δ|) + 1 个站距
    pub excluded_mm: Option<[f32; 2]>,
    /// 护栏：|Δ| 的上限（mm）
    pub end_limit_mm: f32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotMeasurement {
    pub idx: Vec<u32>,
    pub d: Vec<f32>,
    pub w: Vec<f32>,
    pub st: Vec<u8>,
    pub px: Vec<[f32; 2]>,
    pub edges: Vec<Option<[[f32; 2]; 2]>>,
    pub candidate_width: Vec<Option<f32>>,
    pub candidate_edges: Vec<Option<[[f32; 2]; 2]>>,
    pub coverage: f32,
    pub ms: u32,
    /// 用了特征定位的视图才有
    pub pose: Option<PoseRecord>,
    /// 这个视图没有位置基准的原因（有胶站已是 ST_NO_REF）
    pub reference_failed: Option<String>,
}

impl ShotMeasurement {
    fn empty(n: usize) -> Self {
        Self { idx: Vec::with_capacity(n), d: Vec::with_capacity(n), w: Vec::with_capacity(n), st: Vec::with_capacity(n), px: Vec::with_capacity(n),
            edges: Vec::with_capacity(n), candidate_width: Vec::with_capacity(n), candidate_edges: Vec::with_capacity(n), coverage: 0.0, ms: 0,
            pose: None, reference_failed: None }
    }

    pub fn into_measured(self, job: &Job) -> Measured {
        let mut m = Measured::empty(job);
        let view = job.recipe.shots[job.k].view;
        if let Some(reason) = self.reference_failed {
            m.reference_failed.push(ReferenceFailure { view, reason });
        }
        m.poses.extend(self.pose.map(|pose| (view, pose)));
        m.score = self.coverage;
        m.ms = self.ms;
        m.idx = self.idx;
        m.d = self.d;
        m.w = self.w;
        m.st = self.st;
        m.px = self.px;
        m
    }

    pub fn record(&self) -> Value {
        let status: Vec<_> = self.st.iter().map(|s| match *s {
            ST_OK => "ok",
            ST_NO_REF => "no_ref",
            ST_OUT_OF_RANGE => "out_of_range",
            _ => "no_bead",
        }).collect();
        json!({"unit": "mm", "ids": self.idx, "status": status, "innerCenter": self.d,
            "width": self.w, "px": self.px, "edges": self.edges, "candidateWidth": self.candidate_width,
            "candidateEdges": self.candidate_edges, "coverage": self.coverage, "ms": self.ms,
            "pose": self.pose, "referenceFailed": self.reference_failed})
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewMeasurement {
    pub view: u8,
    pub reading: Option<ShotMeasurement>,
    pub error: Option<String>,
}

pub struct MultiViewMeasurement {
    pub reading: ShotMeasurement,
    pub views: Vec<ViewMeasurement>,
}

impl MultiViewMeasurement {
    /// 各视图没有位置基准的原因（帧徽标、测量记录用）
    pub fn reference_failed(&self) -> Vec<ReferenceFailure> {
        self.views.iter().filter_map(|v| {
            let reason = v.reading.as_ref()?.reference_failed.clone()?;
            Some(ReferenceFailure { view: v.view, reason })
        }).collect()
    }

    pub fn into_measured(self, job: &Job) -> Measured {
        let reference_failed = self.reference_failed();
        let poses = self.views.iter().filter_map(|v| Some((v.view, v.reading.as_ref()?.pose.clone()?))).collect();
        let mut measured = self.reading.into_measured(job);
        measured.reference_failed = reference_failed;
        measured.poses = poses;
        measured.views = self.views;
        measured
    }
}

pub fn measure_views_with(
    recipe: &Recipe, k: usize, images: &[(u8, Arc<FrameImage>)],
    mut measure: impl FnMut(u8, &FrameImage) -> Result<ShotMeasurement, String>,
) -> Result<MultiViewMeasurement, String> {
    let shot = recipe.shots.get(k).ok_or("拍照点不存在")?;
    let selected = shot.enabled_views();
    for view in &selected {
        if images.iter().filter(|(v, _)| v == view).count() != 1 {
            return Err(format!("拍照点 {} 图 {view} 缺失或重复", shot.id));
        }
    }
    let mut out = ShotMeasurement::empty(0);
    let mut views = Vec::new();
    for view in selected {
        let image = &images.iter().find(|(v, _)| *v == view).unwrap().1;
        let expected: Vec<_> = recipe.view_points(k, view).collect();
        match measure(view, image) {
            Ok(reading) if reading.idx.iter().copied().map(|v| v as usize).eq(expected.iter().copied())
                && reading.st.len() == expected.len() && reading.d.len() == expected.len()
                && reading.w.len() == expected.len() && reading.px.len() == expected.len()
                && reading.edges.len() == expected.len()
                && reading.candidate_width.len() == expected.len() && reading.candidate_edges.len() == expected.len()
                && reading.candidate_width.iter().zip(&reading.candidate_edges).all(|(width, edge)| match (width, edge) {
                    (Some(width), Some(points)) => width.is_finite() && *width > 0.0 && points.iter().flatten().all(|v| v.is_finite()),
                    (None, None) => true,
                    _ => false,
                })
                // 有胶边 ⇔ 测到胶宽；测到胶的站只能是有基准、无基准或超出检测范围
                && reading.edges.iter().enumerate().all(|(i, edge)| edge.is_some() == reading.w[i].is_finite() && match edge {
                    Some(points) => matches!(reading.st[i], ST_OK | ST_NO_REF | ST_OUT_OF_RANGE) && points.iter().flatten().all(|v| v.is_finite()),
                    None => true,
                })
                && reading.coverage.is_finite() && (0.0..=1.0).contains(&reading.coverage)
                && reading.px.iter().flatten().all(|v| v.is_finite())
                && (0..reading.st.len()).all(|i| crate::measure::valid_station(reading.st[i], reading.d[i], reading.w[i])) => {
                out.idx.extend(&reading.idx); out.d.extend(&reading.d); out.w.extend(&reading.w);
                out.st.extend(&reading.st); out.px.extend(&reading.px); out.edges.extend(&reading.edges);
                out.candidate_width.extend(&reading.candidate_width); out.candidate_edges.extend(&reading.candidate_edges);
                out.ms = out.ms.saturating_add(reading.ms);
                views.push(ViewMeasurement { view, reading: Some(reading), error: None });
            }
            result => {
                let error = result.err().unwrap_or_else(|| "单图测量结果结构与测点表不一致".into());
                for j in expected {
                    out.idx.push(j as u32); out.d.push(f32::NAN); out.w.push(f32::NAN);
                    out.st.push(crate::measure::ST_INVALID); out.px.push([recipe.points.x[j], recipe.points.y[j]]);
                    out.edges.push(None); out.candidate_width.push(None); out.candidate_edges.push(None);
                }
                views.push(ViewMeasurement { view, reading: None, error: Some(error) });
            }
        }
    }
    // 有胶比例，与单图 parse_result 同口径：无基准、超出检测范围但测到胶宽的站也算有胶
    out.coverage = out.w.iter().filter(|w| w.is_finite()).count() as f32 / out.st.len().max(1) as f32;
    Ok(MultiViewMeasurement { reading: out, views })
}

/// `bases`：各视图的模板目录（glue.locate 的 base_dir），只有用特征定位的视图需要，查不到的视图给空串。
pub fn measure_views(engine: &Engine, recipe: &Recipe, k: usize, images: &[(u8, Arc<FrameImage>)], run_id: &str, bases: &BTreeMap<u8, String>) -> Result<MultiViewMeasurement, String> {
    measure_views_with(recipe, k, images, |view, image| {
        let projected = recipe.for_view(k, view)?;
        measure_shot(engine, &projected, k, image, &format!("{run_id}-v{view}"), bases.get(&view).map_or("", String::as_str))
    })
}

struct ShotPlan {
    graph: Value,
    idx: Vec<usize>,
    scale: f64,
    step: f64,
    half: f64,
    fixture: Option<FixtureSpec>,
    /// 护栏：两端切向搬移 |Δ| 的上限（mm）；没有特征定位时不用
    end_limit_mm: f64,
}

/// 测量图的像素参数：旧拍照点的示教中线与段配方的检测走廊共用同一套 lyFlow 契约约束（glue.taught_path + glue.bead_width）。
struct GraphParams {
    /// mm/px
    scale: f64,
    /// 站距（px）
    step: f64,
    /// 搜索半宽（px）
    half: f64,
    lo: f64,
    hi: f64,
    /// 中心偏移容差（px）= 搜索半宽 − 胶宽上限的一半
    tolerance: f64,
    /// 中线总长（px）
    length: f64,
}

/// 按当量、站距与检测参数换算像素参数并核对 lyFlow 的取值范围；出错时返回原因（调用方加上主语）。
fn graph_params(path: &[[f32; 2]], scale: f64, step: f64, detect: &DetectParams) -> Result<GraphParams, &'static str> {
    if !scale.is_finite() || scale <= 0.0 || scale > 10.0 { return Err("像素当量必须在 0–10 mm/px 之间"); }
    if path.iter().flatten().any(|x| !x.is_finite()) { return Err("中线坐标必须为有限数"); }
    let length: f64 = path.windows(2).map(|p| {
        (p[1][0] as f64 - p[0][0] as f64).hypot(p[1][1] as f64 - p[0][1] as f64)
    }).sum();
    if !length.is_finite() || length < 8.0 { return Err("lyFlow 示教中线至少需要 8 px"); }
    if !step.is_finite() || step < 1.0 { return Err("lyFlow 站距至少需要 1 px，请检查站距和像素当量"); }
    let half = detect.search_mm as f64 / scale;
    let lo = detect.width_range[0] as f64 / scale;
    let hi = detect.width_range[1] as f64 / scale;
    if !half.is_finite() || half < 2.0 { return Err("lyFlow 搜索半宽至少需要 2 px"); }
    if !lo.is_finite() || !hi.is_finite() || lo <= 0.0 || hi <= lo || hi >= 2.0 * half {
        return Err("胶宽范围需满足 0 < 下限 < 上限 < 两倍搜索半宽");
    }
    if !(4.0..=400.0).contains(&hi) { return Err("lyFlow 胶宽搜索上限需在 4–400 px 之间"); }
    let tolerance = half - 0.5 * hi;
    if !(0.0..=400.0).contains(&tolerance) { return Err("lyFlow 中心偏移容差（搜索半宽减去胶宽上限的一半）需在 0–400 px 之间"); }
    Ok(GraphParams { scale, step, half, lo, hi, tolerance, length })
}

/// lyFlow 沿中线按站距重采样的站数（含两端）。
fn station_count(length: f64, step: f64) -> usize {
    ((length * 1000.0).round() / 1000.0 / step + 1e-6 / step).floor() as usize + 1
}

/// 沿中线量胶的测量图：io.load_image → glue.taught_path（示教中线）→ glue.bead_width（逐站胶边与胶宽）。
fn taught_graph(path: &[[f32; 2]], detect: &DetectParams, p: &GraphParams) -> Result<Value, String> {
    Ok(json!({
        "schemaVersion": 1, "id": "01GLUESIGHTTAUGHT00000000",
        "nodes": [
            {"id": "n_load", "op": "io.load_image", "params": {"source": "inputs"}},
            {"id": "n_path", "op": "glue.taught_path", "params": {
                "points": serde_json::to_string(path).map_err(|e| e.to_string())?,
                "zone": [0.0, 0.0], "tolerance": p.tolerance, "widthMax": p.hi.ceil(),
                "polarity": if detect.polarity == Polarity::Light { "bright" } else { "dark" }, "sharpMin": 0.4
            }},
            {"id": "n_width", "op": "glue.bead_width", "params": {
                "form": "straight", "widthRange": [p.lo, p.hi], "stationStep": p.step, "searchHalf": p.half,
                "window": 30.0, "mergeGap": 8.0, "contrastMin": 16.0,
                "presentRatio": 0.5, "centerRatio": 0.3, "contrastRatio": 0.3
            }}
        ],
        "edges": [
            {"id": "e_image", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_width", "port": "image"}},
            {"id": "e_path", "from": {"node": "n_path", "port": "path"}, "to": {"node": "n_width", "port": "path"}}
        ],
        "outputs": {
            "stations": {"node": "n_width", "port": "bead.stations"},
            "beadInfo": {"node": "n_width", "port": "bead.info"}
        }
    }))
}

/// 旧拍照点模型：拍照点 k 当前视图的测量计划（示教中线逐点比对，测点对应配方全局站表）。
fn plan(recipe: &Recipe, k: usize) -> Result<ShotPlan, String> {
    let shot = recipe.shots.get(k).ok_or("拍照点不存在")?;
    let fail = |why: &str| format!("拍照点 {}：{why}", shot.id);
    if !shot.measured() { return Err(fail("设为不检，不需要图像测量")); }
    if !shot.taught() { return Err(fail("尚未示教胶路中线和像素当量")); }
    let detect = recipe.shot_detect(k);
    let params = graph_params(&shot.path, shot.mm_per_px.unwrap() as f64, (recipe.spacing / shot.mm_per_px.unwrap()) as f64, detect)
        .map_err(fail)?;
    let (scale, step, half) = (params.scale, params.step, params.half);
    let idx: Vec<_> = recipe.view_points(k, shot.view).collect();
    if idx.len() < 3 || idx.len() > 200_000 || idx.iter().any(|&j| j >= recipe.points.x.len() || j >= recipe.points.y.len()) {
        return Err(fail("配方测点表无效或不足三站"));
    }
    if station_count(params.length, step) != idx.len() {
        return Err(fail("中线末站与 lyFlow 像素重采样数量不一致，请微调中线端点或站距；不会补造或丢弃测点"));
    }
    for (i, &j) in idx.iter().enumerate() {
        let expected = shot.path_at(i as f32 * step as f32);
        let actual = [recipe.points.x[j], recipe.points.y[j]];
        if actual.iter().any(|v| !v.is_finite()) || (actual[0] - expected[0]).hypot(actual[1] - expected[1]) > 0.01 {
            return Err(fail("配方测点表与示教中线不一致"));
        }
    }
    let mut graph = taught_graph(&shot.path, detect, &params)?;
    let mut end_limit_mm = 0.0;
    if let Some(f) = &shot.fixture {
        // 恒等位姿下每端也排除示教余量 m 加 1 个站距内的站（决定一字面的“不超过”），剩不下 3 站时每帧都判不了位置
        if judged_stations(idx.len(), recipe.spacing as f64, f.end_margin_mm as f64) < MIN_JUDGED_STATIONS {
            return Err(fail(&format!("胶路太短或示教余量过大：特征定位两端各排除示教余量 {:.1} mm 与 1 个站距后，剩不下 {MIN_JUDGED_STATIONS} 站可判", f.end_margin_mm)));
        }
        let end = |j: usize| [recipe.points.x[j] as f64, recipe.points.y[j] as f64];
        end_limit_mm = f.end_shift_limit_mm([end(idx[0]), end(idx[idx.len() - 1])], scale);
        graph["nodes"].as_array_mut().unwrap().push(json!({"id": "n_locate", "op": "glue.locate", "params": {
            "template": fixture::TEMPLATE_FILE, "templatePad": FIXTURE_TEMPLATE_PAD_PX, "anchor": [f.rect[0] as f64, f.rect[1] as f64],
            "searchRadius": f.search_radius_px, "angleRange": f.angle_range_deg, "angleStep": FIXTURE_ANGLE_STEP_DEG,
            "minScore": f.min_score, "maxPeakRatio": f.max_peak_ratio
        }}));
        graph["edges"].as_array_mut().unwrap().push(json!({"id": "e_locate", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_locate", "port": "image"}}));
        graph["outputs"]["pose"] = json!({"node": "n_locate", "port": "pose"});
    }
    Ok(ShotPlan { graph, idx, scale, step, half, fixture: shot.fixture.clone(), end_limit_mm })
}

/// 运行用的测量计划：用特征定位的拍照点必须给出模板目录（绝对路径，内有 template.pgm）。
/// lyFlow 在 base_dir 为空时按进程当前目录解析相对路径，会悄悄打开别处的同名文件。
fn plan_run(recipe: &Recipe, k: usize, base_dir: &str) -> Result<ShotPlan, String> {
    let p = plan(recipe, k)?;
    if p.fixture.is_some() {
        let id = &recipe.shots[k].id;
        let dir = Path::new(base_dir);
        if base_dir.is_empty() || !dir.is_absolute() {
            return Err(format!("拍照点 {id}：使用了特征定位，但没有指定模板目录（base_dir）"));
        }
        let template = dir.join(fixture::TEMPLATE_FILE);
        if !template.is_file() {
            return Err(format!("拍照点 {id}：定位模板缺失：{}", template.display()));
        }
    }
    Ok(p)
}

pub fn build_taught_graph(recipe: &Recipe, k: usize) -> Result<Value, String> {
    Ok(plan(recipe, k)?.graph)
}

pub fn measure_shot(engine: &Engine, recipe: &Recipe, k: usize, image: &FrameImage, run_id: &str, base_dir: &str) -> Result<ShotMeasurement, String> {
    let p = plan_run(recipe, k, base_dir)?;
    run_shot(engine, recipe, k, image, run_id, base_dir, p)
}

pub fn measure_shot_with_graph(engine: &Engine, recipe: &Recipe, k: usize, image: &FrameImage, run_id: &str, base_dir: &str, graph: &Value) -> Result<ShotMeasurement, String> {
    let mut p = plan_run(recipe, k, base_dir)?;
    if graph != &p.graph { return Err("发布包算法图与配方的示教测量契约不一致".into()); }
    p.graph = graph.clone();
    run_shot(engine, recipe, k, image, run_id, base_dir, p)
}

/// 评测用：按节点覆盖算法图参数（{"n_width": {...}}）后测量，用来比较参数取值，不进生产路径。
#[cfg(test)]
pub(super) fn measure_shot_patched(engine: &Engine, recipe: &Recipe, k: usize, image: &FrameImage, run_id: &str, patch: &Value) -> Result<ShotMeasurement, String> {
    let mut p = plan(recipe, k)?;
    for node in p.graph["nodes"].as_array_mut().into_iter().flatten() {
        let Some(over) = node["id"].as_str().and_then(|id| patch.get(id)).and_then(Value::as_object) else { continue };
        for (key, value) in over { node["params"][key] = value.clone(); }
    }
    run_shot(engine, recipe, k, image, run_id, "", p)
}

fn run_shot(engine: &Engine, recipe: &Recipe, k: usize, image: &FrameImage, run_id: &str, base_dir: &str, p: ShotPlan) -> Result<ShotMeasurement, String> {
    let start = Instant::now();
    if image.width == 0 || image.height == 0 || image.pixels.len() != image.width as usize * image.height as usize {
        return Err("图像像素缓冲与尺寸不一致".into());
    }
    if recipe.shots[k].path.iter().any(|xy| xy[0] < 0.0 || xy[1] < 0.0 || xy[0] as f64 > image.width as f64 - 1.0 || xy[1] as f64 > image.height as f64 - 1.0) {
        return Err("示教中线超出当前完整图像，请确认视角和图像尺寸".into());
    }
    if p.fixture.is_some() { engine.locate.clone()?; }
    let result = engine.run(&p.graph.to_string(), run_id, base_dir, image, &json!({}))?;
    let mut measured = parse_result(recipe, &p, &result)?;
    measured.ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;
    Ok(measured)
}

fn parse_result(recipe: &Recipe, p: &ShotPlan, result: &RunResult) -> Result<ShotMeasurement, String> {
    if result.status() != "ok" { return Err(result.failure()); }
    let stations = result.record("stations").ok_or("算法缺少逐站结果")?;
    let info = result.record("beadInfo").ok_or("算法缺少逐站汇总")?;
    if stations["unit"] != "px" || info["unit"] != "px" || info["lineSource"] != "taught" || info["pathOk"] != true {
        return Err("算法输出的单位或示教胶路契约不一致".into());
    }
    let n = p.idx.len();
    if stations["count"].as_u64() != Some(n as u64) || info["stations"].as_u64() != Some(n as u64) {
        return Err("算法逐站结果数量与配方测点表不一致".into());
    }
    for key in ["s", "center", "normal", "present", "lo", "hi", "widthPx"] {
        if stations[key].as_array().map(Vec::len) != Some(n) { return Err(format!("算法逐站结果 {key} 长度与配方不一致")); }
    }
    let candidate_keys = ["candidateLo", "candidateHi", "candidateWidthPx"];
    let has_candidates = candidate_keys.iter().any(|key| stations.get(*key).is_some());
    if has_candidates && candidate_keys.iter().any(|key| stations[*key].as_array().map(Vec::len) != Some(n)) {
        return Err("算法逐站候选结果长度与配方不一致".into());
    }
    let number = |key: &str, i: usize| stations[key][i].as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("算法第 {} 站的 {key} 不是有限数", i + 1));
    let xy = |key: &str, i: usize| -> Result<[f64; 2], String> {
        let a = stations[key][i].as_array().filter(|a| a.len() == 2).ok_or_else(|| format!("算法第 {} 站的 {key} 不是二维坐标", i + 1))?;
        Ok([a[0].as_f64().filter(|v| v.is_finite()).ok_or("算法坐标不是有限数")?, a[1].as_f64().filter(|v| v.is_finite()).ok_or("算法坐标不是有限数")?])
    };
    let mut m = ShotMeasurement::empty(n);
    // 逐站的算法中线点与法向、有胶站的胶中心（px），特征定位算胶偏用
    let mut normals = Vec::with_capacity(n);
    let mut beads = Vec::with_capacity(n);
    let mut present = 0;
    for (i, &j) in p.idx.iter().enumerate() {
        let s = number("s", i)?;
        let expected_s = i as f64 * p.step;
        if (s - expected_s).abs() > 0.0015 + expected_s.abs() * f32::EPSILON as f64 * 4.0 {
            return Err(format!("算法第 {} 站弧长与配方不一致", i + 1));
        }
        let center = xy("center", i)?;
        let normal = xy("normal", i)?;
        if (normal[0].hypot(normal[1]) - 1.0).abs() > 0.002 || (center[0] - recipe.points.x[j] as f64).hypot(center[1] - recipe.points.y[j] as f64) > 2.01 {
            return Err(format!("算法第 {} 站的中线或法向与配方不一致", i + 1));
        }
        let has_bead = stations["present"][i].as_bool().ok_or("算法有胶状态不是布尔值")?;
        m.idx.push(j as u32);
        normals.push(normal);
        if has_candidates && candidate_keys.iter().any(|key| !stations[*key][i].is_null()) {
            let lo = number("candidateLo", i)?;
            let hi = number("candidateHi", i)?;
            let width = number("candidateWidthPx", i)?;
            if hi <= lo || width <= 0.0 || (width - (hi - lo)).abs() > 0.003 || lo < -p.half - 0.001 || hi > p.half + 0.001 {
                return Err(format!("算法第 {} 站候选胶边或胶宽超出搜索范围", i + 1));
            }
            m.candidate_width.push(Some((width * p.scale) as f32));
            m.candidate_edges.push(Some([
                [(center[0] + normal[0] * lo) as f32, (center[1] + normal[1] * lo) as f32],
                [(center[0] + normal[0] * hi) as f32, (center[1] + normal[1] * hi) as f32],
            ]));
        } else {
            m.candidate_width.push(None);
            m.candidate_edges.push(None);
        }
        if has_bead {
            let lo = number("lo", i)?;
            let hi = number("hi", i)?;
            let width = number("widthPx", i)?;
            if hi <= lo || width <= 0.0 || (width - (hi - lo)).abs() > 0.003 || lo < -p.half - 0.001 || hi > p.half + 0.001 {
                return Err(format!("算法第 {} 站胶边或胶宽超出搜索范围", i + 1));
            }
            let mid = (lo + hi) * 0.5;
            beads.push(Some([center[0] + normal[0] * mid, center[1] + normal[1] * mid]));
            m.d.push((mid * p.scale) as f32);
            m.w.push((width * p.scale) as f32);
            m.st.push(ST_OK);
            m.px.push([(center[0] + normal[0] * mid) as f32, (center[1] + normal[1] * mid) as f32]);
            m.edges.push(Some([
                [(center[0] + normal[0] * lo) as f32, (center[1] + normal[1] * lo) as f32],
                [(center[0] + normal[0] * hi) as f32, (center[1] + normal[1] * hi) as f32],
            ]));
            present += 1;
        } else {
            if ["lo", "hi", "widthPx"].iter().any(|key| !stations[*key][i].is_null()) {
                return Err("算法无胶站仍返回胶宽，输出契约不一致".into());
            }
            beads.push(None);
            m.d.push(f32::NAN);
            m.w.push(f32::NAN);
            m.st.push(ST_GAP);
            m.px.push([center[0] as f32, center[1] as f32]);
            m.edges.push(None);
        }
    }
    // 整条中线一站都没有胶：照常作为测量交给判定（整段断胶判 NG），不当作测量失败
    m.coverage = present as f32 / n as f32;
    if info["present"].as_u64() != Some(present as u64) || info["coverage"].as_f64().is_none_or(|x| !x.is_finite() || (x - m.coverage as f64).abs() > 0.00015) {
        return Err("算法逐站汇总与有胶状态不一致".into());
    }
    match (&p.fixture, result.record("pose")) {
        (None, None) => {}
        (Some(f), Some(record)) => apply_pose(recipe, p, f, record, &normals, &beads, &mut m)?,
        _ => return Err("算法的定位输出与配方特征定位不一致".into()),
    }
    Ok(m)
}

/// 特征定位视图在恒等位姿下至少要留下这么多站可判（两端各排除示教余量 m 与 1 个站距之后）。
pub const MIN_JUDGED_STATIONS: usize = 3;

/// n 站、站距 spacing（mm）的胶路在恒等位姿下两端各排除 m + 1 个站距（含端点上的站，与 `apply_pose` 同口径）后剩下的站数。
/// 示教余量建议（验证页）与 `plan` 都用它判断 m 是否过大。
pub fn judged_stations(n: usize, spacing: f64, margin_mm: f64) -> usize {
    let length = n.saturating_sub(1) as f64 * spacing;
    let excluded = margin_mm.max(0.0) + spacing;
    (0..n).filter(|&i| { let s = i as f64 * spacing; s > excluded + 1e-6 && length - s > excluded + 1e-6 }).count()
}

/// 按 glue.locate 的位姿把静态胶偏改成相对工件的胶偏（定稿 §2、§10）：
/// - 示教余量 m（决定一第 3 条，GlueSight 侧实现，见定稿 §13）：离 N 首、末弧长不到 m 的站不管定位成败一律超出检测范围，
///   并清掉胶宽与胶边——与 `glue.taught_path` 取 zone = [m, L − m] 时这些站根本不存在等价，站表、弧长核对与偏差投影
///   （仍用整条 N）都不用变；
/// - 不采用（定位失败，或开了转角却看不出角度）：有胶站全部无基准，不做两端排除；
/// - 采用：d 改为胶中心到 N' = T(N) 的有向法向距离（正向取 n' = R·n，与静态 d 同号），两端按切向搬移 |Δ| 排除，
///   最后 |Δ| 超过护栏上限时全部有胶站再去掉基准（先排除再护栏：排除区不进断胶区间，后沿的假断胶不会盖过 ERR 92）。
fn apply_pose(recipe: &Recipe, p: &ShotPlan, f: &FixtureSpec, record: &Value, normals: &[[f64; 2]], beads: &[Option<[f64; 2]>], m: &mut ShotMeasurement) -> Result<(), String> {
    // lyFlow 总是输出 templateSize（内框）。1.0.0 不认外圈，会把整个文件当模板，这里报成 w + 8
    let size = record["templateSize"].as_array().filter(|a| a.len() == 2).and_then(|a| Some([a[0].as_f64()?, a[1].as_f64()?]));
    if size != Some([f.rect[2] as f64, f.rect[3] as f64]) {
        return Err("定位模板与配方示教框不一致".into());
    }
    let pose = Pose::from_record(record)?;
    let observable = pose.angle_observable.ok_or("定位输出缺少 angleObservable，glue.locate 版本过旧")?;
    // angleRange = 0 时 lyFlow 恒报看得出角度；开了转角的拍照点看不出角度按定位失败处理（定稿 §10 决定二）
    let accepted = pose.ok && observable;
    let finite = |v: f64| v.is_finite().then_some(v as f32);
    let mut out = PoseRecord {
        accepted,
        reason: if !pose.ok { Some(pose.reason.clone().unwrap_or_else(|| "unknown".into())) } else if !observable { Some("angle_unobservable".into()) } else { None },
        score: finite(pose.score),
        peak_ratio: pose.peak_ratio.and_then(finite),
        angle: finite(pose.angle),
        offset: finite(pose.center[0] - pose.teach_center[0]).zip(finite(pose.center[1] - pose.teach_center[1])).map(|(x, y)| [x, y]),
        end_shift_mm: None,
        excluded_mm: None,
        end_limit_mm: p.end_limit_mm as f32,
    };
    let n = p.idx.len();
    let spacing = recipe.spacing as f64;
    let length = (n - 1) as f64 * spacing;
    let margin = f.end_margin_mm as f64;
    for i in 0..n {
        let s = i as f64 * spacing;
        if margin > 0.0 && (s < margin - 1e-6 || length - s < margin - 1e-6) {
            m.st[i] = ST_OUT_OF_RANGE;
            m.d[i] = f32::NAN;
            m.w[i] = f32::NAN;
            m.edges[i] = None;
        }
    }
    if !accepted {
        for i in 0..n { drop_reference(&mut m.st[i], &mut m.d[i]); }
        m.reference_failed = Some(REFERENCE_LOCATE_FAILED.into());
        m.pose = Some(out);
        return Ok(());
    }
    // 示教线 N 用配方测点（与评测一致），法向用算法法向（与静态 d 同号）
    let nominal: Vec<[f64; 2]> = p.idx.iter().map(|&j| [recipe.points.x[j] as f64, recipe.points.y[j] as f64]).collect();
    let deviations = fixture::deviations(&nominal, normals, beads, &pose)?;
    let unit = |a: [f64; 2], b: [f64; 2]| { let (x, y) = (b[0] - a[0], b[1] - a[1]); let l = x.hypot(y); [x / l, y / l] };
    let shift = |q: [f64; 2], t: [f64; 2]| { let moved = pose.apply(q); ((moved[0] - q[0]) * t[0] + (moved[1] - q[1]) * t[1]) * p.scale };
    let delta = [shift(nominal[0], unit(nominal[0], nominal[1])), shift(nominal[n - 1], unit(nominal[n - 2], nominal[n - 1]))];
    // 从 N 端量起：示教余量 m 与搬移 |Δ| 取大者，再加 1 个站距（= 从 zone 起点量的 max(0, |Δ| − m) + 1 个站距）
    let excluded = delta.map(|d| d.abs().max(margin) + spacing);
    for i in 0..n {
        let s = i as f64 * spacing;
        if s <= excluded[0] + 1e-6 || length - s <= excluded[1] + 1e-6 {
            // 超出检测范围：不判胶偏、不判断胶；有胶站的胶宽与胶边留着（余量段上面已清掉）
            m.st[i] = ST_OUT_OF_RANGE;
            m.d[i] = f32::NAN;
        } else if m.st[i] == ST_OK {
            match deviations[i] {
                Some(v) => m.d[i] = (v * p.scale) as f32,
                None => drop_reference(&mut m.st[i], &mut m.d[i]),
            }
        }
    }
    if delta[0].abs().max(delta[1].abs()) > p.end_limit_mm {
        for i in 0..n { drop_reference(&mut m.st[i], &mut m.d[i]); }
        m.reference_failed = Some(REFERENCE_END_SHIFT.into());
    }
    out.end_shift_mm = Some(delta.map(|d| d as f32));
    out.excluded_mm = Some(excluded.map(|e| e as f32));
    m.pose = Some(out);
    Ok(())
}

// ───────────── 段配方：每台相机一条检测走廊，每帧独立测（D-SB-12、D-SB-17） ─────────────

/// 胶条从走廊端点一侧走出的判据：紧挨端部缺胶段的有胶站，胶中心偏离走廊超过中心偏移容差的这一比例。
/// 默认做法 4 的实现，判据没有经过真实弯道图像验证。
const EXIT_OFFSET_RATIO: f64 = 0.5;

/// 段配方一段、一台检测相机的走廊测量图（D-SB-45）：沿走廊按站距取测站。测站不进配方全局站表，
/// 每帧只输出帧内汇总的 `FrameMetrics`（规格 §4 每帧独立指标）。
#[derive(Clone, Debug)]
pub struct CorridorGraph {
    pub camera: String,
    /// 计划相机下标（`Recipe.inspect` 的顺序），即 `FrameMetrics.camera`
    pub camera_idx: usize,
    /// 走廊所属的段（`PathSegment.index`），即 `FrameMetrics.corridor`
    pub segment: u32,
    /// 示教帧尺寸；每帧必须与它相同
    pub size: [u32; 2],
    pub graph: Value,
    graph_text: String,
    /// 各测站在图里的位置（px），沿走廊弧长 i × step
    stations: Vec<[f64; 2]>,
    scale: f64,
    step: f64,
    half: f64,
    tolerance: f64,
    spacing_mm: f64,
}

/// 一台相机一帧的测量：帧内指标与逐站胶边（px，无胶站为 None，叠图用）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameMeasurement {
    pub metrics: FrameMetrics,
    pub edges: Vec<Option<[[f32; 2]; 2]>>,
    pub ms: u32,
}

/// 走廊各点都在 size 的图像内（像素中心坐标 0..=宽−1、0..=高−1）。
pub fn corridor_inside(corridor: &[[f32; 2]], size: [u32; 2]) -> bool {
    size[0] > 0 && size[1] > 0 && corridor.iter().all(|p| {
        p[0] >= 0.0 && p[1] >= 0.0 && p[0] as f64 <= size[0] as f64 - 1.0 && p[1] as f64 <= size[1] as f64 - 1.0
    })
}

/// 折线上弧长 t（px）处的点。
fn polyline_at(path: &[[f32; 2]], t: f64) -> [f64; 2] {
    let mut left = t.max(0.0);
    for w in path.windows(2) {
        let (a, b) = ([w[0][0] as f64, w[0][1] as f64], [w[1][0] as f64, w[1][1] as f64]);
        let l = (b[0] - a[0]).hypot(b[1] - a[1]);
        if left <= l && l > 0.0 {
            let f = left / l;
            return [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
        }
        left -= l;
    }
    path.last().map_or([0.0, 0.0], |p| [p[0] as f64, p[1] as f64])
}

/// 段配方第 `segment` 段、一台检测相机的走廊测量图：示教帧尺寸为 size。走廊必须完整落在图内（不然 lyFlow 量不到走廊两端）；
/// 当量与检测参数按相机，站距与取值范围与旧拍照点中线相同；至少三站。
pub fn plan_corridor(
    inspect: &CameraInspect,
    segment: u32,
    recipe: &Recipe,
    size: [u32; 2],
) -> Result<CorridorGraph, String> {
    let camera_idx = recipe
        .inspect
        .iter()
        .position(|i| i.camera == inspect.camera)
        .ok_or_else(|| format!("相机 {} 不是配方 {} 的检测相机", inspect.camera, recipe.id))?;
    let fail = |why: &str| format!("第 {segment} 段相机 {}：{why}", inspect.camera);
    let corridor = inspect
        .segment_corridor(segment)
        .map(|s| s.corridor.as_slice())
        .unwrap_or_default();
    if corridor.len() < 2 {
        return Err(fail("尚未示教检测走廊"));
    }
    let Some(scale) = inspect.mm_per_px else {
        return Err(fail("缺少像素当量"));
    };
    let detect = inspect.detect.as_ref().unwrap_or(&recipe.detect);
    let params =
        graph_params(corridor, scale, recipe.spacing as f64 / scale, detect).map_err(fail)?;
    if !corridor_inside(corridor, size) {
        return Err(fail(&format!(
            "检测走廊超出示教帧 {}×{}，请在这一段的示教帧上重新示教走廊",
            size[0], size[1]
        )));
    }
    let n = station_count(params.length, params.step);
    if !(3..=200_000).contains(&n) {
        return Err(fail("检测走廊不足三站或超过 20 万站，请调整走廊长度或站距"));
    }
    let stations = (0..n)
        .map(|i| polyline_at(corridor, i as f64 * params.step))
        .collect();
    let graph = taught_graph(corridor, detect, &params)?;
    Ok(CorridorGraph {
        camera: inspect.camera.clone(),
        camera_idx,
        segment,
        size,
        graph_text: graph.to_string(),
        graph,
        stations,
        scale,
        step: params.step,
        half: params.half,
        tolerance: params.tolerance,
        spacing_mm: recipe.spacing as f64,
    })
}

/// 段配方全部“段 × 检测相机”的走廊测量图（D-SB-45），按（计划相机下标, 段号）取。`sizes` 为各计划相机的示教帧尺寸
/// （计划相机顺序，一台相机的各段示教帧同尺寸）。没示教的组合不建图，取用时返回 None。
#[derive(Clone, Debug, Default)]
pub struct CorridorSet {
    graphs: BTreeMap<(usize, u32), CorridorGraph>,
}

impl CorridorSet {
    pub fn build(recipe: &Recipe, sizes: &[[u32; 2]]) -> Result<Self, String> {
        let cameras = recipe.plan_cameras();
        let mut graphs = BTreeMap::new();
        for segment in &recipe.path_segments {
            for camera in crate::plan::segment_cameras(segment, &cameras) {
                let Some((idx, inspect)) = recipe
                    .inspect
                    .iter()
                    .enumerate()
                    .find(|(_, i)| i.camera == camera)
                else {
                    continue;
                };
                if !inspect.segment_taught(segment.index) {
                    continue;
                }
                let size = *sizes
                    .get(idx)
                    .ok_or_else(|| format!("相机 {camera} 没有示教帧尺寸"))?;
                graphs.insert(
                    (idx, segment.index),
                    plan_corridor(inspect, segment.index, recipe, size)?,
                );
            }
        }
        Ok(Self { graphs })
    }

    pub fn get(&self, camera: usize, segment: u32) -> Option<&CorridorGraph> {
        self.graphs.get(&(camera, segment))
    }

    pub fn iter(&self) -> impl Iterator<Item = &CorridorGraph> {
        self.graphs.values()
    }

    pub fn is_empty(&self) -> bool {
        self.graphs.is_empty()
    }
}

/// 用 lyFlow 量一帧（段配方）：见 [`measure_frame_with`]。
pub fn measure_frame(engine: &Engine, corridor: &CorridorGraph, image: &FrameImage, seq: u64, run_id: &str) -> Result<FrameMeasurement, String> {
    let start = Instant::now();
    let mut m = measure_frame_with(corridor, image, seq, |graph, image| engine.run(graph, run_id, "", image, &json!({})))?;
    m.ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;
    Ok(m)
}

/// 量一帧：`run` 用走廊测量图（JSON 文本）跑这张图。图像尺寸与示教帧不同、算法失败或输出不合契约时返回 Err，
/// 只影响这一帧（调用方记为 Error 帧，规格 §0）；胶条从走廊一侧走出时这一帧为 OutOfCorridor（默认做法 4），都不 panic。
pub fn measure_frame_with(
    corridor: &CorridorGraph, image: &FrameImage, seq: u64,
    run: impl FnOnce(&str, &FrameImage) -> Result<RunResult, String>,
) -> Result<FrameMeasurement, String> {
    if [image.width, image.height] != corridor.size {
        return Err(format!("相机 {} 这一帧图像 {}×{} 与示教帧 {}×{} 尺寸不一致", corridor.camera, image.width, image.height, corridor.size[0], corridor.size[1]));
    }
    if image.pixels.len() != image.width as usize * image.height as usize {
        return Err("图像像素缓冲与尺寸不一致".into());
    }
    let result = run(&corridor.graph_text, image)?;
    parse_corridor(corridor, &result, seq)
}

/// 逐站输出 → 帧内指标。契约核对与旧拍照点 `parse_result` 相同（单位、示教胶路、站数、弧长、中线与法向、胶边与胶宽、汇总），
/// 不读候选胶边，也没有定位输出。
fn parse_corridor(c: &CorridorGraph, result: &RunResult, seq: u64) -> Result<FrameMeasurement, String> {
    if result.status() != "ok" { return Err(result.failure()); }
    let stations = result.record("stations").ok_or("算法缺少逐站结果")?;
    let info = result.record("beadInfo").ok_or("算法缺少逐站汇总")?;
    if stations["unit"] != "px" || info["unit"] != "px" || info["lineSource"] != "taught" || info["pathOk"] != true {
        return Err("算法输出的单位或示教胶路契约不一致".into());
    }
    let n = c.stations.len();
    if stations["count"].as_u64() != Some(n as u64) || info["stations"].as_u64() != Some(n as u64) {
        return Err("算法逐站结果数量与走廊测站数不一致".into());
    }
    for key in ["s", "center", "normal", "present", "lo", "hi", "widthPx"] {
        if stations[key].as_array().map(Vec::len) != Some(n) { return Err(format!("算法逐站结果 {key} 长度与走廊测站数不一致")); }
    }
    let number = |key: &str, i: usize| stations[key][i].as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("算法第 {} 站的 {key} 不是有限数", i + 1));
    let xy = |key: &str, i: usize| -> Result<[f64; 2], String> {
        let a = stations[key][i].as_array().filter(|a| a.len() == 2).ok_or_else(|| format!("算法第 {} 站的 {key} 不是二维坐标", i + 1))?;
        Ok([a[0].as_f64().filter(|v| v.is_finite()).ok_or("算法坐标不是有限数")?, a[1].as_f64().filter(|v| v.is_finite()).ok_or("算法坐标不是有限数")?])
    };
    // 有胶站的胶宽（px）与胶中心相对走廊的法向偏移（px，沿算法法向为正）
    let mut beads: Vec<Option<(f64, f64)>> = Vec::with_capacity(n);
    let mut edges = Vec::with_capacity(n);
    for i in 0..n {
        let s = number("s", i)?;
        let expected_s = i as f64 * c.step;
        if (s - expected_s).abs() > 0.0015 + expected_s.abs() * f32::EPSILON as f64 * 4.0 {
            return Err(format!("算法第 {} 站弧长与走廊不一致", i + 1));
        }
        let center = xy("center", i)?;
        let normal = xy("normal", i)?;
        let station = c.stations[i];
        if (normal[0].hypot(normal[1]) - 1.0).abs() > 0.002 || (center[0] - station[0]).hypot(center[1] - station[1]) > 2.01 {
            return Err(format!("算法第 {} 站的中线或法向与走廊不一致", i + 1));
        }
        if stations["present"][i].as_bool().ok_or("算法有胶状态不是布尔值")? {
            let lo = number("lo", i)?;
            let hi = number("hi", i)?;
            let width = number("widthPx", i)?;
            if hi <= lo || width <= 0.0 || (width - (hi - lo)).abs() > 0.003 || lo < -c.half - 0.001 || hi > c.half + 0.001 {
                return Err(format!("算法第 {} 站胶边或胶宽超出搜索范围", i + 1));
            }
            beads.push(Some((width, (lo + hi) * 0.5)));
            edges.push(Some([
                [(center[0] + normal[0] * lo) as f32, (center[1] + normal[1] * lo) as f32],
                [(center[0] + normal[0] * hi) as f32, (center[1] + normal[1] * hi) as f32],
            ]));
        } else {
            if ["lo", "hi", "widthPx"].iter().any(|key| !stations[*key][i].is_null()) {
                return Err("算法无胶站仍返回胶宽，输出契约不一致".into());
            }
            beads.push(None);
            edges.push(None);
        }
    }
    let present = beads.iter().flatten().count();
    let coverage = present as f64 / n as f64;
    if info["present"].as_u64() != Some(present as u64) || info["coverage"].as_f64().is_none_or(|x| !x.is_finite() || (x - coverage).abs() > 0.00015) {
        return Err("算法逐站汇总与有胶状态不一致".into());
    }
    let mut metrics = frame_metrics(c, seq, &beads);
    if let Some(why) = corridor_exit(&beads, c.tolerance, c.scale) {
        metrics.state = FrameState::OutOfCorridor;
        metrics.error = Some(why);
    }
    Ok(FrameMeasurement { metrics, edges, ms: 0 })
}

/// 帧内汇总（规格 §4）：逐站胶宽（mm）、胶宽极值与均值、最长连续缺胶（缺胶站数 × 站距，与旧模型断胶长度同口径）、
/// 有胶站占比、胶中心相对走廊的法向偏移均值 x（AbsoluteX，直线段胶偏用）。一站胶都没有时胶宽与 x 为空，整条走廊算缺胶。
fn frame_metrics(c: &CorridorGraph, seq: u64, beads: &[Option<(f64, f64)>]) -> FrameMetrics {
    let widths: Vec<Option<f64>> = beads.iter().map(|b| b.map(|(w, _)| w * c.scale)).collect();
    let present: Vec<f64> = widths.iter().flatten().copied().collect();
    let offsets: Vec<f64> = beads.iter().flatten().map(|&(_, x)| x * c.scale).collect();
    let mean = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    let longest_gap = beads.split(Option::is_some).map(<[_]>::len).max().unwrap_or(0);
    FrameMetrics {
        camera: c.camera_idx,
        seq,
        state: FrameState::Measured,
        error: None,
        width_min: present.iter().copied().reduce(f64::min),
        width_max: present.iter().copied().reduce(f64::max),
        width_mean: mean(&present),
        widths: widths.iter().map(|w| w.map(|w| w as f32)).collect(),
        spacing_mm: c.spacing_mm,
        gap_max_mm: longest_gap as f64 * c.spacing_mm,
        corridor: Some(c.segment),
        present_ratio: present.len() as f64 / beads.len().max(1) as f64,
        x_mm: mean(&offsets),
    }
}

/// 胶条从走廊端点一侧走出（默认做法 4：弯道帧常见，记检测不完整、不判 NG）：缺胶只在连到走廊端点的一段或两段上，
/// 且紧挨每段缺胶的有胶站胶中心都已偏离走廊超过中心偏移容差的 `EXIT_OFFSET_RATIO`。整帧没有胶、有胶段中间有缺胶、
/// 或紧挨端部缺胶的胶中心仍在走廊中部时不算走出（照常按缺胶判，免得把真断胶、停胶当成检测不完整放过）。返回原因。
fn corridor_exit(beads: &[Option<(f64, f64)>], tolerance_px: f64, scale: f64) -> Option<String> {
    let first = beads.iter().position(Option::is_some)?;
    let last = beads.iter().rposition(Option::is_some)?;
    if beads[first..=last].iter().any(Option::is_none) {
        return None;
    }
    let exits = |i: usize| beads[i].is_some_and(|(_, x)| x.abs() >= EXIT_OFFSET_RATIO * tolerance_px);
    let ends: Vec<(&str, usize)> = [(first > 0).then_some(("起点", first)), (last + 1 < beads.len()).then_some(("终点", last))]
        .into_iter().flatten().collect();
    if ends.is_empty() || !ends.iter().all(|&(_, i)| exits(i)) {
        return None;
    }
    let detail: Vec<String> = ends.iter().map(|&(end, i)| {
        format!("{end}一侧第 {} 站胶中心偏离走廊 {:.2} mm", i + 1, beads[i].map_or(0.0, |(_, x)| x * scale))
    }).collect();
    Some(format!("胶条从走廊走出：{}，之外无胶", detail.join("；")))
}

#[cfg(test)]
#[path = "taught/tests.rs"]
mod tests;
