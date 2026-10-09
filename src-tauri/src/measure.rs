//! 逐帧测量。测量后端只量不判：模拟或 lyFlow（飞拍流程），由系统设置选择；判定统一在 judge。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc::{channel, Sender, UnboundedSender};
use tokio::sync::Semaphore;

use crate::cycle::{CycleHost, Input};
use crate::frame::FrameImage;
use crate::judge::PointState;
use crate::recipe::{Recipe, SegmentKind};
use crate::settings::RecordMode;
use crate::sim::Scenario;
use crate::vision;

/// 测量队列容量。满了说明测量跟不上帧率，新帧直接记为测量出错，不在内存里堆积。
pub const MEASURE_QUEUE: usize = 32;

/// 第 k 个拍照点的一帧，测该点负责的全部测量点。
pub struct Job {
    pub run_id: u64,
    pub sn: u32,
    /// 拍照点序号
    pub k: usize,
    pub cam: u8,
    pub recipe: Arc<Recipe>,
    pub scenario: Scenario,
    pub image: Option<Arc<FrameImage>>,
}

pub const ST_OK: u8 = 0;
pub const ST_GAP: u8 = 1;
pub const ST_INVALID: u8 = 2;

/// 单帧测量结果，只含该帧负责的测量点。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Measured {
    #[serde(skip)]
    pub run_id: u64,
    pub sn: u32,
    pub k: usize,
    pub cam: u8,
    pub located: bool,
    pub score: f32,
    pub ms: u32,
    /// 测量本身没做成（lyFlow 运行失败、没有示教资料、队列满等）
    pub error: Option<String>,
    pub idx: Vec<u32>,
    pub d: Vec<f32>,
    /// 胶宽；没测时为 NaN（前端收到 null）
    pub w: Vec<f32>,
    pub st: Vec<u8>,
    /// 图像测量时各点在原图里的像素位置（叠加显示用）
    pub px: Vec<[f32; 2]>,
}

impl Measured {
    pub fn point_state(&self, i: usize) -> PointState {
        match self.st[i] {
            ST_OK => PointState::Measured { d: self.d[i], w: self.w.get(i).copied().unwrap_or(f32::NAN) },
            ST_GAP => PointState::Gap,
            _ => PointState::Invalid,
        }
    }

    pub fn empty(job: &Job) -> Self {
        Self {
            run_id: job.run_id,
            sn: job.sn,
            k: job.k,
            cam: job.cam,
            located: false,
            score: 0.0,
            ms: 0,
            error: None,
            idx: Vec::new(),
            d: Vec::new(),
            w: Vec::new(),
            st: Vec::new(),
            px: Vec::new(),
        }
    }

    pub fn failed(job: &Job, error: impl Into<String>) -> Self {
        Self { error: Some(error.into()), ..Self::empty(job) }
    }
}

/// 测量后端：拿一帧图像，给出这一帧负责的测量点的结果。
pub trait Measurer: Send + Sync {
    fn measure(&self, job: &Job, image: &FrameImage) -> Result<Measured, String>;
}

/// 需要整帧图像的场合：图像测量或帧录制。设置变了之后调一次。
pub fn apply_settings(app: &AppHandle) {
    let settings = app.state::<CycleHost>().settings();
    let record = settings.record != RecordMode::Off;
    app.state::<CycleHost>().camera.set_capture(settings.vision || record);
}

fn run_image(app: &AppHandle, job: &Job, image: &FrameImage) -> Measured {
    vision::LyFlowMeasurer { app: app.clone() }.measure(job, image).unwrap_or_else(|e| Measured::failed(job, e))
}

/// 测量工作线程：有界队列，同时最多 `permits` 帧在测。
pub fn spawn_worker(app: AppHandle, out: UnboundedSender<Input>) -> Sender<Job> {
    let (tx, mut rx) = channel::<Job>(MEASURE_QUEUE);
    let permits = Arc::new(Semaphore::new(std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(2, 4)));
    tauri::async_runtime::spawn(async move {
        while let Some(job) = rx.recv().await {
            let Ok(permit) = permits.clone().acquire_owned().await else { break };
            let (app, out) = (app.clone(), out.clone());
            tauri::async_runtime::spawn(async move {
                let started = Instant::now();
                let vision = app.state::<CycleHost>().settings().vision;
                let mut m = match (job.image.clone(), vision) {
                    (Some(image), true) => {
                        let fallback = Measured::failed(&job, "测量线程异常退出");
                        tauri::async_runtime::spawn_blocking(move || run_image(&app, &job, &image)).await.unwrap_or(fallback)
                    }
                    (None, true) => Measured::failed(&job, "这一帧没有图像：相机未拷贝整帧"),
                    (_, false) => {
                        tokio::time::sleep(Duration::from_millis(170 + (job.k as u64 * 13) % 60)).await;
                        simulate(&job)
                    }
                };
                m.ms = started.elapsed().as_millis() as u32;
                let _ = out.send(Input::Measured(m));
                drop(permit);
            });
        }
    });
    tx
}

fn noise(s: f32) -> f32 {
    let v = (s * 12.9898).sin() * 43758.547;
    (v - v.floor()) * 2.0 - 1.0
}

/// 模拟测量：不看图像，按场景生成测量值。
fn simulate(job: &Job) -> Measured {
    let r = &job.recipe;
    let mut m = Measured::empty(job);
    let located = job.scenario.locate_fail_frame(r.shot_count()) != Some(job.k);
    let gap = job.scenario.gap_points(r);
    let bump_at = (job.scenario == Scenario::Excursion).then(|| {
        let seg = &r.segments[4.min(r.segments.len() - 1)];
        seg.s0 + (seg.s1 - seg.s0) * 0.3
    });
    m.located = located;
    m.score = if located { 0.91 + ((job.k * 7) % 5) as f32 / 100.0 } else { 0.38 };
    for j in r.owned_points(job.k) {
        let s = j as f32 * r.spacing;
        let seg = &r.segments[r.points.seg[j] as usize];
        let mut d = 0.74 + 0.09 * (s / 43.0).sin() + 0.035 * (s / 5.7 + 1.3).sin() + 0.03 * noise(s);
        if seg.kind == SegmentKind::Corner {
            d += 0.12 * ((s - seg.s0) / (seg.s1 - seg.s0) * std::f32::consts::PI).sin();
        }
        if let Some(c) = bump_at {
            d += (-((s - c) / 1.5).powi(2)).exp();
        }
        let st = if !located {
            ST_INVALID
        } else if gap.contains(&j) {
            ST_GAP
        } else {
            ST_OK
        };
        m.idx.push(j as u32);
        m.d.push(d);
        m.w.push(f32::NAN);
        m.st.push(st);
    }
    m
}
