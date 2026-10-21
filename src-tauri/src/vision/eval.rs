//! 算法评测（默认忽略）：实拍样本集 + 合成扰动 / 合成 NG，走与生产相同的示教提取、测量与判定链路。
//! 入口 scripts/Test-AlgorithmEval.ps1；结果写到 GLUESIGHT_EVAL_OUT（缺省 output/algorithm-eval）。

mod locate;
pub(super) mod synth;
mod teach;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use serde::Deserialize;
use serde_json::{json, Value};

use super::{measure_shot, Engine, FrameImage, ShotMeasurement};
use crate::judge::{judge, PointState, Verdict};
use crate::measure::{ST_GAP, ST_OK};
use crate::recipe::{default_limits, DetectParams, Polarity, Recipe, RecipeDoc, ShotLimits, ShotSpec, TriggerMode, RECIPE_SCHEMA};
use crate::workspace::centerline::{extract, BrushGuide, ExtractionError};
use synth::{BeadFrame, Defect, Perturb};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    sample_set_id: String,
    part_id: String,
    nominal_width_mm: f32,
    brush_radius_px: f32,
    point_spacing_px: f32,
    cameras: BTreeMap<String, CameraFixture>,
    shots: Vec<ShotFixture>,
    /// 现场反馈过的提取失败笔划，评测时逐条复核
    #[serde(default)]
    extract_cases: Vec<ExtractCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtractCase {
    shot_id: String,
    stroke: Vec<[f32; 2]>,
    radius_px: f32,
    note: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CameraFixture {
    mm_per_px: Option<f32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShotFixture {
    shot_id: String,
    stroke: Vec<[f32; 2]>,
    /// 提取失败时按笔划直接生成中线（对应示教页"笔划直接生成中线"）
    #[serde(default)]
    fallback_stroke: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetDoc {
    id: String,
    shots: Vec<SetShot>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetShot {
    shot_id: String,
    camera_id: String,
}

struct Sample {
    shot_id: String,
    camera: String,
    image: FrameImage,
}

struct Config {
    set_dir: PathBuf,
    core: PathBuf,
    fixture: PathBuf,
    out: PathBuf,
    stages: Vec<String>,
    detect: Option<DetectParams>,
    workers: usize,
    repeats: usize,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        Ok(Self {
            set_dir: var("GLUESIGHT_SAMPLE_SET").map(PathBuf::from).ok_or("缺少 GLUESIGHT_SAMPLE_SET")?,
            core: var("LYFLOW_CORE_DLL").map(PathBuf::from).ok_or("缺少 LYFLOW_CORE_DLL")?,
            fixture: var("GLUESIGHT_EVAL_FIXTURE").map(PathBuf::from).unwrap_or_else(|| root.join("tests/algorithm/gs-20261017-normal.json")),
            out: var("GLUESIGHT_EVAL_OUT").map(PathBuf::from).unwrap_or_else(|| root.join("output/algorithm-eval")),
            stages: var("GLUESIGHT_EVAL_STAGES").unwrap_or_else(|| "baseline,repeat,perturb,defect,timing,edge,extract".into())
                .split(',').map(|s| s.trim().to_string()).collect(),
            detect: var("GLUESIGHT_EVAL_DETECT").map(|s| serde_json::from_str(&s).map_err(|e| format!("GLUESIGHT_EVAL_DETECT 无效：{e}"))).transpose()?,
            workers: var("GLUESIGHT_EVAL_WORKERS").and_then(|v| v.parse().ok()).unwrap_or(8).max(1),
            repeats: var("GLUESIGHT_EVAL_REPEATS").and_then(|v| v.parse().ok()).unwrap_or(5),
        })
    }

    fn has(&self, stage: &str) -> bool {
        self.stages.iter().any(|s| s == stage || s == "all")
    }
}

fn par_map<T: Sync, R: Send>(items: &[T], workers: usize, f: impl Fn(usize, &T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..workers.min(items.len()).max(1) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= items.len() { break; }
                let r = f(i, &items[i]);
                out.lock().unwrap()[i] = Some(r);
            });
        }
    });
    out.into_inner().unwrap().into_iter().map(Option::unwrap).collect()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.retain(|x| x.is_finite());
    if v.is_empty() { return f64::NAN; }
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    if v.len() % 2 == 1 { v[m] } else { 0.5 * (v[m - 1] + v[m]) }
}

fn pct(mut v: Vec<f64>, q: f64) -> f64 {
    v.retain(|x| x.is_finite());
    if v.is_empty() { return f64::NAN; }
    v.sort_by(f64::total_cmp);
    let pos = q / 100.0 * (v.len() - 1) as f64;
    let (i, j) = (pos.floor() as usize, pos.ceil() as usize);
    v[i] + (v[j] - v[i]) * (pos - i as f64)
}

fn r3(v: f64) -> Value {
    if v.is_finite() { json!((v * 1000.0).round() / 1000.0) } else { Value::Null }
}

fn load_samples(cfg: &Config, fx: &Fixture) -> Result<Vec<Sample>, String> {
    let text = std::fs::read_to_string(cfg.set_dir.join("sample-set.json")).map_err(|e| format!("读取样本集清单失败：{e}"))?;
    let doc: SetDoc = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("样本集清单无效：{e}"))?;
    if doc.id != fx.sample_set_id { return Err(format!("样本集 {} 与评测夹具 {} 不符", doc.id, fx.sample_set_id)); }
    let dir = cfg.set_dir.join("parts").join(&fx.part_id);
    let names: Vec<String> = std::fs::read_dir(&dir).map_err(|e| format!("读取 {} 失败：{e}", dir.display()))?
        .flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).collect();
    fx.shots.iter().map(|s| {
        let shot = doc.shots.iter().find(|x| x.shot_id == s.shot_id).ok_or_else(|| format!("样本集没有拍照点 {}", s.shot_id))?;
        let prefix = format!("{}_{}_v1.", shot.shot_id, shot.camera_id);
        let name = names.iter().find(|n| n.starts_with(&prefix)).ok_or_else(|| format!("缺图 {prefix}*"))?;
        Ok(Sample { shot_id: s.shot_id.clone(), camera: shot.camera_id.clone(), image: crate::image_file::load(&dir.join(name))? })
    }).collect()
}

