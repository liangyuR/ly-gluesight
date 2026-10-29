//! 找零件边的判定（D-SB-48）：lyFlow 1.2 的 `glue.part_edges` 只给每站两侧的全部合格跳变，
//! 这里按 1.1 `glue.edge_distance`（`reference = bead_center`）的口径取边、选侧、剔野、算距离，
//! 输出与 1.1 `edge` 记录逐站同口径的状态与距离（px）。
//!
//! 1.1 从胶边外 3 px 起取剖面、剖面下标 k ≥ 5 才算候选，取第一个合格跳变；跳变位置 t = e + 3 + k + δ（|δ| ≤ 0.5，
//! e 是这一侧胶边沿这一侧往外的距离），所以等价的窗口是 t ∈ [e + 7.5, e + 2.5 + ⌊searchLength⌋ − farRun]。
//! 1.2 的剖面从胶路起按整数 t 采样，与 1.1 的采样相位不同，同一条边的位置有亚像素差，贴着门限的跳变也可能一边有一边没有。
//! 胶不跨胶路、这一侧胶边在胶路另一边（e < −2.5）时，1.1 的窗口有一段在 t < 4.5，`start ≥ 0` 的剖面取不到。

use serde_json::Value;

/// 1.1 剖面起点在胶边外的距离（px）。
const GAP_PX: f64 = 3.0;
/// 1.1 剖面上第一个候选的下标（近侧均值要用前面 4 个采样）。
const FIRST_K: f64 = 5.0;

/// 找边窗口：1.1 `glue.edge_distance` 的 `searchLength` 与 `farRun`（px）。
#[derive(Clone, Copy, Debug)]
pub struct EdgeWindow {
    pub search_px: f64,
    pub far_run_px: f64,
}

impl EdgeWindow {
    fn far(&self) -> f64 {
        self.far_run_px.round().max(1.0)
    }

    /// 胶边在 e 时，1.1 能取到的跳变位置范围（沿这一侧往外，px）。
    fn range(&self, e: f64) -> (f64, f64) {
        (e + GAP_PX + FIRST_K - 0.5, e + GAP_PX - 0.5 + self.search_px.floor() - self.far())
    }

    /// `glue.part_edges`（`start = 0`）的 `searchLength` 至少要这么长，胶边离胶路最远 `max_offset_px`（取卡尺半长）时
    /// 才盖得住 1.1 的整个窗口与窗口末端的远侧。
    pub fn profile_length(&self, max_offset_px: f64) -> f64 {
        max_offset_px.ceil() + GAP_PX + 1.0 + self.search_px.floor()
    }
}

/// 零件边在胶路的哪一侧（沿法向 n 为右）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeSide {
    Right,
    Left,
}

/// 一站的找边状态，与 1.1 `edge.status` 同名。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeStatus {
    Ok,
    /// 找到了边，但离沿 s 的滑动中值太远
    Outlier,
    /// 有胶，选中的那一侧没找到边
    None,
    /// 这一站没胶，没找
    NoBead,
}

/// 一站的结果：`distance_px` 是胶中线到零件边的距离，找到边（ok / outlier）时才有，同 1.1 `distancePx`。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StationEdge {
    pub status: EdgeStatus,
    pub distance_px: Option<f64>,
}

/// 一帧的找边判定：选中的一侧（两侧都没找到为 None）、两侧找到边的站数与逐站结果（与输入的站一一对应）。
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeJudgement {
    pub side: Option<EdgeSide>,
    pub votes_right: usize,
    pub votes_left: usize,
    pub stations: Vec<StationEdge>,
}

