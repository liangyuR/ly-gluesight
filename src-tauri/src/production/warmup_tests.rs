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
    assert!(error.contains("排队或预热超过 30 ms"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(slot.available_permits(), 0);
    assert!(!incumbent.is_finished());
    release.0.release();
    assert_eq!(incumbent.await.unwrap().unwrap(), 41);
    wait_permits(&slot, 1).await;
}

#[tokio::test]
async fn timed_out_warmup_keeps_the_permit_and_late_success_cannot_replace_failure() {
    let slot = Arc::new(Semaphore::new(1));
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
    assert!(failure.contains("排队或预热超过 200 ms"));
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
        assert!(error.contains("排队或预热超过 200 ms"));
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
    assert!(error.contains("图像引擎预热异常"));
    wait_permits(&slot, 1).await;
    assert_eq!(run_warmup(slot.clone(), Duration::from_secs(1), || Ok(42)).await.unwrap(), 42);
    wait_permits(&slot, 1).await;
}
