use super::*;
use crate::release::{self, PublishInput, ResourceSource, ShotInput, Versions};

fn image(width: u32, height: u32) -> FrameImage {
    let mut pixels = vec![210; (width * height) as usize];
    for y in 74..90 {
        for x in 20..220 { pixels[(y * width + x) as usize] = 35; }
    }
    FrameImage::new(width, height, pixels)
}

fn pgm(image: &FrameImage) -> Vec<u8> {
    let mut bytes = format!("P5\n{} {}\n255\n", image.width, image.height).into_bytes();
    bytes.extend_from_slice(&image.pixels);
    bytes
}

#[test]
fn graphs_preserve_each_shot_and_only_skip_explicitly_skipped_shots() {
    let mut doc = crate::recipe::samples().remove(1);
    doc.shots[1].skip = true;
    doc.shots[2].camera = "cam3".into();
    doc.shots[2].view = 3;
    let graph = graphs(&doc.build().unwrap()).unwrap();
    assert_eq!(graph["shots"].as_array().unwrap().len(), 4);
    assert!(graph["shots"][1]["graph"].is_null());
    assert!(graph["shots"][0]["graph"].is_object());
    assert_eq!(graph["shots"][2]["camera"], "cam3");
    assert_eq!(graph["shots"][2]["view"], 3);
    doc.shots[0].path.clear();
    assert!(graphs(&doc.build().unwrap()).unwrap_err().contains("尚未示教"));
}

#[test]
fn graphs_keep_exact_identity_after_json_round_trip_with_nonbinary_pixel_scale() {
    let mut doc = crate::recipe::samples().remove(1);
    doc.spacing = 1.0;
    for shot in &mut doc.shots {
        shot.path = vec![[980.0, 480.0], [300.0, 460.0]];
        shot.mm_per_px = Some(0.112);
    }
    let graph = graphs(&doc.build().unwrap()).unwrap();
    let stored: Value = serde_json::from_slice(&serde_json::to_vec(&graph).unwrap()).unwrap();
    let restored: crate::recipe::RecipeDoc = serde_json::from_slice(&serde_json::to_vec(&doc).unwrap()).unwrap();
    assert_eq!(stored, graphs(&restored.build().unwrap()).unwrap());
    for k in 0..4 {
        let restored_graph = &stored["shots"][k]["graph"];
        assert_eq!(restored_graph, &vision::build_taught_graph(&restored.build().unwrap(), k).unwrap());
    }
}

#[test]
fn engine_compatibility_uses_the_declared_version_across_restarts_and_paths() {
    assert!(engine_version_matches("1.1.0", "1.1.0"));
    assert!(engine_version_matches("1.1.0:legacy-ignored-value", "1.1.0"));
    assert!(!engine_version_matches("1.0.0:legacy-ignored-value", "1.1.0"));
}

#[test]
#[ignore = "requires LYFLOW_CORE_DLL with glue.taught_path; immutable production bundle integration"]
fn native_bundle_warms_measures_copied_resources_and_validates_dimensions() {
    let dll = std::env::var_os("LYFLOW_CORE_DLL").expect("Set LYFLOW_CORE_DLL to a taught-path core");
    let engine = Engine::load(std::path::Path::new(&dll)).unwrap();
    let root = std::env::temp_dir().join(format!("gluesight-production-{}-{}", std::process::id(), ly_plc::now_ms()));
    std::fs::create_dir_all(&root).unwrap();
    let engine = Arc::new(engine);
    let mut doc = crate::recipe::samples().remove(1);
    doc.spacing = 1.0;
    doc.detect = crate::recipe::DetectParams { search_mm: 8.0, polarity: crate::recipe::Polarity::Dark, width_range: [1.5, 6.5] };
    for (k, shot) in doc.shots.iter_mut().enumerate() {
        shot.camera = format!("cam{}", k % 3 + 1);
        shot.view = [1, 2, 3, 1][k];
        shot.path = vec![[40.0, 80.5], [200.0, 80.5]];
        shot.mm_per_px = Some(0.25);
    }
    let recipe = doc.build().unwrap();
    let sizes = [[256, 160], [320, 200], [288, 176], [256, 160]];
    let external = root.join("external-calibration.json");
    std::fs::write(&external, r#"{"mmPerPx":0.25}"#).unwrap();
    let input = PublishInput { recipe: doc.clone(),
        versions: Versions { engine: engine.version.clone(), graph: GRAPH_VERSION.into() },
        graph: ResourceSource::Bytes(serde_json::to_vec(&graphs(&recipe).unwrap()).unwrap()),
        shots: sizes.iter().enumerate().map(|(k, size)| ShotInput { k,
            image: Some(ResourceSource::Bytes(pgm(&image(size[0], size[1])))),
            calibration: Some(ResourceSource::File(external.clone())) }).collect() };
    let bundle = release::publish(&root.join("releases"), input).unwrap();
    let prepared = Prepared::load(bundle.clone(), engine.clone(), &recipe).unwrap();
    let started = Instant::now();
    prepared.warm().unwrap();
    let warm_ms = started.elapsed().as_millis();
    let first = prepared.measure(0, &image(256, 160), "production-frozen-before").unwrap();
    assert_eq!(first.idx.len(), 41);
    assert!(first.w.iter().all(|width| (*width - 4.0).abs() < 0.4));
    std::fs::write(&external, r#"{"mmPerPx":9.0}"#).unwrap();
    prepared.verify().unwrap();
    let second = prepared.measure(0, &image(256, 160), "production-frozen-after").unwrap();
    assert_eq!(first.w, second.w);
    assert_eq!(first.d, second.d);
    for (k, size) in sizes.iter().enumerate() {
        assert_eq!(prepared.measure(k, &image(size[0], size[1]), &format!("production-{k}")).unwrap().idx.len(), 41);
    }
    assert!(prepared.measure(1, &image(256, 160), "production-wrong-size").unwrap_err().contains("尺寸"));
    let mut wrong = recipe.clone();
    wrong.revision_id = "wrong-version".into();
    assert!(Prepared::load(bundle.clone(), engine.clone(), &wrong).err().unwrap().contains("生产版本"));
    let mut wrong_layout = recipe.clone();
    wrong_layout.shots[0].camera = "other-device".into();
    assert!(Prepared::load(bundle.clone(), engine, &wrong_layout).err().unwrap().contains("生产版本"));
    let target = bundle.root.join(bundle.manifest.shots[0].calibration.as_ref().unwrap());
    std::fs::write(&target, r#"{"mmPerPx":9.0}"#).unwrap();
    assert!(prepared.verify().is_err());
    println!("Frozen production: cold warmup {warm_ms} ms, steady {} ms, bundle {}", second.ms, bundle.id);
    std::fs::remove_dir_all(root).unwrap();
}
