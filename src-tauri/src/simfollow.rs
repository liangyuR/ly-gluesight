//! 随动模拟相机：按胶嘴当前位置画出"胶嘴 + 身后已涂的胶条"。胶条的偏移、胶宽、断胶由场景决定，
//! 模拟测量（不带图像）用同一套胶条模型，图像测量与模拟测量的结果可以互相对照。

use crate::follow::{behind, FollowCalib};
use crate::frame::FrameImage;
use crate::recipe::Recipe;
use crate::sim::Scenario;
use crate::simimage;

/// 场景作用的弧长区间：第 seg 段的 [a, b] 比例处。
fn region(recipe: &Recipe, seg: usize, a: f32, b: f32) -> (f32, f32) {
    let g = &recipe.segments[seg.min(recipe.segments.len() - 1)];
    (g.s0 + (g.s1 - g.s0) * a, g.s0 + (g.s1 - g.s0) * b)
}

fn wave(s: f32) -> f32 {
    0.06 * (s / 31.0).sin() + 0.03 * (s / 7.3 + 0.8).sin()
}

/// 胶条中线相对名义胶路的横向偏移（mm）。
pub fn offset(recipe: &Recipe, scenario: Scenario, s: f32) -> f32 {
    let mut d = wave(s);
    if scenario == Scenario::Excursion {
        let (a, b) = region(recipe, 4, 0.3, 0.3);
        let c = (a + b) / 2.0;
        d += 1.3 * (-((s - c) / 2.5).powi(2)).exp();
    }
    d
}

/// 胶宽（mm）。
pub fn width(recipe: &Recipe, scenario: Scenario, s: f32) -> f32 {
    let nominal = recipe.follow.as_ref().map_or(2.0, |f| f.bead_width);
    let mut w = nominal * (1.0 + 0.04 * (s / 11.0).sin());
    if scenario == Scenario::Narrow {
        let (a, b) = region(recipe, 2, 0.35, 0.35);
        let c = (a + b) / 2.0;
        w *= 1.0 - 0.55 * (-((s - c) / 5.0).powi(4)).exp();
    }
    w
}

/// 断胶区间。
pub fn gap(recipe: &Recipe, scenario: Scenario) -> Option<(f32, f32)> {
    (scenario == Scenario::Gap).then(|| {
        let (a, _) = region(recipe, 2, 0.5, 0.5);
        (a, a + 5.0)
    })
}


