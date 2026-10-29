//! 示教走廊（直胶）的逐站判定：输入 `glue.lateral_runs` 沿示教折线量出的全部暗段，按 lyFlow 1.1
//! `glue.bead_width` 示教分支的规则判每站有无胶。步骤：
//! 1. 按胶宽范围筛暗段；
//! 2. 偏移轨迹（T3）：离示教线不超过容差的暗段投票定常数偏移，再沿 s 取中位数逐站细化，断口按两头插值，
//!    轨迹外一段整体横移且过渡处有斜胶时跟过去；
//! 3. 轨迹外与轨迹胶同宽的子段（被淡肩并宽的胶）补回后重算轨迹；
//! 4. 轨迹上的站不到 3 站或 10%、或轨迹陡度 P25 < sharpMin（D15）时整段判无胶（T4）；
//! 5. D16：参考宽、参考对比度只取轨迹上的站，每站取离期望中心最近的暗段，够宽、够近、够浓才算有胶。

use std::fmt;

use serde::Deserialize;

/// 偏移轨迹逐站细化时沿 s 取中位数的半窗（px）
const TRACK_WINDOW: f64 = 40.0;
/// 落在轨迹上的站不到这个比例（或不到 3 站）= 沿示教线没有胶
const TRACK_MIN_SUPPORT: f64 = 0.1;
const TRACK_MIN_COUNT: usize = 3;
/// 逐站细化最多几遍（落在轨迹上的那些站不再变就停）
const TRACK_PASSES: usize = 8;
/// 胶整体横移的一段至少这么多站才跟过去
const SHIFT_MIN_STATIONS: usize = 3;
/// 横移段里落在同一相对偏移上的站至少占这么多
const SHIFT_COVER: f64 = 0.6;
/// 两侧轨迹上的站在同一相对偏移处也有胶样暗段的比例超过它 = 一直伴着胶的平行暗线
const SHIFT_FLANK_SHARE: f64 = 0.3;
/// 横跳过渡处斜着的胶至少这么宽（× 轨迹胶宽）
const SHIFT_RAMP_MIN_WIDTH: f64 = 0.5;
/// 中心门限的下限（px）
const GATE_MIN: f64 = 6.0;

/// `glue.LateralRuns` 记录（lyFlow glue-measure-ops §4）里判定用到的字段。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LateralRuns {
    /// 卡尺半长（px）
    pub search_half: f64,
    pub stations: Vec<RunStation>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RunStation {
    /// 沿胶路的弧长（px）
    pub s: f64,
    /// 按 lo 升序的全部暗段
    pub runs: Vec<LateralRun>,
}

/// 一站卡尺上的一个暗段；偏移沿法向（px，右正左负）。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LateralRun {
    pub lo: f64,
    pub hi: f64,
    pub width: f64,
    pub mid: f64,
    pub peak: f64,
    pub lo_complete: bool,
    pub hi_complete: bool,
    /// D15 陡度；采样无效时为 None
    pub sharpness: Option<f64>,
    /// 段内最亮的局部亮度；没接 level 或全无效时为 None
    pub level: Option<f64>,
}

impl LateralRun {
    /// 示教胶路的对比度：峰值 / 段内最亮的局部亮度（不足 1 按 1）。
    fn contrast(&self) -> f64 {
        self.peak / self.level.map_or(1.0, |l| l.max(1.0))
    }
}

/// 示教走廊的判定门限（px 量已按当量换算）。
#[derive(Clone, Debug)]
pub struct TaughtParams {
    /// 中心偏移容差：胶中心离示教线不超过它才算这条胶（1.1 `taught_path.tolerance`）
    pub tolerance: f64,
    /// 轨迹陡度 P25 的下限（D15，1.1 `taught_path.sharpMin`）
    pub sharp_min: f64,
    /// 胶宽范围 [下限, 上限]（1.1 `bead_width.widthRange`）
    pub width_range: [f64; 2],
    /// 有胶的站至少这么多倍参考宽（D16）
    pub present_ratio: f64,
    /// 胶中心离期望中心不超过 max(6, 它 × 参考宽)，也是轨迹的门限（D16、T3）
    pub center_ratio: f64,
    /// 有胶的站对比度至少这么多倍参考对比度（D16）
    pub contrast_ratio: f64,
}

