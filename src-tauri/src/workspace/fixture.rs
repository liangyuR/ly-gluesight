//! 工作台的特征定位接线（定稿 docs/discussions/fixture-position/final.md §5、§13）：示教图检查进试测、
//! 画框时的只读检查、其他帧上的定位试测、开转角的显式确认。门限与统计在 `vision::fixture_quality`。

use super::*;


/// 每个（拍照点，图）的转角范围，按拍照点编号对上；拍照点级 fixture 跟随所选视图。
fn angle_ranges(doc: &RecipeDoc) -> BTreeMap<(String, u8), f32> {
    let mut out = BTreeMap::new();
    for shot in &doc.shots {
        if let Some(f) = &shot.fixture { out.insert((shot.id.clone(), shot.view), f.angle_range_deg); }
        for v in &shot.views {
            if let Some(f) = &v.fixture { out.insert((shot.id.clone(), v.view), f.angle_range_deg); }
        }
    }
    out
}

/// 开转角只能走示教页高级设置的显式确认（定稿 §10 决定二）：草稿自动保存与整份保存不能把任何视图的转角调大。
pub(super) fn reject_unconfirmed_rotation(old: &RecipeDoc, new: &RecipeDoc) -> Result<(), String> {
    let before = angle_ranges(old);
    match angle_ranges(new).into_iter().find(|(key, angle)| *angle > before.get(key).copied().unwrap_or(0.0)) {
        Some(((shot, view), _)) => Err(format!("拍照点 {shot} 图 {view}：转角只能在示教页的高级设置里确认后开启")),
        None => Ok(()),
    }
}

/// 每个（拍照点，图）的两端参数（maxEndShiftMm、示教余量 m），按拍照点编号对上；拍照点级 fixture 跟随所选视图。
fn end_params(doc: &RecipeDoc) -> BTreeMap<(String, u8), (Option<f32>, f32)> {
    let mut out = BTreeMap::new();
    for shot in &doc.shots {
        if let Some(f) = &shot.fixture { out.insert((shot.id.clone(), shot.view), (f.max_end_shift_mm, f.end_margin_mm)); }
        for v in &shot.views {
            if let Some(f) = &v.fixture { out.insert((shot.id.clone(), v.view), (f.max_end_shift_mm, f.end_margin_mm)); }
        }
    }
    out
}

/// 两端参数只由验证建议写入（`apply_fixture_suggestion`，定稿 §10 决定一）：草稿自动保存、整份保存与保存示教参数
/// 只能原样带过，或回到默认（不设上限、m = 0，如新框）；回到默认在验证里告警（`EndParams`）。
/// 否则前端载荷可以把 m 设到 100 mm、上限设得很大，两端永远不判、护栏失效，验证与发布却照样通过。
pub(super) fn reject_end_param_edit(old: &RecipeDoc, new: &RecipeDoc) -> Result<(), String> {
    let before = end_params(old);
    match end_params(new).into_iter().find(|(key, value)| *value != (None, 0.0) && before.get(key) != Some(value)) {
        Some(((shot, view), _)) => Err(format!("拍照点 {shot} 图 {view}：两端搬移上限与示教余量只能在验证页采用建议时写入")),
        None => Ok(()),
    }
}

/// 模板目录名：`<来源冻结图>-r<x>_<y>_<w>_<h>-p<外圈>`（`ensure_template` 按来源寻址）。
pub(super) fn template_dir_name(fixture: &crate::recipe::FixtureSpec) -> String {
    let [x, y, w, h] = fixture.rect;
    format!("{}-r{x}_{y}_{w}_{h}-p{}", fixture.source, crate::recipe::FIXTURE_TEMPLATE_PAD_PX)
}

/// 临时模板文件放了这么久还在，就是写到一半的残留（正常写入随即改名）。
const STALE_TMP: std::time::Duration = std::time::Duration::from_secs(600);

/// 清理 `<候选目录>/fixtures/`：删掉不再对应候选（含待生效版本）任何定位框的模板目录，以及放了很久的临时文件。
/// 模板按需从冻结图重裁，误删只多裁一次；删不掉（文件正被 lyFlow 打开）就留到下次保存。G2 只在删除工作区时清理。
pub(super) fn prune_fixture_dirs(dir: &Path, w: &Workspace) {
    let Ok(entries) = std::fs::read_dir(dir.join("fixtures")) else { return };
    let docs = std::iter::once(&w.doc).chain(w.pending.as_ref().map(|p| &p.doc));
    let keep: std::collections::HashSet<String> = docs.flat_map(|doc| &doc.shots)
        .flat_map(|s| s.fixture.iter().chain(s.views.iter().filter_map(|v| v.fixture.as_ref())))
        .map(template_dir_name).collect();
    for entry in entries.flatten() {
        let path = entry.path();
        if !keep.contains(&*entry.file_name().to_string_lossy()) {
            let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
            continue;
        }
        for file in std::fs::read_dir(&path).into_iter().flatten().flatten() {
            let stale = file.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > STALE_TMP));
            if stale && file.file_name().to_string_lossy().contains(".tmp-") { let _ = std::fs::remove_file(file.path()); }
        }
    }
}
