//! 特征定位判胶偏的纯几何（定稿 docs/discussions/fixture-position/final.md §2）：
//! glue.locate 的位姿 T 把示教胶中线 N 搬到当帧 N' = T(N)，胶中心 B 到 N' 的有向法向距离就是胶偏。
//! 只算像素，不做 IO；换算 mm、扣名义值、判定都在调用方。
// P0 只有算法评测在用；G2 把 parse_result 接上偏差计算后去掉这一行。
#![cfg_attr(not(test), allow(dead_code))]

use serde_json::Value;

/// 线段法向与 n'_i 的夹角上限 45°（cos 值）：拐角处不投到另一条腿上。
const MAX_ANGLE_COS: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// 投影参数允许越出 [0, 1] 的量：站点正好落在折线顶点上时两侧线段都算在内。
const EDGE_EPS: f64 = 1e-6;

/// glue.locate 输出的 glue.Pose2D：T(p) = R(angle)·(p − teachCenter) + center。
/// 与 LyFlow `packs/glue/algo/flyshot.cpp` 的 `Pose2D::rotate` / `apply` 一致：
/// 图像坐标 y 向下，angle（度）> 0 是屏幕上的顺时针。
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    pub ok: bool,
    pub score: f64,
    pub angle: f64,
    pub center: [f64; 2],
    pub teach_center: [f64; 2],
    /// locate 1.1.0 起才有：次峰 / 主峰，越小越唯一
    pub peak_ratio: Option<f64>,
    /// locate 1.1.0 起才有：失败原因（low_score / ambiguous / at_search_edge / …）
    pub reason: Option<String>,
}

impl Pose {
    pub fn identity() -> Self {
        Self { ok: true, score: 1.0, angle: 0.0, center: [0.0, 0.0], teach_center: [0.0, 0.0], peak_ratio: None, reason: None }
    }

    /// 解析 glue.locate 的 `pose` Record 数据。定位失败时 LyFlow 可能把非有限坐标写成 null，
    /// 此时只保留 ok = false；定位成功却缺字段或非有限数视为输出契约不一致。
    pub fn from_record(v: &Value) -> Result<Self, String> {
        let ok = v["ok"].as_bool().ok_or("定位结果缺少 ok")?;
        let num = |key: &str| v[key].as_f64().filter(|x| x.is_finite());
        let xy = |key: &str| -> Option<[f64; 2]> {
            let a = v[key].as_array().filter(|a| a.len() == 2)?;
            let p = [a[0].as_f64()?, a[1].as_f64()?];
            p.iter().all(|x| x.is_finite()).then_some(p)
        };
        let (score, angle, center, teach_center) = (num("score"), num("angle"), xy("center"), xy("teachCenter"));
        if ok && (score.is_none() || angle.is_none() || center.is_none() || teach_center.is_none()) {
            return Err("定位成功但位姿字段缺失或不是有限数".into());
        }
        let nan = [f64::NAN; 2];
        Ok(Self {
            ok,
            score: score.unwrap_or(f64::NAN),
            angle: angle.unwrap_or(f64::NAN),
            center: center.unwrap_or(nan),
            teach_center: teach_center.unwrap_or(nan),
            peak_ratio: num("peakRatio"),
            reason: v["reason"].as_str().map(str::to_string),
        })
    }

    pub fn rotate(&self, v: [f64; 2]) -> [f64; 2] {
        let (s, c) = self.angle.to_radians().sin_cos();
        [c * v[0] - s * v[1], s * v[0] + c * v[1]]
    }

    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        let r = self.rotate([p[0] - self.teach_center[0], p[1] - self.teach_center[1]]);
        [r[0] + self.center[0], r[1] + self.center[1]]
    }
}

/// 就近投影的站号窗口 k = ⌈searchRadiusPx / stationStepPx⌉：位姿最多把胶路沿切向搬这么多站。
pub fn window(search_radius_px: f64, station_step_px: f64) -> usize {
    if !(search_radius_px > 0.0) || !(station_step_px > 0.0) { return 1; }
    ((search_radius_px / station_step_px).ceil() as usize).max(1)
}

