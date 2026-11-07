//! 拟合胶路（逐帧定向）的判定：输入是 lyFlow 1.2 `glue.fan_scan` / `glue.track_path` / `glue.lateral_runs` 输出的记录，
//! 规则迁自 1.1 `glue.bead_path`（findBeadPath 选方向与 ok 判据）与 `glue.bead_width` 拟合胶路分支（measureStations 的 D16），
//! 有一处有意不同：先只在最强峰与扫描里是局部峰的候选中选，选中的能找到胶就用它；都找不到胶时才按 1.1 在全部候选里选。
//! 1.1 按得分取前三个相距 > 15° 的方向，扇区边上得分一路升上去的方向也会成为候选，从那里起跑的跟踪接上同一条胶后
//! 覆盖率可能更高而被选中，方向读数随之跳动。

use serde_json::Value;

/// 拟合胶路至少要粗找到这么多胶点（1.1 写死的 3，等于 `track_path.minPoints`）。
const MIN_COARSE_POINTS: u64 = 3;
/// 候选不像胶时得分乘它（D15）。
const NOT_BEADLIKE_FACTOR: f64 = 0.1;
/// 候选峰的射线得分不到最强峰的这一比例时，得分再打对折。
const WEAK_RAY_RATIO: f64 = 0.25;
const WEAK_RAY_FACTOR: f64 = 0.5;
/// 中心偏离门限的下限（px）。
const CENTER_GATE_MIN_PX: f64 = 6.0;

/// 选方向的门限（1.1 `glue.bead_path` 的 minCoverage、sharpMin）。
#[derive(Clone, Copy, Debug)]
pub struct PathGate {
    pub min_coverage: f64,
    /// 候选的陡度 P25 不低于它才算像胶；None = 不看陡度（胶色跟踪：得分本身就是像不像胶）
    pub sharp_min: Option<f64>,
}

/// 选方向的结果。
#[derive(Clone, Debug, PartialEq)]
pub struct PathChoice {
    /// 找到了胶：选中候选的覆盖率达标、像胶、胶点 ≥ 3。为假时全段无胶
    pub ok: bool,
    /// 选中的候选在 `tracks` 里的下标；一个有效候选都没有时为 None
    pub chosen: Option<usize>,
    /// 选中候选的起始方向（度）；没有候选时取扇区中线
    pub heading: Option<f64>,
    /// 选中候选的覆盖率（胶点数 / 步数）
    pub coverage: f64,
    /// 选中候选的陡度 P25；可算出的胶点不到 3 个时为 None
    pub sharpness: Option<f64>,
    /// 没找到胶的原因，与 1.1 `info.reason` 同一句；ok 时为空
    pub reason: String,
}

struct Candidate {
    heading: f64,
    found: u64,
    coverage: f64,
    sharpness: Option<f64>,
    score: f64,
}

