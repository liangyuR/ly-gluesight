//! 特征定位评测（阶段 `fixture`，定稿 docs/discussions/fixture-position/final.md §9）：
//! 每张图自动提议定位模板，在同图上跑 glue.locate 选框（E1）；整图刚体 / 成像扰动下的定位精度（E2）；
//! 共同运动下按位姿算的胶偏 d' 是否不变（E3）；合成只移胶的检出（E4）；定位耗时（E5）；复拍对比（E6）。
//! 模板按 PGM 写到评测输出目录，作为 glue.locate 的 base_dir；样本集目录只读。
//! 选出的框都要人工复核：随枪相机下喷嘴、胶枪跟相机走，框进去会让位姿恒为恒等；同相机 NCC 只能剔除整块不动的结构。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};

use super::synth::{self, BeadFrame, Defect, Perturb};
use super::{defect_specs, frame_of, median, ok_class, par_map, pct, place, r3, run, shot_judge, Ctx};
use crate::measure::{ST_INVALID, ST_OK};
use crate::vision::fixture::{deviations, Pose};
use crate::vision::{Engine, FrameImage, ShotMeasurement};

const SEARCH_RADIUS_PX: f64 = 60.0;
const ANGLE_RANGE_DEG: f64 = 3.0;
const ANGLE_STEP_DEG: f64 = 0.5;
const MIN_SCORE: f64 = 0.6;
const SIZES: [u32; 2] = [96, 128];
const STRIDE: u32 = 32;
/// 模板离胶检测走廊（中线 ± 搜索半宽）至少再留这么多
const CORRIDOR_PAD_PX: f64 = 10.0;
/// 模板灰度标准差下限
const MIN_STD: f64 = 12.0;
/// 每张图最多试这么多候选：按纹理取 8 个，再补杠杆臂最短的 4 个
const BY_TEXTURE: usize = 8;
const BY_LEVER: usize = 4;
/// 选框时的试探扰动：平移 (−23, +17) px、旋转 −1.5°；定位误差超过 1 px 或 0.2° 判试探失败。
/// 取一个不在 E2 刚体扰动表里的位姿，免得选框时就把 E2 的某个用例试过一遍；
/// 但试探与 E2 都在模板自己的原图上做，E2/E3 仍是同图内的结果，跨图证据只有 E6。
const PROBE: Perturb = Perturb::Affine { dx: -23.0, dy: 17.0, deg: -1.5, scale: 1.0 };
const PROBE_TOL_PX: f64 = 1.0;
const PROBE_TOL_DEG: f64 = 0.2;
/// 同一相机其他拍照点同一位置的 NCC 达到这个值，判为随相机不动的结构（喷嘴、胶枪），不作候选。
/// 随枪相机下机器人在不同点位看到的工件内容不同，只有装在枪上的东西每张图都在同一处。
const CAMERA_FIXED_NCC: f64 = 0.6;
/// 亮度余量：灰度 ≥ 215 的像素在整图 +40 时会饱和，只记录、不筛
const BRIGHT_LEVEL: u8 = 215;
const TIMING_REPEATS: usize = 5;
/// E6 的复拍对
const RESHOOT: [(&str, &str); 3] = [("S001", "S034"), ("S002", "S035"), ("S003", "S036")];
/// 同相机两张图整图 NCC 达到这个值，视为同一点位的重复拍摄（复拍、重复上下件），不拿来判随相机不动的结构：
/// 否则工件上的真实特征也会因为两张图内容相同被剔掉。2026-10-17 样本集复拍对 0.992–0.996，其他同相机图 ≤ 0.71。
const SAME_VIEW_NCC: f64 = 0.9;
/// 门证据的锁定判据：位姿与真值的模板中心误差 ≤ 2 px 且角度误差 ≤ 0.3° 算锁对，否则算错锁。
/// 看不出角度（angleObservable = false）、真转角又 > 0.3° 的位姿单列为“转角没补偿”，不算错锁也不算锁对：
/// 这时 0° 上的平移本身也跟着偏（2026-10-17 样本集 ±1…2° 时 2…8 px），要先于中心容差判，否则这些调用被算成错锁，
/// 把“另拒看不出角度”的效果算成拦下了错锁。中心偏过 UNCOMPENSATED_MAX_PX 的仍算错锁（不是转角能解释的偏）。
const LOCK_TOL_PX: f64 = 2.0;
const LOCK_TOL_DEG: f64 = 0.3;
const UNCOMPENSATED_MAX_PX: f64 = 10.0;
/// 离线扫的 maxPeakRatio 与 minScore：评测按 maxPeakRatio = 1（不判）调用，locate 里歧义门排在最后、分数门排在最前，
/// 所以对 ok = true 的调用“peakRatio > t 即拒”“score < t 即拒”与在 locate 里设同样的门等价
const PEAK_SWEEP: [f64; 6] = [1.0, 0.95, 0.9, 0.85, 0.8, 0.7];
const SCORE_SWEEP: [f64; 5] = [0.6, 0.7, 0.8, 0.85, 0.9];
/// 示教时按亮像素比例筛框：离线看错锁的框会不会被筛掉，并实跑“每张图取排序最前、亮像素比例低于门限的框”
const BRIGHT_SCREEN: [f64; 4] = [0.05, 0.1, 0.2, 0.3];
/// 单端排除长度超过这些值的拍照点数（定稿“待决定一”护栏的候选；护栏改按 |Δ| 判，|Δ| = 排除长度 − 1 个站距，JSON 键名沿用 maxEndLossMm）
const END_LOSS_LIMITS: [f64; 4] = [3.0, 5.0, 6.0, 8.0];
const REVIEW: &str ="每个选中的模板框都要人工复核：同相机 NCC 只能剔除整块随相机不动的结构，压到一角喷嘴或胶枪的框仍可能漏过；特征是否与胶同面、是否刚性也判断不了";

#[derive(Clone)]
struct Template {
    file: String,
    rect: [u32; 4],
    std: f64,
    /// 杠杆臂 L：模板中心到最远站点（px）
    lever: f64,
    /// 同相机其他拍照点同一位置的最大 NCC
    fixed_ncc: f64,
    /// 灰度 ≥ BRIGHT_LEVEL 的像素比例
    bright: f64,
}

/// 两张图同一矩形的归一化互相关；任一侧没有纹理时为 0。
fn ncc_same_rect(a: &FrameImage, b: &FrameImage, r: [u32; 4]) -> f64 {
    let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for y in r[1]..r[1] + r[3] {
        let row = (y * a.width + r[0]) as usize;
        for (&x, &z) in a.pixels[row..row + r[2] as usize].iter().zip(&b.pixels[row..row + r[2] as usize]) {
            let (x, z) = (x as f64, z as f64);
            sa += x; sb += z; saa += x * x; sbb += z * z; sab += x * z;
        }
    }
    let n = (r[2] * r[3]) as f64;
    let (va, vb) = (saa - sa * sa / n, sbb - sb * sb / n);
    if va < n || vb < n { return 0.0; }
    (sab - sa * sb / n) / (va * vb).sqrt()
}

impl Template {
    fn teach_center(&self) -> [f64; 2] {
        [self.rect[0] as f64 + (self.rect[2] as f64 - 1.0) / 2.0, self.rect[1] as f64 + (self.rect[3] as f64 - 1.0) / 2.0]
    }
}

struct Located {
    pose: Pose,
    /// 定位失败原因：locate 1.1.0 的 reason，旧版只有 message
    why: String,
    ms: f64,
}

/// `angle_range` 为 0 时只找平移。
fn locate(engine: &Engine, dir: &Path, t: &Template, img: &FrameImage, angle_range: f64) -> Result<Located, String> {
    let graph = json!({
        "schemaVersion": 1, "id": "01GLUESIGHTLOCATE00000000",
        "nodes": [
            {"id": "n_load", "op": "io.load_image", "params": {"source": "inputs"}},
            {"id": "n_locate", "op": "glue.locate", "params": {
                "template": t.file, "anchor": [t.rect[0] as f64, t.rect[1] as f64],
                "searchRadius": SEARCH_RADIUS_PX, "angleRange": angle_range, "angleStep": ANGLE_STEP_DEG, "minScore": MIN_SCORE
            }}
        ],
        "edges": [{"id": "e_image", "from": {"node": "n_load", "port": "image"}, "to": {"node": "n_locate", "port": "image"}}],
        "outputs": {"pose": {"node": "n_locate", "port": "pose"}}
    });
    let start = Instant::now();
    let r = engine.run(&graph.to_string(), "eval-locate", &dir.to_string_lossy(), img, &json!({}))?;
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    if r.status() != "ok" { return Err(r.failure()); }
    let rec = r.record("pose").ok_or("定位缺少 pose 输出")?;
    let pose = Pose::from_record(rec)?;
    let why = if pose.ok { "ok".into() } else {
        pose.reason.clone().or_else(|| rec["message"].as_str().filter(|m| !m.is_empty()).map(str::to_string)).unwrap_or_else(|| "未给原因".into())
    };
    Ok(Located { pose, why, ms })
}

fn write_pgm(path: &Path, img: &FrameImage, r: [u32; 4]) -> Result<(), String> {
    let mut bytes = format!("P5\n{} {}\n255\n", r[2], r[3]).into_bytes();
    for y in r[1]..r[1] + r[3] {
        let row = (y * img.width + r[0]) as usize;
        bytes.extend_from_slice(&img.pixels[row..row + r[2] as usize]);
    }
    std::fs::write(path, bytes).map_err(|e| format!("写模板 {} 失败：{e}", path.display()))
}

/// 示教中线每 2 px 取一点，用来量模板到走廊的距离。
fn dense(path: &[[f32; 2]]) -> Vec<[f64; 2]> {
    let mut out = Vec::new();
    for w in path.windows(2) {
        let (a, b) = ([w[0][0] as f64, w[0][1] as f64], [w[1][0] as f64, w[1][1] as f64]);
        let n = ((b[0] - a[0]).hypot(b[1] - a[1]) / 2.0).ceil().max(1.0) as usize;
        out.extend((0..n).map(|i| { let t = i as f64 / n as f64; [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t] }));
    }
    if let Some(p) = path.last() { out.push([p[0] as f64, p[1] as f64]); }
    out
}

