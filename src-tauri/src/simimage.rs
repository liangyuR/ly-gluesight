//! 模拟相机的合成图像：按配方的名义胶路画出暗开口、亮翻边、暗胶条与翻边上的安装孔，
//! 让 lyFlow 真实流程（定位 + 逐点卡尺）在没有相机时也能跑，并生成对应的示教资料。

use std::f64::consts::PI;
use std::path::Path;

use serde_json::json;

use crate::recipe::Recipe;
use crate::sim::Scenario;
use crate::vision::{FrameImage, ShotAssets, VisionAssets};

/// 模拟相机的像素当量（mm/px）。真实方案约 0.04，模拟取 0.08 让单帧约 5MP，合成与处理都快一些。
pub const SIM_MM_PER_PX: f64 = 0.08;
/// 名义胶条中线到内边的距离（mm），与配方的名义值一致。
const INNER_OFFSET: f64 = 0.75;
const BEAD_WIDTH: f64 = 1.0;
const HOLE_RADIUS: f64 = 2.5;

/// 机器人带来的位姿偏差（工件坐标里，绕拍照点中心）。
#[derive(Clone, Copy, Debug, Default)]
pub struct PoseError {
    pub dx: f64,
    pub dy: f64,
    pub deg: f64,
}

struct Scene<'a> {
    recipe: &'a Recipe,
    holes: Vec<(f64, f64)>,
    gap: Option<(f64, f64)>,
    bump_at: Option<f64>,
}

impl<'a> Scene<'a> {
    fn new(recipe: &'a Recipe, scenario: Scenario) -> Self {
        let pts = &recipe.points;
        let n = pts.x.len();
        let mut holes = Vec::new();
        let mut j = 40;
        while j < n {
            let (nx, ny) = outward(recipe, j);
            holes.push((pts.x[j] as f64 + 5.0 * nx, pts.y[j] as f64 + 5.0 * ny));
            j += 110;
        }
        let gap = scenario.gap_points(recipe).first().map(|&j| {
            let center = (j as f64 + 0.5) * recipe.spacing as f64;
            // 图像卡尺会沿切向平均，定位也有亚像素误差；两个边界点宽的缺口不稳定。
            // 缺口仍跨归属边界，并留出足够余量超过当前规则的允许断胶长度。
            let width = (recipe.max_gap_len as f64 + 2.0 * recipe.spacing as f64 + 1.0).max(3.0);
            (center - width / 2.0, center + width / 2.0)
        });
        let bump_at = (scenario == Scenario::Excursion).then(|| {
            let seg = &recipe.segments[4.min(recipe.segments.len() - 1)];
            (seg.s0 + (seg.s1 - seg.s0) * 0.3) as f64
        });
        Self { recipe, holes, gap, bump_at }
    }

    /// 到名义胶路（圆角矩形）的有符号距离，外正内负（mm）。
    fn sdf(&self, x: f64, y: f64) -> f64 {
        let [w, h, r] = self.recipe.part.map(|v| v as f64);
        let qx = (x - w / 2.0).abs() - (w / 2.0 - r);
        let qy = (y - h / 2.0).abs() - (h / 2.0 - r);
        let ox = qx.max(0.0);
        let oy = qy.max(0.0);
        (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
    }

    /// 最近的胶路点的弧长（与 recipe::rounded_rect 同一走向：从 (R, 0) 顺时针）。
    fn arc(&self, x: f64, y: f64) -> f64 {
        let [w, h, r] = self.recipe.part.map(|v| v as f64);
        let (a, b) = (w - 2.0 * r, h - 2.0 * r);
        let q = PI * r / 2.0;
        let cx = x.clamp(r, w - r);
        let cy = y.clamp(r, h - r);
        let (dx, dy) = (x - cx, y - cy);
        if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
            // 在内部直段区域：离哪条边近算哪条
            let d = [y, w - x, h - y, x];
            let i = (0..4).min_by(|&i, &j| d[i].total_cmp(&d[j])).unwrap();
            return [cx - r, a + q + (cy - r), a + q + b + q + (w - r - cx), 2.0 * a + b + 3.0 * q + (h - r - cy)][i];
        }
        match (dx.abs() < 1e-9, dy.abs() < 1e-9) {
            (true, false) if dy < 0.0 => cx - r,
            (false, true) if dx > 0.0 => a + q + (cy - r),
            (true, false) => a + q + b + q + (w - r - cx),
            (false, true) => 2.0 * a + b + 3.0 * q + (h - r - cy),
            _ => {
                let ang = dy.atan2(dx);
                let (base, a0) = if dx > 0.0 && dy < 0.0 {
                    (a, -PI / 2.0)
                } else if dx > 0.0 {
                    (a + q + b, 0.0)
                } else if dy > 0.0 {
                    (2.0 * a + b + 2.0 * q, PI / 2.0)
                } else {
                    (2.0 * a + 2.0 * b + 3.0 * q, PI)
                };
                let mut t = ang - a0;
                while t < 0.0 {
                    t += 2.0 * PI;
                }
                base + t.min(PI / 2.0) * r
            }
        }
    }

