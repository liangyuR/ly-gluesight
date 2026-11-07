//! 手动配置数据（docs/decisions/recipe-build-flow.md「手动配置数据」）：写仿历史录制件，供端到端与长稳测试走与现场相同的
//! 选件、导入路径。每件按真机维护采集件的做法落盘：用 `Recorder` 在真机录制根下写录制目录（`part.json` 带 `partMode`、
//! `devices`、`plc`，原图 `{相机}_s{seq:06}.pgm`），在记录库写 `parts` 行、逐帧记录与录制证据。
//!
//! 来源标记：`part.json` 带 `fixture: {label}`，SN 以 [`SN_PREFIX`] 开头，记录的原因与软件版本注明手动配置数据。
//! 记录用的配方是录制时生效的生产配方（通常是采集版）：修订号必须是软件分配过的版本，否则以后同修订号的真实记录会因快照不同被拒。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::burst_judge::{BurstJudgement, FrameState};
use crate::frame::{CounterSource, Frame, FrameImage};
use crate::judge::{Judgement, Verdict};
use crate::recipe::Recipe;
use crate::recorder::{Cleanup, Recorder, RecordingOutcome};
use crate::settings::RecordMode;
use crate::store::{
    BurstRecord, FrameKey, FrameRecord, PartMode, PartRecord, PlcDelivery, PlcDeliveryState,
    RecordSource, RecordingEvidence, RecordingState, Store,
};

/// 仿历史件的 SN 前缀。
pub const SN_PREFIX: &str = "FIXTURE-";
const DEVICE_SESSION: u64 = 1;
const COUNTER_BASE: u64 = 100;
const RECORDING_TIMEOUT: Duration = Duration::from_secs(120);

/// 一件的帧：`frames[seq - 1][计划相机下标]`，缺帧为 None；`hz` 为写帧时间戳用的出帧频率。
pub struct FixtureFrames {
    pub frames: Vec<Vec<Option<FrameImage>>>,
    pub hz: f64,
}

/// 写成的一件。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixturePart {
    pub record_id: i64,
    pub cycle_id: String,
    pub sn: String,
    pub ts: i64,
    pub directory: String,
    pub group_count: u64,
    pub camera_frames: Vec<u32>,
}

pub struct FixtureSite {
    store: Store,
    recorder: Recorder,
    outcomes: Mutex<Receiver<RecordingOutcome>>,
    recipe: Arc<Recipe>,
    devices: Vec<Value>,
    label: String,
    request_seq: Mutex<u32>,
}

impl FixtureSite {
    /// `db` 为项目已有的记录库（不新建、版本须一致），`records` 为真机录制根；`configs` 按计划相机顺序给出设备配置
    /// （与软件核对历史件时同一口径：`CameraConfig` 的序列化）。
    pub fn open(
        db: &Path,
        records: &Path,
        recipe: Recipe,
        configs: Vec<Value>,
        label: &str,
    ) -> Result<Self, String> {
        if label.trim().is_empty() {
            return Err("手动配置数据的标签不能为空".into());
        }
        if !recipe.is_burst() {
            return Err("配方不是段配方".into());
        }
        if recipe.version == 0 || recipe.revision_id != format!("{}-v{}", recipe.id, recipe.version)
        {
            return Err("配方修订号与编号、版本不一致：应是已发布的生产配方".into());
        }
        let cameras = recipe.plan_cameras();
        if configs.len() != cameras.len() {
            return Err(format!("设备配置应有 {} 台计划相机", cameras.len()));
        }
        if !records.is_dir() {
            return Err(format!("真机录制根 {} 不存在", records.display()));
        }
        check_database(db)?;
        let store = Store::open_deferred_recovery(db)?;
        if let Some(previous) = store.recipe_snapshot(&recipe.revision_id)? {
            let same = serde_json::to_value(&previous).map_err(|e| e.to_string())?
                == serde_json::to_value(&recipe).map_err(|e| e.to_string())?;
            if !same {
                return Err(format!(
                    "记录库已有修订 {} 的另一份配方快照：应传录制时生效的生产配方",
                    recipe.revision_id
                ));
            }
        }
        let devices = cameras
            .iter()
            .zip(configs)
            .map(|(camera, config)| {
                json!({
                    "cameraId": camera,
                    "cameraConfig": config,
                    "imaging": crate::camera::DEFAULT_IMAGING,
                    "deviceSession": DEVICE_SESSION,
                })
            })
            .collect();
        let (tx, rx) = channel();
        let root = records.to_path_buf();
        // 只写不清：软件自己的待入库目录与旧录制都算保护范围，滚动清理留给软件
        let cleanup = Cleanup {
            protection: Arc::new(move || Ok(vec![root.clone()])),
            warning: Arc::new(|_| {}),
            release: Arc::new(|_| Err("手动配置数据不清理录制".into())),
        };
        let recorder = Recorder::guarded(
            records.to_path_buf(),
            Some(Arc::new(move |outcome| {
                let _ = tx.send(outcome);
            })),
            cleanup,
        );
        Ok(Self {
            store,
            recorder,
            outcomes: Mutex::new(rx),
            recipe: Arc::new(recipe),
            devices,
            label: label.to_string(),
            request_seq: Mutex::new(0),
        })
    }

