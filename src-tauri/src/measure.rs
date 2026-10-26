//! 逐帧测量。测量后端只量不判：模拟或 lyFlow（飞拍流程），由系统设置选择；判定在 burst_judge。
//!
//! 段配方的 `FrameJob`：每台相机每一帧独立测，输出 `FrameMetrics`（D-SB-12）；排队、并发、超时与异常回收见 `dispatch_tasks`。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::burst_judge::{FrameMetrics, FrameState};
use crate::cycle::CycleHost;
use crate::frame::FrameImage;
use crate::recipe::Recipe;
use crate::settings::RecordMode;
use crate::offline::Scenario;
use crate::vision;

/// 段配方的测量队列容量（D-SB-32，决策记录第七节的 MEASURE_QUEUE）：图像内存已由 FramePool 每相机预算封顶，
/// 条数只防失控；满时该帧记 Unmeasured，不裁帧、不判失败。
pub const FRAME_MEASURE_QUEUE: usize = 1024;
/// 段配方的测量线程上限（P1 实测回填，决策记录 7.1）；运行时取 min(4, 逻辑核 − 2)，至少 1。
pub const MEASURE_WORKERS: usize = 4;
pub const MEASURE_TIMEOUT: &str = "单帧测量超时";

/// 需要整帧图像的场合：图像测量或帧录制。设置变了之后调一次。
pub fn apply_settings(app: &AppHandle) {
    app.state::<crate::production::ProductionHost>().clear();
    let record = crate::site::record_mode(app) != RecordMode::Off;
    app.state::<CycleHost>().camera.set_capture(crate::site::image_measurement(app) || record);
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
    entered_at: Option<Instant>,
    finished: bool,
    timed_out: bool,
}

struct Execution {
    health: Arc<WorkerHealth>,
    status: Mutex<ExecutionStatus>,
}

fn clock() -> Instant {
    tokio::time::Instant::now().into_std()
}

impl Execution {
    fn start(&self, deadline: Instant) -> bool {
        let entered_at = clock();
        let mut status = self.status.lock().unwrap();
        status.entered_at = Some(entered_at);
        if status.timed_out || clock() >= deadline {
            status.finished = true;
            return false;
        }
        status.started = true;
        self.health.counts.lock().unwrap().running += 1;
        true
    }

