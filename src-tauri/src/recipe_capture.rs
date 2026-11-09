use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::camera::CameraImaging;
use crate::cycle::CycleHost;
use crate::frame::FrameImage;

mod burst;

pub use burst::{CaptureRoundV2, HistoryFrame, HistoryRound, RoundOrigin, FORMAT};

/// 三目拼接图的拆分布局。旧拍照点模型：段配方只用单视角相机，不经过这里（D-SB-11）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CompositeLayout {
    Horizontal,
    Vertical,
    Rects { rects: [ViewRect; 3] },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CompositeLayout {
    pub fn split(&self, image: &FrameImage) -> Result<Vec<Arc<FrameImage>>, String> {
        let (w, h) = (image.width, image.height);
        if image.is_color() {
            return Err("三目拼接只支持灰度图，彩色帧不能拆分".into());
        }
        if w == 0 || h == 0 || (w as usize).checked_mul(h as usize) != Some(image.pixels.len()) {
            return Err("拼接原图的尺寸或像素长度无效".into());
        }
        let rects = match self {
            Self::Horizontal if w % 3 == 0 => std::array::from_fn(|i| ViewRect { x: i as u32 * (w / 3), y: 0, width: w / 3, height: h }),
            Self::Vertical if h % 3 == 0 => std::array::from_fn(|i| ViewRect { x: 0, y: i as u32 * (h / 3), width: w, height: h / 3 }),
            Self::Rects { rects } => rects.clone(),
            _ => return Err("拼接图尺寸不能按已配置方向三等分，请核对 SDK 交付布局".into()),
        };
        for (i, r) in rects.iter().enumerate() {
            if r.width == 0 || r.height == 0 || r.x.checked_add(r.width).is_none_or(|v| v > w) || r.y.checked_add(r.height).is_none_or(|v| v > h) {
                return Err(format!("图 {} 的裁切区域越界或为空", i + 1));
            }
            if rects[..i].iter().any(|other| r.x < other.x + other.width && other.x < r.x + r.width && r.y < other.y + other.height && other.y < r.y + r.height) {
                return Err("三幅图的裁切区域不能重叠".into());
            }
        }
        Ok(rects.iter().map(|r| {
            let mut pixels = Vec::with_capacity(r.width as usize * r.height as usize);
            for y in r.y..r.y + r.height {
                let start = y as usize * w as usize + r.x as usize;
                pixels.extend_from_slice(&image.pixels[start..start + r.width as usize]);
            }
            Arc::new(FrameImage::new(r.width, r.height, pixels))
        }).collect())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureState { WaitingStart, Receiving, Draining, Complete, Failed }

impl CaptureState {
    fn active(&self) -> bool { matches!(self, Self::WaitingStart | Self::Receiving | Self::Draining) }
}

/// 一轮采集最多使用的相机数。
const MAX_CAMERAS: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureCamera {
    pub camera_id: String,
    pub camera_config: serde_json::Value,
    pub imaging: CameraImaging,
    pub device_session: u64,
    pub received_count: u32,
}

/// 按历史件 cycleId 查它的录制目录（检测记录库里记的目录，可在任意盘）；记录不在本机或没有录制目录为 None。
pub type RecordingLookup = Arc<dyn Fn(&str) -> Result<Option<PathBuf>, String> + Send + Sync>;

pub struct RecipeCaptureHost {
    root: PathBuf,
    recordings: RecordingLookup,
}

/// 一轮段配方采集轮次（序列化为 CaptureRoundV2 的形状）。
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum AnyRound {
    Burst(CaptureRoundV2),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSummary {
    pub round_id: String,
    pub recipe_id: String,
    pub camera_ids: Vec<String>,
    pub planned_count: Option<u32>,
    pub received_count: u32,
    pub state: CaptureState,
    pub created_at: i64,
    pub error: Option<String>,
    /// 历史件导入或手动配置数据的来源；以前示教实拍的轮次为空
    pub origin: Option<RoundOrigin>,
}

impl RecipeCaptureHost {
    fn safe_round_path(&self, round_id: &str) -> Result<PathBuf, String> {
        if !round_id.starts_with("capture-") || !round_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("无效的采集轮次 ID".into());
        }
        let root = std::fs::canonicalize(&self.root).map_err(|e| e.to_string())?;
        let path = std::fs::canonicalize(self.root.join(round_id)).map_err(|e| e.to_string())?;
        if !path.starts_with(&root) || path == root { return Err("采集目录不在允许的保存位置".into()); }
        Ok(path)
    }

    /// 打开采集目录：启动时仍在进行中的轮次判为中断，读不了的记录改名备份。`recordings` 用来找历史件导入轮次的录制原图。
    pub fn open(root: PathBuf, recordings: RecordingLookup) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let canonical_root = std::fs::canonicalize(&root).map_err(|e| e.to_string())?;
        let backup_broken = |path: &Path| -> Result<(), String> {
            let backup = path.with_file_name(format!("round.broken-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S")));
            std::fs::rename(path, &backup).map_err(|e| format!("采集记录无法读取且备份失败：{e}"))
        };
        for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path().join("round.json");
            if !path.is_file() { continue; }
            if !std::fs::canonicalize(&path).map_err(|e| e.to_string())?.starts_with(&canonical_root) { return Err("采集记录路径越出保存目录".into()); }
            let text = crate::fsio::read_text(&path).map_err(|e| e.to_string())?;
            if !burst::is_v2(&text) {
                continue;
            }
            match serde_json::from_str::<CaptureRoundV2>(&text) {
                Ok(round) => {
                    burst::restore(&path, round)?;
                }
                Err(_) => backup_broken(&path)?,
            }
        }
        Ok(Arc::new(Self { root, recordings }))
    }

    /// 按轮次 ID 读一轮，经记录核对；旧拍照点配方的采集轮次已停用，按无效记录报错。
    pub fn get_any(&self, round_id: &str) -> Result<AnyRound, String> {
        let dir = self.safe_round_path(round_id)?;
        if crate::fsio::read_text(&dir.join("round.json")).is_ok_and(|text| !burst::is_v2(&text)) {
            return Err(format!(
                "采集轮次 {round_id} 是旧拍照点配方的采集记录，已停用"
            ));
        }
        self.get_burst(round_id).map(AnyRound::Burst)
    }

    /// 段配方轮次的 ID（round.json 为 format 2 或 3 的目录）。
    fn round_ids(&self) -> Result<Vec<String>, String> {
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if crate::fsio::read_text(&entry.path().join("round.json"))
                .is_ok_and(|text| burst::is_v2(&text))
            {
                ids.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(ids)
    }

    /// 最近创建的一轮；记录读不了的轮次跳过。
    pub fn latest(&self) -> Result<Option<AnyRound>, String> {
        let latest = self
            .round_ids()?
            .iter()
            .filter_map(|id| self.burst_summary(id).ok())
            .max_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then_with(|| a.round_id.cmp(&b.round_id))
            });
        latest.map(|r| self.get_any(&r.round_id)).transpose()
    }

    pub fn list(&self, recipe_id: &str) -> Result<Vec<CaptureSummary>, String> {
        let mut rounds = Vec::new();
        for id in self.round_ids()? {
            let r = self.burst_summary(&id)?;
            if r.recipe_id == recipe_id {
                rounds.push(CaptureSummary {
                    camera_ids: r.camera_ids(),
                    received_count: u32::try_from(r.group_count).unwrap_or(u32::MAX),
                    round_id: r.round_id,
                    recipe_id: r.recipe_id,
                    planned_count: r.plc_shot_count.map(u32::from),
                    state: r.state,
                    created_at: r.created_at,
                    error: r.error,
                    origin: r.origin,
                });
            }
        }
        rounds.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.round_id.cmp(&a.round_id)));
        Ok(rounds)
    }

    /// 删除一轮的目录（只删本轮目录：引用的录制原图不动，旧轮次的硬链接只删链接）。
    pub fn delete_round(&self, round_id: &str) -> Result<(), String> {
        let dir = self.safe_round_path(round_id)?;
        std::fs::remove_dir_all(&dir).map_err(|e| format!("删除采集轮次 {round_id} 失败：{e}"))
    }

    /// 删除某配方编号的全部轮次（含读不了统计的旧轮次），返回删掉的轮次 ID。
    pub fn delete_recipe_rounds(&self, recipe_id: &str) -> Result<Vec<String>, String> {
        let mut deleted = Vec::new();
        for id in self.round_ids()? {
            let owned = crate::fsio::read_text(&self.root.join(&id).join("round.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .is_some_and(|v| v["recipeId"].as_str() == Some(recipe_id));
            if owned {
                self.delete_round(&id)?;
                deleted.push(id);
            }
        }
        Ok(deleted)
    }
}

/// 登记历史件引用（导入示教件）与清理录制、记录（滚动清理、按天清理）互斥：清理从查引用到删除持有它，导入从核对到写好轮次
/// 持有它，免得清理按旧的引用快照删掉刚被引用的件。
pub static REFERENCES: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn lock_references() -> std::sync::MutexGuard<'static, ()> {
    REFERENCES.lock().unwrap_or_else(|e| e.into_inner())
}