fn rect_distance(p: [f64; 2], r: [u32; 4]) -> f64 {
    let (x0, y0, x1, y1) = (r[0] as f64, r[1] as f64, (r[0] + r[2] - 1) as f64, (r[1] + r[3] - 1) as f64);
    let dx = (x0 - p[0]).max(p[0] - x1).max(0.0);
    let dy = (y0 - p[1]).max(p[1] - y1).max(0.0);
    dx.hypot(dy)
}

fn overlap(a: [u32; 4], b: [u32; 4]) -> f64 {
    let w = (a[0] + a[2]).min(b[0] + b[2]) as f64 - a[0].max(b[0]) as f64;
    let h = (a[1] + a[3]).min(b[1] + b[3]) as f64 - a[1].max(b[1]) as f64;
    if w <= 0.0 || h <= 0.0 { return 0.0; }
    w * h / ((a[2] * a[3]).min(b[2] * b[3]) as f64)
}

/// E1 候选：图内且离边 ≥ 搜索半径 + 旋转余量，不压走廊，纹理够，不是随相机不动的结构；
/// 按纹理与杠杆臂各取几个，互相重叠过半的只留一个。返回候选与因随相机不动被剔除的框数。
fn propose(img: &FrameImage, others: &[&FrameImage], path: &[[f32; 2]], stations: &[[f64; 2]], half_px: f64, shot: &str) -> (Vec<Template>, usize) {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut sum = vec![0f64; (w + 1) * (h + 1)];
    let mut sq = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let (mut rs, mut rq) = (0f64, 0f64);
        for x in 0..w {
            let v = img.pixels[y * w + x] as f64;
            rs += v;
            rq += v * v;
            sum[(y + 1) * (w + 1) + x + 1] = sum[y * (w + 1) + x + 1] + rs;
            sq[(y + 1) * (w + 1) + x + 1] = sq[y * (w + 1) + x + 1] + rq;
        }
    }
    let area = |t: &[f64], r: [u32; 4]| {
        let (x0, y0, x1, y1) = (r[0] as usize, r[1] as usize, (r[0] + r[2]) as usize, (r[1] + r[3]) as usize);
        t[y1 * (w + 1) + x1] - t[y0 * (w + 1) + x1] - t[y1 * (w + 1) + x0] + t[y0 * (w + 1) + x0]
    };
    let line = dense(path);
    let mut all = Vec::new();
    let mut fixed_rejected = 0usize;
    for size in SIZES {
        let margin = SEARCH_RADIUS_PX + 0.5 * (size as f64 * std::f64::consts::SQRT_2) * ANGLE_RANGE_DEG.to_radians().sin() + 2.0;
        let lo = margin.ceil() as u32;
        let (Some(xmax), Some(ymax)) = ((w as u32).checked_sub(size + lo), (h as u32).checked_sub(size + lo)) else { continue };
        for y in (lo..=ymax).step_by(STRIDE as usize) {
            for x in (lo..=xmax).step_by(STRIDE as usize) {
                let rect = [x, y, size, size];
                if line.iter().any(|&p| rect_distance(p, rect) <= half_px + CORRIDOR_PAD_PX) { continue; }
                let n = (size * size) as f64;
                let mean = area(&sum, rect) / n;
                let std = (area(&sq, rect) / n - mean * mean).max(0.0).sqrt();
                if std < MIN_STD { continue; }
                let c = [x as f64 + (size as f64 - 1.0) / 2.0, y as f64 + (size as f64 - 1.0) / 2.0];
                let lever = stations.iter().map(|s| (s[0] - c[0]).hypot(s[1] - c[1])).fold(0.0, f64::max);
                let fixed_ncc = others.iter().filter(|o| o.width == img.width && o.height == img.height)
                    .map(|o| ncc_same_rect(img, o, rect)).fold(f64::NAN, f64::max);
                if fixed_ncc >= CAMERA_FIXED_NCC { fixed_rejected += 1; continue; }
                let bright = (y..y + size).map(|yy| img.pixels[(yy * img.width + x) as usize..(yy * img.width + x + size) as usize].iter().filter(|&&v| v >= BRIGHT_LEVEL).count()).sum::<usize>() as f64 / n;
                all.push(Template { file: String::new(), rect, std, lever, fixed_ncc, bright });
            }
        }
    }
    let mut picked: Vec<Template> = Vec::new();
    fn take(sorted: Vec<Template>, quota: usize, picked: &mut Vec<Template>) {
        let mut added = 0;
        for t in sorted {
            if added == quota { break; }
            if picked.iter().any(|p| overlap(p.rect, t.rect) > 0.5) { continue; }
            picked.push(t);
            added += 1;
        }
    }
    let mut by_std = all.clone();
    by_std.sort_by(|a, b| b.std.total_cmp(&a.std));
    take(by_std, BY_TEXTURE, &mut picked);
    let mut by_lever = all;
    by_lever.sort_by(|a, b| a.lever.total_cmp(&b.lever));
    take(by_lever, BY_LEVER, &mut picked);
    for (i, t) in picked.iter_mut().enumerate() {
        t.file = format!("{shot}-{i:02}-{}x{}-{}-{}.pgm", t.rect[2], t.rect[3], t.rect[0], t.rect[1]);
    }
    (picked, fixed_rejected)
}

struct Shot {
    k: usize,
    frame: BeadFrame,
    chosen: Option<Template>,
    /// 通过试探的候选，按选框排序（第一个就是 chosen），亮像素筛框实验用
    passed: Vec<Template>,
    /// 原图上的自定位（作为基线位姿）
    pose0: Option<Pose>,
    summary: Value,
}

/// 原图上按位姿算的胶偏（mm）；无胶或无基准为 None。
fn fixture_d(ctx: &Ctx, k: usize, frame: &BeadFrame, m: &ShotMeasurement, pose: &Pose) -> Vec<Option<f64>> {
    let beads: Vec<Option<[f64; 2]>> = (0..m.st.len()).map(|i| (m.st[i] == ST_OK).then(|| [m.px[i][0] as f64, m.px[i][1] as f64])).collect();
    let mm = ctx.mm_of(k) as f64;
    deviations(&frame.c, &frame.n, &beads, pose)
        .map(|d| d.into_iter().map(|v| v.map(|v| v * mm)).collect())
        .unwrap_or_else(|_| vec![None; m.st.len()])
}

/// 把 d 换成按位姿算的 d'，有胶无基准的站暂按测不了（G1 的 ST_NO_REF 之前的近似）后判定。
fn fixture_verdict(ctx: &Ctx, k: usize, m: &ShotMeasurement, d: &[Option<f64>]) -> super::Judged {
    let mut f = m.clone();
    for i in 0..f.st.len() {
        if f.st[i] != ST_OK { continue; }
        match d[i] {
            Some(v) => f.d[i] = v as f32,
            None => f.st[i] = ST_INVALID,
        }
    }
    shot_judge(ctx.recipe, k, &f)
}

/// 定稿“待决定一”的单端排除长度（m = 0）：两端各按位姿算切向搬移 |Δ|，排除 |Δ| + 1 个站距，取两端较大者（mm）。
/// 只估长度、不改判定：用来核对护栏（maxEndShiftMm，按 |Δ| 判）的默认值会不会在正常搬移下就触发。
fn end_loss_mm(ctx: &Ctx, k: usize, frame: &BeadFrame, pose: &Pose) -> f64 {
    let c = &frame.c;
    let n = c.len();
    if n < 2 { return f64::NAN; }
    let along = |i: usize, j: usize| {
        let (p, q) = (c[i], c[j]);
        let t = [q[0] - p[0], q[1] - p[1]];
        let m = pose.apply(p);
        ((m[0] - p[0]) * t[0] + (m[1] - p[1]) * t[1]).abs() / t[0].hypot(t[1])
    };
    along(0, 1).max(along(n - 1, n - 2)) * ctx.mm_of(k) as f64 + ctx.recipe.spacing as f64
}

fn abs_stats(v: &[f64]) -> Value {
    let a: Vec<f64> = v.iter().map(|x| x.abs()).filter(|x| x.is_finite()).collect();
    json!({"p50": r3(pct(a.clone(), 50.0)), "p95": r3(pct(a.clone(), 95.0)), "max": r3(a.iter().copied().fold(f64::NAN, f64::max)), "n": a.len()})
}

fn dist(v: &[f64]) -> Value {
    let a: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    json!({"n": a.len(), "min": r3(a.iter().copied().fold(f64::NAN, f64::min)), "p05": r3(pct(a.clone(), 5.0)), "p50": r3(pct(a.clone(), 50.0)),
        "p95": r3(pct(a.clone(), 95.0)), "max": r3(a.iter().copied().fold(f64::NAN, f64::max))})
}

/// 锁定判据（LOCK_TOL_PX / LOCK_TOL_DEG）下的一次调用：看不出角度、真转角又超过 0.3° 的是转角没补偿
/// （宿主须当作转角未补偿处理；中心偏 ≤ UNCOMPENSATED_MAX_PX），不算错锁；其余中心超差为错锁；中心对、看得出角度但角度超差单列。
#[derive(Clone, Copy, PartialEq)]
enum Lock { Correct, Wrong, AngleOff, Uncompensated, NoPose }

/// 一次定位调用的门证据。失败的调用也按位姿本身比真值：locate 失败时仍给出最终角度上的位姿（出图时坐标为 null，记为无位姿）。
#[derive(Clone)]
struct Call {
    group: String,
    shot: String,
    ok: bool,
    reason: String,
    score: f64,
    peak: Option<f64>,
    observable: Option<bool>,
    center_err: f64,
    angle_err: f64,
    true_angle: f64,
    bright: f64,
    /// 用例叠加了成像扰动（噪声、亮度、模糊）：这时看不出角度、中心偏 2…10 px 不一定是转角没补偿造成的
    /// （亮度 +40 单独就会在看不出角度的分支上错锁 2…38 px），“转角没补偿”里这部分单独计数
    imaging: bool,
}

impl Call {
    fn new(group: &str, shot: &str, l: &Located, truth: [f64; 2], true_angle: f64, bright: f64, imaging: bool) -> Self {
        let p = &l.pose;
        Self { group: group.into(), shot: shot.into(), ok: p.ok, reason: l.why.clone(), score: p.score, peak: p.peak_ratio, observable: p.angle_observable,
            center_err: (p.center[0] - truth[0]).hypot(p.center[1] - truth[1]), angle_err: p.angle - true_angle, true_angle, bright, imaging }
    }

