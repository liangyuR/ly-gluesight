//! 逐帧测量。测量后端只量不判：模拟或 lyFlow（飞拍流程），由系统设置选择；判定统一在 judge。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc::{channel, Receiver, Sender, UnboundedSender};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::cycle::{CycleHost, Input};
use crate::frame::FrameImage;
use crate::judge::PointState;
use crate::recipe::Recipe;
use crate::settings::RecordMode;
use crate::offline::Scenario;
use crate::vision;

/// 测量队列容量。满了说明测量跟不上帧率，新帧直接记为测量出错，不在内存里堆积。
pub const MEASURE_QUEUE: usize = 32;
pub const MEASURE_TIMEOUT: &str = "单帧测量超时";

/// 第 k 个拍照点的一帧，测该点负责的全部测量点。
pub struct Job {
    pub cycle_id: String,
    pub shot_id: String,
    pub camera: String,
    pub bundle_id: Option<String>,
    pub production: Option<Arc<crate::production::Prepared>>,
    pub candidate: Option<Arc<crate::candidate::Candidate>>,
    pub submitted_at: Instant,
    pub timeout: Duration,
    pub image_measurement: bool,
    pub sn: String,
    /// 拍照点序号
    pub k: usize,
    pub cam: u8,
    pub recipe: Arc<Recipe>,
    pub scenario: Scenario,
    pub image: Option<Arc<FrameImage>>,
    pub images: Vec<(u8, Arc<FrameImage>)>,
}

/// 测站状态。取值与记录库测量点 BLOB（store `POINTS_FORMAT` 3）一致；3 在 BLOB 里是“未测”，这里不用。
pub const ST_OK: u8 = 0;
pub const ST_GAP: u8 = 1;
pub const ST_INVALID: u8 = 2;
/// 有胶、无位置基准：胶宽有效，d 不用（位置判定见 judge 的基准无效规则）
pub const ST_NO_REF: u8 = 4;
/// 超出检测范围：落在位姿搬过去的胶路两端之外；d 不用，w 为 NaN 表示没测到胶宽
pub const ST_OUT_OF_RANGE: u8 = 5;

/// 定位失败的原因（帧徽标显示）
pub const REFERENCE_LOCATE_FAILED: &str = "定位失败";
/// 两端切向搬移超过配方 maxEndShiftMm（定稿 §10 决定一的护栏）
pub const REFERENCE_END_SHIFT: &str = "两端搬移超限";

/// 去掉一站的位置基准：有基准的有胶站改为 ST_NO_REF、d = NaN，胶宽保留；其余状态不动。
/// Measured::lose_reference 与 parse_result 的定位失败、护栏共用这一条，免得两处语义分叉。
pub fn drop_reference(st: &mut u8, d: &mut f32) {
    if *st == ST_OK {
        *st = ST_NO_REF;
        *d = f32::NAN;
    }
}

/// 测站状态与数值的契约：有基准的有胶站 d、w 都有限；无基准的有胶站 d 为 NaN、w 有限；超出检测范围的站 d 为 NaN、
/// w 为有限值或 NaN；断胶、测不了的站数值不用。d 必须是 NaN，免得实时帧带着的 d 和记录库里存的（NaN）对不上。
pub fn valid_station(st: u8, d: f32, w: f32) -> bool {
    match st {
        ST_OK => d.is_finite() && w.is_finite(),
        ST_NO_REF => d.is_nan() && w.is_finite(),
        ST_OUT_OF_RANGE => d.is_nan() && !w.is_infinite(),
        ST_GAP | ST_INVALID => true,
        _ => false,
    }
}

/// 测站状态 → 判定用的点状态。状态说了算：无基准、超出检测范围的站不读 d。
pub fn point_state(st: u8, d: f32, w: f32) -> PointState {
    match st {
        ST_OK => PointState::Measured { d: Some(d), w },
        ST_NO_REF => PointState::Measured { d: None, w },
        ST_OUT_OF_RANGE => PointState::OutOfRange { w: w.is_finite().then_some(w) },
        ST_GAP => PointState::Gap,
        _ => PointState::Invalid,
    }
}

/// 某个视图没有位置基准（帧徽标用）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceFailure {
    pub view: u8,
    /// 简短原因，如“定位失败”“两端搬移超限”
    pub reason: String,
}