fn teach(sample: &Sample, shot: &ShotFixture, fx: &Fixture) -> Result<Vec<[f32; 2]>, String> {
    let r = fx.brush_radius_px;
    let (w, h) = (sample.image.width as f32, sample.image.height as f32);
    let fold = |f: fn(f32, f32) -> f32, axis: usize, init: f32| shot.stroke.iter().map(|p| p[axis]).fold(init, f);
    let x0 = (fold(f32::min, 0, f32::MAX) - r).max(0.0).floor();
    let y0 = (fold(f32::min, 1, f32::MAX) - r).max(0.0).floor();
    let x1 = (fold(f32::max, 0, f32::MIN) + r).min(w - 1.0).ceil();
    let y1 = (fold(f32::max, 1, f32::MIN) + r).min(h - 1.0).ceil();
    let roi = [x0 as u32, y0 as u32, (x1 - x0) as u32 + 1, (y1 - y0) as u32 + 1];
    match extract(&sample.image, roi, Polarity::Dark, Some(&BrushGuide { points: shot.stroke.clone(), radius_px: r }), fx.point_spacing_px) {
        Ok(extracted) => Ok(extracted.path),
        Err(ExtractionError::Invalid(m)) => Err(m),
        Err(ExtractionError::Failed(f)) => Err(format!("{}（{}，{:?}）", f.message, f.reason, f.point)),
    }
}

fn build_recipe(samples: &[Sample], paths: &[Vec<[f32; 2]>], mm: &BTreeMap<String, f32>, detect: &DetectParams) -> Result<Recipe, String> {
    let shots = samples.iter().zip(paths).map(|(s, p)| ShotSpec {
        id: s.shot_id.clone(), pose_id: s.shot_id.clone(), camera: s.camera.clone(), view: 1, views: Vec::new(), calib: None,
        bead: "J1".into(), skip: p.is_empty(), path: p.clone(), mm_per_px: (!p.is_empty()).then(|| mm[&s.camera]), detect: None, limits: None,
        fixture: None,
    }).collect();
    let mut doc = RecipeDoc {
        id: "EVAL-GS".into(), name: "算法评测".into(), version: 1, plan_version: 1, teaching_id: None, product_code: 1,
        trigger_mode: TriggerMode::Fly, schema_version: RECIPE_SCHEMA, spacing: 1.0, filter_window: 5,
        detect: detect.clone(), limits: LIMITS.get().cloned().flatten().unwrap_or_else(default_limits), shots, cameras: BTreeMap::new(),
    };
    doc.sync_imaging();
    doc.build()
}

static GRAPH_PATCH: std::sync::OnceLock<Option<Value>> = std::sync::OnceLock::new();
/// GLUESIGHT_EVAL_LIMITS：整份判定限值（ShotLimits JSON），比较限值取值用
static LIMITS: std::sync::OnceLock<Option<ShotLimits>> = std::sync::OnceLock::new();

fn run(engine: &Engine, recipe: &Recipe, k: usize, image: &FrameImage) -> Result<ShotMeasurement, String> {
    if recipe.shots[k].skip { return Err("未示教".into()); }
    match GRAPH_PATCH.get().and_then(Option::as_ref) {
        Some(patch) => super::measure_shot_patched(engine, recipe, k, image, &format!("eval-{k}"), patch),
        None => measure_shot(engine, recipe, k, image, &format!("eval-{k}"), ""),
    }
}

struct Judged {
    verdict: Verdict,
    reason: String,
}

fn shot_judge(recipe: &Recipe, k: usize, m: &ShotMeasurement) -> Judged {
    let view = recipe.for_view(k, recipe.shots[k].view).unwrap();
    let mut table = vec![PointState::Pending; recipe.point_count()];
    for (i, &j) in m.idx.iter().enumerate() {
        table[j as usize] = crate::measure::point_state(m.st[i], m.d[i], m.w[i]);
    }
    let j = judge(&view, &table);
    Judged { verdict: j.verdict, reason: j.reason }
}

fn ok_class(v: Verdict) -> bool {
    v <= Verdict::OkWithExcursion
}

/// 超出限值或无胶的站（未做中值滤波），用于定位缺陷。
fn flagged(recipe: &Recipe, k: usize, m: &ShotMeasurement) -> Vec<bool> {
    let limits = recipe.shots[k].limits.as_ref().unwrap_or(&recipe.limits);
    (0..m.st.len()).map(|i| {
        m.st[i] != ST_OK
            || limits.width.as_ref().is_some_and(|p| m.w[i] < p.lower() || m.w[i] > p.upper())
            || limits.position.as_ref().is_some_and(|p| m.d[i] < p.lower() || m.d[i] > p.upper())
    }).collect()
}

fn frame_of(recipe: &Recipe, m: &ShotMeasurement) -> Option<BeadFrame> {
    let centers: Vec<[f32; 2]> = m.idx.iter().map(|&j| [recipe.points.x[j as usize], recipe.points.y[j as usize]]).collect();
    BeadFrame::new(&centers, &m.edges)
}

fn shot_summary(recipe: &Recipe, k: usize, m: &Result<ShotMeasurement, String>, mm: f32) -> Value {
    match m {
        Err(e) => json!({"shot": recipe.shots[k].id, "error": e}),
        Ok(m) => {
            let w: Vec<f64> = m.w.iter().zip(&m.st).filter(|(_, s)| **s == ST_OK).map(|(w, _)| *w as f64).collect();
            let d: Vec<f64> = m.d.iter().zip(&m.st).filter(|(_, s)| **s == ST_OK).map(|(d, _)| *d as f64).collect();
            let j = shot_judge(recipe, k, m);
            json!({"shot": recipe.shots[k].id, "camera": recipe.shots[k].camera, "stations": m.st.len(), "coverage": r3(m.coverage as f64),
                "widthMedianMm": r3(median(w.clone())), "widthP05Mm": r3(pct(w.clone(), 5.0)), "widthP95Mm": r3(pct(w.clone(), 95.0)),
                "widthMedianPx": r3(median(w.clone()) / mm as f64),
                "offsetMedianMm": r3(median(d.clone())), "offsetAbsP95Mm": r3(pct(d.iter().map(|v| v.abs()).collect(), 95.0)),
                "verdict": j.verdict, "reason": j.reason, "ms": m.ms,
                "states": m.st.iter().map(|s| match *s { ST_OK => '.', ST_GAP => 'x', _ => '?' }).collect::<String>(),
                "widthsMm": m.w.iter().map(|w| r3(*w as f64)).collect::<Vec<_>>()})
        }
    }
}

