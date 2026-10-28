//! 逐帧定向离线评估（D-SB-47 / D-SB-48 确认前的原型）。不是测试：结论取决于现场数据，没有固定的回归可防。
//!
//! 读一个项目里某个候选的 workspace.json（段、各段检测相机与段走廊、已确认的手动分段）和 recipe-capture 原图，
//! 对每个已确认轮次的每一帧、真实所在段的每台检测相机比较三种测法“测到胶”的比例（有胶测站 ≥ 80%）：
//! 1. 示教走廊，按手动分段的真实段选（现行做法的上限）；
//! 2. 示教走廊，按累计 N_exp 预测的段选（现行生产 D-SB-45，预测段不由这台相机检测时这一帧不测）；
//! 3. `glue.bead_path` 定向 + `glue.bead_width`（D-SB-47），锚点取“本段走廊起点”或“这台相机各段走廊起点的中位”，
//!    检测窗口 [s₀, s₀ + L] 取 `HEADING_ZONES`（mm，默认 `2:10,5:15`），扇区取这台相机各段示教方向 ± 25°；
//!    `info.ok` 为假时按预测段的示教方向回退再测一次。
//!
//! 另外：用直线段检测相机的方向序列在比例边界 ±4 帧内细化分段边界（留一标定 N_exp，对照手动边界）；
//! 按相邻帧图像位移估计每帧后移量（mm），给出窗口长度 L 的下限；在直线段上试 `glue.edge_distance` 找零件边（D-SB-48）。
//! 起涂、收尾（D-SB-50）：每轮第 1 段从第 1 帧正向、末段从最后一帧倒着，按测法 1 判帧组成形（这一段带走廊的检测相机
//! 有胶测站都 ≥ 80%；有相机没有结果为无法判断），用 `unformed_lead` 数第一处连续 `ONSET_RUN` 个成形帧组之前的帧组数；
//! 数不出时这一头不排除并列出原因。各轮的值与分布单列一节，其余统计（图像位移除外）排除这些帧组。
//! 逐次运行写入 `<输出目录>/runs.csv`（含全部帧组）。
//!
//! ```powershell
//! $env:LYFLOW_CORE_DLL = "<运行库>\lyflow_core.dll"
//! cargo run --manifest-path src-tauri/Cargo.toml --release --example heading_eval -- C:\ProgramData\ly\GlueSight-dev\projects\seg-new14 NEW-14 <输出目录>
//! ```

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gluesight_lib::probe_api::{
    calibrate_counts, downsample, load_image, unformed_lead, Engine, FrameImage, GroupForm,
    RecipeDoc, SegmentKind, ONSET_RUN,
};
use serde_json::{json, Value};

const PRESENT_OK: f64 = 0.8;
const SECTOR_MARGIN: f64 = 25.0;
const FALLBACK_TOL: f64 = 15.0;
const REFINE_WINDOW: u64 = 4;
const REFINE_TOL: f64 = 8.0;
const SHIFT_FACTOR: u32 = 8;
const SHIFT_MAX: i32 = 40;

struct Cam {
    id: String,
    mm_per_px: f64,
    corridors: BTreeMap<u32, Vec<[f64; 2]>>,
}

impl Cam {
    fn start(&self, segment: u32) -> Option<[f64; 2]> {
        self.corridors.get(&segment).map(|c| c[0])
    }

    fn heading(&self, segment: u32) -> Option<f64> {
        let c = self.corridors.get(&segment)?;
        let (a, b) = (c[0], c[c.len() - 1]);
        Some((b[1] - a[1]).atan2(b[0] - a[0]).to_degrees())
    }

    fn median_start(&self) -> [f64; 2] {
        let mut xs: Vec<f64> = self.corridors.values().map(|c| c[0][0]).collect();
        let mut ys: Vec<f64> = self.corridors.values().map(|c| c[0][1]).collect();
        [median(&mut xs), median(&mut ys)]
    }

    fn sector(&self) -> [f64; 2] {
        let headings: Vec<f64> = self
            .corridors
            .keys()
            .filter_map(|&s| self.heading(s))
            .collect();
        let base = headings[0];
        let rel: Vec<f64> = headings.iter().map(|h| wrap(h - base)).collect();
        let lo = rel.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = rel.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        [base + lo - SECTOR_MARGIN, base + hi + SECTOR_MARGIN]
    }
}

struct Seg {
    index: u32,
    straight: bool,
    cameras: Vec<String>,
    n_exp: Option<u32>,
    tolerance: u32,
}

struct Truth {
    round: String,
    total: u64,
    cuts: Vec<u64>,
}

impl Truth {
    fn segment_at(&self, seq: u64) -> usize {
        self.cuts.iter().take_while(|&&cut| seq > cut).count()
    }

