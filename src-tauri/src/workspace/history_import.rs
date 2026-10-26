//! 历史件导入为采集轮次（docs/architecture/online-commissioning.md 5.2、5.3）：候选采集页“从历史选用”列出本配方的真机录制件
//! 并逐件给出能否导入的原因；导入时按 `RecipeCaptureHost::import_history` 物化成普通采集轮次，下游示教、分段复核与标定不改。
//! 已导入并用于走廊示教或多轮标定的历史件不能再作独立验证样本（[`check_history_sample`]）。

use std::collections::BTreeSet;

use super::*;
use crate::recipe_capture::{
    CaptureCamera, HistoryFrame, HistoryRound, RecipeCaptureHost, RoundOrigin,
};
use crate::store::{PartMode, PartSummary, RecordSource, RecordingEvidence, RecordingState};

/// 列表最多核对的历史件数（最新在前）。
const LIST_LIMIT: u32 = 200;
/// 一次最多导入的件数。
const IMPORT_LIMIT: usize = 50;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCameraFrames {
    pub camera_id: String,
    pub frames: u32,
}

/// “从历史选用”的一行（TS `HistoryCandidate`）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCandidate {
    pub record_id: i64,
    pub cycle_id: Option<String>,
    pub ts: i64,
    pub sn: String,
    pub mode: PartMode,
    pub group_count: u64,
    /// 按录制的计划相机顺序
    pub cameras: Vec<HistoryCameraFrames>,
    /// 原图完整：录制已核验落盘，没有缺帧、没有录制队满未录的帧
    pub complete: bool,
    /// 不能导入的原因；为空即可导入
    pub reasons: Vec<String>,
    /// 已导入为本候选的轮次
    pub imported_round: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPick {
    pub record_id: i64,
    #[serde(default)]
    pub standard: bool,
}

pub(super) struct Assessed {
    pub view: HistoryCandidate,
    /// 能导入时要物化的轮次（标准件标记为 false，由导入时按操作员勾选改）
    pub round: Option<HistoryRound>,
}

/// 本候选已导入的历史件：cycleId → 轮次。
fn imported_cycles(
    capture: &RecipeCaptureHost,
    recipe_id: &str,
) -> Result<BTreeMap<String, String>, String> {
    Ok(capture
        .list(recipe_id)?
        .into_iter()
        .filter_map(|r| r.origin.map(|o| (o.cycle_id().to_string(), r.round_id)))
        .collect())
}

/// 读录制目录里的 part.json：目录须在真机录制根下。
fn read_part(root: &Path, recording: &RecordingEvidence) -> Result<(PathBuf, Value), String> {
    let dir = PathBuf::from(recording.directory.as_deref().ok_or("记录没有录制目录")?);
    let root = root
        .canonicalize()
        .map_err(|_| "录制根目录不存在".to_string())?;
    let checked = dir
        .canonicalize()
        .map_err(|_| "录制目录不存在或已按保留策略清理".to_string())?;
    if !checked.starts_with(&root) || checked == root {
        return Err("录制目录不在真机录制根目录下".into());
    }
    let path = checked.join("part.json");
    crate::image_file::checked_source_path(&path)
        .map_err(|_| "录制元数据 part.json 缺失或不可读".to_string())?;
    let value = serde_json::from_slice(
        &std::fs::read(&path).map_err(|e| format!("录制元数据读取失败：{e}"))?,
    )
    .map_err(|e| format!("录制元数据损坏：{e}"))?;
    Ok((checked, value))
}

