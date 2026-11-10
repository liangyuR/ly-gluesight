use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::burst_judge::FrameState;

const RUNNING: u8 = 0;
const CANCELLED: u8 = 1;
const SAVING: u8 = 2;
const FINISHED: u8 = 3;
pub const CANCELLED_MESSAGE: &str = "OPERATION_CANCELLED: 任务已取消，原结果未更改";
const TERMINAL_LIMIT: usize = 64;

#[derive(Default)]
struct Registry {
    active: HashMap<String, Arc<AtomicU8>>,
    terminal: VecDeque<Progress>,
}

#[derive(Clone, Default)]
pub struct Operations(Arc<Mutex<Registry>>);

impl Operations {
    fn register(&self, task_id: &str) -> Result<Arc<AtomicU8>, String> {
        if task_id.is_empty() || task_id.len() > 128 {
            return Err("任务编号无效".into());
        }
        let mut tasks = self.0.lock().unwrap();
        if tasks.active.contains_key(task_id) {
            return Err("任务编号已在使用".into());
        }
        let state = Arc::new(AtomicU8::new(RUNNING));
        tasks
            .terminal
            .retain(|progress| progress.task_id != task_id);
        tasks.active.insert(task_id.into(), state.clone());
        Ok(state)
    }

    /// 进行中的带编号任务数。
    pub fn active(&self) -> usize {
        self.0.lock().unwrap().active.len()
    }

    pub fn cancel(&self, task_id: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .active
            .get(task_id)
            .is_some_and(|state| {
                state
                    .compare_exchange(RUNNING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            })
    }

    pub fn status(&self, task_id: &str) -> Option<Progress> {
        self.0
            .lock()
            .unwrap()
            .terminal
            .iter()
            .find(|progress| progress.task_id == task_id)
            .cloned()
    }

    fn complete(&self, progress: Progress) {
        let mut tasks = self.0.lock().unwrap();
        tasks.active.remove(&progress.task_id);
        tasks.terminal.push_back(progress);
        while tasks.terminal.len() > TERMINAL_LIMIT {
            tasks.terminal.pop_front();
        }
    }
}

#[tauri::command]
pub fn operation_cancel(operations: State<'_, Operations>, task_id: String) -> bool {
    operations.cancel(&task_id)
}

#[tauri::command]
pub fn operation_status(operations: State<'_, Operations>, task_id: String) -> Option<Progress> {
    operations.status(&task_id)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    task_id: String,
    stage: &'static str,
    completed: u64,
    total: u64,
    failed: u64,
    skipped: u64,
    elapsed_ms: u64,
}

pub struct Operation {
    operations: Operations,
    publish: Box<dyn Fn(&Progress) + Send + Sync>,
    task_id: Option<String>,
    state: Arc<AtomicU8>,
    started: Instant,
    progress: Mutex<(Progress, Instant)>,
    /// 任务进行期间不做原图空闲压缩
    _busy: crate::raw_compress::Busy<'static>,
}

impl Operation {
    pub fn start(app: &AppHandle, task_id: Option<String>) -> Result<Self, String> {
        let operations = app.state::<Operations>().inner().clone();
        let app = app.clone();
        Self::with_events(operations, task_id, move |progress| {
            let _ = app.emit("operation://progress", progress);
        })
    }

    pub fn with_events(
        operations: Operations,
        task_id: Option<String>,
        publish: impl Fn(&Progress) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let state = match &task_id {
            Some(id) => operations.register(id)?,
            None => Arc::new(AtomicU8::new(RUNNING)),
        };
        let started = Instant::now();
        let operation = Self {
            operations,
            publish: Box::new(publish),
            state,
            started,
            progress: Mutex::new((
                Progress {
                    task_id: task_id.clone().unwrap_or_default(),
                    stage: "preparing",
                    completed: 0,
                    total: 0,
                    failed: 0,
                    skipped: 0,
                    elapsed_ms: 0,
                },
                started,
            )),
            task_id,
            _busy: crate::raw_compress::activity().busy(),
        };
        operation.emit(true);
        Ok(operation)
    }

    pub fn check(&self) -> Result<(), String> {
        if self.state.load(Ordering::Acquire) == CANCELLED {
            Err(CANCELLED_MESSAGE.into())
        } else {
            Ok(())
        }
    }

    fn emit(&self, force: bool) {
        if self.task_id.is_none() {
            return;
        }
        let mut progress = self.progress.lock().unwrap();
        if !force && progress.1.elapsed() < Duration::from_millis(150) {
            return;
        }
        progress.0.elapsed_ms = self.started.elapsed().as_millis() as u64;
        progress.1 = Instant::now();
        let snapshot = progress.0.clone();
        drop(progress);
        (self.publish)(&snapshot);
    }

    pub fn stage(&self, stage: &'static str) -> Result<(), String> {
        self.check()?;
        self.progress.lock().unwrap().0.stage = stage;
        self.emit(true);
        Ok(())
    }

