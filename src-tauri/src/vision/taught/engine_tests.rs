//! 特征定位接真引擎的集成测试（定稿 docs/discussions/fixture-position/final.md §6 G3、§7）：合成工件经
//! release::publish → release::load → Prepared::load（含预热的定位验收）→ measure_views 走生产同一条路，
//! 在 lyFlow 的 glue.locate + bead_width 上测量，再交给 judge。单测用的是手写的 pose 记录，这里防的是与真实算子
//! 和发布包接线对不上：位姿方向与 teachCenter、带外圈模板的裁剪对位、法向正向、失败时的输出形状、生产 base_dir。
//! 默认忽略；需要 LYFLOW_CORE_DLL 指向带 lyFlow#32 glue.locate 的 lyflow_core.dll，命令见 docs/testing/TESTING.md。

use std::sync::OnceLock;

use super::*;
use crate::judge::{judge, Judgement, PointState, Verdict, fault};
use crate::measure::point_state;
use crate::production::Prepared;
use crate::recipe::{DetectParams, JudgeParams, RecipeDoc, ShotLimits};
use crate::release::{PublishInput, ResourceSource, ShotInput, ValidationSite, Versions};
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
/// 示教图编号 = 定位框的模板来源
const TEACH_ID: &str = "capture-engine-0-v1";

fn engine() -> Arc<Engine> {
    static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let path = std::env::var("LYFLOW_CORE_DLL").expect("需要 LYFLOW_CORE_DLL 指向 lyflow_core.dll");
        let engine = Engine::load(Path::new(&path)).unwrap();
        engine.locate.clone().expect("这个核心库的 glue.locate 不支持带外圈模板（需要 lyFlow#32 之后的构建）");
        Arc::new(engine)
    }).clone()
}

/// 合成工件：灰 120 ± 3 噪声底；定位框 rect 里两个暗孔、一个亮 L 角；暗胶条沿示教线，横向偏 bead_dy px。
fn scene(rect: [u32; 4], bead_dy: i32) -> FrameImage {
    let mut rng = Rng(0x5EED);
    let mut px: Vec<u8> = (0..W * H).map(|_| (120.0 + 3.0 * rng.gauss()).round() as u8).collect();
    let mut fill = |inside: &dyn Fn(f64, f64) -> bool, v: u8| {
        for y in 0..H {
            for x in 0..W {
                if inside(x as f64, y as f64) { px[(y * W + x) as usize] = v; }
            }
        }
    };
    let [rx, ry, _, _] = rect.map(f64::from);
    fill(&|x, y| (rx + 4.0..rx + 60.0).contains(&x) && (ry + 4.0..ry + 12.0).contains(&y), 210);
    fill(&|x, y| (rx + 4.0..rx + 12.0).contains(&x) && (ry + 4.0..ry + 56.0).contains(&y), 210);
    fill(&|x, y| (x - rx - 30.0).hypot(y - ry - 24.0) <= 7.0, 40);
    fill(&|x, y| (x - rx - 46.0).hypot(y - ry - 46.0) <= 9.0, 40);
    let top = f64::from(BEAD_Y as i32 + bead_dy);
    fill(&|x, y| (60.0..=360.0).contains(&x) && (top..top + BEAD_H as f64).contains(&y), 40);
    FrameImage::new(W, H, px)
}

fn copy(image: &FrameImage) -> FrameImage {
    FrameImage::new(image.width, image.height, image.pixels.clone())
}

/// 单拍照点单视图、定位框为 rect 的配方：位置公差 ±0.5 mm，允许断胶 1.5 mm（比测试里两端露出的空走廊短），只找平移。
fn document(rect: [u32; 4]) -> RecipeDoc {
    let mut doc = crate::recipe::samples().remove(0);
    doc.shots.truncate(1);
    let shot = &mut doc.shots[0];
    shot.path = PATH.to_vec();
    shot.mm_per_px = Some(MM_PER_PX);
    shot.fixture = Some(FixtureSpec { rect, source: TEACH_ID.into(), search_radius_px: 40.0, angle_range_deg: 0.0, min_score: 0.6, max_peak_ratio: 1.0, max_end_shift_mm: None, end_margin_mm: 0.0 });
    doc.spacing = 1.0;
    doc.detect = DetectParams { search_mm: 3.0, polarity: Polarity::Dark, width_range: [0.8, 2.4] };
    doc.limits = ShotLimits {
        position: Some(JudgeParams { nominal: 0.0, tol_upper: 0.5, tol_lower: 0.5, abs_min: -3.0, abs_max: 3.0, max_excursion_len: 3.0 }),
        width: Some(JudgeParams { nominal: 1.6, tol_upper: 0.5, tol_lower: 0.5, abs_min: 0.5, abs_max: 3.0, max_excursion_len: 3.0 }),
        max_gap_len: 1.5,
        min_present: 0.8,
        gauge_ref: None,
    };
    doc
}