/// 整段判无胶的原因（与 1.1 `bead_width` 的 `info.reason` 同口径）。
#[derive(Clone, Debug, PartialEq)]
pub enum NoBead {
    /// 示教线 ± 容差以内一站都没有暗段
    NoCandidate { tolerance: f64 },
    /// 落在偏移轨迹上的站不到 3 站或 10%
    TooFewOnTrack { count: usize, support: f64 },
    /// 轨迹上暗段边缘陡度的 P25 低于 sharpMin（压痕、阴影）
    Blunt { sharpness: f64, sharp_min: f64 },
}

impl fmt::Display for NoBead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCandidate { tolerance } => write!(f, "示教线 ± {tolerance:.2} px 以内没有一站量到胶样的暗段"),
            Self::TooFewOnTrack { count, support } => write!(
                f,
                "沿示教线只有 {count} 站（{:.2}%）量到落在同一条偏移轨迹上的胶，不到 3 站或 10%",
                100.0 * support
            ),
            Self::Blunt { sharpness, sharp_min } => write!(
                f,
                "示教线上的暗带边缘太缓（陡度 P25 = {sharpness:.2} < sharpMin {sharp_min:.2}），像压痕或阴影，不像胶"
            ),
        }
    }
}

/// 一段示教走廊一帧的判定。`expect`、`no_bead` 留给流程调试页叠加判定结果（LF-6），测量本身只用 `beads`。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct TaughtJudgement {
    /// 每站的胶边 (lo, hi)（px，沿法向）；无胶为 None
    pub beads: Vec<Option<(f64, f64)>>,
    /// 每站期望的胶中心（偏移轨迹，px）
    pub expect: Vec<f64>,
    /// 整段判无胶时的原因（这时 beads 全为 None）
    pub no_bead: Option<NoBead>,
}

/// 按 1.1 示教直胶的规则判一段走廊。
///
/// `runs`：`glue.lateral_runs`（`split = half_max`、`mergePeakRatio = 0.4`、`segmentMin = contrastMin / 2`、
/// `peakMin = contrastMin`、`mergeWidthMax = 胶宽上限`、`searchHalf ≥ tolerance + 胶宽上限 / 2`、接 `level`）的输出。
/// `split`：同一节点参数只把 `mergeGap` 改成 0 的输出（合并前的子段），用来补回被淡肩并宽的胶；
/// 不给就不补回。两份记录的站必须一一对应。记录与参数不合契约时返回 Err。
pub fn judge_taught(
    runs: &LateralRuns,
    split: Option<&LateralRuns>,
    p: &TaughtParams,
) -> Result<TaughtJudgement, String> {
    let [w_min, w_max] = p.width_range;
    if !(p.tolerance >= 0.0) || !p.tolerance.is_finite() {
        return Err("中心偏移容差必须是非负有限数".into());
    }
    if !(w_min >= 0.0 && w_max >= w_min) {
        return Err("胶宽范围需满足 0 ≤ 下限 ≤ 上限".into());
    }
    if split.is_some_and(|x| x.stations.len() != runs.stations.len()) {
        return Err("合并前子段的站数与横向卡尺不一致".into());
    }
    let tol = p.tolerance;
    let mut st: Vec<Station> = runs
        .stations
        .iter()
        .map(|x| Station {
            s: x.s,
            runs: x.runs.iter().filter(|r| r.width >= w_min && r.width <= w_max).cloned().collect(),
        })
        .collect();
    let n = st.len();

    let mut tr = track_offsets(&st, tol, p.center_ratio);
    if let (None, Some(split)) = (&tr.reason, split) {
        // 轨迹外的站：合并前的子段里有与轨迹胶同宽、离期望中心不远的，补回来（紧贴胶边的淡肩把胶并宽了）
        let reference = median(tr.on_track_runs(&st).map(|r| r.width).collect());
        let gate = GATE_MIN.max(p.center_ratio * reference);
        let half = split.search_half;
        let mut recovered = Vec::new();
        for i in (0..n).filter(|&i| tr.on_track[i].is_none()) {
            let e = tr.expect[i];
            let best = nearest(split.stations[i].runs.iter().filter(|r| {
                r.lo_complete
                    && r.hi_complete
                    && r.lo > -half
                    && r.hi < half
                    && r.hi > r.lo
                    && r.width >= w_min
                    && r.width <= w_max
                    && (r.width - reference).abs() <= gate
                    && (r.mid - e).abs() <= gate
                    && r.mid.abs() <= tol
                    && !st[i].runs.iter().any(|x| x.lo == r.lo && x.hi == r.hi)
            }), e);
            if let Some(r) = best {
                recovered.push((i, r.clone()));
            }
        }
        if !recovered.is_empty() {
            for (i, r) in recovered {
                st[i].runs.push(r);
            }
            tr = track_offsets(&st, tol, p.center_ratio);
        }
    }

    // D15 用在整条轨迹上（T5）：压痕、阴影也是暗带，边缘却是缓的；逐站不卡
    let sharp: Vec<f64> = tr.on_track_runs(&st).filter_map(|r| r.sharpness).filter(|v| v.is_finite()).collect();
    if tr.reason.is_none() && sharp.len() >= 3 {
        let sharpness = percentile(sharp, 25.0);
        if sharpness < p.sharp_min {
            tr.reason = Some(NoBead::Blunt { sharpness, sharp_min: p.sharp_min });
        }
    }
    if let Some(reason) = tr.reason {
        return Ok(TaughtJudgement { beads: vec![None; n], expect: tr.expect, no_bead: Some(reason) });
    }

    // D16：参考宽、参考对比度只取落在轨迹上的站（断口里、轨迹外的暗段不进参考）
    let w_ref = median(tr.on_track_runs(&st).map(|r| r.width).collect());
    let min_contrast = p.contrast_ratio * median(tr.on_track_runs(&st).map(LateralRun::contrast).collect());
    let min_width = p.present_ratio * w_ref;
    let max_offset = GATE_MIN.max(p.center_ratio * w_ref);
    let beads = st
        .iter()
        .zip(&tr.expect)
        .map(|(x, &e)| {
            nearest(x.runs.iter(), e)
                .filter(|r| r.width >= min_width && (r.mid - e).abs() <= max_offset && r.contrast() >= min_contrast)
                .map(|r| (r.lo, r.hi))
        })
        .collect();
    Ok(TaughtJudgement { beads, expect: tr.expect, no_bead: None })
}