/// 按 1.1 findBeadPath 选方向：`scan` 是 `glue.fan_scan.scan`，`tracks` 是接同一扇形扫描的各 `glue.track_path.track`
/// （peakIndex 0、1、2；峰不存在的 `valid = false`，不参与）。先只在最强峰与局部峰候选中选，选中的找不到胶时再在全部候选中选。
/// 每个候选的得分 = 覆盖率 ×（像胶 ? 1 : 0.1），
/// 它的峰在扫描里的得分不到最强峰的四分之一时再 × 0.5，取得分最高的（同分取靠前的）；
/// 然后依次查覆盖率 ≥ minCoverage、像胶、胶点 ≥ 3，第一条不满足的给出原因。
/// `track_path` 须用 `minPoints = 3`，ok 时选中候选的 `line` 才是拟合胶路。记录缺字段时返回 Err。
pub fn choose_path(scan: &Value, tracks: &[Value], gate: PathGate) -> Result<PathChoice, String> {
    let peaks = scan["peaks"].as_array().ok_or("扇形扫描结果没有 peaks")?;
    let peak_score = |k: u64| peaks.get(k as usize).and_then(|p| p["score"].as_f64());
    let strongest = peak_score(0).unwrap_or(0.0);
    let scores = scan["scores"].as_array().ok_or("扇形扫描结果没有 scores")?;
    let score_at = |i: usize| {
        scores
            .get(i)
            .and_then(Value::as_f64)
            .unwrap_or(f64::NEG_INFINITY)
    };
    // 不在扇区两端、且不低于左右相邻方向
    let local_peak = |k: u64| {
        let Some(i) = peaks
            .get(k as usize)
            .and_then(|p| p["index"].as_u64())
            .map(|i| i as usize)
        else {
            return false;
        };
        i > 0
            && i + 1 < scores.len()
            && score_at(i) >= score_at(i - 1)
            && score_at(i) >= score_at(i + 1)
    };
    let mut candidates: Vec<(usize, Candidate, bool)> = Vec::new();
    for (i, track) in tracks.iter().enumerate() {
        let what = |key: &str| format!("第 {} 个粗跟踪结果缺少 {key}", i + 1);
        if !track["valid"].as_bool().ok_or_else(|| what("valid"))? {
            continue;
        }
        let steps = track["steps"].as_u64().ok_or_else(|| what("steps"))?;
        let found = track["found"].as_u64().ok_or_else(|| what("found"))?;
        let sharpness = track["sharpness"].as_f64();
        // 胶色跟踪另给去掉无信息步（高光、暗区）后的覆盖率，有就用它
        let coverage = match track["coverageInformative"].as_f64() {
            Some(c) if c.is_finite() => c,
            _ if steps > 0 => found as f64 / steps as f64,
            _ => 0.0,
        };
        let beadlike = beadlike(sharpness, gate);
        let mut score = if beadlike {
            coverage
        } else {
            NOT_BEADLIKE_FACTOR * coverage
        };
        let ray = track["peakIndex"].as_u64().and_then(peak_score);
        if strongest > 0.0 && ray.is_some_and(|r| r < WEAK_RAY_RATIO * strongest) {
            score *= WEAK_RAY_FACTOR;
        }
        let local = track["peakIndex"]
            .as_u64()
            .is_none_or(|k| k == 0 || local_peak(k));
        let c = Candidate {
            heading: track["heading"].as_f64().ok_or_else(|| what("heading"))?,
            found,
            coverage,
            sharpness,
            score,
        };
        candidates.push((i, c, local));
    }
    let best = |only_local: bool| {
        candidates
            .iter()
            .filter(|(_, _, local)| *local || !only_local)
            .fold(None::<&(usize, Candidate, bool)>, |b, c| match b {
                Some(b) if c.1.score <= b.1.score => Some(b),
                _ => Some(c),
            })
    };
    let chosen = match best(true) {
        Some(c) if verdict(&c.1, gate).is_empty() => Some(c),
        _ => best(false),
    };
    let Some((i, c, _)) = chosen else {
        let sector = scan["sector"].as_array();
        let mid = sector.and_then(|s| Some(0.5 * (s.first()?.as_f64()? + s.get(1)?.as_f64()?)));
        return Ok(PathChoice {
            ok: false,
            chosen: None,
            heading: mid,
            coverage: 0.0,
            sharpness: None,
            reason: "扇区里一个方向都没有（检查 sector / heading）".into(),
        });
    };
    let reason = verdict(c, gate);
    Ok(PathChoice {
        ok: reason.is_empty(),
        chosen: Some(*i),
        heading: Some(c.heading),
        coverage: c.coverage,
        sharpness: c.sharpness,
        reason,
    })
}

fn beadlike(sharpness: Option<f64>, gate: PathGate) -> bool {
    gate.sharp_min
        .is_none_or(|min| sharpness.unwrap_or(-1.0) >= min)
}

/// 依次查覆盖率 ≥ minCoverage、像胶、胶点 ≥ 3，返回第一条不满足的原因（与 1.1 `info.reason` 同一句）；都满足为空。
fn verdict(c: &Candidate, gate: PathGate) -> String {
    let sharp = c.sharpness.unwrap_or(-1.0);
    if c.coverage < gate.min_coverage {
        format!(
            "覆盖率 {:.2} 低于 minCoverage {:.2}",
            c.coverage, gate.min_coverage
        )
    } else if let Some(min) = gate.sharp_min.filter(|_| !beadlike(c.sharpness, gate)) {
        format!(
            "找到的暗带边缘太缓（陡度 P25 = {sharp:.2} < sharpMin {min:.2}），像压痕或阴影，不像胶"
        )
    } else if c.found < MIN_COARSE_POINTS {
        "粗找到的胶点不到 3 个".into()
    } else {
        String::new()
    }
}