/// 胶嘴位于弧长 s 时这台相机的画面：亮的金属背景、暗胶条、图像下方的胶嘴。
pub fn render(recipe: &Recipe, calib: &FollowCalib, s: f32, scenario: Scenario, seed: u64) -> FrameImage {
    let [w, h] = calib.image_size;
    let (wf, hf) = (w as f32, h as f32);
    let px = calib.mm_per_px;
    let mut pixels = vec![0u8; (w * h) as usize];
    let mut sd = vec![f32::MAX; (w * h) as usize];

    // 胶条：已涂部分（身后 0 到整幅图像对角线长度）逐段光栅化，记录到胶条边缘的有符号距离（像素）
    let reach = (wf.hypot(hf) * px) + 5.0;
    let sp = recipe.spacing;
    let n = recipe.point_count();
    let gap = gap(recipe, scenario);
    let nozzle = recipe.pos(s);
    let mut centers: Vec<Option<([f32; 2], f32)>> = Vec::new();
    let first = ((s - reach) / sp).floor() as i64;
    let last = (s / sp).floor() as i64;
    for i in first..=last {
        let j = if recipe.closed { i.rem_euclid(n as i64) as usize } else if i < 0 || i >= n as i64 { centers.push(None); continue } else { i as usize };
        let sj = j as f32 * sp;
        let laid = behind(recipe, s, j).is_some_and(|b| b <= reach);
        let in_gap = gap.is_some_and(|(a, b)| sj >= a && sj <= b);
        if !laid || in_gap {
            centers.push(None);
            continue;
        }
        let nrm = recipe.normal(sj);
        let o = offset(recipe, scenario, sj);
        let c = [recipe.points.x[j] + nrm[0] * o - nozzle[0], recipe.points.y[j] + nrm[1] * o - nozzle[1]];
        centers.push(Some((calib.to_px(c), width(recipe, scenario, sj) / px / 2.0)));
    }
    // 胶条最新的一端连到胶嘴下方
    centers.push(Some((calib.nozzle, recipe.follow.as_ref().map_or(2.0, |f| f.bead_width) / px / 2.0)));
    for pair in centers.windows(2) {
        let (Some((a, ra)), Some((b, rb))) = (pair[0], pair[1]) else { continue };
        let r = ra.max(rb) + 2.0;
        let (x0, x1) = ((a[0].min(b[0]) - r).max(0.0) as u32, (a[0].max(b[0]) + r).min(wf - 1.0).max(0.0) as u32);
        let (y0, y1) = ((a[1].min(b[1]) - r).max(0.0) as u32, (a[1].max(b[1]) + r).min(hf - 1.0).max(0.0) as u32);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = (dx * dx + dy * dy).max(1e-6);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px_, py_) = (x as f32 - a[0], y as f32 - a[1]);
                let t = ((px_ * dx + py_ * dy) / l2).clamp(0.0, 1.0);
                let d = ((px_ - dx * t).powi(2) + (py_ - dy * t).powi(2)).sqrt() - (ra + (rb - ra) * t);
                let i = (y * w + x) as usize;
                if d < sd[i] {
                    sd[i] = d;
                }
            }
        }
    }

    let (nx, ny) = (calib.nozzle[0], calib.nozzle[1]);
    let nozzle_r = (calib.mask_px * 0.85).max(8.0);
    // 三台模拟相机同时出图：每幅最多用三分之一的核，别把检测节拍的线程挤掉
    let threads = (std::thread::available_parallelism().map_or(4, |n| n.get()) / 3).clamp(1, 8);
    let rows_per = (h as usize).div_ceil(threads);
    std::thread::scope(|scope| {
        for (chunk, (band, sd_band)) in pixels.chunks_mut(rows_per * w as usize).zip(sd.chunks(rows_per * w as usize)).enumerate() {
            scope.spawn(move || {
                for (r, (row, sd_row)) in band.chunks_mut(w as usize).zip(sd_band.chunks(w as usize)).enumerate() {
                    let y = (chunk * rows_per + r) as f32;
                    for (x, (v, &d)) in row.iter_mut().zip(sd_row).enumerate() {
                        let xf = x as f32;
                        let mut val = 168.0 + 34.0 * (xf / wf) - 26.0 * (y / hf);
                        let cover = (0.5 - d).clamp(0.0, 1.0);
                        // 胶条暗，中间一道弱反光
                        let bead = 58.0 + if d < -2.5 { 10.0 * (1.0 + d / 6.0).max(0.0) } else { 0.0 };
                        val += (bead - val) * cover;
                        let rn = ((xf - nx).powi(2) + (y - ny).powi(2)).sqrt();
                        if rn < nozzle_r + 1.0 {
                            let c = (nozzle_r + 0.5 - rn).clamp(0.0, 1.0);
                            val += (30.0 + 12.0 * (rn / nozzle_r) - val) * c;
                        }
                        let i = (y as u64) * w as u64 + x as u64;
                        *v = (val + 3.0 * simimage::noise(i, seed) as f32).round().clamp(0.0, 255.0) as u8;
                    }
                }
            });
        }
    });
    FrameImage::new(w, h, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caliper;
    use crate::follow::{self, FollowCalib};
    use crate::measure::{Job, JobKind, Measured, ST_GAP, ST_OK};
    use crate::recipe::builtin;

    #[test]
    fn caliper_recovers_rendered_bead() {
        let recipe = builtin().into_iter().find(|r| r.follow.is_some()).unwrap();
        let spec = recipe.follow.clone().unwrap();
        let mut checked = 0;
        for (angle, s, scenario) in [(0.0, 60.0, Scenario::Normal), (120.0, 300.0, Scenario::Normal), (240.0, 520.0, Scenario::Narrow)] {
            let calib = FollowCalib { nozzle: [640.0, 900.0], angle_deg: angle, mirror: false, mm_per_px: 0.05, mask_px: 60.0, image_size: [1280, 1024] };
            let img = render(&recipe, &calib, s, scenario, 7);
            let points: Vec<u32> = follow::visible(&recipe, &spec, &calib, s).into_iter().map(|j| j as u32).collect();
            if points.is_empty() {
                continue;
            }
            let job = Job { run_id: 0, sn: 1, k: 0, cam: 0, recipe: recipe.clone(), scenario, image: None, kind: JobKind::Follow { s, points: points.clone(), calib: calib.clone(), start_probe: false } };
            let mut m = Measured::empty(&job);
            caliper::measure_follow(&recipe, &spec, &calib, s, &points, &img, &mut m);
            checked += 1;
            let ok: Vec<usize> = (0..m.idx.len()).filter(|&i| m.st[i] == ST_OK).collect();
            assert!(ok.len() * 10 >= m.idx.len() * 9, "角度 {angle}：{}/{} 点测到", ok.len(), m.idx.len());
            for &i in &ok {
                let sj = m.idx[i] as f32 * recipe.spacing;
                assert!((m.d[i] - offset(&recipe, scenario, sj)).abs() < 0.12, "偏移 {} vs {}", m.d[i], offset(&recipe, scenario, sj));
                assert!((m.w[i] - width(&recipe, scenario, sj)).abs() < 0.15, "胶宽 {} vs {}", m.w[i], width(&recipe, scenario, sj));
            }
        }
        assert!(checked >= 2, "只有 {checked} 个用例落在可测窗口里");
        // 断胶处测成缺胶
        let (a, _) = gap(&recipe, Scenario::Gap).unwrap();
        let s = a + 10.0;
        let calib = (0..3)
            .map(|c| FollowCalib { nozzle: [640.0, 900.0], angle_deg: 120.0 * c as f32, mirror: false, mm_per_px: 0.05, mask_px: 60.0, image_size: [1280, 1024] })
            .max_by_key(|c| follow::visible(&recipe, &spec, c, s).len())
            .unwrap();
        let img = render(&recipe, &calib, s, Scenario::Gap, 3);
        let j = ((a + 1.5) / recipe.spacing) as u32;
        let job = Job { run_id: 0, sn: 1, k: 0, cam: 0, recipe: recipe.clone(), scenario: Scenario::Gap, image: None, kind: JobKind::Follow { s, points: vec![j], calib: calib.clone(), start_probe: false } };
        let mut m = Measured::empty(&job);
        caliper::measure_follow(&recipe, &spec, &calib, s, &[j], &img, &mut m);
        assert_eq!(m.st, vec![ST_GAP]);
    }

    #[test]
    fn start_probe_recovers_nozzle_error() {
        let recipe = builtin().into_iter().find(|r| r.follow.is_some()).unwrap();
        let spec = recipe.follow.clone().unwrap();
        // 起点在第一条直边上，沿 +x 走；胶条在胶嘴身后（图像左侧）
        let calib = FollowCalib { nozzle: [640.0, 900.0], angle_deg: 0.0, mirror: false, mm_per_px: 0.05, mask_px: 60.0, image_size: [1280, 1024] };
        for (s_true, s_est) in [(8.0f32, 13.0f32), (13.0, 10.0)] {
            let img = render(&recipe, &calib, s_true, Scenario::Normal, 5);
            let d = caliper::find_start(&recipe, &spec, &calib, s_est, &img).expect("找不到起点");
            assert!((d - (s_est - s_true)).abs() < 0.8, "δ={d}，应为 {}", s_est - s_true);
        }
    }
}
