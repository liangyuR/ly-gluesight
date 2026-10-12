use super::*;
use crate::offline::sample_set::{SampleCamera, SampleShot};

fn option(id: &str, runnable: bool, target: Option<Target>) -> RecipeOption {
    RecipeOption { id: id.into(), name: id.into(), version: 1, product_code: 1, shot_count: 4, published: true, runnable,
        reason: (!runnable).then(|| "不可开工".into()), target }
}

fn facts() -> Facts {
    Facts {
        site: Site::Real, plc_connected: true, session: Some(SessionPhase::Idle), session_message: None, phase: Phase::Idle, fault: None,
        cameras: Ok(()), engine: Some(Ok("1.1.0".into())), workers: Ok(()), audit: Check::Ok, record: RecordMode::Off,
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
fn readiness_blocks_the_item_preventing_start_and_points_to_where_it_is_fixed() {
    struct Case { name: &'static str, facts: Facts, key: Option<ItemKey>, target: Option<Target>, hint: bool }
    let cases = [
        Case { name: "全部就绪 → 可开工", facts: facts(), key: None, target: None, hint: false },
        Case { name: "PLC 下发型号且没有可开工配方 → 配方项阻塞并指向配方工作台",
            facts: Facts { recipes: vec![option("A", false, Some(Target::Workspace))], ..facts() }, key: Some(ItemKey::Recipe), target: Some(Target::Workspace), hint: true },
        Case { name: "人工选中的配方相机不符 → 配方项阻塞并指向该配方的去处",
            facts: Facts { product: Product::Manual(Some("B".into())), recipes: vec![option("A", true, None), option("B", false, Some(Target::Camera))], ..facts() },
            key: Some(ItemKey::Recipe), target: Some(Target::Camera), hint: true },
        Case { name: "真机待复位 → PLC 项阻塞并给出复位前提，去 PLC 页",
            facts: pending_reset(facts()), key: Some(ItemKey::Plc), target: Some(Target::Plc), hint: true },
        Case { name: "离线待复位 → PLC 项阻塞并说明自动复位，去离线控制台",
            facts: pending_reset(Facts { site: Site::SampleSet("mx11".into()), ..facts() }), key: Some(ItemKey::Plc), target: Some(Target::Offline), hint: true },
        Case { name: "真机 lyFlow 未加载 → 图像测量项阻塞并指向系统设置",
            facts: Facts { engine: Some(Err("找不到核心库".into())), ..facts() }, key: Some(ItemKey::Engine), target: Some(Target::Settings), hint: false },
        Case { name: "合成图不需要 lyFlow → 可开工",
            facts: Facts { site: Site::Synthetic, engine: None, ..facts() }, key: None, target: None, hint: false },
    ];
    for case in cases {
        let report = evaluate(case.facts);
        let not_ok: Vec<&ReadinessItem> = report.items.iter().filter(|i| i.state != ItemState::Ok).collect();
        assert_eq!(report.ready, case.key.is_none(), "{}", case.name);
        assert_eq!(not_ok.iter().map(|i| i.key).collect::<Vec<_>>(), case.key.into_iter().collect::<Vec<_>>(), "{}", case.name);
        if let Some(blocked) = not_ok.first() {
            assert_eq!(blocked.state, ItemState::Blocked, "{}", case.name);
            assert_eq!(blocked.target, case.target, "{}", case.name);
            assert_eq!(blocked.hint.is_some(), case.hint, "{}", case.name);
        }
    }
}

#[test]
fn sample_set_accepts_only_recipes_whose_shots_cameras_and_views_it_provides() {
    let recipe = crate::recipe::samples().remove(1).build().unwrap();
    let shot = |id: &str, camera: &str, views: &[u8]| SampleShot { shot_id: id.into(), camera_id: camera.into(), views: views.to_vec(), origin: None, mm_per_px: None };
    let manifest = |shots: Vec<SampleShot>| Manifest { schema_version: 1, id: "s".into(), name: "s".into(), source: String::new(), image_size: None,
        mm_per_px: None, cameras: vec![SampleCamera { id: "cam1".into(), views: vec![1] }], shots, parts: Vec::new() };
    let all = |camera: &str, views: &[u8]| (1..=4).map(|k| shot(&format!("P{k}"), camera, views)).collect::<Vec<_>>();
    let mut three_views = recipe.clone();
    three_views.shots[0].view = 3;
    let cases = [
        ("编号、相机、视角齐全", &recipe, all("cam1", &[1]), false),
        ("样本集少一个拍照点", &recipe, all("cam1", &[1]).into_iter().take(3).collect(), true),
        ("相机编号不同", &recipe, all("cam2", &[1]), true),
        ("配方要视角 3，样本集只有视角 1", &three_views, all("cam1", &[1]), true),
    ];
    for (name, recipe, shots, mismatch) in cases {
        assert_eq!(sample_set_mismatch(recipe, &manifest(shots)).is_some(), mismatch, "{name}");
    }
}