/// 采集目录 `root` 里历史件导入轮次引用的来源 cycleId：这些录制不参与滚动清理与按天清理。目录不在为空；记录读不了的轮次跳过。
pub fn history_cycles(root: &Path) -> Result<std::collections::BTreeSet<String>, String> {
    let mut cycles = std::collections::BTreeSet::new();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(cycles),
        Err(e) => return Err(format!("读取采集目录 {} 失败：{e}", root.display())),
    };
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path().join("round.json");
        if !path.is_file() {
            continue;
        }
        let text = crate::fsio::read_text(&path).map_err(|e| format!("{}：{e}", path.display()))?;
        let Ok(round) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if round
            .pointer("/origin/kind")
            .and_then(serde_json::Value::as_str)
            == Some("history")
        {
            if let Some(cycle) = round
                .pointer("/origin/cycleId")
                .and_then(serde_json::Value::as_str)
            {
                cycles.insert(cycle.to_string());
            }
        }
    }
    Ok(cycles)
}

#[tauri::command]
pub fn recipe_capture_get(app: AppHandle, round_id: Option<String>) -> Result<Option<AnyRound>, String> {
    let cycle = app.state::<CycleHost>();
    let capture = cycle.camera.recipe_capture();
    match round_id {
        Some(id) => capture.get_any(&id).map(Some),
        None => capture.latest(),
    }
}

