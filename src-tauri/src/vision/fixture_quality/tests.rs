use std::collections::BTreeSet;

use super::*;

fn spec() -> FixtureSpec {
    FixtureSpec { rect: [40, 40, 32, 32], source: "capture-1-0-v1".into(), search_radius_px: 20.0, angle_range_deg: 0.0, min_score: 0.6,
        max_peak_ratio: 1.0, max_end_shift_mm: None, end_margin_mm: 0.0 }
}

/// 200×200，灰度 60–180 的伪随机纹理（σ 约 35）
fn textured() -> FrameImage {
    let mut seed = 0x2545_F491u32;
    FrameImage::new(200, 200, (0..200 * 200).map(|_| { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); 60 + (seed >> 24) as u8 % 121 }).collect())
}

fn fill(img: &mut FrameImage, rect: [u32; 4], value: u8, fraction: f64) {
    let [x, y, w, h] = rect;
    let rows = (f64::from(h) * fraction).round() as u32;
    for row in y..y + rows { for col in x..x + w { img.pixels[(row * img.width + col) as usize] = value; } }
}

fn pose(accepted: bool, offset: [f32; 2], angle: f32) -> PoseRecord {
    PoseRecord { accepted, reason: (!accepted).then(|| "ambiguous".into()), score: Some(0.95), peak_ratio: Some(0.4), angle: Some(angle),
        offset: Some(offset), end_shift_mm: Some([0.3, -0.2]), excluded_mm: None, end_limit_mm: 6.0 }
}

fn failed_blocks(checks: &[CheckItem]) -> BTreeSet<String> {
    checks.iter().filter(|c| c.blocks()).map(|c| format!("{:?}", c.id)).collect()
}

fn failed_warnings(checks: &[CheckItem]) -> BTreeSet<String> {
    checks.iter().filter(|c| c.level == Level::Warn && !c.passed).map(|c| format!("{:?}", c.id)).collect()
}

