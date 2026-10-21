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