    fn entered_at(&self) -> Option<Instant> {
        self.status.lock().unwrap().entered_at
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

/// 一个测量任务：排队、并发、超时与执行线程异常的处理见 `dispatch_tasks`，这里只定义怎样记结果。
trait Task: Send + Sync + 'static {
    type Output: Send + 'static;
    fn submitted_at(&self) -> Instant;
    fn timeout(&self) -> Duration;
    /// 没测成：超时时间溢出、工作线程关闭、执行线程异常退出
    fn failed(&self, error: &str) -> Self::Output;
    /// 排队加执行超过 T_proc
    fn timed_out(&self) -> Self::Output;
    /// 执行器按时跑完：记执行耗时
    fn executed(&self, output: &mut Self::Output, engine_ms: u32);
    /// 进了执行器才有排队耗时
    fn set_queue_ms(output: &mut Self::Output, queue_ms: Option<u32>);
    /// 提交到交出的总耗时
    fn set_total_ms(output: &mut Self::Output, ms: u32);
}

type Runner<T> = Arc<dyn Fn(&T) -> <T as Task>::Output + Send + Sync>;
type Sink<O> = Arc<dyn Fn(O) + Send + Sync>;

fn duration_ms(duration: Duration) -> u32 {
    duration.as_millis().min(u32::MAX as u128) as u32
}

fn report<T: Task>(sink: &Sink<T::Output>, task: &T, mut output: T::Output) {
    T::set_total_ms(&mut output, duration_ms(clock().saturating_duration_since(task.submitted_at())));
    sink(output);
}

/// 有界队列里的任务逐个拿执行许可（同时最多 `health.capacity` 个在测），每个任务恰好交出一个结果：
/// 排队或执行超过时限记超时，执行线程异常记失败；超时仍在跑的执行器继续占着许可，直到真正返回（`WorkerHealth`）。
async fn dispatch_tasks<T: Task>(mut rx: Receiver<T>, health: Arc<WorkerHealth>, runner: Runner<T>, sink: Sink<T::Output>) {
    while let Some(job) = rx.recv().await {
        let Some(deadline) = job.submitted_at().checked_add(job.timeout()) else {
            report(&sink, &job, job.failed("测量超时时间超出可用范围"));
            continue;
        };
        let deadline = tokio::time::Instant::from_std(deadline);
        if tokio::time::Instant::now() >= deadline {
            report(&sink, &job, job.timed_out());
            continue;
        }
        let permit = tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => {
                report(&sink, &job, job.timed_out());
                continue;
            }
            permit = health.permits.clone().acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => {
                    report(&sink, &job, job.failed("测量工作线程已关闭"));
                    continue;
                }
            }
        };
        let job = Arc::new(job);
        let execution = Arc::new(Execution { health: health.clone(), status: Mutex::new(ExecutionStatus::default()) });
        let (worker_job, worker_execution, runner) = (job.clone(), execution.clone(), runner.clone());
        let task = tokio::task::spawn_blocking(move || {
            if !worker_execution.start(deadline.into_std()) {
                return (worker_job.timed_out(), clock());
            }
            let entered_at = worker_execution.entered_at().unwrap();
            let _running = RunningJob { execution: worker_execution, _permit: permit };
            let mut output = runner(&worker_job);
            let completed = clock();
            worker_job.executed(&mut output, duration_ms(completed.saturating_duration_since(entered_at)));
            (output, completed)
        });
        let sink = sink.clone();
        tokio::spawn(async move {
            let mut result = match tokio::time::timeout_at(deadline, task).await {
                Ok(Ok((output, completed))) if tokio::time::Instant::from_std(completed) <= deadline => output,
                Ok(Err(_)) => job.failed("测量线程异常退出"),
                _ => {
                    execution.timeout();
                    job.timed_out()
                }
            };
            T::set_queue_ms(&mut result, execution.entered_at().map(|entered| duration_ms(entered.saturating_duration_since(job.submitted_at()))));
            report(&sink, &*job, result);
        });
    }
}

/// 段配方一台相机的一帧。
pub struct FrameJob {
    pub cycle_id: String,
    pub bundle_id: Option<String>,
    pub production: Option<Arc<crate::production::Prepared>>,
    pub candidate: Option<Arc<crate::candidate::Candidate>>,
    /// 计划相机下标（`Recipe.inspect` 的顺序）
    pub camera_idx: usize,
    pub camera: String,
    /// 帧组序号：frame_counter − 该相机基线，从 1 起
    pub seq: u64,
    /// 用第几段（`PathSegment.index`）的走廊测（D-SB-45）
    pub segment: u32,
    pub image: Option<Arc<FrameImage>>,
    pub image_measurement: bool,
    pub recipe: Arc<Recipe>,
    pub scenario: Scenario,
    pub submitted_at: Instant,
    pub timeout: Duration,
}

/// 一帧没测成的原因（D-SB-13：都只记检测不完整，不改 OK/NG）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeasureError {
    /// 测量队列满：测量跟不上帧率
    QueueFull,
    /// 排队加执行超过 T_proc
    Timeout,
    /// 测量失败：lyFlow 运行失败、输出不合契约、没有图像、发布包不一致等
    Failed(String),
}

impl MeasureError {
    /// 写进帧指标的状态：队满、超时是没测（Unmeasured），其余是测量失败（Error）。
    pub fn state(&self) -> FrameState {
        match self {
            MeasureError::QueueFull | MeasureError::Timeout => FrameState::Unmeasured,
            MeasureError::Failed(_) => FrameState::Error,
        }
    }

    pub fn message(&self) -> String {
        match self {
            MeasureError::QueueFull => "测量队列已满：测量跟不上帧率".into(),
            MeasureError::Timeout => format!("{MEASURE_TIMEOUT}（含排队时间）"),
            MeasureError::Failed(why) => why.clone(),
        }
    }
}

