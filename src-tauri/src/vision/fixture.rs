//! 特征定位判胶偏的纯几何（定稿 docs/discussions/fixture-position/final.md §2）：
//! glue.locate 的位姿 T 把示教胶中线 N 搬到当帧 N' = T(N)，胶中心 B 到 N' 的有向法向距离就是胶偏。
//! 只算像素，不做 IO；换算 mm、扣名义值、判定都在调用方。

use serde_json::Value;

use crate::frame::FrameImage;
use crate::recipe::FIXTURE_TEMPLATE_PAD_PX;

/// 图里 glue.locate 的模板文件名（相对 base_dir）。每个（拍照点，视图）各给一个 base_dir。
pub const TEMPLATE_FILE: &str = "template.pgm";

/// 示教框（内框）加 FIXTURE_TEMPLATE_PAD_PX 外圈后的矩形；外圈出了 w × h 的图时为 None。
pub fn padded_rect(r: [u32; 4], w: u32, h: u32) -> Option<[u32; 4]> {
    let p = FIXTURE_TEMPLATE_PAD_PX;
    let (x, y) = (r[0].checked_sub(p)?, r[1].checked_sub(p)?);
    let (pw, ph) = (r[2].checked_add(2 * p)?, r[3].checked_add(2 * p)?);
    (x.checked_add(pw)? <= w && y.checked_add(ph)? <= h).then_some([x, y, pw, ph])
}

/// glue.locate 的模板文件（lyFlow#32）：示教图上示教框四周各外扩 FIXTURE_TEMPLATE_PAD_PX 裁下的一块，
/// 存成 PGM，大小 (w + 2·pad) × (h + 2·pad)，左上角在 (x − pad, y − pad)；anchor 仍是示教框左上角。
///
/// 裁法属于测量定义：改这里必须同时改 `store::measurement_layout` 里的 `fixtureTemplate`，否则模板身份
/// (来源, 框, 外圈) 不变、模板像素却变了，历史重判会复用对着旧模板量的 d′。
pub fn template_pgm(image: &FrameImage, rect: [u32; 4]) -> Result<Vec<u8>, String> {
    if image.pixels.len() as u64 != u64::from(image.width) * u64::from(image.height) {
        return Err("示教原图像素缓冲与尺寸不一致".into());
    }
    let r = padded_rect(rect, image.width, image.height)
        .ok_or_else(|| format!("定位框 {rect:?} 加 {FIXTURE_TEMPLATE_PAD_PX} px 外圈超出示教原图（{}×{}）", image.width, image.height))?;
    let mut bytes = format!("P5\n{} {}\n255\n", r[2], r[3]).into_bytes();
    for y in r[1]..r[1] + r[3] {
        let row = (y * image.width + r[0]) as usize;
        bytes.extend_from_slice(&image.pixels[row..row + r[2] as usize]);
    }
    Ok(bytes)
}

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
