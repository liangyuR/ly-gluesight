use serde::{Deserialize, Serialize};

use crate::recipe::{JudgeParams, Recipe, Segment};

/// 声明顺序即严重程度，整件结果取各段最严重的一个。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Ok,
    OkWithExcursion,
    NgPosition,
    NgWidth,
    NgAbsolute,
    NgGap,
    ErrInspect,
}

impl Verdict {
    pub fn plc_code(self) -> u16 {
        match self {
            Verdict::Ok => 1,
            Verdict::OkWithExcursion => 2,
            Verdict::NgPosition => 11,
            Verdict::NgAbsolute => 12,
            Verdict::NgGap => 13,
            Verdict::NgWidth => 14,
            Verdict::ErrInspect => 90,
        }
    }
}

pub mod fault {
    pub const MISSING_FRAME: u16 = 91;
    pub const LOCATE_FAILED: u16 = 92;
    pub const INVALID_POINTS: u16 = 93;
    pub const SHOT_COUNT_MISMATCH: u16 = 94;
    pub const NO_RECIPE: u16 = 95;
    pub const EXTRA_FRAME: u16 = 96;
    pub const MOTION_TIMEOUT: u16 = 97;
    pub const DEVICE_LOST: u16 = 98;
    pub const PROCESS_TIMEOUT: u16 = 99;
    pub const PLAN_MISMATCH: u16 = 100;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointState {
    Pending,
    /// d：胶条中线相对示教中线的横向偏移；w：胶宽（没测时为 NaN）
    Measured { d: f32, w: f32 },
    /// 沿示教中线找了、没有胶
    Gap,
    /// 该点测不了（被遮挡、出了图像、胶边被截断）
    Invalid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentResult {
    pub verdict: Verdict,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub excursion_len: f32,
    #[serde(default)]
    pub w_min: Option<f32>,
    #[serde(default)]
    pub w_max: Option<f32>,
    #[serde(default)]
    pub w_excursion_len: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GapRun {
    pub segment: usize,
    pub s0: f32,
    pub s1: f32,
    pub len: f32,
    /// 缺胶点所在的拍照点
    pub frames: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Judgement {
    pub verdict: Verdict,
    pub plc_code: u16,
    pub fault_code: u16,
    pub reason: String,
    pub segments: Vec<SegmentResult>,
    pub gaps: Vec<GapRun>,
}

impl Judgement {
    pub fn error(fault_code: u16, reason: impl Into<String>) -> Self {
        Self {
            verdict: Verdict::ErrInspect,
            plc_code: Verdict::ErrInspect.plc_code(),
            fault_code,
            reason: reason.into(),
            segments: Vec::new(),
            gaps: Vec::new(),
        }
    }
}

const MAX_INVALID_LEN: f32 = 2.0;

#[derive(Clone, Copy, PartialEq)]
enum GapKind {
    /// 整段一站胶都没有
    Whole,
    /// 断胶区间里夹着超差站或测不了的站
    Mixed,
    Plain,
}

/// 一段里某个量（位置或胶宽）的统计：极值、有没有超绝对限、最长连续超差。
#[derive(Default)]
struct Stat {
    min: Option<f32>,
    max: Option<f32>,
    absolute: bool,
    excursion_len: f32,
    /// 最长那次超差的段内弧长起止
    excursion_at: Option<(f32, f32)>,
}

/// 段内满足条件的连续区间。每个拍照点自成一段，段与段之间不连（D-10）。
fn runs_in(seg: &Segment, pred: impl Fn(usize) -> bool) -> Vec<Vec<usize>> {
    let mut runs = Vec::new();
    let mut cur = Vec::new();
    for j in seg.first..seg.first + seg.count {
        if pred(j) {
            cur.push(j);
        } else if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    runs
}

/// 段内按连续测到的点做中值滤波。
fn filtered(recipe: &Recipe, table: &[PointState], value: impl Fn(&PointState) -> Option<f32>) -> Vec<Option<f32>> {
    let mut out = vec![None; table.len()];
    for g in &recipe.segments {
        for run in runs_in(g, |j| value(&table[j]).is_some()) {
            let v: Vec<f32> = run.iter().map(|&j| value(&table[j]).unwrap()).collect();
            for (&j, f) in run.iter().zip(median_filter(&v, recipe.filter_window)) {
                out[j] = Some(f);
            }
        }
    }
    out
}

fn stat(recipe: &Recipe, g: &Segment, values: &[Option<f32>], p: &JudgeParams) -> Stat {
    let sp = recipe.spacing;
    let mut s = Stat::default();
    for v in values[g.first..g.first + g.count].iter().flatten() {
        s.min = Some(s.min.map_or(*v, |m| m.min(*v)));
        s.max = Some(s.max.map_or(*v, |m| m.max(*v)));
        s.absolute |= *v < p.abs_min || *v > p.abs_max;
    }
    for run in runs_in(g, |j| values[j].is_some_and(|v| v < p.lower() || v > p.upper())) {
        let len = run.len() as f32 * sp;
        if len > s.excursion_len {
            s.excursion_len = len;
            s.excursion_at = Some((g.s(run[0], sp), g.s(run[run.len() - 1], sp) + sp));
        }
    }
    s
}

fn grade(s: &Stat, p: &JudgeParams, ng: Verdict) -> Verdict {
    if s.excursion_len > p.max_excursion_len {
        ng
    } else if s.excursion_len > 0.0 {
        Verdict::OkWithExcursion
    } else {
        Verdict::Ok
    }
}

pub fn aggregate_views(results: impl IntoIterator<Item = Verdict>) -> Verdict {
    let results: Vec<_> = results.into_iter().collect();
    results.iter().copied().filter(|v| *v <= Verdict::OkWithExcursion).min()
        .unwrap_or_else(|| results.into_iter().max().unwrap_or(Verdict::ErrInspect))
}

pub fn judge(recipe: &Recipe, table: &[PointState]) -> Judgement {
    let sp = recipe.spacing;
    let n = table.len();

    if n != recipe.point_count() {
        return Judgement::error(fault::INVALID_POINTS, "测量点数量与配方不一致");
    }
    let invalid: Vec<bool> = recipe.segments.iter().map(|g| {
        (g.first..g.first + g.count).any(|j| table[j] == PointState::Pending)
            || runs_in(g, |j| table[j] == PointState::Invalid).iter()
                .any(|run| run.len() as f32 * sp > MAX_INVALID_LEN)
            || (g.first..g.first + g.count).all(|j| table[j] == PointState::Invalid)
    }).collect();

    let d = filtered(recipe, table, |p| match p {
        PointState::Measured { d, .. } if d.is_finite() => Some(*d),
        _ => None,
    });
    let w = filtered(recipe, table, |p| match p {
        PointState::Measured { w, .. } if w.is_finite() => Some(*w),
        _ => None,
    });
    let pos: Vec<Option<Stat>> = recipe.segments.iter().map(|g| g.position.as_ref().map(|p| stat(recipe, g, &d, p))).collect();
    let width: Vec<Option<Stat>> = recipe.segments.iter().map(|g| g.width.as_ref().map(|p| stat(recipe, g, &w, p))).collect();
    let mut segments: Vec<SegmentResult> = recipe
        .segments
        .iter()
        .enumerate()
        .map(|(gi, g)| {
            let mut verdict = if invalid[gi] { Verdict::ErrInspect } else { Verdict::Ok };
            if let (Some(s), Some(p)) = (&pos[gi], &g.position) {
                verdict = verdict.max(if s.absolute { Verdict::NgAbsolute } else { grade(s, p, Verdict::NgPosition) });
            }
            if let (Some(s), Some(p)) = (&width[gi], &g.width) {
                verdict = verdict.max(if s.absolute { Verdict::NgWidth } else { grade(s, p, Verdict::NgWidth) });
            }
            let p = pos[gi].as_ref();
            let wd = width[gi].as_ref();
            SegmentResult {
                verdict,
                min: p.and_then(|s| s.min),
                max: p.and_then(|s| s.max),
                excursion_len: p.map_or(0.0, |s| s.excursion_len),
                w_min: wd.and_then(|s| s.min),
                w_max: wd.and_then(|s| s.max),
                w_excursion_len: wd.map_or(0.0, |s| s.excursion_len),
            }
        })
        .collect();

    // 断胶区间：断胶站连同紧挨着的超差站、测不了的站一起算长度，免得"断—细—断"交替把一处大缺口
    // 拆成几段都不超限的短断口；区间里至少要有一站断胶。纯断胶区间与原规则一致。
    let mut gaps = Vec::new();
    let mut gap_kind = Vec::new();
    let mut sparse = vec![None; segments.len()];
    for (gi, g) in recipe.segments.iter().enumerate() {
        let gap_count = (g.first..g.first + g.count).filter(|&j| table[j] == PointState::Gap).count();
        let present = (g.first..g.first + g.count).filter(|&j| matches!(table[j], PointState::Measured { .. })).count();
        let out = |p: &Option<JudgeParams>, v: &[Option<f32>], j: usize| p.as_ref().is_some_and(|p| v[j].is_some_and(|v| v < p.lower() || v > p.upper()));
        let defect = |j: usize| matches!(table[j], PointState::Gap | PointState::Invalid) || out(&g.position, &d, j) || out(&g.width, &w, j);
        for run in runs_in(g, defect) {
            let gap_stations = run.iter().filter(|&&j| table[j] == PointState::Gap).count();
            let len = run.len() as f32 * sp;
            // 整段一站胶都没有时，允许断胶长度设得再大也判断胶
            if gap_stations == 0 || (len <= g.max_gap_len && present > 0) {
                continue;
            }
            segments[gi].verdict = segments[gi].verdict.max(Verdict::NgGap);
            gaps.push(GapRun { segment: gi, s0: g.s(run[0], sp), s1: g.s(run[run.len() - 1], sp) + sp, len, frames: vec![g.shot as u8] });
            gap_kind.push(if present == 0 { GapKind::Whole } else if gap_stations < run.len() { GapKind::Mixed } else { GapKind::Plain });
        }
        // 零散断胶：每处都不超长但加起来缺得多。只在累计断胶超过单处允许长度时才看比例，
        // 这样一处允许范围内的断口在短段上不会被比例误判。
        let ratio = present as f32 / (present + gap_count).max(1) as f32;
        if gap_count as f32 * sp > g.max_gap_len && ratio < g.min_present {
            segments[gi].verdict = segments[gi].verdict.max(Verdict::NgGap);
            sparse[gi] = Some(ratio);
        }
    }

    let shot_verdicts: Vec<_> = recipe.shots.iter().enumerate().filter(|(_, shot)| shot.measured()).map(|(k, _)| {
        (k, aggregate_views(recipe.segments.iter().zip(&segments).filter(|(g, _)| g.shot == k).map(|(_, s)| s.verdict)))
    }).collect();
    let verdict = shot_verdicts.iter().map(|(_, verdict)| *verdict).max().unwrap_or(Verdict::ErrInspect);
    let contributes = |gi: usize| shot_verdicts.iter().any(|(k, value)| *k == recipe.segments[gi].shot && *value == verdict);
    let at = |r: Option<(f32, f32)>| r.map(|(a, b)| format!(" · s={a:.1}–{b:.1}")).unwrap_or_default();
    let reason = match verdict {
        Verdict::NgGap => {
            let gi = segments.iter().enumerate().position(|(gi, s)| s.verdict == Verdict::NgGap && contributes(gi)).unwrap();
            let seg = &recipe.segments[gi];
            match gaps.iter().zip(&gap_kind).filter(|(g, _)| g.segment == gi).max_by(|a, b| a.0.len.total_cmp(&b.0.len)) {
                Some((g, GapKind::Whole)) => format!("{} 检测区内没找到胶（整段 {:.1} mm）", seg.name, g.len),
                Some((g, kind)) => {
                    let mixed = if *kind == GapKind::Mixed { "（含相连的超差或测不了的站）" } else { "" };
                    format!("{} 断胶 {:.1} mm{mixed} > {:.1} mm · s={:.1}–{:.1}", seg.name, g.len, seg.max_gap_len, g.s0, g.s1)
                }
                None => format!("{} 胶条断续：有胶站只占 {:.0}% < {:.0}%", seg.name, sparse[gi].unwrap_or(0.0) * 100.0, seg.min_present * 100.0),
            }
        }
        Verdict::Ok => format!("{} 个拍照点全部合格 · {n} 点", recipe.shots.iter().filter(|s| s.measured()).count()),
        Verdict::ErrInspect => {
            match recipe.segments.iter().enumerate().find(|(gi, _)| invalid[*gi] && contributes(*gi)) {
                Some((_, g)) => {
                    if let Some(run) = runs_in(g, |j| table[j] == PointState::Invalid).into_iter().max_by_key(Vec::len) {
                        format!("{} 胶路连续 {:.1} mm 测不了 · s={:.1}", g.name, run.len() as f32 * sp, g.s(run[0], sp))
                    } else { format!("{} 存在未完成测量点", g.name) }
                }
                None => "没有完成示教的有效检测图像".into(),
            }
        },
        _ => {
            let gi = segments.iter().enumerate().position(|(gi, s)| s.verdict == verdict && contributes(gi)).unwrap();
            let (g, s) = (&recipe.segments[gi], &segments[gi]);
            match verdict {
                Verdict::NgAbsolute => {
                    let p = g.position.as_ref().unwrap();
                    format!("{} 位置超出绝对限 [{:.2}, {:.2}] · 实测 {:.2}–{:.2}", g.name, p.abs_min, p.abs_max, s.min.unwrap_or(0.0), s.max.unwrap_or(0.0))
                }
                Verdict::NgPosition => {
                    let p = g.position.as_ref().unwrap();
                    format!("{} 位置连续超差 {:.1} mm > 允许 {:.1} mm{}", g.name, s.excursion_len, p.max_excursion_len, at(pos[gi].as_ref().and_then(|s| s.excursion_at)))
                }
                Verdict::NgWidth => {
                    let p = g.width.as_ref().unwrap();
                    format!(
                        "{} 胶宽 {:.2}–{:.2} mm 超出 [{:.2}, {:.2}] · 连续 {:.1} mm{}",
                        g.name,
                        s.w_min.unwrap_or(0.0),
                        s.w_max.unwrap_or(0.0),
                        p.lower(),
                        p.upper(),
                        s.w_excursion_len,
                        at(width[gi].as_ref().and_then(|s| s.excursion_at))
                    )
                }
                _ => format!("{} 局部超差 {:.1} mm，在允许范围内", g.name, s.excursion_len.max(s.w_excursion_len)),
            }
        }
    };

    Judgement { verdict, plc_code: verdict.plc_code(), fault_code: if verdict == Verdict::ErrInspect { fault::INVALID_POINTS } else { 0 }, reason, segments, gaps }
}

fn median_filter(values: &[f32], window: usize) -> Vec<f32> {
    let half = window / 2;
    if half == 0 || values.len() < window {
        return values.to_vec();
    }
    (0..values.len())
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(values.len());
            let mut w = values[lo..hi].to_vec();
            w.sort_by(|a, b| a.total_cmp(b));
            w[w.len() / 2]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::builtin;

    fn base(recipe: &Recipe) -> Vec<PointState> {
        vec![PointState::Measured { d: 0.0, w: 4.0 }; recipe.point_count()]
    }

    #[test]
    fn selected_views_or_then_whole_part_error_priority() {
        let mut doc = crate::recipe::samples().remove(0);
        doc.shots.truncate(2);
        let shot = &mut doc.shots[0];
        shot.views = [1, 2].into_iter().map(|view| crate::recipe::ShotViewSpec { view, enabled: true,
            path: shot.path.clone(), mm_per_px: shot.mm_per_px, detect: None, limits: None, calib: None }).collect();
        let recipe = doc.build().unwrap();
        let mut table = base(&recipe);
        for j in recipe.view_points(0, 1) { table[j] = PointState::Invalid; }
        let ok = judge(&recipe, &table);
        assert_eq!(ok.verdict, Verdict::Ok);
        assert_eq!(ok.segments[0].verdict, Verdict::ErrInspect);
        for j in recipe.view_points(0, 2) { table[j] = PointState::Gap; }
        assert_eq!(judge(&recipe, &table).verdict, Verdict::ErrInspect);
        for j in recipe.view_points(0, 1) { table[j] = PointState::Gap; }
        assert_eq!(judge(&recipe, &table).verdict, Verdict::NgGap);
        for j in recipe.owned_points(1) { table[j] = PointState::Invalid; }
        assert_eq!(judge(&recipe, &table).verdict, Verdict::ErrInspect);
    }

    #[test]
    fn projected_trial_ignores_pending_points_from_other_shots() {
        let recipe = builtin().remove(0);
        let projected = recipe.for_view(2, 1).unwrap();
        let mut table = vec![PointState::Pending; recipe.point_count()];
        for j in projected.view_points(2, 1) { table[j] = PointState::Gap; }
        assert_eq!(judge(&projected, &table).verdict, Verdict::NgGap);
    }

    #[test]
    fn long_gap_in_one_shot_is_ng() {
        let recipe = builtin().remove(1);
        let mut table = base(&recipe);
        let g = &recipe.segments[1];
        (g.first + 10..g.first + 18).for_each(|j| table[j] = PointState::Gap);
        let r = judge(&recipe, &table);
        assert_eq!(r.verdict, Verdict::NgGap);
        assert_eq!((r.gaps[0].segment, r.gaps[0].frames.as_slice(), r.gaps[0].s0, r.gaps[0].len), (1, &[1u8][..], 10.0, 8.0));
        assert!(r.reason.starts_with("P2 · J1 断胶 8.0 mm > 6.0 mm"), "{}", r.reason);
    }

    #[test]
    fn gaps_do_not_join_across_shots() {
        let recipe = builtin().remove(1);
        let mut table = base(&recipe);
        let (a, b) = (&recipe.segments[0], &recipe.segments[1]);
        // 第一个拍照点末尾 4 mm、第二个拍照点开头 4 mm：各自都不超 6 mm，不跨拍照点相连
        (a.first + a.count - 4..a.first + a.count).for_each(|j| table[j] = PointState::Gap);
        (b.first..b.first + 4).for_each(|j| table[j] = PointState::Gap);
        let r = judge(&recipe, &table);
        assert_eq!(r.verdict, Verdict::Ok, "{}", r.reason);
        assert!(r.gaps.is_empty());
    }

    #[test]
    fn short_excursion_is_allowed_long_one_is_ng() {
        let recipe = builtin().remove(0);
        let mut table = base(&recipe);
        let f = recipe.segments[2].first;
        (f + 20..f + 24).for_each(|j| table[j] = PointState::Measured { d: 3.0, w: 4.0 });
        assert_eq!(judge(&recipe, &table).verdict, Verdict::OkWithExcursion);
        (f + 20..f + 30).for_each(|j| table[j] = PointState::Measured { d: 3.0, w: 4.0 });
        let r = judge(&recipe, &table);
        assert_eq!(r.verdict, Verdict::NgPosition);
        assert!(r.reason.contains("s=20.0–30.0"), "{}", r.reason);
    }

    #[test]
    fn narrow_bead_is_ng_width() {
        let recipe = builtin().remove(0);
        let mut table = base(&recipe);
        let f = recipe.segments[0].first;
        (f + 5..f + 15).for_each(|j| table[j] = PointState::Measured { d: 0.0, w: 1.8 });
        let r = judge(&recipe, &table);
        assert_eq!(r.verdict, Verdict::NgWidth, "{}", r.reason);
        assert_eq!(r.segments[0].w_min, Some(1.8));
    }

    #[test]
    fn position_is_not_judged_without_limits() {
        let mut doc = crate::recipe::samples().remove(1);
        doc.limits.position = None;
        let recipe = doc.build().unwrap();
        let table = vec![PointState::Measured { d: 9.0, w: 4.0 }; recipe.point_count()];
        assert_eq!(judge(&recipe, &table).verdict, Verdict::Ok);
    }

    #[test]
    fn gap_bridged_by_thin_bead_is_one_gap() {
        let recipe = builtin().remove(1);
        let mut table = base(&recipe);
        let f = recipe.segments[1].first;
        // 断 4 mm、细 4 mm、断 4 mm：每段单看都不超限，连起来 12 mm 是一处断胶
        (f + 10..f + 14).for_each(|j| table[j] = PointState::Gap);
        (f + 14..f + 18).for_each(|j| table[j] = PointState::Measured { d: 0.0, w: 2.0 });
        (f + 18..f + 22).for_each(|j| table[j] = PointState::Gap);
        let r = judge(&recipe, &table);
        assert_eq!(r.verdict, Verdict::NgGap, "{}", r.reason);
        assert_eq!((r.gaps.len(), r.gaps[0].s0, r.gaps[0].len), (1, 10.0, 12.0));
        assert!(r.reason.starts_with("P2 · J1 断胶 12.0 mm（含相连的超差或测不了的站） > 6.0 mm"), "{}", r.reason);
    }

    #[test]
    fn short_gap_with_tapered_edges_stays_ok() {
        let recipe = builtin().remove(1);
        let mut table = base(&recipe);
        let f = recipe.segments[1].first;
        // 2 mm 断口两侧各 2 mm 收细：连起来 6 mm 不超断胶限，收细部分按胶宽规则至多算局部超差
        (f + 8..f + 10).for_each(|j| table[j] = PointState::Measured { d: 0.0, w: 2.0 });
        (f + 10..f + 12).for_each(|j| table[j] = PointState::Gap);
        (f + 12..f + 14).for_each(|j| table[j] = PointState::Measured { d: 0.0, w: 2.0 });
        let r = judge(&recipe, &table);
        assert!(r.verdict <= Verdict::OkWithExcursion, "{}", r.reason);
        assert!(r.gaps.is_empty());
    }

    #[test]
    fn scattered_short_gaps_are_ng() {
        let recipe = builtin().remove(1);
        let mut table = base(&recipe);
        let g = &recipe.segments[2];
        // 每隔一站缺一站：每处断口 1 mm，有胶站只占一半
        (g.first..g.first + g.count).step_by(2).for_each(|j| table[j] = PointState::Gap);
        let r = judge(&recipe, &table);
        assert_eq!(r.verdict, Verdict::NgGap, "{}", r.reason);
        assert!(r.gaps.is_empty());
        assert!(r.reason.starts_with("P3 · J1 胶条断续：有胶站只占") && r.reason.ends_with("< 80%"), "{}", r.reason);
    }

    #[test]
    fn whole_shot_without_bead_is_ng_gap() {
        let recipe = builtin().remove(1);
        let mut table = base(&recipe);
        let g = &recipe.segments[0];
        (g.first..g.first + g.count).for_each(|j| table[j] = PointState::Gap);
        let r = judge(&recipe, &table);
        assert_eq!((r.verdict, r.fault_code), (Verdict::NgGap, 0));
        assert!(r.reason.starts_with("P1 · J1 检测区内没找到胶（整段"), "{}", r.reason);
    }

    #[test]
    fn whole_shot_without_bead_is_ng_even_if_all_gaps_are_allowed() {
        let mut doc = crate::recipe::samples().remove(1);
        doc.limits.max_gap_len = 1000.0;
        let recipe = doc.build().unwrap();
        let mut table = base(&recipe);
        let g = &recipe.segments[0];
        (g.first..g.first + g.count).for_each(|j| table[j] = PointState::Gap);
        table[g.first + 3] = PointState::Invalid;
        let r = judge(&recipe, &table);
        assert_eq!(r.verdict, Verdict::NgGap, "{}", r.reason);
        assert!(r.reason.contains("检测区内没找到胶"), "{}", r.reason);
    }

    #[test]
    fn allowed_gap_in_short_shot_is_not_sparse() {
        let mut doc = crate::recipe::samples().remove(1);
        doc.limits.min_present = 0.95;
        let recipe = doc.build().unwrap();
        let mut table = base(&recipe);
        let f = recipe.segments[0].first;
        // 一处 6 mm 断口在允许范围内：比例再低也不按断续判
        (f + 2..f + 8).for_each(|j| table[j] = PointState::Gap);
        assert_eq!(judge(&recipe, &table).verdict, Verdict::Ok);
    }

    #[test]
    fn long_unmeasurable_run_is_err() {
        let recipe = builtin().remove(1);
        let mut table = base(&recipe);
        let f = recipe.segments[3].first;
        (f..f + 3).for_each(|j| table[j] = PointState::Invalid);
        let r = judge(&recipe, &table);
        assert_eq!((r.verdict, r.fault_code), (Verdict::ErrInspect, fault::INVALID_POINTS));
        assert!(r.reason.contains("P4 · J1 胶路连续 3.0 mm 测不了"), "{}", r.reason);
    }
}