fn write_overlay(path: &Path, img: &FrameImage, recipe: &Recipe, k: usize, m: Option<&ShotMeasurement>, mark: Option<(usize, usize)>) {
    let mut rgb = image::RgbImage::from_fn(img.width, img.height, |x, y| {
        let v = img.pixels[(y * img.width + x) as usize];
        image::Rgb([v, v, v])
    });
    let mut dot = |x: f32, y: f32, c: [u8; 3]| {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (px, py) = (x.round() as i64 + dx, y.round() as i64 + dy);
                if px >= 0 && py >= 0 && (px as u32) < img.width && (py as u32) < img.height { rgb.put_pixel(px as u32, py as u32, image::Rgb(c)); }
            }
        }
    };
    for seg in recipe.shots[k].path.windows(2) {
        let n = ((seg[1][0] - seg[0][0]).hypot(seg[1][1] - seg[0][1]) / 2.0).ceil().max(1.0) as usize;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            dot(seg[0][0] + (seg[1][0] - seg[0][0]) * t, seg[0][1] + (seg[1][1] - seg[0][1]) * t, [0, 200, 0]);
        }
    }
    if let Some(m) = m {
        for (i, e) in m.edges.iter().enumerate() {
            let hot = mark.is_some_and(|(a, b)| (a..=b).contains(&i));
            match e {
                Some([l, r]) => { dot(l[0], l[1], if hot { [255, 160, 0] } else { [255, 40, 40] }); dot(r[0], r[1], if hot { [255, 160, 0] } else { [60, 120, 255] }); }
                None => dot(m.px[i][0], m.px[i][1], [255, 0, 255]),
            }
        }
    }
    let _ = rgb.save(path);
}

struct Ctx<'a> {
    cfg: &'a Config,
    engine: &'a Engine,
    recipe: &'a Recipe,
    samples: &'a [Sample],
    mm: &'a BTreeMap<String, f32>,
}

impl Ctx<'_> {
    fn mm_of(&self, k: usize) -> f32 {
        self.mm[&self.samples[k].camera]
    }
}

fn calibrate(cfg: &Config, engine: &Engine, samples: &[Sample], paths: &[Vec<[f32; 2]>], fx: &Fixture, detect: &DetectParams) -> Result<(BTreeMap<String, f32>, Value), String> {
    let mut mm: BTreeMap<String, f32> = fx.cameras.iter().map(|(c, f)| (c.clone(), f.mm_per_px.unwrap_or(0.2))).collect();
    let fixed: Vec<String> = fx.cameras.iter().filter(|(_, f)| f.mm_per_px.is_some()).map(|(c, _)| c.clone()).collect();
    let mut rounds = Vec::new();
    for round in 0..3 {
        let recipe = build_recipe(samples, paths, &mm, detect)?;
        let readings = par_map(samples, cfg.workers, |k, s| run(engine, &recipe, k, &s.image));
        let mut per: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for (k, r) in readings.iter().enumerate() {
            if let Ok(m) = r {
                let scale = mm[&samples[k].camera] as f64;
                per.entry(samples[k].camera.clone()).or_default().extend(m.w.iter().zip(&m.st).filter(|(_, s)| **s == ST_OK).map(|(w, _)| *w as f64 / scale));
            }
        }
        let mut row = serde_json::Map::new();
        for (camera, px) in &per {
            let med = median(px.clone());
            row.insert(camera.clone(), json!({"mmPerPx": mm[camera], "widthMedianPx": r3(med), "widthP05Px": r3(pct(px.clone(), 5.0)), "widthP95Px": r3(pct(px.clone(), 95.0)), "stations": px.len()}));
            if !fixed.contains(camera) && med.is_finite() && med > 0.0 {
                mm.insert(camera.clone(), ((fx.nominal_width_mm as f64 / med) * 10000.0).round() as f32 / 10000.0);
            }
        }
        rounds.push(json!({"round": round, "cameras": row}));
        if fixed.len() == fx.cameras.len() { break; }
    }
    Ok((mm.clone(), json!({"nominalWidthMm": fx.nominal_width_mm, "fixed": fixed, "rounds": rounds, "result": mm})))
}

fn baseline(ctx: &Ctx) -> Vec<Result<ShotMeasurement, String>> {
    par_map(ctx.samples, ctx.cfg.workers, |k, s| run(ctx.engine, ctx.recipe, k, &s.image))
}

fn repeat(ctx: &Ctx, base: &[Result<ShotMeasurement, String>]) -> Value {
    let jobs: Vec<(usize, usize)> = (0..ctx.samples.len()).flat_map(|k| (0..ctx.cfg.repeats).map(move |r| (k, r))).collect();
    let results = par_map(&jobs, ctx.cfg.workers, |_, &(k, _)| run(ctx.engine, ctx.recipe, k, &ctx.samples[k].image));
    let (mut max_d, mut max_w, mut st_diff, mut err_diff, mut runs) = (0f64, 0f64, 0usize, 0usize, 0usize);
    for (&(k, _), r) in jobs.iter().zip(&results) {
        runs += 1;
        match (&base[k], r) {
            (Ok(a), Ok(b)) => {
                st_diff += a.st.iter().zip(&b.st).filter(|(x, y)| x != y).count();
                for i in 0..a.st.len() {
                    if a.st[i] == ST_OK && b.st[i] == ST_OK {
                        max_d = max_d.max((a.d[i] - b.d[i]).abs() as f64);
                        max_w = max_w.max((a.w[i] - b.w[i]).abs() as f64);
                    }
                }
            }
            (Err(a), Err(b)) if a == b => {}
            _ => err_diff += 1,
        }
    }
    json!({"runs": runs, "repeatsPerShot": ctx.cfg.repeats, "maxAbsOffsetDiffMm": max_d, "maxAbsWidthDiffMm": max_w, "stationStateDiffs": st_diff, "resultTypeDiffs": err_diff})
}

