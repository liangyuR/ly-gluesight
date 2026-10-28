//! 模拟相机的合成图像：一台装在胶枪上的相机拍到的一帧——亮的金属面、右侧的胶嘴、沿检测走廊拖出的暗胶条。
//! 断胶、局部超差按场景画进图里，让沿走廊量胶的流程在没有相机时也能跑。
//!
//! 段配方按脉冲的真值画（[`render_burst`]）：直线段胶条沿这台相机的走廊（没示教时按相机取缺省位置），
//! 弯道段胶条绕胶嘴在视野内转动，转角随段内进度呈拱形。

use crate::camera::BurstRender;
use crate::frame::FrameImage;
use crate::plan::SegmentKind;
use crate::recipe::SIM_MM_PER_PX;
use crate::offline::Scenario;
use std::sync::Arc;

/// 模拟画面尺寸，与现场相机（MV-CU013-80GC）一致。
pub const SIM_SIZE: [u32; 2] = [1280, 1024];
/// 名义胶宽（mm），与样例配方的名义值一致。
const BEAD_WIDTH: f32 = 4.0;

/// 由像素序号与种子得到 [-1, 1) 的确定性噪声（splitmix64）。
pub fn noise(i: u64, seed: u64) -> f64 {
    let mut z = i.wrapping_add(seed.wrapping_mul(0x9E3779B97F4A7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// 点 p 到折线的最近距离、最近点所在的弧长（px）与有向横向距离。
fn nearest(path: &[[f32; 2]], p: [f32; 2]) -> (f32, f32, f32) {
    let mut best = (f32::MAX, 0.0, 0.0);
    let mut s0 = 0.0;
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = (dx * dx + dy * dy).max(1e-9);
        let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
        let (qx, qy) = (a[0] + dx * t, a[1] + dy * t);
        let d = (p[0] - qx).hypot(p[1] - qy);
        if d < best.0 {
            let side = dx * (p[1] - a[1]) - dy * (p[0] - a[0]);
            best = (d, s0 + t * l2.sqrt(), d * side.signum());
        }
        s0 += l2.sqrt();
    }
    best
}

pub fn background_views(view_count: u8, seed: u64) -> Result<Vec<Arc<FrameImage>>, String> {
    if !matches!(view_count, 1 | 3) {
        return Err("视角数量只能是 1 或 3".into());
    }
    let [w, h] = SIM_SIZE;
    Ok((0..view_count).map(|view| {
        let pixels = (0..w * h).map(|i| {
            let x = i % w;
            let y = i / w;
            (background(x, y, w, h) + 5.0 * noise(i as u64, seed.wrapping_add(view as u64)) as f32).clamp(0.0, 255.0) as u8
        }).collect();
        Arc::new(FrameImage::new(w, h, pixels))
    }).collect())
}

/// 弯道帧胶条绕胶嘴转动的最大角度（度），在弯道段中点达到。
pub const CORNER_SWEEP_DEG: f32 = 25.0;
/// 弯道段胶条比名义胶宽略宽（mm）。
const CORNER_EXTRA_WIDTH: f32 = 0.3;

/// 段配方没示教走廊时各台相机的缺省胶条位置：三台相机从不同方位看胶嘴，胶条在画面里上下错开。
fn burst_default_path(camera_idx: usize) -> Vec<[f32; 2]> {
    let dy = 80.0 * (camera_idx.min(2) as f32 - 1.0);
    vec![[980.0, 480.0 + dy], [300.0, 460.0 + dy]]
}

/// 弯道帧胶条的转角（度）：直线段 0；弯道段按段内进度呈拱形，两端小、中点最大（[`CORNER_SWEEP_DEG`]）。
pub fn corner_angle(kind: SegmentKind, progress: f32) -> f32 {
    match kind {
        SegmentKind::Straight => 0.0,
        SegmentKind::Corner => CORNER_SWEEP_DEG * (std::f32::consts::PI * progress.clamp(0.0, 1.0)).sin(),
    }
}

/// 段配方一台相机的一帧（单视角）：直线段胶条沿走廊，弯道段绕胶嘴转 [`corner_angle`]。
/// 断胶 / 局部超差工况画在第 1 台相机第 2 帧上（与模拟测量 `measure::simulate_frame` 标的同一帧）：
/// 胶条中部缺一段（比允许断口长 2 mm），或 1/3 处约 3 mm 长的一段加宽到超出胶宽上限 1 mm。
pub fn render_burst(r: &BurstRender) -> FrameImage {
    render_burst_span(r, [0.0, 1.0])
}

/// 同 [`render_burst`]，胶条只画沿走廊从出胶点量起、弧长占比在 `span` 内的一段，模拟起涂与收尾：
/// 起涂为 [0, f]（胶条从胶嘴往外长），收尾为 [g, 1]（阀门关后胶尾离开胶嘴），空区间即无胶。
pub fn render_burst_span(r: &BurstRender, span: [f32; 2]) -> FrameImage {
    let recipe = &r.recipe;
    let inspect = recipe.inspect.get(r.camera_idx);
    let path = inspect
        .and_then(|i| i.segments.iter().find(|s| s.corridor.len() >= 2))
        .map_or_else(|| burst_default_path(r.camera_idx), |s| s.corridor.clone());
    let mm = inspect
        .and_then(|i| i.mm_per_px)
        .map_or(SIM_MM_PER_PX, |v| v as f32);
    let checks = recipe
        .path_segments
        .iter()
        .find(|s| s.kind == r.kind)
        .or(recipe.path_segments.first())
        .map(|s| &s.checks);
    let nominal = checks.map_or(BEAD_WIDTH, |c| c.width.nominal_mm as f32)
        + if r.kind == SegmentKind::Corner {
            CORNER_EXTRA_WIDTH
        } else {
            0.0
        };
    // 弯道：整条胶绕胶嘴端（中线起点）转动
    let (sin, cos) = corner_angle(r.kind, r.progress).to_radians().sin_cos();
    let o = path[0];
    let path: Vec<[f32; 2]> = path.iter().map(|p| {
        let (x, y) = (p[0] - o[0], p[1] - o[1]);
        [o[0] + x * cos - y * sin, o[1] + x * sin + y * cos]
    }).collect();
    let length: f32 = path.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
    let from = span[0].max(0.0) * length;
    let to = if span[1] >= 1.0 { f32::INFINITY } else { span[1] * length };
    let marked = r.camera_idx == 0 && r.seq == 2;
    let gap = match (r.scenario, checks) {
        (Scenario::Gap, Some(c)) if marked => Some(((c.gap.max_gap_mm as f32 + 2.0) / mm).min(length)),
        _ => None,
    }.map(|g| ((length - g) / 2.0, (length + g) / 2.0));
    let excursion = match (r.scenario, checks) {
        (Scenario::Excursion, Some(c)) if marked => Some((length / 3.0, (c.width.nominal_mm + c.width.tol_upper + 1.0) as f32)),
        _ => None,
    };
    let nozzle = {
        let (a, b) = (path[0], path[1]);
        let l = (b[0] - a[0]).hypot(b[1] - a[1]).max(1e-6);
        [a[0] - (b[0] - a[0]) / l * 60.0, a[1] - (b[1] - a[1]) / l * 60.0]
    };
    let [w, h] = SIM_SIZE;
    let seed = r.seed.wrapping_mul(31).wrapping_add(r.camera_idx as u64 * 7 + r.seq);
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = [x as f32, y as f32];
            let mut v = background(x, y, w, h);
            let (d, s, _) = nearest(&path, p);
            let width = excursion.filter(|(c, _)| (s - c).abs() * mm < 1.5).map_or(nominal, |(_, wide)| wide);
            let half = width / mm / 2.0;
            let missing = gap.is_some_and(|(a, b)| s >= a && s < b);
            if !missing && s > from && s <= to {
                let edge = ((half - d) / 2.0).clamp(0.0, 1.0);
                v = v * (1.0 - edge) + 38.0 * edge;
            }
            let dn = (p[0] - nozzle[0]).hypot(p[1] - nozzle[1]);
            if dn < 45.0 {
                v = 235.0 - dn;
            }
            v += 5.0 * noise((y * w + x) as u64, seed) as f32;
            pixels.push(v.clamp(0.0, 255.0) as u8);
        }
    }
    FrameImage::new(w, h, pixels)
}

fn background(x: u32, y: u32, w: u32, h: u32) -> f32 {
    150.0 + 60.0 * (x as f32 / w as f32) + 25.0 * (y as f32 / h as f32) + 6.0 * (x as f32 * 0.05 + y as f32 * 0.31).sin()
}
