//! 按胶色把粗笔划吸附到胶中心：示教 MCP 的 `teach_corridor` 与示教页刷选共用（docs/integration/agent-teaching.md）。

use crate::frame::FrameImage;
use crate::recipe::DetectParams;
use serde_json::{json, Value};

/// 胶色吸附后的平滑半窗（胶中心线站距 4 px，5 点即 ±20 px）
const SNAP_SMOOTH: usize = 5;
/// 胶色吸附第二遍的偏移上限（px）
const SNAP_REFINE: f32 = 10.0;

/// 笔划直接作走廊：各段按不超过 `spacing` px 等分插点（同示教页“笔划直接生成走廊”的点间距）
pub(super) fn densify(stroke: &[[f32; 2]], spacing: f32) -> Vec<[f32; 2]> {
    let mut out = vec![stroke[0]];
    for pair in stroke.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let n = ((b[0] - a[0]).hypot(b[1] - a[1]) / spacing).ceil().max(1.0) as usize;
        for j in 1..=n {
            let t = j as f32 / n as f32;
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    out
}

/// 叠图 overlay 里第一条 `role` 折线的点
fn overlay_polyline(overlay: &Value, role: &str) -> Option<Vec<[f32; 2]>> {
    overlay["items"]
        .as_array()?
        .iter()
        .find(|item| item["kind"] == "polyline" && item["role"] == role)?["points"]
        .as_array()?
        .iter()
        .map(|p| Some([p[0].as_f64()? as f32, p[1].as_f64()? as f32]))
        .collect()
}

/// 滑动平均（窗口 2·`half`+1 点，两端收窄），压掉逐站居中留下的台阶
fn smooth(points: &[[f32; 2]], half: usize) -> Vec<[f32; 2]> {
    (0..points.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(points.len()));
            let window = &points[a..b];
            let n = window.len() as f32;
            [
                window.iter().map(|p| p[0]).sum::<f32>() / n,
                window.iter().map(|p| p[1]).sum::<f32>() / n,
            ]
        })
        .collect()
}

/// 吸附只校正横向位置：两端沿切线补到笔划起点、终点在切线上的投影，走廊沿胶路的起止仍由笔划决定
/// （颜色模型沿走廊取样，走廊缩短会少采一段胶；出胶点未标定时还由定向段的走廊起点推出）
fn extend_ends(mut path: Vec<[f32; 2]>, ends: ([f32; 2], [f32; 2])) -> Vec<[f32; 2]> {
    let reach = |a: [f32; 2], b: [f32; 2], target: [f32; 2]| -> Option<[f32; 2]> {
        let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
        let length = dx.hypot(dy);
        if length < 1e-3 {
            return None;
        }
        let (ux, uy) = (dx / length, dy / length);
        let ahead = (target[0] - a[0]) * ux + (target[1] - a[1]) * uy;
        (ahead > 1.0).then(|| [a[0] + ux * ahead, a[1] + uy * ahead])
    };
    if path.len() >= 2 {
        if let Some(head) = reach(path[0], path[1], ends.0) {
            path.insert(0, head);
        }
        let n = path.len();
        if let Some(tail) = reach(path[n - 1], path[n - 2], ends.1) {
            path.push(tail);
        }
    }
    path
}

/// 每隔 `every` 个点留一个，末点总留
fn thin(points: &[[f32; 2]], every: usize) -> Vec<[f32; 2]> {
    let mut out: Vec<_> = points.iter().copied().step_by(every.max(1)).collect();
    if let (Some(&last), Some(&kept)) = (points.last(), out.last()) {
        if last != kept {
            out.push(last);
        }
    }
    out
}

/// 吸附流程：笔划插点作示教线，`recenter(线, 偏移上限)` 给出重新居中后的胶中心线（站距 4 px）与平均移动距离；
/// 没找到胶的站保留原偏移会留下台阶，所以平滑后以小偏移上限再居中一遍、再平滑，按 20 px 抽稀，两端补回笔划首尾。
/// 返回走廊与第一遍的平均移动距离（px）。
fn snap_path(
    stroke: &[[f32; 2]],
    shift: f32,
    mut recenter: impl FnMut(&[[f32; 2]], f32) -> Result<(Vec<[f32; 2]>, Option<f64>), String>,
) -> Result<(Vec<[f32; 2]>, Option<f64>), String> {
    let (first, moved) = recenter(&densify(stroke, 8.0), shift)?;
    let (second, _) = recenter(&smooth(&first, SNAP_SMOOTH), shift.min(SNAP_REFINE))?;
    let path = thin(&smooth(&second, SNAP_SMOOTH), 5);
    Ok((
        extend_ends(path, (stroke[0], stroke[stroke.len() - 1])),
        moved,
    ))
}

/// 按胶色把粗笔划吸附到胶中心（[`snap_path`]）：重新居中用 `glue.color_model`，以线为示教线，每站在偏移上限 `shift`（px）内找胶色段。
/// `scale` 为像素当量（mm/px），胶宽范围按它换成像素。返回走廊与第一遍的平均移动距离（px）。
#[allow(clippy::too_many_arguments)]
pub(super) fn snap_color(
    engine: &crate::vision::Engine,
    recipe: &str,
    camera: &str,
    detect: &DetectParams,
    scale: f64,
    image: &FrameImage,
    stroke: &[[f32; 2]],
    shift: f32,
) -> Result<(Vec<[f32; 2]>, Option<f64>), String> {
    let recenter =
        |line: &[[f32; 2]], shift: f32| -> Result<(Vec<[f32; 2]>, Option<f64>), String> {
            let mut params = super::burst::color_teach_params(line, detect, scale)?;
            params["shiftMax"] = json!(shift);
            let result = engine.run_targets(
                crate::vision::template::color_teach().text,
                &crate::vision::unique_run_id(&format!("snap-{recipe}-{camera}")),
                image,
                &params,
                &["n_model".to_string()],
            )?;
            if result.status() != "ok" {
                return Err(format!("按胶色吸附失败：{}", result.failure()));
            }
            let centers = result
                .record("overlay")
                .and_then(|o| overlay_polyline(o, "center"))
                .filter(|c| c.len() >= 2)
                .ok_or("按胶色没有找到胶中心：笔划可能没压在胶条上")?;
            let moved = result
                .record("model")
                .and_then(|m| m["quality"]["centerShift"].as_f64());
            Ok((centers, moved))
        };
    snap_path(stroke, shift, recenter)
}
