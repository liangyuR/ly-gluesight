//! 工作台的特征定位接线（定稿 docs/discussions/fixture-position/final.md §5、§13）：示教图检查进试测、
//! 画框时的只读检查、其他帧上的定位试测、开转角的显式确认。门限与统计在 `vision::fixture_quality`。

use super::*;
use crate::recipe::Clearance;
use crate::vision::fixture_quality::{self, FixtureCheck, FixtureTeachReport, TeachInput};
use crate::vision::PoseRecord;

/// 开转角的独立证据（定稿 §10 决定二条件 2），三选一。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RotationBasis {
    /// 同一视图两个不同的模板估出的 θ 一致
    TwoTemplates,
    /// 验证帧上 ±3° 的 |Δd′| 明显低于只找平移
    LowerDeviation,
    /// 多件采集时实物确认转过
    Physical,
}

/// 示教页“开启转角”的人工确认：只记进应用日志，不进配方。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationConfirmation {
    pub basis: RotationBasis,
    pub note: String,
}

/// 位置公差 min(上公差, 下公差)（mm）；没设位置限值时为空。
pub(super) fn position_tolerance(limits: &crate::recipe::ShotLimits) -> Option<f64> {
    limits.position.as_ref().map(|p| f64::from(p.tol_upper.min(p.tol_lower)))
}

/// 示教图检查：r 是按视图投影过的配方（fixture 是这个视图的），`clearances` 是本框到同组各走廊的余量。没有特征定位时为空。
pub(super) fn teach_geometry(r: &Recipe, k: usize, image: &FrameImage, clearances: &[Clearance]) -> Option<FixtureCheck> {
    let shot = r.shots.get(k)?;
    let fixture = shot.fixture.as_ref()?;
    Some(fixture_quality::geometry_checks(&TeachInput {
        fixture, image, clearances, path: &shot.path, mm_per_px: shot.mm_per_px.map(f64::from),
        tolerance_mm: position_tolerance(shot.limits.as_ref().unwrap_or(&r.limits)),
    }))
}

/// 示教图检查报告（几何、纹理加示教图上的定位验收），存进试测记录。
pub(super) fn teach_report(r: &Recipe, k: usize, image: &FrameImage, clearances: &[Clearance], pose: Option<&PoseRecord>) -> Option<FixtureTeachReport> {
    let fixture = r.shots.get(k)?.fixture.as_ref()?;
    Some(fixture_quality::teach_acceptance(teach_geometry(r, k, image, clearances)?, pose, fixture.angle_range_deg, fixture.max_peak_ratio))
}

/// 本框（拍照点 k 图 view）到同组各走廊的余量。
pub(super) fn clearances_of(doc: &RecipeDoc, k: usize, view: u8) -> Result<Vec<Clearance>, String> {
    Ok(doc.fixture_clearances()?.into_iter().filter(|c| c.k == k && c.view == view).collect())
}

/// 把示教页的草稿套到候选副本上（与 `apply_draft` 同口径：来源按 `bind_fixture_sources` 定），不做配方校验：
/// 压走廊的框要在检查结果里显示出来，而不是让校验报错。
fn draft_doc(w: &Workspace, k: usize, params: workflow::DraftTeach) -> Result<RecipeDoc, String> {
    let mut doc = w.doc.clone();
    let shot = doc.shots.get_mut(k).ok_or("拍照点不存在")?;
    shot.path = params.path; shot.mm_per_px = params.mm_per_px; shot.detect = params.detect; shot.limits = params.limits; shot.fixture = params.fixture;
    let selected = shot.view;
    if let Some(v) = shot.views.iter_mut().find(|v| v.view == selected) {
        v.path = shot.path.clone(); v.mm_per_px = shot.mm_per_px; v.detect = shot.detect.clone(); v.limits = shot.limits.clone(); v.fixture = shot.fixture.clone();
    }
    bind_fixture_sources(w, &mut doc, None)?;
    Ok(doc)
}

