use serde_json::{json, Value};

use super::*;

/// 容差 30 px、胶宽 4–30 px，其余取 1.1 模板的默认值。
fn params() -> TaughtParams {
    TaughtParams {
        tolerance: 30.0,
        sharp_min: 0.4,
        width_range: [4.0, 30.0],
        present_ratio: 0.5,
        center_ratio: 0.3,
        contrast_ratio: 0.3,
    }
}

/// 一个暗段：峰值 60、局部亮度 120、陡度 0.8、两边完整。
fn run(mid: f64, width: f64) -> Value {
    run_with(mid - width / 2.0, mid + width / 2.0, 60.0, 120.0, 0.8)
}

fn run_with(lo: f64, hi: f64, peak: f64, level: f64, sharpness: f64) -> Value {
    json!({"lo": lo, "hi": hi, "width": hi - lo, "mid": (lo + hi) / 2.0, "peak": peak, "parts": 1,
        "loComplete": true, "hiComplete": true, "edgeLo": 10.0, "edgeHi": 10.0, "depth": 12.0,
        "sharpness": sharpness, "level": level})
}

/// 站距 step、卡尺半长 60 的 glue.LateralRuns 记录：stations[i] 是第 i 站的暗段。
fn record(step: f64, stations: Vec<Vec<Value>>) -> LateralRuns {
    let n = stations.len();
    let stations: Vec<Value> = stations
        .into_iter()
        .enumerate()
        .map(|(i, runs)| json!({"s": i as f64 * step, "center": [0.0, 0.0], "normal": [0.0, 1.0], "validFraction": 1.0, "runs": runs}))
        .collect();
    serde_json::from_value(
        json!({"count": n, "stationStep": step, "sRange": [0.0, (n - 1) as f64 * step],
        "searchHalf": 60.0, "split": "half_max", "stations": stations}),
    )
    .unwrap()
}

fn kind(j: &TaughtJudgement) -> Option<&'static str> {
    j.no_bead.as_ref().map(|r| match r {
        NoBead::NoCandidate { .. } => "no_candidate",
        NoBead::TooFewOnTrack { .. } => "too_few",
        NoBead::Blunt { .. } => "blunt",
    })
}

#[test]
fn no_bead_along_taught_line_rejects_whole_segment() {
    // 防的回归（T4、D15）：容差内没有暗段、落在轨迹上的站不到 3 站、轨迹陡度 P25 < sharpMin（压痕、阴影）时
    // 仍逐站判出零星有胶，或陡度够的满胶段被整段判无胶
    let cases: [(&str, Vec<Vec<Value>>, Option<&str>); 4] = [
        (
            "暗段都在容差外",
            vec![vec![run(45.0, 10.0)]; 20],
            Some("no_candidate"),
        ),
        (
            "只有两站",
            (0..20)
                .map(|i| if i < 2 { vec![run(0.0, 10.0)] } else { vec![] })
                .collect(),
            Some("too_few"),
        ),
        (
            "边缘太缓",
            vec![vec![run_with(-5.0, 5.0, 60.0, 120.0, 0.2)]; 20],
            Some("blunt"),
        ),
        ("陡度够", vec![vec![run(0.0, 10.0)]; 20], None),
    ];
    for (name, stations, expected) in cases {
        let j = judge_taught(&record(10.0, stations), None, &params()).unwrap();
        assert_eq!(kind(&j), expected, "{name}");
        assert!(
            j.beads.iter().all(|b| b.is_some() == expected.is_none()),
            "{name}"
        );
    }
}

#[test]
fn gap_longer_than_window_interpolates_expected_center() {
    // 防的回归（T3）：断口里的期望中心不是两头的胶连起来的那条线（停在投票常数或一侧），
    // 断口附近的暗段按错的中心判
    let stations: Vec<Vec<Value>> = (0..15)
        .map(|i| match i {
            0..=3 => vec![run(0.0, 10.0)],
            11.. => vec![run(4.0, 10.0)],
            _ => vec![],
        })
        .collect();
    let j = judge_taught(&record(20.0, stations), None, &params()).unwrap();
    assert_eq!(
        j.expect,
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0]
    );
    assert!(j.beads[4..=10].iter().all(Option::is_none));
    assert_eq!(j.beads[11], Some((-1.0, 9.0)));
}

#[test]
fn same_width_sub_run_off_track_is_recovered_from_split_record() {
    // 防的回归（1.1 recovered）：紧贴胶边的淡肩把胶并宽、中心被拽出轨迹时，没有从合并前的子段里补回同宽的胶，
    // 这一站被判成断胶
    let normal = || (0..12).map(|_| vec![run(0.0, 10.0)]).collect::<Vec<_>>();
    let mut merged = normal();
    merged[5] = vec![run_with(-5.0, 20.0, 60.0, 120.0, 0.8)];
    let mut sub = normal();
    sub[5] = vec![run(0.0, 10.0), run(14.0, 12.0)];
    let (merged, sub) = (record(10.0, merged), record(10.0, sub));
    for (split, expected) in [(Some(&sub), Some((-5.0, 5.0))), (None, None)] {
        let j = judge_taught(&merged, split, &params()).unwrap();
        assert_eq!(j.beads[5], expected);
        assert!(j
            .beads
            .iter()
            .enumerate()
            .all(|(i, b)| i == 5 || b.is_some()));
    }
}

#[test]
fn shifted_section_is_followed_only_when_ramps_link_it() {
    // 防的回归（1.1 followShifts）：胶整段横移、过渡处有斜胶时没跟过去（整段报断胶）；
    // 或两头接不上（断口旁的孔、槽阴影）也跟过去，把阴影当成胶
    for ramp in [true, false] {
        let stations: Vec<Vec<Value>> = (0..30)
            .map(|i| match i {
                12 | 20 if ramp => vec![run(10.0, 20.0)],
                12 | 20 => vec![],
                13..=19 => vec![run(20.0, 10.0)],
                _ => vec![run(0.0, 10.0)],
            })
            .collect();
        let j = judge_taught(&record(10.0, stations), None, &params()).unwrap();
        let shifted = if ramp { Some((15.0, 25.0)) } else { None };
        assert!(
            j.beads[13..=19].iter().all(|b| *b == shifted),
            "ramp = {ramp}"
        );
        assert!(j.beads[12].is_none() && j.beads[20].is_none());
        assert!(j.beads[..12]
            .iter()
            .chain(&j.beads[21..])
            .all(Option::is_some));
    }
}

#[test]
fn contrast_compares_peak_over_local_level() {
    // 防的回归（D16 示教胶路）：对比度直接比峰值时，照明暗处（局部亮度低）的整段胶被当成残影；
    // 同一亮度背景上淡得多的淡带却该判无胶
    for (level, present) in [(30.0, true), (120.0, false)] {
        let mut stations: Vec<Vec<Value>> = (0..10).map(|_| vec![run(0.0, 10.0)]).collect();
        stations[4] = vec![run_with(-5.0, 5.0, 15.0, level, 0.8)];
        let j = judge_taught(&record(10.0, stations), None, &params()).unwrap();
        assert_eq!(j.beads[4].is_some(), present, "level = {level}");
    }
}