    fn lock(&self) -> Lock {
        if !self.center_err.is_finite() { return Lock::NoPose; }
        if self.observable == Some(false) && self.true_angle.abs() > LOCK_TOL_DEG && self.center_err <= UNCOMPENSATED_MAX_PX { return Lock::Uncompensated; }
        if self.center_err > LOCK_TOL_PX { return Lock::Wrong; }
        if self.angle_err.abs() <= LOCK_TOL_DEG { Lock::Correct } else { Lock::AngleOff }
    }

    fn json(&self) -> Value {
        json!({"case": self.group, "shot": self.shot, "ok": self.ok, "reason": self.reason, "score": r3(self.score), "peakRatio": self.peak.map(r3),
            "angleObservable": self.observable, "centerErrPx": r3(self.center_err), "angleErrDeg": r3(self.angle_err), "brightFrac": r3(self.bright)})
    }
}

/// 一组调用上的门效果：发布的默认门（minScore 0.6 + 贴边门，maxPeakRatio = 1），再离线扫 maxPeakRatio 与 minScore。
fn gate_report(calls: &[&Call]) -> Value {
    let of = |l: Lock| -> Vec<&Call> { calls.iter().copied().filter(|c| c.lock() == l).collect() };
    let (wrong, angle_off, correct) = (of(Lock::Wrong), of(Lock::AngleOff), of(Lock::Correct));
    let effect = |reject: &dyn Fn(&Call) -> bool| {
        let n = |set: &[&Call]| set.iter().filter(|c| reject(c)).count();
        json!({"wrongCaught": n(&wrong), "wrongAccepted": wrong.len() - n(&wrong), "angleOffCaught": n(&angle_off), "correctRejected": n(&correct)})
    };
    let by_reason = |set: &[&Call]| {
        let mut m = BTreeMap::<String, usize>::new();
        for c in set.iter().filter(|c| !c.ok) { *m.entry(c.reason.clone()).or_default() += 1; }
        m
    };
    let peak_sweep: Vec<Value> = PEAK_SWEEP.iter().map(|&t| {
        let mut v = effect(&|c: &Call| !c.ok || c.peak.is_some_and(|p| p > t));
        v["maxPeakRatio"] = json!(t);
        v
    }).collect();
    let score_sweep: Vec<Value> = SCORE_SWEEP.iter().map(|&t| {
        let mut v = effect(&|c: &Call| !c.ok || !(c.score >= t));
        v["minScore"] = json!(t);
        v
    }).collect();
    let accepted = |set: &[&Call], f: &dyn Fn(&Call) -> Option<f64>| dist(&set.iter().filter(|c| c.ok).filter_map(|c| f(c)).collect::<Vec<_>>());
    let mut shipped = effect(&|c: &Call| !c.ok);
    shipped["wrongCaughtByReason"] = json!(by_reason(&wrong));
    shipped["correctRejectedByReason"] = json!(by_reason(&correct));
    // 宿主另把“看不出角度”当失败时的效果（locate 不这么做，只看它与错锁是否相关）
    shipped["ifRejectUnobservable"] = effect(&|c: &Call| !c.ok || c.observable == Some(false));
    json!({"calls": calls.len(), "correct": correct.len(), "wrong": wrong.len(), "wrongOver10Px": wrong.iter().filter(|c| c.center_err > 10.0).count(),
        "angleOff": angle_off.len(), "uncompensated": of(Lock::Uncompensated).len(),
        "uncompensatedOver2Px": of(Lock::Uncompensated).iter().filter(|c| c.center_err > LOCK_TOL_PX).count(),
        "uncompensatedWithImaging": of(Lock::Uncompensated).iter().filter(|c| c.imaging).count(),
        "uncompensatedWithImagingOver2Px": of(Lock::Uncompensated).iter().filter(|c| c.imaging && c.center_err > LOCK_TOL_PX).count(),
        "noPose": of(Lock::NoPose).len(),
        "shipped": shipped, "peakRatioSweep": peak_sweep, "minScoreSweep": score_sweep,
        "acceptedWrongPeakRatio": accepted(&wrong, &|c| c.peak), "acceptedCorrectPeakRatio": accepted(&correct, &|c| c.peak),
        "acceptedWrongScore": accepted(&wrong, &|c| Some(c.score)), "acceptedCorrectScore": accepted(&correct, &|c| Some(c.score)),
        "wrongLocks": wrong.iter().take(80).map(|c| c.json()).collect::<Vec<_>>(),
        "angleOffCalls": angle_off.iter().take(40).map(|c| c.json()).collect::<Vec<_>>()})
}

/// GLUESIGHT_EVAL_TEMPLATES：上一次评测的 results.json，按其中每个拍照点选中的框固定模板（拍照点 → rect）。
/// 候选网格只由图像决定，同一样本集上框的坐标可以直接复用。
fn templates_path() -> Option<String> {
    std::env::var("GLUESIGHT_EVAL_TEMPLATES").ok().filter(|v| !v.trim().is_empty())
}

fn pinned_templates() -> Option<BTreeMap<String, [u32; 4]>> {
    let path = templates_path()?;
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 GLUESIGHT_EVAL_TEMPLATES {path} 失败：{e}"));
    let doc: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("GLUESIGHT_EVAL_TEMPLATES {path} 无效：{e}"));
    Some(doc["fixture"]["templates"].as_array().into_iter().flatten().filter_map(|t| {
        let r: Vec<u32> = t["chosen"]["rect"].as_array()?.iter().filter_map(|v| v.as_u64().map(|v| v as u32)).collect();
        (r.len() == 4).then(|| (t["shot"].as_str().unwrap_or_default().to_string(), [r[0], r[1], r[2], r[3]]))
    }).collect())
}

/// 选不出模板的拍照点也返回一行摘要（`Err`），保证 E1 表里每个示教过的拍照点都有一行。
fn choose(ctx: &Ctx, dir: &Path, k: usize, base: &ShotMeasurement, pinned: Option<[u32; 4]>) -> Result<Shot, Value> {
    let s = &ctx.samples[k];
    let Some(frame) = frame_of(ctx.recipe, base) else {
        return Err(json!({"shot": s.shot_id, "camera": s.camera, "candidates": [], "error": "基准测量没有一站量到胶边，建不了 BeadFrame"}));
    };
    let half = ctx.recipe.shot_detect(k).search_mm as f64 / ctx.mm_of(k) as f64;
    // 同相机的其他拍照点，去掉整图几乎相同的重复拍摄（同一点位）
    let (w, h) = (s.image.width, s.image.height);
    let (mut others, mut same_view) = (Vec::new(), Vec::new());
    for (j, o) in ctx.samples.iter().enumerate() {
        if j == k || o.camera != s.camera || o.image.width != w || o.image.height != h { continue; }
        if ncc_same_rect(&s.image, &o.image, [0, 0, w, h]) >= SAME_VIEW_NCC { same_view.push(o.shot_id.clone()); } else { others.push(&o.image); }
    }
    let (mut candidates, fixed_rejected) = propose(&s.image, &others, &ctx.recipe.shots[k].path, &frame.c, half, &s.shot_id);
    let probe_img = PROBE.apply(&s.image, k as u64);
    let mut rows = Vec::new();
    let mut ranked: Vec<(bool, f64, i64, f64, usize, Pose)> = Vec::new();
    for (ci, t) in candidates.iter().enumerate() {
        if let Err(e) = write_pgm(&dir.join(&t.file), &s.image, t.rect) { rows.push(json!({"rect": t.rect, "error": e})); continue; }
        let own = locate(ctx.engine, dir, t, &s.image, ANGLE_RANGE_DEG);
        let probe = locate(ctx.engine, dir, t, &probe_img, ANGLE_RANGE_DEG);
        let truth = PROBE.moved(t.teach_center(), w, h);
        let (perr, aerr, pscore, pass) = match &probe {
            Ok(p) if p.pose.ok => {
                let e = (p.pose.center[0] - truth[0]).hypot(p.pose.center[1] - truth[1]);
                let Perturb::Affine { deg, .. } = PROBE else { unreachable!() };
                let a = p.pose.angle - deg;
                (e, a, p.pose.score, e <= PROBE_TOL_PX && a.abs() <= PROBE_TOL_DEG)
            }
            Ok(p) => (f64::NAN, f64::NAN, p.pose.score, false),
            Err(_) => (f64::NAN, f64::NAN, f64::NAN, false),
        };
        let own_ok = own.as_ref().is_ok_and(|o| o.pose.ok);
        let peak = own.as_ref().ok().and_then(|o| o.pose.peak_ratio);
        rows.push(json!({"rect": t.rect, "std": r3(t.std), "leverPx": r3(t.lever), "leverMm": r3(t.lever * ctx.mm_of(k) as f64),
            "cameraFixedNcc": r3(t.fixed_ncc), "brightFrac": r3(t.bright),
            "score": own.as_ref().ok().map(|o| r3(o.pose.score)), "peakRatio": peak.map(r3), "selfOk": own_ok,
            "selfAngleObservable": own.as_ref().ok().and_then(|o| o.pose.angle_observable),
            "probeAngleObservable": probe.as_ref().ok().and_then(|p| p.pose.angle_observable),
            "selfError": own.as_ref().err(), "probeScore": r3(pscore), "probeErrPx": r3(perr), "probeAngleErrDeg": r3(aerr), "probePass": pass && own_ok}));
        if let Ok(o) = own {
            // 排序：试探通过 > peakRatio 小（未知视为相同）> 杠杆臂短（100 px 一档）> 试探分数高
            ranked.push((pass && own_ok, peak.unwrap_or(0.0), (t.lever / 100.0).floor() as i64, pscore, ci, o.pose));
        }
    }
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)).then(b.3.total_cmp(&a.3)));
    // 固定框时亮像素筛框只看固定的那个框（不再按这一版运行库的排序重选），否则固定框运行的筛框结果与自动选框逐项相同
    let passed: Vec<Template> = match pinned {
        Some(rect) => candidates.iter().filter(|t| t.rect == rect).cloned().collect(),
        None => ranked.iter().filter(|r| r.0).map(|r| candidates[r.4].clone()).collect(),
    };
    // 固定框（GLUESIGHT_EVAL_TEMPLATES）时不看排序与试探，只要原图上能自定位；用来在另一版运行库上复用同一组框做对照
    let best = match pinned {
        Some(rect) => ranked.into_iter().find(|r| candidates[r.4].rect == rect),
        None => ranked.into_iter().next().filter(|r| r.0),
    };
    let (chosen, pose0) = match best {
        Some((_, _, _, _, ci, pose)) => (Some(candidates.swap_remove(ci)), Some(pose)),
        None => (None, None),
    };
    let error = match (&chosen, rows.is_empty(), pinned) {
        (Some(_), _, _) => Value::Null,
        (None, true, _) => json!(format!("没有候选框（另有 {fixed_rejected} 个框判为随相机不动被剔除）")),
        (None, false, Some(rect)) => json!(format!("固定框 {rect:?} 不在候选里或原图上定位失败")),
        (None, false, None) => json!("没有通过试探定位的候选框"),
    };
    let summary = json!({"shot": s.shot_id, "camera": s.camera, "candidates": rows, "rejectedCameraFixed": fixed_rejected, "comparedShots": others.len(), "sameViewShots": same_view,
        "chosen": chosen.as_ref().map(|t| json!({"rect": t.rect, "file": t.file, "std": r3(t.std), "leverPx": r3(t.lever),
            "leverMm": r3(t.lever * ctx.mm_of(k) as f64), "score": pose0.as_ref().map(|p| r3(p.score)),
            "cameraFixedNcc": r3(t.fixed_ncc), "brightFrac": r3(t.bright),
            "peakRatio": pose0.as_ref().and_then(|p| p.peak_ratio).map(r3),
            "angleObservable": pose0.as_ref().and_then(|p| p.angle_observable), "needsReview": true})),
        "passedCandidates": passed.len(), "error": error});
    Ok(Shot { k, frame, chosen, passed, pose0, summary })
}

