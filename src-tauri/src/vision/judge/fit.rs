//! 拟合胶路（逐帧定向）的判定：输入是 lyFlow 1.2 `glue.fan_scan` / `glue.track_path` / `glue.lateral_runs` 输出的记录，
//! 规则迁自 1.1 `glue.bead_path`（findBeadPath 选方向与 ok 判据）与 `glue.bead_width` 拟合胶路分支（measureStations 的 D16），
//! 有一处有意不同：先只在最强峰与扫描里是局部峰的候选中选，选中的能找到胶就用它；都找不到胶时才按 1.1 在全部候选里选。
//! 1.1 按得分取前三个相距 > 15° 的方向，扇区边上得分一路升上去的方向也会成为候选，从那里起跑的跟踪接上同一条胶后
//! 覆盖率可能更高而被选中，方向读数随之跳动。
//! 搜索与回退（扇区 = 示教方向 ± 容差）用同一套规则，只是扇形扫描的扇区不同。

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
    /// 候选的陡度 P25 不低于它才算像胶
    pub sharp_min: f64,
}

/// 选方向的结果。
#[derive(Clone, Debug, PartialEq)]
pub struct PathChoice {
    /// 找到了胶：选中候选的覆盖率达标、像胶、胶点 ≥ 3。为假时 GS 按回退处理，回退仍为假则全段无胶
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
        let coverage = if steps > 0 {
            found as f64 / steps as f64
        } else {
            0.0
        };
        let beadlike = sharpness.unwrap_or(-1.0) >= gate.sharp_min;
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

/// 依次查覆盖率 ≥ minCoverage、像胶、胶点 ≥ 3，返回第一条不满足的原因（与 1.1 `info.reason` 同一句）；都满足为空。
fn verdict(c: &Candidate, gate: PathGate) -> String {
    let sharp = c.sharpness.unwrap_or(-1.0);
    if c.coverage < gate.min_coverage {
        format!(
            "覆盖率 {:.2} 低于 minCoverage {:.2}",
            c.coverage, gate.min_coverage
        )
    } else if sharp < gate.sharp_min {
        format!(
            "找到的暗带边缘太缓（陡度 P25 = {sharp:.2} < sharpMin {:.2}），像压痕或阴影，不像胶",
            gate.sharp_min
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

/// 按 1.1 拟合胶路分支（D16）逐站判有无胶。`runs` 是选中候选的 `glue.lateral_runs.runs`，须按 1.1 的等价设置跑
/// （`split = connected`、`mergePeakRatio = 0`、`segmentMin = contrastMin / 2`、`peakMin = contrastMin`、`mergeWidthMax = 胶宽上限`）。
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
    for (call, kept) in calls.iter_mut().zip(&picked) {
        if let Some(r) = nearest(kept) {
            if r.width >= min_width && r.mid.abs() <= max_offset && r.peak >= min_peak {
                call.bead = Some((r.lo, r.hi));
            }
        }
    }
    Ok(calls)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    const GATE: PathGate = PathGate {
        min_coverage: 0.3,
        sharp_min: 0.4,
    };

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

    fn track(peak: usize, steps: u64, found: u64, sharpness: Option<f64>) -> Value {
        json!({"valid": true, "peakIndex": peak, "heading": -90.0 + 20.0 * peak as f64,
               "steps": steps, "found": found, "sharpness": sharpness})
    }

    #[test]
    fn chosen_candidate_failing_a_gate_is_not_ok_with_that_gates_reason() {
        let no_peak = json!({"valid": false, "peakIndex": 0, "heading": null, "steps": null, "found": null, "sharpness": null});
        let cases = [
            (vec![track(0, 10, 8, Some(0.9))], true, ""),
            (
                vec![track(0, 10, 2, Some(0.9))],
                false,
                "覆盖率 0.20 低于 minCoverage 0.30",
            ),
            (
                vec![track(0, 10, 8, Some(0.25))],
                false,
                "陡度 P25 = 0.25 < sharpMin 0.40",
            ),
            (vec![track(0, 10, 8, None)], false, "陡度 P25 = -1.00"),
            (vec![track(0, 6, 2, Some(0.9))], false, "胶点不到 3 个"),
            (vec![no_peak], false, "扇区里一个方向都没有"),
        ];
        for (tracks, ok, reason) in cases {
            let c = choose_path(&scan(&[40.0]), &tracks, GATE).unwrap();
            assert_eq!(c.ok, ok, "{reason}");
            assert!(
                c.reason.contains(reason) && c.reason.is_empty() == ok,
                "{} 应含 {reason}",
                c.reason
            );
        }
    }

    #[test]
    fn candidate_score_penalises_not_beadlike_and_weak_rays() {
        let cases = [
            // 覆盖率高但不像胶（×0.1）输给覆盖率低而像胶的
            (
                vec![40.0, 30.0],
                vec![track(0, 10, 9, Some(0.1)), track(1, 10, 5, Some(0.9))],
                1,
            ),
            // 峰的射线得分不到最强的四分之一（×0.5）：0.9 × 0.5 < 0.6
            (
                vec![40.0, 9.0],
                vec![track(0, 10, 6, Some(0.9)), track(1, 10, 9, Some(0.9))],
                0,
            ),
            // 恰好四分之一不打折：0.9 > 0.6
            (
                vec![40.0, 10.0],
                vec![track(0, 10, 6, Some(0.9)), track(1, 10, 9, Some(0.9))],
                1,
            ),
            // 同分取靠前的峰
            (
                vec![40.0, 30.0],
                vec![track(0, 10, 6, Some(0.9)), track(1, 10, 6, Some(0.9))],
                0,
            ),
        ];
        for (scores, tracks, chosen) in cases {
            let c = choose_path(&scan(&scores), &tracks, GATE).unwrap();
            assert_eq!(c.chosen, Some(chosen), "峰得分 {scores:?}");
            assert_eq!(c.heading, tracks[chosen]["heading"].as_f64());
        }
    }

    #[test]
    fn non_local_peak_candidate_is_used_only_when_local_ones_find_no_bead() {
        // 防的回归：扇区边上得分一路升上去的方向当了候选，从那里起跑接上同一条胶、覆盖率更高而被选中；
        // 也防反过来把它彻底排除，局部峰候选都找不到胶时不再退到它
        let found = |top: u64| [track(0, 10, top, Some(0.9)), track(1, 10, 9, Some(0.9))];
        for (scores, peak1, top_found, chosen) in [
            (vec![10.0, 40.0, 20.0, 25.0, 30.0], 4, 6, 0), // 第二个峰在扇区末端
            (vec![10.0, 40.0, 20.0, 30.0, 35.0, 5.0], 3, 6, 0), // 第二个峰低于右邻
            (vec![10.0, 40.0, 20.0, 30.0, 25.0, 5.0], 3, 6, 1), // 第二个峰是局部峰，照 1.1 按覆盖率选
            (vec![10.0, 40.0, 20.0, 25.0, 30.0], 4, 2, 1),      // 最强峰找不到胶，退到末端那个
        ] {
            let s = json!({"sector": [-120.0, -60.0], "scores": scores,
                "peaks": [{"index": 1, "angle": -90.0, "score": 40.0}, {"index": peak1, "angle": -70.0, "score": scores[peak1]}]});
            assert_eq!(
                choose_path(&s, &found(top_found), GATE).unwrap().chosen,
                Some(chosen),
                "第二个峰在 {peak1}，最强峰找到 {top_found}"
            );
        }
    }

    const STATIONS: StationGate = StationGate {
        width_range: [10.0, 30.0],
        present_ratio: 0.6,
        center_ratio: 0.3,
        contrast_ratio: 0.3,
    };

    fn run(lo: f64, hi: f64, peak: f64) -> Value {
        json!({"lo": lo, "hi": hi, "width": hi - lo, "mid": 0.5 * (lo + hi), "peak": peak,
               "loComplete": true, "hiComplete": true})
    }

    fn lateral(stations: Vec<Vec<Value>>) -> Value {
        let st: Vec<Value> = stations
            .into_iter()
            .enumerate()
            .map(|(i, runs)| json!({"s": 4.0 * i as f64, "center": [100.0, 100.0 + 4.0 * i as f64], "normal": [-1.0, 0.0], "runs": runs}))
            .collect();
        json!({"stations": st})
    }

    /// 前四站是宽 20、峰值 50 的正常胶（参考宽 20、参考峰值 50），第五站换成被测的那几段。
    fn judge_last(last: Vec<Value>, path_ok: bool) -> Vec<StationCall> {
        let mut st = vec![vec![run(-10.0, 10.0, 50.0)]; 4];
        st.push(last);
        judge_stations(&lateral(st), STATIONS, path_ok).unwrap()
    }

    #[test]
    fn station_present_needs_width_center_and_peak_against_references() {
        let cases = [
            (vec![run(-6.5, 6.5, 50.0)], Some((-6.5, 6.5))),
            (vec![run(-5.5, 5.5, 50.0)], None), // 宽 11 < 0.6 × 20，但在胶宽范围内
            (vec![run(-4.0, 16.0, 50.0)], Some((-4.0, 16.0))), // 中心偏 6 = max(6, 0.3 × 20)
            (vec![run(-3.0, 17.0, 50.0)], None), // 中心偏 7 > 6
            (vec![run(-10.0, 10.0, 14.0)], None), // 峰值 < 0.3 × 50
            (vec![run(-10.0, 10.0, 16.0)], Some((-10.0, 10.0))),
            (vec![], None),
        ];
        for (last, bead) in cases {
            assert_eq!(judge_last(last.clone(), true)[4].bead, bead, "{last:?}");
        }
    }

    #[test]
    fn station_takes_nearest_run_within_width_range() {
        // 离胶路更近但宽过上限的一段不算，取范围内 |中心| 最小的
        let calls = judge_last(
            vec![
                run(-20.0, 20.0, 50.0),
                run(-15.0, 3.0, 50.0),
                run(0.0, 14.0, 50.0),
            ],
            true,
        );
        assert_eq!(calls[4].bead, Some((-15.0, 3.0)));
    }

    #[test]
    fn path_not_found_marks_every_station_empty() {
        let calls = judge_last(vec![run(-10.0, 10.0, 50.0)], false);
        assert_eq!(calls.len(), 5);
        assert!(calls.iter().all(|c| c.bead.is_none()));
        assert_eq!(calls[4].center, [100.0, 116.0]);
    }
}
