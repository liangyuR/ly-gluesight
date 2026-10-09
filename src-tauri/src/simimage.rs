//! 模拟相机的合成图像：一台装在胶枪上的相机拍到的一帧——亮的金属面、右侧的胶嘴、沿拍照点示教中线拖出的暗胶条。
//! 断胶、横向超差、机器人位姿偏差按场景画进图里，让沿中线量胶的流程在没有相机时也能跑。

use crate::frame::FrameImage;
use crate::recipe::{Recipe, ShotSpec, SIM_MM_PER_PX};
use crate::sim::Scenario;

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

/// 拍照点 k 的一帧。seed 决定噪声；不检、未示教的拍照点照常出图（缺省中线）。
pub fn render(recipe: &Recipe, k: usize, scenario: Scenario, pose: PoseError, seed: u64) -> FrameImage {
    let shot: Option<&ShotSpec> = recipe.shots.get(k);
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
            let mut v = 150.0 + 60.0 * (x as f32 / w as f32) + 25.0 * (y as f32 / h as f32) + 6.0 * (x as f32 * 0.05 + y as f32 * 0.31).sin();
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

    #[test]
    fn bead_is_dark_along_the_taught_line() {
        let r = builtin().remove(1);
        let img = render(&r, 0, Scenario::Normal, PoseError::default(), 1);
        assert_eq!([img.width, img.height], SIM_SIZE);
        let shot = &r.shots[0];
        let mid = shot.path_at(shot.path_len_px() / 2.0);
        assert!(at(&img, mid) < 70, "胶条中心 {}", at(&img, mid));
        // 离中线 40 px（约 4.5 mm）已是背景
        assert!(at(&img, [mid[0], mid[1] + 40.0]) > 120);
    }

    #[test]
    fn gap_scenario_removes_bead_inside_the_gap() {
        let r = builtin().remove(1);
        let pts = Scenario::Gap.gap_points(&r);
        let g = r.shot_segment(1).unwrap();
        assert!(pts.iter().all(|&j| j >= g.first && j < g.first + g.count));
        let img = render(&r, 1, Scenario::Gap, PoseError::default(), 1);
        let mid = pts[pts.len() / 2];
        assert!(at(&img, [r.points.x[mid], r.points.y[mid]]) > 120, "缺口处应是背景");
        let outside = g.first + 2;
        assert!(at(&img, [r.points.x[outside], r.points.y[outside]]) < 70);
    }
}