/// 一帧的测量结果。Ok 的帧指标可能是 Measured 或 OutOfCorridor（胶条走出走廊）。
#[derive(Clone, Debug)]
pub struct FrameMeasured {
    pub cycle_id: String,
    pub camera_idx: usize,
    pub seq: u64,
    /// 用第几段的走廊测
    pub segment: u32,
    pub result: Result<FrameMetrics, MeasureError>,
    /// 提交到交出的总耗时
    pub ms: u32,
    pub queue_ms: Option<u32>,
    pub engine_ms: Option<u32>,
}

impl FrameMeasured {
    fn new(job: &FrameJob, result: Result<FrameMetrics, MeasureError>) -> Self {
        Self {
            cycle_id: job.cycle_id.clone(),
            camera_idx: job.camera_idx,
            seq: job.seq,
            segment: job.segment,
            result,
            ms: 0,
            queue_ms: None,
            engine_ms: None,
        }
    }

    /// 写进整件的帧指标：没测成的帧按原因记 Unmeasured 或 Error，原因放 `error`，数值为空。
    pub fn metrics(&self) -> FrameMetrics {
        match &self.result {
            Ok(metrics) => metrics.clone(),
            Err(error) => FrameMetrics {
                camera: self.camera_idx,
                seq: self.seq,
                state: error.state(),
                error: Some(error.message()),
                width_min: None,
                width_max: None,
                width_mean: None,
                widths: Vec::new(),
                spacing_mm: 0.0,
                gap_max_mm: 0.0,
                corridor: Some(self.segment),
                present_ratio: 0.0,
                x_mm: None,
            },
        }
    }
}

impl Task for FrameJob {
    type Output = FrameMeasured;
    fn submitted_at(&self) -> Instant { self.submitted_at }
    fn timeout(&self) -> Duration { self.timeout }
    fn failed(&self, error: &str) -> FrameMeasured { FrameMeasured::new(self, Err(MeasureError::Failed(error.into()))) }
    fn timed_out(&self) -> FrameMeasured { FrameMeasured::new(self, Err(MeasureError::Timeout)) }
    fn executed(&self, measured: &mut FrameMeasured, engine_ms: u32) { measured.engine_ms = Some(engine_ms); }
    fn set_queue_ms(measured: &mut FrameMeasured, queue_ms: Option<u32>) { measured.queue_ms = queue_ms; }
    fn set_total_ms(measured: &mut FrameMeasured, ms: u32) { measured.ms = ms; }
}

/// 帧测量结果的去处（节拍的输入通道 `cycle::Input::FrameMeasured`）。
pub type FrameSink = Sink<FrameMeasured>;

/// 段配方的测量池：排队与超时处理见 `dispatch_tasks`，线程数取 `frame_workers`，队列 `FRAME_MEASURE_QUEUE`。
pub struct FramePool {
    tx: Sender<FrameJob>,
    sink: FrameSink,
    health: Arc<WorkerHealth>,
}

impl FramePool {
    /// 提交一帧，不阻塞相机回调。每个提交的帧恰好得到一个 `FrameMeasured`：队满时当场交出 QueueFull（记 Unmeasured），
    /// 不裁帧、不静默丢帧（D-SB-32）。
    pub fn submit(&self, job: FrameJob) {
        match self.tx.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(job)) => report(&self.sink, &job, FrameMeasured::new(&job, Err(MeasureError::QueueFull))),
            Err(TrySendError::Closed(job)) => report(&self.sink, &job, job.failed("测量工作线程已关闭")),
        }
    }

    pub fn health(&self) -> &Arc<WorkerHealth> {
        &self.health
    }
}

/// 段配方测量线程数：min(MEASURE_WORKERS, 逻辑核 − 2)，至少 1（决策记录 7.1：给相机回调、界面与写盘留两个核）。
fn frame_workers(logical: usize) -> usize {
    MEASURE_WORKERS.min(logical.saturating_sub(2)).max(1)
}

/// 启动段配方的测量池。
pub fn spawn_frame_worker(sink: FrameSink) -> FramePool {
    let (tx, rx) = channel::<FrameJob>(FRAME_MEASURE_QUEUE);
    let health = Arc::new(WorkerHealth::new(frame_workers(std::thread::available_parallelism().map_or(1, |n| n.get()))));
    let runner: Runner<FrameJob> = Arc::new(|job| FrameMeasured::new(job, run_frame(job)));
    tauri::async_runtime::spawn(dispatch_tasks(rx, health.clone(), runner, sink.clone()));
    FramePool { tx, sink, health }
}

