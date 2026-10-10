use serde_json::{json, Value};

use crate::judge::{self, Judgement, PointState};
use crate::production::Prepared;

pub(super) fn measure(prepared: &Prepared, count: usize, sn: u32, cycle_id: &str,
    mut load: impl FnMut(usize) -> Result<crate::frame::FrameImage, String>) -> Result<(Judgement, Vec<Value>), String> {
    let recipe = &prepared.recipe;
    if count != recipe.shot_count() { return Err("原图数量与原发布计划不一致".into()); }
    let mut table = vec![PointState::Pending; recipe.point_count()];
    let mut measurements = Vec::new();
    let mut failures = Vec::new();
    for (k, shot) in recipe.shots.iter().enumerate() {
        if !shot.measured() { continue; }
        let image = load(k)?;
        match prepared.measure(k, &image, &format!("reproduce-{cycle_id}-{k}-{}", ly_plc::now_ms())) {
            Ok(reading) => {
                for (i, &j) in reading.idx.iter().enumerate() {
                    table[j as usize] = if reading.st[i] == crate::measure::ST_OK {
                        PointState::Measured { d: reading.d[i], w: reading.w[i] }
                    } else { PointState::Gap };
                }
                measurements.push(json!({"cycleId":cycle_id,"bundleId":prepared.bundle.id,
                    "shotId":shot.id,"camera":shot.camera,"sn":sn,"k":k,"cam":0,"located":true,
                    "score":reading.coverage,"ms":reading.ms,"error":null,"idx":reading.idx,
                    "d":reading.d,"w":reading.w,"st":reading.st,"px":reading.px}));
            }
            Err(error) => {
                failures.push(format!("拍照点 {}：{error}", shot.id));
                for j in recipe.owned_points(k) { table[j] = PointState::Invalid; }
                measurements.push(json!({"cycleId":cycle_id,"bundleId":prepared.bundle.id,
                    "shotId":shot.id,"camera":shot.camera,"sn":sn,"k":k,"cam":0,"located":false,
                    "score":0,"ms":0,"error":error,"idx":[],"d":[],"w":[],"st":[],"px":[]}));
            }
        }
    }
    prepared.verify()?;
    let judgement = if failures.is_empty() { judge::judge(recipe, &table) }
        else { Judgement::error(judge::fault::PROCESS_TIMEOUT, failures.join("；")) };
    Ok((judgement, measurements))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::frame::FrameImage;
    use crate::judge::Verdict;
    use crate::release::{self, PublishInput, ResourceSource, ShotInput, Versions};

    fn image() -> FrameImage {
        let mut pixels = vec![210; 256 * 160];
        for y in 74..90 { for x in 20..220 { pixels[y * 256 + x] = 35; } }
        FrameImage::new(256, 160, pixels)
    }

    fn pgm(image: &FrameImage) -> Vec<u8> {
        let mut bytes = format!("P5\n{} {}\n255\n", image.width, image.height).into_bytes();
        bytes.extend_from_slice(&image.pixels);
        bytes
    }

    #[test]
    #[ignore = "requires LYFLOW_CORE_DLL with glue.taught_path; original release reproduction"]
    fn native_original_reproduction_keeps_frozen_resources_and_reports_each_failed_shot() {
        let dll = std::env::var_os("LYFLOW_CORE_DLL").expect("Set LYFLOW_CORE_DLL");
        let engine = Arc::new(crate::vision::Engine::load(std::path::Path::new(&dll)).unwrap());
        let root = super::super::tests::ImportTestDir::new();
        let mut doc = crate::recipe::samples().remove(1);
        doc.spacing = 1.0;
        doc.detect = crate::recipe::DetectParams { search_mm: 8.0, polarity: crate::recipe::Polarity::Dark, width_range: [1.5, 6.5] };
        for (k, shot) in doc.shots.iter_mut().enumerate() {
            shot.camera = format!("cam{}", k % 3 + 1);
            shot.view = [1, 2, 3, 1][k];
            shot.path = vec![[40.0, 80.5], [200.0, 80.5]];
            shot.mm_per_px = Some(0.25);
        }
        let original = doc.build().unwrap();
        let external = root.0.join("station.json");
        std::fs::write(&external, r#"{"mmPerPx":0.25}"#).unwrap();
        let input = PublishInput { recipe: doc, versions: Versions { engine: engine.identity.clone(), graph: crate::production::GRAPH_VERSION.into() },
            graph: ResourceSource::Bytes(serde_json::to_vec(&crate::production::graphs(&original).unwrap()).unwrap()),
            shots: (0..4).map(|k| ShotInput { k, image: Some(ResourceSource::Bytes(pgm(&image()))),
                calibration: Some(ResourceSource::File(external.clone())) }).collect() };
        let bundle = release::publish(&root.0.join("releases"), input).unwrap();
        let prepared = Prepared::load(bundle, engine, &original).unwrap();
        let (first, first_measurements) = measure(&prepared, 4, 101, "same-sn-first", |_| Ok(image())).unwrap();
        assert_eq!(first.verdict, Verdict::Ok);
        assert_eq!(first_measurements.len(), 4);
        std::fs::write(&external, r#"{"mmPerPx":9.0}"#).unwrap();
        let (again, again_measurements) = measure(&prepared, 4, 101, "same-sn-second", |_| Ok(image())).unwrap();
        assert_eq!(again.verdict, first.verdict);
        for k in 0..4 {
            for field in ["idx", "d", "w", "st", "px"] { assert_eq!(again_measurements[k][field], first_measurements[k][field]); }
            assert_eq!(again_measurements[k]["cycleId"], "same-sn-second");
            assert_eq!(again_measurements[k]["bundleId"], prepared.bundle.id);
            assert_eq!(again_measurements[k]["camera"], original.shots[k].camera);
        }
        let (gap, gap_measurements) = measure(&prepared, 4, 101, "gap-piece", |k| {
            let mut raw = image();
            if k == 1 {
                for y in 74..90 { for x in 100..140 { raw.pixels[y * 256 + x] = 210; } }
            }
            Ok(raw)
        }).unwrap();
        assert_eq!(gap.verdict, Verdict::NgGap);
        assert_eq!(gap.plc_code, 13);
        let states = gap_measurements[1]["st"].as_array().unwrap();
        assert!(states.iter().any(|status| status == crate::measure::ST_GAP));
        assert!(states.iter().any(|status| status == crate::measure::ST_OK));
        assert!(gap.gaps.iter().any(|gap| gap.frames == [1] && gap.len > 6.0));
        for k in [0, 2, 3] { assert_eq!(gap_measurements[k]["st"], first_measurements[k]["st"]); }
        let (empty, empty_measurements) = measure(&prepared, 4, 101, "empty-shot", |k|
            Ok(if k == 1 { FrameImage::new(256, 160, vec![210; 256 * 160]) } else { image() })).unwrap();
        assert_eq!(empty.verdict, Verdict::ErrInspect);
        assert_eq!(empty.fault_code, judge::fault::PROCESS_TIMEOUT);
        assert_eq!(empty_measurements.len(), 4);
        assert_eq!(empty_measurements[1]["located"], false);
        assert!(empty_measurements[1]["error"].as_str().unwrap().contains("没找到胶"));
        assert!(empty_measurements[1]["idx"].as_array().unwrap().is_empty());
        let (failed, failed_measurements) = measure(&prepared, 4, 101, "failed-piece", |k|
            Ok(if k == 2 { FrameImage::new(200, 100, vec![210; 200 * 100]) } else { image() })).unwrap();
        assert_eq!(failed.verdict, Verdict::ErrInspect);
        assert_eq!(failed.fault_code, judge::fault::PROCESS_TIMEOUT);
        assert_eq!(failed_measurements.len(), 4);
        assert_eq!(failed_measurements[2]["located"], false);
        assert!(failed_measurements[2]["error"].as_str().unwrap().contains("尺寸"));
        assert!(measure(&prepared, 3, 101, "missing-piece", |_| Ok(image())).unwrap_err().contains("数量"));
        assert!(measure(&prepared, 4, 101, "missing-raw", |_| Err("原图不可读取".into())).unwrap_err().contains("不可读取"));
        let frozen = prepared.bundle.shot(0).unwrap().calibration.unwrap();
        std::fs::write(frozen, b"invalid calibration JSON").unwrap();
        assert!(measure(&prepared, 4, 101, "invalid-calibration", |_| Ok(image())).is_err());
    }
}