/// 发布到临时目录、再从磁盘加载并预热的生产测量；目录随它删除。
struct Production {
    prepared: Prepared,
    root: std::path::PathBuf,
}

impl Drop for Production {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); }
}

impl Production {
    fn recipe(&self) -> &Recipe { &self.prepared.recipe }

    /// 发布包里这个视图的定位模板（生产 base_dir 下的 template.pgm）。
    fn template(&self) -> std::path::PathBuf {
        let view = self.recipe().shots[0].view;
        self.prepared.bundle.view(0, view).unwrap().template.unwrap()
    }
}

/// 每次一个临时目录：并行的测试各自发布、各自删。
fn release_root(tag: &str) -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("gs-locate-engine-{tag}-{}-{seq}", std::process::id()))
}

/// 示教原图与定位框为 rect 的配方经 release::publish 发布到 root（模板由包内示教原图裁出）。
fn publish(root: &Path, teach: &FrameImage, rect: [u32; 4]) -> Result<crate::release::ReleaseBundle, String> {
    let doc = document(rect);
    let view = doc.shots[0].view;
    let mut pgm = format!("P5\n{} {}\n255\n", teach.width, teach.height).into_bytes();
    pgm.extend_from_slice(&teach.pixels);
    // 测量图按能建出的配方生成；建不出（如框压走廊）时交给 publish 自己报错
    let graph = doc.build().ok().map(|recipe| serde_json::to_vec(&crate::production::graphs(&recipe).unwrap()).unwrap());
    crate::release::publish(root, PublishInput {
        recipe: doc, validation_site: ValidationSite::Offline,
        versions: Versions { engine: engine().identity.clone(), graph: crate::production::GRAPH_VERSION.into() },
        graph: ResourceSource::Bytes(graph.unwrap_or_else(|| b"{}".to_vec())),
        shots: vec![ShotInput { k: 0, view, image: Some(ResourceSource::Bytes(pgm)), image_id: Some(TEACH_ID.into()),
            calibration: Some(ResourceSource::Bytes(serde_json::to_vec(&json!({"mmPerPx": MM_PER_PX as f64})).unwrap())) }],
    })
}

/// 发布后 release::load 读回，Prepared::load 加载并预热。
fn production(teach: &FrameImage, rect: [u32; 4], tag: &str) -> Production {
    let root = release_root(tag);
    let published = publish(&root, teach, rect).unwrap();
    let recipe = document(rect).build().unwrap();
    let bundle = crate::release::load(&root, &recipe.id, &published.id).unwrap();
    let prepared = Prepared::load(bundle, engine(), &recipe).unwrap();
    prepared.warm().unwrap();
    Production { prepared, root }
}

