use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Condvar;
use tokio::sync::oneshot;

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

async fn wait_permits(slot: &Semaphore, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while slot.available_permits() != expected {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }).await.unwrap();
}

#[tokio::test]
async fn queued_warmup_deadline_includes_the_permit_wait() {
    let slot = Arc::new(Semaphore::new(1));
    let gate = Arc::new(Gate::default());
    let release = ReleaseOnDrop(gate.clone());
    let (entered, started) = oneshot::channel();
    let incumbent = tokio::spawn(run_warmup(slot.clone(), Duration::from_secs(5), move || {
        let _ = entered.send(());
        gate.wait();
        Ok(41)
    }));
    tokio::time::timeout(Duration::from_secs(2), started).await.unwrap().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = calls.clone();
    let error = run_warmup(slot.clone(), Duration::from_millis(30), move || {
        entered.fetch_add(1, Ordering::SeqCst);
        Ok(42)
    }).await.unwrap_err();
    assert!(matches!(&error, WarmupFailure::QueueTimeout(_)));
    assert!(error.message().contains("排队或预热超过 30 ms"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(slot.available_permits(), 0);
    assert!(!incumbent.is_finished());
    release.0.release();
    assert_eq!(incumbent.await.unwrap().unwrap(), 41);
    wait_permits(&slot, 1).await;
}

#[tokio::test]
async fn timed_out_warmup_keeps_the_permit_and_late_success_cannot_replace_failure() {
    let host = ProductionHost::default();
    let slot = host.slot.clone();
    let gate = Arc::new(Gate::default());
    let release = ReleaseOnDrop(gate.clone());
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let saved = outcomes.clone();
    let completed = Arc::new(AtomicBool::new(false));
    let finished = completed.clone();
    let (entered, started) = oneshot::channel();
    let worker_slot = slot.clone();
    let worker = tokio::spawn(async move {
        let result = run_warmup(worker_slot, Duration::from_millis(200), move || {
            let _ = entered.send(());
            gate.wait();
            finished.store(true, Ordering::SeqCst);
            Ok(42)
        }).await;
        saved.lock().unwrap().push(result);
    });
    tokio::time::timeout(Duration::from_secs(2), started).await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(2), worker).await.unwrap().unwrap();
    let failure = outcomes.lock().unwrap()[0].as_ref().unwrap_err().clone();
    assert!(matches!(&failure, WarmupFailure::Failed(_)));
    assert!(failure.message().contains("排队或预热超过 200 ms"));
    let failed_at = Instant::now();
    host.entries.lock().unwrap().insert("timed-out".into(), failure.clone().into_entry(failed_at));
    assert!(host.begin("timed-out", failed_at + Duration::from_secs(3600)).is_err());
    assert_eq!(slot.available_permits(), 0);
    assert!(!completed.load(Ordering::SeqCst));
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = calls.clone();
    assert!(run_warmup(slot.clone(), Duration::from_millis(30), move || {
        entered.fetch_add(1, Ordering::SeqCst);
        Ok(43)
    }).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    release.0.release();
    wait_permits(&slot, 1).await;
    assert!(completed.load(Ordering::SeqCst));
    assert_eq!(*outcomes.lock().unwrap(), vec![Err(failure)]);
    assert!(host.begin("timed-out", failed_at + Duration::from_secs(3600)).is_err());
    assert!(matches!(host.entries.lock().unwrap().get("timed-out"), Some(Entry::Failed(_))));
    assert_eq!(run_warmup(slot.clone(), Duration::from_secs(1), || Ok(44)).await.unwrap(), 44);
    wait_permits(&slot, 1).await;
}

#[test]
fn warmup_expired_in_the_real_blocking_pool_never_runs_the_operation() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_time().max_blocking_threads(1).build().unwrap();
    let gate = Arc::new(Gate::default());
    let release = ReleaseOnDrop(gate.clone());
    runtime.block_on(async move {
        let slot = Arc::new(Semaphore::new(1));
        let (entered, started) = oneshot::channel();
        let occupied = tokio::task::spawn_blocking(move || {
            let _ = entered.send(());
            gate.wait();
        });
        tokio::time::timeout(Duration::from_secs(2), started).await.unwrap().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = calls.clone();
        let worker = tokio::spawn(run_warmup(slot.clone(), Duration::from_millis(200), move || {
            entered.fetch_add(1, Ordering::SeqCst);
            Ok(42)
        }));
        wait_permits(&slot, 0).await;
        let error = tokio::time::timeout(Duration::from_secs(2), worker).await.unwrap().unwrap().unwrap_err();
        assert!(matches!(&error, WarmupFailure::QueueTimeout(_)));
        assert!(error.message().contains("排队或预热超过 200 ms"));
        assert_eq!(slot.available_permits(), 0);
        release.0.release();
        occupied.await.unwrap();
        wait_permits(&slot, 1).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(run_warmup(slot.clone(), Duration::from_secs(1), || Ok(43)).await.unwrap(), 43);
    });
}

#[tokio::test]
async fn panicking_warmup_releases_the_permit_for_a_new_operation() {
    let slot = Arc::new(Semaphore::new(1));
    let error = run_warmup::<(), _>(slot.clone(), Duration::from_secs(1), || panic!("controlled warmup panic")).await.unwrap_err();
    assert!(error.message().contains("图像引擎预热异常"));
    wait_permits(&slot, 1).await;
    assert_eq!(run_warmup(slot.clone(), Duration::from_secs(1), || Ok(42)).await.unwrap(), 42);
    wait_permits(&slot, 1).await;
}

#[tokio::test]
async fn queued_recipe_can_retry_after_cumulative_wait_expires_and_the_slot_recovers() {
    let host = ProductionHost::default();
    let held = host.slot.clone().acquire_owned().await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut retries = Vec::new();
    for key in ["recipe-b", "recipe-c"] {
        assert!(host.begin(key, Instant::now()).unwrap().is_none());
        let counted = calls.clone();
        let failure = run_warmup(host.slot.clone(), Duration::from_millis(20), move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }).await.unwrap_err();
        assert!(matches!(&failure, WarmupFailure::QueueTimeout(_)));
        let failed_at = Instant::now();
        host.entries.lock().unwrap().insert(key.into(), failure.into_entry(failed_at));
        for _ in 0..10 { assert!(host.begin(key, failed_at).is_err()); }
        assert!(matches!(host.entries.lock().unwrap().get(key), Some(Entry::Retryable { .. })));
        retries.push((key, failed_at));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(held);
    for (key, failed_at) in retries {
        assert!(host.begin(key, failed_at + WARMUP_RETRY_DELAY).unwrap().is_none());
        assert!(host.begin(key, failed_at + WARMUP_RETRY_DELAY).is_err());
        let counted = calls.clone();
        assert_eq!(run_warmup(host.slot.clone(), Duration::from_secs(1), move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(42)
        }).await.unwrap(), 42);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    wait_permits(&host.slot, 1).await;
}

#[tokio::test]
async fn actual_engine_failure_remains_permanent() {
    let host = ProductionHost::default();
    assert!(host.begin("failed", Instant::now()).unwrap().is_none());
    let failed = run_warmup::<(), _>(host.slot.clone(), Duration::from_secs(1), || Err("controlled engine error".into())).await.unwrap_err();
    assert!(matches!(&failed, WarmupFailure::Failed(_)));
    let now = Instant::now();
    host.entries.lock().unwrap().insert("failed".into(), failed.into_entry(now));
    assert!(host.begin("failed", now + Duration::from_secs(3600)).is_err());
    assert!(matches!(host.entries.lock().unwrap().get("failed"), Some(Entry::Failed(_))));
    wait_permits(&host.slot, 1).await;
}

#[test]
fn concurrent_refreshes_cannot_start_duplicate_retries() {
    let host = Arc::new(ProductionHost::default());
    let now = Instant::now();
    host.entries.lock().unwrap().insert("recipe".into(), WarmupFailure::QueueTimeout("queued".into()).into_entry(now));
    let started = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|scope| {
        for _ in 0..12 {
            let host = host.clone();
            let started = started.clone();
            scope.spawn(move || {
                if host.begin("recipe", now + WARMUP_RETRY_DELAY).is_ok() { started.fetch_add(1, Ordering::SeqCst); }
            });
        }
    });
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert!(matches!(host.entries.lock().unwrap().get("recipe"), Some(Entry::Preparing)));
}
