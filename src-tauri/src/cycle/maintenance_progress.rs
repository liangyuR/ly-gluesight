use std::time::{Duration, Instant};

use tokio::sync::oneshot::{self, error::TryRecvError};

use crate::settings::Maintenance;
use crate::store::{CollectionProgress, TrialProgress};

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Default, PartialEq)]
pub(super) struct ProgressData {
    pub(super) collection: Vec<CollectionProgress>,
    pub(super) trial: Vec<TrialProgress>,
}

#[derive(Default)]
pub(super) struct MaintenanceProgress {
    mode: Option<Maintenance>,
    generation: u64,
    data: ProgressData,
    completed_at: Option<Instant>,
    in_flight: Option<(u64, oneshot::Receiver<ProgressData>)>,
}

impl MaintenanceProgress {
    pub(super) fn poll(
        &mut self,
        mode: Option<&Maintenance>,
        now: Instant,
        load: impl FnOnce() -> ProgressData + Send + 'static,
    ) -> bool {
        let mut changed = self.mode.as_ref() != mode;
        if changed {
            self.mode = mode.cloned();
            self.generation = self.generation.wrapping_add(1);
            self.data = ProgressData::default();
            self.completed_at = None;
        }

        if let Some((generation, response)) = &mut self.in_flight {
            let data = match response.try_recv() {
                Ok(data) => Some(data),
                Err(TryRecvError::Empty) => return changed,
                Err(TryRecvError::Closed) => None,
            };
            if *generation == self.generation {
                self.completed_at = Some(now);
                if let Some(data) = data.filter(|data| *data != self.data) {
                    self.data = data;
                    changed = true;
                }
            }
            self.in_flight = None;
        }

        if mode.is_some()
            && self
                .completed_at
                .is_none_or(|completed| now.duration_since(completed) >= REFRESH_INTERVAL)
        {
            let (send, receive) = oneshot::channel();
            self.in_flight = Some((self.generation, receive));
            tokio::task::spawn_blocking(move || {
                let _ = send.send(load());
            });
        }
        changed
    }

    pub(super) fn snapshot(&self, mode: Option<&Maintenance>) -> ProgressData {
        if self.mode.as_ref() == mode {
            self.data.clone()
        } else {
            ProgressData::default()
        }
    }
}
