use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

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
    pub cameras: Result<(), String>,
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

pub fn evaluate(f: Facts) -> Readiness {
    let camera = match &f.cameras {
        Ok(()) => item(ItemKey::Camera, "相机", ItemState::Ok, "相机就绪", None, None),
        Err(reason) => item(ItemKey::Camera, "相机", ItemState::Blocked, reason.clone(), None, Some(Target::Camera)),
    };
    let items = vec![plc_item(&f), camera, recipe_item(&f), engine_item(&f), audit_item(&f), site_item(&f)];
    Readiness { ready: items.iter().all(|i| i.state == ItemState::Ok), items, recipes: f.recipes }
}

/// 样本集按拍照点编号、相机编号与视角取图：配方的每个拍照点都要在样本集里有同编号、同相机、视角齐全的图。
pub fn sample_set_mismatch(recipe: &Recipe, manifest: &Manifest) -> Option<String> {
    let missing: Vec<&str> = recipe.shots.iter().filter(|s| !manifest.shots.iter().any(|m| m.shot_id == s.id)).map(|s| s.id.as_str()).collect();
    if !missing.is_empty() {
        return Some(format!("配方 {} 个拍照点，样本集 {} 个；拍照点 {} 不在样本集中", recipe.shot_count(), manifest.shots.len(), missing.join("、")));
    }
    for shot in &recipe.shots {
        let sample = manifest.shots.iter().find(|m| m.shot_id == shot.id)?;
        if sample.camera_id != shot.camera {
            return Some(format!("拍照点 {} 用相机 {}，样本集里是 {}", shot.id, shot.camera, sample.camera_id));
        }
        let needed = shot.enabled_views().into_iter().chain([shot.view]).max().unwrap_or(1);
        if let Some(view) = (1..=needed).find(|v| !sample.views.contains(v)) {
            return Some(format!("拍照点 {} 需要视角 {view}，样本集只有视角 {:?}", shot.id, sample.views));
        }
    }
    None
}

static VERIFIED_BUNDLES: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());

/// 发布包按配方修订不可变，核对通过一次就记住这一修订。
fn bundle_ready(app: &AppHandle, recipe: &Recipe) -> Result<(), String> {
    let key = (recipe.id.clone(), recipe.revision_id.clone());
    if VERIFIED_BUNDLES.lock().unwrap().contains(&key) { return Ok(()); }
    crate::workspace::published_bundle(app, recipe)?;
    VERIFIED_BUNDLES.lock().unwrap().insert(key);
    Ok(())
}

fn recipe_problem(app: &AppHandle, host: &CycleHost, recipe: &Recipe, image: bool, set: Option<&SampleSet>) -> Result<Vec<u8>, (String, Target)> {
    let workspace = |e: String| (e, Target::Workspace);
    recipe.ready().map_err(workspace)?;
    if image { bundle_ready(app, recipe).map_err(workspace)?; }
    if let Some(reason) = set.and_then(|set| sample_set_mismatch(recipe, set.manifest())) {
        return Err((format!("与样本集不符：{reason}"), Target::Workspace));
    }
    let cams = usable_cams(app, recipe).map_err(|e| (e, Target::Camera))?;
    let device = crate::plc::sole_device_id(&host.camera.configs()).map_err(|e| (e, Target::Camera))?;
    crate::plc_plan::PlcPlan::from_recipe(recipe, device).map_err(workspace)?;
    Ok(cams)
}

fn option(recipe: &Recipe, problem: Option<(String, Target)>) -> RecipeOption {
    let (reason, target) = problem.map_or((None, None), |(reason, target)| (Some(reason), Some(target)));
    RecipeOption {
        id: recipe.id.clone(), name: recipe.name.clone(), version: recipe.version, product_code: recipe.product_code,
        shot_count: recipe.shot_count(), published: true, runnable: reason.is_none(), reason, target,
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
        Ok(cams) => (option(recipe, None), Some(cams)),
        Err(problem) => (option(recipe, Some(problem)), None),
    }).collect();
    let mut recipes: Vec<RecipeOption> = checked.iter().map(|(o, _)| o.clone()).collect();
    for draft in crate::workspace::workspace_list(app.state()) {
        if library.iter().any(|r| r.id == draft.doc.id) { continue; }
        recipes.push(RecipeOption {
            id: draft.doc.id.clone(), name: draft.doc.name.clone(), version: draft.doc.version, product_code: draft.doc.product_code,
            shot_count: draft.doc.shots.len(), published: false, runnable: false,
            reason: Some("未发布：在配方工作台验证并发布后才能运行".into()), target: Some(Target::Workspace),
        });
    }
    let product = match settings.product_source {
        ProductSource::Plc => Product::Plc,
        ProductSource::Manual => Product::Manual(settings.manual_recipe_id.clone()),
    };
    let relevant: Vec<u8> = match &product {
        Product::Manual(Some(id)) => checked.iter().find(|(o, _)| &o.id == id).and_then(|(_, c)| c.clone()).unwrap_or_default(),
        _ => checked.iter().filter_map(|(_, c)| c.clone()).flatten().collect::<BTreeSet<u8>>().into_iter().collect(),
    };
    let cams = if relevant.is_empty() { (0..host.camera.slots().len() as u8).collect() } else { relevant };
    let cameras = if cams.is_empty() { Err("相机组里没有相机".to_string()) } else { host.camera.check_ready_at(&cams) };
    let engine = image.then(|| {
        let status = app.state::<crate::vision::VisionHost>().status(settings.lyflow_core.as_deref());
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
