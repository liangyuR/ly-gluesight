//! 逐帧测量。测量后端只量不判：模拟、lyFlow（飞拍流程）、本程序卡尺（随动）实现同一个接口，
//! 由系统设置选择；判定统一在 judge。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc::{channel, Sender, UnboundedSender};
use tokio::sync::Semaphore;

use crate::caliper;
use crate::cycle::{CycleHost, Input};
use crate::follow::{self, FollowCalib};
use crate::frame::FrameImage;
use crate::judge::PointState;
use crate::recipe::{InspectMode, Recipe, SegmentKind};
use crate::settings::RecordMode;
use crate::sim::Scenario;
use crate::simfollow;
use crate::vision;

/// 测量队列容量。满了说明测量跟不上帧率，新帧直接记为测量出错，不在内存里堆积。
pub const MEASURE_QUEUE: usize = 32;

#[derive(Clone, Debug)]
pub enum JobKind {
    /// 飞拍第 k 个拍照点，测该点负责的全部测量点
    Shot { k: usize },
    /// 随动：胶嘴位于弧长 s 时的一帧，测分给它的点；start_probe 时顺带找胶条起点做起点同步
    Follow { s: f32, points: Vec<u32>, calib: FollowCalib, start_probe: bool },
}

pub struct Job {
    pub sn: u32,
    /// 帧在本件里的序号：飞拍是拍照点 k，随动是第几个被测的帧
    pub k: usize,
    pub cam: u8,
    pub recipe: Arc<Recipe>,
    pub scenario: Scenario,
    pub image: Option<Arc<FrameImage>>,
    pub kind: JobKind,
}

pub const ST_OK: u8 = 0;
pub const ST_GAP: u8 = 1;
pub const ST_INVALID: u8 = 2;

/// 单帧测量结果，只含该帧负责的测量点。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Measured {
    pub sn: u32,
    pub k: usize,
    pub cam: u8,
    /// 随动：拍这一帧时胶嘴所在弧长
    pub s: Option<f32>,
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
    /// 随动同步：这一帧是否找过胶条起点、起点同步与拐角横向同步得出的 δ（推算 − 实际，mm）
    pub start_probe: bool,
    pub start_sync: Option<f32>,
    pub lateral_sync: Option<f32>,
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
        let s = match &job.kind {
            JobKind::Follow { s, .. } => Some(*s),
            JobKind::Shot { .. } => None,
        };
        Self {
            sn: job.sn,
            k: job.k,
            cam: job.cam,
            s,
            located: false,
            score: 0.0,
            ms: 0,
            error: None,
            idx: Vec::new(),
            d: Vec::new(),
            w: Vec::new(),
            st: Vec::new(),
            px: Vec::new(),
            start_probe: matches!(job.kind, JobKind::Follow { start_probe: true, .. }),
            start_sync: None,
            lateral_sync: None,
        }
    }

    pub fn failed(job: &Job, error: impl Into<String>) -> Self {
        Self { error: Some(error.into()), ..Self::empty(job) }
    }
}

/// 图像测量引擎：飞拍用 lyFlow 流程，随动用本程序卡尺。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// lyFlow 流程（模板定位 + 逐点卡尺）
    LyFlow,
    /// 本程序内置卡尺
    Native,
}

/// 测量后端：拿一帧图像，给出这一帧负责的测量点的结果。
pub trait Measurer: Send + Sync {
    fn measure(&self, job: &Job, image: &FrameImage) -> Result<Measured, String>;
}

struct NativeMeasurer;

/// 使用实际图像运行内置随动卡尺；离线复测与在线测量共享同一入口。
pub fn measure_native(job: &Job, image: &FrameImage) -> Result<Measured, String> {
    NativeMeasurer.measure(job, image)
}

impl Measurer for NativeMeasurer {
    fn measure(&self, job: &Job, image: &FrameImage) -> Result<Measured, String> {
        let JobKind::Follow { s, points, calib, start_probe } = &job.kind else {
            return Err("本程序卡尺目前只支持随动配方；飞拍配方请在系统设置里改用 lyFlow 或模拟测量".into());
        };
        let spec = job.recipe.follow.as_ref().ok_or("配方没有随动参数")?;
        let mut m = Measured::empty(job);
        caliper::measure_follow(&job.recipe, spec, calib, *s, points, image, &mut m);
        if *start_probe && spec.auto_sync {
            if let Some(d) = caliper::find_start(&job.recipe, spec, calib, *s, image) {
                // 找到了胶条起点：按修正后的胶嘴位置把这一帧重测一遍，连同此刻看得到的起点附近的点，
                // 同步前按超前位置测错的那一段就从这张图里补回来
                let s2 = *s - d;
                let pts: Vec<u32> = follow::visible(&job.recipe, spec, calib, s2).into_iter().map(|j| j as u32).collect();
                m = Measured { s: Some(s2), start_sync: Some(d), ..Measured::empty(job) };
                caliper::measure_follow(&job.recipe, spec, calib, s2, &pts, image, &mut m);
            }
        }
        Ok(m)
    }
}

/// 某种工况的配方是否用图像测量（否则模拟测量），以及用哪个引擎。
pub fn engine(app: &AppHandle, mode: InspectMode) -> Option<Engine> {
    let settings = app.state::<CycleHost>().settings();
    match mode {
        InspectMode::FlyShot => settings.vision.then_some(Engine::LyFlow),
        InspectMode::Follow => settings.follow_vision.then_some(Engine::Native),
    }
}

/// 需要整帧图像的场合：图像测量或帧录制。设置变了之后调一次。
pub fn apply_settings(app: &AppHandle) {
    let settings = app.state::<CycleHost>().settings();
    let record = settings.record != RecordMode::Off;
    app.state::<CycleHost>().camera.set_capture(settings.vision || record, settings.follow_vision || record);
}

fn run_image(app: &AppHandle, engine: Engine, job: &Job, image: &FrameImage) -> Measured {
    let r = match engine {
        Engine::Native => measure_native(job, image),
        Engine::LyFlow => vision::LyFlowMeasurer { app: app.clone() }.measure(job, image),
    };
    r.unwrap_or_else(|e| Measured::failed(job, e))
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
                let engine = engine(&app, job.recipe.mode);
                let mut m = match (job.image.clone(), engine) {
                    (Some(image), Some(engine)) => {
                        let fallback = Measured::failed(&job, "测量线程异常退出");
                        tauri::async_runtime::spawn_blocking(move || run_image(&app, engine, &job, &image)).await.unwrap_or(fallback)
                    }
                    (None, Some(_)) => Measured::failed(&job, "这一帧没有图像：相机未拷贝整帧"),
                    (_, None) => {
                        let delay = if matches!(job.kind, JobKind::Shot { .. }) { 170 + (job.k as u64 * 13) % 60 } else { 15 };
                        tokio::time::sleep(Duration::from_millis(delay)).await;
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
    if r.mode == InspectMode::Follow {
        let JobKind::Follow { points, .. } = &job.kind else { return m };
        let gap = simfollow::gap(r, job.scenario);
        m.located = true;
        m.score = 0.9;
        for &j in points {
            let s = j as f32 * r.spacing;
            let in_gap = gap.is_some_and(|(a, b)| s >= a && s <= b);
            m.idx.push(j);
            m.d.push(simfollow::offset(r, job.scenario, s) + 0.02 * noise(s));
            m.w.push(if in_gap { f32::NAN } else { simfollow::width(r, job.scenario, s) + 0.03 * noise(s + 7.0) });
            m.st.push(if in_gap { ST_GAP } else { ST_OK });
        }
        return m;
    }
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