/// 逐站有无胶的门限（1.1 `glue.bead_width` 的同名参数）。
#[derive(Clone, Copy, Debug)]
pub struct StationGate {
    /// 胶宽范围 [下限, 上限]（px）：不在范围内的暗段不参与
    pub width_range: [f64; 2],
    /// 宽度 ≥ 它 × 参考宽才算有胶
    pub present_ratio: f64,
    /// 中心偏离胶路 ≤ max(6, 它 × 参考宽) 才算有胶
    pub center_ratio: f64,
    /// 峰值 ≥ 它 × 参考峰值才算有胶
    pub contrast_ratio: f64,
}

/// 一站的判定结果。
#[derive(Clone, Debug, PartialEq)]
pub struct StationCall {
    pub s: f64,
    pub center: [f64; 2],
    /// 单位法向，指向行进方向右侧；lo、hi 沿它量
    pub normal: [f64; 2],
    /// 有胶时胶两边沿法向的偏移 (lo, hi)（px），无胶为 None
    pub bead: Option<(f64, f64)>,
    /// 无信息（胶色卡尺）：没量到胶，但该有胶的地方被高光、暗区盖住，不算缺胶也不算有胶
    pub unknown: bool,
}

struct Run {
    lo: f64,
    hi: f64,
    width: f64,
    mid: f64,
    peak: f64,
}

/// 离胶路最近（|mid| 最小，同距取靠前）的一段。
fn nearest(runs: &[Run]) -> Option<&Run> {
    runs.iter().fold(None, |best: Option<&Run>, r| match best {
        Some(b) if r.mid.abs() >= b.mid.abs() => Some(b),
        _ => Some(r),
    })
}

/// 中位数；偶数个取中间两个的均值（与 1.1 相同）。
fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    Some(if v.len() % 2 == 1 {
        v[m]
    } else {
        0.5 * (v[m - 1] + v[m])
    })
}

fn xy(v: &Value) -> Option<[f64; 2]> {
    Some([v.get(0)?.as_f64()?, v.get(1)?.as_f64()?])
}