/// 画框时的只读检查：外圈在图内、走廊、纹理、亮像素与杠杆臂，不跑引擎、不改候选。前端通过后才把框写进草稿，
/// 免得压走廊的框让草稿自动保存反复撞上配方校验。
#[tauri::command]
pub async fn workspace_fixture_check(app: AppHandle, id: String, revision: u64, k: usize, params: workflow::DraftTeach) -> Result<FixtureCheck, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let (doc, frozen, view) = {
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选不存在")?;
            w.expect(revision)?;
            if params.fixture.is_none() { return Err("没有定位框".into()); }
            let view = w.doc.shots.get(k).ok_or("拍照点不存在")?.view;
            let frozen = w.frames.get(k).and_then(|f| frozen_view(f, view)).cloned().ok_or("含特征定位的图需要先冻结示教原图")?;
            (draft_doc(w, k, params)?, frozen, view)
        };
        let image = crate::image_file::load(&image_file(&host, &id, &frozen.id)?)?;
        let shot = doc.shots[k].for_view(view)?;
        let fixture = shot.fixture.as_ref().ok_or("没有定位框")?;
        let clearances = clearances_of(&doc, k, view)?;
        Ok(fixture_quality::geometry_checks(&TeachInput {
            fixture, image: &image, clearances: &clearances, path: &shot.path, mm_per_px: shot.mm_per_px.map(f64::from),
            tolerance_mm: position_tolerance(shot.limits.as_ref().unwrap_or(&doc.limits)),
        }))
    }).await.map_err(|e| e.to_string())?
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialStation {
    pub id: u32,
    pub status: &'static str,
    pub d: Option<f32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureTrialRow {
    pub label: String,
    /// "teach"：示教图本身（对照）；"sample"：样本库里同一拍照点、同一幅图
    pub source: &'static str,
    pub sample_id: Option<String>,
    pub pose: Option<PoseRecord>,
    pub stations: Vec<TrialStation>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureTrial {
    pub revision: u64,
    pub k: usize,
    pub view: u8,
    pub rows: Vec<FixtureTrialRow>,
}

fn trial_row(label: String, source: &'static str, sample_id: Option<String>, result: Result<crate::vision::ShotMeasurement, String>) -> FixtureTrialRow {
    match result {
        Ok(m) => FixtureTrialRow { label, source, sample_id, pose: m.pose.clone(), error: None, stations: m.idx.iter().enumerate().map(|(i, &id)| TrialStation {
            id,
            status: match m.st[i] { crate::measure::ST_OK => "ok", crate::measure::ST_NO_REF => "no_ref", crate::measure::ST_OUT_OF_RANGE => "out_of_range", _ => "no_bead" },
            d: (m.st[i] == crate::measure::ST_OK).then_some(m.d[i]),
        }).collect() },
        Err(e) => FixtureTrialRow { label, source, sample_id, pose: None, stations: Vec::new(), error: Some(e) },
    }
}

/// 定位试测（定稿 §5“示教”、D4）：用已保存的定位框，在示教图（对照）与样本库里同一拍照点、同一幅图的原图上跑定位与胶偏。
/// 只作诊断，不落盘；锁内取快照、锁外计算，结束时修订号变了就丢弃。示教图上定位没被采用时，这次试测本身无效。
#[tauri::command]
pub async fn workspace_fixture_trial(app: AppHandle, id: String, revision: u64, k: usize) -> Result<FixtureTrial, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let (w, recipe, view, frozen, samples) = {
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选不存在")?;
            w.expect(revision)?;
            let recipe = w.doc.build()?;
            let view = recipe.shots.get(k).ok_or("拍照点不存在")?.view;
            let projected = recipe.for_view(k, view)?;
            let fixture = projected.shots[k].fixture.clone().ok_or("这幅图没有保存定位框；先画框并运行试测")?;
            let frozen = fixture_source(&w.frames[k], view, &fixture)?.clone();
            let single = workflow::selected_views(w, k).len() == 1;
            let dir = host.dir(&id).join("samples");
            let samples: Vec<(String, String, PathBuf)> = w.sample_bank.iter().filter_map(|b| {
                let path = dir.join(&b.id).join(format!("k{k}-v{view}.pgm"));
                let path = if path.is_file() { path } else { Some(dir.join(&b.id).join(format!("k{k}.pgm"))).filter(|p| single && p.is_file())? };
                Some((b.id.clone(), b.name.clone(), path))
            }).collect();
            (w.clone(), recipe, view, frozen, samples)
        };
        let projected = recipe.for_view(k, view)?;
        let engine = app.state::<vision::VisionHost>().engine().ok_or("lyFlow 核心库未加载")?;
        let teach = crate::image_file::load(&image_file(&host, &id, &frozen.id)?)?;
        let base = fixture_bases(&host, &w, &recipe, k, &[(view, &teach)])?.remove(&view).ok_or("这幅图没有定位模板")?;
        let run = vision::unique_run_id(&format!("fixture-trial-{}-{k}", recipe.revision_id));
        let mut rows = vec![trial_row(format!("示教图 {}", frozen.id), "teach", None, vision::measure_shot(&engine, &projected, k, &teach, &format!("{run}-teach"), &base))];
        for (i, (sample_id, name, path)) in samples.into_iter().enumerate() {
            let result = crate::image_file::load(&path).and_then(|image| {
                if [image.width, image.height] != frozen.size { return Err("样本尺寸与示教图不一致".into()); }
                vision::measure_shot(&engine, &projected, k, &image, &format!("{run}-{i}"), &base)
            });
            rows.push(trial_row(name, "sample", Some(sample_id), result));
        }
        let items = host.items.lock().unwrap();
        items.get(&id).ok_or("候选不存在")?.expect(revision).map_err(|_| "定位试测期间候选已变化，结果已丢弃".to_string())?;
        Ok(FixtureTrial { revision, k, view, rows })
    }).await.map_err(|e| e.to_string())?
}

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

