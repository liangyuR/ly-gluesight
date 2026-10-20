//! 特征定位判胶偏的纯几何（定稿 docs/discussions/fixture-position/final.md §2）：
//! glue.locate 的位姿 T 把示教胶中线 N 搬到当帧 N' = T(N)，胶中心 B 到 N' 的有向法向距离就是胶偏。
//! 只算像素，不做 IO；换算 mm、扣名义值、判定都在调用方。
// P0 只有算法评测在用；G2 把 parse_result 接上偏差计算后去掉这一行。
#![cfg_attr(not(test), allow(dead_code))]

use serde_json::Value;

/// 线段法向与 n'_i 的夹角上限 45°（cos 值）：拐角处不投到另一条腿上。
/// 正好 45° 的算合格：直角拐点上的站（法向取两腿的角平分线）与两条腿都恰成 45°，严格小于会让这个站必然丢基准。
const MAX_ANGLE_COS: f64 = std::f64::consts::FRAC_1_SQRT_2 - 1e-9;
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
    /// locate 1.1.0 起才有：false = 模板看不出角度，angle 报 0、center 是 0° 上最好的平移，
    /// 即位姿只补了平移、转角没补偿（不等于“零件没转”）；1.0.0 的记录没有这个字段（None）
    pub angle_observable: Option<bool>,
}

