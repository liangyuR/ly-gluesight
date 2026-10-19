//! 示教提取评测：以基准测得的胶中心为参考，模拟操作员笔划（居中 / 偏一侧、不同笔刷）和矩形框选，
//! 走生产的 `extract`，统计成功、偏离参考和失败原因。

use serde_json::{json, Value};

use super::synth::Rng;
use super::{median, pct, r3, Ctx, ExtractCase, Fixture};
use crate::measure::ST_OK;
use crate::recipe::Polarity;
use crate::vision::{FrameImage, ShotMeasurement};
use crate::workspace::centerline::{extract, BrushGuide, Extraction, ExtractionError};

const RADII: [f32; 3] = [30.0, 40.0, 60.0];
const OFFSETS: [f32; 5] = [0.0, 8.0, -8.0, 15.0, -15.0];
const ROI_MARGINS: [f32; 2] = [15.0, 40.0];
const ROI_SECTION_PX: f32 = 200.0;
const PASS_MAX_PX: f64 = 8.0;
const PASS_P95_PX: f64 = 3.0;

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn length(p: &[[f32; 2]]) -> f32 {
    p.windows(2).map(|w| dist(w[0], w[1])).sum()
}

fn to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    dist(p, [a[0] + d[0] * t, a[1] + d[1] * t])
}

fn to_polyline(p: [f32; 2], line: &[[f32; 2]]) -> f32 {
    line.windows(2).map(|w| to_segment(p, w[0], w[1])).fold(f32::INFINITY, f32::min)
}

/// 到参考折线的距离；最近处落在参考两端之外（提取比参考长出的一截）时不计。
fn to_reference(p: [f32; 2], line: &[[f32; 2]]) -> Option<f32> {
    let n = line.len();
    let beyond = |a: [f32; 2], b: [f32; 2]| (p[0] - a[0]) * (b[0] - a[0]) + (p[1] - a[1]) * (b[1] - a[1]) < 0.0;
    let d = to_polyline(p, line);
    let at_start = (dist(p, line[0]) - d).abs() < 1e-3 && beyond(line[0], line[1]);
    let at_end = (dist(p, line[n - 1]) - d).abs() < 1e-3 && beyond(line[n - 1], line[n - 2]);
    (!at_start && !at_end).then_some(d)
}

fn densify(p: &[[f32; 2]], step: f32) -> Vec<[f32; 2]> {
    let mut out = vec![p[0]];
    for w in p.windows(2) {
        let n = (dist(w[0], w[1]) / step).ceil().max(1.0) as usize;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            out.push([w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t]);
        }
    }
    out
}

/// 按弧长每 step 取一点。
fn resample(p: &[[f32; 2]], step: f32) -> Vec<[f32; 2]> {
    let dense = densify(p, 0.5);
    let mut out = vec![dense[0]];
    let mut acc = 0.0;
    for w in dense.windows(2) {
        acc += dist(w[0], w[1]);
        if acc >= step { out.push(w[1]); acc = 0.0; }
    }
    if dist(*out.last().unwrap(), *dense.last().unwrap()) > 0.5 * step { out.push(*dense.last().unwrap()); }
    out
}

fn normals(p: &[[f32; 2]]) -> Vec<[f32; 2]> {
    (0..p.len()).map(|i| {
        let (a, b) = (p[i.saturating_sub(1)], p[(i + 1).min(p.len() - 1)]);
        let l = dist(a, b).max(1e-6);
        [-(b[1] - a[1]) / l, (b[0] - a[0]) / l]
    }).collect()
}

/// 参考胶中心：基准有胶站的胶中心，按站序。
pub(super) fn reference(m: &ShotMeasurement) -> Vec<[f32; 2]> {
    m.px.iter().zip(&m.st).filter(|(_, s)| **s == ST_OK).map(|(p, _)| *p).collect()
}

/// 模拟操作员沿胶刷的一笔：每 25 px 一个点，整体偏 offset，逐点再抖 ±4 px。
fn operator_stroke(reference: &[[f32; 2]], offset: f32, seed: u64) -> Vec<[f32; 2]> {
    let pts = resample(reference, 25.0);
    let n = normals(&pts);
    let mut rng = Rng(seed);
    pts.iter().zip(&n).map(|(p, n)| {
        let o = offset + (rng.unit() as f32 - 0.5) * 8.0;
        [p[0] + n[0] * o, p[1] + n[1] * o]
    }).collect()
}

fn stroke_roi(stroke: &[[f32; 2]], r: f32, img: &FrameImage) -> [u32; 4] {
    let (w, h) = (img.width as f32, img.height as f32);
    let x0 = (stroke.iter().map(|p| p[0]).fold(f32::MAX, f32::min) - r).max(0.0).floor();
    let y0 = (stroke.iter().map(|p| p[1]).fold(f32::MAX, f32::min) - r).max(0.0).floor();
    let x1 = (stroke.iter().map(|p| p[0]).fold(f32::MIN, f32::max) + r).min(w - 1.0).ceil();
    let y1 = (stroke.iter().map(|p| p[1]).fold(f32::MIN, f32::max) + r).min(h - 1.0).ceil();
    [x0 as u32, y0 as u32, (x1 - x0) as u32 + 1, (y1 - y0) as u32 + 1]
}

