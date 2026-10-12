//! 站点模式：真机 / 离线。软件启动永远是真机；进入离线时一次性派生临时运行环境
//! （PLC → 进程内 S7，相机组 → 按来源生成，测量 → lyFlow / 模拟，帧录制 → 保留全部），退出时整体恢复。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ly_plc::PlcConfig;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::camera::{Acquisition, CameraConfig, CameraSource};
use crate::cycle::CycleHost;
use crate::offline::sample_set::{Manifest, SampleSet, SampleSetSummary};
use crate::offline::{OfflineHost, SampleRun};
use crate::settings::RecordMode;
use crate::store::RecordSource;
use crate::vision::VisionHost;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    SampleSet,
    Synthetic,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OfflineSource {
    SampleSet { dir: String },
    Synthetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SiteMode {
    Real,
    Offline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Measurement {
    #[serde(rename = "lyFlow")]
    LyFlow,
    #[serde(rename = "simulated")]
    Simulated,
}

/// 上次的离线选择：来源与样本集目录（切到合成图也记住目录），便于一键再进入。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineChoice {
    pub kind: SourceKind,
    pub sample_set_dir: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteStatus {
    pub mode: SiteMode,
    pub source: Option<SourceKind>,
    pub last: Option<OfflineChoice>,
    pub measurement: Measurement,
    pub port: Option<u16>,
    pub running: bool,
    pub continuous: bool,
    pub parts: u32,
    pub message: String,
    pub heartbeat_stopped: bool,
    pub sample_set: Option<SampleSetSummary>,
    pub next_part: Option<String>,
    pub recent: Vec<SampleRun>,
}

/// 随站点模式切换的运行环境。
#[derive(Clone, Debug, PartialEq)]
pub struct Environment {
    pub plc: PlcConfig,
    pub cameras: Vec<CameraConfig>,
    pub record: RecordMode,
}

pub enum Derivation<'a> {
    SampleSet(&'a Manifest),
    Synthetic,
}

/// 切换模式前要满足的条件。
pub struct Preconditions {
    /// 有在途事务或检测未回到空闲 / 故障
    pub busy: bool,
    /// 示教整圈采集进行中
    pub capturing: bool,
    /// lyFlow 是否已加载（只有样本集需要）
    pub lyflow: Result<(), String>,
}

pub fn check_switch(busy: bool, capturing: bool) -> Result<(), String> {
    if busy { return Err("检测仍有在途事务，完成握手或复位故障后再切换站点模式".into()); }
    if capturing { return Err("示教整圈采集正在进行，结束本轮采集后再切换站点模式".into()); }
    Ok(())
}

/// 一次离线：保存进入前的真实环境，退出时原样交回。
pub struct OfflineSession {
    kind: SourceKind,
    real: Environment,
    applied: Environment,
}

impl OfflineSession {
    pub fn enter(real: Environment, derivation: Derivation, port: u16, pre: Preconditions) -> Result<Self, String> {
        check_switch(pre.busy, pre.capturing)?;
        let (kind, cameras) = match derivation {
            Derivation::SampleSet(manifest) => {
                pre.lyflow.map_err(|reason| format!("离线·样本集要用 lyFlow 测量现场原图，但 lyFlow 未加载：{reason}"))?;
                (SourceKind::SampleSet, sample_set_cameras(manifest)?)
            }
            Derivation::Synthetic => (SourceKind::Synthetic, synthetic_cameras(&real.cameras)),
        };
        let applied = Environment { plc: offline_plc(port)?, cameras, record: RecordMode::All };
        Ok(Self { kind, real, applied })
    }

    pub fn kind(&self) -> SourceKind { self.kind }

    pub fn applied(&self) -> &Environment { &self.applied }

    pub fn leave(self) -> Environment { self.real }
}

fn offline_plc(port: u16) -> Result<PlcConfig, String> {
    let mut config = crate::inspection::default_plc_config();
    config.connection.host = "127.0.0.1".into();
    config.connection.port = port;
    config.auto_connect = false;
    crate::plc::validate_config(&config)?;
    Ok(config)
}

fn sample_set_cameras(manifest: &Manifest) -> Result<Vec<CameraConfig>, String> {
    manifest.cameras.iter().map(|camera| {
        let mut views = camera.views.clone();
        views.sort_unstable();
        let view_count = match views.as_slice() {
            [1] => 1,
            [1, 2, 3] => 3,
            _ => return Err(format!("样本集相机 {} 的视角 {:?} 无法生成离线相机：只支持单视角 [1] 或三视角 [1, 2, 3]", camera.id, camera.views)),
        };
        Ok(CameraConfig {
            id: camera.id.clone(), name: format!("样本集 {}", camera.id), source: CameraSource::SampleSet, view_count,
            acquisition: Acquisition::Triggered, trigger_source: "Software".into(), ..CameraConfig::default()
        })
    }).collect()
}

/// 合成图沿用真实相机组的编号与视角数，配方照常引用。
fn synthetic_cameras(real: &[CameraConfig]) -> Vec<CameraConfig> {
    real.iter().map(|camera| CameraConfig {
        id: camera.id.clone(), name: camera.name.clone(), source: CameraSource::Sim, view_count: camera.view_count,
        composite_layout: camera.composite_layout.clone(), acquisition: Acquisition::Triggered, trigger_source: "Software".into(),
        ..CameraConfig::default()
    }).collect()
}

/// 真机与离线·样本集用 lyFlow 测量图像；离线·合成图用模拟测量。
pub fn measurement_of(source: Option<SourceKind>) -> Measurement {
    if source == Some(SourceKind::Synthetic) { Measurement::Simulated } else { Measurement::LyFlow }
}

pub fn measurement(app: &AppHandle) -> Measurement {
    measurement_of(crate::offline::source_kind(app))
}

/// 检测记录的来源：写库与录制时按当前站点模式与离线来源决定。
pub fn record_source(app: &AppHandle) -> RecordSource {
    match crate::offline::source_kind(app) {
        None => RecordSource::Real,
        Some(SourceKind::Synthetic) => RecordSource::OfflineSynthetic,
        Some(SourceKind::SampleSet) => RecordSource::OfflineSample,
    }
}

pub fn image_measurement(app: &AppHandle) -> bool {
    measurement(app) == Measurement::LyFlow
}

/// 生效的帧录制方式：离线期间用派生值，真机用系统设置。
pub fn record_mode(app: &AppHandle) -> RecordMode {
    crate::offline::applied_record(app).unwrap_or_else(|| app.state::<CycleHost>().settings().record)
}

fn choice_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("site-offline.json"))
}