/// 按 1.1 拟合胶路分支（D16）逐站判有无胶。`runs` 是选中候选的 `glue.lateral_runs.runs`，按 `split = half_max`、
/// `mergePeakRatio = 0`、`segmentMin = contrastMin / 2`、`peakMin = contrastMin`、`mergeWidthMax = 胶宽上限` 跑；
/// 1.1 按连通段拆分，胶条旁有平行暗带时两者连成一段超宽、整站被丢，这里有意不同。
/// 每站只看宽度在胶宽范围内的暗段，取离胶路最近的一段；参考宽、参考峰值是各站这一段宽度、峰值的中位数。
/// 有胶 = 宽度 ≥ presentRatio × 参考宽、|中心| ≤ max(6, centerRatio × 参考宽)、峰值 ≥ contrastRatio × 参考峰值。
/// 不要求两边完整（1.1 只在诊断用的候选胶边上要求）。`path_ok` 为假（没找到胶）时全部无胶。记录缺字段时返回 Err。
pub fn judge_stations(
    runs: &Value,
    gate: StationGate,
    path_ok: bool,
) -> Result<Vec<StationCall>, String> {
    let stations = runs["stations"]
        .as_array()
        .ok_or("横向卡尺结果没有 stations")?;
    let [w_min, w_max] = gate.width_range;
    let mut calls = Vec::with_capacity(stations.len());
    let mut picked = Vec::with_capacity(stations.len());
    for (i, st) in stations.iter().enumerate() {
        let what = |key: &str| format!("横向卡尺第 {} 站缺少 {key}", i + 1);
        calls.push(StationCall {
            s: st["s"].as_f64().ok_or_else(|| what("s"))?,
            center: xy(&st["center"]).ok_or_else(|| what("center"))?,
            normal: xy(&st["normal"]).ok_or_else(|| what("normal"))?,
            bead: None,
            unknown: false,
        });
        let mut kept = Vec::new();
        for r in st["runs"].as_array().ok_or_else(|| what("runs"))? {
            let num = |key: &str| r[key].as_f64().ok_or_else(|| what(&format!("暗段 {key}")));
            let run = Run {
                lo: num("lo")?,
                hi: num("hi")?,
                width: num("width")?,
                mid: num("mid")?,
                peak: num("peak")?,
            };
            if run.width >= w_min && run.width <= w_max {
                kept.push(run);
            }
        }
        picked.push(kept);
    }
    if !path_ok {
        return Ok(calls);
    }
    let refs: Vec<&Run> = picked.iter().filter_map(|k| nearest(k)).collect();
    let (Some(w_ref), Some(peak_ref)) = (
        median(refs.iter().map(|r| r.width).collect()),
        median(refs.iter().map(|r| r.peak).collect()),
    ) else {
        return Ok(calls);
    };
    let min_width = gate.present_ratio * w_ref;
    let max_offset = CENTER_GATE_MIN_PX.max(gate.center_ratio * w_ref);
    let min_peak = gate.contrast_ratio * peak_ref;
    for ((call, kept), st) in calls.iter_mut().zip(&picked).zip(stations) {
        if let Some(r) = nearest(kept) {
            if r.width >= min_width && r.mid.abs() <= max_offset && r.peak >= min_peak {
                call.bead = Some((r.lo, r.hi));
            }
        }
        if call.bead.is_none() {
            call.unknown = hidden_by_unknown(st, w_ref);
        }
    }
    Ok(calls)
}

/// 胶色卡尺一站没量到胶时，该有胶的区间是胶路 ± 参考宽的一半（规则见 `super::hidden_by_unknown`）。
fn hidden_by_unknown(station: &Value, w_ref: f64) -> bool {
    let spans = |key: &str| -> Vec<(f64, f64)> {
        station[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| Some((v["lo"].as_f64()?, v["hi"].as_f64()?)))
            .collect()
    };
    super::hidden_by_unknown(
        &spans("unknownSpans"),
        &spans("runs"),
        (-0.5 * w_ref, 0.5 * w_ref),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    /// 每个峰都是扫描里的局部峰：峰 k 在第 2k+1 个方向，两旁的方向得分为 0。
    fn scan(peak_scores: &[f64]) -> Value {
        let mut scores = vec![0.0; 2 * peak_scores.len() + 1];
        let peaks: Vec<Value> = peak_scores
            .iter()
            .enumerate()
            .map(|(k, s)| {
                scores[2 * k + 1] = *s;
                json!({"index": 2 * k + 1, "angle": -90.0 + 20.0 * k as f64, "score": s})
            })
            .collect();
        json!({"sector": [-120.0, -60.0], "scores": scores, "peaks": peaks})
    }

    #[test]
    fn color_track_without_sharpness_counts_coverage_over_informative_steps() {
        // 防的回归：胶色跟踪没有陡度，按灰度门限一律判"不像胶"而回退；或高光、暗区这些无信息的步算进覆盖率，
        // 湿胶反光多的帧被判覆盖率不足
        let gate = PathGate {
            min_coverage: 0.3,
            sharp_min: None,
        };
        let color = |steps: u64, found: u64, informative: Value| {
            json!({"valid": true, "peakIndex": 0, "heading": -90.0, "steps": steps, "found": found,
                   "sharpness": null, "coverageInformative": informative})
        };
        let cases = [
            (color(20, 4, json!(0.8)), true, 0.8),
            (color(20, 4, Value::Null), false, 0.2),
            (color(20, 16, json!(0.1)), false, 0.1),
        ];
        for (track, ok, coverage) in cases {
            let choice = choose_path(&scan(&[1.0]), &[track], gate).unwrap();
            assert_eq!(
                (choice.ok, choice.coverage),
                (ok, coverage),
                "{}",
                choice.reason
            );
        }
    }

}
