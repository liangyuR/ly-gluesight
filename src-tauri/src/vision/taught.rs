use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::fixture::{self, Pose};
use super::{Engine, FrameImage, RunResult};
use crate::measure::{drop_reference, Job, Measured, ReferenceFailure, REFERENCE_END_SHIFT, REFERENCE_LOCATE_FAILED, ST_GAP, ST_NO_REF, ST_OK, ST_OUT_OF_RANGE};
use crate::recipe::{FixtureSpec, Polarity, Recipe, FIXTURE_ANGLE_STEP_DEG, FIXTURE_END_MARGIN_MM, FIXTURE_TEMPLATE_PAD_PX};

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
    /// 两端各排除的长度（mm）：max(0, |Δ| − m) + 1 个站距
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

fn plan(recipe: &Recipe, k: usize) -> Result<ShotPlan, String> {
    let shot = recipe.shots.get(k).ok_or("拍照点不存在")?;
    let fail = |why: &str| format!("拍照点 {}：{why}", shot.id);
    if !shot.measured() { return Err(fail("设为不检，不需要图像测量")); }
    if !shot.taught() { return Err(fail("尚未示教胶路中线和像素当量")); }
    let scale = shot.mm_per_px.unwrap() as f64;
    if !scale.is_finite() || scale <= 0.0 || scale > 10.0 { return Err(fail("像素当量必须在 0–10 mm/px 之间")); }
    if shot.path.iter().flatten().any(|x| !x.is_finite()) { return Err(fail("中线坐标必须为有限数")); }
    let length: f64 = shot.path.windows(2).map(|p| {
        (p[1][0] as f64 - p[0][0] as f64).hypot(p[1][1] as f64 - p[0][1] as f64)
    }).sum();
    if !length.is_finite() || length < 8.0 { return Err(fail("lyFlow 示教中线至少需要 8 px")); }
    let step = (recipe.spacing / shot.mm_per_px.unwrap()) as f64;
    if !step.is_finite() || step < 1.0 { return Err(fail("lyFlow 站距至少需要 1 px，请检查站距和像素当量")); }
    let detect = recipe.shot_detect(k);
    let half = detect.search_mm as f64 / scale;
    let lo = detect.width_range[0] as f64 / scale;
    let hi = detect.width_range[1] as f64 / scale;
    if !half.is_finite() || half < 2.0 { return Err(fail("lyFlow 搜索半宽至少需要 2 px")); }
    if !lo.is_finite() || !hi.is_finite() || lo <= 0.0 || hi <= lo || hi >= 2.0 * half {
        return Err(fail("胶宽范围需满足 0 < 下限 < 上限 < 两倍搜索半宽"));
    }
    if !(4.0..=400.0).contains(&hi) { return Err(fail("lyFlow 胶宽搜索上限需在 4–400 px 之间")); }
    let tolerance = half - 0.5 * hi;
    if !(0.0..=400.0).contains(&tolerance) { return Err(fail("lyFlow 中心偏移容差（搜索半宽减去胶宽上限的一半）需在 0–400 px 之间")); }
    let idx: Vec<_> = recipe.view_points(k, shot.view).collect();
    if idx.len() < 3 || idx.len() > 200_000 || idx.iter().any(|&j| j >= recipe.points.x.len() || j >= recipe.points.y.len()) {
        return Err(fail("配方测点表无效或不足三站"));
    }
    let native_count = ((length * 1000.0).round() / 1000.0 / step + 1e-6 / step).floor() as usize + 1;
    if native_count != idx.len() {
        return Err(fail("中线末站与 lyFlow 像素重采样数量不一致，请微调中线端点或站距；不会补造或丢弃测点"));
    }
    for (i, &j) in idx.iter().enumerate() {
        let expected = shot.path_at(i as f32 * step as f32);
        let actual = [recipe.points.x[j], recipe.points.y[j]];
        if actual.iter().any(|v| !v.is_finite()) || (actual[0] - expected[0]).hypot(actual[1] - expected[1]) > 0.01 {
            return Err(fail("配方测点表与示教中线不一致"));
        }
    }
    let mut graph = json!({
        "schemaVersion": 1, "id": "01GLUESIGHTTAUGHT00000000",
        "nodes": [
            {"id": "n_load", "op": "io.load_image", "params": {"source": "inputs"}},
            {"id": "n_path", "op": "glue.taught_path", "params": {
                "points": serde_json::to_string(&shot.path).map_err(|e| e.to_string())?,
                "zone": [0.0, 0.0], "tolerance": tolerance, "widthMax": hi.ceil(),
                "polarity": if detect.polarity == Polarity::Light { "bright" } else { "dark" }, "sharpMin": 0.4
            }},
            {"id": "n_width", "op": "glue.bead_width", "params": {
                "form": "straight", "widthRange": [lo, hi], "stationStep": step, "searchHalf": half,
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
    });
    let mut end_limit_mm = 0.0;
    if let Some(f) = &shot.fixture {
        // 恒等位姿下每端也排除 1 个站距内的站（决定一字面的“不超过”，两站），3–4 站的视图会整段超出检测范围
        if idx.len() < 5 { return Err(fail("胶路太短：特征定位每端至少排除 2 站，剩不下可判的站")); }
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

/// 按 glue.locate 的位姿把静态胶偏改成相对工件的胶偏（定稿 §2、§10）：
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
    let spacing = recipe.spacing as f64;
    let excluded = delta.map(|d| (d.abs() - FIXTURE_END_MARGIN_MM as f64).max(0.0) + spacing);
    let length = (n - 1) as f64 * spacing;
    for i in 0..n {
        let s = i as f64 * spacing;
        if s <= excluded[0] + 1e-6 || length - s <= excluded[1] + 1e-6 {
            // 超出检测范围：不判胶偏、不判断胶；有胶站的胶宽与胶边留着
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

#[cfg(test)]
#[path = "taught/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "taught/engine_tests.rs"]
mod engine_tests;
