use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use ly_plc::LinkState;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::{host, plc, usable_cams, CycleHost, Phase};
use crate::offline::sample_set::{Manifest, SampleSet};
use crate::plc_session::{SessionPhase, NOTICE};
use crate::recipe::Recipe;
use crate::settings::{ProductSource, RecordMode};
use crate::site::SourceKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemKey { Plc, Camera, Recipe, Engine, Audit, Site }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemState { Ok, Waiting, Blocked }

/// 处理该项要去的页面。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Target { Plc, Camera, Workspace, Settings, Offline }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessItem {
    pub key: ItemKey,
    pub title: &'static str,
    pub state: ItemState,
    pub detail: String,
    /// 操作的前提说明（如复位故障需要满足的条件）
    pub hint: Option<String>,
    pub target: Option<Target>,
}

/// 配方能否在当前站点（相机组、样本集、测量方式）开工。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeOption {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub product_code: u16,
    pub shot_count: usize,
    pub published: bool,
    pub runnable: bool,
    pub reason: Option<String>,
    pub target: Option<Target>,
    /// 可运行但仍需等待的原因（如生产预热中）
    pub waiting: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    pub ready: bool,
    pub items: Vec<ReadinessItem>,
    pub recipes: Vec<RecipeOption>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Site { Real, SampleSet(String), Synthetic }

#[derive(Clone, Debug, PartialEq)]
pub enum Product { Plc, Manual(Option<String>) }

#[derive(Clone, Debug, PartialEq)]
pub enum Check { Ok, Waiting(String), Blocked(String) }

pub struct Facts {
    pub site: Site,
    pub plc_connected: bool,
    pub session: Option<SessionPhase>,
    pub session_message: Option<String>,
    pub phase: Phase,
    pub fault: Option<String>,
    pub cameras: Check,
    /// lyFlow 加载结果（版本或原因）；模拟测量不需要时为 None
    pub engine: Option<Result<String, String>>,
    pub workers: Result<(), String>,
    pub audit: Check,
    pub record: RecordMode,
    pub product: Product,
    pub activation_uncertain: bool,
    pub recipes: Vec<RecipeOption>,
}

fn item(key: ItemKey, title: &'static str, state: ItemState, detail: impl Into<String>, hint: Option<String>, target: Option<Target>) -> ReadinessItem {
    ReadinessItem { key, title, state, detail: detail.into(), hint, target }
}

fn reset_hint(offline: bool) -> String {
    if offline {
        "离线模式由离线产线自动复位：进入离线和每次运行前都会由虚拟 PLC 发 faultReset；仍未复位时在离线控制台查看原因".into()
    } else {
        "复位前提：PLC 已连接且心跳在翻转、相机与追溯就绪、无在途工件。真机由 PLC 发 faultReset 复位，也可在 PLC 侧条件满足后点“复位故障”".into()
    }
}

fn plc_item(f: &Facts) -> ReadinessItem {
    let offline = f.site != Site::Real;
    let home = Some(if offline { Target::Offline } else { Target::Plc });
    const TITLE: &str = "PLC 连接与会话";
    if !f.plc_connected {
        let detail = if offline { "离线 S7 服务尚未连接" } else { "PLC 未连接" };
        return item(ItemKey::Plc, TITLE, ItemState::Blocked, detail, None, home);
    }
    let pending = f.session == Some(SessionPhase::ResetRequired);
    if pending || f.session == Some(SessionPhase::Fault) || f.phase == Phase::Fault {
        let reason = f.fault.clone().or_else(|| f.session_message.clone()).unwrap_or_else(|| "S7 会话需要明确复位".into());
        let detail = if pending { format!("待复位：{reason}") } else { reason };
        return item(ItemKey::Plc, TITLE, ItemState::Blocked, detail, Some(reset_hint(offline)), home);
    }
    if let Some(notice) = f.session_message.as_deref().filter(|m| m.starts_with(NOTICE)) {
        return item(ItemKey::Plc, TITLE, ItemState::Waiting, notice.trim_start_matches(NOTICE), None, home);
    }
    let detail = if f.phase == Phase::Idle { "已连接，会话空闲，视觉就绪" } else { "已连接，检测进行中" };
    item(ItemKey::Plc, TITLE, ItemState::Ok, detail, None, None)
}