/// 保存示教参数时开转角：转角调大必须带上确认（独立证据与说明），确认记进应用日志。
pub(super) fn confirm_rotation(old: &RecipeDoc, new: &RecipeDoc, rotation: Option<&RotationConfirmation>) -> Result<(), String> {
    if reject_unconfirmed_rotation(old, new).is_ok() { return Ok(()); }
    let confirmed = rotation.filter(|r| !r.note.trim().is_empty() && r.note.chars().count() <= 200 && !r.note.chars().any(char::is_control))
        .ok_or("开启转角需要在高级设置里选一种独立证据并填写说明（最长 200 个字符）")?;
    let changed: Vec<String> = angle_ranges(new).into_iter().filter(|(key, angle)| *angle > angle_ranges(old).get(key).copied().unwrap_or(0.0))
        .map(|((shot, view), angle)| format!("拍照点 {shot} 图 {view} ±{angle}°")).collect();
    crate::applog::write("warn", "特征定位开转角", &format!("配方 {}：{}；依据 {:?}：{}", new.id, changed.join("、"), confirmed.basis, confirmed.note.trim()));
    Ok(())
}

/// 验证时一件正常样本在一个特征定位视图上的结果（`measure_images_from` 收集，`validation` 按视图汇总）。
pub(super) struct ProbeEntry {
    pub k: usize,
    pub view: u8,
    pub sample: fixture_quality::FixtureSample,
}

