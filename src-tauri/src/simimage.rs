//! 模拟相机的合成图像：一台装在胶枪上的相机拍到的一帧——亮的金属面、右侧的胶嘴、沿拍照点示教中线拖出的暗胶条。
//! 断胶、横向超差、机器人位姿偏差按场景画进图里，让沿中线量胶的流程在没有相机时也能跑。
//!
//! 旧拍照点配方按拍照点示教中线画（[`render_views`]）；段配方按脉冲的真值画（[`render_burst`]）：直线段胶条沿这台相机的
//! 走廊（没示教时按相机取缺省位置），弯道段胶条绕胶嘴在视野内转动，转角随段内进度呈拱形。

use crate::camera::BurstRender;
use crate::frame::FrameImage;
use crate::plan::SegmentKind;
use crate::recipe::{Recipe, ShotSpec, SIM_MM_PER_PX};
use crate::offline::Scenario;
use std::sync::Arc;

/// 模拟画面尺寸，与现场相机（MV-CU013-80GC）一致。
pub const SIM_SIZE: [u32; 2] = [1280, 1024];
/// 名义胶宽（mm），与样例配方的名义值一致。
const BEAD_WIDTH: f32 = 4.0;

/// 机器人带来的偏差（图像里）：整条胶平移 dx、dy 像素，绕中线起点转 deg 度。
#[derive(Clone, Copy, Debug, Default)]
pub struct PoseError {
    pub dx: f32,
    pub dy: f32,
    pub deg: f32,
}