pub const MANUAL_ONLY: &str = "人工选型号模式下只能运行所选配方";

fn recipe_item(f: &Facts) -> ReadinessItem {
    const TITLE: &str = "可开工配方";
    const CREATE: &str = "去配方工作台新建并发布配方；整圈采集不受影响";
    let blocked = |detail: String, hint: Option<String>, target| item(ItemKey::Recipe, TITLE, ItemState::Blocked, detail, hint, target);
    if f.activation_uncertain {
        return blocked("生产生效版本尚未确认".into(), Some("在配方工作台核实发布状态后再开工".into()), Some(Target::Workspace));
    }
    let runnable: Vec<&RecipeOption> = f.recipes.iter().filter(|r| r.runnable).collect();
    match &f.product {
        Product::Manual(None) => blocked("人工选型号：尚未选择检测配方".into(), Some("在在线检测页顶部选择一个可开工配方".into()), None),
        Product::Manual(Some(id)) => match f.recipes.iter().find(|r| &r.id == id) {
            None => blocked(format!("选中的配方 {id} 不在配方库中"), None, Some(Target::Workspace)),
            Some(r) if !r.runnable => blocked(format!("配方 {id} 不能开工：{}", r.reason.as_deref().unwrap_or("原因未知")), Some(CREATE.into()), r.target),
            Some(RecipeOption { waiting: Some(wait), .. }) => item(ItemKey::Recipe, TITLE, ItemState::Waiting, format!("配方 {id}：{wait}"), None, None),
            Some(r) => item(ItemKey::Recipe, TITLE, ItemState::Ok, format!("{} · v{}", r.id, r.version), None, None),
        },
        Product::Plc if runnable.is_empty() => {
            let first = f.recipes.iter().find(|r| !r.runnable);
            let detail = match first {
                None => "尚无可开工的生产配方：配方库为空".to_string(),
                Some(r) => format!("尚无可开工的生产配方（{}：{}）", r.id, r.reason.as_deref().unwrap_or("原因未知")),
            };
            blocked(detail, Some(CREATE.into()), first.and_then(|r| r.target).or(Some(Target::Workspace)))
        }
        Product::Plc if runnable.iter().all(|r| r.waiting.is_some()) => item(ItemKey::Recipe, TITLE, ItemState::Waiting,
            runnable.iter().map(|r| format!("{}：{}", r.id, r.waiting.as_deref().unwrap_or_default())).collect::<Vec<_>>().join("；"), None, None),
        Product::Plc => item(ItemKey::Recipe, TITLE, ItemState::Ok,
            format!("{} 个可开工配方，按 PLC 产品代码匹配：{}", runnable.len(), runnable.iter().map(|r| r.id.as_str()).collect::<Vec<_>>().join("、")), None, None),
    }
}

fn engine_item(f: &Facts) -> ReadinessItem {
    const TITLE: &str = "图像测量";
    if let Err(reason) = &f.workers {
        return item(ItemKey::Engine, TITLE, ItemState::Blocked, reason.clone(), Some("排查图像引擎或重启检测服务".into()), None);
    }
    match &f.engine {
        None => item(ItemKey::Engine, TITLE, ItemState::Ok, "合成图使用模拟测量，不需要 lyFlow", None, None),
        Some(Ok(version)) => item(ItemKey::Engine, TITLE, ItemState::Ok, format!("lyFlow {version} 已加载"), None, None),
        Some(Err(reason)) => item(ItemKey::Engine, TITLE, ItemState::Blocked, format!("lyFlow 未加载：{reason}"), None, Some(Target::Settings)),
    }
}