#[tauri::command]
pub fn recipe_capture_list(app: AppHandle, recipe_id: String) -> Result<Vec<CaptureSummary>, String> {
    app.state::<CycleHost>().camera.recipe_capture().list(&recipe_id)
}

/// 某台相机第 seq 帧（帧组序号，从 1 起）的原图，格式见 `frame::preview_bytes`；这台相机缺这一帧时报错。
#[tauri::command]
pub async fn recipe_capture_frame_image(
    app: AppHandle,
    round_id: String,
    camera_id: String,
    seq: u64,
) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = app
            .state::<CycleHost>()
            .camera
            .recipe_capture()
            .burst_frame_path(&round_id, &camera_id, seq)?;
        let image = crate::image_file::load(&path)?;
        Ok(tauri::ipc::Response::new(crate::frame::preview_bytes(
            &image,
        )))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 缩略图下采样倍数：1280×1024 → 160×128。
const THUMB_FACTOR: u32 = 8;

/// 一台相机指定帧组范围的缩略图条带，每批最多 32 组。
#[tauri::command]
pub async fn recipe_capture_round_thumbs(
    app: AppHandle,
    round_id: String,
    camera_id: String,
    start_seq: u64,
    count: u32,
) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = app
            .state::<CycleHost>()
            .camera
            .recipe_capture()
            .burst_frame_thumbs(
                &round_id,
                &camera_id,
                start_seq,
                count,
                crate::image_file::load,
            )?;
        Ok(tauri::ipc::Response::new(bytes))
    })
    .await
    .map_err(|e| e.to_string())?
}