/// 验证用的额外定位（定稿 §10）：同一张样本图、同一个模板目录上，按配方门限跑整图亮度 ±40，
/// 再用 ±3° 的投影配方跑原图与 ±40（转角证据）。`reading` 是这张图按配方的正式测量。
pub(super) fn probe_sample(engine: &vision::Engine, projected: &Recipe, k: usize, image: &FrameImage, base: &str, run: &str,
    reading: Option<&vision::ShotMeasurement>, error: Option<String>) -> fixture_quality::FixtureSample {
    use fixture_quality::{brightness, BRIGHTNESS_PROBE, ROTATION_PROBE_DEG};
    let locate = |recipe: &Recipe, img: &FrameImage, tag: &str| vision::measure_shot(engine, recipe, k, img, &format!("{run}-{tag}"), base).ok().and_then(|m| m.pose);
    let brighter_image = brightness(image, BRIGHTNESS_PROBE);
    let darker_image = brightness(image, -BRIGHTNESS_PROBE);
    let mut rotated = projected.clone();
    if let Some(f) = rotated.shots[k].fixture.as_mut() { f.angle_range_deg = f.angle_range_deg.max(ROTATION_PROBE_DEG); }
    fixture_quality::FixtureSample {
        name: String::new(),
        pose: reading.and_then(|r| r.pose.clone()),
        deviations_mm: reading.map(|r| r.st.iter().zip(&r.d).filter(|(st, _)| **st == crate::measure::ST_OK).map(|(_, d)| *d).collect()).unwrap_or_default(),
        brighter: locate(projected, &brighter_image, "bright"),
        darker: locate(projected, &darker_image, "dark"),
        rotation: vec![locate(&rotated, image, "rot"), locate(&rotated, &brighter_image, "rot-bright"), locate(&rotated, &darker_image, "rot-dark")],
        error,
    }
}

/// 候选里每个启用的特征定位视图的验证背景（门限、公差、杠杆臂、站表）。
pub(super) fn view_contexts(recipe: &Recipe) -> Result<Vec<fixture_quality::ViewContext>, String> {
    let mut out = Vec::new();
    for (k, shot) in recipe.shots.iter().enumerate().filter(|(_, s)| s.measured()) {
        for view in shot.enabled_views() {
            let projected = recipe.for_view(k, view)?;
            let Some(fixture) = projected.shots[k].fixture.clone() else { continue };
            let limits = projected.shots[k].limits.as_ref().unwrap_or(&projected.limits);
            let position = limits.position.as_ref().ok_or_else(|| format!("拍照点 {} 图 {view} 使用了特征定位，必须设置位置判定限值", shot.id))?;
            let stations: Vec<[f32; 2]> = recipe.view_points(k, view).map(|j| [recipe.points.x[j], recipe.points.y[j]]).collect();
            let mm = projected.shots[k].mm_per_px.map(f64::from);
            let lever = fixture_quality::lever(&fixture, &stations, mm).and_then(|l| l.mm).unwrap_or(0.0);
            out.push(fixture_quality::ViewContext { k, view, shot_id: shot.id.clone(), tolerance_mm: f64::from(position.tol_upper.min(position.tol_lower)),
                nominal_mm: f64::from(position.nominal), lever_mm: lever, spacing_mm: f64::from(recipe.spacing), stations: stations.len(), fixture });
        }
    }
    Ok(out)
}

/// 按视图汇总验证统计，并给出“特征定位”这一项发布前检查；候选没有特征定位时为空。
pub(super) fn validation_reports(recipe: &Recipe, probes: Vec<ProbeEntry>) -> Result<(Vec<fixture_quality::FixtureViewReport>, Option<Check>), String> {
    let contexts = view_contexts(recipe)?;
    if contexts.is_empty() { return Ok((Vec::new(), None)); }
    let reports: Vec<_> = contexts.iter().map(|ctx| {
        let samples: Vec<_> = probes.iter().filter(|p| p.k == ctx.k && p.view == ctx.view).map(|p| p.sample.clone()).collect();
        fixture_quality::validation_report(ctx, &samples)
    }).collect();
    let failed = reports.iter().find_map(|r| fixture_quality::first_block(&r.checks).map(|c| format!("拍照点 {} 图 {}：{}", r.shot_id, r.view, c.detail)));
    let warnings = reports.iter().flat_map(|r| &r.checks).filter(|c| c.level == fixture_quality::Level::Warn && !c.passed).count();
    let check = Check { name: "特征定位".into(), passed: failed.is_none(), detail: failed.unwrap_or_else(|| format!(
        "{} 个特征定位视图在独立正常样本上全部定位成功、σ 合格、亮度 ±40 没有错锁{}", reports.len(),
        if warnings > 0 { format!("；另有 {warnings} 条告警，见特征定位面板") } else { String::new() })) };
    Ok((reports, Some(check)))
}