fn audit_item(f: &Facts) -> ReadinessItem {
    const TITLE: &str = "审计与录制";
    let record = match f.record { RecordMode::Off => "关闭", RecordMode::Failed => "只留 NG / ERR 件", RecordMode::All => "全部" };
    let scope = if f.site == Site::Real { "" } else { "（离线临时）" };
    match &f.audit {
        Check::Ok => item(ItemKey::Audit, TITLE, ItemState::Ok, format!("追溯可写 · 帧录制{scope}：{record}"), None, None),
        Check::Waiting(reason) => item(ItemKey::Audit, TITLE, ItemState::Waiting, reason.clone(), None, None),
        Check::Blocked(reason) => item(ItemKey::Audit, TITLE, ItemState::Blocked, reason.clone(), Some("排查记录库与磁盘后重启检测服务".into()), Some(Target::Settings)),
    }
}

fn site_item(f: &Facts) -> ReadinessItem {
    let detail = match &f.site {
        Site::Real => "真机".to_string(),
        Site::SampleSet(name) => format!("离线 · 样本集 {name}（lyFlow 测量）"),
        Site::Synthetic => "离线 · 合成图（模拟测量）".to_string(),
    };
    item(ItemKey::Site, "站点模式", ItemState::Ok, detail, None, Some(Target::Offline))
}

fn camera_item(f: &Facts) -> ReadinessItem {
    match &f.cameras {
        Check::Ok => item(ItemKey::Camera, "相机", ItemState::Ok, "相机就绪", None, None),
        Check::Waiting(reason) => item(ItemKey::Camera, "相机", ItemState::Waiting, reason.clone(), None, Some(Target::Camera)),
        Check::Blocked(reason) => item(ItemKey::Camera, "相机", ItemState::Blocked, reason.clone(), None, Some(Target::Camera)),
    }
}

/// 各可运行配方的相机是否全部就绪：至少一个配方的相机全部就绪即可开工，其余配方掉线只告警。
pub fn camera_check(groups: &[(String, Result<(), String>)]) -> Check {
    let down: Vec<(&String, &String)> = groups.iter().filter_map(|(id, r)| r.as_ref().err().map(|e| (id, e))).collect();
    match down.first() {
        None => Check::Ok,
        Some((_, reason)) if down.len() == groups.len() => Check::Blocked((*reason).clone()),
        Some(_) => Check::Waiting(down.iter().map(|(id, e)| format!("配方 {id} 暂时开不了工：{e}")).collect::<Vec<_>>().join("；")),
    }
}

pub fn evaluate(mut f: Facts) -> Readiness {
    if let Product::Manual(selected) = &f.product {
        for r in f.recipes.iter_mut().filter(|r| r.runnable && Some(&r.id) != selected.as_ref()) {
            r.runnable = false;
            r.reason = Some(MANUAL_ONLY.into());
            r.target = None;
            r.waiting = None;
        }
    }
    let items = vec![plc_item(&f), camera_item(&f), recipe_item(&f), engine_item(&f), audit_item(&f), site_item(&f)];
    Readiness { ready: items.iter().all(|i| i.state != ItemState::Blocked), items, recipes: f.recipes }
}

/// 样本集按拍照点编号、相机编号与视角取图：配方的每个拍照点都要在样本集里有同编号、同相机的图，
/// 且提供该相机配置的全部视角（样本集相机每次取 1..=视角数）。
pub fn sample_set_mismatch(recipe: &Recipe, manifest: &Manifest, view_count: impl Fn(&str) -> Option<u8>) -> Option<String> {
    let missing: Vec<&str> = recipe.shots.iter().filter(|s| !manifest.shots.iter().any(|m| m.shot_id == s.id)).map(|s| s.id.as_str()).collect();
    if !missing.is_empty() {
        return Some(format!("配方 {} 个拍照点，样本集 {} 个；拍照点 {} 不在样本集中", recipe.shot_count(), manifest.shots.len(), missing.join("、")));
    }
    for shot in &recipe.shots {
        let sample = manifest.shots.iter().find(|m| m.shot_id == shot.id)?;
        if sample.camera_id != shot.camera {
            return Some(format!("拍照点 {} 用相机 {}，样本集里是 {}", shot.id, shot.camera, sample.camera_id));
        }
        let needed = view_count(&shot.camera).unwrap_or(1);
        if let Some(view) = (1..=needed).find(|v| !sample.views.contains(v)) {
            return Some(format!("拍照点 {} 需要视角 {view}，样本集只有视角 {:?}", shot.id, sample.views));
        }
    }
    None
}

