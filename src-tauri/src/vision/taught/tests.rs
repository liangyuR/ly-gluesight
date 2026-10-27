use super::*;
use crate::recipe::DetectParams;

const FRAME: [u32; 2] = [400, 300];

/// 段配方：三台相机的走廊都沿 y = 80 从 x = 40 到 200；0.25 mm/px、站距 1 mm（4 px）→ 41 站，
/// 搜索半宽 32 px、胶宽上限 24 px → 中心偏移容差 20 px。
fn burst_recipe() -> Recipe {
    let mut doc = crate::recipe::samples_burst();
    doc.spacing = 1.0;
    doc.detect = DetectParams { search_mm: 8.0, polarity: Polarity::Dark, width_range: [2.0, 6.0] };
    for inspect in &mut doc.inspect {
        inspect.segments = vec![crate::recipe::SegmentCorridor {
            segment: 1,
            corridor: vec![[40.0, 80.0], [200.0, 80.0]],
        }];
        inspect.mm_per_px = Some(0.25);
    }
    doc.build().unwrap()
}

fn corridor(camera: usize) -> CorridorGraph {
    let recipe = burst_recipe();
    plan_corridor(&recipe.inspect[camera], 1, &recipe, FRAME).unwrap()
}

/// 走廊逐站的算法输出（法向 (0, 1)）：beads[i] = Some((胶中心偏移 px, 胶宽 px)) 是有胶站。
fn corridor_output(beads: &[Option<(f64, f64)>]) -> RunResult {
    let n = beads.len();
    let field = |f: fn(f64, f64) -> f64| beads.iter().map(|b| b.map_or(Value::Null, |(x, w)| json!(f(x, w)))).collect::<Vec<_>>();
    let present = beads.iter().flatten().count();
    RunResult { summary: json!({"status": "ok"}), outputs: json!({
        "stations": {"value": {"data": {"count": n, "unit": "px", "s": (0..n).map(|i| i as f64 * 4.0).collect::<Vec<_>>(),
            "center": (0..n).map(|i| [40.0 + 4.0 * i as f64, 80.0]).collect::<Vec<_>>(), "normal": vec![[0.0, 1.0]; n],
            "present": beads.iter().map(Option::is_some).collect::<Vec<_>>(),
            "lo": field(|x, w| x - w / 2.0), "hi": field(|x, w| x + w / 2.0), "widthPx": field(|_, w| w)}}},
        "beadInfo": {"value": {"data": {"unit": "px", "lineSource": "taught", "pathOk": true, "stations": n, "present": present,
            "coverage": present as f64 / n as f64}}}
    }) }
}

/// 41 站：absent 里的站无胶，其余胶宽 16 px（4 mm）、胶中心偏 +2 px；edge 改给定站的胶中心偏移。
fn beads(absent: &[std::ops::RangeInclusive<usize>], edge: &[(usize, f64)]) -> Vec<Option<(f64, f64)>> {
    (0..41).map(|i| {
        if absent.iter().any(|r| r.contains(&i)) { return None; }
        Some((edge.iter().find(|(j, _)| *j == i).map_or(2.0, |(_, x)| *x), 16.0))
    }).collect()
}

fn measure(c: &CorridorGraph, size: [u32; 2], output: RunResult) -> Result<FrameMeasurement, String> {
    let image = FrameImage::new(size[0], size[1], vec![0; (size[0] * size[1]) as usize]);
    measure_frame_with(c, &image, 7, |_, _| Ok(output))
}

#[test]
fn engine_station_output_reduces_to_frame_width_gap_presence_and_x_mm() {
    // 防的回归（规格 §4 每帧独立指标）：胶宽漏乘当量、最长断胶取成断胶站总数、有胶比例分母错、x 的正向或均值算错、
    // 帧指标挂错相机下标
    // 站 5–6、20–24 无胶（最长 5 站 = 5 mm）；站 10 胶宽 20 px、站 30 胶宽 14 px 且胶中心 −6 px，其余 16 px、+2 px
    let mut b = beads(&[5..=6, 20..=24], &[]);
    b[10] = Some((2.0, 20.0));
    b[30] = Some((-6.0, 14.0));
    let m = measure(&corridor(1), FRAME, corridor_output(&b)).unwrap();
    let mut widths = vec![Some(4.0f32); 41];
    for i in [5, 6, 20, 21, 22, 23, 24] { widths[i] = None; }
    widths[10] = Some(5.0);
    widths[30] = Some(3.5);
    assert_eq!(
        m.metrics,
        FrameMetrics {
            camera: 1,
            seq: 7,
            state: FrameState::Measured,
            error: None,
            width_min: Some(3.5),
            width_max: Some(5.0),
            width_mean: Some((32.0 * 4.0 + 5.0 + 3.5) / 34.0),
            widths,
            spacing_mm: 1.0,
            gap_max_mm: 5.0,
            present_ratio: 34.0 / 41.0,
            corridor: Some(1),
            x_mm: Some((33.0 * 0.5 - 1.5) / 34.0),
            heading_deg: None,
            heading_fallback: false,
            edge_mm: None,
        }
    );
    assert_eq!(m.edges[0], Some([[40.0, 74.0], [40.0, 90.0]]));
    assert!(m.edges[5].is_none());
}