    /// 写一件：先入库（录制证据待定），再录制原图，最后补原图引用与录制证据。录制没完整落盘时记录保留失败证据并报错。
    pub fn write(&self, sn: &str, part: FixtureFrames) -> Result<FixturePart, String> {
        if !sn.starts_with(SN_PREFIX) {
            return Err(format!("手动配置数据的 SN 须以 {SN_PREFIX} 开头"));
        }
        let cameras = self.recipe.plan_cameras();
        let groups = part.frames.len() as u64;
        if groups == 0 {
            return Err("一件没有帧组：配方剖面或样本集为空".into());
        }
        if part.frames.iter().any(|row| row.len() != cameras.len()) {
            return Err(format!("每个帧组应有 {} 台计划相机的帧", cameras.len()));
        }
        let camera_frames: Vec<u32> = (0..cameras.len())
            .map(|camera| {
                part.frames
                    .iter()
                    .filter(|row| row[camera].is_some())
                    .count() as u32
            })
            .collect();
        let cycle_id = self.store.reserve_cycle_ids(1)?.remove(0);
        let ts = ly_plc::now_ms();
        let reason = format!(
            "维护采集：只录图、不判定（{groups} 个帧组）；手动配置数据（{}）",
            self.label
        );
        let judgement = Judgement {
            plc_code: 0,
            ..Judgement::error(0, reason.clone())
        };
        let frames: Vec<FrameRecord> = part
            .frames
            .iter()
            .enumerate()
            .flat_map(|(k, row)| {
                let seq = k as u64 + 1;
                row.iter()
                    .enumerate()
                    .map(move |(camera, image)| FrameRecord {
                        camera,
                        seq,
                        status: if image.is_some() {
                            FrameState::Unmeasured
                        } else {
                            FrameState::Missing
                        },
                        frame_counter: image.is_some().then_some(COUNTER_BASE + seq),
                        trigger_counter: image.is_some().then_some(seq),
                        region: None,
                        metrics: None,
                        raw_file: None,
                    })
            })
            .collect();
        let burst = BurstRecord {
            expected_total: self.recipe.expected_total(),
            tolerance_total: self.recipe.tolerance_total(),
            device_triggers: u32::try_from(groups).ok(),
            camera_frames: camera_frames.clone(),
            judgement: BurstJudgement {
                plc_code: 0,
                group_count: u32::try_from(groups).unwrap_or(u32::MAX),
                alarms: crate::frame_group::loss_alarms(&camera_frames),
                ..BurstJudgement::error(0, reason.clone())
            },
            frames,
        };
        let software = format!("capture_fixture {}", env!("CARGO_PKG_VERSION"));
        let record_id = self.store.insert(&PartRecord {
            mode: PartMode::MaintenanceCollect,
            ts,
            sn,
            recipe: Some(&self.recipe),
            judgement: &judgement,
            drain_ms: None,
            frames: &[],
            frames_expected: self.recipe.expected_total().unwrap_or(0) as usize,
            frames_received: groups as usize,
            triggers: groups,
            table: None,
            software_version: &software,
            cycle_id: Some(&cycle_id),
            bundle_id: None,
            delivery: &PlcDelivery {
                state: PlcDeliveryState::Acknowledged,
                updated_at: ts,
                message: None,
            },
            shots: &[],
            source: RecordSource::Real,
            sample: None,
            burst: Some(&burst),
        })?;
        let outcome = self.record(sn, &cycle_id, ts, &reason, part);
        let evidence = match &outcome {
            Ok(outcome) => RecordingEvidence {
                state: if outcome.available {
                    RecordingState::Complete
                } else {
                    RecordingState::Incomplete
                },
                available: outcome.available,
                directory: outcome
                    .directory
                    .as_ref()
                    .map(|d| d.to_string_lossy().into_owned()),
                errors: outcome.errors.clone(),
            },
            Err(error) => RecordingEvidence {
                state: RecordingState::Failed,
                available: false,
                directory: None,
                errors: vec![error.clone()],
            },
        };
        if let Ok(outcome) = &outcome {
            let files: BTreeMap<FrameKey, String> = outcome
                .files
                .iter()
                .map(|f| (f.frame, f.file.clone()))
                .collect();
            self.store.update_frame_raw_files(&cycle_id, &files)?;
        }
        self.store.update_recording(&cycle_id, &evidence)?;
        if !evidence.available {
            return Err(format!(
                "SN {sn} 原图录制未完整落盘：{}",
                evidence.errors.join("；")
            ));
        }
        Ok(FixturePart {
            record_id,
            cycle_id,
            sn: sn.to_string(),
            ts,
            directory: evidence.directory.unwrap_or_default(),
            group_count: groups,
            camera_frames,
        })
    }