/// 单帧测量结果，只含该帧负责的测量点。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Measured {
    pub cycle_id: String,
    pub shot_id: String,
    pub camera: String,
    pub bundle_id: Option<String>,
    pub sn: String,
    pub k: usize,
    pub cam: u8,
    /// 没有位置基准的视图。这些视图的有胶站已是 ST_NO_REF，判定只看站状态。
    pub reference_failed: Vec<ReferenceFailure>,
    pub score: f32,
    pub ms: u32,
    pub queue_ms: Option<u32>,
    pub engine_ms: Option<u32>,
    pub core_ms: Option<u32>,
    /// 测量本身没做成（lyFlow 运行失败、没有示教资料、队列满等）
    pub error: Option<String>,
    pub idx: Vec<u32>,
    pub d: Vec<f32>,
    /// 胶宽；没测时为 NaN（前端收到 null）
    pub w: Vec<f32>,
    pub st: Vec<u8>,
    /// 图像测量时各点在原图里的像素位置（叠加显示用）
    pub px: Vec<[f32; 2]>,
    pub views: Vec<vision::ViewMeasurement>,
    /// 用特征定位的视图的位姿（单图与多图两条路径都填），随帧记录保存，供追溯与 G5 叠图
    #[serde(skip)]
    pub poses: Vec<(u8, vision::PoseRecord)>,
}

impl Measured {
    pub fn point_state(&self, i: usize) -> PointState {
        point_state(self.st[i], self.d[i], self.w.get(i).copied().unwrap_or(f32::NAN))
    }

    /// 视图 view 没有位置基准：该视图的有胶站改为 ST_NO_REF（胶宽保留），帧上记下原因。
    /// 这是位置基准生产方的约定：定位失败（原因“定位失败”）、两端切向搬移超过配方 maxEndShiftMm（原因“两端搬移超限”）
    /// 都这样标，judge 按站状态判 ERR 92 或让位于 NG，不另设护栏逻辑。
    pub fn lose_reference(&mut self, recipe: &Recipe, view: u8, reason: &str) {
        for (i, &j) in self.idx.iter().enumerate() {
            let in_view = recipe.points.seg.get(j as usize).and_then(|&g| recipe.segments.get(g as usize)).is_some_and(|g| g.view == view);
            if in_view { drop_reference(&mut self.st[i], &mut self.d[i]); }
        }
        if !self.reference_failed.iter().any(|f| f.view == view) {
            self.reference_failed.push(ReferenceFailure { view, reason: reason.into() });
        }
    }