/// 生产测量一帧；返回 (测量, 判定)。
fn run(production: &Production, image: FrameImage, tag: &str) -> (MultiViewMeasurement, Judgement) {
    let recipe = production.recipe();
    let view = recipe.shots[0].view;
    let multi = production.prepared.measure_views(0, &[(view, Arc::new(image))], tag).unwrap();
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
    let teach = scene(RECT, 0);
    let production = production(&teach, RECT, "moved");
    let (base, judged) = run(&production, copy(&teach), "teach");
    assert_eq!(judged.verdict, Verdict::Ok, "{}", judged.reason);
    let d0 = &base.reading.d;
    // （整图平移 px，切向搬移 mm，每端排除站数 = ⌊|Δ| + 1 站距⌋ + 1）
    for (dx, dy, delta, ends) in [(12.0, 7.0, 1.2, 3), (-25.0, 0.0, -2.5, 4)] {
        let moved = Perturb::Affine { dx, dy, deg: 0.0, scale: 1.0 }.apply(&teach, 0);
        let (multi, judged) = run(&production, moved, "moved");
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
    let teach = scene(RECT, 0);
    let production = production(&teach, RECT, "bead");
    let (base, _) = run(&production, copy(&teach), "teach");
    let (multi, judged) = run(&production, scene(RECT, 8), "bead");
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
    let teach = scene(RECT, 0);
    let production = production(&teach, RECT, "broken");
    let (base, _) = run(&production, copy(&teach), "teach");
    let mut broken = copy(&teach);
    let mut rng = Rng(0xBAD);
    let [x, y, w, h] = RECT;
    for yy in y - 8..y + h + 8 {
        for xx in x - 8..x + w + 8 { broken.pixels[(yy * W + xx) as usize] = (120.0 + 3.0 * rng.gauss()).round() as u8; }
    }
    let (multi, judged) = run(&production, broken, "broken");
    let m = &multi.views[0].reading.as_ref().unwrap();
    let pose = m.pose.as_ref().unwrap();
    assert!(!pose.accepted && pose.reason.is_some(), "{pose:?}");
    assert_eq!(multi.reference_failed(), [ReferenceFailure { view: production.recipe().shots[0].view, reason: REFERENCE_LOCATE_FAILED.into() }]);
    for i in 0..m.st.len() {
        assert_eq!(m.st[i], ST_NO_REF, "站 {i}");
        assert!((m.w[i] - base.reading.w[i]).abs() < 0.02, "站 {i}：胶宽 {} 示教帧 {}", m.w[i], base.reading.w[i]);
    }
    assert_eq!((judged.verdict, judged.fault_code), (Verdict::ErrInspect, fault::LOCATE_FAILED), "{}", judged.reason);
}

#[test]
#[ignore = "需要 LYFLOW_CORE_DLL（带 lyFlow#32 glue.locate），见 docs/testing/TESTING.md"]
fn production_locate_reads_the_template_inside_the_bundle_it_was_loaded_from() {
    // 防的回归（生产 base_dir 接线）：生产测量的 glue.locate 没从发布包自己的视图目录读模板（base_dir 为空、指向工作台
    // 或别的目录），包里的模板换掉了位姿照样被采用。加载后把包内模板换成同尺寸、特征被抹平的一块：下一帧位姿不再被采用
    let teach = scene(RECT, 0);
    let production = production(&teach, RECT, "flattened");
    let template = production.template();
    let size = std::fs::read(&template).unwrap().len();
    let [_, _, w, h] = RECT;
    let mut flat = format!("P5\n{} {}\n255\n", w + 8, h + 8).into_bytes();
    flat.resize(size, 120);
    std::fs::write(&template, flat).unwrap();
    let (multi, judged) = run(&production, copy(&teach), "flattened");
    let m = &multi.views[0].reading.as_ref().unwrap();
    assert!(m.pose.as_ref().is_some_and(|p| !p.accepted), "{:?}", m.pose);
    assert!(m.st.iter().all(|&s| s == ST_NO_REF), "{:?}", m.st);
    assert_eq!((judged.verdict, judged.fault_code), (Verdict::ErrInspect, fault::LOCATE_FAILED), "{}", judged.reason);
}

#[test]
#[ignore = "需要 LYFLOW_CORE_DLL（带 lyFlow#32 glue.locate），见 docs/testing/TESTING.md"]
fn bead_drifted_to_its_corridor_edge_beside_the_closest_publishable_template_still_judges_ng_position() {
    // 防的回归（定稿 §11 跨阶段约束、§12 走廊检查替代 reject_fixture）：发布不经走廊检查、或走廊半宽 / 余量算小时，
    // 发布允许的定位框离胶太近，胶在检测走廊里漂向框时进了模板，真实 glue.locate 的位姿跟着胶走、胶偏被吸收。
    // 定位框沿法向逐像素挪向胶路：发布第一次拒绝要发生在带外圈的框碰到漂移后的胶之前；最近一个能发布的框上，
    // 胶漂到能测的最远处（−16 px；中心偏移容差 = 搜索半宽 30 − 胶宽上限一半 12 = 18 px）时位姿不动、d 按位移变化、判 NG_POSITION
    const DRIFT: i32 = -16;
    let [x, _, w, h] = RECT;
    let pad = crate::recipe::FIXTURE_TEMPLATE_PAD_PX;
    let at = |y: u32| [x, y, w, h];
    // 带外圈的框最下一行像素在 y + h + pad − 1，要在漂移后的胶条第一行之上
    let drifted_top = (BEAD_Y as i32 + DRIFT) as u32;
    let refused = (RECT[1]..).take_while(|&y| y + h + pad - 1 < drifted_top)
        .find(|&y| document(at(y)).build().is_err())
        .expect("定位框带外圈碰到漂移后的胶之前配方校验都没有拒绝：走廊检查没起作用");
    let root = release_root("corridor-refused");
    let e = publish(&root, &scene(at(refused), 0), at(refused)).unwrap_err();
    let _ = std::fs::remove_dir_all(&root);
    assert!(e.contains("走廊"), "发布应以走廊检查拒绝：{e}");

    let rect = at(refused - 1);
    let teach = scene(rect, 0);
    let production = production(&teach, rect, "corridor-edge");
    let (base, judged) = run(&production, copy(&teach), "teach");
    assert_eq!(judged.verdict, Verdict::Ok, "{}", judged.reason);
    let (multi, judged) = run(&production, scene(rect, DRIFT), "drifted");
    let m = &multi.views[0].reading.as_ref().unwrap();
    assert!(m.pose.as_ref().is_some_and(|p| p.accepted && p.offset.unwrap().iter().all(|v| v.abs() < 0.05)), "{:?}", m.pose);
    let want = DRIFT as f32 * MM_PER_PX * normal_y_sign(m);
    let judged_stations: Vec<usize> = (0..m.st.len()).filter(|&i| m.st[i] == ST_OK).collect();
    assert_eq!(judged_stations.len(), m.st.len() - 4, "恒等位姿下每端只排除 1 个站距");
    for i in judged_stations {
        assert!((m.d[i] - base.reading.d[i] - want).abs() < 0.02, "站 {i}：d {} 示教帧 {}", m.d[i], base.reading.d[i]);
    }
    assert_eq!(judged.verdict, Verdict::NgPosition, "{}", judged.reason);
}

/// 工作台示教试测的同一条路（G4，`workspace_trial`）：模板从这张示教图裁出、放进临时目录，在这张图上测量，
/// 再按示教图检查出报告；`frames` 是同一模板目录上另跑的帧（如亮度 ±40），一并返回它们的位姿。
fn teach_check(teach: &FrameImage, frames: &[&FrameImage], tag: &str) -> (super::super::fixture_quality::FixtureTeachReport, Vec<Option<PoseRecord>>) {
    use super::super::fixture_quality::{geometry_checks, teach_acceptance, TeachInput};
    let doc = document(RECT);
    let recipe = doc.build().unwrap();
    let dir = release_root(tag);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(fixture::TEMPLATE_FILE), fixture::template_pgm(teach, RECT).unwrap()).unwrap();
    let base = dir.to_str().unwrap().to_string();
    let measured = measure_shot(&engine(), &recipe, 0, teach, &format!("{tag}-teach"), &base).unwrap();
    let poses = frames.iter().enumerate().map(|(i, image)| measure_shot(&engine(), &recipe, 0, image, &format!("{tag}-{i}"), &base).unwrap().pose).collect();
    let _ = std::fs::remove_dir_all(&dir);
    let clearances: Vec<_> = doc.fixture_clearances().unwrap().into_iter().filter(|c| c.k == 0).collect();
    let f = recipe.shots[0].fixture.as_ref().unwrap();
    let geometry = geometry_checks(&TeachInput { fixture: f, image: teach, clearances: &clearances, path: &recipe.shots[0].path,
        mm_per_px: Some(f64::from(MM_PER_PX)), tolerance_mm: Some(0.5) });
    (teach_acceptance(geometry, measured.pose.as_ref(), f.angle_range_deg, f.max_peak_ratio), poses)
}