    fn record(
        &self,
        sn: &str,
        cycle_id: &str,
        ts: i64,
        reason: &str,
        part: FixtureFrames,
    ) -> Result<RecordingOutcome, String> {
        let outcomes = self.outcomes.lock().unwrap();
        let receive = || {
            outcomes
                .recv_timeout(RECORDING_TIMEOUT)
                .map_err(|_| "等待录制收尾超时".to_string())
        };
        let Some(mut rec) =
            self.recorder
                .begin(RecordMode::All, sn, self.recipe.clone(), cycle_id, None)
        else {
            let outcome = receive()?;
            return Err(outcome.errors.join("；"));
        };
        let period_ms = 1000.0 / part.hz;
        for (k, row) in part.frames.into_iter().enumerate() {
            let seq = k as u64 + 1;
            for (camera, image) in row.into_iter().enumerate() {
                let Some(image) = image else { continue };
                let frame = Frame {
                    cam: camera as u8,
                    session: DEVICE_SESSION,
                    counter: CounterSource::Synthetic,
                    frame_counter: COUNTER_BASE + seq,
                    trigger_counter: seq,
                    lost_packets: 0,
                    ts: ts + (k as f64 * period_ms).round() as i64,
                    dev_ts: Some((COUNTER_BASE + seq) * crate::frame_clock::SYNTH_PERIOD_NS),
                    manual: false,
                    images: vec![Arc::new(image)],
                    bayer: None,
                };
                self.recorder.burst_frame(&mut rec, &frame, camera, seq);
            }
        }
        let request_seq = {
            let mut next = self.request_seq.lock().unwrap();
            *next = next.wrapping_add(1);
            *next
        };
        rec.annotate("partMode", json!(PartMode::MaintenanceCollect));
        rec.annotate("devices", json!(self.devices));
        rec.annotate(
            "plc",
            json!({"requestSeq": request_seq, "productCode": self.recipe.product_code}),
        );
        rec.annotate("fixture", json!({"label": self.label}));
        self.recorder.finish(
            rec,
            Verdict::ErrInspect,
            reason,
            u32::MAX,
            u64::MAX,
            Vec::new(),
        );
        let outcome = receive()?;
        if outcome.cycle_id != cycle_id {
            return Err(format!(
                "录制收尾的 cycleId {} 与本件不符",
                outcome.cycle_id
            ));
        }
        Ok(outcome)
    }
}

