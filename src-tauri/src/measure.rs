//! 逐帧测量。测量后端只量不判：模拟或 lyFlow（飞拍流程），由系统设置选择；判定统一在 judge。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc::{channel, Receiver, Sender, UnboundedSender};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::cycle::{CycleHost, Input};
use crate::frame::FrameImage;
use crate::judge::PointState;
use crate::recipe::Recipe;
use crate::settings::RecordMode;
use crate::sim::Scenario;
use crate::vision;

/// 测量队列容量。满了说明测量跟不上帧率，新帧直接记为测量出错，不在内存里堆积。
pub const MEASURE_QUEUE: usize = 32;
pub const MEASURE_TIMEOUT: &str = "单帧测量超时";

/// 第 k 个拍照点的一帧，测该点负责的全部测量点。
pub struct Job {
    pub run_id: u64,
    pub cycle_id: String,
    pub shot_id: String,
    pub camera: String,
    pub bundle_hash: Option<String>,
    pub production: Option<Arc<crate::production::Prepared>>,
    pub submitted_at: Instant,
    pub timeout: Duration,
    pub image_measurement: bool,
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
    pub cycle_id: String,
    pub shot_id: String,
    pub camera: String,
    pub bundle_hash: Option<String>,
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
            cycle_id: job.cycle_id.clone(),
            shot_id: job.shot_id.clone(),
            camera: job.camera.clone(),
            bundle_hash: job.bundle_hash.clone(),
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
    app.state::<crate::production::ProductionHost>().clear();
    let settings = app.state::<CycleHost>().settings();
    let record = settings.record != RecordMode::Off;
    app.state::<CycleHost>().camera.set_capture(settings.vision || record);
}

fn run_image(_app: &AppHandle, job: &Job, image: &FrameImage) -> Measured {
    vision::LyFlowMeasurer.measure(job, image).unwrap_or_else(|e| Measured::failed(job, e))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHealthSnapshot {
    pub capacity: usize,
    pub running: usize,
    pub timed_out: usize,
    pub available_capacity: usize,
}

#[derive(Default)]
struct WorkerCounts {
    running: usize,
    timed_out: usize,
}

pub struct WorkerHealth {
    capacity: usize,
    permits: Arc<Semaphore>,
    counts: Mutex<WorkerCounts>,
}

impl WorkerHealth {
    fn new(capacity: usize) -> Self {
        Self { capacity, permits: Arc::new(Semaphore::new(capacity)), counts: Mutex::new(WorkerCounts::default()) }
    }

    pub fn snapshot(&self) -> WorkerHealthSnapshot {
        let counts = self.counts.lock().unwrap();
        WorkerHealthSnapshot {
            capacity: self.capacity,
            running: counts.running,
            timed_out: counts.timed_out,
            available_capacity: self.available_capacity(),
        }
    }

    pub fn available_capacity(&self) -> usize {
        self.permits.available_permits()
    }

    pub fn can_arm(&self) -> bool {
        self.counts.lock().unwrap().timed_out < self.capacity
    }
}

#[derive(Default)]
struct ExecutionStatus {
    started: bool,
    finished: bool,
    timed_out: bool,
}

struct Execution {
    health: Arc<WorkerHealth>,
    status: Mutex<ExecutionStatus>,
}

impl Execution {
    fn start(&self, deadline: Instant) -> bool {
        let mut status = self.status.lock().unwrap();
        if status.timed_out || Instant::now() >= deadline {
            status.finished = true;
            return false;
        }
        status.started = true;
        self.health.counts.lock().unwrap().running += 1;
        true
    }

    fn timeout(&self) {
        let mut status = self.status.lock().unwrap();
        if status.finished || status.timed_out {
            return;
        }
        status.timed_out = true;
        if status.started {
            self.health.counts.lock().unwrap().timed_out += 1;
        }
    }

    fn finish(&self) {
        let mut status = self.status.lock().unwrap();
        status.finished = true;
        let mut counts = self.health.counts.lock().unwrap();
        counts.running -= 1;
        if status.timed_out {
            counts.timed_out -= 1;
        }
    }
}

struct RunningJob {
    execution: Arc<Execution>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for RunningJob {
    fn drop(&mut self) {
        self.execution.finish();
    }
}

type JobRunner = Arc<dyn Fn(&Job) -> Measured + Send + Sync>;

fn timeout_result(job: &Job) -> Measured {
    Measured::failed(job, format!("{MEASURE_TIMEOUT}：拍照点 {} 超过 T_proc {} ms（含排队时间）", job.shot_id, job.timeout.as_millis()))
}

fn report(out: &UnboundedSender<Input>, job: &Job, mut measured: Measured) {
    measured.ms = job.submitted_at.elapsed().as_millis().min(u32::MAX as u128) as u32;
    let _ = out.send(Input::Measured(measured));
}

async fn dispatch(mut rx: Receiver<Job>, health: Arc<WorkerHealth>, runner: JobRunner, out: UnboundedSender<Input>) {
    while let Some(job) = rx.recv().await {
        let Some(deadline) = job.submitted_at.checked_add(job.timeout) else {
            report(&out, &job, Measured::failed(&job, "测量超时时间超出可用范围"));
            continue;
        };
        let deadline = tokio::time::Instant::from_std(deadline);
        if tokio::time::Instant::now() >= deadline {
            report(&out, &job, timeout_result(&job));
            continue;
        }
        let permit = tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => {
                report(&out, &job, timeout_result(&job));
                continue;
            }
            permit = health.permits.clone().acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => {
                    report(&out, &job, Measured::failed(&job, "测量工作线程已关闭"));
                    continue;
                }
            }
        };
        let job = Arc::new(job);
        let execution = Arc::new(Execution { health: health.clone(), status: Mutex::new(ExecutionStatus::default()) });
        let (worker_job, worker_execution, runner) = (job.clone(), execution.clone(), runner.clone());
        let task = tokio::task::spawn_blocking(move || {
            if !worker_execution.start(deadline.into_std()) {
                return (timeout_result(&worker_job), Instant::now());
            }
            let _running = RunningJob { execution: worker_execution, _permit: permit };
            let measured = runner(&worker_job);
            (measured, Instant::now())
        });
        let out = out.clone();
        tokio::spawn(async move {
            let result = match tokio::time::timeout_at(deadline, task).await {
                Ok(Ok((measured, completed))) if tokio::time::Instant::from_std(completed) <= deadline => measured,
                Ok(Err(_)) => Measured::failed(&job, "测量线程异常退出"),
                _ => {
                    execution.timeout();
                    timeout_result(&job)
                }
            };
            report(&out, &job, result);
        });
    }
}

