//! 特征定位接真引擎的集成测试（定稿 docs/discussions/fixture-position/final.md §7）：合成工件上跑 lyFlow 的
//! glue.locate + bead_width，经 measure_views 与 judge 走生产同一条路。单测用的是手写的 pose 记录，
//! 这里防的是与真实算子的约定对不上：位姿方向与 teachCenter、带外圈模板的裁剪对位、法向正向、失败时的输出形状。
//! 默认忽略；需要 LYFLOW_CORE_DLL 指向带 lyFlow#32 glue.locate 的 lyflow_core.dll，命令见 docs/testing/TESTING.md。

use std::sync::OnceLock;

use super::*;
use crate::judge::{judge, Judgement, PointState, Verdict, fault};
use crate::measure::point_state;
use crate::recipe::{DetectParams, JudgeParams, ShotLimits};
use crate::vision::eval::synth::{Perturb, Rng};

const W: u32 = 400;
const H: u32 = 300;
const MM_PER_PX: f32 = 0.1;
/// 定位框：孔与亮角所在的一块，离胶路走廊（y = 200 ± 30 px）足够远
const RECT: [u32; 4] = [80, 40, 64, 64];
/// 胶路沿 x 从 60 到 360（站距 10 px = 1 mm，31 站），胶条 16 px 宽、中心 y = 199.5
const PATH: [[f32; 2]; 2] = [[60.0, 200.0], [360.0, 200.0]];
const BEAD_Y: u32 = 192;
const BEAD_H: u32 = 16;

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let path = std::env::var("LYFLOW_CORE_DLL").expect("需要 LYFLOW_CORE_DLL 指向 lyflow_core.dll");
        let engine = Engine::load(Path::new(&path)).unwrap();
        engine.locate.clone().expect("这个核心库的 glue.locate 不支持带外圈模板（需要 lyFlow#32 之后的构建）");
        engine
    })
}

/// 合成工件：灰 120 ± 3 噪声底；定位框里两个暗孔、一个亮 L 角；暗胶条沿示教线，横向偏 bead_dy px。
fn scene(bead_dy: u32) -> FrameImage {
    let mut rng = Rng(0x5EED);
    let mut px: Vec<u8> = (0..W * H).map(|_| (120.0 + 3.0 * rng.gauss()).round() as u8).collect();
    let mut fill = |inside: &dyn Fn(f64, f64) -> bool, v: u8| {
        for y in 0..H {
            for x in 0..W {
                if inside(x as f64, y as f64) { px[(y * W + x) as usize] = v; }
            }
        }
    };
    let [rx, ry, _, _] = RECT.map(f64::from);
    fill(&|x, y| (rx + 4.0..rx + 60.0).contains(&x) && (ry + 4.0..ry + 12.0).contains(&y), 210);
    fill(&|x, y| (rx + 4.0..rx + 12.0).contains(&x) && (ry + 4.0..ry + 56.0).contains(&y), 210);
    fill(&|x, y| (x - rx - 30.0).hypot(y - ry - 24.0) <= 7.0, 40);
    fill(&|x, y| (x - rx - 46.0).hypot(y - ry - 46.0) <= 9.0, 40);
    let top = (BEAD_Y + bead_dy) as f64;
    fill(&|x, y| (60.0..=360.0).contains(&x) && (top..top + BEAD_H as f64).contains(&y), 40);
    FrameImage::new(W, H, px)
}

fn copy(image: &FrameImage) -> FrameImage {
    FrameImage::new(image.width, image.height, image.pixels.clone())
}

/// 单拍照点单视图的配方：位置公差 ±0.5 mm，允许断胶 1.5 mm（比测试里两端露出的空走廊短），只找平移。
fn recipe() -> Recipe {
    let mut doc = crate::recipe::samples().remove(0);
    doc.shots.truncate(1);
    let shot = &mut doc.shots[0];
    shot.path = PATH.to_vec();
    shot.mm_per_px = Some(MM_PER_PX);
    shot.fixture = Some(FixtureSpec { rect: RECT, search_radius_px: 40.0, angle_range_deg: 0.0, min_score: 0.6, max_peak_ratio: 1.0, max_end_shift_mm: None });
    doc.spacing = 1.0;
    doc.detect = DetectParams { search_mm: 3.0, polarity: Polarity::Dark, width_range: [0.8, 2.4] };
    doc.limits = ShotLimits {
        position: Some(JudgeParams { nominal: 0.0, tol_upper: 0.5, tol_lower: 0.5, abs_min: -3.0, abs_max: 3.0, max_excursion_len: 3.0 }),
        width: Some(JudgeParams { nominal: 1.6, tol_upper: 0.5, tol_lower: 0.5, abs_min: 0.5, abs_max: 3.0, max_excursion_len: 3.0 }),
        max_gap_len: 1.5,
        min_present: 0.8,
        gauge_ref: None,
    };
    doc.build().unwrap()
}