impl EdgeJudgement {
    /// 胶中线到零件边距离的均值（px），与现在 `edge_mm` 的汇总同口径：有胶站里 ok 的不到一半时为 None（这一帧胶偏记未测）。
    pub fn mean_distance_px(&self) -> Option<f64> {
        let searched = self.stations.iter().filter(|s| s.status != EdgeStatus::NoBead).count();
        let ok: Vec<f64> = self
            .stations
            .iter()
            .filter(|s| s.status == EdgeStatus::Ok)
            .filter_map(|s| s.distance_px)
            .collect();
        if searched == 0 || ok.len() * 2 < searched {
            return None;
        }
        Some(ok.iter().sum::<f64>() / ok.len() as f64)
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    if v.len() % 2 == 1 {
        v[m]
    } else {
        0.5 * (v[m - 1] + v[m])
    }
}

/// 零件边一侧：找到边的站多的一侧；两侧都没找到为 None。平票取右侧（同 1.1），平票改判未测还没定，改只改这里。
fn pick_side(votes_right: usize, votes_left: usize) -> Option<EdgeSide> {
    match (votes_right, votes_left) {
        (0, 0) => None,
        (r, l) if r >= l => Some(EdgeSide::Right),
        _ => Some(EdgeSide::Left),
    }
}

/// 一侧的全部跳变位置 t（px，升序）。
fn side_hits(station: &Value, key: &str, i: usize) -> Result<Vec<f64>, String> {
    let edges = station[key]["edges"]
        .as_array()
        .ok_or_else(|| format!("找边结果第 {} 站缺少{}侧跳变", i + 1, if key == "right" { "右" } else { "左" }))?;
    edges
        .iter()
        .map(|e| e["t"].as_f64().filter(|t| t.is_finite()).ok_or_else(|| format!("找边结果第 {} 站的跳变位置不是有限数", i + 1)))
        .collect()
}

/// 按 1.1 `glue.edge_distance`（`reference = bead_center`）判一帧的零件边。
///
/// - `edges`：`glue.part_edges` 的 `edges` 记录，与横向卡尺同一条胶路、同样的站距与布站范围，`sides = both`；
/// - `beads`：每站选中的胶两边沿法向的偏移 `(lo, hi)`（px，`lo < hi`），无胶站为 None；
/// - `window`：1.1 的找边窗口。
///
/// 每个有胶站两侧各取窗口里的第一个跳变，按 [`pick_side`] 选零件边一侧；该侧找到边的站算距离
/// `t − 侧号 × (lo + hi) / 2`，沿 s 取 7 站滑动中值，离它超过 4·MAD + 2（MAD 另加 0.5）的记 outlier。
/// 记录与站数不一致、缺一侧、或剖面长度盖不住窗口时返回 Err（测量图配置或算法输出不合契约）。
pub fn judge_edges(edges: &Value, beads: &[Option<(f64, f64)>], window: &EdgeWindow) -> Result<EdgeJudgement, String> {
    let stations = edges["stations"].as_array().ok_or("找边结果缺少逐站结果")?;
    if stations.len() != beads.len() {
        return Err(format!("找边结果有 {} 站，胶宽测量有 {} 站", stations.len(), beads.len()));
    }
    let reach = edges["start"].as_f64().unwrap_or(f64::NAN) + edges["searchLength"].as_f64().unwrap_or(f64::NAN);
    let max_offset = beads.iter().flatten().map(|&(lo, hi)| hi.max(-lo)).fold(0.0, f64::max);
    if !(reach >= window.profile_length(max_offset)) {
        return Err(format!("找边剖面长度不足：需从胶路起至少 {} px", window.profile_length(max_offset)));
    }
    let first = |hits: Vec<f64>, e: f64| {
        let (a, b) = window.range(e);
        hits.into_iter().find(|&t| t >= a).filter(|&t| t <= b)
    };
    let mut right = vec![None; beads.len()];
    let mut left = vec![None; beads.len()];
    for (i, (station, bead)) in stations.iter().zip(beads).enumerate() {
        let Some((lo, hi)) = *bead else { continue };
        right[i] = first(side_hits(station, "right", i)?, hi);
        left[i] = first(side_hits(station, "left", i)?, -lo);
    }
    let votes_right = right.iter().flatten().count();
    let votes_left = left.iter().flatten().count();
    let mut out: Vec<StationEdge> = beads
        .iter()
        .map(|b| StationEdge { status: if b.is_some() { EdgeStatus::None } else { EdgeStatus::NoBead }, distance_px: None })
        .collect();
    let Some(side) = pick_side(votes_right, votes_left) else {
        return Ok(EdgeJudgement { side: None, votes_right, votes_left, stations: out });
    };
    let (chosen, sign) = match side {
        EdgeSide::Right => (&right, 1.0),
        EdgeSide::Left => (&left, -1.0),
    };
    let found: Vec<(usize, f64)> = chosen
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let (lo, hi) = beads[i]?;
            Some((i, (*t)? - sign * 0.5 * (lo + hi)))
        })
        .collect();
    let d: Vec<f64> = found.iter().map(|&(_, d)| d).collect();
    let dev: Vec<f64> = (0..d.len())
        .map(|i| (d[i] - median(d[i.saturating_sub(3)..(i + 4).min(d.len())].to_vec())).abs())
        .collect();
    let mad = median(dev.clone()) + 0.5;
    for (&(i, distance), dev) in found.iter().zip(dev) {
        out[i] = StationEdge {
            status: if dev <= 4.0 * mad + 2.0 { EdgeStatus::Ok } else { EdgeStatus::Outlier },
            distance_px: Some(distance),
        };
    }
    Ok(EdgeJudgement { side: Some(side), votes_right, votes_left, stations: out })
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    const WINDOW: EdgeWindow = EdgeWindow { search_px: 100.0, far_run_px: 12.0 };
    /// 胶两边 (−2, 6)：胶中线 2，右侧胶边 6、左侧胶边 2
    const BEAD: Option<(f64, f64)> = Some((-2.0, 6.0));

    /// `glue.part_edges` 记录：每站右、左两侧的跳变位置，剖面从胶路起够长。
    fn record(stations: &[(&[f64], &[f64])]) -> Value {
        let side = |ts: &[f64]| json!({"length": 200, "edges": ts.iter().map(|t| json!({"t": t})).collect::<Vec<_>>()});
        json!({
            "start": 0.0, "searchLength": 200.0,
            "stations": stations.iter().map(|(r, l)| json!({"right": side(r), "left": side(l)})).collect::<Vec<_>>(),
        })
    }

    fn statuses(j: &EdgeJudgement) -> Vec<(EdgeStatus, Option<f64>)> {
        j.stations.iter().map(|s| (s.status, s.distance_px)).collect()
    }

    #[test]
    fn edges_on_minority_side_are_ignored_and_distance_is_from_bead_center() {
        use EdgeStatus::{None as Miss, Ok};
        let cases: [(&[(&[f64], &[f64])], EdgeSide, Vec<(EdgeStatus, Option<f64>)>); 2] = [
            (&[(&[20.0], &[]), (&[22.0], &[]), (&[], &[15.0])], EdgeSide::Right, vec![(Ok, Some(18.0)), (Ok, Some(20.0)), (Miss, None)]),
            (&[(&[20.0], &[]), (&[], &[15.0]), (&[], &[16.0])], EdgeSide::Left, vec![(Miss, None), (Ok, Some(17.0)), (Ok, Some(18.0))]),
        ];
        for (stations, side, expected) in cases {
            let j = judge_edges(&record(stations), &[BEAD; 3], &WINDOW).unwrap();
            assert_eq!((j.side, statuses(&j)), (Some(side), expected), "{side:?}");
        }
    }

    #[test]
    fn only_first_edge_inside_window_beyond_bead_edge_counts() {
        // 右侧胶边 6：窗口 [6 + 7.5, 6 + 2.5 + 100 − 12] = [13.5, 96.5]
        let cases: [(&[f64], Option<f64>); 5] = [
            (&[10.0, 20.0], Some(20.0)),
            (&[13.0, 30.0], Some(30.0)),
            (&[13.6, 15.0], Some(13.6)),
            (&[96.0], Some(96.0)),
            (&[97.0], None),
        ];
        for (hits, expected) in cases {
            let j = judge_edges(&record(&[(hits, &[])]), &[BEAD], &WINDOW).unwrap();
            assert_eq!(j.stations[0].distance_px, expected.map(|t| t - 2.0), "{hits:?}");
        }
    }

    #[test]
    fn station_far_from_running_median_is_outlier_and_left_out_of_mean() {
        let hits = [20.0, 21.0, 20.0, 40.0, 21.0, 20.0, 21.0];
        let stations: Vec<(&[f64], &[f64])> = hits.iter().map(|t| (std::slice::from_ref(t), &[][..])).collect();
        let j = judge_edges(&record(&stations), &[BEAD; 7], &WINDOW).unwrap();
        let outliers: Vec<usize> = (0..7).filter(|&i| j.stations[i].status == EdgeStatus::Outlier).collect();
        assert_eq!((outliers, j.mean_distance_px()), (vec![3], Some(18.5)));
    }

    #[test]
    fn frame_mean_needs_ok_on_at_least_half_of_bead_stations() {
        let beads = [BEAD, BEAD, BEAD, BEAD, None, None];
        let cases: [(&[(&[f64], &[f64])], Option<f64>); 2] = [
            (&[(&[20.0], &[]), (&[22.0], &[]), (&[], &[]), (&[], &[]), (&[], &[]), (&[], &[])], Some(19.0)),
            (&[(&[20.0], &[]), (&[], &[]), (&[], &[]), (&[], &[]), (&[30.0], &[]), (&[30.0], &[])], None),
        ];
        for (stations, expected) in cases {
            let j = judge_edges(&record(stations), &beads, &WINDOW).unwrap();
            assert_eq!(j.mean_distance_px(), expected);
        }
    }

    #[test]
    fn record_not_covering_the_window_or_stations_is_rejected() {
        let mut short = record(&[(&[20.0], &[])]);
        short["searchLength"] = json!(100.0);
        let ok = record(&[(&[20.0], &[])]);
        for (edges, beads) in [(&short, &[BEAD][..]), (&ok, &[BEAD, BEAD][..])] {
            assert!(judge_edges(edges, beads, &WINDOW).is_err());
        }
    }
}