    fn value(&self, x: f64, y: f64, px: f64) -> f64 {
        let cover = |d: f64| (0.5 - d / px).clamp(0.0, 1.0);
        let sd = self.sdf(x, y);
        let mut v = 186.0 + 6.0 * (x / 37.0).sin() + 4.0 * (y / 23.0).cos();
        if sd.abs() < BEAD_WIDTH + 2.5 {
            let s = self.arc(x, y);
            let mut off = 0.0;
            if let Some(c) = self.bump_at {
                off += 1.0 * (-((s - c) / 1.5).powi(2)).exp();
            }
            let in_gap = self.gap.is_some_and(|(a, b)| s >= a && s <= b);
            if !in_gap {
                v += (72.0 - v) * cover((sd - off).abs() - BEAD_WIDTH / 2.0);
            }
        }
        for &(hx, hy) in &self.holes {
            let d = ((x - hx).powi(2) + (y - hy).powi(2)).sqrt() - HOLE_RADIUS;
            if d < px {
                v += (48.0 - v) * cover(d);
            }
        }
        v + (38.0 - v) * cover(sd + INNER_OFFSET)
    }
}

/// 胶路点 j 处指向翻边（远离开口）的单位法向。
fn outward(recipe: &Recipe, j: usize) -> (f64, f64) {
    let p = &recipe.points;
    let n = p.x.len();
    let a = (j + n - 1) % n;
    let b = (j + 1) % n;
    let (tx, ty) = ((p.x[b] - p.x[a]) as f64, (p.y[b] - p.y[a]) as f64);
    let l = (tx * tx + ty * ty).sqrt().max(1e-9);
    // 顺时针走（y 向下），右手法向 (−ty, tx) 朝里，朝外取反
    (ty / l, -tx / l)
}

/// 拍照点 k 的视野左上角（工件坐标，mm）与图像尺寸。
fn view(recipe: &Recipe, k: usize) -> (f64, f64, u32, u32) {
    let [cx, cy] = recipe.shots[k].center.map(|v| v as f64);
    let [fw, fh] = recipe.shot_fov(k).map(|v| v as f64);
    (cx - fw / 2.0, cy - fh / 2.0, (fw / SIM_MM_PER_PX).round() as u32, (fh / SIM_MM_PER_PX).round() as u32)
}