const FAILED_CHECK_TTL: Duration = Duration::from_secs(5);

/// 发布包核对：通过的结果不缓存（包被删或损坏后下一次就能发现），失败结果到期前不重查。
#[derive(Default)]
pub struct BundleChecks(Mutex<HashMap<(String, String), (String, Instant)>>);

impl BundleChecks {
    pub fn check<T>(&self, key: (String, String), now: Instant, run: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        if let Some((error, expires)) = self.0.lock().unwrap().get(&key) {
            if now < *expires { return Err(error.clone()); }
        }
        let result = run();
        let mut failed = self.0.lock().unwrap();
        match &result {
            Ok(_) => { failed.remove(&key); }
            Err(error) => { failed.insert(key, (error.clone(), now + FAILED_CHECK_TTL)); }
        }
        result
    }
}

static BUNDLE_CHECKS: LazyLock<BundleChecks> = LazyLock::new(BundleChecks::default);

/// 已发布版本能否在当前站点运行，以及生产预热进度（只读查询，不触发预热）。
fn production_state(app: &AppHandle, recipe: &Recipe) -> Result<Option<String>, (String, Target)> {
    use crate::production::Warmup;
    let workspace = |e: String| (e, Target::Workspace);
    let key = (recipe.id.clone(), recipe.revision_id.clone());
    let manifest = BUNDLE_CHECKS.check(key, Instant::now(), || crate::workspace::published_manifest(app, recipe)).map_err(workspace)?;
    crate::production::site_admits(manifest.validation_site, crate::offline::enabled(app)).map_err(workspace)?;
    match crate::production::warmup(app, recipe) {
        Warmup::Ready => Ok(None),
        Warmup::Pending => Ok(Some("等待生产预热".into())),
        Warmup::Preparing => Ok(Some("发布资源与图像引擎预热中".into())),
        Warmup::Retryable(error) => Err((error, Target::Settings)),
        Warmup::Failed(error) => Err((format!("生产预热失败：{error}"), Target::Workspace)),
    }
}

fn recipe_problem(app: &AppHandle, host: &CycleHost, recipe: &Recipe, image: bool, set: Option<&SampleSet>) -> Result<(Vec<u8>, Option<String>), (String, Target)> {
    let workspace = |e: String| (e, Target::Workspace);
    recipe.ready().map_err(workspace)?;
    let waiting = if image { production_state(app, recipe)? } else { None };
    let view_count = |id: &str| host.camera.require(id).ok().and_then(|c| host.camera.slot(c as usize)).map(|slot| slot.config().view_count);
    if let Some(reason) = set.and_then(|set| sample_set_mismatch(recipe, set.manifest(), view_count)) {
        return Err((format!("与样本集不符：{reason}"), Target::Workspace));
    }
    let cams = usable_cams(app, recipe).map_err(|e| (e, Target::Camera))?;
    crate::plc::recipe_plan(host, recipe).map_err(workspace)?;
    Ok((cams, waiting))
}

fn option(recipe: &Recipe, problem: Option<(String, Target)>, waiting: Option<String>) -> RecipeOption {
    let (reason, target) = problem.map_or((None, None), |(reason, target)| (Some(reason), Some(target)));
    RecipeOption {
        id: recipe.id.clone(), name: recipe.name.clone(), version: recipe.version, product_code: recipe.product_code,
        shot_count: recipe.shot_count(), published: true, runnable: reason.is_none(), reason, target, waiting,
    }
}