#[test]
#[ignore = "需要 LYFLOW_CORE_DLL（带 lyFlow#32 glue.locate），见 docs/testing/TESTING.md"]
fn teach_check_accepts_the_identity_pose_and_brightness_probes_do_not_mislock_but_a_flattened_box_fails() {
    // 防的回归（G4 E8）：示教图自定位的恒等容差（0.5 px）与真实 glue.locate 的输出对不上，正常框试测无效；
    // peakRatio 没接进报告；亮度 ±40 的验证帧在合成工件上就错锁（锁定判据接错）；纹理被抹平的框仍通过示教检查
    use super::super::fixture_quality::{brightness, validation_report, CheckId, FixtureSample, ViewContext, BRIGHTNESS_PROBE};
    let teach = scene(RECT, 0);
    let brighter = brightness(&teach, BRIGHTNESS_PROBE);
    let darker = brightness(&teach, -BRIGHTNESS_PROBE);
    let (report, poses) = teach_check(&teach, &[&brighter, &darker], "e8");
    assert!(report.passed, "{:?}", report.checks);
    let pose = report.pose.clone().unwrap();
    assert!(pose.peak_ratio.is_some(), "{pose:?}");
    let sample = FixtureSample { name: "示教图".into(), pose: Some(pose), deviations_mm: vec![0.0, 0.01],         brighter: poses[0].clone().ok_or_else(|| "没有定位结果".into()), darker: poses[1].clone().ok_or_else(|| "没有定位结果".into()), ..Default::default() };
    let ctx = ViewContext { k: 0, view: 1, shot_id: "P1".into(), fixture: document(RECT).shots[0].fixture.clone().unwrap(), tolerance_mm: 0.5, nominal_mm: 0.0,
        lever_mm: 30.0, spacing_mm: 1.0, stations: 31 };
    let validation = validation_report(&ctx, &[sample]);
    let lock = validation.checks.iter().find(|c| c.id == CheckId::BrightnessLock).unwrap();
    assert!(lock.passed, "{}；+40 {:?}，−40 {:?}", lock.detail, poses[0], poses[1]);

    // 定位框（含外圈附近）抹成 120 ± 3 的噪声：纹理不足，示教检查不过
    let mut flat = copy(&teach);
    let mut rng = Rng(0xF1A7);
    let [x, y, w, h] = RECT;
    for yy in y - 8..y + h + 8 {
        for xx in x - 8..x + w + 8 { flat.pixels[(yy * W + xx) as usize] = (120.0 + 3.0 * rng.gauss()).round() as u8; }
    }
    let (report, _) = teach_check(&flat, &[], "e8-flat");
    assert!(!report.passed);
    assert!(report.checks.iter().any(|c| c.id == CheckId::Texture && !c.passed), "{:?}", report.checks);
}