fn write_overlay(path: &Path, ctx: &Ctx, shot: &Shot, candidates: &[Value]) {
    let img = &ctx.samples[shot.k].image;
    let mut rgb = image::RgbImage::from_fn(img.width, img.height, |x, y| {
        let v = img.pixels[(y * img.width + x) as usize];
        image::Rgb([v, v, v])
    });
    let (w, h) = (img.width as i64, img.height as i64);
    let mut dot = |x: f64, y: f64, c: [u8; 3], r: i64| {
        for dy in -r..=r {
            for dx in -r..=r {
                let (px, py) = (x.round() as i64 + dx, y.round() as i64 + dy);
                if px >= 0 && py >= 0 && px < w && py < h { rgb.put_pixel(px as u32, py as u32, image::Rgb(c)); }
            }
        }
    };
    let half = ctx.recipe.shot_detect(shot.k).search_mm as f64 / ctx.mm_of(shot.k) as f64;
    for p in dense(&ctx.recipe.shots[shot.k].path) { dot(p[0], p[1], [0, 200, 0], 1); }
    for (c, n) in shot.frame.c.iter().zip(&shot.frame.n) {
        for s in [-half, half] { dot(c[0] + n[0] * s, c[1] + n[1] * s, [0, 220, 255], 1); }
    }
    let mut rect = |r: [u32; 4], c: [u8; 3], t: i64| {
        let (x0, y0, x1, y1) = (r[0] as f64, r[1] as f64, (r[0] + r[2] - 1) as f64, (r[1] + r[3] - 1) as f64);
        for i in 0..=r[2] { let x = x0 + i as f64; dot(x, y0, c, t); dot(x, y1, c, t); }
        for i in 0..=r[3] { let y = y0 + i as f64; dot(x0, y, c, t); dot(x1, y, c, t); }
    };
    for cand in candidates {
        if let Some(r) = cand["rect"].as_array().filter(|a| a.len() == 4) {
            let r: Vec<u32> = r.iter().filter_map(|v| v.as_u64().map(|v| v as u32)).collect();
            if r.len() == 4 { rect([r[0], r[1], r[2], r[3]], [255, 140, 0], 0); }
        }
    }
    if let Some(t) = &shot.chosen {
        rect(t.rect, [255, 255, 0], 1);
        let c = t.teach_center();
        if let Some(far) = shot.frame.c.iter().max_by(|a, b| (a[0] - c[0]).hypot(a[1] - c[1]).total_cmp(&(b[0] - c[0]).hypot(b[1] - c[1]))) {
            let n = ((far[0] - c[0]).hypot(far[1] - c[1]) / 4.0).ceil() as usize;
            for i in 0..=n { let f = i as f64 / n.max(1) as f64; dot(c[0] + (far[0] - c[0]) * f, c[1] + (far[1] - c[1]) * f, [255, 60, 60], 1); }
            dot(far[0], far[1], [255, 0, 0], 3);
        }
    }
    let _ = rgb.save(path);
}

fn rigid_cases() -> Vec<(&'static str, Perturb)> {
    let affine = |dx: f64, dy: f64, deg: f64, scale: f64| Perturb::Affine { dx, dy, deg, scale };
    vec![
        ("平移 +5,0", affine(5.0, 0.0, 0.0, 1.0)), ("平移 0,+5", affine(0.0, 5.0, 0.0, 1.0)), ("平移 +10,+10", affine(10.0, 10.0, 0.0, 1.0)),
        ("平移 +15,0", affine(15.0, 0.0, 0.0, 1.0)), ("平移 0,−15", affine(0.0, -15.0, 0.0, 1.0)), ("平移 +25,0", affine(25.0, 0.0, 0.0, 1.0)),
        ("平移 +40,0", affine(40.0, 0.0, 0.0, 1.0)), ("旋转 +1°", affine(0.0, 0.0, 1.0, 1.0)), ("旋转 −1°", affine(0.0, 0.0, -1.0, 1.0)),
        ("旋转 +2°", affine(0.0, 0.0, 2.0, 1.0)), ("旋转 −2°", affine(0.0, 0.0, -2.0, 1.0)), ("缩放 ×1.03", affine(0.0, 0.0, 0.0, 1.03)),
        ("平移 +20,−10 旋转 +1.5°", affine(20.0, -10.0, 1.5, 1.0)),
    ]
}

fn imaging_cases() -> Vec<(&'static str, Perturb)> {
    vec![
        ("噪声 σ=3", Perturb::Noise(3.0)), ("噪声 σ=6", Perturb::Noise(6.0)), ("亮度 −40", Perturb::Brightness(-40.0)),
        ("亮度 +40", Perturb::Brightness(40.0)), ("高斯模糊 σ=1", Perturb::Blur(1.0)), ("高斯模糊 σ=2", Perturb::Blur(2.0)),
    ]
}

struct Case {
    name: String,
    rigid: Option<Perturb>,
    imaging: Option<Perturb>,
    /// glue.locate 的 angleRange；0 = 只找平移
    angle_range: f64,
}

/// 刚体扰动单独、成像扰动单独、三种刚体 × 全部成像扰动；另把成像扰动与平移 +15,0 各跑一遍“只找平移”，
/// 用来区分角度估计本身带来的误差。
fn cases() -> Vec<Case> {
    let rigid = rigid_cases();
    let imaging = imaging_cases();
    let case = |name: String, rigid: Option<Perturb>, imaging: Option<Perturb>, angle_range: f64| Case { name, rigid, imaging, angle_range };
    let mut out: Vec<Case> = rigid.iter().map(|(n, p)| case(n.to_string(), Some(*p), None, ANGLE_RANGE_DEG)).collect();
    out.extend(imaging.iter().map(|(n, p)| case(n.to_string(), None, Some(*p), ANGLE_RANGE_DEG)));
    for (rn, rp) in rigid.iter().filter(|(n, _)| ["平移 +15,0", "旋转 +2°", "平移 +20,−10 旋转 +1.5°"].contains(n)) {
        out.extend(imaging.iter().map(|(n, p)| case(format!("{rn} + {n}"), Some(*rp), Some(*p), ANGLE_RANGE_DEG)));
    }
    out.extend(imaging.iter().map(|(n, p)| case(format!("{n}（只找平移）"), None, Some(*p), 0.0)));
    let (sn, sp) = rigid.iter().find(|(n, _)| *n == "平移 +15,0").copied().unwrap();
    out.push(case(format!("{sn}（只找平移）"), Some(sp), None, 0.0));
    out.extend(imaging.iter().take(2).map(|(n, p)| case(format!("{sn} + {n}（只找平移）"), Some(sp), Some(*p), 0.0)));
    // 真转动时只找平移：转角整个没补偿，量出 v1 默认 angleRange = 0 的代价
    out.extend(rigid.iter().filter(|(n, _)| ["旋转 +1°", "旋转 −1°", "旋转 +2°", "平移 +20,−10 旋转 +1.5°"].contains(n))
        .map(|(n, p)| case(format!("{n}（只找平移）"), Some(*p), None, 0.0)));
    // 超出 searchRadius / angleRange 的真位姿：看贴边门（at_search_edge / at_angle_edge）拦不拦得住；放在最后，不改前面用例的随机种子
    let affine = |dx: f64, deg: f64| Some(Perturb::Affine { dx, dy: 0.0, deg, scale: 1.0 });
    out.push(case("平移 +70,0（超出搜索半径）".into(), affine(70.0, 0.0), None, ANGLE_RANGE_DEG));
    out.push(case("旋转 +4°（超出角度范围）".into(), affine(0.0, 4.0), None, ANGLE_RANGE_DEG));
    out
}

/// 扰动定义与 Pose 约定是否一致：scale = 1 的 Affine 等价于 angle = deg、teachCenter = 图心、center = 图心 + (dx, dy)。
fn convention_check(w: u32, h: u32) -> f64 {
    let mut worst = 0f64;
    for (_, p) in rigid_cases() {
        let Perturb::Affine { dx, dy, deg, scale } = p else { continue };
        if scale != 1.0 { continue; }
        let c = [w as f64 * 0.5, h as f64 * 0.5];
        let pose = Pose { ok: true, score: 1.0, angle: deg, center: [c[0] + dx, c[1] + dy], teach_center: c, peak_ratio: None, reason: None, angle_observable: None };
        for q in [[0.0, 0.0], [w as f64 - 1.0, 0.0], [123.0, 877.0], [w as f64 - 1.0, h as f64 - 1.0]] {
            let (a, b) = (pose.apply(q), p.moved(q, w, h));
            worst = worst.max((a[0] - b[0]).hypot(a[1] - b[1]));
        }
    }
    worst
}