struct Station {
    s: f64,
    runs: Vec<LateralRun>,
}

struct Track {
    /// 每站期望的胶中心（px）
    expect: Vec<f64>,
    /// 每站落在轨迹上的那一段在 `Station::runs` 里的下标
    on_track: Vec<Option<usize>>,
    reason: Option<NoBead>,
}

impl Track {
    fn on_track_runs<'a>(&'a self, st: &'a [Station]) -> impl Iterator<Item = &'a LateralRun> + 'a {
        self.on_track.iter().zip(st).filter_map(|(k, x)| k.map(|k| &x.runs[k]))
    }
}

/// 离 e 最近的一段（并列取先出现的）。
fn nearest<'a>(runs: impl Iterator<Item = &'a LateralRun>, e: f64) -> Option<&'a LateralRun> {
    let mut best: Option<&LateralRun> = None;
    for r in runs {
        if best.is_none_or(|b| (r.mid - e).abs() < (b.mid - e).abs()) {
            best = Some(r);
        }
    }
    best
}

/// 离 e 最近、且不超过 gate 的那一段的下标。
fn nearest_index(x: &Station, e: f64, gate: f64, keep: impl Fn(&LateralRun) -> bool) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (k, r) in x.runs.iter().enumerate() {
        if keep(r) && best.is_none_or(|b| (r.mid - e).abs() < (x.runs[b].mid - e).abs()) {
            best = Some(k);
        }
    }
    best.filter(|&b| (x.runs[b].mid - e).abs() <= gate)
}