/// 由像素序号与种子得到 [-1, 1) 的确定性噪声（splitmix64）。
pub fn noise(i: u64, seed: u64) -> f64 {
    let mut z = i.wrapping_add(seed.wrapping_mul(0x9E3779B97F4A7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// 画拍照点 k 的一帧。pose 是机器人带来的工件偏差；seed 决定噪声。
pub fn render(recipe: &Recipe, k: usize, scenario: Scenario, pose: PoseError, seed: u64) -> FrameImage {
    let mut scene = Scene::new(recipe, scenario);
    let (x0, y0, w, h) = view(recipe, k);
    let [fw, fh] = recipe.shot_fov(k).map(|v| v as f64);
    scene.holes.retain(|&(hx, hy)| hx > x0 - 6.0 && hx < x0 + fw + 6.0 && hy > y0 - 6.0 && hy < y0 + fh + 6.0);
    let [pcx, pcy] = recipe.shots[k].center.map(|v| v as f64);
    let (s, c) = (-pose.deg * PI / 180.0).sin_cos();
    let mut pixels = vec![0u8; (w * h) as usize];
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let rows_per = (h as usize).div_ceil(threads);
    std::thread::scope(|scope| {
        for (chunk, band) in pixels.chunks_mut(rows_per * w as usize).enumerate() {
            let scene = &scene;
            scope.spawn(move || {
                for (r, row) in band.chunks_mut(w as usize).enumerate() {
                    let v = chunk * rows_per + r;
                    for (u, px) in row.iter_mut().enumerate() {
                        // 像素 → 世界（mm）→ 工件：世界里的工件是工件坐标经 R(deg)、平移 (dx, dy) 放过来的
                        let wx = x0 + u as f64 * SIM_MM_PER_PX - pcx - pose.dx;
                        let wy = y0 + v as f64 * SIM_MM_PER_PX - pcy - pose.dy;
                        let x = c * wx - s * wy + pcx;
                        let y = s * wx + c * wy + pcy;
                        let i = v as u64 * w as u64 + u as u64;
                        let val = scene.value(x, y, SIM_MM_PER_PX) + 2.5 * noise(i, seed);
                        *px = val.round().clamp(0.0, 255.0) as u8;
                    }
                }
            });
        }
    });
    FrameImage::new(w, h, pixels)
}

fn write_pgm(path: &Path, img: &FrameImage, x: u32, y: u32, w: u32, h: u32) -> Result<(), String> {
    let mut out = format!("P5\n{w} {h}\n255\n").into_bytes();
    for row in y..y + h {
        let start = (row * img.width + x) as usize;
        out.extend_from_slice(&img.pixels[start..start + w as usize]);
    }
    std::fs::write(path, out).map_err(|e| format!("写模板失败：{e}"))
}

/// 模拟相机的“示教”：按名义位姿画每个拍照点的示教图，以离视野中心最近的安装孔为中心裁模板，
/// 把这一帧负责的名义测量点换到示教图像素坐标，写成 lyFlow 测量点文件；标定按已知像素当量写。
pub fn teach(recipe: &Recipe, dir: &Path) -> Result<VisionAssets, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let scene = Scene::new(recipe, Scenario::Normal);
    let calib = dir.join("sim_calib.json");
    let px = SIM_MM_PER_PX;
    std::fs::write(
        &calib,
        json!({"kind": "Record", "type": "image.PlaneCalib", "data": {"H": [px, 0, 0, 0, px, 0, 0, 0, 1], "unit": "mm"}}).to_string(),
    )
    .map_err(|e| e.to_string())?;
    let mut shots = Vec::new();
    for k in 0..recipe.shot_count() {
        let (x0, y0, w, h) = view(recipe, k);
        let img = render(recipe, k, Scenario::Normal, PoseError::default(), 1);
        let [cx, cy] = recipe.shots[k].center.map(|v| v as f64);
        let &(hx, hy) = scene
            .holes
            .iter()
            .min_by(|a, b| ((a.0 - cx).hypot(a.1 - cy)).total_cmp(&(b.0 - cx).hypot(b.1 - cy)))
            .ok_or("场景里没有安装孔，裁不出模板")?;
        // 模板跨在内边与安装孔之间：一条贯穿模板的直边定角度，孔定沿边方向的位置（只有圆孔时角度不可辨）
        let j = (0..recipe.point_count())
            .min_by(|&a, &b| {
                let da = (recipe.points.x[a] as f64 - hx).hypot(recipe.points.y[a] as f64 - hy);
                let db = (recipe.points.x[b] as f64 - hx).hypot(recipe.points.y[b] as f64 - hy);
                da.total_cmp(&db)
            })
            .unwrap_or(0);
        let (tx, ty) = ((recipe.points.x[j] as f64 + hx) / 2.0, (recipe.points.y[j] as f64 + hy) / 2.0);
        // 模板离图像边留 40 px：工件一动，贴边的模板有几行会落到图外，和边界填充比出假角度
        let size = (28.0 / px).round() as u32;
        let margin = 40i64;
        let ax = (((tx - x0) / px) as i64 - size as i64 / 2).clamp(margin, w as i64 - size as i64 - margin) as u32;
        let ay = (((ty - y0) / px) as i64 - size as i64 / 2).clamp(margin, h as i64 - size as i64 - margin) as u32;
        let template = dir.join(format!("k{k}.template.pgm"));
        write_pgm(&template, &img, ax, ay, size, size)?;

        let (mut points, mut normals, mut ids) = (Vec::new(), Vec::new(), Vec::new());
        for j in recipe.owned_points(k) {
            let (nx, ny) = outward(recipe, j);
            points.push(json!([(recipe.points.x[j] as f64 - x0) / px, (recipe.points.y[j] as f64 - y0) / px]));
            normals.push(json!([nx, ny]));
            ids.push(j);
        }
        let stations = crate::vision::save_stations(dir, k, points, normals, ids)?;
        shots.push(ShotAssets { template, anchor: [ax as f64, ay as f64], stations, calib: calib.clone() });
    }
    Ok(VisionAssets {
        recipe_id: recipe.id.clone(),
        recipe_hash: recipe.hash.clone(),
        sim_mm_per_px: Some(px),
        shots,
    })
}