// ---- P1 吞吐摸底（docs/decisions/segment-burst.md 第七节）----
// 探针，不是回归测试：量 1280×1024 单帧在发布加载路径上的测量耗时（p50/p95/max，串行与并发）和 PGM 连续写盘速率，
// 数值回填 MEASURE_WORKERS 与 IMAGE_BUDGET_PER_CAMERA。断言只保证量的是正常路径（胶测得到、文件写满），数值本身不设门槛。
// 只用中线测量的算子（glue.taught_path + glue.bead_width），没有特征定位，与段模型走廊测量是同一组算子；
// 不要求带 lyFlow#32 的构建，已发布的运行库即可：
//   $env:LYFLOW_CORE_DLL = 'src-tauri\target\dependency-cache\lyflow\1.1.5\lyflow_core.dll'
//   cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored throughput_probe --nocapture

const PROBE_W: u32 = 1280;
const PROBE_H: u32 = 1024;
/// 与离线端到端的模拟标定当量一致（同分辨率现场相机的假设当量），走廊约 121 mm、122 站
const PROBE_MM_PER_PX: f32 = 0.112;
const PROBE_PATH: [[f32; 2]; 2] = [[100.0, 512.0], [1180.0, 512.0]];
const PROBE_MEASURES: usize = 300;
const PROBE_PGMS: usize = 3000;
/// 每张 sync_data 的写盘变体只写这么多张：量不经系统缓存的盘速
const PROBE_SYNCED_PGMS: usize = 300;
/// 三台相机各写自己的目录
const PROBE_WRITERS: usize = 3;

/// 已发布的运行库即可（不要求 glue.locate 带外圈模板）。
fn probe_engine() -> Arc<Engine> {
    static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let path = std::env::var("LYFLOW_CORE_DLL").expect("需要 LYFLOW_CORE_DLL 指向 lyflow_core.dll（已发布的运行库即可）");
        Arc::new(Engine::load(Path::new(&path)).unwrap())
    }).clone()
}

/// 1280×1024：灰 120 ± 3 噪声底，一条 16 px 宽的暗胶条沿 y = 512 横穿示教走廊。
fn probe_scene() -> FrameImage {
    let mut rng = Rng(0x5EED);
    let mut px: Vec<u8> = (0..PROBE_W * PROBE_H).map(|_| (120.0 + 3.0 * rng.gauss()).round() as u8).collect();
    for y in 504..520 {
        for x in 100..=1180 { px[(y * PROBE_W + x) as usize] = 40; }
    }
    FrameImage::new(PROBE_W, PROBE_H, px)
}