/// 原样导入现场的一件录制：录制目录（`part.json` 与原图）原样放到真机录制根下（同卷硬链接、跨卷复制），记录库按
/// `part.json` 写 `parts` 行、逐帧记录与录制证据，cycleId、SN、时间、帧计数与录制时生效的配方都保持现场原值。
/// 这样从它物化的示教件轮次与现场逐行相同，回传包导入现场后能从现场的历史记录补回原图。
pub fn import_recording(db: &Path, records: &Path, source: &Path) -> Result<FixturePart, String> {
    crate::image_file::checked_source_path(&source.join("part.json"))?;
    let part: Value = serde_json::from_str(
        std::fs::read_to_string(source.join("part.json"))
            .map_err(|e| format!("读取 {} 失败：{e}", source.join("part.json").display()))?
            .trim_start_matches('\u{feff}'),
    )
    .map_err(|e| format!("part.json 不是有效 JSON：{e}"))?;
    let text = |key: &str| {
        part[key]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("part.json 缺少 {key}"))
    };
    let cycle_id = text("cycleId")?;
    if cycle_id.len() > 128
        || !cycle_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("cycleId 只能包含字母、数字、- 和 _".into());
    }
    let sn = text("sn")?;
    let reason = text("reason")?;
    let verdict = text("verdict")?;
    let recorded_verdict: Verdict = serde_json::from_value(part["verdict"].clone())
        .map_err(|_| "part.json 的 verdict 无效".to_string())?;
    let ts = part["startedTs"]
        .as_i64()
        .ok_or("part.json 缺少 startedTs")?;
    let mode: PartMode = serde_json::from_value(part["partMode"].clone())
        .map_err(|_| "part.json 的 partMode 无效".to_string())?;
    let recipe: Recipe = serde_json::from_value(part["recipe"].clone())
        .map_err(|e| format!("part.json 里的配方无法读取：{e}"))?;
    if part["recipeRevision"].as_str() != Some(recipe.revision_id.as_str()) {
        return Err("part.json 的配方修订号与配方快照不一致".into());
    }
    let cameras: Vec<String> = serde_json::from_value(part["cameras"].clone())
        .map_err(|_| "part.json 的相机列表无效".to_string())?;
    if cameras != recipe.plan_cameras() {
        return Err("part.json 的相机与配方的计划相机不一致".into());
    }
    let groups = part["groupCount"]
        .as_u64()
        .filter(|g| *g > 0)
        .ok_or("part.json 没有帧组")?;
    let mut frames = Vec::new();
    let mut files = BTreeMap::new();
    let mut camera_frames = vec![0u32; cameras.len()];
    for frame in part["frames"].as_array().ok_or("part.json 没有帧")? {
        let camera = frame["camera"]
            .as_u64()
            .map(|c| c as usize)
            .filter(|c| *c < cameras.len())
            .ok_or("part.json 的帧相机下标无效")?;
        let seq = frame["seq"]
            .as_u64()
            .filter(|s| (1..=groups).contains(s))
            .ok_or("part.json 的帧组序号越界")?;
        let available = frame["available"].as_bool() == Some(true);
        let file = frame["file"].as_str().unwrap_or_default();
        if available {
            if file != format!("{}_s{seq:06}.pgm", cameras[camera]) || !source.join(file).is_file()
            {
                return Err(format!(
                    "相机 {} 第 {seq} 帧的原图 {file} 缺失或命名不符",
                    cameras[camera]
                ));
            }
            let path = source.join(file);
            crate::image_file::checked_source_path(&path)?;
            let bytes = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
            if !crate::image_file::read_pnm_info(&path)?.is_some_and(|info| {
                frame["width"].as_u64() == Some(u64::from(info.width))
                    && frame["height"].as_u64() == Some(u64::from(info.height))
                    && info.file_len() == bytes
            }) {
                return Err(format!(
                    "相机 {} 第 {seq} 帧的原图 {file} 尺寸或像素长度与录制元数据不符",
                    cameras[camera]
                ));
            }
            camera_frames[camera] += 1;
            files.insert(FrameKey { camera, seq }, file.to_string());
        }
        frames.push(FrameRecord {
            camera,
            seq,
            status: if available {
                FrameState::Unmeasured
            } else {
                FrameState::Missing
            },
            frame_counter: frame["frameCounter"].as_u64().filter(|_| available),
            trigger_counter: frame["triggerCounter"].as_u64().filter(|_| available),
            region: None,
            metrics: None,
            raw_file: None,
        });
    }
    if part["available"].as_bool() != Some(true)
        || camera_frames
            .iter()
            .any(|&count| u64::from(count) != groups)
        || ["missingFrames", "rawDropped", "errors"]
            .iter()
            .any(|key| part[*key].as_array().is_some_and(|items| !items.is_empty()))
    {
        return Err("原样导入要求原图完整且无录制错误".into());
    }
    if !records.is_dir() {
        return Err(format!(
            "真机录制根不存在：先创建 {} 再导入",
            records.display()
        ));
    }
    let records = records.canonicalize().map_err(|e| e.to_string())?;
    let started = chrono::DateTime::from_timestamp_millis(ts)
        .ok_or("startedTs 无效")?
        .with_timezone(&chrono::Local);
    let name = started.format("%Y%m%d_%H%M%S_%3f").to_string();
    let relative = format!("{}/{name}_{verdict}_cycle_{cycle_id}", &name[..8]);
    let target = records.join(&relative);
    if target.exists() {
        return Err(format!("录制目录 {} 已存在", target.display()));
    }
    check_database(db)?;
    let store = Store::open_deferred_recovery(db)?;
    if let Some(previous) = store.recipe_snapshot(&recipe.revision_id)? {
        if serde_json::to_value(&previous).map_err(|e| e.to_string())?
            != serde_json::to_value(&recipe).map_err(|e| e.to_string())?
        {
            return Err(format!(
                "记录库已有修订 {} 的另一份配方快照，与这件录制时的配方不同",
                recipe.revision_id
            ));
        }
    }
    let judgement = Judgement {
        verdict: recorded_verdict,
        plc_code: 0,
        ..Judgement::error(0, reason)
    };
    for frame in &mut frames {
        frame.raw_file = files
            .get(&FrameKey {
                camera: frame.camera,
                seq: frame.seq,
            })
            .map(|file| format!("{relative}/{file}"));
    }
    let burst = BurstRecord {
        expected_total: recipe.expected_total(),
        tolerance_total: recipe.tolerance_total(),
        device_triggers: u32::try_from(groups).ok(),
        camera_frames: camera_frames.clone(),
        judgement: BurstJudgement {
            verdict: recorded_verdict,
            plc_code: 0,
            group_count: u32::try_from(groups).unwrap_or(u32::MAX),
            alarms: crate::frame_group::loss_alarms(&camera_frames),
            ..BurstJudgement::error(0, reason)
        },
        frames,
    };
    let software = format!("import_recording {}", env!("CARGO_PKG_VERSION"));
    let record = PartRecord {
        mode,
        ts,
        sn,
        recipe: Some(&recipe),
        judgement: &judgement,
        drain_ms: None,
        frames: &[],
        frames_expected: recipe.expected_total().unwrap_or(0) as usize,
        frames_received: groups as usize,
        triggers: groups,
        table: None,
        software_version: &software,
        cycle_id: Some(cycle_id),
        bundle_id: part["bundleId"].as_str(),
        delivery: &PlcDelivery {
            state: PlcDeliveryState::Acknowledged,
            updated_at: ts,
            message: None,
        },
        shots: &[],
        source: RecordSource::Real,
        sample: None,
        burst: Some(&burst),
    };
    let staging = records.join(format!(".importing-{cycle_id}"));
    std::fs::create_dir(&staging).map_err(|e| {
        format!(
            "创建导入临时目录 {} 失败（已有目录时请先确认没有并发导入）：{e}",
            staging.display()
        )
    })?;
    let mut renamed = false;
    let imported = (|| {
        for entry in std::fs::read_dir(source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                continue;
            }
            let to = staging.join(entry.file_name());
            if std::fs::hard_link(entry.path(), &to).is_err() {
                std::fs::copy(entry.path(), &to)
                    .map_err(|e| format!("复制 {} 失败：{e}", entry.path().display()))?;
            }
        }
        store.insert_recording(&record, &target, || {
            std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::rename(&staging, &target)
                .map_err(|e| format!("完成录制目录 {} 失败：{e}", target.display()))?;
            renamed = true;
            Ok(())
        })
    })();
    let record_id = match imported {
        Ok(id) => id,
        Err(error) => {
            let owned = if renamed { &target } else { &staging };
            return Err(match std::fs::remove_dir_all(owned) {
                Ok(()) => error,
                Err(cleanup) => format!(
                    "{error}；清理本次导入目录 {} 失败：{cleanup}",
                    owned.display()
                ),
            });
        }
    };
    Ok(FixturePart {
        record_id,
        cycle_id: cycle_id.to_string(),
        sn: sn.to_string(),
        ts,
        directory: target.to_string_lossy().into_owned(),
        group_count: groups,
        camera_frames,
    })
}

