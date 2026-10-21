use std::path::Path;

use serde_json::{json, Value};

use crate::frame::FrameImage;
use crate::measure::{ST_GAP, ST_OK};
use crate::recipe::{DetectParams, Polarity};
use crate::vision::{measure_shot, Engine};

pub fn check_packaged_runtime(report_path: &Path) -> Result<(), String> {
    let result = check_runtime();
    let report = match &result {
        Ok(report) => report.clone(),
        Err(error) => json!({"status": "failed", "error": error}),
    };
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    std::fs::write(report_path, bytes).map_err(|error| format!("写入运行库验证报告失败：{error}"))?;
    result.map(|_| ())
}

fn check_runtime() -> Result<Value, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let root = executable.parent().ok_or("无法解析安装目录")?;
    let engine = Engine::load(&root.join("runtime/lyflow/lyflow_core.dll"))?;
    let measurements = check_measurement(&engine)?;
    let calibration = check_calibration(&engine)?;
    let locate = check_locate(&engine)?;
    let api = crate::mvs::load_bundled_api()?;
    let devices = crate::mvs::enumerate_with_api(&api)?;
    Ok(json!({
        "status": "ok", "executable": executable,
        "lyflow": {"path": engine.path, "version": engine.version,
            "clientAbiVersion": lyflow_client::ABI_VERSION,
            "measurement": measurements, "calibration": calibration, "locate": locate},
        "mvs": {"path": api.library_path, "version": api.version, "deviceCount": devices.len()},
        "acceptance": "synthetic pixels and SDK enumeration; not physical camera or field acceptance"
    }))
}

fn check_measurement(engine: &Engine) -> Result<Value, String> {
    let mut document = crate::recipe::samples().remove(0);
    document.shots.truncate(1);
    document.shots[0].path = vec![[40.0, 80.5], [200.0, 80.5]];
    document.shots[0].mm_per_px = Some(0.25);
    document.shots[0].views.clear();
    document.spacing = 1.0;
    document.detect = DetectParams { search_mm: 8.0, polarity: Polarity::Dark, width_range: [2.0, 6.0] };
    let recipe = document.build()?;
    let image = |center: Option<usize>| {
        let mut pixels = vec![220; 256 * 160];
        if let Some(center) = center {
            for y in center - 8..center + 8 {
                for x in 16..240 {
                    pixels[y * 256 + x] = 28;
                }
            }
        }
        FrameImage::new(256, 160, pixels)
    };
    let full = measure_shot(engine, &recipe, 0, &image(Some(82)), "package-full", "")?;
    if full.idx.len() != 41 || full.st.iter().any(|&state| state != ST_OK)
        || full.w.iter().any(|width| !width.is_finite() || (width - 4.0).abs() >= 0.4) {
        return Err("随包引擎的实际胶路测量结果无效".into());
    }
    let blank = measure_shot(engine, &recipe, 0, &image(None), "package-blank", "")?;
    if blank.idx.len() != 41 || blank.coverage != 0.0 || blank.st.iter().any(|&state| state != ST_GAP) {
        return Err("随包引擎复用了上一帧结果，空白图未报告缺胶".into());
    }
    let shifted = measure_shot(engine, &recipe, 0, &image(Some(88)), "package-shifted", "")?;
    if shifted.idx.len() != 41 || shifted.st.iter().any(|&state| state != ST_OK)
        || shifted.d.iter().zip(&full.d).any(|(next, previous)| !next.is_finite() || !previous.is_finite()
            || ((next - previous) - 1.5).abs() >= 0.3) {
        return Err("随包引擎没有使用新帧像素完成偏移测量".into());
    }
    Ok(json!({"full": full, "blank": blank, "shifted": shifted}))
}