/// 单拍照点、无特征定位、走廊横穿整幅图的配方。
fn probe_document() -> RecipeDoc {
    let mut doc = document(RECT);
    let shot = &mut doc.shots[0];
    shot.path = PROBE_PATH.to_vec();
    shot.mm_per_px = Some(PROBE_MM_PER_PX);
    shot.fixture = None;
    doc
}

/// 与生产同一条加载路径：publish → release::load → Prepared::load → warm。
fn probe_production(teach: &FrameImage) -> Production {
    let root = release_root("probe");
    let doc = probe_document();
    let view = doc.shots[0].view;
    let recipe = doc.build().unwrap();
    let mut pgm = format!("P5\n{} {}\n255\n", teach.width, teach.height).into_bytes();
    pgm.extend_from_slice(&teach.pixels);
    let graph = serde_json::to_vec(&crate::production::graphs(&recipe).unwrap()).unwrap();
    let published = crate::release::publish(&root, PublishInput {
        recipe: doc, validation_site: ValidationSite::Offline,
        versions: Versions { engine: probe_engine().identity.clone(), graph: crate::production::GRAPH_VERSION.into() },
        graph: ResourceSource::Bytes(graph),
        shots: vec![ShotInput { k: 0, view, image: Some(ResourceSource::Bytes(pgm)), image_id: Some(TEACH_ID.into()),
            calibration: Some(ResourceSource::Bytes(serde_json::to_vec(&json!({"mmPerPx": PROBE_MM_PER_PX as f64})).unwrap())) }],
    }).unwrap();
    let bundle = crate::release::load(&root, &recipe.id, &published.id).unwrap();
    let prepared = Prepared::load(bundle, probe_engine(), &recipe).unwrap();
    prepared.warm().unwrap();
    Production { prepared, root }
}

struct Dist { n: usize, mean: f64, p50: f64, p95: f64, max: f64 }

impl Dist {
    /// 最近秩百分位。
    fn of(mut ms: Vec<f64>) -> Self {
        ms.sort_by(f64::total_cmp);
        let n = ms.len();
        let rank = |pct: usize| ms[(n * pct).div_ceil(100) - 1];
        Self { n, mean: ms.iter().sum::<f64>() / n as f64, p50: rank(50), p95: rank(95), max: ms[n - 1] }
    }

    fn line(&self) -> String {
        format!("n={} mean={:.2} p50={:.2} p95={:.2} max={:.2} ms", self.n, self.mean, self.p50, self.p95, self.max)
    }
}

/// workers 个线程共同测 PROBE_MEASURES 帧：返回每帧耗时（ms）与墙钟（s）。
fn probe_measure(prepared: &Prepared, image: &FrameImage, workers: usize) -> (Vec<f64>, f64) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let wall = Instant::now();
    let per_worker: Vec<Vec<f64>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers).map(|w| {
            let next = &next;
            scope.spawn(move || {
                let mut ms = Vec::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= PROBE_MEASURES { break ms; }
                    let start = Instant::now();
                    let m = prepared.measure(0, image, &format!("probe-w{workers}-{w}-{i}")).unwrap();
                    ms.push(start.elapsed().as_secs_f64() * 1e3);
                    assert!(m.st.len() > 100 && m.coverage > 0.95, "量到的不是正常路径：站数 {}，有胶比例 {}", m.st.len(), m.coverage);
                }
            })
        }).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    (per_worker.concat(), wall.elapsed().as_secs_f64())
}

/// 探针写盘的临时目录，随它删除。
struct ProbeDir(std::path::PathBuf);