fn inside(stroke: &[[f32; 2]], img: &FrameImage) -> Vec<[f32; 2]> {
    stroke.iter().map(|p| [p[0].clamp(0.0, img.width as f32 - 1.0), p[1].clamp(0.0, img.height as f32 - 1.0)]).collect()
}

#[derive(Default, Clone)]
struct Tally {
    runs: usize,
    ok: usize,
    wrong: usize,
    failed: std::collections::BTreeMap<String, usize>,
    p95: Vec<f64>,
    details: Vec<String>,
    inputs: Vec<Value>,
}

impl Tally {
    fn json(&self, name: &str) -> Value {
        json!({"name": name, "runs": self.runs, "ok": self.ok, "wrong": self.wrong, "failed": self.failed,
            "devP95MedianPx": r3(median(self.p95.clone())), "devP95MaxPx": r3(self.p95.iter().copied().fold(f64::NAN, f64::max)),
            "details": self.details, "failedInputs": self.inputs})
    }
}

/// 一次提取的结论：Ok(偏离 P95) 通过；Err(原因) 失败或偏离参考。
fn grade(result: Result<Vec<[f32; 2]>, ExtractionError>, reference: &[[f32; 2]], expect_len: f32) -> Result<f64, String> {
    let path = match result {
        Ok(p) => p,
        Err(ExtractionError::Failed(f)) => return Err(format!("{}@{:.0},{:.0}", f.reason, f.point[0], f.point[1])),
        Err(ExtractionError::Invalid(m)) => return Err(format!("invalid:{m}")),
    };
    let dev: Vec<f64> = densify(&path, 2.0).iter().filter_map(|&p| to_reference(p, reference)).map(f64::from).collect();
    if dev.is_empty() { return Err("wrong(提取结果全在参考范围外)".into()); }
    let (p95, max) = (pct(dev.clone(), 95.0), dev.iter().copied().fold(0.0, f64::max));
    let cover = length(&path) / expect_len.max(1.0);
    if max > PASS_MAX_PX || p95 > PASS_P95_PX || cover < 0.7 {
        return Err(format!("wrong(p95 {p95:.1}px max {max:.1}px 长度 {:.0}%)", cover * 100.0));
    }
    Ok(p95)
}

fn record(t: &mut Tally, shot: &str, label: &str, input: Value, g: Result<f64, String>) {
    t.runs += 1;
    match g {
        Ok(p95) => { t.ok += 1; t.p95.push(p95); }
        Err(e) => {
            if e.starts_with("wrong") { t.wrong += 1; } else { *t.failed.entry(e.split('@').next().unwrap().to_string()).or_default() += 1; }
            t.details.push(format!("{shot} {label}：{e}"));
            t.inputs.push(json!({"shot": shot, "label": label, "input": input}));
        }
    }
}