    fn counts(&self) -> Vec<u32> {
        let mut previous = 0;
        self.cuts
            .iter()
            .chain(std::iter::once(&self.total))
            .map(|&cut| {
                let n = cut - previous;
                previous = cut;
                n as u32
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
struct Zone {
    s0: f64,
    len: f64,
}

#[derive(Clone)]
enum Variant {
    Taught {
        label: String,
        corridor: Vec<[f64; 2]>,
        edge: Option<&'static str>,
    },
    Bead {
        label: String,
        anchor: [f64; 2],
        zone: Zone,
        sector: [f64; 2],
        fallback: Option<f64>,
        edge: Option<&'static str>,
    },
}

struct Job {
    round: usize,
    camera: usize,
    seq: u64,
    variants: Vec<Variant>,
}

#[derive(Clone, Default)]
struct Outcome {
    label: String,
    present: Option<f64>,
    heading: Option<f64>,
    ok: Option<bool>,
    fallback: bool,
    beadlike: Option<bool>,
    edge_found: Option<f64>,
    edge_mm: Option<f64>,
    offset_mm: Option<f64>,
    ms: f64,
    error: Option<String>,
}

struct Measured {
    round: usize,
    camera: usize,
    seq: u64,
    outcomes: Vec<Outcome>,
}

fn wrap(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() {
        return f64::NAN;
    }
    v[v.len() / 2]
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() {
        return f64::NAN;
    }
    v[((v.len() - 1) as f64 * p).round() as usize]
}

struct Px {
    lo: f64,
    hi: f64,
    half: f64,
    step: f64,
}

fn px(doc: &RecipeDoc, scale: f64) -> Px {
    Px {
        lo: f64::from(doc.detect.width_range[0]) / scale,
        hi: f64::from(doc.detect.width_range[1]) / scale,
        half: f64::from(doc.detect.search_mm) / scale,
        step: f64::from(doc.spacing) / scale,
    }
}

fn width_node(p: &Px) -> Value {
    json!({"id": "n_width", "op": "glue.bead_width", "params": {
        "form": "straight", "widthRange": [p.lo, p.hi], "stationStep": p.step, "searchHalf": p.half,
        "window": 30.0, "mergeGap": 8.0, "contrastMin": 16.0,
        "presentRatio": 0.5, "centerRatio": 0.3, "contrastRatio": 0.3
    }})
}

fn graph(path: Value, path_image: bool, edge: Option<&str>, p: &Px, mut outputs: Value) -> String {
    let mut nodes = vec![
        json!({"id": "n_load", "op": "io.load_image", "params": {"source": "inputs"}}),
        path,
        width_node(p),
    ];
    let mut edges = vec![
        json!({"id": "e_image", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_width", "port": "image"}}),
        json!({"id": "e_path", "from": {"node": "n_path", "port": "path"}, "to": {"node": "n_width", "port": "path"}}),
    ];
    if path_image {
        edges.push(json!({"id": "e_pimage", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_path", "port": "image"}}));
    }
    outputs["stations"] = json!({"node": "n_width", "port": "bead.stations"});
    if let Some(polarity) = edge {
        nodes.push(
            json!({"id": "n_edge", "op": "glue.edge_distance", "params": {
                "searchLength": 200.0, "reference": "bead_center", "edgePolarity": polarity
            }}),
        );
        edges.push(json!({"id": "e_eimage", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_edge", "port": "image"}}));
        edges.push(json!({"id": "e_bead", "from": {"node": "n_width", "port": "bead"}, "to": {"node": "n_edge", "port": "bead"}}));
        outputs["edge"] = json!({"node": "n_edge", "port": "edge"});
    }
    json!({"schemaVersion": 1, "id": "01GLUESIGHTHEADEVALGRAPH0", "nodes": nodes, "edges": edges, "outputs": outputs}).to_string()
}

fn taught_graph(corridor: &[[f64; 2]], edge: Option<&str>, p: &Px) -> String {
    let path = json!({"id": "n_path", "op": "glue.taught_path", "params": {
        "points": serde_json::to_string(corridor).unwrap(),
        "zone": [0.0, 0.0], "tolerance": p.half - 0.5 * p.hi, "widthMax": p.hi.ceil(),
        "polarity": "dark", "sharpMin": 0.4
    }});
    graph(path, false, edge, p, json!({}))
}

fn bead_graph(
    anchor: [f64; 2],
    zone: [f64; 2],
    sector: [f64; 2],
    heading: Option<f64>,
    edge: Option<&str>,
    p: &Px,
) -> String {
    let mut params = json!({
        "nozzle": anchor, "zone": zone, "widthMax": p.hi.ceil(), "polarity": "dark", "sector": sector
    });
    if let Some(h) = heading {
        params["headingSource"] = json!("param");
        params["heading"] = json!(h);
        params["headingTol"] = json!(FALLBACK_TOL);
    }
    let path = json!({"id": "n_path", "op": "glue.bead_path", "params": params});
    graph(
        path,
        true,
        edge,
        p,
        json!({"pathInfo": {"node": "n_path", "port": "path.info"}}),
    )
}

fn read_edge(r: &Value, scale: f64, out: &mut Outcome) {
    let status = r["status"].as_array().cloned().unwrap_or_default();
    let searched = status
        .iter()
        .filter(|s| s.as_str() != Some("nobead"))
        .count();
    let ok: Vec<f64> = r["distancePx"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    out.edge_found = (searched > 0).then(|| ok.len() as f64 / searched as f64);
    out.edge_mm = (!ok.is_empty()).then(|| ok.iter().sum::<f64>() / ok.len() as f64 * scale);
}

fn line_offset(stations: &Value) -> Option<f64> {
    let present = stations["present"].as_array()?;
    let (lo, hi) = (stations["lo"].as_array()?, stations["hi"].as_array()?);
    let d: Vec<f64> = present
        .iter()
        .zip(lo.iter().zip(hi))
        .filter(|(ok, _)| ok.as_bool() == Some(true))
        .filter_map(|(_, (l, h))| Some(((l.as_f64()? + h.as_f64()?) * 0.5).abs()))
        .collect();
    (!d.is_empty()).then(|| d.iter().sum::<f64>() / d.len() as f64)
}

fn present_ratio(stations: &Value) -> Option<f64> {
    let present = stations["present"].as_array()?;
    if present.is_empty() {
        return Some(0.0);
    }
    Some(present.iter().filter(|v| v.as_bool() == Some(true)).count() as f64 / present.len() as f64)
}

fn run_variant(
    engine: &Engine,
    image: &FrameImage,
    variant: &Variant,
    p: &Px,
    scale: f64,
) -> Outcome {
    let started = Instant::now();
    let mut out = Outcome::default();
    let run = |graph: &str| engine.run(graph, "heading-eval", "", image, &json!({}));
    match variant {
        Variant::Taught {
            label,
            corridor,
            edge,
        } => {
            out.label = label.clone();
            match run(&taught_graph(corridor, *edge, p)) {
                Ok(r) if r.status() == "ok" => {
                    out.present = r.record("stations").and_then(present_ratio);
                    out.offset_mm = r
                        .record("stations")
                        .and_then(line_offset)
                        .map(|v| v * scale);
                    if let Some(e) = r.record("edge") {
                        read_edge(e, scale, &mut out);
                    }
                }
                Ok(r) => out.error = Some(r.failure()),
                Err(e) => out.error = Some(e),
            }
        }
        Variant::Bead {
            label,
            anchor,
            zone,
            sector,
            fallback,
            edge,
        } => {
            out.label = label.clone();
            let window = [zone.s0 / scale, (zone.s0 + zone.len) / scale];
            let attempt = |heading: Option<f64>, out: &mut Outcome| match run(&bead_graph(
                *anchor, window, *sector, heading, *edge, p,
            )) {
                Ok(r) if r.status() == "ok" => {
                    let info = r.record("pathInfo").cloned().unwrap_or(Value::Null);
                    out.ok = info["ok"].as_bool();
                    out.heading = info["heading"].as_f64();
                    out.beadlike = info["beadlike"].as_bool();
                    out.present = r.record("stations").and_then(present_ratio);
                    if let Some(e) = r.record("edge") {
                        read_edge(e, scale, out);
                    }
                    out.error = None;
                }
                Ok(r) => out.error = Some(r.failure()),
                Err(e) => out.error = Some(e),
            };
            attempt(None, &mut out);
            if out.ok == Some(false) {
                if let Some(h) = fallback {
                    out.fallback = true;
                    attempt(Some(*h), &mut out);
                }
            }
        }
    }
    out.ms = started.elapsed().as_secs_f64() * 1e3;
    out
}

fn shift_mm(a: &FrameImage, b: &FrameImage, scale: f64) -> f64 {
    let (a, b) = (downsample(a, SHIFT_FACTOR), downsample(b, SHIFT_FACTOR));
    let (w, h) = (a.width as i32, a.height as i32);
    let margin = SHIFT_MAX;
    let mut best = (f64::INFINITY, 0, 0);
    for dy in -SHIFT_MAX..=SHIFT_MAX {
        for dx in -SHIFT_MAX..=SHIFT_MAX {
            let mut sum = 0u64;
            let mut n = 0u64;
            for y in (margin..h - margin).step_by(2) {
                for x in (margin..w - margin).step_by(2) {
                    let pa = a.pixels[(y * w + x) as usize];
                    let pb = b.pixels[((y + dy) * w + x + dx) as usize];
                    sum += u64::from(pa.abs_diff(pb));
                    n += 1;
                }
            }
            let mad = sum as f64 / n as f64;
            if mad < best.0 {
                best = (mad, dx, dy);
            }
        }
    }
    f64::from(best.1).hypot(f64::from(best.2)) * f64::from(SHIFT_FACTOR) * scale
}

fn refine_cut(
    prop: u64,
    total: u64,
    straight_first: bool,
    theta: &BTreeMap<u64, f64>,
    reference: f64,
) -> Option<u64> {
    let lo = prop.saturating_sub(REFINE_WINDOW).max(1);
    let hi = (prop + REFINE_WINDOW).min(total - 1);
    let mut best: Option<(i64, u64)> = None;
    for cut in lo..=hi {
        let mut score = 0i64;
        let mut seen = 0;
        for seq in lo..=hi + 1 {
            let Some(&t) = theta.get(&seq) else { continue };
            seen += 1;
            let in_straight = wrap(t - reference).abs() <= REFINE_TOL;
            let expect_straight = (seq <= cut) == straight_first;
            score += if in_straight == expect_straight {
                1
            } else {
                -1
            };
        }
        if seen == 0 {
            return None;
        }
        let better = match best {
            None => true,
            Some((s, c)) => score > s || (score == s && cut.abs_diff(prop) < c.abs_diff(prop)),
        };
        if better {
            best = Some((score, cut));
        }
    }
    best.map(|(_, cut)| cut)
}

struct Canvas {
    w: i64,
    h: i64,
    rgb: Vec<u8>,
}

impl Canvas {
    fn new(image: &FrameImage) -> Self {
        Self {
            w: i64::from(image.width),
            h: i64::from(image.height),
            rgb: image.pixels.iter().flat_map(|&g| [g, g, g]).collect(),
        }
    }

    fn dot(&mut self, x: f64, y: f64, r: i64, c: [u8; 3]) {
        let (cx, cy) = (x.round() as i64, y.round() as i64);
        for dy in -r..=r {
            for dx in -r..=r {
                let (px, py) = (cx + dx, cy + dy);
                if dx * dx + dy * dy <= r * r && px >= 0 && py >= 0 && px < self.w && py < self.h {
                    let i = ((py * self.w + px) * 3) as usize;
                    self.rgb[i..i + 3].copy_from_slice(&c);
                }
            }
        }
    }

    fn line(&mut self, a: [f64; 2], b: [f64; 2], r: i64, c: [u8; 3]) {
        let n = ((b[0] - a[0]).hypot(b[1] - a[1]).ceil() as usize).max(1);
        for i in 0..=n {
            let t = i as f64 / n as f64;
            self.dot(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, r, c);
        }
    }

    fn polyline(&mut self, pts: &[[f64; 2]], r: i64, c: [u8; 3]) {
        for w in pts.windows(2) {
            self.line(w[0], w[1], r, c);
        }
    }

    fn save(self, path: &Path) -> Result<(), String> {
        let img = image::RgbImage::from_raw(self.w as u32, self.h as u32, self.rgb)
            .ok_or("画布尺寸不符")?;
        let small = image::imageops::resize(
            &img,
            img.width() / 2,
            img.height() / 2,
            image::imageops::FilterType::Triangle,
        );
        small.save(path).map_err(|e| e.to_string())
    }
}

const GREEN: [u8; 3] = [40, 220, 60];
const YELLOW: [u8; 3] = [255, 220, 0];
const RED: [u8; 3] = [255, 40, 40];
const ORANGE: [u8; 3] = [255, 140, 0];
const BLUE: [u8; 3] = [60, 120, 255];
const CYAN: [u8; 3] = [0, 230, 230];

fn points(v: &Value) -> Vec<[f64; 2]> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| Some([p[0].as_f64()?, p[1].as_f64()?]))
                .collect()
        })
        .unwrap_or_default()
}

fn draw_edges(canvas: &mut Canvas, stations: &Value, c: [u8; 3]) {
    let empty = Vec::new();
    let present = stations["present"].as_array().unwrap_or(&empty);
    for (i, ok) in present.iter().enumerate() {
        if ok.as_bool() != Some(true) {
            continue;
        }
        let (center, normal) = (&stations["center"][i], &stations["normal"][i]);
        let (Some(cx), Some(cy), Some(nx), Some(ny)) = (
            center[0].as_f64(),
            center[1].as_f64(),
            normal[0].as_f64(),
            normal[1].as_f64(),
        ) else {
            continue;
        };
        for key in ["lo", "hi"] {
            if let Some(d) = stations[key][i].as_f64() {
                canvas.dot(cx + nx * d, cy + ny * d, 2, c);
            }
        }
    }
}

/// 一轮的叠图：每帧、真实所在段的每台检测相机一张（半分辨率 PNG）。
/// 绿 = 本段示教走廊，黄 = 示教走廊测到的胶边；红点 = bead_path 锚点（本段走廊起点），橙 = 扇区两条边与窗口内外弧，
/// 蓝 = bead_path 拟合的胶路，青 = 它测到的胶边。
#[allow(clippy::too_many_arguments)]
fn dump_overlays(
    engine: &Engine,
    capture: &Path,
    out: &Path,
    truth: &Truth,
    segs: &[Seg],
    cams: &[Cam],
    predicted: &dyn Fn(u64) -> usize,
    zone: Zone,
    p: &Px,
) -> Result<usize, String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut written = 0;
    for seq in 1..=truth.total {
        let s = truth.segment_at(seq);
        let seg = &segs[s];
        for cam in cams
            .iter()
            .filter(|c| seg.cameras.contains(&c.id) && c.corridors.contains_key(&seg.index))
        {
            let image = load_image(
                &capture
                    .join(&truth.round)
                    .join(&cam.id)
                    .join(format!("{seq:06}.pgm")),
            )?;
            let mut canvas = Canvas::new(&image);
            let corridor = &cam.corridors[&seg.index];
            canvas.polyline(corridor, 1, GREEN);
            if let Ok(r) = engine.run(
                &taught_graph(corridor, None, p),
                "heading-dump",
                "",
                &image,
                &json!({}),
            ) {
                if let Some(st) = r.record("stations") {
                    draw_edges(&mut canvas, st, YELLOW);
                }
            }
            let anchor = cam.start(seg.index).unwrap();
            let sector = cam.sector();
            let window = [
                zone.s0 / cam.mm_per_px,
                (zone.s0 + zone.len) / cam.mm_per_px,
            ];
            let run_bead = |heading: Option<f64>| {
                let mut params = json!({"nozzle": anchor, "zone": window, "widthMax": p.hi.ceil(), "polarity": "dark", "sector": sector});
                if let Some(h) = heading {
                    params["headingSource"] = json!("param");
                    params["heading"] = json!(h);
                    params["headingTol"] = json!(FALLBACK_TOL);
                }
                let path = json!({"id": "n_path", "op": "glue.bead_path", "params": params});
                let g = graph(
                    path,
                    true,
                    None,
                    p,
                    json!({"pathInfo": {"node": "n_path", "port": "path.info"}, "pathLine": {"node": "n_path", "port": "path.line"}}),
                );
                engine.run(&g, "heading-dump", "", &image, &json!({}))
            };
            let mut result = run_bead(None)?;
            let mut tag = "ok";
            if result.record("pathInfo").and_then(|i| i["ok"].as_bool()) == Some(false) {
                tag = "fallback";
                if let Some(h) = cam
                    .heading(segs[predicted(seq)].index)
                    .or_else(|| cam.heading(seg.index))
                {
                    result = run_bead(Some(h))?;
                    if result.record("pathInfo").and_then(|i| i["ok"].as_bool()) == Some(false) {
                        tag = "nobead";
                    }
                }
            }
            let (a0, a1) = (sector[0], sector[1]);
            for a in [a0, a1] {
                let d = [a.to_radians().cos(), a.to_radians().sin()];
                canvas.line(
                    [anchor[0] + d[0] * window[0], anchor[1] + d[1] * window[0]],
                    [anchor[0] + d[0] * window[1], anchor[1] + d[1] * window[1]],
                    1,
                    ORANGE,
                );
            }
            for r in window {
                let n = ((a1 - a0) / 2.0).ceil().max(2.0) as usize;
                let arc: Vec<[f64; 2]> = (0..=n)
                    .map(|i| {
                        let a = (a0 + (a1 - a0) * i as f64 / n as f64).to_radians();
                        [anchor[0] + a.cos() * r, anchor[1] + a.sin() * r]
                    })
                    .collect();
                canvas.polyline(&arc, 1, ORANGE);
            }
            if let Some(line) = result.record("pathLine") {
                canvas.polyline(&points(&line["points"]), 2, BLUE);
            }
            if let Some(st) = result.record("stations") {
                draw_edges(&mut canvas, st, CYAN);
            }
            canvas.dot(anchor[0], anchor[1], 7, RED);
            let heading = result
                .record("pathInfo")
                .and_then(|i| i["heading"].as_f64())
                .unwrap_or(f64::NAN);
            let kind = if seg.straight { "直线" } else { "弯道" };
            let name = format!(
                "{seq:02}_{}_s{}{kind}_{tag}_θ{heading:.0}_θ0{:.0}.png",
                cam.id,
                seg.index,
                cam.heading(seg.index).unwrap_or(f64::NAN)
            );
            canvas.save(&out.join(name))?;
            written += 1;
        }
    }
    Ok(written)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (project, candidate, out_dir) = match args.as_slice() {
        [p, c] => (
            PathBuf::from(p),
            c.clone(),
            std::env::temp_dir().join("heading-eval"),
        ),
        [p, c, o] => (PathBuf::from(p), c.clone(), PathBuf::from(o)),
        _ => {
            return Err(
                "用法：heading_eval <项目目录> <候选编号> [输出目录]（需设 LYFLOW_CORE_DLL）"
                    .into(),
            )
        }
    };
    let dll = std::env::var("LYFLOW_CORE_DLL")
        .map_err(|_| "需设 LYFLOW_CORE_DLL 指向 lyflow_core.dll".to_string())?;
    let engine = Arc::new(Engine::load(Path::new(&dll))?);
    let zones: Vec<Zone> = std::env::var("HEADING_ZONES")
        .unwrap_or_else(|_| "2:10,5:15".into())
        .split(',')
        .map(|z| {
            let (a, b) = z
                .split_once(':')
                .ok_or(format!("窗口格式 s0:L，收到 {z}"))?;
            Ok(Zone {
                s0: a.parse().map_err(|_| z.to_string())?,
                len: b.parse().map_err(|_| z.to_string())?,
            })
        })
        .collect::<Result<_, String>>()?;

    let ws_path = project
        .join("workspaces")
        .join(&candidate)
        .join("workspace.json");
    let workspace: Value = serde_json::from_str(
        &std::fs::read_to_string(&ws_path)
            .map_err(|e| format!("读 {} 失败：{e}", ws_path.display()))?,
    )
    .map_err(|e| e.to_string())?;
    let doc: RecipeDoc = serde_json::from_value(workspace["doc"].clone())
        .map_err(|e| format!("配方文档无法解析：{e}"))?;
    let plan_cameras = doc.plan_cameras();
    let segs: Vec<Seg> = doc
        .segments
        .iter()
        .map(|s| Seg {
            index: s.index,
            straight: s.kind == SegmentKind::Straight,
            cameras: if s.cameras.is_empty() {
                plan_cameras.clone()
            } else {
                s.cameras.clone()
            },
            n_exp: s.expected_frames,
            tolerance: s.tolerance,
        })
        .collect();
    let cams: Vec<Cam> = doc
        .inspect
        .iter()
        .map(|c| Cam {
            id: c.camera.clone(),
            mm_per_px: c.mm_per_px.unwrap_or(0.11),
            corridors: c
                .segments
                .iter()
                .filter(|s| s.corridor.len() >= 2)
                .map(|s| {
                    (
                        s.segment,
                        s.corridor
                            .iter()
                            .map(|p| [f64::from(p[0]), f64::from(p[1])])
                            .collect(),
                    )
                })
                .collect(),
        })
        .collect();
    let mut truths: Vec<Truth> = workspace["captureReview"]["rounds"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|(_, r)| r["status"] == "confirmed")
        .filter_map(|(round, r)| {
            let cuts: Vec<u64> = r["cuts"]
                .as_array()?
                .iter()
                .filter_map(Value::as_u64)
                .collect();
            (cuts.len() + 1 == segs.len()).then(|| Truth {
                round: round.clone(),
                total: r["groupCount"].as_u64().unwrap_or(0),
                cuts,
            })
        })
        .collect();
    truths.sort_by(|a, b| a.round.cmp(&b.round));
    let n_exp: Vec<u32> = segs.iter().map(|s| s.n_exp.unwrap_or(0)).collect();
    let predicted = |seq: u64| -> usize {
        let mut c = 0u64;
        for (k, &n) in n_exp.iter().enumerate() {
            c += u64::from(n);
            if seq <= c {
                return k;
            }
        }
        n_exp.len() - 1
    };
    let cam_idx = |id: &str| cams.iter().position(|c| c.id == id).unwrap();
    let scale = cams[0].mm_per_px;
    let p = px(&doc, scale);
    println!(
        "lyFlow {}；候选 {candidate}：{} 段，已确认轮次 {}，像素当量 {scale} mm/px，窗口 {:?}",
        engine.version,
        segs.len(),
        truths.len(),
        zones
            .iter()
            .map(|z| format!("[{}, {}] mm", z.s0, z.s0 + z.len))
            .collect::<Vec<_>>()
    );
    for c in cams.iter().filter(|c| !c.corridors.is_empty()) {
        let starts: Vec<String> = c
            .corridors
            .iter()
            .map(|(s, cor)| {
                format!(
                    "s{s}({:.0},{:.0}) {:.0}°",
                    cor[0][0],
                    cor[0][1],
                    c.heading(*s).unwrap()
                )
            })
            .collect();
        let m = c.median_start();
        println!(
            "  {}：走廊起点与方向 {}；中位锚点 ({:.0},{:.0})；扇区 {:?}",
            c.id,
            starts.join("，"),
            m[0],
            m[1],
            c.sector().map(|v| v.round())
        );
    }

    if let Ok(which) = std::env::var("HEADING_DUMP") {
        let round = truths
            .iter()
            .find(|t| t.round.contains(&which))
            .ok_or(format!("没有轮次包含 {which}"))?;
        let dir = out_dir.join("overlays").join(&round.round);
        let n = dump_overlays(
            &engine,
            &project.join("recipe-capture"),
            &dir,
            round,
            &segs,
            &cams,
            &predicted,
            zones[0],
            &p,
        )?;
        println!("叠图 {n} 张：{}", dir.display());
        return Ok(());
    }
    let mut jobs: Vec<Job> = Vec::new();
    for (r, truth) in truths.iter().enumerate() {
        for seq in 1..=truth.total {
            let s = truth.segment_at(seq);
            let ps = predicted(seq);
            for (ci, cam) in cams.iter().enumerate() {
                let detects =
                    segs[s].cameras.contains(&cam.id) && cam.corridors.contains_key(&segs[s].index);
                let mut variants = Vec::new();
                if detects {
                    variants.push(Variant::Taught {
                        label: "走廊·真实段".into(),
                        corridor: cam.corridors[&segs[s].index].clone(),
                        edge: None,
                    });
                    if segs[ps].cameras.contains(&cam.id) {
                        if let Some(c) = cam.corridors.get(&segs[ps].index) {
                            variants.push(Variant::Taught {
                                label: "走廊·预测段".into(),
                                corridor: c.clone(),
                                edge: None,
                            });
                        }
                    }
                    let fallback = cam
                        .heading(segs[ps].index)
                        .or_else(|| cam.heading(segs[s].index));
                    for (zi, zone) in zones.iter().enumerate() {
                        variants.push(Variant::Bead {
                            label: format!("定向·段起点·窗{zi}"),
                            anchor: cam.start(segs[s].index).unwrap(),
                            zone: *zone,
                            sector: cam.sector(),
                            fallback,
                            edge: None,
                        });
                        variants.push(Variant::Bead {
                            label: format!("定向·中位锚点·窗{zi}"),
                            anchor: cam.median_start(),
                            zone: *zone,
                            sector: cam.sector(),
                            fallback,
                            edge: None,
                        });
                    }
                    if segs[s].straight {
                        for polarity in ["bright_to_dark", "dark_to_bright"] {
                            variants.push(Variant::Taught {
                                label: format!("找边·走廊·{polarity}"),
                                corridor: cam.corridors[&segs[s].index].clone(),
                                edge: Some(polarity),
                            });
                            variants.push(Variant::Bead {
                                label: format!("找边·定向·{polarity}"),
                                anchor: cam.start(segs[s].index).unwrap(),
                                zone: zones[0],
                                sector: cam.sector(),
                                fallback,
                                edge: Some(polarity),
                            });
                        }
                    }
                }
                for (k, seg) in segs.iter().enumerate().filter(|(_, g)| g.straight) {
                    if !seg.cameras.contains(&cam.id) || !cam.corridors.contains_key(&seg.index) {
                        continue;
                    }
                    let near = truth.cuts.iter().enumerate().any(|(b, &cut)| {
                        (b + 1 == k || b == k) && cut.abs_diff(seq) <= REFINE_WINDOW + 3
                    });
                    if near && s != k {
                        let h = cam.heading(seg.index).unwrap();
                        variants.push(Variant::Bead {
                            label: format!("细化·s{}", seg.index),
                            anchor: cam.start(seg.index).unwrap(),
                            zone: zones[0],
                            sector: [h - 60.0, h + 60.0],
                            fallback: None,
                            edge: None,
                        });
                    }
                }
                if !variants.is_empty() {
                    jobs.push(Job {
                        round: r,
                        camera: ci,
                        seq,
                        variants,
                    });
                }
            }
        }
    }
    let total_runs: usize = jobs.iter().map(|j| j.variants.len()).sum();
    println!(
        "共 {} 个（轮次, 相机, 帧），{total_runs} 次运行",
        jobs.len()
    );

    let capture = project.join("recipe-capture");
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Measured>> = Mutex::new(Vec::new());
    let started = Instant::now();
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(6);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(job) = jobs.get(i) else { break };
                let path = capture
                    .join(&truths[job.round].round)
                    .join(&cams[job.camera].id)
                    .join(format!("{:06}.pgm", job.seq));
                let outcomes = match load_image(&path) {
                    Ok(image) => job
                        .variants
                        .iter()
                        .map(|v| run_variant(&engine, &image, v, &p, cams[job.camera].mm_per_px))
                        .collect(),
                    Err(e) => job
                        .variants
                        .iter()
                        .map(|_| Outcome {
                            error: Some(e.clone()),
                            ..Default::default()
                        })
                        .collect(),
                };
                results.lock().unwrap().push(Measured {
                    round: job.round,
                    camera: job.camera,
                    seq: job.seq,
                    outcomes,
                });
                if i % 200 == 0 {
                    eprintln!("  …{i}/{}", jobs.len());
                }
            });
        }
    });
    let results = results.into_inner().unwrap();
    println!(
        "运行耗时 {:.1} s（{threads} 线程）",
        started.elapsed().as_secs_f64()
    );

    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let csv_path = out_dir.join("runs.csv");
    let mut csv =
        std::io::BufWriter::new(std::fs::File::create(&csv_path).map_err(|e| e.to_string())?);
    writeln!(csv, "round,camera,seq,true_segment,pred_segment,variant,present,heading,ok,fallback,beadlike,edge_found,edge_mm,ms,error").unwrap();
    for m in &results {
        let truth = &truths[m.round];
        for o in &m.outcomes {
            let f = |v: Option<f64>| v.map_or(String::new(), |x| format!("{x:.3}"));
            let b = |v: Option<bool>| v.map_or(String::new(), |x| x.to_string());
            writeln!(
                csv,
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{:.1},{}",
                truth.round,
                cams[m.camera].id,
                m.seq,
                segs[truth.segment_at(m.seq)].index,
                segs[predicted(m.seq)].index,
                o.label,
                f(o.present),
                f(o.heading),
                b(o.ok),
                o.fallback,
                b(o.beadlike),
                f(o.edge_found),
                f(o.edge_mm),
                o.ms,
                o.error.as_deref().unwrap_or("").replace(',', "；")
            )
            .unwrap();
        }
    }
    drop(csv);
    println!("逐次结果：{}", csv_path.display());

    let taught_present: BTreeMap<(usize, usize, u64), Option<f64>> = results
        .iter()
        .flat_map(|m| {
            m.outcomes
                .iter()
                .filter(|o| o.label == "走廊·真实段")
                .map(move |o| {
                    (
                        (m.round, m.camera, m.seq),
                        o.present.filter(|_| o.error.is_none()),
                    )
                })
        })
        .collect();
    let detecting = |k: usize| -> Vec<usize> {
        cams.iter()
            .enumerate()
            .filter(|(_, c)| {
                segs[k].cameras.contains(&c.id) && c.corridors.contains_key(&segs[k].index)
            })
            .map(|(ci, _)| ci)
            .collect()
    };
    let ends: Vec<[Result<u32, String>; 2]> = truths
        .iter()
        .enumerate()
        .map(|(r, truth)| {
            let form = |seq: u64| {
                let mut form = GroupForm::Formed;
                for ci in detecting(truth.segment_at(seq)) {
                    match taught_present.get(&(r, ci, seq)).copied().flatten() {
                        None => return GroupForm::Unknown,
                        Some(v) if v < PRESENT_OK => form = GroupForm::Unformed,
                        Some(_) => {}
                    }
                }
                form
            };
            let lead = |k: usize, seqs: &mut dyn Iterator<Item = u64>| {
                if detecting(k).is_empty() {
                    return Err(format!("s{} 没有带走廊的检测相机", segs[k].index));
                }
                unformed_lead(seqs.map(form)).map_err(|e| format!("{e:?}"))
            };
            let head_end = truth.cuts.first().copied().unwrap_or(truth.total);
            let tail_start = truth.cuts.last().copied().unwrap_or(0) + 1;
            [
                lead(0, &mut (1..=head_end)),
                lead(segs.len() - 1, &mut (tail_start..=truth.total).rev()),
            ]
        })
        .collect();
    let in_ramp = |m: &Measured| {
        let [head, tail] = &ends[m.round];
        head.as_ref().is_ok_and(|&h| m.seq <= u64::from(h))
            || tail
                .as_ref()
                .is_ok_and(|&t| m.seq + u64::from(t) > truths[m.round].total)
    };
    let kept: Vec<&Measured> = results.iter().filter(|m| !in_ramp(m)).collect();
    println!(
        "\n〇、起涂、收尾帧组（D-SB-50：第 1 段正向、末段倒序，“走廊·真实段”各检测相机有胶测站都 ≥ {:.0}% 为成形，\
         数第一处连续 {ONSET_RUN} 个成形帧组之前的帧组数；Unknown(n) = 成形前第 n 个帧组有相机没有结果，NotFound = 本段没有连续成形）",
        PRESENT_OK * 100.0
    );
    let show = |v: &Result<u32, String>| match v {
        Ok(n) => n.to_string(),
        Err(e) => format!("不排除（{e}）"),
    };
    for (truth, [head, tail]) in truths.iter().zip(&ends) {
        println!(
            "  {:<26} 起涂 {}  收尾 {}",
            truth.round,
            show(head),
            show(tail)
        );
    }
    for (k, name) in ["起涂", "收尾"].into_iter().enumerate() {
        let mut dist: BTreeMap<u32, usize> = BTreeMap::new();
        for e in &ends {
            if let Ok(v) = &e[k] {
                *dist.entry(*v).or_default() += 1;
            }
        }
        match (dist.keys().next(), dist.keys().next_back()) {
            (Some(min), Some(max)) => println!(
                "  {name}：{}/{} 轮有值，最小 {min}，最大 {max}；分布 {dist:?}",
                dist.values().sum::<usize>(),
                truths.len()
            ),
            _ => println!("  {name}：没有轮次有值"),
        }
    }
    println!(
        "  以下各节（五除外）排除其中 {} 个（轮次, 相机, 帧）",
        results.len() - kept.len()
    );

    let place = |m: &Measured| -> &'static str {
        let truth = &truths[m.round];
        let s = truth.segment_at(m.seq);
        if truth
            .cuts
            .iter()
            .any(|&cut| m.seq + 1 >= cut && m.seq <= cut + 2)
        {
            "边界±2帧"
        } else if segs[s].straight {
            "直线段"
        } else {
            "弯道"
        }
    };
    let mut table: BTreeMap<(String, &'static str), (usize, usize, usize)> = BTreeMap::new();
    let mut times: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut fallback: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in &kept {
        let where_ = place(m);
        let detect_labels: Vec<&str> = m
            .outcomes
            .iter()
            .filter(|o| !o.label.starts_with("细化") && !o.label.starts_with("找边"))
            .map(|o| o.label.as_str())
            .collect();
        if detect_labels.is_empty() {
            continue;
        }
        let has_pred = detect_labels.contains(&"走廊·预测段");
        for o in m.outcomes.iter().filter(|o| !o.label.starts_with("细化")) {
            times
                .entry(o.label.split('·').next().unwrap().to_string())
                .or_default()
                .push(o.ms);
            if let Some(e) = &o.error {
                errors.entry(o.label.clone()).or_default().push(e.clone());
            }
            if o.label.starts_with("找边") {
                continue;
            }
            let e = table.entry((o.label.clone(), where_)).or_default();
            e.0 += 1;
            if o.present.is_some_and(|r| r >= PRESENT_OK) {
                e.1 += 1;
            }
            if o.label.starts_with("定向") {
                let f = fallback.entry(o.label.clone()).or_default();
                f.0 += 1;
                f.1 += usize::from(o.fallback);
            }
        }
        if !has_pred {
            let e = table.entry(("走廊·预测段".into(), where_)).or_default();
            e.0 += 1;
            e.2 += 1;
        }
    }
    println!(
        "\n一、测到胶的比例（有胶测站 ≥ {:.0}%；分母 = 真实所在段的检测相机帧）",
        PRESENT_OK * 100.0
    );
    println!(
        "{:<20} {:>16} {:>16} {:>16}",
        "测法", "直线段", "弯道", "边界±2帧"
    );
    let labels: Vec<String> = {
        let mut l: Vec<String> = table.keys().map(|(l, _)| l.clone()).collect();
        l.dedup();
        l
    };
    for label in &labels {
        let cell = |w: &'static str| {
            table
                .get(&(label.clone(), w))
                .map_or("-".into(), |&(n, ok, skip)| {
                    format!(
                        "{ok}/{n} {:.0}%{}",
                        100.0 * ok as f64 / n.max(1) as f64,
                        if skip > 0 {
                            format!("（{skip}不测）")
                        } else {
                            String::new()
                        }
                    )
                })
        };
        println!(
            "{label:<20} {:>16} {:>16} {:>16}",
            cell("直线段"),
            cell("弯道"),
            cell("边界±2帧")
        );
    }
    for (label, (n, f)) in &fallback {
        println!("  {label}：定向失败回退 {f}/{n}");
    }
    for (label, errs) in &errors {
        println!("  {label}：运行错误 {} 次，例如 {}", errs.len(), errs[0]);
    }

    println!("\n二、定向是否偏离示教方向（直线段核心帧，与该段走廊方向相差 > 10° 记为偏离，可能锁到别的暗带）");
    let mut dev: BTreeMap<String, (usize, usize, Vec<f64>)> = BTreeMap::new();
    for m in &kept {
        let truth = &truths[m.round];
        let s = truth.segment_at(m.seq);
        if !segs[s].straight || place(m) != "直线段" {
            continue;
        }
        let Some(h0) = cams[m.camera].heading(segs[s].index) else {
            continue;
        };
        for o in m
            .outcomes
            .iter()
            .filter(|o| o.label.starts_with("定向") && !o.fallback)
        {
            let Some(h) = o.heading else { continue };
            let d = wrap(h - h0).abs();
            let e = dev.entry(o.label.clone()).or_default();
            e.0 += 1;
            e.1 += usize::from(d > 10.0);
            e.2.push(d);
        }
    }
    for (label, (n, off, mut d)) in dev {
        println!(
            "  {label:<20} 偏离 {off}/{n}，|θ − θ₀| 中位 {:.1}°、p90 {:.1}°",
            median(&mut d.clone()),
            percentile(&mut d, 0.9)
        );
    }

    println!("\n三、单次运行耗时（ms，含读图后到出结果；定向含回退重跑）");
    for (label, mut t) in times {
        println!(
            "  {label:<8} n={:<6} p50 {:.1}  p95 {:.1}  max {:.1}",
            t.len(),
            percentile(&mut t, 0.5),
            percentile(&mut t, 0.95),
            percentile(&mut t, 1.0)
        );
    }

    println!("\n四、方向序列细化边界（直线段检测相机，比例边界 ±{REFINE_WINDOW} 帧内找方向开始 / 停止偏离 θ₀ ± {REFINE_TOL}° 的帧；留一标定 N_exp）");
    let mut theta: BTreeMap<(usize, u32, u64), Vec<f64>> = BTreeMap::new();
    for m in &kept {
        for o in &m.outcomes {
            let seg_index = if let Some(s) = o.label.strip_prefix("细化·s") {
                s.parse::<u32>().ok()
            } else if o.label == "定向·段起点·窗0" {
                let s = truths[m.round].segment_at(m.seq);
                segs[s].straight.then_some(segs[s].index)
            } else {
                None
            };
            if let (Some(si), Some(h), Some(true)) = (seg_index, o.heading, o.ok) {
                let h0 = cams[m.camera].heading(si).unwrap();
                theta
                    .entry((m.round, si, m.seq))
                    .or_default()
                    .push(h0 + wrap(h - h0));
            }
        }
    }
    let mut prop_err: BTreeMap<u64, usize> = BTreeMap::new();
    let mut refined_err: BTreeMap<u64, usize> = BTreeMap::new();
    for (held, truth) in truths.iter().enumerate() {
        let mut nexp = Vec::new();
        for k in 0..segs.len() {
            let counts: Vec<u32> = truths
                .iter()
                .enumerate()
                .filter(|&(i, _)| i != held)
                .map(|(_, t)| t.counts()[k])
                .collect();
            nexp.push(u64::from(
                calibrate_counts(&counts, 0, segs[k].tolerance)
                    .map_err(|e| format!("{e:?}"))?
                    .n_exp,
            ));
        }
        let sum: u64 = nexp.iter().sum();
        let mut c = 0;
        let mut line = Vec::new();
        for b in 0..truth.cuts.len() {
            c += nexp[b];
            let prop = ((truth.total * c) as f64 / sum as f64).round() as u64;
            let (k, straight_first) = if segs[b].straight {
                (b, true)
            } else {
                (b + 1, false)
            };
            let si = segs[k].index;
            let reference = segs[k]
                .cameras
                .iter()
                .filter_map(|id| cams[cam_idx(id)].heading(si))
                .next()
                .unwrap_or(0.0);
            let series: BTreeMap<u64, f64> = theta
                .range((held, si, 0)..=(held, si, u64::MAX))
                .map(|(&(_, _, seq), v)| {
                    let mut v = v.clone();
                    (seq, median(&mut v))
                })
                .collect();
            let refined =
                refine_cut(prop, truth.total, straight_first, &series, reference).unwrap_or(prop);
            let (pe, re) = (
                prop.abs_diff(truth.cuts[b]),
                refined.abs_diff(truth.cuts[b]),
            );
            *prop_err.entry(pe).or_default() += 1;
            *refined_err.entry(re).or_default() += 1;
            line.push(format!("{}→比例{prop}/细化{refined}", truth.cuts[b]));
        }
        println!("  {:<26} {}", truth.round, line.join("  "));
    }
    let summarize = |name: &str, e: &BTreeMap<u64, usize>| {
        let n: usize = e.values().sum();
        let within = |l: u64| {
            e.iter()
                .filter(|(&k, _)| k <= l)
                .map(|(_, &v)| v)
                .sum::<usize>()
        };
        println!(
            "  {name}：{n} 个边界，0 帧 {}，≤1 帧 {}（{:.0}%），最大 {} 帧；分布 {e:?}",
            within(0),
            within(1),
            100.0 * within(1) as f64 / n.max(1) as f64,
            e.keys().max().unwrap_or(&0)
        );
    };
    summarize("比例边界", &prop_err);
    summarize("方向细化", &refined_err);

    println!("\n五、每帧后移量（相邻两帧图像位移，{SHIFT_FACTOR}× 下采样整像素搜索 ±{} px；同一段内、该段检测相机）", SHIFT_MAX * SHIFT_FACTOR as i32);
    let mut shifts: BTreeMap<u32, Vec<f64>> = BTreeMap::new();
    for truth in &truths {
        for seq in 1..truth.total {
            let s = truth.segment_at(seq);
            if truth.segment_at(seq + 1) != s {
                continue;
            }
            let Some(cam) = segs[s].cameras.first().map(|id| &cams[cam_idx(id)]) else {
                continue;
            };
            let load = |q: u64| {
                load_image(
                    &capture
                        .join(&truth.round)
                        .join(&cam.id)
                        .join(format!("{q:06}.pgm")),
                )
            };
            if let (Ok(a), Ok(b)) = (load(seq), load(seq + 1)) {
                shifts
                    .entry(segs[s].index)
                    .or_default()
                    .push(shift_mm(&a, &b, cam.mm_per_px));
            }
        }
    }
    let mut all = Vec::new();
    for (si, mut v) in shifts {
        all.extend(v.iter().copied());
        let kind = if segs
            .iter()
            .find(|g| g.index == si)
            .is_some_and(|g| g.straight)
        {
            "直线"
        } else {
            "弯道"
        };
        println!(
            "  s{si}（{kind}）n={:<4} 中位 {:.2} mm  p90 {:.2} mm  max {:.2} mm",
            v.len(),
            percentile(&mut v.clone(), 0.5),
            percentile(&mut v.clone(), 0.9),
            percentile(&mut v, 1.0)
        );
    }
    println!(
        "  全部：p90 {:.2} mm → 窗口长度 L 的下限建议取 p90 以上",
        percentile(&mut all, 0.9)
    );

    println!(
        "\n六、找零件边（glue.edge_distance，直线段检测相机，窗口 0，reference = bead_center）"
    );
    let mut edges: BTreeMap<(String, u32), (usize, usize, Vec<f64>)> = BTreeMap::new();
    let mut per_round: BTreeMap<(String, u32, usize), Vec<f64>> = BTreeMap::new();
    for m in &kept {
        let s = truths[m.round].segment_at(m.seq);
        for o in m.outcomes.iter().filter(|o| o.label.starts_with("找边")) {
            let e = edges.entry((o.label.clone(), segs[s].index)).or_default();
            e.0 += 1;
            if o.edge_found.is_some_and(|f| f >= 0.5) {
                e.1 += 1;
                if let Some(d) = o.edge_mm {
                    e.2.push(d);
                    per_round
                        .entry((o.label.clone(), segs[s].index, m.round))
                        .or_default()
                        .push(d);
                }
            }
        }
    }
    for ((label, si), (n, found, mut d)) in edges {
        let spreads: Vec<f64> = per_round
            .iter()
            .filter(|((l, s, _), _)| *l == label && *s == si)
            .map(|(_, v)| {
                let mean = v.iter().sum::<f64>() / v.len() as f64;
                (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
            })
            .collect();
        let mut spreads = spreads;
        println!(
            "  {label:<22} s{si}：找到边（≥半数测站）{found}/{n}，胶边距中位 {:.2} mm，轮内标准差中位 {:.2} mm",
            median(&mut d),
            median(&mut spreads)
        );
    }

    println!("\n七、胶条相对示教线的横向偏离（示教走廊·真实段，有胶测站的胶中心到示教线的平均法向距离；胶条贴着示教线（即贴着出胶点）时应接近 0）");
    let mut offsets: BTreeMap<u32, Vec<f64>> = BTreeMap::new();
    for m in &kept {
        let s = truths[m.round].segment_at(m.seq);
        for o in m.outcomes.iter().filter(|o| o.label == "走廊·真实段") {
            if let (Some(v), true) = (o.offset_mm, o.present.is_some_and(|r| r >= PRESENT_OK)) {
                offsets.entry(segs[s].index).or_default().push(v);
            }
        }
    }
    for (si, mut v) in offsets {
        println!(
            "  s{si} n={:<4} 中位 {:.2} mm  p90 {:.2} mm  max {:.2} mm",
            v.len(),
            percentile(&mut v.clone(), 0.5),
            percentile(&mut v.clone(), 0.9),
            percentile(&mut v, 1.0)
        );
    }
    Ok(())
}