/// 启动时读回上次的离线选择；软件启动永远是真机，读不了只记日志不阻塞。
pub fn restore_choice(app: &AppHandle) -> Option<String> {
    let host = app.try_state::<OfflineHost>()?;
    match choice_file(app).and_then(|path| crate::fsio::read_json::<OfflineChoice>(&path)) {
        Ok(choice) => { host.set_last(choice); None }
        Err(note) => Some(note),
    }
}

fn save_choice(app: &AppHandle, choice: &OfflineChoice) -> Result<(), String> {
    crate::fsio::write_atomic(&choice_file(app)?, &serde_json::to_string_pretty(choice).map_err(|e| e.to_string())?)
}

async fn load_sample_set(dir: &str) -> Result<SampleSet, String> {
    let dir = dir.trim().to_string();
    if dir.is_empty() { return Err("请选择样本集目录".into()); }
    tauri::async_runtime::spawn_blocking(move || SampleSet::load(Path::new(&dir))).await.map_err(|e| e.to_string())?
}

async fn lyflow_state(app: &AppHandle) -> Result<(), String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let path = app.state::<CycleHost>().settings().lyflow_core;
        let status = app.state::<VisionHost>().status(path.as_deref());
        if status.loaded { Ok(()) } else { Err(status.message) }
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn site_status(offline: State<'_, OfflineHost>) -> SiteStatus {
    offline.status()
}

#[tauri::command]
pub async fn site_preview_sample_set(dir: String) -> Result<SampleSetSummary, String> {
    load_sample_set(&dir).await.map(|set| set.summary())
}

#[tauri::command]
pub async fn site_enter_offline(app: AppHandle, source: OfflineSource) -> Result<SiteStatus, String> {
    let offline = app.state::<OfflineHost>();
    let previous_dir = offline.last().and_then(|choice| choice.sample_set_dir);
    let (set, choice, lyflow) = match source {
        OfflineSource::SampleSet { dir } => {
            let set = Arc::new(load_sample_set(&dir).await?);
            (Some(set), OfflineChoice { kind: SourceKind::SampleSet, sample_set_dir: Some(dir.trim().to_string()) }, lyflow_state(&app).await)
        }
        OfflineSource::Synthetic => (None, OfflineChoice { kind: SourceKind::Synthetic, sample_set_dir: previous_dir }, Ok(())),
    };
    crate::offline::enter(&app, set, lyflow).await?;
    crate::offline::reset_after_enter(app.clone());
    if let Err(error) = save_choice(&app, &choice) {
        crate::cycle::log(&app, "warn", "站点模式", format!("上次的离线选择没能保存：{error}"));
    }
    offline.set_last(Some(choice));
    Ok(offline.status())
}

#[tauri::command]
pub async fn site_leave_offline(app: AppHandle) -> Result<SiteStatus, String> {
    crate::offline::leave(&app).await?;
    Ok(app.state::<OfflineHost>().status())
}

#[tauri::command]
pub async fn site_pick_dir(window: tauri::WebviewWindow, directory: String, title: Option<String>) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new().set_parent(&window).set_title(title.as_deref().unwrap_or("选择样本集目录"));
        let directory = PathBuf::from(directory);
        if directory.is_dir() {
            dialog = dialog.set_directory(directory);
        }
        dialog.pick_folder().map(|path| path.to_string_lossy().into_owned())
    }).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