fn frame_count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 逐条核对一件历史件能否导入为本候选的采集轮次（5.2 可用条件）：维护模式或生产的真机件、属于本候选的型号、
/// 有一期录制元数据（相机设备与 PLC 请求）、计划相机齐全且设备配置与当前一致（`current` 按相机编号，`state_value` 口径）、
/// 没有提前结束、原图完整（无缺帧、无 rawDropped、每帧已落盘）、帧号与帧组序号一致；`imported` 为已导入的 cycleId → 轮次。
pub(super) fn assess(
    doc: &RecipeDoc,
    current: &BTreeMap<String, Value>,
    imported: &BTreeMap<String, String>,
    summary: &PartSummary,
    recording: &RecordingEvidence,
    part: Result<(PathBuf, Value), String>,
) -> Assessed {
    let mut reasons = Vec::new();
    let mut view = HistoryCandidate {
        record_id: summary.id,
        cycle_id: summary.cycle_id.clone(),
        ts: summary.ts,
        sn: summary.sn.clone(),
        mode: summary.mode,
        group_count: 0,
        cameras: Vec::new(),
        complete: false,
        reasons: Vec::new(),
        imported_round: summary
            .cycle_id
            .as_ref()
            .and_then(|c| imported.get(c))
            .cloned(),
    };
    if summary.source != RecordSource::Real {
        reasons.push(format!("{}记录不能导入", summary.source.label()));
    }
    if let Some(round) = &view.imported_round {
        reasons.push(format!("已导入为轮次 {round}"));
    }
    if summary.flags.as_ref().is_some_and(|f| f.short_part) {
        reasons.push("PLC 提前结束运动（疑似提前终止）".into());
    }
    let recorded = recording.state == RecordingState::Complete && recording.available;
    if !recorded {
        reasons.push("录制不完整或原图已清理".into());
    }
    let rejected = |mut view: HistoryCandidate, mut reasons: Vec<String>, reason: String| {
        reasons.push(reason);
        view.reasons = reasons;
        Assessed { view, round: None }
    };
    let Some(cycle_id) = summary.cycle_id.as_deref() else {
        return rejected(view, reasons, "记录没有 cycleId".into());
    };
    let (dir, part) = match part {
        Ok(found) if found.1["cycleId"].as_str() == Some(cycle_id) => found,
        Ok(_) => return rejected(view, reasons, "录制元数据的 cycleId 与记录不一致".into()),
        Err(error) => return rejected(view, reasons, error),
    };
    let cameras: Vec<String> = part["cameras"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let group_count = part["groupCount"].as_u64().unwrap_or(0);
    view.group_count = group_count;
    let frames = part["frames"].as_array().cloned().unwrap_or_default();
    let mut saved = vec![BTreeSet::new(); cameras.len()];
    let mut history_frames = Vec::new();
    let mut baselines: Vec<Option<u64>> = vec![None; cameras.len()];
    let (mut unsaved, mut manual, mut misnamed, mut counter_gap) =
        (0usize, false, false, BTreeSet::new());
    for frame in &frames {
        let camera = frame["camera"]
            .as_u64()
            .and_then(|c| usize::try_from(c).ok())
            .filter(|&c| c < cameras.len());
        let (Some(camera), Some(seq)) = (
            camera,
            frame["seq"]
                .as_u64()
                .filter(|&s| (1..=group_count).contains(&s)),
        ) else {
            misnamed = true;
            continue;
        };
        if frame["available"].as_bool() != Some(true) {
            unsaved += 1;
            continue;
        }
        manual |= frame["manual"].as_bool() == Some(true);
        let file = frame["file"].as_str().unwrap_or_default();
        if frame["cameraId"].as_str() != Some(cameras[camera].as_str())
            || file != format!("{}_s{seq:06}.pgm", cameras[camera])
        {
            misnamed = true;
            continue;
        }
        let counter = frame["frameCounter"].as_u64().unwrap_or(0);
        match counter.checked_sub(seq) {
            Some(base) if *baselines[camera].get_or_insert(base) == base => {}
            _ => {
                counter_gap.insert(cameras[camera].clone());
            }
        }
        if !saved[camera].insert(seq) {
            misnamed = true;
            continue;
        }
        let size = |field: &str| {
            frame[field]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .filter(|&v| v > 0)
        };
        let (Some(width), Some(height)) = (size("width"), size("height")) else {
            misnamed = true;
            continue;
        };
        history_frames.push(HistoryFrame {
            camera,
            seq,
            frame_counter: counter,
            width,
            height,
            ts: frame["ts"].as_i64().unwrap_or(summary.ts),
            source: dir.join(file),
        });
    }
    view.cameras = cameras
        .iter()
        .zip(&saved)
        .map(|(camera_id, seqs)| HistoryCameraFrames {
            camera_id: camera_id.clone(),
            frames: frame_count(seqs.len()),
        })
        .collect();
    let missing = part["missingFrames"].as_array().map_or(0, Vec::len);
    let dropped = part["rawDropped"].as_array().map_or(0, Vec::len);
    let absent =
        cameras.len() as u64 * group_count - saved.iter().map(|s| s.len() as u64).sum::<u64>();
    view.complete = recorded
        && group_count > 0
        && missing == 0
        && dropped == 0
        && unsaved == 0
        && absent == 0
        && !misnamed;
    if group_count == 0 {
        reasons.push("没有帧组".into());
    }
    if missing > 0 {
        reasons.push(format!("缺帧 {missing} 帧"));
    }
    if dropped > 0 {
        reasons.push(format!("{dropped} 帧原图未录（录制队满，rawDropped）"));
    }
    if unsaved > 0 {
        reasons.push(format!("{unsaved} 帧原图未确认落盘"));
    }
    if misnamed {
        reasons.push("录制元数据的帧身份或原图文件名无效".into());
    }
    if manual {
        reasons.push("含手动触发帧".into());
    }
    for camera in &counter_gap {
        reasons.push(format!("相机 {camera} 的帧号与帧组序号对不上"));
    }
    let devices = part["devices"].as_array();
    let plc = part["plc"].as_object();
    match (devices, plc) {
        (Some(_), Some(plc)) => {
            let code = plc.get("productCode").and_then(Value::as_u64);
            if code != Some(u64::from(doc.product_code)) {
                reasons.push(format!(
                    "型号 {} 不属于本候选（型号 {}）",
                    code.map_or("未知".into(), |c| c.to_string()),
                    doc.product_code
                ));
            }
            if plc.get("earlyEnd").and_then(Value::as_bool) == Some(true) {
                reasons.push("PLC 提前结束运动".into());
            }
        }
        _ => reasons.push("旧录制没有相机设备与 PLC 请求元数据，只能作验证样本".into()),
    }
    let mut planned = doc.plan_cameras();
    planned.sort();
    let mut actual = cameras.clone();
    actual.sort();
    if actual != planned {
        reasons.push(format!(
            "录制相机 {cameras:?} 与计划相机 {:?} 不同",
            doc.plan_cameras()
        ));
    }
    let mut capture_cameras = Vec::with_capacity(cameras.len());
    for camera_id in &cameras {
        let Some(device) =
            devices.and_then(|d| d.iter().find(|d| d["cameraId"].as_str() == Some(camera_id)))
        else {
            if devices.is_some() {
                reasons.push(format!("相机 {camera_id} 没有设备记录"));
            }
            continue;
        };
        let config = &device["cameraConfig"];
        if config.is_null() || current.get(camera_id) != Some(config) {
            reasons.push(format!("相机 {camera_id} 的设备配置与当前不一致"));
        }
        let imaging = serde_json::from_value::<CameraImaging>(device["imaging"].clone()).ok();
        let session = device["deviceSession"].as_u64().filter(|&s| s > 0);
        match (imaging, session) {
            (Some(imaging), Some(device_session)) => capture_cameras.push(CaptureCamera {
                camera_id: camera_id.clone(),
                camera_config: config.clone(),
                imaging,
                device_session,
                received_count: 0,
            }),
            _ => reasons.push(format!("相机 {camera_id} 缺曝光增益或设备会话记录")),
        }
    }
    let round = reasons.is_empty().then(|| HistoryRound {
        recipe_id: doc.id.clone(),
        origin: RoundOrigin::History {
            cycle_id: cycle_id.to_string(),
            record_id: summary.id,
            standard: false,
        },
        cameras: capture_cameras,
        group_count,
        frames: history_frames,
    });
    view.reasons = reasons;
    Assessed { view, round }
}

fn current_configs(cycle: &CycleHost) -> BTreeMap<String, Value> {
    cycle
        .camera
        .configs()
        .iter()
        .map(|c| (c.id.clone(), state_value(c)))
        .collect()
}

fn assess_all(app: &AppHandle, doc: &RecipeDoc) -> Result<Vec<Assessed>, String> {
    if !doc.is_burst() {
        return Err("旧拍照点配方已停用，请创建段配方".into());
    }
    let cycle = app.state::<CycleHost>();
    let current = current_configs(&cycle);
    let imported = imported_cycles(cycle.camera.recipe_capture(), &doc.id)?;
    let root = cycle.recorders.root(RecordSource::Real).to_path_buf();
    let parts = app.state::<Store>().recorded_parts(&doc.id, LIST_LIMIT)?;
    Ok(parts
        .iter()
        .map(|(summary, recording)| {
            assess(
                doc,
                &current,
                &imported,
                summary,
                recording,
                read_part(&root, recording),
            )
        })
        .collect())
}

/// 候选采集页“从历史选用”：本配方最近的真机录制件与能否导入（只读）。
#[tauri::command]
pub async fn workspace_history_candidates(
    app: AppHandle,
    id: String,
) -> Result<Vec<HistoryCandidate>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let doc = app
            .state::<WorkspaceHost>()
            .items
            .lock()
            .unwrap()
            .get(&id)
            .ok_or("候选配置不存在")?
            .doc
            .clone();
        Ok(assess_all(&app, &doc)?
            .into_iter()
            .map(|a| a.view)
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 把选中的历史件逐件导入为本候选的采集轮次，返回新轮次 ID（按选择顺序）。导入前在配置门内重新核对每一件，
/// 任一件不能导入时一件也不导；导入只建硬链接与轮次记录，不占用设备，生产中也可操作。
#[tauri::command]
pub async fn workspace_history_import(
    app: AppHandle,
    id: String,
    revision: u64,
    picks: Vec<HistoryPick>,
) -> Result<Vec<String>, String> {
    let data = json!({
        "recipeId": id,
        "records": picks.iter().map(|p| json!({"recordId": p.record_id, "standard": p.standard})).collect::<Vec<_>>(),
    });
    let result = history_import(app.clone(), id, revision, picks).await;
    let mut data = data;
    data["rounds"] = json!(result.as_ref().ok());
    crate::cycle::log_op(
        &app,
        &crate::applog::codes::OP_CAPTURE_IMPORT,
        data,
        &result,
    );
    result
}

async fn history_import(
    app: AppHandle,
    id: String,
    revision: u64,
    picks: Vec<HistoryPick>,
) -> Result<Vec<String>, String> {
    if picks.is_empty() || picks.len() > IMPORT_LIMIT {
        return Err(format!("每次导入 1–{IMPORT_LIMIT} 件"));
    }
    let mut unique = BTreeSet::new();
    if picks.iter().any(|p| !unique.insert(p.record_id)) {
        return Err("不能重复选择同一件".into());
    }
    let cycle = app.state::<CycleHost>();
    let _config = cycle.config_gate.lock().await;
    let task_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let doc = {
            let host = task_app.state::<WorkspaceHost>();
            let items = host.items.lock().unwrap();
            let w = items.get(&id).ok_or("候选配置不存在")?;
            w.expect(revision)?;
            w.doc.clone()
        };
        let mut assessed: BTreeMap<i64, Assessed> = assess_all(&task_app, &doc)?
            .into_iter()
            .map(|a| (a.view.record_id, a))
            .collect();
        let mut rounds = Vec::with_capacity(picks.len());
        for pick in &picks {
            let entry = assessed
                .remove(&pick.record_id)
                .ok_or_else(|| format!("记录 {} 不在本配方最近的录制件里", pick.record_id))?;
            let mut round = entry.round.ok_or_else(|| {
                format!(
                    "记录 {}（SN {}）不能导入：{}",
                    pick.record_id,
                    entry.view.sn,
                    entry.view.reasons.join("；")
                )
            })?;
            let RoundOrigin::History { standard, .. } = &mut round.origin;
            *standard = pick.standard;
            rounds.push(round);
        }
        let cycle = task_app.state::<CycleHost>();
        let capture = cycle.camera.recipe_capture();
        let mut imported = Vec::with_capacity(rounds.len());
        for round in rounds {
            let record = round.origin.cycle_id().to_string();
            match capture.import_history(round) {
                Ok(round) => imported.push(round.round_id),
                Err(error) if imported.is_empty() => return Err(error),
                Err(error) => {
                    return Err(format!(
                        "已导入 {} 件（{}），cycleId {record} 导入失败：{error}",
                        imported.len(),
                        imported.join("、")
                    ))
                }
            }
        }
        Ok(imported)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 历史记录作独立验证样本前的核对（5.3）：这件已导入为本候选的轮次且该轮用于走廊示教或多轮标定时拒绝（`origins` 为本候选
/// 各轮次的来源）；录制有相机设备记录时，各计划相机的设备配置须与当前一致（`current` 按相机编号，`state_value` 口径）。
pub(super) fn check_history_sample(
    w: &Workspace,
    cycle_id: Option<&str>,
    origins: &BTreeMap<String, RoundOrigin>,
    devices: Option<&Value>,
    current: &BTreeMap<String, Value>,
) -> Result<(), String> {
    if let Some(cycle_id) = cycle_id {
        let trained = origins.iter().find(|(round, origin)| {
            origin.cycle_id() == cycle_id
                && (w.camera_teaching.iter().any(|t| &t.round == *round)
                    || w.calibration.rounds.iter().any(|r| &r.round_id == *round))
        });
        if let Some((round, _)) = trained {
            return Err(format!(
                "这件已导入为轮次 {round} 并用于走廊示教或多轮标定：训练数据不能再作独立验证样本"
            ));
        }
    }
    for device in devices.and_then(Value::as_array).into_iter().flatten() {
        let camera = device["cameraId"].as_str().unwrap_or_default();
        if w.doc.plan_cameras().iter().any(|c| c == camera)
            && !device["cameraConfig"].is_null()
            && current.get(camera) != Some(&device["cameraConfig"])
        {
            return Err(format!(
                "录制时相机 {camera} 的设备配置与当前不同，不能作验证样本"
            ));
        }
    }
    Ok(())
}

/// 本候选各轮次的来源（历史件导入的轮次），轮次 → 来源。
pub(super) fn round_origins(
    capture: &RecipeCaptureHost,
    recipe_id: &str,
) -> Result<BTreeMap<String, RoundOrigin>, String> {
    Ok(capture
        .list(recipe_id)?
        .into_iter()
        .filter_map(|r| r.origin.map(|o| (r.round_id, o)))
        .collect())
}

/// 历史记录录制时的相机设备记录（part.json 的 devices）；没有录制或旧录制为 None。
pub(super) fn recorded_devices(app: &AppHandle, recording: &RecordingEvidence) -> Option<Value> {
    let root = app
        .state::<CycleHost>()
        .recorders
        .root(RecordSource::Real)
        .to_path_buf();
    read_part(&root, recording)
        .ok()
        .map(|(_, part)| part["devices"].clone())
        .filter(|d| !d.is_null())
}

#[cfg(test)]
#[path = "history_import_tests.rs"]
mod tests;
