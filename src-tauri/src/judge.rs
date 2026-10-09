use serde::{Deserialize, Serialize};

use crate::recipe::{JudgeParams, Recipe};

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
    /// d：位置量；w：胶宽（没测胶宽时为 NaN）
    Measured { d: f32, w: f32 },
    /// 找到内边但未找到胶条
    Gap,
    /// 该点测不了（内边未找到、被遮挡、出了图像）
    Invalid,
}

impl PointState {
    #[cfg(test)]
    pub fn measured(d: f32) -> Self {
        PointState::Measured { d, w: f32::NAN }
    }
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

/// 一段里某个量（位置或胶宽）的统计：极值、有没有超绝对限、最长连续超差。
#[derive(Default)]
struct Stat {
    min: Option<f32>,
    max: Option<f32>,
    absolute: bool,
    excursion_len: f32,
    /// 记在这一段的最长那次超差的弧长起止
    excursion_at: Option<(f32, f32)>,
}

/// 沿整条胶路（闭合时首尾相接）按连续测到的点做中值滤波：滤波与连续超差都不在分段边界处断开，
/// 否则折线导入的胶路每条边一段，几毫米长的超差会被切成好几截、短段上的单点噪声也滤不掉。
fn filtered(table: &[PointState], closed: bool, window: usize, value: impl Fn(&PointState) -> Option<f32>) -> Vec<Option<f32>> {
    let mut out = vec![None; table.len()];
    for run in runs_where(table, closed, |_, p| value(p).is_some()) {
        let v: Vec<f32> = run.iter().map(|&j| value(&table[j]).unwrap()).collect();
        for (&j, f) in run.iter().zip(median_filter(&v, window)) {
            out[j] = Some(f);
        }
    }
    out
}

/// 各段的统计。超差按整条胶路上的连续区间算（各点按所在段的公差判），一次超差只判一次：
/// 全长记在点最多的那一段、按它的允许长度判；碰到的其余段只记落在本段里的那一截。
fn stats(recipe: &Recipe, table: &[PointState], values: &[Option<f32>], limits: impl Fn(usize) -> Option<JudgeParams>) -> Vec<Stat> {
    let seg = |j: usize| recipe.points.seg[j] as usize;
    let mut st: Vec<Stat> = recipe.segments.iter().map(|_| Stat::default()).collect();
    let params: Vec<Option<JudgeParams>> = (0..recipe.segments.len()).map(&limits).collect();
    for (j, v) in values.iter().enumerate() {
        let (Some(v), Some(p)) = (*v, &params[seg(j)]) else { continue };
        let s = &mut st[seg(j)];
        s.min = Some(s.min.map_or(v, |m| m.min(v)));
        s.max = Some(s.max.map_or(v, |m| m.max(v)));
        s.absolute |= v < p.abs_min || v > p.abs_max;
    }
    let out = |j: usize| values[j].zip(params[seg(j)].as_ref()).is_some_and(|(v, p)| v < p.lower() || v > p.upper());
    let sp = recipe.spacing;
    for run in runs_where(table, recipe.closed, |j, _| out(j)) {
        // 各段在这次超差里的那一截（run 里的下标范围）
        struct Share {
            seg: usize,
            from: usize,
            to: usize,
        }
        let mut shares: Vec<Share> = Vec::new();
        for (i, &j) in run.iter().enumerate() {
            match shares.last_mut() {
                Some(sh) if sh.seg == seg(j) => sh.to = i,
                _ => shares.push(Share { seg: seg(j), from: i, to: i }),
            }
        }
        let owner = shares.iter().max_by_key(|sh| (sh.to - sh.from, std::cmp::Reverse(sh.from))).unwrap().seg;
        for sh in &shares {
            let (from, to) = if sh.seg == owner { (0, run.len() - 1) } else { (sh.from, sh.to) };
            let len = (to - from + 1) as f32 * sp;
            let s = &mut st[sh.seg];
            if len > s.excursion_len {
                s.excursion_len = len;
                s.excursion_at = Some((run[from] as f32 * sp, (run[to] + 1) as f32 * sp));
            }
        }
    }
    st
}

pub fn judge(recipe: &Recipe, table: &[PointState]) -> Judgement {
    let sp = recipe.spacing;
    let n = table.len();
    let closed = recipe.closed;

    if let Some(j) = table.iter().position(|p| *p == PointState::Pending) {
        return Judgement::error(fault::INVALID_POINTS, format!("测量点 j={j} 未填写"));
    }
    for run in runs_where(table, closed, |_, p| *p == PointState::Invalid) {
        let len = run.len() as f32 * sp;
        if len > MAX_INVALID_LEN {
            let seg = &recipe.segments[recipe.points.seg[run[0]] as usize];
            return Judgement::error(fault::INVALID_POINTS, format!("{} 内边连续 {len:.1} mm 没测到 · s={:.1}", seg.name, run[0] as f32 * sp));
        }
    }

    let d = filtered(table, closed, recipe.filter_window, |p| match p {
        PointState::Measured { d, .. } => Some(*d),
        _ => None,
    });
    let w = filtered(table, closed, recipe.filter_window, |p| match p {
        PointState::Measured { w, .. } if w.is_finite() => Some(*w),
        _ => None,
    });
    let pos_stats = stats(recipe, table, &d, |gi| Some(recipe.segments[gi].params.clone()));
    let width_stats = stats(recipe, table, &w, |gi| recipe.segments[gi].width.clone());
    let pos_at: Vec<_> = pos_stats.iter().map(|s| s.excursion_at).collect();
    let width_at: Vec<_> = width_stats.iter().map(|s| s.excursion_at).collect();
    let at = |r: Option<(f32, f32)>| r.map(|(a, b)| format!(" · s={a:.1}–{b:.1}")).unwrap_or_default();
    let mut segments: Vec<SegmentResult> = recipe
        .segments
        .iter()
        .zip(pos_stats.into_iter().zip(width_stats))
        .map(|(seg, (pos, width))| {
            let width = seg.width.as_ref().map(|_| width);
            let mut verdict = if pos.absolute {
                Verdict::NgAbsolute
            } else if pos.excursion_len > seg.params.max_excursion_len {
                Verdict::NgPosition
            } else if pos.excursion_len > 0.0 {
                Verdict::OkWithExcursion
            } else {
                Verdict::Ok
            };
            if let (Some(w), Some(p)) = (&width, &seg.width) {
                let wv = if w.absolute || w.excursion_len > p.max_excursion_len {
                    Verdict::NgWidth
                } else if w.excursion_len > 0.0 {
                    Verdict::OkWithExcursion
                } else {
                    Verdict::Ok
                };
                verdict = verdict.max(wv);
            }
            SegmentResult {
                verdict,
                min: pos.min,
                max: pos.max,
                excursion_len: pos.excursion_len,
                w_min: width.as_ref().and_then(|w| w.min),
                w_max: width.as_ref().and_then(|w| w.max),
                w_excursion_len: width.as_ref().map_or(0.0, |w| w.excursion_len),
            }
        })
        .collect();

    let mut gaps = Vec::new();
    for run in runs_where(table, closed, |_, p| *p == PointState::Gap) {
        let len = run.len() as f32 * sp;
        if len <= recipe.max_gap_len {
            continue;
        }
        let segment = recipe.points.seg[run[0]] as usize;
        let mut frames: Vec<u8> = run.iter().map(|&j| recipe.points.k[j]).collect();
        frames.dedup();
        segments[segment].verdict = segments[segment].verdict.max(Verdict::NgGap);
        gaps.push(GapRun { segment, s0: run[0] as f32 * sp, s1: (run[run.len() - 1] + 1) as f32 * sp, len, frames });
    }

    let verdict = segments.iter().map(|s| s.verdict).max().unwrap_or(Verdict::Ok);
    let reason = match verdict {
        Verdict::NgGap => {
            let g = &gaps[0];
            let frames = if g.frames.is_empty() {
                String::new()
            } else {
                let f = g.frames.iter().map(|k| format!("帧 {k}")).collect::<Vec<_>>().join(" + ");
                format!(" · {f}{}", if g.frames.len() > 1 { "，跨帧合并" } else { "" })
            };
            format!("{} 断胶 {:.1} mm > {:.1} mm · s={:.1}–{:.1}{frames}", recipe.segments[g.segment].name, g.len, recipe.max_gap_len, g.s0, g.s1)
        }
        Verdict::Ok => format!("{} 段全部合格 · {n} 点", segments.len()),
        _ => {
            let (gi, s) = segments.iter().enumerate().find(|(_, s)| s.verdict == verdict).unwrap();
            let seg = &recipe.segments[gi];
            match verdict {
                Verdict::NgAbsolute => format!(
                    "{} 超出绝对限 [{:.2}, {:.2}] · 实测 {:.2}–{:.2}",
                    seg.name,
                    seg.params.abs_min,
                    seg.params.abs_max,
                    s.min.unwrap_or(0.0),
                    s.max.unwrap_or(0.0)
                ),
                Verdict::NgPosition => format!("{} 连续超差 {:.1} mm > 允许 {:.1} mm{}", seg.name, s.excursion_len, seg.params.max_excursion_len, at(pos_at[gi])),
                Verdict::NgWidth => {
                    let p = seg.width.as_ref().unwrap();
                    format!(
                        "{} 胶宽 {:.2}–{:.2} mm 超出 [{:.2}, {:.2}] · 连续 {:.1} mm{}",
                        seg.name,
                        s.w_min.unwrap_or(0.0),
                        s.w_max.unwrap_or(0.0),
                        p.lower(),
                        p.upper(),
                        s.w_excursion_len,
                        at(width_at[gi])
                    )
                }
                _ => {
                    let len = s.excursion_len.max(s.w_excursion_len);
                    format!("{} 局部超差 {len:.1} mm ≤ 允许 {:.1} mm", seg.name, seg.params.max_excursion_len)
                }
            }
        }
    };

    Judgement { verdict, plc_code: verdict.plc_code(), fault_code: 0, reason, segments, gaps }
}

/// 满足条件的连续区间。闭合胶路首尾相接处合并为一段。
fn runs_where(table: &[PointState], closed: bool, pred: impl Fn(usize, &PointState) -> bool) -> Vec<Vec<usize>> {
    let n = table.len();
    let mut runs = Vec::new();
    let mut cur = Vec::new();
    if !closed {
        for (j, p) in table.iter().enumerate() {
            if pred(j, p) {
                cur.push(j);
            } else if !cur.is_empty() {
                runs.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            runs.push(cur);
        }
        return runs;
    }
    let Some(start) = (0..n).find(|&j| !pred(j, &table[j])) else {
        return if n > 0 { vec![(0..n).collect()] } else { Vec::new() };
    };
    for i in 1..=n {
        let j = (start + i) % n;
        if pred(j, &table[j]) {
            cur.push(j);
        } else if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
    }
    runs
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
        vec![PointState::measured(0.75); recipe.point_count()]
    }

    #[test]
    fn gap_split_across_frames_is_merged() {
        let recipe = builtin().remove(0);
        let mut table = base(&recipe);
        let j = (0..recipe.point_count() - 1).find(|&j| recipe.points.k[j] == 1 && recipe.points.k[j + 1] == 2).unwrap();
        table[j] = PointState::Gap;
        table[j + 1] = PointState::Gap;
        let r = judge(&recipe, &table);
        assert_eq!(r.verdict, Verdict::NgGap);
        assert_eq!(r.gaps[0].frames, vec![1, 2]);
    }

    #[test]
    fn gap_wrapping_path_start_is_one_run() {
        let recipe = builtin().remove(0);
        let mut table = base(&recipe);
        let n = table.len();
        table[0] = PointState::Gap;
        table[n - 1] = PointState::Gap;
        let r = judge(&recipe, &table);
        assert_eq!(r.gaps.len(), 1);
        assert_eq!(r.gaps[0].len, 1.0);
    }

    #[test]
    fn short_excursion_is_allowed_long_one_is_ng() {
        let recipe = builtin().remove(0);
        let mut table = base(&recipe);
        (100..104).for_each(|j| table[j] = PointState::measured(1.6));
        assert_eq!(judge(&recipe, &table).verdict, Verdict::OkWithExcursion);
        (100..110).for_each(|j| table[j] = PointState::measured(1.6));
        assert_eq!(judge(&recipe, &table).verdict, Verdict::NgPosition);
    }

    #[test]
    fn narrow_bead_is_ng_width() {
        let mut doc = crate::recipe::samples().remove(0);
        let width = JudgeParams { nominal: 2.0, tol_upper: 0.7, tol_lower: 0.6, abs_min: 0.8, abs_max: 3.8, max_excursion_len: 3.0 };
        doc.line.width = Some(width.clone());
        doc.corner.width = Some(width);
        let recipe = doc.build().unwrap();
        let mut table = vec![PointState::Measured { d: 0.75, w: 2.0 }; recipe.point_count()];
        (200..220).for_each(|j| table[j] = PointState::Measured { d: 0.75, w: 1.1 });
        assert_eq!(judge(&recipe, &table).verdict, Verdict::NgWidth);
    }
}