struct CaseRow {
    located: Result<Located, String>,
    trans_err: f64,
    angle_err: f64,
    far_err: f64,
    truth_shift: f64,
    call: Option<Call>,
    /// 基准判 OK 的拍照点：(Δd' mm, Δd mm, 有胶无基准站数, 定位后判 NG 时的判定与原因, 静态判定是否 OK)
    /// d′ − d₀、静态 Δd、无基准站数、定位后判定（非 OK 时）、静态是否 OK、单端排除长度（mm）
    inv: Option<Result<(Vec<f64>, Vec<f64>, usize, Option<String>, bool, f64), String>>,
}

pub fn evaluate(ctx: &Ctx, base: &[Result<ShotMeasurement, String>]) -> Value {
    let root = ctx.cfg.out.join("fixture");
    let dir = root.join("templates");
    let overlay_dir = root.join("overlay");
    for d in [&dir, &overlay_dir] { let _ = std::fs::create_dir_all(d); }
    let mm_k = |k: usize| ctx.mm_of(k) as f64;
    let taught: Vec<usize> = (0..ctx.samples.len()).filter(|&k| !ctx.recipe.shots[k].skip && base[k].is_ok()).collect();
    let (w, h) = (ctx.samples[0].image.width, ctx.samples[0].image.height);
    let convention = convention_check(w, h);
    assert!(convention < 1e-6, "扰动定义与 Pose 约定不一致：{convention} px");

    // E1：选模板
    let pins = pinned_templates();
    let chosen: Vec<Result<Shot, Value>> = par_map(&taught, ctx.cfg.workers, |_, &k| {
        let pin = pins.as_ref().map(|p| p.get(&ctx.samples[k].shot_id).copied().unwrap_or([0; 4]));
        choose(ctx, &dir, k, base[k].as_ref().unwrap(), pin)
    });
    let templates: Vec<Value> = chosen.iter().map(|c| match c { Ok(s) => s.summary.clone(), Err(v) => v.clone() }).collect();
    let shots: Vec<Shot> = chosen.into_iter().filter_map(Result::ok).collect();
    for s in &shots {
        write_overlay(&overlay_dir.join(format!("{}.png", ctx.samples[s.k].shot_id)), ctx, s, s.summary["candidates"].as_array().map_or(&[][..], Vec::as_slice));
    }
    let ready: Vec<&Shot> = shots.iter().filter(|s| s.chosen.is_some()).collect();
    let peak_known = shots.iter().flat_map(|s| s.summary["candidates"].as_array().into_iter().flatten()).any(|c| !c["peakRatio"].is_null());
    let base_ok = |k: usize| base[k].as_ref().is_ok_and(|m| ok_class(shot_judge(ctx.recipe, k, m).verdict));

    // 原图：按位姿算的 d' 与静态 d 是否一致、定位后判定
    let mut agree = Vec::new();
    let (mut base_noref, mut base_fixture_ng) = (0usize, Vec::new());
    let d0: BTreeMap<usize, Vec<Option<f64>>> = ready.iter().map(|s| {
        let m = base[s.k].as_ref().unwrap();
        let d = fixture_d(ctx, s.k, &s.frame, m, s.pose0.as_ref().unwrap());
        for i in 0..m.st.len() {
            if m.st[i] != ST_OK { continue; }
            match d[i] { Some(v) => agree.push(v - m.d[i] as f64), None => base_noref += 1 }
        }
        let j = fixture_verdict(ctx, s.k, m, &d);
        if base_ok(s.k) && !ok_class(j.verdict) { base_fixture_ng.push(format!("{} {:?} {}", ctx.samples[s.k].shot_id, j.verdict, j.reason)); }
        (s.k, d)
    }).collect();

    // E2 + E3
    let list = cases();
    let jobs: Vec<(usize, usize)> = (0..list.len()).flat_map(|c| ready.iter().map(move |s| (c, s.k))).collect();
    let by_k: BTreeMap<usize, &Shot> = ready.iter().map(|s| (s.k, *s)).collect();
    let rows: Vec<CaseRow> = par_map(&jobs, ctx.cfg.workers, |_, &(c, k)| {
        let s = by_k[&k];
        let t = s.chosen.as_ref().unwrap();
        let Case { rigid, imaging, angle_range, .. } = &list[c];
        let seed = (k as u64) << 8 | c as u64;
        let orig = &ctx.samples[k].image;
        let moved_img = rigid.map(|p| p.apply(orig, seed));
        let src = moved_img.as_ref().unwrap_or(orig);
        let imaged = imaging.map(|p| p.apply(src, seed ^ 0x55));
        let img = imaged.as_ref().unwrap_or(src);
        let moved = |q: [f64; 2]| rigid.map_or(q, |p| p.moved(q, w, h));
        let deg = match rigid { Some(Perturb::Affine { deg, .. }) => *deg, _ => 0.0 };
        let tc = t.teach_center();
        let truth = moved(tc);
        let far = s.frame.c.iter().copied().max_by(|a, b| (a[0] - tc[0]).hypot(a[1] - tc[1]).total_cmp(&(b[0] - tc[0]).hypot(b[1] - tc[1]))).unwrap();
        let located = locate(ctx.engine, &dir, t, img, *angle_range);
        let (trans_err, angle_err, far_err) = match &located {
            Ok(l) if l.pose.ok => {
                let (q, fq) = (l.pose.apply(tc), l.pose.apply(far));
                let (tq, tf) = (truth, moved(far));
                debug_assert!((q[0] - l.pose.center[0]).hypot(q[1] - l.pose.center[1]) < 1e-6);
                ((q[0] - tq[0]).hypot(q[1] - tq[1]), l.pose.angle - deg, (fq[0] - tf[0]).hypot(fq[1] - tf[1]))
            }
            _ => (f64::NAN, f64::NAN, f64::NAN),
        };
        let inv = match &located {
            Ok(l) if l.pose.ok && base_ok(k) => Some(run(ctx.engine, ctx.recipe, k, img).map(|m| {
                let b = base[k].as_ref().unwrap();
                let d = fixture_d(ctx, k, &s.frame, &m, &l.pose);
                let (mut dd, mut ds, mut noref) = (Vec::new(), Vec::new(), 0usize);
                for i in 0..m.st.len() {
                    if m.st[i] == ST_OK && d[i].is_none() { noref += 1; }
                    if let (Some(a), Some(z)) = (d[i], d0[&k][i]) { dd.push(a - z); }
                    if m.st[i] == ST_OK && b.st[i] == ST_OK { ds.push((m.d[i] - b.d[i]) as f64); }
                }
                let fj = fixture_verdict(ctx, k, &m, &d);
                let fixture_ng = (!ok_class(fj.verdict)).then(|| format!("{} {:?} {}", ctx.samples[k].shot_id, fj.verdict, fj.reason));
                let static_ok = ok_class(shot_judge(ctx.recipe, k, &m).verdict);
                (dd, ds, noref, fixture_ng, static_ok, end_loss_mm(ctx, k, &s.frame, &l.pose))
            })),
            _ => None,
        };
        // 真转动时只找平移的用例是模型不符（转角本来就不补），不算定位门的证据
        let call = located.as_ref().ok().filter(|_| !(*angle_range == 0.0 && deg != 0.0)).map(|l| Call::new(&list[c].name, &ctx.samples[k].shot_id, l, truth, deg, t.bright, imaging.is_some()));
        CaseRow { located, trans_err, angle_err, far_err, truth_shift: (truth[0] - tc[0]).hypot(truth[1] - tc[1]), call, inv }
    });
    let mut accuracy = Vec::new();
    let mut invariance = Vec::new();
    for (c, Case { name, rigid, imaging, angle_range }) in list.iter().enumerate() {
        let kind = match (rigid, imaging) { (Some(_), Some(_)) => "叠加", (Some(_), None) => "刚体", _ => "成像" };
        let mut at_edge = 0usize;
        let mut per_shot = Vec::new();
        let (mut ok, mut n, mut reasons, mut te, mut ae, mut fe, mut fe_mm, mut shift) = (0usize, 0usize, BTreeMap::<String, usize>::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), 0f64);
        let (mut dd, mut ds, mut noref, mut fixture_ng, mut static_ng, mut inv_shots, mut inv_err) = (Vec::new(), Vec::new(), 0usize, Vec::new(), 0usize, 0usize, Vec::new());
        let mut inv_per_shot = Vec::new();
        let mut end_loss = Vec::new();
        // 看不出角度（angleObservable = false）的成功定位：转角没补偿；另按可观测的子集统计角度与 Δd'
        let (mut unobs, mut ae_obs, mut fe_obs_mm, mut dd_obs, mut inv_unobs) = (0usize, Vec::new(), Vec::new(), Vec::new(), 0usize);
        for (&(cc, k), r) in jobs.iter().zip(&rows) {
            if cc != c { continue; }
            n += 1;
            shift = shift.max(r.truth_shift);
            match &r.located {
                Ok(l) => {
                    *reasons.entry(l.why.clone()).or_default() += 1;
                    if l.pose.ok { ok += 1; }
                    // locate 1.1.0 会把角度贴扫描边界判 at_angle_edge；1.0.0 照样报成功
                    if l.pose.ok && *angle_range > 0.0 && l.pose.angle.abs() >= angle_range - 1e-3 { at_edge += 1; }
                }
                Err(e) => *reasons.entry(format!("运行失败：{e}")).or_default() += 1,
            }
            let observable = r.located.as_ref().ok().and_then(|l| l.pose.angle_observable);
            if r.located.as_ref().is_ok_and(|l| l.pose.ok) && observable == Some(false) { unobs += 1; }
            if r.trans_err.is_finite() {
                te.push(r.trans_err); ae.push(r.angle_err); fe.push(r.far_err); fe_mm.push(r.far_err * mm_k(k));
                if observable != Some(false) { ae_obs.push(r.angle_err); fe_obs_mm.push(r.far_err * mm_k(k)); }
            }
            per_shot.push(json!({"shot": ctx.samples[k].shot_id, "ok": r.located.as_ref().is_ok_and(|l| l.pose.ok),
                "score": r.located.as_ref().ok().map(|l| r3(l.pose.score)),
                "peakRatio": r.located.as_ref().ok().and_then(|l| l.pose.peak_ratio).map(r3), "why": r.located.as_ref().ok().map(|l| l.why.clone()),
                "angleObservable": observable, "transErrPx": r3(r.trans_err), "angleErrDeg": r3(r.angle_err), "farErrMm": r3(r.far_err * mm_k(k))}));
            match &r.inv {
                Some(Ok((a, b, nr, fok, sok, el))) => {
                    inv_shots += 1;
                    end_loss.push(*el);
                    inv_per_shot.push(json!({"shot": ctx.samples[k].shot_id, "fixtureAbsDiffMm": abs_stats(a), "staticAbsDiffMm": abs_stats(b), "noRefStations": nr, "angleObservable": observable}));
                    dd.extend(a); ds.extend(b); noref += nr;
                    if observable == Some(false) { inv_unobs += 1; } else { dd_obs.extend(a); }
                    if let Some(detail) = fok { fixture_ng.push(detail.clone()); }
                    if !sok { static_ng += 1; }
                }
                Some(Err(e)) => inv_err.push(format!("{}：{e}", ctx.samples[k].shot_id)),
                None => {}
            }
        }
        accuracy.push(json!({"name": name, "kind": kind, "angleRangeDeg": angle_range, "shots": n, "ok": ok, "okRate": r3(ok as f64 / n.max(1) as f64), "reasons": reasons, "atAngleEdge": at_edge, "angleUnobservable": unobs,
            "angleErrObservableDeg": abs_stats(&ae_obs), "farStationErrObservableMm": abs_stats(&fe_obs_mm),
            "truthShiftMaxPx": r3(shift), "transErrPx": abs_stats(&te), "angleErrDeg": abs_stats(&ae), "angleErrMedianDeg": r3(median(ae.clone())),
            "farStationErrPx": abs_stats(&fe), "farStationErrMm": abs_stats(&fe_mm), "perShot": per_shot}));
        invariance.push(json!({"name": name, "kind": kind, "angleRangeDeg": angle_range, "shots": inv_shots, "fixtureAbsDiffMm": abs_stats(&dd), "staticAbsDiffMm": abs_stats(&ds),
            "angleUnobservableShots": inv_unobs, "fixtureAbsDiffObservableMm": abs_stats(&dd_obs), "noRefStations": noref, "endLossMm": abs_stats(&end_loss),
            "endLossOverMm": END_LOSS_LIMITS.iter().map(|&t| json!({"maxEndLossMm": t, "shots": end_loss.iter().filter(|&&x| x > t).count()})).collect::<Vec<_>>(),
            "fixtureNg": fixture_ng.len(), "fixtureNgShots": fixture_ng, "staticNg": static_ng, "errors": inv_err, "perShot": inv_per_shot}));
    }

    // E4：只移胶
    let specs: Vec<_> = defect_specs().into_iter().filter(|s| matches!(s.kind, Defect::Offset(_))).collect();
    let spacing = ctx.recipe.spacing as f64;
    let e4_shots: Vec<&Shot> = ready.iter().copied().filter(|s| base_ok(s.k)).collect();
    let e4_jobs: Vec<(usize, usize)> = (0..specs.len()).flat_map(|d| e4_shots.iter().map(move |s| (d, s.k))).collect();
    let e4: Vec<Option<Result<(bool, bool, f64, bool, f64, String), String>>> = par_map(&e4_jobs, ctx.cfg.workers, |_, &(d, k)| {
        let spec = &specs[d];
        let s = by_k[&k];
        let t = s.chosen.as_ref().unwrap();
        let b = base[k].as_ref().unwrap();
        let ramp = (spec.ramp_mm / spacing).round() as usize;
        let stations = ((spec.len_mm / spacing).round() as usize).max(1);
        let (a, bb) = place(b, stations, ramp)?;
        let Defect::Offset(v) = spec.kind else { unreachable!() };
        let img = synth::defect(&ctx.samples[k].image, &s.frame, a as f64, bb as f64, Defect::Offset(v / mm_k(k)), ramp as f64);
        let touched = (t.rect[1]..t.rect[1] + t.rect[3]).any(|y| {
            let row = (y * img.width + t.rect[0]) as usize;
            img.pixels[row..row + t.rect[2] as usize] != ctx.samples[k].image.pixels[row..row + t.rect[2] as usize]
        });
        Some((|| {
            let l = locate(ctx.engine, &dir, t, &img, ANGLE_RANGE_DEG)?;
            if !l.pose.ok { return Err(format!("定位失败：{}", l.why)); }
            let p0 = s.pose0.as_ref().unwrap();
            let pose_shift = (l.pose.center[0] - p0.center[0]).hypot(l.pose.center[1] - p0.center[1]);
            let m = run(ctx.engine, ctx.recipe, k, &img)?;
            let d = fixture_d(ctx, k, &s.frame, &m, &l.pose);
            let rec: Vec<f64> = (a..=bb).filter_map(|i| Some(d[i]? - d0[&k][i]?)).collect();
            let fj = fixture_verdict(ctx, k, &m, &d);
            let sj = shot_judge(ctx.recipe, k, &m);
            Ok((!ok_class(fj.verdict), !ok_class(sj.verdict), median(rec) - v, touched, pose_shift, format!("{:?}", fj.verdict)))
        })())
    });
    let mut bead_offset = Vec::new();
    for (d, spec) in specs.iter().enumerate() {
        let (mut placed, mut f_hit, mut s_hit, mut errs, mut rec, mut touched, mut shift, mut classes) = (0usize, 0usize, 0usize, Vec::new(), Vec::new(), 0usize, 0f64, BTreeMap::<String, usize>::new());
        for (&(dd, k), r) in e4_jobs.iter().zip(&e4) {
            if dd != d { continue; }
            let Some(r) = r else { continue };
            placed += 1;
            match r {
                Ok((fng, sng, e, t, ps, class)) => {
                    if *fng == spec.ng { f_hit += 1; }
                    if *sng == spec.ng { s_hit += 1; }
                    rec.push(*e);
                    if *t { touched += 1; }
                    shift = shift.max(*ps);
                    *classes.entry(class.clone()).or_default() += 1;
                }
                Err(e) => errs.push(format!("{}：{e}", ctx.samples[k].shot_id)),
            }
        }
        bead_offset.push(json!({"name": spec.name, "ng": spec.ng, "placed": placed, "fixtureAsExpected": f_hit, "staticAsExpected": s_hit,
            "fixtureClasses": classes, "recoveredErrMm": abs_stats(&rec), "recoveredErrMedianMm": r3(median(rec.clone())),
            "templateTouched": touched, "poseShiftMaxPx": r3(shift), "errors": errs}));
    }

    // E5：单线程逐张计时
    let mut all_ms = Vec::new();
    let per_shot: Vec<Value> = ready.iter().map(|s| {
        let t = s.chosen.as_ref().unwrap();
        let ms: Vec<f64> = (0..TIMING_REPEATS).filter_map(|_| locate(ctx.engine, &dir, t, &ctx.samples[s.k].image, ANGLE_RANGE_DEG).ok().map(|l| l.ms)).collect();
        all_ms.extend(ms.iter().copied());
        json!({"shot": ctx.samples[s.k].shot_id, "size": t.rect[2], "p50": r3(pct(ms.clone(), 50.0)), "p95": r3(pct(ms.clone(), 95.0))})
    }).collect();
    let timing = json!({"repeats": TIMING_REPEATS, "runs": all_ms.len(), "p50": r3(pct(all_ms.clone(), 50.0)), "p95": r3(pct(all_ms.clone(), 95.0)),
        "max": r3(all_ms.iter().copied().fold(f64::NAN, f64::max)), "perShot": per_shot,
        "profile": if cfg!(debug_assertions) { "test（gluesight opt-level 2 + debug 断言）" } else { "release" }, "note": "含建图、调用与解析，单线程"});

    // E6：复拍对。真值未知：门证据里把只找平移的位姿当参考中心、真转角按 0（同一点位复拍，静态 Δd P95 ≤ 0.1 mm）
    let index = |id: &str| ctx.samples.iter().position(|s| s.shot_id == id);
    let mut reshoot_calls = Vec::new();
    let reshoot: Vec<Value> = RESHOOT.iter().map(|&(a, b)| {
        let (Some(ka), Some(kb)) = (index(a), index(b)) else { return json!({"pair": [a, b], "error": "样本集缺图"}) };
        let Some(s) = by_k.get(&ka) else { return json!({"pair": [a, b], "error": "原图没有选出模板"}) };
        let t = s.chosen.as_ref().unwrap();
        let m = match run(ctx.engine, ctx.recipe, ka, &ctx.samples[kb].image) { Ok(m) => m, Err(e) => return json!({"pair": [a, b], "error": e}) };
        let located: Vec<(f64, Result<Located, String>)> = [ANGLE_RANGE_DEG, 0.0].into_iter().map(|range| (range, locate(ctx.engine, &dir, t, &ctx.samples[kb].image, range))).collect();
        if let (Ok(l), Some(Ok(r))) = (&located[0].1, located.get(1).map(|x| &x.1)) {
            if r.pose.center.iter().all(|x| x.is_finite()) { reshoot_calls.push(Call::new(&format!("{a}→{b}"), a, l, r.pose.center, 0.0, t.bright, false)); }
        }
        let variants: Vec<Value> = located.into_iter().map(|(range, l)| (|| {
            let l = l?;
            let b0 = base[ka].as_ref().unwrap();
            let (mut ds, mut dd) = (Vec::new(), Vec::new());
            let d = if l.pose.ok { fixture_d(ctx, ka, &s.frame, &m, &l.pose) } else { vec![None; m.st.len()] };
            for i in 0..m.st.len() {
                if m.st[i] == ST_OK && b0.st[i] == ST_OK { ds.push((m.d[i] - b0.d[i]) as f64); }
                if let (Some(x), Some(z)) = (d[i], d0[&ka][i]) { dd.push(x - z); }
            }
            let p0 = s.pose0.as_ref().unwrap();
            Ok::<Value, String>(json!({"angleRangeDeg": range, "poseOk": l.pose.ok, "why": l.why, "score": r3(l.pose.score), "peakRatio": l.pose.peak_ratio.map(r3),
                "angleObservable": l.pose.angle_observable,
                "poseShiftPx": [r3(l.pose.center[0] - p0.center[0]), r3(l.pose.center[1] - p0.center[1])], "poseAngleDeg": r3(l.pose.angle - p0.angle),
                "staticDiffMm": abs_stats(&ds), "staticDiffMedianMm": r3(median(ds.clone())),
                "fixtureDiffMm": abs_stats(&dd), "fixtureDiffMedianMm": r3(median(dd.clone())),
                "staticVerdict": shot_judge(ctx.recipe, ka, &m).verdict, "fixtureVerdict": fixture_verdict(ctx, ka, &m, &d).verdict}))
        })().unwrap_or_else(|e| json!({"angleRangeDeg": range, "error": e}))).collect();
        json!({"pair": [a, b], "variants": variants})
    }).collect();

    // 门证据：E2/E3 的每次定位（E3 复用 E2 的位姿）与 E6
    let calls: Vec<&Call> = rows.iter().filter_map(|r| r.call.as_ref()).collect();
    let pick = |f: &dyn Fn(&Call) -> bool| -> Vec<&Call> { calls.iter().copied().filter(|c| f(c)).collect() };
    let bright_plus = |c: &Call| c.group.contains("亮度 +40");
    let beyond = |c: &Call| c.group.contains("超出");
    let gates = json!({
        "criteria": {"centerTolPx": LOCK_TOL_PX, "angleTolDeg": LOCK_TOL_DEG, "note": "失败的调用也按 locate 给出的位姿比真值；看不出角度时只比中心，真转角 > 0.3° 记为 uncompensated；叠加了成像扰动的另计 uncompensatedWithImaging（偏移不一定来自转角，可能是错锁）"},
        "sweepNote": "评测按 maxPeakRatio = 1（不判）调用；locate 里分数门最先、歧义门最后，对 ok = true 的调用离线加门与在 locate 里设门等价",
        "subsets": [
            {"name": "E2/E3 范围内用例", "report": gate_report(&pick(&|c| !beyond(c)))},
            {"name": "亮度 +40", "report": gate_report(&pick(&|c| c.group == "亮度 +40"))},
            {"name": "含亮度 +40 的全部用例（叠加、只找平移）", "report": gate_report(&pick(&bright_plus))},
            {"name": "范围内、不含亮度 +40 的用例", "report": gate_report(&pick(&|c| !bright_plus(c) && !beyond(c)))},
            {"name": "超范围用例（平移 +70 px、旋转 +4°）", "report": gate_report(&pick(&beyond))},
            {"name": "E6 复拍（参考 = 只找平移的位姿，真转角按 0）", "report": gate_report(&reshoot_calls.iter().collect::<Vec<_>>())},
        ],
    });

    // 看不出角度的频率：每个拍照点在扫角度（angleRange > 0）的 E2 用例里成功定位、angleObservable = false 的次数
    let mut unobs_by_shot = BTreeMap::<String, (usize, usize)>::new();
    for (&(c, k), r) in jobs.iter().zip(&rows) {
        if list[c].angle_range <= 0.0 { continue; }
        let Ok(l) = &r.located else { continue };
        if !l.pose.ok { continue; }
        let e = unobs_by_shot.entry(ctx.samples[k].shot_id.clone()).or_default();
        e.1 += 1;
        if l.pose.angle_observable == Some(false) { e.0 += 1; }
    }
    let unobservable = json!({
        "note": "angleObservable = false：angle 报 0、只补平移，转角没补偿；评测照此位姿算 d'（不另作处理）",
        "selfLocate": ready.iter().filter(|s| s.pose0.as_ref().and_then(|p| p.angle_observable) == Some(false)).map(|s| ctx.samples[s.k].shot_id.clone()).collect::<Vec<_>>(),
        "perShot": unobs_by_shot.iter().filter(|(_, v)| v.0 > 0).map(|(s, v)| json!({"shot": s, "unobservable": v.0, "okCalls": v.1})).collect::<Vec<_>>(),
        "shotsAlways": unobs_by_shot.iter().filter(|(_, v)| v.0 > 0 && v.0 == v.1).count(),
        "shotsSometimes": unobs_by_shot.iter().filter(|(_, v)| v.0 > 0 && v.0 < v.1).count(),
        "shotsNever": unobs_by_shot.iter().filter(|(_, v)| v.0 == 0).count(),
    });

    // E7：示教时按亮像素比例筛框。离线：亮度 +40 下错锁的框会不会被筛掉；实跑：每张图取排序最前、亮像素比例低于门限的通过试探的框
    let wrong_plus40: Vec<&Call> = calls.iter().copied().filter(|c| c.group == "亮度 +40" && c.lock() == Lock::Wrong).collect();
    let bright_cases = [("亮度 +40", Perturb::Brightness(40.0)), ("亮度 −40", Perturb::Brightness(-40.0))];
    let screen_jobs: Vec<(usize, usize)> = (0..BRIGHT_SCREEN.len()).flat_map(|i| ready.iter().map(move |s| (i, s.k))).collect();
    let screened: Vec<Result<Option<(bool, Vec<Call>)>, String>> = par_map(&screen_jobs, ctx.cfg.workers, |_, &(i, k)| {
        let s = by_k[&k];
        let Some(t) = s.passed.iter().find(|t| t.bright < BRIGHT_SCREEN[i]) else { return Ok(None) };
        let changed = s.chosen.as_ref().is_some_and(|c| c.rect != t.rect);
        let calls = bright_cases.iter().map(|(name, p)| {
            let l = locate(ctx.engine, &dir, t, &p.apply(&ctx.samples[k].image, 0), ANGLE_RANGE_DEG)?;
            Ok(Call::new(name, &ctx.samples[k].shot_id, &l, t.teach_center(), 0.0, t.bright, true))
        }).collect::<Result<Vec<_>, String>>()?;
        Ok(Some((changed, calls)))
    });
    let bright_screen: Vec<Value> = BRIGHT_SCREEN.iter().enumerate().map(|(i, &limit)| {
        let (mut with, mut changed, mut errors, mut calls_i) = (0usize, 0usize, Vec::new(), Vec::new());
        for (&(ii, k), r) in screen_jobs.iter().zip(&screened) {
            if ii != i { continue; }
            match r {
                Ok(Some((ch, cs))) => { with += 1; if *ch { changed += 1; } calls_i.extend(cs.iter().cloned()); }
                Ok(None) => {}
                Err(e) => errors.push(format!("{}：{e}", ctx.samples[k].shot_id)),
            }
        }
        let cases: Vec<Value> = bright_cases.iter().map(|(name, _)| {
            let sub: Vec<&Call> = calls_i.iter().filter(|c| c.group == *name).collect();
            json!({"name": name, "report": gate_report(&sub)})
        }).collect();
        json!({"maxBrightFrac": limit, "mode": if templates_path().is_some() { "固定框：只看固定的框是否低于门限，不重选" } else { "每张图取排序最前、低于门限的通过试探的框" },
            "chosenScreenedOut": ready.iter().filter(|s| s.chosen.as_ref().is_some_and(|t| t.bright >= limit)).count(),
            "wrongPlus40ScreenedOut": wrong_plus40.iter().filter(|c| c.bright >= limit).count(), "wrongPlus40": wrong_plus40.len(),
            "shotsWithTemplate": with, "shotsWithoutTemplate": ready.len() - with, "templatesChanged": changed, "cases": cases, "errors": errors})
    }).collect();


    json!({
        "params": {"searchRadiusPx": SEARCH_RADIUS_PX, "angleRangeDeg": ANGLE_RANGE_DEG, "angleStepDeg": ANGLE_STEP_DEG, "minScore": MIN_SCORE,
            "sizes": SIZES, "stridePx": STRIDE, "corridorPadPx": CORRIDOR_PAD_PX, "minStd": MIN_STD, "byTexture": BY_TEXTURE, "byLever": BY_LEVER,
            "probe": format!("{PROBE:?}"), "probeTolPx": PROBE_TOL_PX, "probeTolDeg": PROBE_TOL_DEG, "maxPeakRatio": "不传（1.1.0 默认 1 = 不判；门效果离线扫）"},
        "baseDir": dir.display().to_string(), "overlayDir": overlay_dir.display().to_string(),
        "peakRatioAvailable": peak_known, "angleObservableAvailable": shots.iter().any(|s| s.pose0.as_ref().is_some_and(|p| p.angle_observable.is_some())),
        "uniqueness": if templates_path().is_some() { "固定框（GLUESIGHT_EVAL_TEMPLATES），不排序" } else if peak_known { "按 peakRatio 排序" } else { "运行库没有 peakRatio（locate < 1.1.0），唯一性未知：按试探定位是否通过、杠杆臂、试探分数排序" },
        "humanReview": REVIEW, "conventionCheckPx": convention, "pinnedTemplates": templates_path(),
        "templates": templates,
        "shotsWithTemplate": ready.len(), "shotsTaught": taught.len(),
        "baseline": {"fixtureMinusStaticMm": abs_stats(&agree), "noRefStations": base_noref, "fixtureNgOnBaseOk": base_fixture_ng},
        "accuracy": accuracy, "invariance": invariance, "beadOffset": bead_offset, "timing": timing, "reshoot": reshoot,
        "gates": gates, "angleUnobservable": unobservable, "brightScreen": bright_screen,
    })
}