impl ProbeDir {
    fn new(tag: &str) -> Self {
        let dir = release_root(tag);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for ProbeDir {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

/// 生产录制用的 image_file::save_pgm 连续写 count 张（文件名从 first 起），返回每张耗时（ms）。
/// 每张改首行 8 个字节（在胶条之外），免得各张内容完全相同。
fn probe_write(dir: &Path, base: &FrameImage, first: usize, count: usize, synced: bool) -> Vec<f64> {
    use std::io::Write;
    let mut image = copy(base);
    (first..first + count).map(|i| {
        image.pixels[..8].copy_from_slice(&(i as u64).to_le_bytes());
        let path = dir.join(format!("{i:05}.pgm"));
        let start = Instant::now();
        if synced {
            let mut file = std::fs::File::create(&path).unwrap();
            file.write_all(format!("P5\n{} {}\n255\n", image.width, image.height).as_bytes()).unwrap();
            file.write_all(&image.pixels).unwrap();
            file.sync_data().unwrap();
        } else {
            crate::image_file::save_pgm(&path, &image).unwrap();
        }
        start.elapsed().as_secs_f64() * 1e3
    }).collect()
}

#[test]
#[ignore = "吞吐摸底探针：需要 LYFLOW_CORE_DLL，写约 4 GB 临时文件；--ignored throughput_probe --nocapture，见 docs/decisions/segment-burst.md 第七节"]
fn throughput_probe() {
    let teach = probe_scene();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let engine = probe_engine();
    println!("[probe] lyFlow {}（{}），逻辑核 {cores}，图像 {PROBE_W}×{PROBE_H}（{} 字节/帧）", engine.version, engine.identity, teach.pixels.len());

    // 一、测量耗时：串行是 p50/p95/max 的依据；并发看 MEASURE_WORKERS 的线性外推是否成立
    let production = probe_production(&teach);
    let stations = production.prepared.measure(0, &teach, "probe-stations").unwrap().st.len();
    println!("[probe] 测点 {stations} 个（间距 1 mm，走廊约 121 mm）");
    let mut levels = vec![1, 2, 3, 4, cores.saturating_sub(2).max(1)];
    levels.sort_unstable();
    levels.dedup();
    let mut serial_p95_ms = 0.0;
    for workers in levels {
        let (ms, wall) = probe_measure(&production.prepared, &teach, workers);
        let dist = Dist::of(ms);
        if workers == 1 { serial_p95_ms = dist.p95; }
        println!("[probe] 测量 并发 {workers}：{}；墙钟 {wall:.2} s，吞吐 {:.1} fps", dist.line(), PROBE_MEASURES as f64 / wall);
    }
    let needed = (75.0 * serial_p95_ms / 1000.0 / 0.7).ceil();
    println!("[probe] MEASURE_WORKERS = ceil(75 fps × 串行 p95 {serial_p95_ms:.2} ms / 0.7) = {needed}；上限 逻辑核 {cores} − 2 = {}", cores.saturating_sub(2));

    // 二、PGM 写盘：生产录制的 save_pgm
    let bytes_per_file = (teach.pixels.len() + format!("P5\n{PROBE_W} {PROBE_H}\n255\n").len()) as f64;
    let report = |label: &str, ms: Vec<f64>, wall: f64| {
        let files = ms.len();
        println!("[probe] 写盘 {label}：{files} 张 × {:.2} MB，{wall:.2} s，{:.1} MB/s（{:.1} 张/s）；单张 {}",
            bytes_per_file / 1e6, files as f64 * bytes_per_file / 1e6 / wall, files as f64 / wall, Dist::of(ms).line());
    };
    {
        let dir = ProbeDir::new("probe-pgm-seq");
        let start = Instant::now();
        let ms = probe_write(&dir.0, &teach, 0, PROBE_PGMS, false);
        report("串行（save_pgm，不 sync）", ms, start.elapsed().as_secs_f64());
        let on_disk = std::fs::metadata(dir.0.join("00000.pgm")).unwrap().len() as f64;
        assert_eq!(on_disk, bytes_per_file, "PGM 文件大小不是头 + 全部像素");
    }
    {
        let dir = ProbeDir::new("probe-pgm-par");
        for cam in 0..PROBE_WRITERS { std::fs::create_dir_all(dir.0.join(format!("cam{cam}"))).unwrap(); }
        let each = PROBE_PGMS / PROBE_WRITERS;
        let start = Instant::now();
        let per_cam: Vec<Vec<f64>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..PROBE_WRITERS).map(|cam| {
                let (dir, teach) = (&dir.0, &teach);
                scope.spawn(move || probe_write(&dir.join(format!("cam{cam}")), teach, 0, each, false))
            }).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        report(&format!("{PROBE_WRITERS} 路并行（每路 {each} 张）"), per_cam.concat(), start.elapsed().as_secs_f64());
    }
    {
        let dir = ProbeDir::new("probe-pgm-sync");
        let start = Instant::now();
        let ms = probe_write(&dir.0, &teach, 0, PROBE_SYNCED_PGMS, true);
        report("串行（每张 sync_data，不经系统缓存）", ms, start.elapsed().as_secs_f64());
    }
}