/// 示教原图裁模板写到临时目录（绝对路径），像工作台那样作为 base_dir 交给 measure_views；返回 (测量, 判定)。
fn run(recipe: &Recipe, teach: &FrameImage, image: FrameImage, tag: &str) -> (MultiViewMeasurement, Judgement) {
    // 每次调用一个目录：并行的测试各自写、各自删，不会删掉别人还没读的模板
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("gs-locate-engine-{tag}-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(fixture::TEMPLATE_FILE), fixture::template_pgm(teach, RECT).unwrap()).unwrap();
    let view = recipe.shots[0].view;
    let bases = BTreeMap::from([(view, dir.to_str().unwrap().to_string())]);
    let multi = measure_views(engine(), recipe, 0, &[(view, Arc::new(image))], tag, &bases);
    let _ = std::fs::remove_dir_all(&dir);
    let multi = multi.unwrap();
    assert!(multi.views.iter().all(|v| v.error.is_none()), "{:?}", multi.views.iter().map(|v| &v.error).collect::<Vec<_>>());
    let mut table = vec![PointState::Pending; recipe.point_count()];
    let r = &multi.reading;
    for (i, &j) in r.idx.iter().enumerate() { table[j as usize] = point_state(r.st[i], r.d[i], r.w[i]); }
    let judgement = judge(recipe, &table);
    (multi, judgement)
}

/// 胶边方向即算法法向 n（edges = [c + n·lo, c + n·hi]）：返回 n·(0, 1) 的符号，胶条沿 +y 平移时 d 按它变号。
fn normal_y_sign(m: &ShotMeasurement) -> f32 {
    let dy: f32 = m.edges.iter().flatten().map(|e| e[1][1] - e[0][1]).sum();
    assert!(dy.abs() > 1.0, "测不出法向");
    dy.signum()
}

#[test]
#[ignore = "需要 LYFLOW_CORE_DLL（带 lyFlow#32 glue.locate），见 docs/testing/TESTING.md"]
fn whole_part_moved_within_search_radius_keeps_offsets_and_excludes_ends_without_false_gap() {
    // 防的回归：位姿方向、teachCenter 或模板外圈对位与真实 glue.locate 不一致时，整件平移会变成胶偏（误判 NG）；
    // 切向搬移后静态走廊一端露出的空段（2–3 mm，超过允许断胶 1.5 mm）没按超出检测范围排除，误判 NG_GAP 或 ERR
    let recipe = recipe();
    let teach = scene(0);
    let (base, judged) = run(&recipe, &teach, copy(&teach), "teach");
    assert_eq!(judged.verdict, Verdict::Ok, "{}", judged.reason);
    let d0 = &base.reading.d;
    // （整图平移 px，切向搬移 mm，每端排除站数 = ⌊|Δ| + 1 站距⌋ + 1）
    for (dx, dy, delta, ends) in [(12.0, 7.0, 1.2, 3), (-25.0, 0.0, -2.5, 4)] {
        let moved = Perturb::Affine { dx, dy, deg: 0.0, scale: 1.0 }.apply(&teach, 0);
        let (multi, judged) = run(&recipe, &teach, moved, "moved");
        let m = &multi.views[0].reading.as_ref().unwrap();
        let pose = m.pose.as_ref().unwrap();
        assert!(pose.accepted, "({dx}, {dy})：{pose:?}");
        let offset = pose.offset.unwrap();
        assert!((offset[0] as f64 - dx).abs() < 0.05 && (offset[1] as f64 - dy).abs() < 0.05, "({dx}, {dy})：{offset:?}");
        assert!(pose.end_shift_mm.unwrap().iter().all(|s| (*s as f64 - delta).abs() < 0.01), "({dx}, {dy})：{pose:?}");
        let n = m.st.len();
        for i in 0..n {
            if i < ends || n - 1 - i < ends {
                assert_eq!(m.st[i], ST_OUT_OF_RANGE, "({dx}, {dy}) 站 {i}");
            } else {
                assert_eq!(m.st[i], ST_OK, "({dx}, {dy}) 站 {i}");
                assert!((m.d[i] - d0[i]).abs() < 0.02, "({dx}, {dy}) 站 {i}：d′ {} 示教帧 {}", m.d[i], d0[i]);
            }
        }
        assert!(multi.reference_failed().is_empty());
        assert_eq!(judged.verdict, Verdict::Ok, "({dx}, {dy})：{}", judged.reason);
        assert!(judged.gaps.is_empty() && judged.fault_code == 0, "({dx}, {dy})：{}", judged.reason);
    }
}

#[test]
#[ignore = "需要 LYFLOW_CORE_DLL（带 lyFlow#32 glue.locate），见 docs/testing/TESTING.md"]
fn bead_moved_alone_changes_offset_by_the_shift_along_the_normal_and_judges_ng_position() {
    // 防的回归：定位把只有胶动的帧也当成整件移动吸收掉（模板裁错、位姿取错），或 d 的正向与算法法向相反
    let recipe = recipe();
    let teach = scene(0);
    let (base, _) = run(&recipe, &teach, copy(&teach), "teach");
    let (multi, judged) = run(&recipe, &teach, scene(8), "bead");
    let m = &multi.views[0].reading.as_ref().unwrap();
    assert!(m.pose.as_ref().is_some_and(|p| p.accepted && p.offset.unwrap().iter().all(|v| v.abs() < 0.05)), "{:?}", m.pose);
    // 胶条沿 +y 移 8 px = 0.8 mm，超出 ±0.5 mm 公差
    let want = 0.8 * normal_y_sign(m);
    let judged_stations: Vec<usize> = (0..m.st.len()).filter(|&i| m.st[i] == ST_OK).collect();
    assert_eq!(judged_stations.len(), m.st.len() - 4, "恒等位姿下每端只排除 1 个站距");
    for i in judged_stations {
        assert!((m.d[i] - base.reading.d[i] - want).abs() < 0.02, "站 {i}：d {} 示教帧 {}", m.d[i], base.reading.d[i]);
    }
    assert_eq!(judged.verdict, Verdict::NgPosition, "{}", judged.reason);
}

#[test]
#[ignore = "需要 LYFLOW_CORE_DLL（带 lyFlow#32 glue.locate），见 docs/testing/TESTING.md"]
fn destroyed_template_region_keeps_widths_and_judges_err_92() {
    // 防的回归：真实 glue.locate 失败时的输出（null 坐标、reason）让测量整帧报错或判测不了（93），
    // 或失败后丢了胶宽、仍拿静态 d 判位置
    let recipe = recipe();
    let teach = scene(0);
    let (base, _) = run(&recipe, &teach, copy(&teach), "teach");
    let mut broken = copy(&teach);
    let mut rng = Rng(0xBAD);
    let [x, y, w, h] = RECT;
    for yy in y - 8..y + h + 8 {
        for xx in x - 8..x + w + 8 { broken.pixels[(yy * W + xx) as usize] = (120.0 + 3.0 * rng.gauss()).round() as u8; }
    }
    let (multi, judged) = run(&recipe, &teach, broken, "broken");
    let m = &multi.views[0].reading.as_ref().unwrap();
    let pose = m.pose.as_ref().unwrap();
    assert!(!pose.accepted && pose.reason.is_some(), "{pose:?}");
    assert_eq!(multi.reference_failed(), [ReferenceFailure { view: recipe.shots[0].view, reason: REFERENCE_LOCATE_FAILED.into() }]);
    for i in 0..m.st.len() {
        assert_eq!(m.st[i], ST_NO_REF, "站 {i}");
        assert!((m.w[i] - base.reading.w[i]).abs() < 0.02, "站 {i}：胶宽 {} 示教帧 {}", m.w[i], base.reading.w[i]);
    }
    assert_eq!((judged.verdict, judged.fault_code), (Verdict::ErrInspect, fault::LOCATE_FAILED), "{}", judged.reason);
}