fn perturbations() -> Vec<(&'static str, Perturb)> {
    let affine = |dx: f64, dy: f64, deg: f64, scale: f64| Perturb::Affine { dx, dy, deg, scale };
    vec![
        ("噪声 σ=3", Perturb::Noise(3.0)), ("噪声 σ=6", Perturb::Noise(6.0)), ("噪声 σ=12", Perturb::Noise(12.0)),
        ("亮度 −40", Perturb::Brightness(-40.0)), ("亮度 +40", Perturb::Brightness(40.0)),
        ("对比度 ×0.6", Perturb::Contrast(0.6)), ("对比度 ×0.4", Perturb::Contrast(0.4)), ("对比度 ×1.4", Perturb::Contrast(1.4)),
        ("Gamma 0.7", Perturb::Gamma(0.7)), ("Gamma 1.5", Perturb::Gamma(1.5)),
        ("高斯模糊 σ=1", Perturb::Blur(1.0)), ("高斯模糊 σ=2", Perturb::Blur(2.0)),
        ("运动模糊 5px 水平", Perturb::Motion(5, 0.0)), ("运动模糊 9px 水平", Perturb::Motion(9, 0.0)), ("运动模糊 9px 竖直", Perturb::Motion(9, 90.0)),
        ("平移 +5,0", affine(5.0, 0.0, 0.0, 1.0)), ("平移 0,+5", affine(0.0, 5.0, 0.0, 1.0)),
        ("平移 +10,+10", affine(10.0, 10.0, 0.0, 1.0)), ("平移 +15,0", affine(15.0, 0.0, 0.0, 1.0)), ("平移 0,−15", affine(0.0, -15.0, 0.0, 1.0)),
        ("平移 +25,0", affine(25.0, 0.0, 0.0, 1.0)), ("平移 +40,0", affine(40.0, 0.0, 0.0, 1.0)),
        ("旋转 +1°", affine(0.0, 0.0, 1.0, 1.0)), ("旋转 −2°", affine(0.0, 0.0, -2.0, 1.0)), ("缩放 ×1.03", affine(0.0, 0.0, 0.0, 1.03)),
    ]
}

/// 几何扰动后胶相对示教线的实际偏移（基准测得的偏移 + 扰动横移，按判定同样做中值滤波）：
/// 明显超绝对限、或明显超差且连续够长的应判位置 NG；明显在公差和绝对限内的应判 OK；夹在中间的不计。
fn expected_position(recipe: &Recipe, k: usize, offset: &[f64]) -> Option<bool> {
    let limits = recipe.shots[k].limits.as_ref().unwrap_or(&recipe.limits);
    let Some(p) = limits.position.as_ref() else { return Some(false) };
    let half = recipe.filter_window / 2;
    let offset: Vec<f64> = (0..offset.len()).map(|i| median(offset[i.saturating_sub(half)..(i + half + 1).min(offset.len())].to_vec())).collect();
    let nominal = p.nominal as f64;
    let beyond = |v: f64, lo: f32, hi: f32, m: f64| v < nominal - (nominal - lo as f64) * m || v > nominal + (hi as f64 - nominal) * m;
    let longest = |m: f64| offset.split(|&v| !beyond(v, p.lower(), p.upper(), m)).map(|run| run.len() as f64 * recipe.spacing as f64).fold(0.0, f64::max);
    let absolute = |m: f64| offset.iter().any(|&v| beyond(v, p.abs_min, p.abs_max, m));
    if absolute(1.15) || longest(1.15) >= p.max_excursion_len as f64 + 2.0 { Some(true) }
    else if !absolute(0.85) && longest(0.85) <= (p.max_excursion_len as f64 - 2.0).max(0.0) { Some(false) }
    else { None }
}

fn perturb(ctx: &Ctx, base: &[Result<ShotMeasurement, String>]) -> Value {
    let list = perturbations();
    let shots: Vec<usize> = (0..ctx.samples.len()).filter(|&k| base[k].as_ref().is_ok_and(|m| ok_class(shot_judge(ctx.recipe, k, m).verdict))).collect();
    let jobs: Vec<(usize, usize)> = (0..list.len()).flat_map(|p| shots.iter().map(move |&k| (p, k))).collect();
    let results = par_map(&jobs, ctx.cfg.workers, |_, &(p, k)| {
        let img = list[p].1.apply(&ctx.samples[k].image, (k as u64) << 8 | p as u64);
        run(ctx.engine, ctx.recipe, k, &img)
    });
    let mut rows = Vec::new();
    for (p, (name, pert)) in list.iter().enumerate() {
        let (mut false_ng, mut errors, mut cov_drop, mut dw, mut derr, mut lost) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), 0usize);
        let (mut expect_ok, mut expect_ng, mut missed_ng, mut ambiguous) = (0usize, 0usize, Vec::new(), 0usize);
        let mut per_shot = Vec::new();
        for (&(pp, k), r) in jobs.iter().zip(&results) {
            if pp != p { continue; }
            let b = base[k].as_ref().unwrap();
            let mm = ctx.mm_of(k) as f64;
            let r = match r { Ok(r) => r, Err(e) => { errors.push(format!("{}：{e}", ctx.samples[k].shot_id)); continue; } };
            let j = shot_judge(ctx.recipe, k, r);
            let frame = frame_of(ctx.recipe, b);
            let shift: Vec<f64> = (0..b.st.len()).map(|i| frame.as_ref().map_or(0.0, |f| {
                let c = f.c[i];
                let q = pert.moved(c, ctx.samples[k].image.width, ctx.samples[k].image.height);
                ((q[0] - c[0]) * f.n[i][0] + (q[1] - c[1]) * f.n[i][1]) * mm
            })).collect();
            let offset: Vec<f64> = (0..b.st.len()).map(|i| shift[i] + if b.st[i] == ST_OK { b.d[i] as f64 } else {
                frame.as_ref().map_or(0.0, |f| 0.5 * (f.lo[i] + f.hi[i]) * mm)
            }).collect();
            let expected = expected_position(ctx.recipe, k, &offset);
            match expected {
                Some(false) => {
                    expect_ok += 1;
                    if !ok_class(j.verdict) { false_ng.push(format!("{} {:?} {}", ctx.samples[k].shot_id, j.verdict, j.reason)); }
                }
                Some(true) => {
                    expect_ng += 1;
                    if ok_class(j.verdict) { missed_ng.push(format!("{} {:?} {}", ctx.samples[k].shot_id, j.verdict, j.reason)); }
                }
                None => ambiguous += 1,
            }
            cov_drop.push((b.coverage - r.coverage) as f64);
            let (mut sdw, mut sderr) = (Vec::new(), Vec::new());
            for i in 0..b.st.len() {
                if b.st[i] != ST_OK { continue; }
                if r.st[i] != ST_OK { lost += 1; continue; }
                sdw.push((r.w[i] - b.w[i]) as f64);
                sderr.push((r.d[i] - b.d[i]) as f64 - shift[i]);
            }
            per_shot.push(json!({"shot": ctx.samples[k].shot_id, "expectNg": expected, "offsetAbsP95Mm": r3(pct(offset.iter().map(|v| v.abs()).collect(), 95.0)),
                "verdict": j.verdict, "coverageDrop": r3((b.coverage - r.coverage) as f64),
                "widthDriftMedianMm": r3(median(sdw.clone())), "offsetErrAbsP95Mm": r3(pct(sderr.iter().map(|v| v.abs()).collect(), 95.0))}));
            dw.extend(sdw);
            derr.extend(sderr);
        }
        rows.push(json!({"name": name, "shots": shots.len(), "expectOk": expect_ok, "falseNg": false_ng.len(), "falseNgDetail": false_ng,
            "expectNg": expect_ng, "missedNg": missed_ng.len(), "missedNgDetail": missed_ng, "ambiguous": ambiguous, "errors": errors,
            "coverageDropMean": r3(cov_drop.iter().sum::<f64>() / cov_drop.len().max(1) as f64), "coverageDropMax": r3(cov_drop.iter().copied().fold(0.0, f64::max)),
            "lostStations": lost, "widthDriftMedianMm": r3(median(dw.clone())), "widthDriftAbsP95Mm": r3(pct(dw.iter().map(|v| v.abs()).collect(), 95.0)),
            "offsetErrMedianMm": r3(median(derr.clone())), "offsetErrAbsP95Mm": r3(pct(derr.iter().map(|v| v.abs()).collect(), 95.0)),
            "perShot": per_shot}));
    }
    json!(rows)
}