#[test]
fn teach_image_failing_any_save_check_makes_the_trial_invalid_and_names_it_but_a_long_lever_only_warns() {
    // 防的回归（定稿 §5“示教”、§10）：纹理、亮像素预筛、外圈出图、压走廊、示教图定位不被采用或自定位不是恒等时仍算有效试测；
    // 或杠杆臂长被当成阻断项
    let path = [[20.0f32, 180.0], [180.0, 180.0]];
    let corridor = |clearance: f64| Clearance { k: 0, view: 1, owner: "拍照点 P1 图 1".into(), clearance, need: 34.0 };
    // （名称，改图，改框，走廊余量，示教图位姿，位置公差 mm，应阻断的项，应告警的项）
    type Case = (&'static str, fn(&mut FrameImage), [u32; 4], f64, Option<PoseRecord>, f64, &'static [&'static str], &'static [&'static str]);
    let ok = Some(pose(true, [0.1, -0.2], 0.0));
    let cases: [Case; 8] = [
        ("纹理不足", |img| fill(img, [40, 40, 32, 32], 120, 1.0), [40, 40, 32, 32], 80.0, ok.clone(), 0.5, &["Texture"], &[]),
        ("亮像素占 25%", |img| fill(img, [40, 40, 32, 32], 230, 0.25), [40, 40, 32, 32], 80.0, ok.clone(), 0.5, &["Bright"], &[]),
        ("外圈出图", |_| {}, [2, 40, 32, 32], 80.0, ok.clone(), 0.5, &["Inside"], &[]),
        ("压走廊", |_| {}, [40, 40, 32, 32], 20.0, ok.clone(), 0.5, &["Corridor"], &[]),
        ("示教图定位没被采用", |_| {}, [40, 40, 32, 32], 80.0, Some(pose(false, [0.0, 0.0], 0.0)), 0.5, &["SelfLocate"], &[]),
        ("示教图没有定位结果", |_| {}, [40, 40, 32, 32], 80.0, None, 0.5, &["SelfLocate"], &[]),
        ("自定位偏开 0.8 px", |_| {}, [40, 40, 32, 32], 80.0, Some(pose(true, [0.8, 0.0], 0.0)), 0.5, &["SelfLocate"], &[]),
        // 中心 (55.5, 55.5) 到 (180, 180) 约 176 px = 17.6 mm；公差 ±0.3 mm 时上限约 13.8 mm
        ("只有杠杆臂长", |_| {}, [40, 40, 32, 32], 80.0, ok.clone(), 0.3, &[], &["Lever"]),
    ];
    for (name, edit, rect, clearance, located, tolerance, blocks, warns) in cases {
        let mut image = textured();
        edit(&mut image);
        let fixture = FixtureSpec { rect, ..spec() };
        let clearances = [corridor(clearance)];
        let geometry = geometry_checks(&TeachInput { fixture: &fixture, image: &image, clearances: &clearances, path: &path, mm_per_px: Some(0.1), tolerance_mm: Some(tolerance) });
        let report = teach_acceptance(geometry, located.as_ref(), fixture.angle_range_deg, fixture.max_peak_ratio);
        let want: BTreeSet<String> = blocks.iter().map(|s| s.to_string()).collect();
        assert_eq!(failed_blocks(&report.checks), want, "{name}");
        assert_eq!(report.passed, blocks.is_empty(), "{name}");
        assert_eq!(failed_warnings(&report.checks), warns.iter().map(|s| s.to_string()).collect(), "{name}");
    }
}

fn context(angle_range_deg: f32) -> ViewContext {
    ViewContext { k: 0, view: 1, shot_id: "P1".into(), fixture: FixtureSpec { angle_range_deg, ..spec() }, tolerance_mm: 0.5, nominal_mm: 0.0,
        lever_mm: 80.0, spacing_mm: 0.5, stations: 41 }
}

/// 12 件都定位成功、±40 锁在原处、±3° 看得出角度且几乎不转、d′ 在 0 附近 ±0.02 mm
fn normal_samples() -> Vec<FixtureSample> {
    (0..12).map(|i| FixtureSample {
        name: format!("S{i:02}"),
        pose: Some(pose(true, [1.0, 0.5], 0.0)),
        deviations_mm: vec![0.02, -0.02, 0.01, -0.01],
        brighter: Some(pose(true, [1.3, 0.5], 0.0)),
        darker: Some(pose(true, [0.8, 0.4], 0.0)),
        rotation: vec![Some(pose(true, [1.0, 0.5], 0.01)); 3],
        error: None,
    }).collect()
}

#[test]
fn validation_blocks_failed_locate_wide_sigma_bright_mislock_and_unobservable_rotation_but_only_warns_otherwise() {
    // 防的回归（定稿 §5“验证”、§10）：有件定位失败、σ 超公差/5、亮度 ±40 错锁、开了转角却看不出角度时还能发布；
    // 或位姿占搜索范围 70% 以上、μ 偏离名义值超公差/3、±40 被拒这些告警项把发布拦住
    type Edit = fn(&mut Vec<FixtureSample>);
    // （名称，转角范围，改样本，应阻断的项，应告警的项）
    let cases: [(&str, f32, Edit, &[&str], &[&str]); 8] = [
        ("全部正常", 0.0, |_| {}, &[], &[]),
        ("一件定位失败", 0.0, |s| s[3].pose = Some(pose(false, [0.0, 0.0], 0.0)), &["LocateAll"], &[]),
        ("σ 超公差/5", 0.0, |s| for x in s.iter_mut() { x.deviations_mm = vec![0.2, -0.2, 0.15, -0.15]; }, &["Sigma"], &[]),
        ("+40 错锁", 0.0, |s| s[5].brighter = Some(pose(true, [6.0, 0.5], 0.0)), &["BrightnessLock"], &[]),
        ("开转角、−40 看不出角度", 3.0, |s| s[2].darker = Some(PoseRecord { reason: Some("angle_unobservable".into()), ..pose(false, [0.0, 0.0], 0.0) }), &["Observable"], &[]),
        ("位姿用到搜索半径 75%", 0.0, |s| { s[0].pose = Some(pose(true, [15.0, 0.5], 0.0)); s[0].brighter = s[0].pose.clone(); s[0].darker = s[0].pose.clone(); }, &[], &["SearchRange"]),
        ("μ 偏名义值超公差/3", 0.0, |s| for x in s.iter_mut() { x.deviations_mm = vec![0.20, 0.21, 0.19, 0.20]; }, &[], &["Nominal"]),
        ("−40 被拒", 0.0, |s| s[1].darker = Some(pose(false, [0.0, 0.0], 0.0)), &[], &["BrightnessReject"]),
    ];
    for (name, angle_range, edit, blocks, warns) in cases {
        let mut samples = normal_samples();
        edit(&mut samples);
        let report = validation_report(&context(angle_range), &samples);
        assert_eq!(failed_blocks(&report.checks), blocks.iter().map(|s| s.to_string()).collect(), "{name}");
        assert_eq!(report.passed, blocks.is_empty(), "{name}");
        assert_eq!(failed_warnings(&report.checks), warns.iter().map(|s| s.to_string()).collect(), "{name}");
    }
}

#[test]
fn end_suggestion_takes_p99_of_both_ends_plus_one_mm_and_plus_one_spacing_and_is_refused_when_too_few_stations_remain() {
    // 防的回归（定稿 §10 决定一）：建议用最大值或单端代替两端合并的 P99、加错余量、没向上取整，
    // 或示教余量会让胶路剩不下 3 站时仍给“采用”
    let mut samples = normal_samples();
    samples.truncate(1);
    // 60 件 × 两端 = 120 个 |Δ|：0.08、0.13 … 6.03 mm，最近秩 P99 = 第 119 个 = 5.98 mm（最大值 6.03）
    samples = (0..60).map(|i| {
        let mut s = samples[0].clone();
        let v = |j: usize| (j as f64 * 0.05 + 0.03) as f32;
        s.pose.as_mut().unwrap().end_shift_mm = Some([v(2 * i + 1), -v(2 * i + 2)]);
        s
    }).collect();
    for (stations, applicable) in [(41, true), (31, false)] {
        let report = validation_report(&ViewContext { stations, ..context(0.0) }, &samples);
        let s = report.suggestion.expect("有定位成功的样本就有建议");
        assert!((s.p99_mm - 5.98).abs() < 1e-5, "{}", s.p99_mm);
        assert_eq!((s.max_end_shift_mm, s.end_margin_mm), (7.0, 6.5), "P99 + 1 mm、P99 + 0.5 mm 站距，向上取到 0.1");
        // 31 站、站距 0.5 mm：长 15 mm，两端各排除 6.5 + 0.5 mm 后剩 1 站
        assert_eq!(s.applicable, applicable, "{stations} 站");
    }
}

#[test]
fn rotation_evidence_meets_the_automatic_conditions_only_when_large_and_observable_on_every_frame_including_brightness() {
    // 防的回归（定稿 §10 决定二条件 1、3）：L·|θ| 的 P95 没超公差/5、或亮度 ±40 的 ±3° 帧里有看不出角度的，仍报可以开转角
    // L = 80 mm、公差/5 = 0.1 mm：0.1° → 0.14 mm（超），0.05° → 0.07 mm（不超）
    type Edit = fn(&mut Vec<FixtureSample>);
    let cases: [(&str, Edit, bool); 3] = [
        ("转得多、全部看得出角度", |s| for x in s.iter_mut() { x.rotation = vec![Some(pose(true, [1.0, 0.5], 0.1)); 3]; }, true),
        ("转得多、一帧 −40 看不出角度", |s| {
            for x in s.iter_mut() { x.rotation = vec![Some(pose(true, [1.0, 0.5], 0.1)); 3]; }
            s[4].rotation[2] = Some(PoseRecord { reason: Some("angle_unobservable".into()), ..pose(false, [0.0, 0.0], 0.0) });
        }, false),
        ("转得少", |s| for x in s.iter_mut() { x.rotation = vec![Some(pose(true, [1.0, 0.5], 0.05)); 3]; }, false),
    ];
    for (name, edit, met) in cases {
        let mut samples = normal_samples();
        edit(&mut samples);
        let report = validation_report(&context(0.0), &samples);
        assert_eq!(report.rotation.auto_conditions, met, "{name}：{:?}", report.rotation);
        // 只找平移的视图转得多时告警（D3：只告警不阻断）
        assert_eq!(failed_warnings(&report.checks).contains("Rotation"), name != "转得少", "{name}");
        assert!(report.passed, "{name}");
    }
}