/// 偏移轨迹：胶相对示教线横着偏了多少，逐站估出来（T3）。
/// 1. 投票定常数偏移 h：每个候选（中心离示教线不超过容差的暗段）的中心外加 0 各是一个假设，票数 = 离它不到 gate
///    有候选的站数；票数不低于最多那个一半的里取离示教线最近的（旁边一条更长的平行暗边拽不走它）；
/// 2. 逐站细化到落在轨迹上的站不再变：每站取离期望最近的候选，沿 s 在 ±40 px 里取中位数；断口按两头线性插值；
/// 3. 夹在 ±容差里，再跟一遍整段横移。
fn track_offsets(st: &[Station], tol: f64, center_ratio: f64) -> Track {
    let n = st.len();
    let mut tr = Track { expect: vec![0.0; n], on_track: vec![None; n], reason: None };
    let cands = || st.iter().flat_map(|x| &x.runs).filter(|r| r.mid.abs() <= tol);
    if cands().next().is_none() {
        tr.reason = Some(NoBead::NoCandidate { tolerance: tol });
        return tr;
    }
    let gate = GATE_MIN.max(center_ratio * median(cands().map(|r| r.width).collect()));
    let near = |x: &Station, e: f64| nearest_index(x, e, gate, |r| r.mid.abs() <= tol);

    let hypotheses: Vec<f64> = std::iter::once(0.0).chain(cands().map(|r| r.mid)).collect();
    let votes: Vec<usize> = hypotheses.iter().map(|&h| st.iter().filter(|x| near(x, h).is_some()).count()).collect();
    let best_votes = votes.iter().copied().max().unwrap_or(0);
    let mut h = 0.0_f64;
    let mut h_votes: Option<usize> = None;
    for (&hyp, &v) in hypotheses.iter().zip(&votes) {
        if 2 * v < best_votes {
            continue;
        }
        let a = hyp.abs();
        if h_votes.is_none_or(|hv| a < h.abs() - 1e-9 || (a <= h.abs() + 1e-9 && v > hv)) {
            h = hyp;
            h_votes = Some(v);
        }
    }
    tr.expect.fill(h);

    let mut last: Option<Vec<f64>> = None;
    for _ in 0..TRACK_PASSES {
        let mids: Vec<f64> = st
            .iter()
            .zip(&tr.expect)
            .map(|(x, &e)| near(x, e).map_or(f64::NAN, |k| x.runs[k].mid))
            .collect();
        let same = |a: &f64, b: &f64| if a.is_nan() { b.is_nan() } else { a == b };
        if last.as_ref().is_some_and(|l| l.iter().zip(&mids).all(|(a, b)| same(a, b))) {
            break;
        }
        let mut e: Vec<f64> = (0..n)
            .map(|i| {
                let w: Vec<f64> = (0..n)
                    .filter(|&j| mids[j].is_finite() && (st[j].s - st[i].s).abs() <= TRACK_WINDOW)
                    .map(|j| mids[j])
                    .collect();
                if w.is_empty() { f64::NAN } else { median(w) }
            })
            .collect();
        // 断口：两侧都有就线性插值，只有一侧取那一侧，都没有就是投票的常数
        let mut prev: Option<usize> = None;
        for i in 0..=n {
            if i < n && !e[i].is_finite() {
                continue;
            }
            for k in prev.map_or(0, |p| p + 1)..i {
                e[k] = match (prev, i < n) {
                    (None, false) => h,
                    (None, true) => e[i],
                    (Some(p), false) => e[p],
                    (Some(p), true) => {
                        let (a, b) = (st[p].s, st[i].s);
                        let f = if b - a > 1e-9 { (st[k].s - a) / (b - a) } else { 0.0 };
                        e[p] + (e[i] - e[p]) * f
                    }
                };
            }
            prev = Some(i);
        }
        for v in &mut e {
            *v = v.clamp(-tol, tol);
        }
        tr.expect = e;
        last = Some(mids);
    }
    tr.on_track = st.iter().zip(&tr.expect).map(|(x, &e)| near(x, e)).collect();
    follow_shifts(st, tol, gate, &mut tr);
    let count = tr.on_track.iter().flatten().count();
    let support = if n > 0 { count as f64 / n as f64 } else { 0.0 };
    if count < TRACK_MIN_COUNT || support < TRACK_MIN_SUPPORT {
        tr.reason = Some(NoBead::TooFewOnTrack { count, support });
    }
    tr
}