    pub fn empty(job: &Job) -> Self {
        Self {
            cycle_id: job.cycle_id.clone(),
            shot_id: job.shot_id.clone(),
            camera: job.camera.clone(),
            bundle_id: job.bundle_id.clone(),
            sn: job.sn.clone(),
            k: job.k,
            cam: job.cam,
            reference_failed: Vec::new(),
            score: 0.0,
            ms: 0,
            queue_ms: None,
            engine_ms: None,
            core_ms: None,
            error: None,
            idx: Vec::new(),
            d: Vec::new(),
            w: Vec::new(),
            st: Vec::new(),
            px: Vec::new(),
            views: Vec::new(),
            poses: Vec::new(),
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
    let record = crate::site::record_mode(app) != RecordMode::Off;
    app.state::<CycleHost>().camera.set_capture(crate::site::image_measurement(app) || record);
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

type JobRunner = Arc<dyn Fn(&Job) -> Measured + Send + Sync>;

fn timeout_result(job: &Job) -> Measured {
    Measured::failed(job, format!("{MEASURE_TIMEOUT}：拍照点 {} 超过 T_proc {} ms（含排队时间）", job.shot_id, job.timeout.as_millis()))
}

fn duration_ms(duration: Duration) -> u32 {
    duration.as_millis().min(u32::MAX as u128) as u32
}

fn report(out: &UnboundedSender<Input>, job: &Job, mut measured: Measured) {
    measured.ms = duration_ms(clock().saturating_duration_since(job.submitted_at));
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
                return (timeout_result(&worker_job), clock());
            }
            let entered_at = worker_execution.entered_at().unwrap();
            let _running = RunningJob { execution: worker_execution, _permit: permit };
            let mut measured = runner(&worker_job);
            let completed = clock();
            measured.engine_ms = Some(duration_ms(completed.saturating_duration_since(entered_at)));
            measured.core_ms = if worker_job.image_measurement && worker_job.image.is_some() && measured.error.is_none() {
                Some(measured.ms)
            } else {
                None
            };
            (measured, completed)
        });
        let out = out.clone();
        tokio::spawn(async move {
            let mut result = match tokio::time::timeout_at(deadline, task).await {
                Ok(Ok((measured, completed))) if tokio::time::Instant::from_std(completed) <= deadline => measured,
                Ok(Err(_)) => Measured::failed(&job, "测量线程异常退出"),
                _ => {
                    execution.timeout();
                    timeout_result(&job)
                }
            };
            result.queue_ms = execution.entered_at().map(|entered| duration_ms(entered.saturating_duration_since(job.submitted_at)));
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
        (Some(image), true) => {
            let mut measured = run_image(&app, job, image);
            apply_scenario(job, &mut measured);
            measured
        }
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

/// 离线定位失败工况：这一帧的各视图按位置基准生产方的约定标成没有基准（胶照常测）。
/// 图像测量里还没有定位步骤，模拟图也不再把整条胶挪出走廊，两条路径都由这里写入定位失败。
fn apply_scenario(job: &Job, m: &mut Measured) {
    if m.error.is_none() && job.scenario.locate_fail_frame(job.recipe.shot_count()) == Some(job.k) {
        for view in job.recipe.shots[job.k].enabled_views() {
            m.lose_reference(&job.recipe, view, REFERENCE_LOCATE_FAILED);
        }
    }
}

/// 模拟测量：不看图像，按场景生成测量值。
pub(crate) fn simulate(job: &Job) -> Measured {
    let r = &job.recipe;
    let mut m = Measured::empty(job);
    let gap = job.scenario.gap_points(r);
    // 超差场景：最后一段 30% 处横向偏出约 3 mm、宽约 3 mm（局部超差，在允许长度内）
    let bump = (job.scenario == Scenario::Excursion).then(|| r.segments.last()).flatten().map(|g| (g.shot, g.length(r.spacing) * 0.3));
    m.score = 0.91 + ((job.k * 7) % 5) as f32 / 100.0;
    for j in r.owned_points(job.k) {
        let seg = &r.segments[r.points.seg[j] as usize];
        let s = seg.s(j, r.spacing);
        let t = j as f32 * r.spacing;
        let mut d = 0.3 * (s / 23.0).sin() + 0.1 * (s / 4.7 + 1.3).sin() + 0.05 * noise(t);
        let w = 4.0 + 0.25 * (s / 17.0 + 0.6).sin() + 0.06 * noise(t + 7.0);
        if let Some((_, c)) = bump.filter(|(k, _)| *k == job.k) {
            d += 3.0 * (-((s - c) / 1.5).powi(2)).exp();
        }
        let st = if gap.contains(&j) { ST_GAP } else { ST_OK };
        m.idx.push(j as u32);
        m.d.push(d);
        m.w.push(w);
        m.st.push(st);
    }
    apply_scenario(job, &mut m);
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
            images: Vec::new(),
            cycle_id: "cycle-7".into(),
            shot_id: format!("P{}", k + 1),
            camera: "cam1".into(),
            bundle_id: Some("bundle-7".into()),
            production: None, candidate: None,
            submitted_at: clock(),
            timeout,
            image_measurement: false,
            sn: "SN-123".into(),
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

    #[tokio::test(start_paused = true)]
    async fn timed_out_blocking_job_keeps_capacity_and_late_completion_is_not_reported_twice() {
        let gate = Arc::new(Gate::default());
        let release = ReleaseOnDrop(gate.clone());
        let entered = Arc::new(AtomicUsize::new(0));
        let calls = entered.clone();
        let (blocked_tx, mut blocked) = unbounded_channel();
        let (tx, health, mut results, dispatcher) = worker(1, Arc::new(move |job| {
            let core_started = Instant::now();
            calls.fetch_add(1, Ordering::SeqCst);
            if job.k == 0 {
                let _ = blocked_tx.send(());
                gate.wait();
            }
            let mut measured = Measured::empty(job);
            if job.image_measurement {
                measured.ms = duration_ms(core_started.elapsed());
            }
            measured
        }));
        let mut image_job = job(0, Duration::from_millis(200));
        image_job.image_measurement = true;
        image_job.image = Some(Arc::new(FrameImage::new(2, 2, vec![128; 4])));
        tx.send(image_job).await.unwrap();
        blocked.recv().await.unwrap();
        assert_eq!(health.snapshot(), WorkerHealthSnapshot { capacity: 1, running: 1, timed_out: 0, available_capacity: 0 });
        assert!(health.can_arm());
        tokio::time::advance(Duration::from_millis(200)).await;
        let result = next_result(&mut results).await;
        assert!(result.error.unwrap().starts_with(MEASURE_TIMEOUT));
        assert_eq!((result.cycle_id.as_str(), result.shot_id.as_str()), ("cycle-7", "P1"));
        assert!(result.ms >= 200);
        assert!(result.queue_ms.is_some());
        assert!(result.engine_ms.is_none());
        assert!(result.core_ms.is_none());
        assert_eq!(health.snapshot(), WorkerHealthSnapshot { capacity: 1, running: 1, timed_out: 1, available_capacity: 0 });
        assert!(!health.can_arm());

        let watchdog = release.0.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(30));
            watchdog.release();
        });
        tx.send(job(1, Duration::from_millis(30))).await.unwrap();
        tx.send(job(2, Duration::from_millis(30))).await.unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(30)).await;
        for k in [1, 2] {
            let result = next_result(&mut results).await;
            assert_eq!(result.k, k);
            assert!(result.error.unwrap().starts_with(MEASURE_TIMEOUT));
            assert!(result.queue_ms.is_none());
            assert!(result.engine_ms.is_none());
            assert!(result.core_ms.is_none());
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
        assert!(recovered.queue_ms.is_some());
        assert!(recovered.engine_ms.is_some());
        assert!(recovered.core_ms.is_none());
        assert_eq!(entered.load(Ordering::SeqCst), 2);
        drop(tx);
        dispatcher.await.unwrap();
        assert!(results.try_recv().is_err());
    }