pub fn gather(app: &AppHandle) -> Readiness {
    let host = host(app);
    let settings = host.settings();
    let snapshot = host.shared.lock().unwrap().snapshot.clone();
    let set = crate::offline::sample_set(app);
    let site = match crate::offline::source_kind(app) {
        None => Site::Real,
        Some(SourceKind::SampleSet) => Site::SampleSet(set.as_ref().map(|s| s.manifest().name.clone()).unwrap_or_default()),
        Some(SourceKind::Synthetic) => Site::Synthetic,
    };
    let image = crate::site::image_measurement(app);
    let library: Vec<Arc<Recipe>> = host.recipes.list();
    let checked: Vec<(RecipeOption, Option<Vec<u8>>)> = library.iter().map(|recipe| match recipe_problem(app, host, recipe, image, set.as_deref()) {
        Ok((cams, waiting)) => (option(recipe, None, waiting), Some(cams)),
        Err(problem) => (option(recipe, Some(problem), None), None),
    }).collect();
    let mut recipes: Vec<RecipeOption> = checked.iter().map(|(o, _)| o.clone()).collect();
    let mut drafts = crate::workspace::draft_summaries(app);
    drafts.sort_by(|a, b| a.id.cmp(&b.id));
    for draft in drafts {
        if library.iter().any(|r| r.id == draft.id) { continue; }
        recipes.push(RecipeOption {
            id: draft.id, name: draft.name, version: draft.version, product_code: draft.product_code,
            shot_count: draft.shot_count, published: false, runnable: false,
            reason: Some("未发布：在配方工作台验证并发布后才能运行".into()), target: Some(Target::Workspace), waiting: None,
        });
    }
    let product = match settings.product_source {
        ProductSource::Plc => Product::Plc,
        ProductSource::Manual => Product::Manual(settings.manual_recipe_id.clone()),
    };
    let groups: Vec<(String, Result<(), String>)> = checked.iter()
        .filter(|(o, _)| match &product { Product::Manual(selected) => selected.as_ref() == Some(&o.id), Product::Plc => true })
        .filter_map(|(o, cams)| cams.as_ref().map(|cams| (o.id.clone(), host.camera.check_ready_at(cams))))
        .collect();
    let cameras = if !groups.is_empty() { camera_check(&groups) } else {
        let all: Vec<u8> = (0..host.camera.slots().len() as u8).collect();
        let checked = if all.is_empty() { Err("相机组里没有相机".to_string()) } else { host.camera.check_ready_at(&all) };
        checked.map_or_else(Check::Blocked, |()| Check::Ok)
    };
    let engine = image.then(|| {
        let status = app.state::<crate::vision::VisionHost>().status();
        if status.loaded { Ok(status.version.unwrap_or_default()) } else { Err(status.message) }
    });
    let workers = match snapshot.as_ref().map(|s| &s.measurement_workers) {
        Some(w) if w.capacity > 0 && w.timed_out >= w.capacity => Err(format!("测量工作线程全部超时未返回（{} / {}），拒绝布防", w.timed_out, w.capacity)),
        _ => Ok(()),
    };
    let audit = match host.audit.readiness() {
        Ok(crate::audit::AuditReadiness::Ready) => Check::Ok,
        Ok(crate::audit::AuditReadiness::WaitingRecording) => Check::Waiting("原图录制尚未完成耐久接受，完成后自动就绪".into()),
        Ok(crate::audit::AuditReadiness::WaitingStore) => Check::Waiting("持久追溯仍有待入库事件，入库后自动就绪".into()),
        Err(error) => Check::Blocked(format!("持久追溯故障：{error}")),
    };
    evaluate(Facts {
        site,
        plc_connected: plc(app).status().state == LinkState::Connected,
        session: snapshot.as_ref().and_then(|s| s.plc_handshake.as_ref()).map(|v| v.phase),
        session_message: snapshot.as_ref().and_then(|s| s.plc_handshake.as_ref()).and_then(|v| v.message.clone()),
        phase: snapshot.as_ref().map_or(Phase::Idle, |s| s.phase),
        fault: snapshot.as_ref().and_then(|s| s.fault.clone()),
        cameras,
        engine,
        workers,
        audit,
        record: crate::site::record_mode(app),
        product,
        activation_uncertain: crate::workspace::activation_uncertain(app),
        recipes,
    })
}

#[cfg(test)]
mod tests;
