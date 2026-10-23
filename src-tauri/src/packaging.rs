use std::path::Path;

use serde_json::{json, Value};

use crate::frame::FrameImage;
use crate::measure::ST_OK;
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
    let locate = check_locate(&engine)?;
    let api = crate::mvs::load_bundled_api()?;
    let devices = crate::mvs::enumerate_with_api(&api)?;
    Ok(json!({
        "status": "ok", "executable": executable,
        "lyflow": {"path": engine.path, "version": engine.version,
            "clientAbiVersion": lyflow_client::ABI_VERSION,
            "locate": locate},
        "mvs": {"path": api.library_path, "version": api.version, "deviceCount": devices.len()},
        "acceptance": "synthetic pixels and SDK enumeration; not physical camera or field acceptance"
    }))
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
        angle_range_deg: 0.0, min_score: 0.6, max_peak_ratio: 1.0, max_end_shift_mm: None, end_margin_mm: 0.0 });
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