/// 由像素序号与种子得到 [-1, 1) 的确定性噪声（splitmix64）。
pub fn noise(i: u64, seed: u64) -> f64 {
    let mut z = i.wrapping_add(seed.wrapping_mul(0x9E3779B97F4A7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// 没示教的拍照点画一条缺省中线，示教时照着它点。
fn default_path() -> Vec<[f32; 2]> {
    vec![[980.0, 480.0], [300.0, 460.0]]
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

pub fn render_views(recipe: &Recipe, k: usize, scenario: Scenario, pose: PoseError, seed: u64, view_count: u8) -> Result<Vec<Arc<FrameImage>>, String> {
    if !matches!(view_count, 1 | 3) {
        return Err("视角数量只能是 1 或 3".into());
    }
    Ok((1..=view_count).map(|view| Arc::new(render_view(recipe, k, scenario, pose, seed, view))).collect())
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
    let recipe = &r.recipe;
    let inspect = recipe.inspect.get(r.camera_idx);
    let path = inspect.filter(|i| i.corridor.len() >= 2).map_or_else(|| burst_default_path(r.camera_idx), |i| i.corridor.clone());
    let mm = inspect.and_then(|i| i.mm_per_px).map_or(SIM_MM_PER_PX, |v| v as f32);
    let checks = recipe.path_segments.iter().find(|s| s.kind == r.kind).or(recipe.path_segments.first()).map(|s| &s.checks);
    let nominal = checks.map_or(BEAD_WIDTH, |c| c.width.nominal_mm as f32) + if r.kind == SegmentKind::Corner { CORNER_EXTRA_WIDTH } else { 0.0 };
    // 弯道：整条胶绕胶嘴端（中线起点）转动
    let (sin, cos) = corner_angle(r.kind, r.progress).to_radians().sin_cos();
    let o = path[0];
    let path: Vec<[f32; 2]> = path.iter().map(|p| {
        let (x, y) = (p[0] - o[0], p[1] - o[1]);
        [o[0] + x * cos - y * sin, o[1] + x * sin + y * cos]
    }).collect();
    let length: f32 = path.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
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
            if !missing && s > 0.0 {
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

fn render_view(recipe: &Recipe, k: usize, scenario: Scenario, pose: PoseError, seed: u64, view: u8) -> FrameImage {
    let shot: Option<&ShotSpec> = recipe.shots.get(k);
    let selected = shot.map_or(1, |s| s.view) == view;
    let path = shot.filter(|s| s.path.len() >= 2).map_or_else(default_path, |s| s.path.clone());
    let mm = shot.and_then(|s| s.mm_per_px).unwrap_or(SIM_MM_PER_PX);
    // 位姿偏差：中线绕起点转、再平移
    let (sin, cos) = pose.deg.to_radians().sin_cos();
    let o = path[0];
    let path: Vec<[f32; 2]> = path
        .iter()
        .map(|p| {
            let (x, y) = (p[0] - o[0], p[1] - o[1]);
            [o[0] + x * cos - y * sin + pose.dx, o[1] + x * sin + y * cos + pose.dy]
        })
        .collect();
    let half = BEAD_WIDTH / mm / 2.0;
    // 断胶：缺口按站号换成中线弧长（px）
    let gap: Option<(f32, f32)> = recipe.shot_segment(k).and_then(|g| {
        let pts: Vec<usize> = scenario.gap_points(recipe).into_iter().filter(|&j| j >= g.first && j < g.first + g.count).collect();
        let step = recipe.spacing / mm;
        Some(((pts.first()? - g.first) as f32 * step, (pts.last()? - g.first + 1) as f32 * step))
    });
    // 横向超差：最后一段 30% 处偏出约 3 mm
    let bump = recipe
        .segments
        .last()
        .filter(|g| scenario == Scenario::Excursion && g.shot == k)
        .map(|g| g.length(recipe.spacing) * 0.3 / mm);
    let nozzle = {
        let (a, b) = (path[0], path[1]);
        let l = (b[0] - a[0]).hypot(b[1] - a[1]).max(1e-6);
        [a[0] - (b[0] - a[0]) / l * 60.0, a[1] - (b[1] - a[1]) / l * 60.0]
    };
    let [w, h] = SIM_SIZE;
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = [x as f32, y as f32];
            // 金属面：左暗右亮的渐变加细纹
            let mut v = background(x, y, w, h);
            if selected {
                let (d, s, side) = nearest(&path, p);
                let d = match bump {
                    Some(c) => (side - 3.0 / mm * (-((s - c) * mm / 1.5).powi(2)).exp()).abs(),
                    None => d,
                };
                let missing = gap.is_some_and(|(a, b)| s >= a && s < b);
                if !missing && s > 0.0 {
                    // 胶条：中间最暗，边缘 2 px 过渡
                    let edge = ((half - d) / 2.0).clamp(0.0, 1.0);
                    v = v * (1.0 - edge) + 38.0 * edge;
                }
                // 胶嘴：亮的金属圆
                let dn = (p[0] - nozzle[0]).hypot(p[1] - nozzle[1]);
                if dn < 45.0 {
                    v = 235.0 - dn;
                }
            }
            v += 5.0 * noise((y * w + x) as u64, seed) as f32;
            pixels.push(v.clamp(0.0, 255.0) as u8);
        }
    }
    FrameImage::new(w, h, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::builtin;

    fn at(img: &FrameImage, p: [f32; 2]) -> u8 {
        img.pixels[(p[1].round() as u32 * img.width + p[0].round() as u32) as usize]
    }

    /// 胶条走向（度）：暗像素（胶条）坐标的主轴方向。
    fn bead_angle(img: &FrameImage) -> f64 {
        let w = img.width as usize;
        let dark: Vec<(f64, f64)> = img.pixels.iter().enumerate().filter(|(_, &v)| v < 90).map(|(i, _)| ((i % w) as f64, (i / w) as f64)).collect();
        let n = dark.len() as f64;
        let (mx, my) = dark.iter().fold((0.0, 0.0), |(x, y), p| (x + p.0 / n, y + p.1 / n));
        let (cxx, cyy, cxy) = dark.iter().fold((0.0, 0.0, 0.0), |(a, b, c), p| (a + (p.0 - mx).powi(2), b + (p.1 - my).powi(2), c + (p.0 - mx) * (p.1 - my)));
        0.5 * (2.0 * cxy).atan2(cxx - cyy).to_degrees()
    }

    /// 规格 §3/§6：合成回归输入可信——离线产线按剖面发的脉冲经 simimage 出图，画成弯道的帧恰好是分段回归入口
    /// （`segmentation::synthetic`）同一剖面与种子的真值边界之间的那些帧，每个真值边界两侧各一帧分别是本段与下一段的类型。
    /// 胶条走向相对第 1 帧（直线段）转过 3° 以上即判为弯道。剖面带首尾、弯道波动，渲染照名义帧数画就会失败。
    #[test]
    fn rendered_burst_sequence_corner_frames_match_truth_boundaries() {
        use crate::offline::line::PulseProfile;
        use crate::segmentation::synthetic;
        use SegmentKind::{Corner, Straight};
        let nominal = vec![(Straight, 3), (Corner, 3), (Straight, 3)];
        let profile = (0..).map(|seed| PulseProfile { hz: 20.0, segments: nominal.clone(), head_tail_jitter: 1, corner_jitter: 1, seed })
            .find(|p| p.counts() != [3, 3, 3]).unwrap();
        let truth = synthetic::generate(&profile.spec(Vec::new()), profile.seed).truth;
        let recipe = Arc::new(crate::recipe::samples_burst().build().unwrap());
        let angles: Vec<f64> = profile.pulses(&[]).iter().map(|pulse| bead_angle(&render_burst(&BurstRender {
            recipe: recipe.clone(), camera_idx: 0, seq: pulse.seq, kind: pulse.kind, progress: pulse.progress, scenario: Scenario::Normal, seed: 3,
        }))).collect();
        let rendered: Vec<SegmentKind> = angles.iter().map(|a| if (a - angles[0]).abs() > 3.0 { Corner } else { Straight }).collect();
        let from_truth: Vec<SegmentKind> = (1..=angles.len() as u64).map(|seq| nominal[truth.iter().filter(|b| b.frame_seq < seq).count()].0).collect();
        assert_eq!(rendered, from_truth, "各帧走向 {angles:?}，真值边界 {truth:?}");
    }

    #[test]
    fn three_views_draw_glue_only_in_the_selected_view() {
        let mut recipe = (*builtin().remove(1)).clone();
        for view in [1, 2, 3, 1] {
            recipe.shots[0].view = view;
            let images = render_views(&recipe, 0, Scenario::Normal, PoseError::default(), 7, 3).unwrap();
            assert_eq!(images.len(), 3);
            let shot = &recipe.shots[0];
            let mid = shot.path_at(shot.path_len_px() / 2.0);
            for (index, image) in images.iter().enumerate() {
                assert_eq!([image.width, image.height], SIM_SIZE);
                if index + 1 == view as usize {
                    assert!(at(image, mid) < 70, "视角 {view} 应有胶条");
                } else {
                    assert!(image.pixels.iter().all(|&v| v > 120), "未选中的视角只能有背景");
                }
            }
        }
    }

}
