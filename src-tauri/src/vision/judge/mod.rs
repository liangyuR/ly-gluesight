//! 测量判定（LF-3）：lyFlow 1.2 的测量算子只给中间结果，有无胶、像不像胶、选哪个方向、每站取哪一段、取哪条边都在这里按算子输出的记录判。

pub mod edge;
pub mod fit;
pub mod taught;

/// 没量到胶的一站，该有胶的区间被无信息区间与胶色段盖住至少这一比例才算无信息（不算缺胶）。
const UNKNOWN_COVER: f64 = 0.8;

/// 胶色卡尺一站没量到胶时，该有胶的区间 `expected`（沿法向，px）被无信息区间（高光、暗区）与已量到的胶色段一起盖住
/// 至少 `UNKNOWN_COVER`、且其中有无信息的部分：胶被反光遮住，不能说缺胶。灰度卡尺没有无信息区间，恒为假。
fn hidden_by_unknown(unknown: &[(f64, f64)], runs: &[(f64, f64)], expected: (f64, f64)) -> bool {
    let (a, b) = expected;
    if unknown.is_empty() || b <= a {
        return false;
    }
    let covered = |set: &mut dyn Iterator<Item = &(f64, f64)>| {
        let mut parts: Vec<(f64, f64)> = set
            .map(|&(lo, hi)| (lo.max(a), hi.min(b)))
            .filter(|(lo, hi)| hi > lo)
            .collect();
        parts.sort_by(|x, y| x.0.total_cmp(&y.0));
        let (mut total, mut end) = (0.0, a);
        for (lo, hi) in parts {
            total += (hi - lo.max(end)).max(0.0);
            end = end.max(hi);
        }
        total
    };
    covered(&mut unknown.iter()) > 0.0
        && covered(&mut unknown.iter().chain(runs)) >= UNKNOWN_COVER * (b - a)
}
