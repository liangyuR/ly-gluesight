use super::*;
use crate::offline::sample_set::{SampleCamera, SampleShot};

fn option(id: &str, runnable: bool, target: Option<Target>) -> RecipeOption {
    RecipeOption { id: id.into(), name: id.into(), version: 1, product_code: 1, shot_count: 4, published: true, runnable,
        reason: (!runnable).then(|| "不可开工".into()), target, waiting: None }
}

fn warming(id: &str) -> RecipeOption {
    RecipeOption { waiting: Some("发布资源与图像引擎预热中".into()), ..option(id, true, None) }
}

fn facts() -> Facts {
    Facts {
        site: Site::Real, plc_connected: true, session: Some(SessionPhase::Idle), session_message: None, phase: Phase::Idle, fault: None,
        cameras: Check::Ok, engine: Some(Ok("1.1.0".into())), workers: Ok(()), audit: Check::Ok, record: RecordMode::Off,
        product: Product::Plc, activation_uncertain: false, recipes: vec![option("A", true, None)],
    }
}

fn pending_reset(mut f: Facts) -> Facts {
    f.session = Some(SessionPhase::ResetRequired);
    f.phase = Phase::Fault;
    f.fault = Some("S7 连接会话已更新，需确认空闲输入后复位".into());
    f
}

