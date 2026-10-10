use std::time::Instant;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{json, Value};

use super::{Engine, FrameImage, RunResult};
use crate::measure::{Job, Measured, ST_GAP, ST_OK};
use crate::recipe::{Polarity, Recipe};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotMeasurement {
    pub idx: Vec<u32>,
    pub d: Vec<f32>,
    pub w: Vec<f32>,
    pub st: Vec<u8>,
    pub px: Vec<[f32; 2]>,
    pub coverage: f32,
    pub ms: u32,
}

impl ShotMeasurement {
    pub fn into_measured(self, job: &Job) -> Measured {
        let mut m = Measured::empty(job);
        m.located = true;
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
        let status: Vec<_> = self.st.iter().map(|s| if *s == ST_OK { "ok" } else { "no_bead" }).collect();
        json!({"unit": "mm", "ids": self.idx, "status": status, "innerCenter": self.d,
            "width": self.w, "px": self.px, "coverage": self.coverage, "ms": self.ms})
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
    pub fn into_measured(self, job: &Job) -> Measured {
        let mut measured = self.reading.into_measured(job);
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
    let mut out = ShotMeasurement { idx: Vec::new(), d: Vec::new(), w: Vec::new(), st: Vec::new(), px: Vec::new(), coverage: 0.0, ms: 0 };
    let mut views = Vec::new();
    for view in selected {
        let image = &images.iter().find(|(v, _)| *v == view).unwrap().1;
        let expected: Vec<_> = recipe.view_points(k, view).collect();
        match measure(view, image) {
            Ok(reading) if reading.idx.iter().copied().map(|v| v as usize).eq(expected.iter().copied())
                && reading.st.len() == expected.len() && reading.d.len() == expected.len()
                && reading.w.len() == expected.len() && reading.px.len() == expected.len()
                && reading.coverage.is_finite() && (0.0..=1.0).contains(&reading.coverage)
                && reading.px.iter().flatten().all(|v| v.is_finite())
                && reading.st.iter().enumerate().all(|(i, &state)| state <= crate::measure::ST_INVALID
                    && (state != ST_OK || (reading.d[i].is_finite() && reading.w[i].is_finite()))) => {
                out.idx.extend(&reading.idx); out.d.extend(&reading.d); out.w.extend(&reading.w);
                out.st.extend(&reading.st); out.px.extend(&reading.px);
                out.ms = out.ms.saturating_add(reading.ms);
                views.push(ViewMeasurement { view, reading: Some(reading), error: None });
            }
            result => {
                let error = result.err().unwrap_or_else(|| "单图测量结果结构与测点表不一致".into());
                for j in expected {
                    out.idx.push(j as u32); out.d.push(f32::NAN); out.w.push(f32::NAN);
                    out.st.push(crate::measure::ST_INVALID); out.px.push([recipe.points.x[j], recipe.points.y[j]]);
                }
                views.push(ViewMeasurement { view, reading: None, error: Some(error) });
            }
        }
    }
    out.coverage = out.st.iter().filter(|&&s| s == ST_OK).count() as f32 / out.st.len().max(1) as f32;
    Ok(MultiViewMeasurement { reading: out, views })
}

pub fn measure_views(engine: &Engine, recipe: &Recipe, k: usize, images: &[(u8, Arc<FrameImage>)], run_id: &str, base_dir: &str) -> Result<MultiViewMeasurement, String> {
    measure_views_with(recipe, k, images, |view, image| {
        let projected = recipe.for_view(k, view)?;
        measure_shot(engine, &projected, k, image, &format!("{run_id}-v{view}"), base_dir)
    })
}

struct ShotPlan {
    graph: Value,
    idx: Vec<usize>,
    scale: f64,
    step: f64,
    half: f64,
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
    let graph = json!({
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
    Ok(ShotPlan { graph, idx, scale, step, half })
}

pub fn build_taught_graph(recipe: &Recipe, k: usize) -> Result<Value, String> {
    Ok(plan(recipe, k)?.graph)
}

pub fn measure_shot(engine: &Engine, recipe: &Recipe, k: usize, image: &FrameImage, run_id: &str, base_dir: &str) -> Result<ShotMeasurement, String> {
    let p = plan(recipe, k)?;
    run_shot(engine, recipe, k, image, run_id, base_dir, p)
}

pub fn measure_shot_with_graph(engine: &Engine, recipe: &Recipe, k: usize, image: &FrameImage, run_id: &str, base_dir: &str, graph: &Value) -> Result<ShotMeasurement, String> {
    let mut p = plan(recipe, k)?;
    if graph != &p.graph { return Err("发布包算法图与配方的示教测量契约不一致".into()); }
    p.graph = graph.clone();
    run_shot(engine, recipe, k, image, run_id, base_dir, p)
}

fn run_shot(engine: &Engine, recipe: &Recipe, k: usize, image: &FrameImage, run_id: &str, base_dir: &str, p: ShotPlan) -> Result<ShotMeasurement, String> {
    let start = Instant::now();
    if image.width == 0 || image.height == 0 || image.pixels.len() != image.width as usize * image.height as usize {
        return Err("图像像素缓冲与尺寸不一致".into());
    }
    if recipe.shots[k].path.iter().any(|xy| xy[0] < 0.0 || xy[1] < 0.0 || xy[0] as f64 > image.width as f64 - 1.0 || xy[1] as f64 > image.height as f64 - 1.0) {
        return Err("示教中线超出当前完整图像，请确认视角和图像尺寸".into());
    }
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
    let number = |key: &str, i: usize| stations[key][i].as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("算法第 {} 站的 {key} 不是有限数", i + 1));
    let xy = |key: &str, i: usize| -> Result<[f64; 2], String> {
        let a = stations[key][i].as_array().filter(|a| a.len() == 2).ok_or_else(|| format!("算法第 {} 站的 {key} 不是二维坐标", i + 1))?;
        Ok([a[0].as_f64().filter(|v| v.is_finite()).ok_or("算法坐标不是有限数")?, a[1].as_f64().filter(|v| v.is_finite()).ok_or("算法坐标不是有限数")?])
    };
    let mut m = ShotMeasurement { idx: Vec::with_capacity(n), d: Vec::with_capacity(n), w: Vec::with_capacity(n), st: Vec::with_capacity(n), px: Vec::with_capacity(n), coverage: 0.0, ms: 0 };
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
        if has_bead {
            let lo = number("lo", i)?;
            let hi = number("hi", i)?;
            let width = number("widthPx", i)?;
            if hi <= lo || width <= 0.0 || (width - (hi - lo)).abs() > 0.003 || lo < -p.half - 0.001 || hi > p.half + 0.001 {
                return Err(format!("算法第 {} 站胶边或胶宽超出搜索范围", i + 1));
            }
            let mid = (lo + hi) * 0.5;
            m.d.push((mid * p.scale) as f32);
            m.w.push((width * p.scale) as f32);
            m.st.push(ST_OK);
            m.px.push([(center[0] + normal[0] * mid) as f32, (center[1] + normal[1] * mid) as f32]);
            present += 1;
        } else {
            if ["lo", "hi", "widthPx"].iter().any(|key| !stations[*key][i].is_null()) {
                return Err("算法无胶站仍返回胶宽，输出契约不一致".into());
            }
            m.d.push(f32::NAN);
            m.w.push(f32::NAN);
            m.st.push(ST_GAP);
            m.px.push([center[0] as f32, center[1] as f32]);
        }
    }
    // 整条中线一站都没有胶：照常作为测量交给判定（整段断胶判 NG），不当作测量失败
    m.coverage = present as f32 / n as f32;
    if info["present"].as_u64() != Some(present as u64) || info["coverage"].as_f64().is_none_or(|x| !x.is_finite() || (x - m.coverage as f64).abs() > 0.00015) {
        return Err("算法逐站汇总与有胶状态不一致".into());
    }
    Ok(m)
}

#[cfg(test)]
#[path = "taught/tests.rs"]
mod tests;