fn check_calibration(engine: &Engine) -> Result<Value, String> {
    let mut pixels = vec![220; 640 * 480];
    for y in 0..280 {
        for x in 0..400 {
            pixels[(y + 80) * 640 + x + 80] = if (x / 40 + y / 40) % 2 == 0 { 30 } else { 220 };
        }
    }
    let graph = json!({"schemaVersion": 1, "id": "01TUJIAOPACKAGECALIB000000", "nodes": [
        {"id": "n_load", "op": "io.load_image", "params": {"source": "inputs"}},
        {"id": "n_calib", "op": "image.board_calib", "params": {"pattern": [9, 6], "square": 5}}
    ], "edges": [{"id": "e0", "from": {"node": "n_load", "port": "image"},
        "to": {"node": "n_calib", "port": "image"}}],
        "outputs": {"calib": {"node": "n_calib", "port": "calib"}}}).to_string();
    let calibrated = engine.run(&graph, "package-board", "", &FrameImage::new(640, 480, pixels), &json!({}))?;
    if calibrated.status() != "ok" {
        return Err(format!("随包引擎标定失败：{}", calibrated.failure()));
    }
    let data = calibrated.record("calib").ok_or("随包引擎没有返回标定结果")?;
    if data["unit"] != "mm"
        || data["mmPerPx"].as_f64().is_none_or(|value| !value.is_finite() || (value - 0.125).abs() >= 0.005)
        || data["rms"].as_f64().is_none_or(|value| !value.is_finite() || value >= 0.1) {
        return Err("随包引擎标定结果超出合成样本预期".into());
    }
    let blank = engine.run(&graph, "package-no-board", "", &FrameImage::new(640, 480, vec![220; 640 * 480]), &json!({}))?;
    if blank.status() != "failed" || !blank.failure().contains("no_board") || blank.record("calib").is_some() {
        return Err("随包引擎错误地在空白图上复用了标定结果".into());
    }
    Ok(data.clone())
}

/// 定位冒烟里定位框的模板来源（配方校验要求有合法的冻结图编号）
const LOCATE_SMOKE_SOURCE: &str = "package-smoke";
const LOCATE_RECT: [u32; 4] = [40, 30, 64, 64];

