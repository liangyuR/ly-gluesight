use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::store::Store;

const CAPACITY: usize = 32;
const LOW_WATER: usize = 16;
const RETRY_DELAY: Duration = Duration::from_secs(1);

struct Allocation {
    count: usize,
    result: Receiver<Result<Vec<String>, String>>,
}

struct Failure {
    reason: String,
    retry_at: Instant,
}

#[derive(Default)]
struct Pool {
    ids: VecDeque<String>,
    allocating: Option<Allocation>,
    failure: Option<Failure>,
}

#[derive(Default)]
pub struct CycleIds {
    pool: Mutex<Pool>,
}

impl CycleIds {
    pub fn ready(&self, app: &AppHandle) -> Result<(), String> {
        let app = app.clone();
        self.ready_with(Instant::now(), move |count| {
            app.state::<Store>().reserve_cycle_ids(count)
        })
    }

    pub fn take(&self, app: &AppHandle) -> Result<String, String> {
        let app = app.clone();
        self.take_with(Instant::now(), move |count| {
            app.state::<Store>().reserve_cycle_ids(count)
        })
    }

    fn ready_with<F>(&self, now: Instant, allocate: F) -> Result<(), String>
    where
        F: FnOnce(usize) -> Result<Vec<String>, String> + Send + 'static,
    {
        self.with_ids(now, allocate, |_| ())
    }

    fn take_with<F>(&self, now: Instant, allocate: F) -> Result<String, String>
    where
        F: FnOnce(usize) -> Result<Vec<String>, String> + Send + 'static,
    {
        self.with_ids(now, allocate, |ids| ids.pop_front().unwrap())
    }

    fn with_ids<F, T>(
        &self,
        now: Instant,
        allocate: F,
        use_ids: impl FnOnce(&mut VecDeque<String>) -> T,
    ) -> Result<T, String>
    where
        F: FnOnce(usize) -> Result<Vec<String>, String> + Send + 'static,
    {
        let mut pool = self
            .pool
            .lock()
            .map_err(|_| "工件 ID 池状态不可用".to_string())?;
        pool.complete(now);
        if pool.ids.is_empty() {
            pool.refill(now, allocate);
            return Err(pool.unavailable());
        }
        let value = use_ids(&mut pool.ids);
        pool.refill(now, allocate);
        Ok(value)
    }
}