    #[tokio::test]
    async fn full_measure_queue_rejects_one_job_then_recovers_without_leaks_or_duplicate_results() {
        let gate = Arc::new(Gate::default());
        let release = ReleaseOnDrop(gate.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = calls.clone();
        let (tx, health, mut results, dispatcher) = worker(1, Arc::new(move |job| {
            entered.fetch_add(1, Ordering::SeqCst);
            gate.wait();
            Measured::empty(job)
        }));
        tx.send(job(0, Duration::from_secs(5))).await.unwrap();
        wait_health(&health, WorkerHealthSnapshot { capacity: 1, running: 1, timed_out: 0, available_capacity: 0 }).await;
        tx.send(job(1, Duration::from_secs(5))).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while tx.capacity() != MEASURE_QUEUE {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }).await.unwrap();
        for k in 2..MEASURE_QUEUE + 2 {
            assert!(tx.try_send(job(k, Duration::from_secs(5))).is_ok());
        }
        assert_eq!(tx.capacity(), 0);
        let rejected = match tx.try_send(job(MEASURE_QUEUE + 2, Duration::from_secs(5))) {
            Err(tokio::sync::mpsc::error::TrySendError::Full(job)) => job,
            _ => panic!("expected the real bounded measurement queue to be full"),
        };
        let failure = Measured::failed(&rejected, "测量队列已满：测量跟不上帧率");
        assert_eq!(failure.error.as_deref(), Some("测量队列已满：测量跟不上帧率"));
        assert_eq!(failure.k, MEASURE_QUEUE + 2);
        assert_eq!(failure.cycle_id, "cycle-7");
        assert_eq!(failure.shot_id, format!("P{}", MEASURE_QUEUE + 3));
        assert_eq!(failure.bundle_id.as_deref(), Some("bundle-7"));
        assert!(failure.queue_ms.is_none());
        assert!(failure.engine_ms.is_none());
        assert!(failure.core_ms.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(results.try_recv().is_err());
        release.0.release();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..MEASURE_QUEUE + 2 {
            let measured = next_result(&mut results).await;
            assert!(measured.k < MEASURE_QUEUE + 2);
            assert!(measured.error.is_none());
            assert!(measured.queue_ms.is_some());
            assert!(measured.engine_ms.is_some());
            assert!(measured.core_ms.is_none());
            assert!(seen.insert(measured.k), "duplicate measurement result");
        }
        assert_eq!(seen.len(), MEASURE_QUEUE + 2);
        assert_eq!(calls.load(Ordering::SeqCst), MEASURE_QUEUE + 2);
        wait_health(&health, WorkerHealthSnapshot { capacity: 1, running: 0, timed_out: 0, available_capacity: 1 }).await;
        assert!(health.can_arm());
        assert_eq!(tx.capacity(), MEASURE_QUEUE);
        tx.send(job(MEASURE_QUEUE + 3, Duration::from_secs(1))).await.unwrap();
        let recovered = next_result(&mut results).await;
        assert_eq!(recovered.k, MEASURE_QUEUE + 3);
        assert!(recovered.error.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), MEASURE_QUEUE + 3);
        wait_health(&health, WorkerHealthSnapshot { capacity: 1, running: 0, timed_out: 0, available_capacity: 1 }).await;
        drop(tx);
        dispatcher.await.unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(2), results.recv()).await.unwrap().is_none());
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
        assert!(result.queue_ms.is_none());
        assert!(result.engine_ms.is_none());
        assert!(result.core_ms.is_none());
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
        assert_eq!(failed.bundle_id.as_deref(), Some("bundle-7"));
        assert!(failed.queue_ms.is_some());
        assert!(failed.engine_ms.is_none());
        assert!(failed.core_ms.is_none());
        wait_health(&health, WorkerHealthSnapshot { capacity: 1, running: 0, timed_out: 0, available_capacity: 1 }).await;
        tx.send(job(1, Duration::from_secs(1))).await.unwrap();
        assert!(next_result(&mut results).await.error.is_none());
        drop(tx);
        dispatcher.await.unwrap();
        assert!(health.can_arm());
    }
}