/// 随包运行库的 glue.locate 冒烟（带外圈模板，templatePad = 4），只报告状态，放不放行由打包脚本按
/// packaging/runtime.json 的 lyflow.locateSmoke 决定（optional：警告；required：失败）：
/// - 运行库不支持带外圈模板（如已发布的 1.1.4，locate 1.0.0）：status = unsupported，写明影响，不当作自检失败，
///   不挡没有特征定位的构建；
/// - 支持：走产品同一条 measure_shot 路径跑合成图，示教图上位姿须被采用、偏移≈0，整图平移 (+6, +4) 后偏移≈(6, 4)、
///   胶偏不变；任一不满足就返回 Err，整个自检失败。
pub(crate) fn check_locate(engine: &Engine) -> Result<Value, String> {
    if let Err(reason) = &engine.locate {
        return Ok(json!({"status": "unsupported", "reason": reason, "effect": "含特征定位的配方不能发布，不能在生产加载"}));
    }
    let recipe = locate_recipe()?;
    let dir = std::env::temp_dir().join(format!("gluesight-locate-smoke-{}-{}", std::process::id(), ly_plc::now_ms()));
    let result = (|| {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建定位冒烟模板目录失败：{e}"))?;
        let teach = locate_scene(0, 0);
        std::fs::write(dir.join(crate::vision::fixture::TEMPLATE_FILE), crate::vision::fixture::template_pgm(&teach, LOCATE_RECT)?)
            .map_err(|e| format!("写定位冒烟模板失败：{e}"))?;
        let base = dir.to_str().ok_or("定位冒烟模板目录不是 UTF-8 路径")?;
        let still = measure_shot(engine, &recipe, 0, &teach, "package-locate-teach", base)?;
        let moved = measure_shot(engine, &recipe, 0, &locate_scene(6, 4), "package-locate-moved", base)?;
        let offset = |m: &crate::vision::ShotMeasurement| m.pose.as_ref().filter(|p| p.accepted).and_then(|p| p.offset);
        let at_rest = offset(&still).ok_or_else(|| format!("随包引擎在示教图上定位失败：{:?}", still.pose))?;
        if at_rest.iter().any(|v| v.abs() >= 0.05) { return Err(format!("随包引擎在示教图上的定位偏移 {at_rest:?} 不为 0")); }
        let shifted = offset(&moved).ok_or_else(|| format!("随包引擎在平移图上定位失败：{:?}", moved.pose))?;
        if (shifted[0] - 6.0).abs() > 0.1 || (shifted[1] - 4.0).abs() > 0.1 { return Err(format!("随包引擎整图平移 (6, 4) 后定位偏移为 {shifted:?}")); }
        let compared: Vec<usize> = (0..still.st.len()).filter(|&i| still.st[i] == ST_OK && moved.st[i] == ST_OK).collect();
        if compared.len() < still.st.len() / 2 || compared.iter().any(|&i| (moved.d[i] - still.d[i]).abs() >= 0.05) {
            return Err("随包引擎整图平移后胶偏变了：定位位姿没有用到胶偏上".into());
        }
        Ok(json!({"status": "ok", "templatePad": crate::recipe::FIXTURE_TEMPLATE_PAD_PX, "teach": still.pose, "shifted": moved.pose}))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// 定位冒烟的配方：一条水平胶路，定位框离走廊足够远（过配方校验，含走廊检查与位置限值），只找平移。
fn locate_recipe() -> Result<crate::recipe::Recipe, String> {
    let mut document = crate::recipe::samples().remove(0);
    document.shots.truncate(1);
    let shot = &mut document.shots[0];
    shot.views.clear();
    shot.path = vec![[40.0, 180.0], [280.0, 180.0]];
    shot.mm_per_px = Some(0.1);
    shot.fixture = Some(crate::recipe::FixtureSpec { rect: LOCATE_RECT, source: LOCATE_SMOKE_SOURCE.into(), search_radius_px: 20.0,
        angle_range_deg: 0.0, min_score: 0.6, max_peak_ratio: 1.0, max_end_shift_mm: None });
    document.spacing = 1.0;
    document.detect = DetectParams { search_mm: 3.0, polarity: Polarity::Dark, width_range: [0.8, 2.4] };
    document.build()
}

/// 定位冒烟的合成图（320 × 240）：灰底加按坐标散列的 ±3 灰度起伏，定位框里一个亮 L 角、两个暗孔，一条暗胶条横过走廊。
/// 整图平移 (dx, dy)：每个像素取原图 (x − dx, y − dy) 处的值（起伏也跟着走），不插值。
fn locate_scene(dx: i64, dy: i64) -> FrameImage {
    let (w, h) = (320i64, 240i64);
    let [rx, ry, _, _] = LOCATE_RECT.map(i64::from);
    let value = |x: i64, y: i64| -> u8 {
        let inside = |x0: i64, x1: i64, y0: i64, y1: i64| (x0..x1).contains(&x) && (y0..y1).contains(&y);
        let disc = |cx: i64, cy: i64, r: i64| (x - cx).pow(2) + (y - cy).pow(2) <= r * r;
        if inside(rx + 4, rx + 60, ry + 4, ry + 12) || inside(rx + 4, rx + 12, ry + 4, ry + 56) { return 210; }
        if disc(rx + 30, ry + 24, 7) || disc(rx + 46, ry + 46, 9) { return 40; }
        if inside(20, 300, 172, 188) { return 40; }
        let hash = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)).wrapping_mul(2_654_435_761) as u64;
        (117 + (hash >> 29) % 7) as u8
    };
    let pixels = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| value(x - dx, y - dy)).collect();
    FrameImage::new(w as u32, h as u32, pixels)
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "需要 LYFLOW_CORE_DLL（带 lyFlow#32 glue.locate），见 docs/testing/TESTING.md"]
    fn locate_smoke_passes_on_a_runtime_with_padded_template_support() {
        // 防的回归：打包自检的定位冒烟在支持带外圈模板的运行库上误报失败（合成图、配方或断言门限不对），L1 发布后
        // locateSmoke 改为 required 时挡住正常的打包
        let path = std::env::var("LYFLOW_CORE_DLL").expect("需要 LYFLOW_CORE_DLL 指向 lyflow_core.dll");
        let engine = super::Engine::load(std::path::Path::new(&path)).unwrap();
        engine.locate.clone().expect("这个核心库的 glue.locate 不支持带外圈模板（需要 lyFlow#32 之后的构建）");
        let report = super::check_locate(&engine).unwrap();
        assert_eq!(report["status"], "ok", "{report}");
    }

    #[test]
    #[ignore = "需要 LYFLOW_RELEASED_CORE_DLL 指向不支持带外圈模板的已发布运行库（1.1.4），见 docs/testing/TESTING.md"]
    fn locate_smoke_reports_unsupported_instead_of_failing_on_a_runtime_without_padded_templates() {
        // 防的回归（定稿 §12 打包自检）：已发布 1.1.4（glue.locate 1.0.0）上自检把定位冒烟当成失败或跑出错，
        // locateSmoke = optional 时也打不出没有特征定位的包；或报告里缺了原因与影响，打包脚本没法给出明确提示
        let path = std::env::var("LYFLOW_RELEASED_CORE_DLL").expect("需要 LYFLOW_RELEASED_CORE_DLL 指向已发布运行库的 lyflow_core.dll");
        let engine = super::Engine::load(std::path::Path::new(&path)).unwrap();
        assert!(engine.locate.is_err(), "这个核心库支持带外圈模板，不是要验证的已发布运行库");
        let report = super::check_locate(&engine).unwrap();
        assert_eq!(report["status"], "unsupported", "{report}");
        assert!(["reason", "effect"].iter().all(|k| report[k].as_str().is_some_and(|s| !s.is_empty())), "{report}");
    }
}