pub(super) fn evaluate(ctx: &Ctx, base: &[Result<ShotMeasurement, String>], fx: &Fixture) -> Value {
    let spacing = fx.point_spacing_px;
    let refs: Vec<Option<Vec<[f32; 2]>>> = base.iter().map(|m| m.as_ref().ok().filter(|m| m.coverage >= 0.9).map(reference)).collect();
    let mut brush = Vec::new();
    for &r in &RADII {
        for &o in &OFFSETS {
            let mut t = Tally::default();
            for (k, s) in ctx.samples.iter().enumerate() {
                let Some(reference) = &refs[k] else { continue };
                let stroke = inside(&operator_stroke(reference, o, (k as u64) << 16 | (r as u64) << 8 | (o + 32.0) as u64), &s.image);
                let roi = stroke_roi(&stroke, r, &s.image);
                let g = grade(extract(&s.image, roi, Polarity::Dark, Some(&BrushGuide { points: stroke.clone(), radius_px: r }), spacing).map(|e| e.path), reference, length(reference));
                record(&mut t, &s.shot_id, &format!("r{r} o{o:+}"), json!({"stroke": stroke, "radiusPx": r}), g);
            }
            brush.push(t.json(&format!("笔刷半径 {r} px · 偏 {o:+} px")));
        }
    }
    let mut roi_rows = Vec::new();
    for &m in &ROI_MARGINS {
        let mut t = Tally::default();
        for (k, s) in ctx.samples.iter().enumerate() {
            let Some(reference) = &refs[k] else { continue };
            let dense = densify(reference, 1.0);
            let total = length(reference);
            let starts: Vec<f32> = if total <= ROI_SECTION_PX * 1.2 { vec![0.0] } else { (0..3).map(|i| (total - ROI_SECTION_PX) * i as f32 / 2.0).collect() };
            for (si, &start) in starts.iter().enumerate() {
                let mut acc = 0.0;
                let mut section = Vec::new();
                for w in dense.windows(2) {
                    if acc >= start && acc <= start + ROI_SECTION_PX { section.push(w[0]); }
                    acc += dist(w[0], w[1]);
                }
                if section.len() < 2 { continue; }
                let roi = stroke_roi(&section, m, &s.image);
                let g = grade(extract(&s.image, roi, Polarity::Dark, None, spacing).map(|e| e.path), reference, length(&section));
                record(&mut t, &s.shot_id, &format!("框选 {si} 边距 {m}"), json!({"roi": roi}), g);
            }
        }
        roi_rows.push(t.json(&format!("矩形框选（{ROI_SECTION_PX} px 一段）· 边距 {m} px")));
    }
    let cases: Vec<Value> = fx.extract_cases.iter().map(|c: &ExtractCase| {
        let Some(k) = ctx.samples.iter().position(|s| s.shot_id == c.shot_id) else { return json!({"shot": c.shot_id, "error": "样本不在评测范围"}) };
        let s = &ctx.samples[k];
        let roi = stroke_roi(&c.stroke, c.radius_px, &s.image);
        let result = match extract(&s.image, roi, Polarity::Dark, Some(&BrushGuide { points: c.stroke.clone(), radius_px: c.radius_px }), spacing) {
            Err(ExtractionError::Failed(f)) => format!("提取失败 {}@{:.0},{:.0}", f.reason, f.point[0], f.point[1]),
            Err(ExtractionError::Invalid(m)) => format!("提取失败：{m}"),
            Ok(Extraction { path, .. }) => {
                let paths: Vec<Vec<[f32; 2]>> = (0..ctx.samples.len()).map(|j| if j == k { path.clone() } else { Vec::new() }).collect();
                let detect = ctx.cfg.detect.clone().unwrap_or_else(crate::recipe::default_detect);
                let measured = super::build_recipe(ctx.samples, &paths, ctx.mm, &detect).and_then(|recipe| super::run(ctx.engine, &recipe, k, &s.image));
                match measured {
                    Ok(m) => {
                        let d: Vec<f64> = m.d.iter().zip(&m.st).filter(|(_, st)| **st == ST_OK).map(|(d, _)| f64::from(*d)).collect();
                        format!("提取 {} 点、长 {:.0} px；lyFlow 按它复测有胶 {}/{} 站，偏移中位 {:.2} mm、|偏移| P95 {:.2} mm",
                            path.len(), length(&path), d.len(), m.st.len(), median(d.clone()), pct(d.iter().map(|v| v.abs()).collect(), 95.0))
                    }
                    Err(e) => format!("提取 {} 点，复测失败：{e}", path.len()),
                }
            }
        };
        json!({"shot": c.shot_id, "note": c.note, "radiusPx": c.radius_px, "result": result})
    }).collect();
    json!({"criteria": format!("通过 = 提取成功、对参考胶中心的偏离 P95 ≤ {PASS_P95_PX} px 且最大 ≤ {PASS_MAX_PX} px、长度 ≥ 70%"),
        "brush": brush, "roi": roi_rows, "cases": cases, "references": refs})
}

pub(super) fn markdown(v: &Value) -> String {
    let mut s = format!("## 示教提取（模拟操作）\n\n{}\n\n| 方式 | 次数 | 通过 | 偏离参考 | 失败（原因） | 偏离 P95 中位 / 最大 px |\n|---|---|---|---|---|---|\n", v["criteria"].as_str().unwrap_or(""));
    for r in v["brush"].as_array().into_iter().flatten().chain(v["roi"].as_array().into_iter().flatten()) {
        let failed: Vec<String> = r["failed"].as_object().into_iter().flatten().map(|(k, n)| format!("{k} {n}")).collect();
        s += &format!("| {} | {} | {} | {} | {} | {} / {} |\n", r["name"].as_str().unwrap_or(""), r["runs"], r["ok"], r["wrong"], failed.join("，"), r["devP95MedianPx"], r["devP95MaxPx"]);
    }
    s += "\n";
    for r in v["brush"].as_array().into_iter().flatten().chain(v["roi"].as_array().into_iter().flatten()) {
        for d in r["details"].as_array().into_iter().flatten() { s += &format!("- {}\n", d.as_str().unwrap_or("")); }
    }
    if let Some(cases) = v["cases"].as_array().filter(|c| !c.is_empty()) {
        s += "\n指定笔划：\n\n";
        for c in cases { s += &format!("- {} {}（半径 {} px）：{}\n", c["shot"].as_str().unwrap_or(""), c["note"].as_str().unwrap_or(""), c["radiusPx"], c["result"].as_str().or(c["error"].as_str()).unwrap_or("")); }
    }
    s + "\n"
}