/// 只往软件已建好的记录库写：文件须存在且版本一致，避免旧库被当成版本不符备份重建。
fn check_database(db: &Path) -> Result<(), String> {
    if !db.is_file() {
        return Err(format!(
            "记录库 {} 不存在：先启动一次软件建库",
            db.display()
        ));
    }
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("打开记录库失败：{e}"))?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| format!("读取记录库版本失败：{e}"))?;
    if version != crate::store::DB_VERSION {
        return Err(format!(
            "记录库版本 {version} 与软件 {} 不同，不写入",
            crate::store::DB_VERSION
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Recording {
        root: PathBuf,
        db: PathBuf,
        records: PathBuf,
        source: PathBuf,
        part: Value,
    }

    impl Recording {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "gluesight-import-{}-{}-{}",
                std::process::id(),
                ly_plc::now_ms(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            let db = root.join("inspection.db");
            let records = root.join("records");
            let source = root.join("source");
            std::fs::create_dir_all(&source).unwrap();
            std::fs::create_dir_all(&records).unwrap();
            drop(Store::open(&db).unwrap());
            let mut doc = crate::recipe::samples_burst();
            doc.version = 7;
            doc.stage = crate::recipe::RecipeStage::Collect;
            let recipe = doc.build().unwrap();
            let cameras = recipe.plan_cameras();
            let mut frames = Vec::new();
            for seq in 1..=3u64 {
                for (camera, id) in cameras.iter().enumerate() {
                    let file = format!("{id}_s{seq:06}.pgm");
                    std::fs::write(source.join(&file), b"P5\n2 2\n255\n\x10\x20\x30\x40").unwrap();
                    frames.push(json!({
                        "camera": camera, "seq": seq, "frameCounter": 9000 + 100 * camera as u64 + seq,
                        "triggerCounter": 700 + seq, "width": 2, "height": 2, "ts": 1700000000000i64 + seq as i64 * 200,
                        "available": true, "file": file,
                    }));
                }
            }
            let part = json!({
                "cycleId": "0123456789abcdef0123456789abcdef", "sn": "FIELD-SN-9",
                "startedTs": 1700000000000i64, "partMode": "maintenanceCollect",
                "verdict": "ERR_INSPECT", "reason": "维护采集", "available": true,
                "recipeRevision": recipe.revision_id, "recipe": recipe,
                "cameras": cameras, "groupCount": 3, "frames": frames,
                "missingFrames": [], "rawDropped": [], "errors": [],
            });
            std::fs::write(
                source.join("part.json"),
                format!("{}\n", serde_json::to_string_pretty(&part).unwrap()),
            )
            .unwrap();
            Self {
                root,
                db,
                records,
                source,
                part,
            }
        }

        fn import(&self) -> Result<FixturePart, String> {
            import_recording(&self.db, &self.records, &self.source)
        }
    }

    impl Drop for Recording {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn verbatim_recording_keeps_field_identity_and_rejects_duplicate_without_extra_files() {
        let fixture = Recording::new();
        let imported = fixture.import().unwrap();
        let store = Store::open(&fixture.db).unwrap();
        let rows = store
            .recorded_parts(fixture.part["recipe"]["id"].as_str().unwrap(), 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        let (record, evidence) = &rows[0];
        assert_eq!(record.cycle_id.as_deref(), fixture.part["cycleId"].as_str());
        assert_eq!(record.sn, fixture.part["sn"].as_str().unwrap());
        assert_eq!(record.ts, fixture.part["startedTs"].as_i64().unwrap());
        assert_eq!(evidence.state, RecordingState::Complete);
        assert!(evidence.available);
        let directory = PathBuf::from(evidence.directory.as_ref().unwrap());
        assert!(directory
            .canonicalize()
            .unwrap()
            .starts_with(fixture.records.canonicalize().unwrap()));
        assert_eq!(
            std::fs::read(directory.join("part.json")).unwrap(),
            std::fs::read(fixture.source.join("part.json")).unwrap()
        );
        let detail = store.detail_by_cycle(&imported.cycle_id).unwrap().unwrap();
        for frame in detail.burst.unwrap().frames {
            let original = fixture.part["frames"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| {
                    f["camera"].as_u64() == Some(frame.camera as u64)
                        && f["seq"].as_u64() == Some(frame.seq)
                })
                .unwrap();
            assert_eq!(frame.frame_counter, original["frameCounter"].as_u64());
            assert_eq!(frame.trigger_counter, original["triggerCounter"].as_u64());
            assert_eq!(
                std::fs::read(fixture.records.join(frame.raw_file.unwrap())).unwrap(),
                std::fs::read(fixture.source.join(original["file"].as_str().unwrap())).unwrap()
            );
        }
        assert!(fixture.import().is_err());
        assert_eq!(
            store
                .recorded_parts(fixture.part["recipe"]["id"].as_str().unwrap(), 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(std::fs::read_dir(&fixture.records).unwrap().count(), 1);
        assert_eq!(
            std::fs::read_dir(directory.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn import_failure_leaves_no_record_or_owned_directory_then_allows_retry() {
        for failure in ["directory", "commit", "truncated", "dimensions"] {
            let fixture = Recording::new();
            let conn = rusqlite::Connection::open(&fixture.db).unwrap();
            let image = fixture.source.join("cam1_s000001.pgm");
            let original = std::fs::read(&image).unwrap();
            let day = chrono::DateTime::from_timestamp_millis(
                fixture.part["startedTs"].as_i64().unwrap(),
            )
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y%m%d")
            .to_string();
            let date_dir = fixture.records.join(day);
            match failure {
                "commit" => conn.execute_batch("CREATE TABLE import_guard (part_id INTEGER REFERENCES parts(id) DEFERRABLE INITIALLY DEFERRED);
                    CREATE TRIGGER reject_import AFTER UPDATE OF recording_state ON parts BEGIN INSERT INTO import_guard VALUES(NEW.id + 100000); END;").unwrap(),
                "directory" => std::fs::write(&date_dir, b"keep-existing-file").unwrap(),
                "truncated" => std::fs::write(&image, &original[..original.len() - 1]).unwrap(),
                "dimensions" => std::fs::write(&image, b"P5\n4 1\n255\n\x10\x20\x30\x40").unwrap(),
                _ => unreachable!(),
            }
            assert!(fixture.import().is_err(), "{failure}");
            let store = Store::open(&fixture.db).unwrap();
            assert!(store
                .detail_by_cycle(fixture.part["cycleId"].as_str().unwrap())
                .unwrap()
                .is_none());
            assert!(store
                .recipe_snapshot(fixture.part["recipeRevision"].as_str().unwrap())
                .unwrap()
                .is_none());
            assert!(!fixture
                .records
                .join(format!(
                    ".importing-{}",
                    fixture.part["cycleId"].as_str().unwrap()
                ))
                .exists());
            if failure == "commit" {
                assert_eq!(std::fs::read_dir(&date_dir).unwrap().count(), 0);
                conn.execute_batch("DROP TRIGGER reject_import; DROP TABLE import_guard;")
                    .unwrap();
            } else if failure == "directory" {
                assert_eq!(std::fs::read(&date_dir).unwrap(), b"keep-existing-file");
                std::fs::remove_file(&date_dir).unwrap();
            } else {
                assert!(!date_dir.exists());
                std::fs::write(&image, original).unwrap();
            }
            fixture.import().unwrap();
            assert_eq!(
                store
                    .recorded_parts(fixture.part["recipe"]["id"].as_str().unwrap(), 10)
                    .unwrap()
                    .len(),
                1
            );
        }
    }
}