#[test]
fn readiness_blocks_only_on_blocked_items_and_points_waiting_or_blocked_items_to_where_they_are_fixed() {
    struct Case { name: &'static str, facts: Facts, ready: bool, key: Option<(ItemKey, ItemState)>, target: Option<Target>, hint: bool }
    let cases = [
        Case { name: "全部就绪 → 可开工", facts: facts(), ready: true, key: None, target: None, hint: false },
        Case { name: "PLC 下发型号且没有可开工配方 → 配方项阻塞并指向配方工作台",
            facts: Facts { recipes: vec![option("A", false, Some(Target::Workspace))], ..facts() }, ready: false,
            key: Some((ItemKey::Recipe, ItemState::Blocked)), target: Some(Target::Workspace), hint: true },
        Case { name: "人工选中的配方相机不符 → 配方项阻塞并指向该配方的去处",
            facts: Facts { product: Product::Manual(Some("B".into())), recipes: vec![option("A", true, None), option("B", false, Some(Target::Camera))], ..facts() },
            ready: false, key: Some((ItemKey::Recipe, ItemState::Blocked)), target: Some(Target::Camera), hint: true },
        Case { name: "真机待复位 → PLC 项阻塞并给出复位前提，去 PLC 页",
            facts: pending_reset(facts()), ready: false, key: Some((ItemKey::Plc, ItemState::Blocked)), target: Some(Target::Plc), hint: true },
        Case { name: "离线待复位 → PLC 项阻塞并说明自动复位，去离线控制台",
            facts: pending_reset(Facts { site: Site::SampleSet("mx11".into()), ..facts() }), ready: false,
            key: Some((ItemKey::Plc, ItemState::Blocked)), target: Some(Target::Offline), hint: true },
        Case { name: "真机 lyFlow 未加载 → 图像测量项阻塞并指向系统设置",
            facts: Facts { engine: Some(Err("找不到核心库".into())), ..facts() }, ready: false,
            key: Some((ItemKey::Engine, ItemState::Blocked)), target: Some(Target::Settings), hint: false },
        Case { name: "合成图不需要 lyFlow → 可开工",
            facts: Facts { site: Site::Synthetic, engine: None, ..facts() }, ready: true, key: None, target: None, hint: false },
        Case { name: "审计等待原图录制 → 审计项等待，不阻塞开工",
            facts: Facts { audit: Check::Waiting("原图录制尚未完成耐久接受".into()), ..facts() }, ready: true,
            key: Some((ItemKey::Audit, ItemState::Waiting)), target: None, hint: false },
        Case { name: "唯一可开工配方正在预热 → 配方项等待，不阻塞开工",
            facts: Facts { recipes: vec![warming("A")], ..facts() }, ready: true,
            key: Some((ItemKey::Recipe, ItemState::Waiting)), target: None, hint: false },
        Case { name: "另一个可开工配方已预热 → 配方项就绪",
            facts: Facts { recipes: vec![warming("A"), option("B", true, None)], ..facts() }, ready: true, key: None, target: None, hint: false },
        Case { name: "部分配方相机掉线 → 相机项等待并指向相机页，不阻塞开工",
            facts: Facts { cameras: Check::Waiting("配方 B 暂时开不了工：cam2 未连接".into()), ..facts() }, ready: true,
            key: Some((ItemKey::Camera, ItemState::Waiting)), target: Some(Target::Camera), hint: false },
    ];
    for case in cases {
        let report = evaluate(case.facts);
        let not_ok: Vec<&ReadinessItem> = report.items.iter().filter(|i| i.state != ItemState::Ok).collect();
        assert_eq!(report.ready, case.ready, "{}", case.name);
        assert_eq!(not_ok.iter().map(|i| (i.key, i.state)).collect::<Vec<_>>(), case.key.into_iter().collect::<Vec<_>>(), "{}", case.name);
        if let Some(item) = not_ok.first() {
            assert_eq!(item.target, case.target, "{}", case.name);
            assert_eq!(item.hint.is_some(), case.hint, "{}", case.name);
        }
    }
}

#[test]
fn manual_product_source_lists_only_the_selected_recipe_as_runnable() {
    let report = evaluate(Facts {
        product: Product::Manual(Some("B".into())),
        recipes: vec![option("A", true, None), option("B", true, None), option("C", false, Some(Target::Workspace))],
        ..facts()
    });
    let runnable: Vec<(&str, bool, Option<&str>)> = report.recipes.iter().map(|r| (r.id.as_str(), r.runnable, r.reason.as_deref())).collect();
    assert_eq!(runnable, [("A", false, Some(MANUAL_ONLY)), ("B", true, None), ("C", false, Some("不可开工"))]);
    assert!(report.ready);
}

#[test]
fn cameras_block_only_when_no_runnable_recipe_has_all_its_cameras_ready() {
    let down = |id: &str| (id.to_string(), Err::<(), _>(format!("{id} 的相机未连接")));
    let up = |id: &str| (id.to_string(), Ok::<(), String>(()));
    let cases = [
        ("全部就绪", vec![up("A"), up("B")], "ok"),
        ("一个配方的相机掉线、另一个就绪", vec![up("A"), down("B")], "waiting"),
        ("全部配方的相机都掉线", vec![down("A"), down("B")], "blocked"),
    ];
    for (name, groups, expected) in cases {
        let state = match camera_check(&groups) { Check::Ok => "ok", Check::Waiting(_) => "waiting", Check::Blocked(_) => "blocked" };
        assert_eq!(state, expected, "{name}");
    }
}

#[test]
fn bundle_check_never_remembers_success_and_retries_failure_only_after_it_expires() {
    let checks = BundleChecks::default();
    let key = || ("A".to_string(), "r1".to_string());
    let now = Instant::now();
    assert!(checks.check(key(), now, || Ok(())).is_ok());
    assert!(checks.check(key(), now, || Err::<(), _>("发布包已被删除".to_string())).is_err(), "通过后包被删 → 下一次核对发现");
    let mut rechecked = false;
    assert!(checks.check(key(), now + Duration::from_secs(1), || { rechecked = true; Ok(()) }).is_err());
    assert!(!rechecked, "失败结果未到期前不重查");
    assert!(checks.check(key(), now + FAILED_CHECK_TTL, || Ok(())).is_ok(), "失败到期后重查");
}

#[test]
fn sample_set_accepts_only_recipes_whose_shots_cameras_and_camera_views_it_provides() {
    let recipe = crate::recipe::samples().remove(1).build().unwrap();
    let shot = |id: &str, camera: &str, views: &[u8]| SampleShot { shot_id: id.into(), camera_id: camera.into(), views: views.to_vec(), origin: None, mm_per_px: None };
    let manifest = |shots: Vec<SampleShot>| Manifest { schema_version: 1, id: "s".into(), name: "s".into(), source: String::new(), image_size: None,
        mm_per_px: None, cameras: vec![SampleCamera { id: "cam1".into(), views: vec![1] }], shots, parts: Vec::new() };
    let all = |camera: &str, views: &[u8]| (1..=4).map(|k| shot(&format!("P{k}"), camera, views)).collect::<Vec<_>>();
    let cases = [
        ("编号、相机、视角齐全", all("cam1", &[1]), 1, false),
        ("样本集少一个拍照点", all("cam1", &[1]).into_iter().take(3).collect(), 1, true),
        ("相机编号不同", all("cam2", &[1]), 1, true),
        ("相机配置 3 个视角，样本集只有视角 1", all("cam1", &[1]), 3, true),
    ];
    for (name, shots, view_count, mismatch) in cases {
        assert_eq!(sample_set_mismatch(&recipe, &manifest(shots), |_| Some(view_count)).is_some(), mismatch, "{name}");
    }
}
