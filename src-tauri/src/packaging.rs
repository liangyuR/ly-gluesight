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
    let api = crate::mvs::load_bundled_api()?;
    let devices = crate::mvs::enumerate_with_api(&api)?;
    Ok(json!({
        "status": "ok", "executable": executable,
        "lyflow": {"path": engine.path, "version": engine.version,
            "clientAbiVersion": lyflow_client::ABI_VERSION,
            "measurement": measurements, "calibration": calibration},
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