/// 轨迹外的一段胶整体横移（几毫米内突然横跳）：逐站细化一遍只挪一个 gate，跟不上跳变。
/// 没落在轨迹上的连续几站里，大多数站在同一相对偏移上有与轨迹胶同宽的暗段，两侧轨迹上的站同一处却没有（不是
/// 一直伴着胶的平行暗线），并且两头有斜着的胶接上（真横移在过渡处有斜胶），这几站的轨迹就跟过去。
fn follow_shifts(st: &[Station], tol: f64, gate: f64, tr: &mut Track) {
    let n = st.len();
    let widths: Vec<f64> = tr.on_track_runs(st).map(|r| r.width).collect();
    if widths.len() < SHIFT_MIN_STATIONS {
        return;
    }
    let w_ref = median(widths);
    let w_tol = gate.max(0.3 * w_ref);
    let near = |x: &Station, e: f64| {
        nearest_index(x, e, gate, |r| r.mid.abs() <= tol && (r.width - w_ref).abs() <= w_tol)
    };
    let mut i = 0;
    while i < n {
        if tr.on_track[i].is_some() {
            i += 1;
            continue;
        }
        let a = i;
        while i < n && tr.on_track[i].is_none() {
            i += 1;
        }
        let b = i;
        let len = b - a;
        if len < SHIFT_MIN_STATIONS {
            continue;
        }
        let expect = &tr.expect;
        let mut delta = 0.0;
        let mut votes = 0;
        for k in a..b {
            for r in &st[k].runs {
                let d = r.mid - expect[k];
                if d.abs() <= gate || near(&st[k], r.mid).is_none() {
                    continue;
                }
                let v = (a..b).filter(|&j| near(&st[j], expect[j] + d).is_some()).count();
                if v > votes {
                    votes = v;
                    delta = d;
                }
            }
        }
        if votes < SHIFT_MIN_STATIONS || (votes as f64) < SHIFT_COVER * len as f64 {
            continue;
        }
        let flank: Vec<usize> = (a.saturating_sub(len)..a)
            .chain(b..n.min(b + len))
            .filter(|&k| tr.on_track[k].is_some())
            .collect();
        let parallel = flank.iter().filter(|&&k| near(&st[k], expect[k] + delta).is_some()).count();
        if flank.len() < SHIFT_MIN_STATIONS || parallel as f64 > SHIFT_FLANK_SHARE * flank.len() as f64 {
            continue;
        }
        let moved: Vec<Option<usize>> = (a..b).map(|k| near(&st[k], expect[k] + delta)).collect();
        let first = (a..b).find(|&k| moved[k - a].is_some()).unwrap_or(b);
        let last = (a..b).rev().find(|&k| moved[k - a].is_some()).unwrap_or(a);
        // 从轨迹上的最后一站 on 到横移段的一头 shifted，中间每站都有一段斜着的胶：中心落在横移前后之间、离两头各至少
        // 半个 gate，不比两头并起来还宽；没有中间站时，这两站至少一站的胶已经斜过去
        let linked = |on: usize, shifted: usize| {
            let ahead: Vec<f64> = if shifted > on {
                (shifted..b).filter_map(|k| moved[k - a].map(|r| st[k].runs[r].mid)).take(SHIFT_MIN_STATIONS).collect()
            } else {
                (a..=shifted).rev().filter_map(|k| moved[k - a].map(|r| st[k].runs[r].mid)).take(SHIFT_MIN_STATIONS).collect()
            };
            let (from, to) = (expect[on], median(ahead));
            let jump = (to - from).abs();
            let sign = if to > from { 1.0 } else { -1.0 };
            let slanted = |k: usize| {
                st[k].runs.iter().any(|r| {
                    let along = (r.mid - from) * sign;
                    r.mid.abs() <= tol
                        && along >= 0.5 * gate
                        && along <= jump - 0.5 * gate
                        && r.width >= SHIFT_RAMP_MIN_WIDTH * w_ref
                        && r.width <= w_ref + jump - gate
                })
            };
            let (lo, hi) = (on.min(shifted), on.max(shifted));
            let mut any = false;
            for k in lo..=hi {
                let s = slanted(k);
                if !s && k != lo && k != hi {
                    return false;
                }
                any |= s;
            }
            any
        };
        if (a > 0 && !linked(a - 1, first)) || (b < n && !linked(b, last)) {
            continue;
        }
        for k in a..b {
            if let Some(r) = moved[k - a] {
                tr.on_track[k] = Some(r);
                tr.expect[k] = st[k].runs[r].mid;
            }
        }
    }
}

/// 中位数（偶数个取中间两个的均值）；空为 NaN。
fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    if v.len() % 2 == 1 { v[m] } else { 0.5 * (v[m - 1] + v[m]) }
}

/// 线性插值的百分位（q ∈ [0, 100]）；空为 NaN。
fn percentile(mut v: Vec<f64>, q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    let pos = q / 100.0 * (v.len() - 1) as f64;
    let i = pos.floor() as usize;
    let j = (i + 1).min(v.len() - 1);
    v[i] + (v[j] - v[i]) * (pos - i as f64)
}

#[cfg(test)]
#[path = "taught/tests.rs"]
mod tests;