/// 发布门（G3 留下、G4 落地）：每个启用的特征定位视图都要在这份验证里有通过的统计（含亮度 ±40）。
/// 挡住 G4 之前的验证记录：它们修订号可能还对得上，却没有这些统计。
pub(super) fn require_fixture_validation(recipe: &Recipe, validated: &Validation) -> Result<(), String> {
    for ctx in view_contexts(recipe)? {
        let passed = validated.fixture.iter().any(|r| r.k == ctx.k && r.view == ctx.view && r.passed);
        if !passed {
            return Err(format!("拍照点 {} 图 {} 的特征定位还没有通过验证统计（全部定位成功、σ、亮度 ±40），请重新验证", ctx.shot_id, ctx.view));
        }
    }
    Ok(())
}

/// 采用验证建议（决定一）：只从同一修订的验证统计取数，写进该视图的 maxEndShiftMm 与示教余量 m。
/// 改 fixture 就是改测量定义：该视图的试测随之作废，修订号前进、整套要重新验证。模板来源不动（不是确认框的入口）。
pub(super) fn apply_fixture_suggestion(w: &mut Workspace, revision: u64, k: usize, view: u8) -> Result<(), String> {
    w.expect(revision)?;
    let report = w.validation.as_ref().filter(|v| v.revision == revision)
        .ok_or("验证结果不是当前修订的，请重新验证后再采用建议")?
        .fixture.iter().find(|r| r.k == k && r.view == view).ok_or("这幅图没有特征定位验证统计")?;
    let suggestion = report.suggestion.clone().filter(|s| s.applicable).ok_or("这幅图没有可采用的两端参数建议")?;
    let mut doc = w.doc.clone();
    let shot = doc.shots.get_mut(k).ok_or("拍照点不存在")?;
    let apply = |f: &mut Option<crate::recipe::FixtureSpec>| -> bool {
        let Some(f) = f.as_mut() else { return false };
        f.max_end_shift_mm = Some(suggestion.max_end_shift_mm);
        f.end_margin_mm = suggestion.end_margin_mm;
        true
    };
    let mut applied = shot.view == view && apply(&mut shot.fixture);
    if let Some(v) = shot.views.iter_mut().find(|v| v.view == view) { applied |= apply(&mut v.fixture); }
    if !applied { return Err("这幅图没有定位框".into()); }
    let recipe = doc.build()?;
    crate::vision::build_taught_graph(&recipe.for_view(k, view)?, k)?;
    let shot_id = doc.shots[k].id.clone();
    w.doc = doc;
    let frame = w.frames.get_mut(k).ok_or("拍照点不存在")?;
    if let Some(state) = frame.view_states.iter_mut().find(|s| s.view == view) { state.trial = None; state.saved = false; }
    if w.doc.shots[k].view == view { frame.trial = None; frame.saved = false; }
    w.changed();
    crate::applog::write("info", "特征定位采用建议", &format!("配方 {} 拍照点 {shot_id} 图 {view}：maxEndShiftMm = {}，示教余量 m = {} mm（两端 |Δ| P99 {:.2} mm）",
        w.doc.id, suggestion.max_end_shift_mm, suggestion.end_margin_mm, suggestion.p99_mm));
    Ok(())
}

#[tauri::command]
pub fn workspace_apply_fixture_suggestion(app: AppHandle, id: String, revision: u64, k: usize, view: u8) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone();
    apply_fixture_suggestion(&mut w, revision, k, view)?;
    host.save(&w)?;
    items.insert(id, w.clone());
    super::view(&app, w)
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