    pub fn measuring(&self, total: usize) -> Result<(), String> {
        self.progress.lock().unwrap().0.total = total as u64;
        self.stage("measuring")
    }

    pub fn advance(&self, state: FrameState) -> Result<(), String> {
        {
            let mut progress = self.progress.lock().unwrap();
            progress.0.completed += 1;
            match state {
                FrameState::Error => progress.0.failed += 1,
                FrameState::Missing | FrameState::NotInspected | FrameState::Unmeasured => {
                    progress.0.skipped += 1
                }
                _ => {}
            }
        }
        self.emit(false);
        self.check()
    }

    pub fn saving(&self) -> Result<(), String> {
        self.state
            .compare_exchange(RUNNING, SAVING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| CANCELLED_MESSAGE.to_string())?;
        self.stage("saving")
    }

    pub fn run<T>(&self, run: impl FnOnce(&Self) -> Result<T, String>) -> Result<T, String> {
        let result = self.check().and_then(|_| run(self));
        let previous = self.state.swap(FINISHED, Ordering::AcqRel);
        let result = if previous == CANCELLED {
            Err(CANCELLED_MESSAGE.into())
        } else {
            result
        };
        self.finish_event(if previous == CANCELLED {
            "cancelled"
        } else if result.is_ok() {
            "completed"
        } else {
            "failed"
        });
        result
    }

    fn finish_event(&self, stage: &'static str) {
        let mut progress = self.progress.lock().unwrap();
        progress.0.stage = stage;
        progress.0.elapsed_ms = self.started.elapsed().as_millis() as u64;
        let snapshot = progress.0.clone();
        drop(progress);
        if self.task_id.is_some() {
            self.operations.complete(snapshot.clone());
            (self.publish)(&snapshot);
        }
    }

    fn finish(&self, stage: &'static str) {
        if self.state.swap(FINISHED, Ordering::AcqRel) != FINISHED {
            self.finish_event(stage);
        }
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        self.finish(if self.state.load(Ordering::Acquire) == CANCELLED {
            "cancelled"
        } else {
            "failed"
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_measurement_prevents_commit_and_releases_task_id() {
        let tasks = Operations::default();
        let events = Arc::new(Mutex::new(Vec::new()));
        let received = events.clone();
        let operation =
            Operation::with_events(tasks.clone(), Some("measurement".into()), move |event| {
                received
                    .lock()
                    .unwrap()
                    .push(serde_json::to_value(event).unwrap())
            })
            .unwrap();
        let mut saved = "previous";
        let result = operation.run(|operation| {
            operation.measuring(3)?;
            assert!(tasks.cancel("measurement"));
            operation.advance(FrameState::Measured)?;
            operation.saving()?;
            saved = "replacement";
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(saved, "previous");
        let events = events.lock().unwrap();
        assert_eq!(events.first().unwrap()["stage"], "preparing");
        assert_eq!(events.last().unwrap()["stage"], "cancelled");
        assert_eq!(events.last().unwrap()["completed"], 1);
        assert!(Operation::with_events(tasks, Some("measurement".into()), |_| {}).is_ok());
    }

    #[test]
    fn saving_rejects_cancel_and_duplicate_task_without_interrupting_commit() {
        let tasks = Operations::default();
        let operation =
            Operation::with_events(tasks.clone(), Some("measurement".into()), |_| {}).unwrap();
        let result = operation.run(|operation| {
            operation.saving()?;
            assert!(!tasks.cancel("measurement"));
            assert!(
                Operation::with_events(tasks.clone(), Some("measurement".into()), |_| {}).is_err()
            );
            Ok("saved")
        });
        assert_eq!(result.unwrap(), "saved");
        assert!(!tasks.cancel("measurement"));
    }

    #[test]
    fn final_snapshot_preserves_throttled_counts_before_terminal_event_delivery() {
        let tasks = Operations::default();
        let observed = tasks.clone();
        let operation =
            Operation::with_events(tasks.clone(), Some("measurement".into()), move |event| {
                let event = serde_json::to_value(event).unwrap();
                if event["stage"] == "completed" {
                    let final_state =
                        serde_json::to_value(observed.status("measurement").unwrap()).unwrap();
                    assert_eq!(
                        serde_json::json!([
                            final_state["completed"],
                            final_state["failed"],
                            final_state["skipped"]
                        ]),
                        serde_json::json!([3, 1, 1])
                    );
                    assert_eq!(final_state, event);
                }
            })
            .unwrap();
        operation
            .run(|operation| {
                operation.measuring(3)?;
                operation.advance(FrameState::Measured)?;
                operation.advance(FrameState::Error)?;
                operation.advance(FrameState::NotInspected)?;
                operation.saving()
            })
            .unwrap();
        assert_eq!(
            serde_json::to_value(tasks.status("measurement").unwrap()).unwrap()["stage"],
            "completed"
        );
    }
}