#[test]
fn malformed_engine_output_fails_only_that_frame() {
    // 防的回归（规格 §0 一帧失败只影响本帧）：输出不合契约（站数、弧长、中线、胶边与胶宽、汇总）或图像尺寸不对时
    // panic、拿错位的数据算指标，或把走廊测量图弄坏、后面正常的帧也测不了
    let c = corridor(0);
    let good = || corridor_output(&beads(&[], &[]));
    for (pointer, value) in [
        ("/summary/status", json!("failed")),
        ("/outputs/stations/value/data/count", json!(40)),
        ("/outputs/stations/value/data/lo", json!([])),
        ("/outputs/stations/value/data/s/3", json!(0.0)),
        ("/outputs/stations/value/data/center/3", json!([0.0, 0.0])),
        ("/outputs/stations/value/data/widthPx/3", json!(1.0)),
        ("/outputs/stations/value/data/present/3", json!(false)),
        ("/outputs/beadInfo/value/data/present", json!(0)),
    ] {
        let output = good();
        let mut raw = json!({"summary": output.summary, "outputs": output.outputs});
        *raw.pointer_mut(pointer).unwrap() = value;
        let bad = RunResult { summary: raw["summary"].clone(), outputs: raw["outputs"].clone() };
        assert!(measure(&c, FRAME, bad).is_err(), "accepted {pointer}");
    }
    assert!(measure(&c, [FRAME[0], FRAME[1] + 1], good()).is_err(), "图像尺寸与示教帧不同");
    assert_eq!(measure(&c, FRAME, good()).unwrap().metrics.state, FrameState::Measured);
}

#[test]
fn bead_leaving_through_a_corridor_end_is_out_of_corridor_but_breaks_and_centred_stops_stay_gaps() {
    // 防的回归（默认做法 4）：弯道帧胶条从走廊侧边走出时整帧按断胶判 NG；或反过来把真断胶、停胶、急转处居中的缺胶
    // 当成“走出走廊”放过（只记检测不完整）。中心偏移容差 20 px，判据是紧挨端部缺胶的胶中心偏离 ≥ 10 px
    for (name, absent, edge, out) in [
        ("终点一侧侧向走出", vec![30..=40], vec![(29, 12.0)], true),
        ("起点一侧侧向走出", vec![0..=8], vec![(9, -11.0)], true),
        ("两端都侧向走出", vec![0..=3, 36..=40], vec![(4, 10.0), (35, 14.0)], true),
        ("端部缺胶但胶中心居中（停胶、急转）", vec![30..=40], vec![(29, 2.0)], false),
        ("一端走出、另一端缺胶处居中", vec![0..=3, 36..=40], vec![(4, 2.0), (35, 14.0)], false),
        ("中段断胶，端部走出", vec![15..=17, 35..=40], vec![(34, 12.0)], false),
        ("整帧没有胶", vec![0..=40], vec![], false),
    ] {
        let m = measure(&corridor(2), FRAME, corridor_output(&beads(&absent, &edge))).unwrap().metrics;
        let want = if out { FrameState::OutOfCorridor } else { FrameState::Measured };
        assert_eq!(m.state, want, "{name}");
        assert_eq!(m.error.is_some(), out, "{name}");
        assert!(m.gap_max_mm > 0.0, "{name}：缺胶照常计入");
    }
}

#[test]
fn corridor_leaving_the_frame_or_camera_outside_the_plan_is_refused() {
    // 防的回归：走廊端点落在图外时 lyFlow 量不到两端（像素中心坐标最大到宽 − 1）；拿不是计划相机的走廊建图，
    // 帧指标的相机下标就会挂错
    let recipe = burst_recipe();
    let inspect = &recipe.inspect[0];
    assert!(
        plan_corridor(inspect, 1, &recipe, [201, 81]).is_ok(),
        "走廊端点恰在最后一列、最后一行"
    );
    assert!(
        plan_corridor(inspect, 1, &recipe, [200, 300]).is_err(),
        "走廊终点 x = 200 在宽 200 的图外"
    );
    assert!(
        plan_corridor(inspect, 1, &recipe, [400, 80]).is_err(),
        "走廊 y = 80 在高 80 的图外"
    );
    let stranger = CameraInspect {
        camera: "cam9".into(),
        ..inspect.clone()
    };
    assert!(plan_corridor(&stranger, 1, &recipe, FRAME).is_err());
}

#[test]
fn corridor_under_three_stations_is_refused_with_its_count_and_what_would_fit() {
    // 防的回归：放过不足三站的走廊（lyFlow 1.1 少于 3 站时不报错、把有胶的帧都测成无胶，判断胶）；
    // 刚好两倍站距的走廊被误拦；报错没有实际站数、可用站距与需要的走廊长度
    let mut recipe = burst_recipe();
    recipe.spacing = 20.0;
    assert!(plan_corridor(&recipe.inspect[0], 1, &recipe, FRAME).is_ok());
    recipe.spacing = 20.5;
    let error = plan_corridor(&recipe.inspect[0], 1, &recipe, FRAME).unwrap_err();
    assert!(
        error.contains("只有 2 站")
            && error.contains("不大于 20 mm")
            && error.contains("不短于 41 mm"),
        "{error}"
    );
}