impl Pose {
    pub fn identity() -> Self {
        Self { ok: true, score: 1.0, angle: 0.0, center: [0.0, 0.0], teach_center: [0.0, 0.0], peak_ratio: None, reason: None, angle_observable: None }
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
            // 1.1.0 成功时 reason 为空串
            reason: v["reason"].as_str().filter(|r| !r.is_empty()).map(str::to_string),
            angle_observable: v["angleObservable"].as_bool(),
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

/// 逐站胶偏（px，沿 n'_i 为正）。`None` 表示该站没有位置基准：无胶、定位失败、
/// 或窗口内没有一条法向合格且投影落在线段内的线段。
///
/// - `nominal` / `normals`：示教胶路的站点与单位法向（示教图像素）；`beads`：当帧测得的胶中心。
/// - 窗口：站 i 只在 N' 上沿弧长离 N'_i 不超过 r_i = |B_i − N'_i| 的线段里找，两侧各再多一段。
///   直线段上 B_i 的垂足离 N'_i 不会比 B_i 本身更远，所以位姿带来的任何切向搬移（平移、绕模板中心
///   转动的杠杆臂分量）都在窗口内；窗口只挡同一条胶路上弧长更远的部分（U 形的另一条腿）。
/// - 窗口内取线段法向（取与 n'_i 同侧）与 n'_i 夹角 ≤ 45°、投影参数在 [0, 1] 内、|距离| 最小的一条。
/// - 距离是 B_i 到该线段的有向垂直距离，正向取 n'_i 一侧。
/// - 弯道外侧的胶可能落在相邻两条线段之间的楔形区（对两条都越界）：两条都合格时按到共同顶点的距离算；
///   越出整条 N' 的两端或窗口两端时仍为 `None`。
pub fn deviations(nominal: &[[f64; 2]], normals: &[[f64; 2]], beads: &[Option<[f64; 2]>], pose: &Pose) -> Result<Vec<Option<f64>>, String> {
    let n = nominal.len();
    if normals.len() != n || beads.len() != n {
        return Err("胶偏计算的站点、法向与胶中心数量不一致".into());
    }
    if !pose.ok { return Ok(vec![None; n]); }
    let moved: Vec<[f64; 2]> = nominal.iter().map(|&p| pose.apply(p)).collect();
    // 示教胶路的累计弧长（刚体变换不改弧长）
    let arc: Vec<f64> = std::iter::once(0.0).chain(nominal.windows(2).scan(0.0, |s, w| {
        *s += (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
        Some(*s)
    })).collect();
    let last = n.saturating_sub(2);
    Ok((0..n).map(|i| {
        let b = beads[i]?;
        if n < 2 { return None; }
        let normal = pose.rotate(normals[i]);
        let dot = |p: [f64; 2], q: [f64; 2]| p[0] * q[0] + p[1] * q[1];
        let reach = (b[0] - moved[i][0]).hypot(b[1] - moved[i][1]);
        if !reach.is_finite() { return None; }
        // 线段 j 覆盖弧长 [arc_j, arc_{j+1}]：取与 [arc_i − r, arc_i + r] 相交的线段，两侧各多一段
        let lo = arc.partition_point(|&a| a < arc[i] - reach).saturating_sub(2);
        let hi = arc.partition_point(|&a| a <= arc[i] + reach).min(last);
        // 窗口内每条线段：是否合格（法向夹角门）与投影参数 u
        let segments: Vec<(usize, bool, f64)> = (lo..=hi).map(|j| {
            let (a, e) = (moved[j], moved[j + 1]);
            let t = [e[0] - a[0], e[1] - a[1]];
            let len2 = dot(t, t);
            if !(len2 > 1e-18) { return (j, false, f64::NAN); }
            let m = [-t[1], t[0]];
            let ok = dot(m, normal).abs() / len2.sqrt() >= MAX_ANGLE_COS;
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

    /// 圆弧胶路：站距 8 px；胶中心在法向朝圆外一侧偏 d_i（第 7 站无胶）。
    /// `normal_sign` 为 −1 时法向朝圆心，胶不动，d_i 随之变号。
    fn arc_scene(normal_sign: f64) -> (Vec<[f64; 2]>, Vec<[f64; 2]>, Vec<Option<[f64; 2]>>, Vec<f64>) {
        let count = 40;
        let (mut nominal, mut normals, mut beads, mut offsets) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for i in 0..count {
            let theta = 0.3 + i as f64 * STEP / RADIUS;
            let n = [theta.cos(), theta.sin()];
            let p = [CENTER[0] + RADIUS * n[0], CENTER[1] + RADIUS * n[1]];
            let d = 3.0 * (i as f64 / 4.0).sin();
            nominal.push(p);
            normals.push([n[0] * normal_sign, n[1] * normal_sign]);
            beads.push((i != 7).then_some([p[0] + n[0] * d, p[1] + n[1] * d]));
            offsets.push(d * normal_sign);
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

    /// 直角拐弯：横腿站 0..=20，拐点为第 20 站（法向取角平分线，与 BeadFrame 中心差分一致），竖腿站 21..=40。
    fn right_angle_corner() -> (Vec<[f64; 2]>, Vec<[f64; 2]>) {
        let mut nominal: Vec<[f64; 2]> = (0..=20).map(|i| [i as f64 * 5.0, 0.0]).collect();
        let mut normals: Vec<[f64; 2]> = (0..20).map(|_| [0.0, 1.0]).collect();
        normals.push([-std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2]);
        for i in 1..=20 {
            nominal.push([100.0, i as f64 * 5.0]);
            normals.push([-1.0, 0.0]);
        }
        (nominal, normals)
    }

    #[test]
    fn pose_with_positive_angle_then_rotates_clockwise_on_screen_like_lyflow() {
        // 契约（flyshot.cpp Pose2D::rotate）：y 向下，angle > 0 时 x 轴转 90° 指向 +y
        let q = pose(90.0, [0.0, 0.0], [0.0, 0.0]).apply([10.0, 0.0]);
        assert!((q[0] - 0.0).abs() < 1e-9 && (q[1] - 10.0).abs() < 1e-9, "{q:?}");
    }

    #[test]
    fn workpiece_and_bead_move_together_rigidly_then_deviation_is_unchanged() {
        // 两种法向朝向：弯道外侧的胶落在楔形区、按顶点距离算，符号也要跟 n'_i 走
        for normal_sign in [1.0, -1.0] {
            let (nominal, normals, beads, offsets) = arc_scene(normal_sign);
            let still = deviations(&nominal, &normals, &beads, &Pose::identity()).unwrap();
            // 两端站的投影可能越出折线（见下面端点测试），只核对中间站
            for (i, d) in still.iter().enumerate().take(nominal.len() - 1).skip(1) {
                match beads[i] {
                    Some(_) => assert!((d.unwrap() - offsets[i]).abs() < 0.06, "法向 {normal_sign} 站 {i}：{d:?} vs {}", offsets[i]),
                    None => assert!(d.is_none()),
                }
            }
            let (angle, teach_center, center) = (7.5, [380.0, 290.0], [431.0, 262.0]);
            let moved: Vec<Option<[f64; 2]>> = beads.iter().map(|b| b.map(|p| lyflow_apply(p, angle, teach_center, center))).collect();
            let together = deviations(&nominal, &normals, &moved, &pose(angle, teach_center, center)).unwrap();
            for i in 0..nominal.len() {
                match (still[i], together[i]) {
                    (Some(a), Some(b)) => assert!((a - b).abs() < 1e-6, "法向 {normal_sign} 站 {i}：{a} → {b}"),
                    (a, b) => assert_eq!(a.is_some(), b.is_some(), "法向 {normal_sign} 站 {i}"),
                }
            }
        }
    }

    #[test]
    fn workpiece_slides_along_curve_while_bead_stays_then_deviation_is_true_normal_distance() {
        // 绕圆心转 0.15 rad：胶路整体沿自身切向搬约 30 px（近 4 站），胶不动；到圆的真实距离仍是 d_i
        let (nominal, normals, beads, offsets) = arc_scene(1.0);
        let slide = pose(0.15f64.to_degrees(), CENTER, CENTER);
        let got = deviations(&nominal, &normals, &beads, &slide).unwrap();
        let ends = 8;
        for i in ends..nominal.len() - ends {
            let Some(b) = beads[i] else { continue };
            let d = got[i].unwrap_or_else(|| panic!("站 {i} 应有基准"));
            assert!((d - offsets[i]).abs() < 0.06, "站 {i}：{d} vs {}", offsets[i]);
            // 按站号对应（B_i − N'_i）·n'_i 会差出 2 px 以上（R(1 − cos 0.15) ≈ 2.2 px），就近投影必须胜过它
            let (p, n) = (slide.apply(nominal[i]), slide.rotate(normals[i]));
            let by_index = (b[0] - p[0]) * n[0] + (b[1] - p[1]) * n[1];
            assert!((by_index - offsets[i]).abs() > 2.0, "站 {i}：场景没有区分出站号对应的误差");
        }
    }

    #[test]
    fn pose_moves_path_tangentially_beyond_search_radius_then_interior_stations_keep_reference() {
        // 直胶路 A + s·u；胶随工件走（与 N' 平行、沿 n' 偏 d），在静态站点的法线上测得胶中心。
        // 三类位姿都在 locate 接受的范围内，切向搬移却超过 searchRadius 60 px：
        // 绕远处模板中心转 ±3°（杠杆臂约 900–1000 px）再加平移；搜索窗是方形，对角平移 (55, 55) 合计约 78 px。
        let d = 4.0;
        let s2 = std::f64::consts::FRAC_1_SQRT_2;
        // 起点、方向、站距、站数、角度、teachCenter、center
        let scenes: [([f64; 2], [f64; 2], f64, usize, f64, [f64; 2], [f64; 2]); 4] = [
            ([0.0, 100.0], [1.0, 0.0], 8.0, 101, 3.0, [400.0, 1000.0], [440.0, 1000.0]),
            ([0.0, 100.0], [1.0, 0.0], 8.0, 101, -3.0, [400.0, 1000.0], [345.0, 1005.0]),
            ([1000.0, 0.0], [0.0, 1.0], 8.0, 201, 3.0, [0.0, 800.0], [0.0, 830.0]),
            ([100.0, 100.0], [s2, s2], 20.0, 40, 0.0, [500.0, 300.0], [555.0, 355.0]),
        ];
        for (a, u, step, count, angle, tc, c) in scenes {
            let normal = [-u[1], u[0]];
            let nominal: Vec<[f64; 2]> = (0..count).map(|i| [a[0] + u[0] * step * i as f64, a[1] + u[1] * step * i as f64]).collect();
            let normals = vec![normal; count];
            let pose = pose(angle, tc, c);
            let (v, m) = (pose.rotate(u), pose.rotate(normal));
            let q0 = pose.apply(a);
            let q = [q0[0] + m[0] * d, q0[1] + m[1] * d];
            // 静态法线 P_i + s·n 与当帧胶线 q + t·v 的交点
            let det = -normal[0] * v[1] + v[0] * normal[1];
            let beads: Vec<Option<[f64; 2]>> = nominal.iter().map(|p| {
                let r = [q[0] - p[0], q[1] - p[1]];
                let s = (-r[0] * v[1] + v[0] * r[1]) / det;
                Some([p[0] + normal[0] * s, p[1] + normal[1] * s])
            }).collect();
            let got = deviations(&nominal, &normals, &beads, &pose).unwrap();
            let length = step * (count - 1) as f64;
            let mut inside = 0;
            for (i, b) in beads.iter().enumerate() {
                let b = b.unwrap();
                // t：胶中心在 N' 上的弧长位置
                let t = (b[0] - q[0]) * v[0] + (b[1] - q[1]) * v[1];
                if t > 1.0 && t < length - 1.0 {
                    inside += 1;
                    assert!(got[i].is_some_and(|x| (x - d).abs() < 1e-6), "角度 {angle} 位姿中心 {c:?} 站 {i}：{:?}", got[i]);
                } else if t < -1.0 || t > length + 1.0 {
                    assert!(got[i].is_none(), "角度 {angle} 位姿中心 {c:?} 站 {i}：{:?}", got[i]);
                }
            }
            assert!(inside > count / 2, "场景太偏，中间站太少：{inside}/{count}");
        }
    }

    #[test]
    fn station_near_right_angle_corner_then_never_projects_onto_other_leg() {
        // 横腿 (0,0)→(100,0) 法向 (0,1)，竖腿 (100,5)→(100,100) 法向 (−1,0)
        let (nominal, normals) = right_angle_corner();
        // 横腿第 19 站（x = 95）的胶在 (96, 6)：离横腿 6 px，离竖腿只有 4 px 且投影落在竖腿线段内
        let mut beads = vec![None; nominal.len()];
        beads[19] = Some([96.0, 6.0]);
        let got = deviations(&nominal, &normals, &beads, &Pose::identity()).unwrap();
        assert!((got[19].unwrap() - 6.0).abs() < 1e-9, "{:?}", got[19]);
    }

    #[test]
    fn station_on_right_angle_vertex_then_keeps_reference() {
        // 拐点站 (100, 0) 的法向是角平分线，与两条腿都恰成 45°：内侧胶到两腿等距，外侧胶按到拐点的距离
        let (nominal, normals) = right_angle_corner();
        for (bead, want) in [([99.0, 1.0], 1.0), ([101.0, -1.0], -std::f64::consts::SQRT_2)] {
            let mut beads = vec![None; nominal.len()];
            beads[20] = Some(bead);
            let got = deviations(&nominal, &normals, &beads, &Pose::identity()).unwrap();
            assert!(got[20].is_some_and(|d| (d - want).abs() < 1e-9), "{bead:?}：{:?}", got[20]);
        }
    }

    #[test]
    fn projection_falls_beyond_moved_path_end_then_station_has_no_reference() {
        // 直胶路 x ∈ [0, 100]，位姿把它沿 ±x 搬 20 px，胶不动：落在 N' 两端之外的站找不到线段；
        // 越过末端时楔形区规则也不能拿末端顶点凑数
        let nominal: Vec<[f64; 2]> = (0..=20).map(|i| [i as f64 * 5.0, 100.0]).collect();
        let normals = vec![[0.0, -1.0]; nominal.len()];
        let beads: Vec<Option<[f64; 2]>> = nominal.iter().map(|p| Some([p[0], p[1] - 3.0])).collect();
        for shift in [20.0, -20.0] {
            let got = deviations(&nominal, &normals, &beads, &pose(0.0, [50.0, 50.0], [50.0 + shift, 50.0])).unwrap();
            let (lo, hi) = (shift, 100.0 + shift);
            for (i, p) in nominal.iter().enumerate() {
                if p[0] < lo || p[0] > hi {
                    assert!(got[i].is_none(), "平移 {shift} 站 {i}：{:?}", got[i]);
                } else if p[0] > lo && p[0] < hi {
                    assert!(got[i].is_some_and(|d| (d - 3.0).abs() < 1e-9), "平移 {shift} 站 {i}：{:?}", got[i]);
                }
            }
        }
    }

    #[test]
    fn locate_failed_then_every_station_has_no_reference() {
        let (nominal, normals, beads, _) = arc_scene(1.0);
        // 分数不够时 LyFlow 照样给出有限位姿（这里恰为恒等）；搜索区出图时坐标可能是 null
        for record in [
            json!({"ok": false, "score": 0.31, "angle": 0.0, "center": [380.0, 290.0], "teachCenter": [380.0, 290.0], "message": "匹配分数低于 minScore"}),
            json!({"ok": false, "score": 0.0, "angle": 0.0, "center": null, "teachCenter": [380.0, 290.0], "reason": "out_of_image"}),
        ] {
            let failed = Pose::from_record(&record).unwrap();
            let got = deviations(&nominal, &normals, &beads, &failed).unwrap();
            assert!(got.iter().all(Option::is_none), "{record}");
        }
    }
}