struct DefectSpec {
    name: &'static str,
    kind: Defect,
    len_mm: f64,
    /// 宽度 / 偏移从正常过渡到缺陷的长度（mm）
    ramp_mm: f64,
    /// 超出公差、应判 NG
    ng: bool,
    expect: &'static [Verdict],
}

fn defect_specs() -> Vec<DefectSpec> {
    use Verdict::*;
    vec![
        DefectSpec { name: "断胶 3mm", kind: Defect::Break, len_mm: 3.0, ramp_mm: 3.0, ng: false, expect: &[] },
        DefectSpec { name: "断胶 10mm", kind: Defect::Break, len_mm: 10.0, ramp_mm: 3.0, ng: true, expect: &[NgGap] },
        DefectSpec { name: "断胶 20mm", kind: Defect::Break, len_mm: 20.0, ramp_mm: 3.0, ng: true, expect: &[NgGap] },
        DefectSpec { name: "窄胶 ×0.75 15mm", kind: Defect::Width(0.75), len_mm: 15.0, ramp_mm: 3.0, ng: false, expect: &[] },
        DefectSpec { name: "窄胶 ×0.5 15mm", kind: Defect::Width(0.5), len_mm: 15.0, ramp_mm: 3.0, ng: true, expect: &[NgWidth, NgGap] },
        DefectSpec { name: "宽胶 ×1.2 15mm", kind: Defect::Width(1.2), len_mm: 15.0, ramp_mm: 3.0, ng: false, expect: &[] },
        DefectSpec { name: "宽胶 ×1.6 15mm", kind: Defect::Width(1.6), len_mm: 15.0, ramp_mm: 3.0, ng: true, expect: &[NgWidth] },
        DefectSpec { name: "偏移 1mm 15mm", kind: Defect::Offset(1.0), len_mm: 15.0, ramp_mm: 5.0, ng: false, expect: &[] },
        DefectSpec { name: "偏移 3mm 15mm（3mm 内突变）", kind: Defect::Offset(3.0), len_mm: 15.0, ramp_mm: 3.0, ng: true, expect: &[NgPosition, NgAbsolute] },
        DefectSpec { name: "偏移 6mm 15mm（3mm 内突变）", kind: Defect::Offset(6.0), len_mm: 15.0, ramp_mm: 3.0, ng: true, expect: &[NgAbsolute, NgPosition] },
        DefectSpec { name: "偏移 3mm 15mm（12mm 渐变）", kind: Defect::Offset(3.0), len_mm: 15.0, ramp_mm: 12.0, ng: true, expect: &[NgPosition, NgAbsolute] },
        DefectSpec { name: "偏移 6mm 15mm（20mm 渐变）", kind: Defect::Offset(6.0), len_mm: 15.0, ramp_mm: 20.0, ng: true, expect: &[NgAbsolute, NgPosition] },
        DefectSpec { name: "气泡 0.5w", kind: Defect::Bubble(0.5), len_mm: 2.0, ramp_mm: 3.0, ng: false, expect: &[] },
        DefectSpec { name: "缺口 半宽 3mm", kind: Defect::Notch, len_mm: 3.0, ramp_mm: 3.0, ng: false, expect: &[] },
        DefectSpec { name: "缺口 半宽 10mm", kind: Defect::Notch, len_mm: 10.0, ramp_mm: 3.0, ng: true, expect: &[NgWidth, NgGap] },
    ]
}

fn place(base: &ShotMeasurement, stations: usize, ramp: usize) -> Option<(usize, usize)> {
    let n = base.st.len();
    let pad = ramp + 4;
    if n < stations + 2 * pad + 10 { return None; }
    let center = n / 2;
    let mut best: Option<(usize, usize)> = None;
    for a in pad + 5..n.saturating_sub(stations + pad + 5) {
        let b = a + stations;
        if (a - pad..=b + pad).all(|i| base.st[i] == ST_OK) {
            let dist = (a + stations / 2).abs_diff(center);
            if best.is_none_or(|(ba, _)| dist < (ba + stations / 2).abs_diff(center)) { best = Some((a, b)); }
        }
    }
    best
}