impl Pool {
    fn complete(&mut self, now: Instant) {
        let Some(allocation) = self.allocating.as_ref() else {
            return;
        };
        let result = match allocation.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("后台分配作业中断".into()),
        };
        let count = self.allocating.take().unwrap().count;
        match result.and_then(|ids| self.validate(count, ids)) {
            Ok(ids) => {
                self.ids.extend(ids);
                self.failure = None;
            }
            Err(reason) => {
                self.failure = Some(Failure {
                    reason,
                    retry_at: now + RETRY_DELAY,
                })
            }
        }
    }

    fn validate(&self, count: usize, ids: Vec<String>) -> Result<Vec<String>, String> {
        if ids.len() != count || self.ids.len() + ids.len() > CAPACITY {
            return Err(format!(
                "持久分配返回 {} 个 ID，预期 {count} 个且总量不能超过 {CAPACITY}",
                ids.len()
            ));
        }
        let mut seen: HashSet<&str> = self.ids.iter().map(String::as_str).collect();
        for id in &ids {
            if id.trim().is_empty() {
                return Err("持久分配返回空工件 ID".into());
            }
            if !seen.insert(id) {
                return Err("持久分配返回重复工件 ID".into());
            }
        }
        Ok(ids)
    }

    fn refill<F>(&mut self, now: Instant, allocate: F)
    where
        F: FnOnce(usize) -> Result<Vec<String>, String> + Send + 'static,
    {
        if self.allocating.is_some()
            || self.ids.len() > LOW_WATER
            || self
                .failure
                .as_ref()
                .is_some_and(|failure| now < failure.retry_at)
        {
            return;
        }
        let count = CAPACITY - self.ids.len();
        let (send, result) = sync_channel(1);
        match std::thread::Builder::new()
            .name("cycle-id-reserve".into())
            .spawn(move || {
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| allocate(count)))
                        .unwrap_or_else(|payload| {
                            let reason = payload
                                .downcast_ref::<&str>()
                                .copied()
                                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                                .unwrap_or("未知异常");
                            Err(format!("后台分配作业异常：{reason}"))
                        });
                let _ = send.send(outcome);
            }) {
            Ok(_) => self.allocating = Some(Allocation { count, result }),
            Err(error) => {
                self.failure = Some(Failure {
                    reason: format!("后台分配作业无法启动：{error}"),
                    retry_at: now + RETRY_DELAY,
                })
            }
        }
    }

    fn unavailable(&self) -> String {
        match (&self.failure, &self.allocating) {
            (Some(failure), Some(_)) => {
                format!("工件 ID 正在后台重试持久分配；上次失败：{}", failure.reason)
            }
            (Some(failure), None) => format!("工件 ID 持久分配失败，等待重试：{}", failure.reason),
            _ => "工件 ID 正在后台持久分配，尚不可布防".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Barrier, Condvar};

    fn allocator(
        next: Arc<AtomicUsize>,
    ) -> impl FnOnce(usize) -> Result<Vec<String>, String> + Send {
        move |count| {
            Ok((0..count)
                .map(|_| format!("cycle-{}", next.fetch_add(1, Ordering::SeqCst)))
                .collect())
        }
    }

    fn unexpected(_: usize) -> Result<Vec<String>, String> {
        panic!("unexpected allocation")
    }

    fn complete(pool: &CycleIds, now: Instant) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let mut state = pool.pool.lock().unwrap();
            state.complete(now);
            if state.allocating.is_none() {
                return;
            }
            drop(state);
            assert!(Instant::now() < until, "allocation did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn bounded(pool: &CycleIds) {
        let state = pool.pool.lock().unwrap();
        assert!(state.ids.len() <= CAPACITY);
        assert!(state.ids.len() + state.allocating.as_ref().map_or(0, |job| job.count) <= CAPACITY);
    }

    #[test]
    fn slow_persistence_cannot_make_an_empty_pool_ready_or_allocate_an_id() {
        let pool = CycleIds::default();
        let now = Instant::now();
        let next = Arc::new(AtomicUsize::new(0));
        let (started, started_wait) = mpsc::channel();
        let (release, release_wait) = mpsc::channel();
        let (finished, finished_wait) = mpsc::channel();
        let allocate = allocator(next);
        assert!(pool
            .ready_with(now, move |count| {
                started.send(count).unwrap();
                release_wait
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|error| error.to_string())?;
                let ids = allocate(count);
                finished.send(()).unwrap();
                ids
            })
            .unwrap_err()
            .contains("正在后台"));
        assert_eq!(
            started_wait.recv_timeout(Duration::from_secs(5)).unwrap(),
            CAPACITY
        );
        for _ in 0..32 {
            assert!(pool
                .ready_with(now, unexpected)
                .unwrap_err()
                .contains("正在后台"));
            assert!(pool
                .take_with(now, unexpected)
                .unwrap_err()
                .contains("正在后台"));
        }
        assert!(matches!(finished_wait.try_recv(), Err(TryRecvError::Empty)));
        release.send(()).unwrap();
        complete(&pool, now);
        pool.ready_with(now, unexpected).unwrap();
        assert_eq!(pool.pool.lock().unwrap().ids.len(), CAPACITY);
        assert_eq!(pool.take_with(now, unexpected).unwrap(), "cycle-0");
        bounded(&pool);
    }

    #[test]
    fn concurrent_ready_calls_start_only_one_allocation() {
        let pool = Arc::new(CycleIds::default());
        let barrier = Arc::new(Barrier::new(9));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let next = Arc::new(AtomicUsize::new(0));
        let (started, started_wait) = mpsc::channel();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let pool = pool.clone();
                let barrier = barrier.clone();
                let gate = gate.clone();
                let calls = calls.clone();
                let started = started.clone();
                let allocate = allocator(next.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    assert!(pool
                        .ready_with(Instant::now(), move |count| {
                            calls.fetch_add(1, Ordering::SeqCst);
                            started.send(count).unwrap();
                            let (open, wake) = &*gate;
                            let mut open = open.lock().unwrap();
                            while !*open {
                                open = wake.wait(open).unwrap();
                            }
                            allocate(count)
                        })
                        .is_err());
                })
            })
            .collect();
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(
            started_wait.recv_timeout(Duration::from_secs(5)).unwrap(),
            CAPACITY
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        bounded(&pool);
        let (open, wake) = &*gate;
        *open.lock().unwrap() = true;
        wake.notify_all();
        complete(&pool, Instant::now());
        for _ in 0..32 {
            pool.ready_with(Instant::now(), unexpected).unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(pool.pool.lock().unwrap().ids.len(), CAPACITY);
    }

    #[test]
    fn durable_ids_remain_unique_and_the_pool_stays_bounded_during_refills() {
        let pool = CycleIds::default();
        let next = Arc::new(AtomicUsize::new(0));
        let now = Instant::now();
        assert!(pool.ready_with(now, allocator(next.clone())).is_err());
        complete(&pool, now);
        let mut seen = HashSet::new();
        for _ in 0..CAPACITY * 4 {
            let id = loop {
                match pool.take_with(now, allocator(next.clone())) {
                    Ok(id) => break id,
                    Err(_) => complete(&pool, now),
                }
            };
            assert!(seen.insert(id));
            bounded(&pool);
        }
        assert_eq!(seen.len(), CAPACITY * 4);
    }

    #[test]
    fn allocation_failure_has_a_one_second_backoff_and_can_recover() {
        let pool = CycleIds::default();
        let now = Instant::now();
        let calls = Arc::new(AtomicUsize::new(0));
        let failed_calls = calls.clone();
        assert!(pool
            .ready_with(now, move |_| {
                failed_calls.fetch_add(1, Ordering::SeqCst);
                Err("disk full".into())
            })
            .is_err());
        complete(&pool, now);
        let before_retry = now + RETRY_DELAY - Duration::from_millis(1);
        for _ in 0..32 {
            assert!(pool
                .ready_with(before_retry, unexpected)
                .unwrap_err()
                .contains("disk full"));
            assert!(pool
                .take_with(before_retry, unexpected)
                .unwrap_err()
                .contains("disk full"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(pool.pool.lock().unwrap().allocating.is_none());
        let retry = now + RETRY_DELAY;
        assert!(pool
            .ready_with(retry, allocator(Arc::new(AtomicUsize::new(0))))
            .is_err());
        complete(&pool, retry);
        pool.ready_with(retry, unexpected).unwrap();
        assert_eq!(pool.take_with(retry, unexpected).unwrap(), "cycle-0");
        assert!(pool.pool.lock().unwrap().failure.is_none());
        bounded(&pool);
    }

    #[test]
    fn allocator_panic_is_visible_clears_in_flight_and_can_recover() {
        let pool = CycleIds::default();
        let now = Instant::now();
        assert!(pool.ready_with(now, |_| panic!("allocator panic")).is_err());
        complete(&pool, now);
        assert!(pool.pool.lock().unwrap().allocating.is_none());
        assert!(pool
            .ready_with(now, unexpected)
            .unwrap_err()
            .contains("allocator panic"));
        assert!(pool
            .take_with(now, unexpected)
            .unwrap_err()
            .contains("allocator panic"));
        let retry = now + RETRY_DELAY;
        assert!(pool
            .ready_with(retry, allocator(Arc::new(AtomicUsize::new(0))))
            .is_err());
        complete(&pool, retry);
        pool.ready_with(retry, unexpected).unwrap();
        assert_eq!(pool.take_with(retry, unexpected).unwrap(), "cycle-0");
        bounded(&pool);
    }

    #[test]
    fn empty_wrong_count_empty_id_and_duplicate_batches_cannot_make_the_pool_ready() {
        for ids in [
            Vec::new(),
            vec!["count".to_string(); CAPACITY - 1],
            vec![" ".to_string(); CAPACITY],
            vec!["duplicate".to_string(); CAPACITY],
            vec!["overflow".to_string(); CAPACITY + 1],
        ] {
            let pool = CycleIds::default();
            let now = Instant::now();
            assert!(pool.ready_with(now, move |_| Ok(ids)).is_err());
            complete(&pool, now);
            assert!(pool.pool.lock().unwrap().ids.is_empty());
            assert!(pool
                .ready_with(now, unexpected)
                .unwrap_err()
                .contains("失败"));
            assert!(pool.take_with(now, unexpected).is_err());
            bounded(&pool);
        }
    }

    #[test]
    fn a_bad_refill_cannot_replace_existing_durable_ids() {
        let pool = CycleIds::default();
        let now = Instant::now();
        assert!(pool
            .ready_with(now, allocator(Arc::new(AtomicUsize::new(0))))
            .is_err());
        complete(&pool, now);
        for _ in 0..CAPACITY - LOW_WATER - 1 {
            pool.take_with(now, unexpected).unwrap();
        }
        pool.take_with(now, |count| {
            let mut ids: Vec<_> = (0..count).map(|i| format!("refill-{i}")).collect();
            ids[0] = format!("cycle-{}", CAPACITY - 1);
            Ok(ids)
        })
        .unwrap();
        complete(&pool, now);
        assert_eq!(pool.pool.lock().unwrap().ids.len(), LOW_WATER);
        pool.ready_with(now, unexpected).unwrap();
        assert_eq!(
            pool.take_with(now, unexpected).unwrap(),
            format!("cycle-{}", CAPACITY - LOW_WATER)
        );
        bounded(&pool);
    }
}
