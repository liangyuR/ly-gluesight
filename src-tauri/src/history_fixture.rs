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
                    manual: false,
                    images: vec![Arc::new(image)],
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
