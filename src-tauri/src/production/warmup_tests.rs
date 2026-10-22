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

#[tokio::test(start_paused = true)]
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
    started.await.unwrap();
    tokio::time::advance(Duration::from_millis(200)).await;
    worker.await.unwrap();
    let failure = outcomes.lock().unwrap()[0].as_ref().unwrap_err().clone();
    // 引擎仍卡在闸门里（completed 为假）就已判失败：只能是执行超时
    assert!(matches!(&failure, WarmupFailure::Failed(_)));
    let failed_at = Instant::now();
    host.entries.lock().unwrap().insert("timed-out".into(), failure.clone().into_entry(failed_at));
    assert!(host.begin("timed-out", failed_at + Duration::from_secs(3600)).is_err());
    assert_eq!(slot.available_permits(), 0);
    assert!(!completed.load(Ordering::SeqCst));
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = calls.clone();
    let queued = tokio::spawn(run_warmup(slot.clone(), Duration::from_millis(30), move || {
        entered.fetch_add(1, Ordering::SeqCst);
        Ok(43)
    }));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(30)).await;
    assert!(matches!(queued.await.unwrap(), Err(WarmupFailure::QueueTimeout(_))));
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

#[tokio::test]
async fn panicking_warmup_releases_the_permit_for_a_new_operation() {
    let slot = Arc::new(Semaphore::new(1));
    let error = run_warmup::<(), _>(slot.clone(), Duration::from_secs(1), || panic!("controlled warmup panic")).await.unwrap_err();
    assert!(matches!(error, WarmupFailure::Failed(_)), "异常不可重试");
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