fn run_frame(job: &FrameJob) -> Result<FrameMetrics, MeasureError> {
    match (job.image.as_deref(), job.image_measurement) {
        (Some(image), true) => vision::LyFlowMeasurer.measure_frame(job, image).map(|m| m.metrics).map_err(MeasureError::Failed),
        (None, true) => Err(MeasureError::Failed("这一帧没有图像：相机未拷贝整帧".into())),
        (_, false) => {
            std::thread::sleep(Duration::from_millis(5 + job.seq % 5));
            Ok(simulate_frame(
                job.scenario,
                job.camera_idx,
                job.seq,
                job.segment,
                &job.recipe,
            ))
        }
    }
}

/// 段配方的模拟测量（不看图像）：沿这台相机在第 `corridor` 段的走廊按站距取站（未示教时 40 站），胶宽约 4 mm、x 小幅摆动的合格帧。
/// 按配方名义剖面（与离线虚拟产线发脉冲用的同一剖面，`offline::line::nominal_frames`）找到这一帧所在的段：
/// 弯道帧胶宽略宽、胶中心 x 随段内进度拱形偏出（峰值 1.5 mm），与合成图里弯道胶条绕胶嘴转动一致；PC 判定照常按未分段。
/// 断胶工况让第 1 台相机第 2 帧中部缺一段（比该段允许的断口长 2 mm），超差工况让同一帧 3 站胶宽超上限 1 mm
/// （超差长度在默认允许范围内）。脉冲串类工况（缺帧、波动、提前终止等）不改指标。
pub(crate) fn simulate_frame(
    scenario: Scenario,
    camera_idx: usize,
    seq: u64,
    corridor: u32,
    recipe: &Recipe,
) -> FrameMetrics {
    let spacing = f64::from(recipe.spacing);
    let n = recipe
        .inspect
        .get(camera_idx)
        .and_then(|inspect| {
            let path = inspect
                .segment_corridor(corridor)
                .map(|s| s.corridor.as_slice())
                .unwrap_or_default();
            let length: f64 = path
                .windows(2)
                .map(|w| f64::from((w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])))
                .sum();
            inspect
                .mm_per_px
                .map(|scale| (length * scale / spacing).floor() as usize + 1)
                .filter(|&n| n >= 3)
        })
        .unwrap_or(40);
    let profile = crate::offline::line::nominal_frames(
        &recipe.path_segments,
        recipe.expected_total(),
        crate::offline::line::SIM_HZ,
    );
    let (segment, corner, progress) = crate::offline::line::locate(&profile, seq).map_or(
        (recipe.path_segments.first(), false, 0.0),
        |(i, kind, p)| {
            (
                recipe.path_segments.get(i),
                kind == crate::plan::SegmentKind::Corner,
                f64::from(p),
            )
        },
    );
    let base = segment.map_or(4.0, |s| s.checks.width.nominal_mm) + if corner { 0.3 } else { 0.0 };
    let marked = camera_idx == 0 && seq == 2;
    let gap = match (scenario, segment) {
        (Scenario::Gap, Some(s)) if marked => (((s.checks.gap.max_gap_mm + 2.0) / spacing).ceil() as usize).min(n),
        _ => 0,
    };
    let gap_from = (n - gap) / 2;
    let widths: Vec<Option<f64>> = (0..n).map(|i| {
        if (gap_from..gap_from + gap).contains(&i) { return None; }
        let s = i as f64 * spacing;
        let mut w = base + 0.25 * ((s + seq as f64 * 3.1 + camera_idx as f64) / 17.0).sin();
        if scenario == Scenario::Excursion && marked && (n / 3..n / 3 + 3).contains(&i) {
            w = segment.map_or(base + 2.5, |s| s.checks.width.nominal_mm + s.checks.width.tol_upper + 1.0);
        }
        Some(w)
    }).collect();
    let drift = if corner { 1.5 * (std::f64::consts::PI * progress).sin() } else { 0.0 };
    let present: Vec<f64> = widths.iter().flatten().copied().collect();
    FrameMetrics {
        camera: camera_idx,
        seq,
        state: FrameState::Measured,
        error: None,
        width_min: present.iter().copied().reduce(f64::min),
        width_max: present.iter().copied().reduce(f64::max),
        width_mean: (!present.is_empty()).then(|| present.iter().sum::<f64>() / present.len() as f64),
        widths: widths.iter().map(|w| w.map(|w| w as f32)).collect(),
        spacing_mm: spacing,
        gap_max_mm: gap as f64 * spacing,
        corridor: Some(corridor),
        present_ratio: present.len() as f64 / n as f64,
        x_mm: Some(0.3 * (seq as f64 / 5.0 + camera_idx as f64).sin() + drift),
    }
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

    async fn wait_health(health: &WorkerHealth, expected: WorkerHealthSnapshot) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while health.snapshot() != expected {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }

    fn frame_job(recipe: &Arc<Recipe>, seq: u64) -> FrameJob {
        frame_job_within(recipe, seq, Duration::from_secs(5))
    }

    fn frame_job_within(recipe: &Arc<Recipe>, seq: u64, timeout: Duration) -> FrameJob {
        FrameJob {
            cycle_id: "cycle-7".into(),
            bundle_id: None,
            production: None,
            candidate: None,
            camera_idx: 0,
            camera: "cam1".into(),
            segment: 1,
            seq,
            image: None,
            image_measurement: false,
            recipe: recipe.clone(),
            scenario: Scenario::Normal,
            submitted_at: clock(),
            timeout,
        }
    }

    async fn next_frame(results: &mut UnboundedReceiver<FrameMeasured>) -> FrameMeasured {
        tokio::time::timeout(Duration::from_secs(2), results.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_blocking_frame_keeps_capacity_and_late_completion_is_not_reported_twice() {
        // 防的回归：执行器卡住超过 T_proc 时这一帧没按时交出超时、超时仍在跑的执行器提前放掉执行许可（同时在测的帧超过上限）、
        // 迟到返回后同一帧再交出一次结果，或排队已超时的帧仍进执行器
        let gate = Arc::new(Gate::default());
        let release = ReleaseOnDrop(gate.clone());
        let entered = Arc::new(AtomicUsize::new(0));
        let calls = entered.clone();
        let (blocked_tx, mut blocked) = unbounded_channel();
        let recipe = Arc::new(crate::recipe::samples_burst().build().unwrap());
        let (tx, rx) = channel(FRAME_MEASURE_QUEUE);
        let (out, mut results) = unbounded_channel();
        let health = Arc::new(WorkerHealth::new(1));
        let sink: FrameSink = Arc::new(move |measured| {
            let _ = out.send(measured);
        });
        let runner: Runner<FrameJob> = Arc::new(move |job| {
            calls.fetch_add(1, Ordering::SeqCst);
            if job.seq == 1 {
                let _ = blocked_tx.send(());
                gate.wait();
            }
            FrameMeasured::new(
                job,
                Ok(simulate_frame(
                    job.scenario,
                    job.camera_idx,
                    job.seq,
                    job.segment,
                    &job.recipe,
                )),
            )
        });
        let dispatcher = tokio::spawn(dispatch_tasks(rx, health.clone(), runner, sink));
        tx.send(frame_job_within(&recipe, 1, Duration::from_millis(200)))
            .await
            .unwrap();
        blocked.recv().await.unwrap();
        assert_eq!(health.snapshot(), WorkerHealthSnapshot { capacity: 1, running: 1, timed_out: 0, available_capacity: 0 });
        assert!(health.can_arm());
        tokio::time::advance(Duration::from_millis(200)).await;
        let result = next_frame(&mut results).await;
        assert_eq!(
            (result.seq, result.result.clone().unwrap_err()),
            (1, MeasureError::Timeout)
        );
        assert!(result.ms >= 200);
        assert!(result.queue_ms.is_some());
        assert!(result.engine_ms.is_none());
        assert_eq!(health.snapshot(), WorkerHealthSnapshot { capacity: 1, running: 1, timed_out: 1, available_capacity: 0 });
        assert!(!health.can_arm());

        let watchdog = release.0.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(30));
            watchdog.release();
        });
        for seq in [2, 3] {
            tx.send(frame_job_within(&recipe, seq, Duration::from_millis(30)))
                .await
                .unwrap();
        }
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(30)).await;
        for seq in [2, 3] {
            let result = next_frame(&mut results).await;
            assert_eq!(
                (result.seq, result.result.clone().unwrap_err()),
                (seq, MeasureError::Timeout)
            );
            assert!(result.queue_ms.is_none());
            assert!(result.engine_ms.is_none());
        }
        assert_eq!(entered.load(Ordering::SeqCst), 1);
        assert_eq!(health.snapshot().timed_out, 1);
        release.0.release();
        wait_health(&health, WorkerHealthSnapshot { capacity: 1, running: 0, timed_out: 0, available_capacity: 1 }).await;
        assert!(health.can_arm());
        assert!(results.try_recv().is_err());
        tx.send(frame_job_within(&recipe, 4, Duration::from_secs(1)))
            .await
            .unwrap();
        let recovered = next_frame(&mut results).await;
        assert_eq!(recovered.seq, 4);
        assert_eq!(recovered.metrics().state, FrameState::Measured);
        assert!(recovered.queue_ms.is_some());
        assert!(recovered.engine_ms.is_some());
        assert_eq!(entered.load(Ordering::SeqCst), 2);
        drop(tx);
        dispatcher.await.unwrap();
        assert!(results.try_recv().is_err());
    }

    #[tokio::test]
    async fn full_frame_queue_reports_the_rejected_frame_as_unmeasured_and_still_measures_every_queued_frame() {
        // 防的回归（规格 §0 不裁帧、D-SB-32）：段配方测量队满时帧被静默丢掉（整件等不到它的结果）、记成测量失败，
        // 或被拒的帧占住队列让后面排着的帧测不到
        let gate = Arc::new(Gate::default());
        let release = ReleaseOnDrop(gate.clone());
        let recipe = Arc::new(crate::recipe::samples_burst().build().unwrap());
        let (tx, rx) = channel(2);
        let (out, mut results) = unbounded_channel();
        let health = Arc::new(WorkerHealth::new(1));
        let sink: FrameSink = Arc::new(move |measured| { let _ = out.send(measured); });
        let blocked = gate.clone();
        let runner: Runner<FrameJob> = Arc::new(move |job| {
            blocked.wait();
            FrameMeasured::new(
                job,
                Ok(simulate_frame(
                    job.scenario,
                    job.camera_idx,
                    job.seq,
                    job.segment,
                    &job.recipe,
                )),
            )
        });
        let pool = FramePool { tx, sink: sink.clone(), health: health.clone() };
        let dispatcher = tokio::spawn(dispatch_tasks(rx, health.clone(), runner, sink));
        // 帧 1 进了执行器卡住，帧 2 等执行许可，帧 3、4 排满队列，帧 5 被拒
        pool.submit(frame_job(&recipe, 1));
        wait_health(&health, WorkerHealthSnapshot { capacity: 1, running: 1, timed_out: 0, available_capacity: 0 }).await;
        pool.submit(frame_job(&recipe, 2));
        tokio::time::timeout(Duration::from_secs(2), async {
            while pool.tx.capacity() != 2 { tokio::time::sleep(Duration::from_millis(1)).await; }
        }).await.unwrap();
        for seq in 3..=5 { pool.submit(frame_job(&recipe, seq)); }
        let rejected = next_frame(&mut results).await;
        assert_eq!((rejected.seq, rejected.result.clone().unwrap_err()), (5, MeasureError::QueueFull));
        assert_eq!(rejected.metrics().state, FrameState::Unmeasured);
        release.0.release();
        let mut measured: Vec<u64> = Vec::new();
        for _ in 1..=4 {
            let frame = next_frame(&mut results).await;
            assert_eq!(frame.metrics().state, FrameState::Measured, "帧 {}", frame.seq);
            measured.push(frame.seq);
        }
        measured.sort();
        assert_eq!(measured, [1, 2, 3, 4]);
        drop(pool);
        dispatcher.await.unwrap();
        assert!(results.try_recv().is_err());
    }

}