fn defects(ctx: &Ctx, base: &[Result<ShotMeasurement, String>]) -> Value {
    let specs = defect_specs();
    let spacing = ctx.recipe.spacing as f64;
    let shots: Vec<usize> = (0..ctx.samples.len()).filter(|&k| base[k].as_ref().is_ok_and(|m| ok_class(shot_judge(ctx.recipe, k, m).verdict))).collect();
    let jobs: Vec<(usize, usize)> = (0..specs.len()).flat_map(|d| shots.iter().map(move |&k| (d, k))).collect();
    let example_dir = ctx.cfg.out.join("defects");
    let _ = std::fs::create_dir_all(&example_dir);
    let results = par_map(&jobs, ctx.cfg.workers, |_, &(d, k)| {
        let spec = &specs[d];
        let b = base[k].as_ref().unwrap();
        let ramp = (spec.ramp_mm / spacing).round() as usize;
        let stations = ((spec.len_mm / spacing).round() as usize).max(1);
        let (a, bb) = place(b, stations, ramp)?;
        let frame = frame_of(ctx.recipe, b)?;
        let mm = ctx.mm_of(k) as f64;
        let kind = match spec.kind { Defect::Offset(v) => Defect::Offset(v / mm), other => other };
        let ramp_st = if matches!(kind, Defect::Width(_) | Defect::Offset(_)) { ramp as f64 } else { 0.0 };
        let img = synth::defect(&ctx.samples[k].image, &frame, a as f64, bb as f64, kind, ramp_st);
        let r = run(ctx.engine, ctx.recipe, k, &img);
        if k == shots[0] || ctx.samples[k].shot_id == "S013" {
            write_overlay(&example_dir.join(format!("{}-{}.png", ctx.samples[k].shot_id, d)), &img, ctx.recipe, k, r.as_ref().ok(), Some((a, bb)));
        }
        Some((a, bb, r))
    });
    let mut rows = Vec::new();
    for (d, spec) in specs.iter().enumerate() {
        let (mut placed, mut detected, mut correct, mut errors, mut loc, mut spill) = (0usize, 0usize, 0usize, Vec::new(), Vec::new(), 0usize);
        let mut misses = Vec::new();
        let mut classes: BTreeMap<String, usize> = BTreeMap::new();
        let mut detail = Vec::new();
        for (&(dd, k), r) in jobs.iter().zip(&results) {
            if dd != d { continue; }
            let Some((a, b, r)) = r else { continue };
            placed += 1;
            let r = match r { Ok(r) => r, Err(e) => { errors.push(format!("{}：{e}", ctx.samples[k].shot_id)); continue; } };
            let j = shot_judge(ctx.recipe, k, r);
            let ng = !ok_class(j.verdict);
            if ng == spec.ng { detected += 1; } else { misses.push(format!("{} {:?} {}", ctx.samples[k].shot_id, j.verdict, j.reason)); }
            if spec.ng && spec.expect.contains(&j.verdict) { correct += 1; }
            *classes.entry(format!("{:?}", j.verdict)).or_default() += 1;
            if spec.ng {
                let states: String = r.st.iter().enumerate().filter(|(i, _)| i + 8 >= *a && *i <= b + 8).map(|(_, s)| match *s { ST_OK => '.', ST_GAP => 'x', _ => '?' }).collect();
                detail.push(json!({"shot": ctx.samples[k].shot_id, "verdict": j.verdict, "reason": j.reason, "states": states}));
                let f = flagged(ctx.recipe, k, r);
                let ramp = (spec.ramp_mm / spacing).round() as usize + 3;
                let hits: Vec<usize> = (0..f.len()).filter(|&i| f[i] && i + ramp >= *a && i <= b + ramp).collect();
                spill += (0..f.len()).filter(|&i| f[i] && (i + ramp < *a || i > b + ramp)).count();
                if let (Some(&lo), Some(&hi)) = (hits.first(), hits.last()) {
                    loc.push(((lo + hi) as f64 * 0.5 - (a + b) as f64 * 0.5).abs() * spacing);
                }
            }
        }
        rows.push(json!({"name": spec.name, "ng": spec.ng, "placed": placed, "asExpected": detected, "rate": r3(detected as f64 / placed.max(1) as f64),
            "correctClass": if spec.ng { json!(correct) } else { Value::Null }, "classes": classes, "detail": detail, "errors": errors, "misses": misses,
            "locErrMedianMm": r3(median(loc.clone())), "locErrMaxMm": r3(loc.iter().copied().fold(f64::NAN, f64::max)), "spillStations": spill}));
    }
    json!({"shots": shots.len(), "rows": rows})
}

fn timing(ctx: &Ctx, base: &[Result<ShotMeasurement, String>]) -> Value {
    let ms: Vec<f64> = base.iter().filter_map(|r| r.as_ref().ok()).map(|m| m.ms as f64).collect();
    let taught: Vec<usize> = (0..ctx.samples.len()).filter(|&k| !ctx.recipe.shots[k].skip).collect();
    let mut seq = Vec::new();
    let mut par = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        for &k in &taught { let _ = run(ctx.engine, ctx.recipe, k, &ctx.samples[k].image); }
        seq.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let _ = par_map(&taught, 4, |_, &k| run(ctx.engine, ctx.recipe, k, &ctx.samples[k].image));
        par.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    json!({"shotMsMedian": r3(median(ms.clone())), "shotMsP95": r3(pct(ms.clone(), 95.0)), "shotMsMax": r3(ms.iter().copied().fold(0.0, f64::max)),
        "partShots": taught.len(), "partSequentialMs": r3(median(seq)), "partFourWorkersMs": r3(median(par)),
        "profile": if cfg!(debug_assertions) { "test（gluesight opt-level 2 + debug 断言）" } else { "release" }})
}