/// 逐站胶偏（px，沿 n'_i 为正）。`None` 表示该站没有位置基准：无胶、定位失败、
/// 或窗口内没有一条法向合格且投影落在线段内的线段。
///
/// - `nominal` / `normals`：示教胶路的站点与单位法向（示教图像素）；`beads`：当帧测得的胶中心。
/// - 站 i 只在 N' 的线段 (N'_j, N'_{j+1})，j ∈ [i − k, i + k − 1] 上找：线段法向（取与 n'_i 同侧）
///   与 n'_i 夹角 < 45°，投影参数在 [0, 1] 内，取 |距离| 最小的一条。
/// - 距离是 B_i 到该线段的有向垂直距离，正向取 n'_i 一侧。
/// - 弯道外侧的胶可能落在相邻两条线段之间的楔形区（对两条都越界）：两条都合格时按到共同顶点的距离算；
///   越出整条 N' 的两端或窗口两端时仍为 `None`。
pub fn deviations(nominal: &[[f64; 2]], normals: &[[f64; 2]], beads: &[Option<[f64; 2]>], pose: &Pose, window: usize) -> Result<Vec<Option<f64>>, String> {
    let n = nominal.len();
    if normals.len() != n || beads.len() != n {
        return Err("胶偏计算的站点、法向与胶中心数量不一致".into());
    }
    if !pose.ok { return Ok(vec![None; n]); }
    let moved: Vec<[f64; 2]> = nominal.iter().map(|&p| pose.apply(p)).collect();
    let k = window.max(1);
    Ok((0..n).map(|i| {
        let b = beads[i]?;
        let normal = pose.rotate(normals[i]);
        let dot = |p: [f64; 2], q: [f64; 2]| p[0] * q[0] + p[1] * q[1];
        // 窗口内每条线段：是否合格（法向夹角门）与投影参数 u
        let segments: Vec<(usize, bool, f64)> = (i.saturating_sub(k)..(i + k).min(n.saturating_sub(1))).map(|j| {
            let (a, e) = (moved[j], moved[j + 1]);
            let t = [e[0] - a[0], e[1] - a[1]];
            let len2 = dot(t, t);
            if !(len2 > 1e-18) { return (j, false, f64::NAN); }
            let m = [-t[1], t[0]];
            let ok = dot(m, normal).abs() / len2.sqrt() > MAX_ANGLE_COS;
            (j, ok, dot([b[0] - a[0], b[1] - a[1]], t) / len2)
        }).collect();
        let mut best: Option<f64> = None;
        let mut keep = |d: f64| if best.is_none_or(|x| d.abs() < x.abs()) { best = Some(d) };
        for (s, &(j, ok, u)) in segments.iter().enumerate() {
            if !ok { continue; }
            let a = moved[j];
            if (-EDGE_EPS..=1.0 + EDGE_EPS).contains(&u) {
                let t = [moved[j + 1][0] - a[0], moved[j + 1][1] - a[1]];
                let mut m = [-t[1], t[0]];
                if dot(m, normal) < 0.0 { m = [-m[0], -m[1]]; }
                keep(dot([b[0] - a[0], b[1] - a[1]], m) / dot(m, m).sqrt());
            } else if u > 1.0 && segments.get(s + 1).is_some_and(|&(_, next_ok, next_u)| next_ok && next_u < 0.0) {
                // 折线内部顶点外侧的楔形区：两侧线段都合格时，最近点就是这个顶点
                let v = moved[j + 1];
                let rel = [b[0] - v[0], b[1] - v[1]];
                keep(dot(rel, rel).sqrt().copysign(dot(rel, normal)));
            }
        }
        best
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CENTER: [f64; 2] = [400.0, 300.0];
    const RADIUS: f64 = 200.0;
    const STEP: f64 = 8.0;

    /// 圆弧胶路：站距 8 px，法向朝圆外；胶中心沿法向偏 d_i（第 7 站无胶）。
    fn arc_scene() -> (Vec<[f64; 2]>, Vec<[f64; 2]>, Vec<Option<[f64; 2]>>, Vec<f64>) {
        let count = 40;
        let (mut nominal, mut normals, mut beads, mut offsets) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for i in 0..count {
            let theta = 0.3 + i as f64 * STEP / RADIUS;
            let n = [theta.cos(), theta.sin()];
            let p = [CENTER[0] + RADIUS * n[0], CENTER[1] + RADIUS * n[1]];
            let d = 3.0 * (i as f64 / 4.0).sin();
            nominal.push(p);
            normals.push(n);
            beads.push((i != 7).then_some([p[0] + n[0] * d, p[1] + n[1] * d]));
            offsets.push(d);
        }
        (nominal, normals, beads, offsets)
    }

    fn pose(angle: f64, teach_center: [f64; 2], center: [f64; 2]) -> Pose {
        Pose::from_record(&json!({"ok": true, "score": 0.93, "angle": angle, "center": center, "teachCenter": teach_center})).unwrap()
    }

    /// 按 LyFlow Pose2D 的公式独立实现的刚体变换，用来摆“当帧”场景。
    fn lyflow_apply(p: [f64; 2], angle: f64, teach_center: [f64; 2], center: [f64; 2]) -> [f64; 2] {
        let (s, c) = angle.to_radians().sin_cos();
        let (x, y) = (p[0] - teach_center[0], p[1] - teach_center[1]);
        [c * x - s * y + center[0], s * x + c * y + center[1]]
    }

    #[test]
    fn workpiece_and_bead_move_together_rigidly_then_deviation_is_unchanged() {
        // 契约：angle > 0 为屏幕顺时针（y 向下），x 轴转 90° 指向 +y
        let quarter = pose(90.0, [0.0, 0.0], [0.0, 0.0]);
        let q = quarter.apply([10.0, 0.0]);
        assert!((q[0] - 0.0).abs() < 1e-9 && (q[1] - 10.0).abs() < 1e-9, "{q:?}");

        let (nominal, normals, beads, offsets) = arc_scene();
        let still = deviations(&nominal, &normals, &beads, &Pose::identity(), window(60.0, STEP)).unwrap();
        // 两端站的投影可能越出折线（见下面端点测试），只核对中间站
        for (i, d) in still.iter().enumerate().take(nominal.len() - 1).skip(1) {
            match beads[i] {
                Some(_) => assert!((d.unwrap() - offsets[i]).abs() < 0.06, "站 {i}：{d:?} vs {}", offsets[i]),
                None => assert!(d.is_none()),
            }
        }
        let (angle, teach_center, center) = (7.5, [380.0, 290.0], [431.0, 262.0]);
        let moved: Vec<Option<[f64; 2]>> = beads.iter().map(|b| b.map(|p| lyflow_apply(p, angle, teach_center, center))).collect();
        let together = deviations(&nominal, &normals, &moved, &pose(angle, teach_center, center), window(60.0, STEP)).unwrap();
        for i in 0..nominal.len() {
            match (still[i], together[i]) {
                (Some(a), Some(b)) => assert!((a - b).abs() < 1e-6, "站 {i}：{a} → {b}"),
                (a, b) => assert_eq!(a.is_some(), b.is_some(), "站 {i}"),
            }
        }
    }

    #[test]
    fn workpiece_slides_along_curve_while_bead_stays_then_deviation_is_true_normal_distance() {
        // 绕圆心转 0.15 rad：胶路整体沿自身切向搬约 30 px（近 4 站），胶不动；到圆的真实距离仍是 d_i
        let (nominal, normals, beads, offsets) = arc_scene();
        let slide = pose(0.15f64.to_degrees(), CENTER, CENTER);
        let k = window(60.0, STEP);
        let got = deviations(&nominal, &normals, &beads, &slide, k).unwrap();
        for i in k..nominal.len() - k {
            let Some(b) = beads[i] else { continue };
            let d = got[i].unwrap_or_else(|| panic!("站 {i} 应有基准"));
            assert!((d - offsets[i]).abs() < 0.06, "站 {i}：{d} vs {}", offsets[i]);
            // 按站号对应（B_i − N'_i）·n'_i 会差出 2 px 以上，就近投影必须胜过它
            let (p, n) = (slide.apply(nominal[i]), slide.rotate(normals[i]));
            let by_index = (b[0] - p[0]) * n[0] + (b[1] - p[1]) * n[1];
            assert!((by_index - offsets[i]).abs() > 1.5, "站 {i}：场景没有区分出站号对应的误差");
        }
    }

    #[test]
    fn station_near_right_angle_corner_then_never_projects_onto_other_leg() {
        // 横腿 (0,0)→(100,0) 法向 (0,1)，竖腿 (100,5)→(100,100) 法向 (−1,0)
        let mut nominal: Vec<[f64; 2]> = (0..=20).map(|i| [i as f64 * 5.0, 0.0]).collect();
        let mut normals: Vec<[f64; 2]> = (0..20).map(|_| [0.0, 1.0]).collect();
        normals.push([-std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2]);
        for i in 1..=20 {
            nominal.push([100.0, i as f64 * 5.0]);
            normals.push([-1.0, 0.0]);
        }
        // 横腿第 19 站（x = 95）的胶在 (96, 6)：离横腿 6 px，离竖腿只有 4 px 且投影落在竖腿线段内
        let mut beads = vec![None; nominal.len()];
        beads[19] = Some([96.0, 6.0]);
        let got = deviations(&nominal, &normals, &beads, &Pose::identity(), 4).unwrap();
        assert!((got[19].unwrap() - 6.0).abs() < 1e-9, "{:?}", got[19]);
    }

    #[test]
    fn projection_falls_beyond_moved_path_end_then_station_has_no_reference() {
        // 直胶路 x ∈ [0, 100]，位姿把它沿 +x 搬 20 px，胶不动：x < 20 的站在 N' 上找不到线段
        let nominal: Vec<[f64; 2]> = (0..=20).map(|i| [i as f64 * 5.0, 100.0]).collect();
        let normals = vec![[0.0, -1.0]; nominal.len()];
        let beads: Vec<Option<[f64; 2]>> = nominal.iter().map(|p| Some([p[0], p[1] - 3.0])).collect();
        let got = deviations(&nominal, &normals, &beads, &pose(0.0, [50.0, 50.0], [70.0, 50.0]), window(60.0, 5.0)).unwrap();
        assert!(got[..4].iter().all(Option::is_none), "{:?}", &got[..4]);
        assert!(got[5..].iter().all(|d| d.is_some_and(|d| (d - 3.0).abs() < 1e-9)), "{:?}", &got[5..]);
    }

    #[test]
    fn locate_failed_then_every_station_has_no_reference() {
        let (nominal, normals, beads, _) = arc_scene();
        // 分数不够时 LyFlow 照样给出有限位姿（这里恰为恒等）；搜索区出图时坐标可能是 null
        for record in [
            json!({"ok": false, "score": 0.31, "angle": 0.0, "center": [380.0, 290.0], "teachCenter": [380.0, 290.0], "message": "匹配分数低于 minScore"}),
            json!({"ok": false, "score": 0.0, "angle": 0.0, "center": null, "teachCenter": [380.0, 290.0], "reason": "out_of_image"}),
        ] {
            let failed = Pose::from_record(&record).unwrap();
            let got = deviations(&nominal, &normals, &beads, &failed, window(60.0, STEP)).unwrap();
            assert!(got.iter().all(Option::is_none), "{record}");
        }
    }
}