pub fn markdown(r: &Value) -> String {
    let v = |x: &Value| match x { Value::Null => "—".to_string(), Value::String(t) => t.clone(), other => other.to_string() };
    let st = |x: &Value| format!("{} / {} / {}", v(&x["p50"]), v(&x["p95"]), v(&x["max"]));
    let mut s = format!("## 特征定位（阶段 fixture）\n\n- 模板目录（base_dir）：{}\n- 叠图：{}\n- 唯一性：{}\n- **{}**\n- 扰动定义与位姿约定核对：{} px\n- 有模板的拍照点：{} / {}\n\n",
        v(&r["baseDir"]), v(&r["overlayDir"]), v(&r["uniqueness"]), v(&r["humanReview"]), v(&r["conventionCheckPx"]), v(&r["shotsWithTemplate"]), v(&r["shotsTaught"]));
    s += "### E1 模板\n\n| 拍照点 | 候选数 | 剔除随相机 | 选中框 [x,y,w,h] | 自匹配分数 | peakRatio | 杠杆臂 mm | 纹理 σ | 同相机 NCC | 亮像素比例 |\n|---|---|---|---|---|---|---|---|---|---|\n";
    for t in r["templates"].as_array().into_iter().flatten() {
        let c = &t["chosen"];
        s += &format!("| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n", v(&t["shot"]), t["candidates"].as_array().map_or(0, Vec::len), v(&t["rejectedCameraFixed"]),
            if c.is_null() { v(&t["error"]) } else { v(&c["rect"]) }, v(&c["score"]), v(&c["peakRatio"]), v(&c["leverMm"]), v(&c["std"]), v(&c["cameraFixedNcc"]), v(&c["brightFrac"]));
    }
    s += &format!("\n原图上按位姿算的 d' 与静态 d 之差 |·| P50 / P95 / 最大 mm：{}；有胶无基准站 {}；基准 OK 而定位后判 NG：{}\n\n",
        st(&r["baseline"]["fixtureMinusStaticMm"]), v(&r["baseline"]["noRefStations"]), v(&r["baseline"]["fixtureNgOnBaseOk"]));
    s += "### E2 定位精度（|误差| P50 / P95 / 最大）\n\n| 扰动 | 类型 | 成功 | 角度贴边 | 看不出角度 | 原因 | 模板中心 px | 角度 ° | 最远站 mm | 最远站 mm（看得出角度的） |\n|---|---|---|---|---|---|---|---|---|---|\n";
    for a in r["accuracy"].as_array().into_iter().flatten() {
        s += &format!("| {} | {} | {}/{} | {} | {} | {} | {} | {} | {} | {} |\n", v(&a["name"]), v(&a["kind"]), v(&a["ok"]), v(&a["shots"]), v(&a["atAngleEdge"]), v(&a["angleUnobservable"]), a["reasons"],
            st(&a["transErrPx"]), st(&a["angleErrDeg"]), st(&a["farStationErrMm"]), st(&a["farStationErrObservableMm"]));
    }
    s += "\n### E3 共同运动下的胶偏变化（mm，|Δ| P50 / P95 / 最大）\n\n| 扰动 | 拍照点 | 定位后 Δd' | 静态 Δd | 看不出角度 | Δd'（看得出角度的） | 有胶无基准站 | 定位后判 NG | 静态判 NG |\n|---|---|---|---|---|---|---|---|---|\n";
    for a in r["invariance"].as_array().into_iter().flatten() {
        s += &format!("| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n", v(&a["name"]), v(&a["shots"]), st(&a["fixtureAbsDiffMm"]), st(&a["staticAbsDiffMm"]),
            v(&a["angleUnobservableShots"]), st(&a["fixtureAbsDiffObservableMm"]), v(&a["noRefStations"]), v(&a["fixtureNg"]), v(&a["staticNg"]));
    }
    s += "\n定位后判 NG 的明细：\n\n";
    for a in r["invariance"].as_array().into_iter().flatten() {
        for d in a["fixtureNgShots"].as_array().into_iter().flatten() { s += &format!("- {}：{}\n", v(&a["name"]), v(d)); }
    }
    s += "\n### E4 只移胶（合成偏移，模板区不动）\n\n| 缺陷 | 应判 | 放置 | 定位后符合 | 静态符合 | 恢复误差中位 mm | 模板区被改 | 位姿最大漂移 px |\n|---|---|---|---|---|---|---|---|\n";
    for a in r["beadOffset"].as_array().into_iter().flatten() {
        s += &format!("| {} | {} | {} | {} | {} | {} | {} | {} |\n", v(&a["name"]), if a["ng"] == true { "NG" } else { "OK" }, v(&a["placed"]),
            v(&a["fixtureAsExpected"]), v(&a["staticAsExpected"]), v(&a["recoveredErrMedianMm"]), v(&a["templateTouched"]), v(&a["poseShiftMaxPx"]));
    }
    s += &format!("\n### E5 定位耗时\n\n单线程 {} 次：P50 {} ms，P95 {} ms，最大 {} ms（{}）\n\n", v(&r["timing"]["runs"]), v(&r["timing"]["p50"]), v(&r["timing"]["p95"]), v(&r["timing"]["max"]), v(&r["timing"]["profile"]));
    s += "### E6 复拍对（原图模板定位复拍图）\n\n| 对 | 角度范围 ° | 定位 | peakRatio | 看得出角度 | 位姿平移 px | 角度 ° | 静态 Δd mm | 定位后 Δd' mm |\n|---|---|---|---|---|---|---|---|---|\n";
    for a in r["reshoot"].as_array().into_iter().flatten() {
        if a["error"].is_string() { s += &format!("| {} | | {} | | | | | | |\n", a["pair"], v(&a["error"])); continue; }
        for x in a["variants"].as_array().into_iter().flatten() {
            if x["error"].is_string() { s += &format!("| {} | {} | {} | | | | | | |\n", a["pair"], v(&x["angleRangeDeg"]), v(&x["error"])); continue; }
            s += &format!("| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n", a["pair"], v(&x["angleRangeDeg"]), v(&x["why"]), v(&x["peakRatio"]), v(&x["angleObservable"]),
                x["poseShiftPx"], v(&x["poseAngleDeg"]), st(&x["staticDiffMm"]), st(&x["fixtureDiffMm"]));
        }
    }
    s += "\n### 门证据（锁对 = 中心误差 ≤ 2 px 且角度误差 ≤ 0.3°；错锁 = 中心误差 > 2 px；E3 复用 E2 的位姿）\n\n| 子集 | 调用 | 锁对 | 错锁（其中 > 10 px） | 中心对、角度超差 | 转角没补偿 | 默认门：错锁拦下 / 放过 / 角度超差拦下 / 误拒锁对 | 拦下原因 | 另拒看不出角度：错锁拦下 / 误拒锁对 |\n|---|---|---|---|---|---|---|---|---|\n";
    let subsets = r["gates"]["subsets"].as_array().cloned().unwrap_or_default();
    for g in &subsets {
        let x = &g["report"];
        s += &format!("| {} | {} | {} | {}（{}） | {} | {} | {} / {} / {} / {} | {} | {} / {} |\n", v(&g["name"]), v(&x["calls"]), v(&x["correct"]), v(&x["wrong"]), v(&x["wrongOver10Px"]),
            v(&x["angleOff"]), v(&x["uncompensated"]), v(&x["shipped"]["wrongCaught"]), v(&x["shipped"]["wrongAccepted"]), v(&x["shipped"]["angleOffCaught"]),
            v(&x["shipped"]["correctRejected"]), x["shipped"]["wrongCaughtByReason"], v(&x["shipped"]["ifRejectUnobservable"]["wrongCaught"]), v(&x["shipped"]["ifRejectUnobservable"]["correctRejected"]));
    }
    let d = |x: &Value| format!("{} / {} / {} / {}", v(&x["min"]), v(&x["p05"]), v(&x["p50"]), v(&x["max"]));
    for g in &subsets {
        let x = &g["report"];
        s += &format!("\n- {}（默认门放过的调用，最小 / P05 / P50 / 最大）：错锁分数 {}，锁对分数 {}；错锁 peakRatio {}，锁对 peakRatio {}", v(&g["name"]),
            d(&x["acceptedWrongScore"]), d(&x["acceptedCorrectScore"]), d(&x["acceptedWrongPeakRatio"]), d(&x["acceptedCorrectPeakRatio"]));
    }
    s += "\n";
    for (key, field) in [("peakRatioSweep", "maxPeakRatio"), ("minScoreSweep", "minScore")] {
        let head: Vec<String> = subsets.first().and_then(|g| g["report"][key].as_array()).into_iter().flatten().map(|p| v(&p[field])).collect();
        s += &format!("\n{field} 离线扫（错锁拦下 / 放过 / 误拒锁对）：\n\n| 子集 |{}\n|---|{}\n", head.iter().map(|h| format!(" {h} |")).collect::<String>(), "---|".repeat(head.len()));
        for g in &subsets {
            s += &format!("| {} |", v(&g["name"]));
            for p in g["report"][key].as_array().into_iter().flatten() { s += &format!(" {} / {} / {} |", v(&p["wrongCaught"]), v(&p["wrongAccepted"]), v(&p["correctRejected"])); }
            s += "\n";
        }
    }
    let u = &r["angleUnobservable"];
    s += &format!("\n看不出角度（扫角度的 E2 用例里成功定位）：总是 {} 张、有时 {} 张、从不 {} 张；原图自定位看不出角度：{}\n",
        v(&u["shotsAlways"]), v(&u["shotsSometimes"]), v(&u["shotsNever"]), u["selfLocate"]);
    s += "\n### 示教时按亮像素比例筛框（实跑，±3°）\n\n| 亮像素比例上限 | 原选框被筛 | 亮度 +40 错锁的框被筛 | 有框 / 换框 | 亮度 +40 错锁 | 亮度 −40 错锁 |\n|---|---|---|---|---|---|\n";
    for b in r["brightScreen"].as_array().into_iter().flatten() {
        let c = |i: usize| { let x = &b["cases"][i]["report"]; format!("{}（> 10 px {}，放过 {}）", v(&x["wrong"]), v(&x["wrongOver10Px"]), v(&x["shipped"]["wrongAccepted"])) };
        s += &format!("| {} | {} | {}/{} | {} / {} | {} | {} |\n", v(&b["maxBrightFrac"]), v(&b["chosenScreenedOut"]), v(&b["wrongPlus40ScreenedOut"]), v(&b["wrongPlus40"]),
            v(&b["shotsWithTemplate"]), v(&b["templatesChanged"]), c(0), c(1));
    }
    s + "\n"
}

#[test]
fn unobservable_rotation_with_drifted_center_counts_as_uncompensated_not_wrong_lock() {
    // 防的回归：中心容差先于“看不出角度”判时，转角没补偿、0° 平移偏 2…8 px 的调用被算成错锁，
    // 门效果里“另拒看不出角度”看起来拦下了错锁（fixture-p0-l1 范围内不含亮度 +40 的 26 次错锁里 18 次是这种）
    let call = |center_err: f64, observable: Option<bool>| Call { group: String::new(), shot: String::new(), ok: true, reason: String::new(),
        score: 1.0, peak: None, observable, center_err, angle_err: -2.0, true_angle: 2.0, bright: 0.0, imaging: false };
    assert!(call(5.0, Some(false)).lock() == Lock::Uncompensated);
    assert!(call(25.0, Some(false)).lock() == Lock::Wrong); // 偏过 10 px 不是转角能解释的
    assert!(call(5.0, Some(true)).lock() == Lock::Wrong);
}