fn edge_cases(ctx: &Ctx) -> Value {
    let k = (0..ctx.samples.len()).find(|&k| !ctx.recipe.shots[k].skip).unwrap();
    let (w, h) = (ctx.samples[k].image.width, ctx.samples[k].image.height);
    let n = (w * h) as usize;
    let mut rng = synth::Rng(7);
    let cases: Vec<(&str, FrameImage)> = vec![
        ("全黑", FrameImage::new(w, h, vec![0; n])),
        ("全白（过曝）", FrameImage::new(w, h, vec![255; n])),
        ("均匀灰 128", FrameImage::new(w, h, vec![128; n])),
        ("均匀随机噪声", FrameImage::new(w, h, (0..n).map(|_| (rng.next() & 255) as u8).collect())),
        ("横向渐变", FrameImage::new(w, h, (0..n).map(|i| ((i as u32 % w) * 255 / w) as u8).collect())),
        ("尺寸缩小一半（中线超出图像）", FrameImage::new(w / 2, h / 2, vec![128; n / 4])),
        ("像素缓冲不符", FrameImage::new(w, h, vec![128; n - 1])),
    ];
    let outside = (0..ctx.samples.len()).find(|&k| !ctx.recipe.shots[k].skip && ctx.recipe.shots[k].path.iter().any(|p| p[0] >= (w / 2) as f32 || p[1] >= (h / 2) as f32)).unwrap_or(k);
    json!(cases.iter().map(|(name, img)| {
        let k = if img.width != w { outside } else { k };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(ctx.engine, ctx.recipe, k, img)));
        match r {
            Err(_) => json!({"case": name, "outcome": "panic"}),
            Ok(Err(e)) => json!({"case": name, "outcome": "error", "message": e}),
            Ok(Ok(m)) => { let j = shot_judge(ctx.recipe, k, &m); json!({"case": name, "outcome": "measured", "coverage": r3(m.coverage as f64), "verdict": j.verdict, "reason": j.reason}) }
        }
    }).collect::<Vec<_>>())
}

fn markdown(report: &Value) -> String {
    let mut s = String::new();
    let v = |x: &Value| match x { Value::Null => "—".to_string(), Value::String(t) => t.clone(), other => other.to_string() };
    s += &format!("# 算法评测结果\n\n- 样本集：{}\n- 核心库：{}（{}）\n- 检测参数：{}\n- 判定限值：{}\n- 算法图参数覆盖：{}\n- 像素当量：{}\n\n", v(&report["sampleSet"]), v(&report["engine"]["version"]), v(&report["engine"]["path"]), report["detect"], report["limits"], report["graphPatch"], report["mmPerPx"]);
    if let Some(teach) = report["teach"].as_array() {
        let failed: Vec<_> = teach.iter().filter(|t| t["error"].is_string()).collect();
        s += &format!("## 示教提取\n\n{} 个拍照点，提取失败 {} 个。\n\n", teach.len(), failed.len());
        for t in failed { s += &format!("- {}{}：{}\n", v(&t["shot"]), if t["fallbackStroke"] == true { "（改用笔划直接生成中线）" } else { "" }, v(&t["error"])); }
        s += "\n";
    }
    if let Some(rows) = report["baseline"].as_array() {
        s += "## 基准（原图）\n\n| 拍照点 | 相机 | 站数 | 覆盖率 | 胶宽中位 mm | P05–P95 mm | 胶宽中位 px | 偏移中位 mm | |偏移| P95 mm | 判定 | ms |\n|---|---|---|---|---|---|---|---|---|---|---|\n";
        for r in rows {
            if r["error"].is_string() { s += &format!("| {} | | | | | | | | | 错误：{} | |\n", v(&r["shot"]), v(&r["error"])); continue; }
            s += &format!("| {} | {} | {} | {} | {} | {}–{} | {} | {} | {} | {} | {} |\n", v(&r["shot"]), v(&r["camera"]), v(&r["stations"]), v(&r["coverage"]),
                v(&r["widthMedianMm"]), v(&r["widthP05Mm"]), v(&r["widthP95Mm"]), v(&r["widthMedianPx"]), v(&r["offsetMedianMm"]), v(&r["offsetAbsP95Mm"]), v(&r["verdict"]), v(&r["ms"]));
        }
        s += "\n";
    }
    if report["repeat"].is_object() { s += &format!("## 重复运行\n\n{}\n\n", report["repeat"]); }
    if let Some(rows) = report["perturb"].as_array() {
        s += "## 扰动稳定性（只看基准判 OK 的拍照点）\n\n应判 OK / 应判 NG 按胶实际偏开示教线多少算：基准测得的偏移加扰动横移，中值滤波后对照位置公差 ±2 mm、允许连续超差 5 mm 和绝对限 ±5 mm，留 15% 余量，夹在中间的不计。\n\n| 扰动 | 应判 OK | 误判 NG | 应判 NG | 漏判 | 不计 | 出错 | 覆盖率降 均/最大 | 丢站 | 胶宽漂移 中位 / |P95| mm | 偏移误差 中位 / |P95| mm |\n|---|---|---|---|---|---|---|---|---|---|---|\n";
        for r in rows {
            s += &format!("| {} | {} | {} | {} | {} | {} | {} | {} / {} | {} | {} / {} | {} / {} |\n", v(&r["name"]), v(&r["expectOk"]), v(&r["falseNg"]), v(&r["expectNg"]), v(&r["missedNg"]), v(&r["ambiguous"]), r["errors"].as_array().map_or(0, Vec::len),
                v(&r["coverageDropMean"]), v(&r["coverageDropMax"]), v(&r["lostStations"]), v(&r["widthDriftMedianMm"]), v(&r["widthDriftAbsP95Mm"]), v(&r["offsetErrMedianMm"]), v(&r["offsetErrAbsP95Mm"]));
        }
        s += "\n";
        for r in rows {
            for d in r["falseNgDetail"].as_array().into_iter().flatten().chain(r["errors"].as_array().into_iter().flatten()) { s += &format!("- {}：{}\n", v(&r["name"]), v(d)); }
            for d in r["missedNgDetail"].as_array().into_iter().flatten() { s += &format!("- {}（漏判）：{}\n", v(&r["name"]), v(d)); }
        }
        s += "\n";
    }
    if let Some(rows) = report["defect"]["rows"].as_array() {
        s += &format!("## 合成缺陷（{} 个基准 OK 的拍照点）\n\n| 缺陷 | 应判 | 放置 | 符合预期 | 比例 | 类别正确 | 判定类别 | 定位误差 中位 / 最大 mm | 溢出站 |\n|---|---|---|---|---|---|---|---|---|\n", v(&report["defect"]["shots"]));
        for r in rows {
            let classes: Vec<String> = r["classes"].as_object().into_iter().flatten().map(|(k, n)| format!("{k} {n}")).collect();
            s += &format!("| {} | {} | {} | {} | {} | {} | {} | {} / {} | {} |\n", v(&r["name"]), if r["ng"] == true { "NG" } else { "OK" }, v(&r["placed"]), v(&r["asExpected"]), v(&r["rate"]),
                v(&r["correctClass"]), classes.join("，"), v(&r["locErrMedianMm"]), v(&r["locErrMaxMm"]), v(&r["spillStations"]));
        }
        s += "\n";
        for r in rows {
            for d in r["misses"].as_array().into_iter().flatten().chain(r["errors"].as_array().into_iter().flatten()) { s += &format!("- {}：{}\n", v(&r["name"]), v(d)); }
        }
        s += "\n";
    }
    if report["extract"].is_object() { s += &teach::markdown(&report["extract"]); }
    if report["fixture"].is_object() { s += &locate::markdown(&report["fixture"]); }
    if report["timing"].is_object() { s += &format!("## 耗时\n\n{}\n\n", report["timing"]); }
    if let Some(rows) = report["edge"].as_array() {
        s += "## 边界输入\n\n";
        for r in rows { s += &format!("- {}：{} {} {}\n", v(&r["case"]), v(&r["outcome"]), v(&r["verdict"]), v(r.get("message").or(r.get("reason")).unwrap_or(&Value::Null))); }
    }
    s
}