/// 测量工作线程：有界队列，同时最多 `permits` 帧在测。
pub fn spawn_worker(app: AppHandle, out: UnboundedSender<Input>) -> (Sender<Job>, Arc<WorkerHealth>) {
    let (tx, rx) = channel::<Job>(MEASURE_QUEUE);
    let capacity = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(2, 4);
    let health = Arc::new(WorkerHealth::new(capacity));
    let runner: JobRunner = Arc::new(move |job| match (job.image.as_deref(), job.image_measurement) {
        (Some(image), true) => run_image(&app, job, image),
        (None, true) => Measured::failed(job, "这一帧没有图像：相机未拷贝整帧"),
        (_, false) => {
            std::thread::sleep(Duration::from_millis(170 + (job.k as u64 * 13) % 60));
            simulate(job)
        }
    });
    tauri::async_runtime::spawn(dispatch(rx, health.clone(), runner, out));
    (tx, health)
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
    // 超差场景：最后一段 30% 处横向偏出约 3 mm、宽约 3 mm（局部超差，在允许长度内）
    let bump = (job.scenario == Scenario::Excursion).then(|| r.segments.last()).flatten().map(|g| (g.shot, g.length(r.spacing) * 0.3));
    m.located = located;
    m.score = if located { 0.91 + ((job.k * 7) % 5) as f32 / 100.0 } else { 0.38 };
    for j in r.owned_points(job.k) {
        let seg = &r.segments[r.points.seg[j] as usize];
        let s = seg.s(j, r.spacing);
        let t = j as f32 * r.spacing;
        let mut d = 0.3 * (s / 23.0).sin() + 0.1 * (s / 4.7 + 1.3).sin() + 0.05 * noise(t);
        let w = 4.0 + 0.25 * (s / 17.0 + 0.6).sin() + 0.06 * noise(t + 7.0);
        if let Some((_, c)) = bump.filter(|(k, _)| *k == job.k) {
            d += 3.0 * (-((s - c) / 1.5).powi(2)).exp();
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
        m.w.push(w);
        m.st.push(st);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Condvar;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    #[derive(Default)]
    struct Gate {
        open: Mutex<bool>,
        wake: Condvar,
    }

    impl Gate {
        fn wait(&self) {
            drop(self.wake.wait_while(self.open.lock().unwrap(), |open| !*open).unwrap());
        }

        fn release(&self) {
            *self.open.lock().unwrap() = true;
            self.wake.notify_all();
        }
    }

    struct ReleaseOnDrop(Arc<Gate>);

    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
        }
    }

    fn job(k: usize, timeout: Duration) -> Job {
        Job {
            run_id: 7,
            cycle_id: "cycle-7".into(),
            shot_id: format!("P{}", k + 1),
            camera: "cam1".into(),
            bundle_hash: Some("bundle-7".into()),
            production: None,
            submitted_at: Instant::now(),
            timeout,
            image_measurement: false,
            sn: 123,
            k,
            cam: 0,
            recipe: crate::recipe::builtin().remove(0),
            scenario: Scenario::Normal,
            image: None,
        }
    }

    fn worker(capacity: usize, runner: JobRunner) -> (Sender<Job>, Arc<WorkerHealth>, UnboundedReceiver<Input>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = channel(MEASURE_QUEUE);
        let (out, results) = unbounded_channel();
        let health = Arc::new(WorkerHealth::new(capacity));
        let task = tokio::spawn(dispatch(rx, health.clone(), runner, out));
        (tx, health, results, task)
    }

    async fn next_result(results: &mut UnboundedReceiver<Input>) -> Measured {
        match tokio::time::timeout(Duration::from_secs(2), results.recv()).await.unwrap().unwrap() {
            Input::Measured(measured) => measured,
            _ => panic!("unexpected worker output"),
        }
    }

    async fn wait_health(health: &WorkerHealth, expected: WorkerHealthSnapshot) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while health.snapshot() != expected {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }).await.unwrap();
    }

    #[test]
    fn measurements_preserve_cycle_shot_camera_and_bundle_identity() {
        let job = job(0, Duration::from_secs(1));
        for measured in [Measured::empty(&job), Measured::failed(&job, "失败"), simulate(&job)] {
            assert_eq!(measured.run_id, 7);
            assert_eq!(measured.cycle_id, "cycle-7");
            assert_eq!(measured.shot_id, "P1");
            assert_eq!(measured.camera, "cam1");
            assert_eq!(measured.bundle_hash.as_deref(), Some("bundle-7"));
            let json = serde_json::to_value(measured).unwrap();
            assert_eq!(json["cycleId"], "cycle-7");
            assert_eq!(json["shotId"], "P1");
            assert_eq!(json["bundleHash"], "bundle-7");
            assert!(json.get("runId").is_none());
        }
    }

    #[test]
    fn timeout_and_completion_race_never_leave_stale_worker_counts() {
        let health = Arc::new(WorkerHealth::new(1));
        for _ in 0..64 {
            let execution = Arc::new(Execution { health: health.clone(), status: Mutex::new(ExecutionStatus::default()) });
            let permit = health.permits.clone().try_acquire_owned().unwrap();
            assert!(execution.start(Instant::now() + Duration::from_secs(1)));
            let running = RunningJob { execution: execution.clone(), _permit: permit };
            let barrier = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    barrier.wait();
                    execution.timeout();
                    execution.timeout();
                });
                scope.spawn(|| {
                    barrier.wait();
                    drop(running);
                });
            });
            assert_eq!(health.snapshot(), WorkerHealthSnapshot { capacity: 1, running: 0, timed_out: 0, available_capacity: 1 });
            assert!(health.can_arm());
        }
    }

    #[test]
    fn timed_out_or_expired_job_cannot_start_from_the_blocking_queue() {
        let health = Arc::new(WorkerHealth::new(1));
        let execution = Arc::new(Execution { health: health.clone(), status: Mutex::new(ExecutionStatus::default()) });
        let permit = health.permits.clone().try_acquire_owned().unwrap();
        execution.timeout();
        assert_eq!(health.snapshot(), WorkerHealthSnapshot { capacity: 1, running: 0, timed_out: 0, available_capacity: 0 });
        assert!(!execution.start(Instant::now() + Duration::from_secs(1)));
        drop(permit);
        execution.timeout();
        assert_eq!(health.snapshot(), WorkerHealthSnapshot { capacity: 1, running: 0, timed_out: 0, available_capacity: 1 });

        let execution = Arc::new(Execution { health: health.clone(), status: Mutex::new(ExecutionStatus::default()) });
        assert!(!execution.start(Instant::now()));
        execution.timeout();
        assert_eq!(health.snapshot(), WorkerHealthSnapshot { capacity: 1, running: 0, timed_out: 0, available_capacity: 1 });
    }

    #[tokio::test]
    async fn timed_out_blocking_job_keeps_capacity_and_late_completion_is_not_reported_twice() {
        let gate = Arc::new(Gate::default());
        let release = ReleaseOnDrop(gate.clone());
        let entered = Arc::new(AtomicUsize::new(0));
        let calls = entered.clone();
        let (tx, health, mut results, dispatcher) = worker(1, Arc::new(move |job| {
            calls.fetch_add(1, Ordering::SeqCst);
            if job.k == 0 {
                gate.wait();
            }
            Measured::empty(job)
        }));
        tx.send(job(0, Duration::from_millis(200))).await.unwrap();
        wait_health(&health, WorkerHealthSnapshot { capacity: 1, running: 1, timed_out: 0, available_capacity: 0 }).await;
        assert!(health.can_arm());
        let result = next_result(&mut results).await;
        assert!(result.error.unwrap().starts_with(MEASURE_TIMEOUT));
        assert_eq!((result.cycle_id.as_str(), result.shot_id.as_str()), ("cycle-7", "P1"));
        assert!(result.ms >= 200);
        assert_eq!(health.snapshot(), WorkerHealthSnapshot { capacity: 1, running: 1, timed_out: 1, available_capacity: 0 });
        assert!(!health.can_arm());

        tx.send(job(1, Duration::from_millis(30))).await.unwrap();
        tx.send(job(2, Duration::from_millis(30))).await.unwrap();
        for k in [1, 2] {
            let result = next_result(&mut results).await;
            assert_eq!(result.k, k);
            assert!(result.error.unwrap().starts_with(MEASURE_TIMEOUT));
        }
        assert_eq!(entered.load(Ordering::SeqCst), 1);
        assert_eq!(health.snapshot().timed_out, 1);
        release.0.release();
        wait_health(&health, WorkerHealthSnapshot { capacity: 1, running: 0, timed_out: 0, available_capacity: 1 }).await;
        assert!(health.can_arm());
        assert!(results.try_recv().is_err());
        tx.send(job(3, Duration::from_secs(1))).await.unwrap();
        let recovered = next_result(&mut results).await;
        assert_eq!(recovered.k, 3);
        assert!(recovered.error.is_none());
        assert_eq!(entered.load(Ordering::SeqCst), 2);
        drop(tx);
        dispatcher.await.unwrap();
        assert!(results.try_recv().is_err());
    }

    #[tokio::test]
    async fn normal_running_capacity_and_partial_timeouts_do_not_block_arming() {
        let gates = [Arc::new(Gate::default()), Arc::new(Gate::default())];
        let release = [ReleaseOnDrop(gates[0].clone()), ReleaseOnDrop(gates[1].clone())];
        let (tx, health, mut results, dispatcher) = worker(2, Arc::new(move |job| {
            gates[job.k].wait();
            Measured::empty(job)
        }));
        tx.send(job(0, Duration::from_millis(200))).await.unwrap();
        tx.send(job(1, Duration::from_secs(1))).await.unwrap();
        wait_health(&health, WorkerHealthSnapshot { capacity: 2, running: 2, timed_out: 0, available_capacity: 0 }).await;
        assert!(health.can_arm());
        assert_eq!(next_result(&mut results).await.k, 0);
        assert_eq!(health.snapshot(), WorkerHealthSnapshot { capacity: 2, running: 2, timed_out: 1, available_capacity: 0 });
        assert!(health.can_arm());
        release[1].0.release();
        let completed = next_result(&mut results).await;
        assert_eq!(completed.k, 1);
        assert!(completed.error.is_none());
        wait_health(&health, WorkerHealthSnapshot { capacity: 2, running: 1, timed_out: 1, available_capacity: 1 }).await;
        release[0].0.release();
        wait_health(&health, WorkerHealthSnapshot { capacity: 2, running: 0, timed_out: 0, available_capacity: 2 }).await;
        drop(tx);
        dispatcher.await.unwrap();
        assert!(results.try_recv().is_err());
    }

    #[tokio::test]
    async fn jobs_expired_in_queue_never_enter_the_blocking_executor() {
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = calls.clone();
        let (tx, health, mut results, dispatcher) = worker(1, Arc::new(move |job| {
            entered.fetch_add(1, Ordering::SeqCst);
            Measured::empty(job)
        }));
        let mut expired = job(0, Duration::from_millis(10));
        expired.submitted_at = Instant::now() - Duration::from_millis(100);
        tx.send(expired).await.unwrap();
        let result = next_result(&mut results).await;
        assert!(result.error.unwrap().starts_with(MEASURE_TIMEOUT));
        assert!(result.ms >= 100);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(health.snapshot(), WorkerHealthSnapshot { capacity: 1, running: 0, timed_out: 0, available_capacity: 1 });
        drop(tx);
        dispatcher.await.unwrap();
    }

    #[tokio::test]
    async fn panicking_executor_reports_failure_and_releases_real_capacity() {
        let (tx, health, mut results, dispatcher) = worker(1, Arc::new(|job| {
            assert_ne!(job.k, 0, "controlled measurement panic");
            Measured::empty(job)
        }));
        tx.send(job(0, Duration::from_secs(1))).await.unwrap();
        let failed = next_result(&mut results).await;
        assert_eq!(failed.error.as_deref(), Some("测量线程异常退出"));
        assert_eq!(failed.bundle_hash.as_deref(), Some("bundle-7"));
        wait_health(&health, WorkerHealthSnapshot { capacity: 1, running: 0, timed_out: 0, available_capacity: 1 }).await;
        tx.send(job(1, Duration::from_secs(1))).await.unwrap();
        assert!(next_result(&mut results).await.error.is_none());
        drop(tx);
        dispatcher.await.unwrap();
        assert!(health.can_arm());
    }
}