#[test]
#[ignore = "需要 GLUESIGHT_SAMPLE_SET、LYFLOW_CORE_DLL 与实拍样本集，见 scripts/Test-AlgorithmEval.ps1"]
fn algorithm_eval() {
    let cfg = Config::from_env().unwrap();
    let fx: Fixture = serde_json::from_str(&std::fs::read_to_string(&cfg.fixture).unwrap()).unwrap();
    std::fs::create_dir_all(&cfg.out).unwrap();
    let samples = load_samples(&cfg, &fx).unwrap();
    let engine = Engine::load(&cfg.core).unwrap();
    let detect = cfg.detect.clone().unwrap_or_else(crate::recipe::default_detect);
    let patch: Option<Value> = std::env::var("GLUESIGHT_EVAL_GRAPH").ok().filter(|v| !v.trim().is_empty()).map(|v| serde_json::from_str(&v).expect("GLUESIGHT_EVAL_GRAPH 无效"));
    GRAPH_PATCH.set(patch.clone()).unwrap();
    LIMITS.set(std::env::var("GLUESIGHT_EVAL_LIMITS").ok().filter(|v| !v.trim().is_empty()).map(|v| serde_json::from_str(&v).expect("GLUESIGHT_EVAL_LIMITS 无效"))).unwrap();

    let taught: Vec<Result<Vec<[f32; 2]>, String>> = samples.iter().zip(&fx.shots).map(|(s, f)| teach(s, f, &fx)).collect();
    let paths: Vec<Vec<[f32; 2]>> = taught.iter().zip(&fx.shots).map(|(t, f)| match t {
        Ok(p) => p.clone(),
        Err(_) if f.fallback_stroke => f.stroke.clone(),
        Err(_) => Vec::new(),
    }).collect();
    let (mm, calib) = calibrate(&cfg, &engine, &samples, &paths, &fx, &detect).unwrap();
    let recipe = build_recipe(&samples, &paths, &mm, &detect).unwrap();
    let ctx = Ctx { cfg: &cfg, engine: &engine, recipe: &recipe, samples: &samples, mm: &mm };

    let mut report = json!({
        "sampleSet": fx.sample_set_id, "engine": {"version": engine.version, "path": cfg.core.display().to_string()},
        "detect": detect, "graphPatch": patch, "limits": recipe.limits, "spacing": recipe.spacing, "mmPerPx": mm, "calibration": calib,
        "teach": samples.iter().zip(&taught).zip(&paths).map(|((s, t), p)| json!({"shot": s.shot_id, "error": t.as_ref().err(),
            "fallbackStroke": t.is_err() && !p.is_empty(), "points": p.len(),
            "lengthPx": r3(p.windows(2).map(|w| ((w[1][0] - w[0][0]) as f64).hypot((w[1][1] - w[0][1]) as f64)).sum()), "path": p})).collect::<Vec<_>>(),
    });
    let base = baseline(&ctx);
    report["baseline"] = json!((0..samples.len()).map(|k| shot_summary(&recipe, k, &base[k], mm[&samples[k].camera])).collect::<Vec<_>>());
    let overlay_dir = cfg.out.join("baseline");
    std::fs::create_dir_all(&overlay_dir).unwrap();
    for (k, s) in samples.iter().enumerate() {
        if !recipe.shots[k].skip { write_overlay(&overlay_dir.join(format!("{}.png", s.shot_id)), &s.image, &recipe, k, base[k].as_ref().ok(), None); }
    }
    if let Ok(dump) = std::env::var("GLUESIGHT_EVAL_DUMP") {
        for id in dump.split(',') {
            let Some(k) = samples.iter().position(|s| s.shot_id == id.trim()) else { continue };
            let graph = super::build_taught_graph(&recipe, k).unwrap();
            let r = engine.run(&graph.to_string(), "eval-dump", "", &samples[k].image, &json!({})).unwrap();
            std::fs::write(cfg.out.join(format!("dump-{id}.json")), serde_json::to_string_pretty(&json!({"graph": graph, "summary": r.summary, "outputs": r.outputs})).unwrap()).unwrap();
        }
    }
    if cfg.has("repeat") { report["repeat"] = repeat(&ctx, &base); }
    if cfg.has("perturb") { report["perturb"] = perturb(&ctx, &base); }
    if cfg.has("defect") { report["defect"] = defects(&ctx, &base); }
    if cfg.has("timing") { report["timing"] = timing(&ctx, &base); }
    if cfg.has("edge") { report["edge"] = edge_cases(&ctx); }
    if cfg.has("extract") { report["extract"] = teach::evaluate(&ctx, &base, &fx); }
    if cfg.has("fixture") { report["fixture"] = locate::evaluate(&ctx, &base); }
    std::fs::write(cfg.out.join("results.json"), serde_json::to_string_pretty(&report).unwrap()).unwrap();
    std::fs::write(cfg.out.join("summary.md"), markdown(&report)).unwrap();
    println!("{}", markdown(&report));
}
