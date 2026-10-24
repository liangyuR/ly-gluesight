use std::future::Future;
use std::time::Duration;
// tokio 时钟：生产中就是系统单调时钟，测试里可用 `start_paused` 推进虚拟时间
use tokio::time::Instant;

pub struct ArmStage {
    pub stage: String,
    pub elapsed: Duration,
    pub interval: Duration,
}

pub struct ArmBudget {
    started: Instant,
    deadline: Option<Instant>,
    timeout: Duration,
    last_elapsed: Duration,
    stages: Vec<ArmStage>,
}

impl ArmBudget {

    pub fn with_start(started: impl Into<Instant>, timeout: Duration) -> Self {
        let started = started.into();
        Self {
            started,
            deadline: started.checked_add(timeout),
            timeout,
            last_elapsed: Duration::ZERO,
            stages: Vec::new(),
        }
    }

    pub fn remaining(&mut self, stage: &str) -> Result<Duration, String> {
        self.remaining_at(stage, Instant::now())
    }

    fn remaining_at(&mut self, stage: &str, now: Instant) -> Result<Duration, String> {
        if let Some(remaining) = self.deadline.and_then(|deadline| deadline.checked_duration_since(now))
            .filter(|remaining| !remaining.is_zero()) {
            return Ok(remaining);
        }
        let elapsed = self.mark_at(stage, now);
        Err(format!(
            "布防总预算超时或截止时间无效：阶段={stage}，耗时={:.3} ms，预算={:.3} ms",
            elapsed.as_secs_f64() * 1000.0,
            self.timeout.as_secs_f64() * 1000.0,
        ))
    }

    pub async fn run<T>(
        &mut self,
        stage: &str,
        work: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        let remaining = self.remaining(stage)?;
        let result = tokio::time::timeout(remaining, work).await;
        self.mark(stage);
        self.remaining(stage)?;
        match result {
            Ok(result) => result.map_err(|error| format!("布防阶段={stage}失败：{error}")),
            Err(_) => Err(format!(
                "布防阶段={stage}超过剩余总预算，累计耗时={:.3} ms，预算={:.3} ms",
                self.elapsed().as_secs_f64() * 1000.0,
                self.timeout.as_secs_f64() * 1000.0,
            )),
        }
    }

    pub async fn run_serial<T>(
        &mut self,
        stage: &str,
        work: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        self.remaining(stage)?;
        let result = work.await;
        self.mark(stage);
        self.remaining(stage)?;
        result.map_err(|error| format!("布防阶段={stage}失败：{error}"))
    }

    pub fn mark(&mut self, stage: &str) -> Duration {
        self.mark_at(stage, Instant::now())
    }

    fn mark_at(&mut self, stage: &str, now: Instant) -> Duration {
        let elapsed = now.saturating_duration_since(self.started);
        self.stages.push(ArmStage {
            stage: stage.into(),
            elapsed,
            interval: elapsed.saturating_sub(self.last_elapsed),
        });
        self.last_elapsed = elapsed;
        elapsed
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    #[cfg(test)]
    pub fn stages(&self) -> &[ArmStage] {
        &self.stages
    }

    pub fn trace(&self) -> serde_json::Value {
        serde_json::json!({
            "budgetMs": self.timeout.as_secs_f64() * 1000.0,
            "elapsedMs": self.elapsed().as_secs_f64() * 1000.0,
            "stages": self.stages.iter().map(|stage| serde_json::json!({
                "stage": stage.stage,
                "elapsedMs": stage.elapsed.as_secs_f64() * 1000.0,
                "intervalMs": stage.interval.as_secs_f64() * 1000.0,
            })).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn serial_arm_waits_for_side_effect_settlement_before_cleanup_and_rejects_late_ok() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let settled = AtomicBool::new(false);
        let mut budget = ArmBudget::with_start(Instant::now(), Duration::from_millis(35));
        let result = budget.run_serial("plc-journal", async {
            tokio::time::sleep(Duration::from_millis(120)).await;
            settled.store(true, Ordering::SeqCst);
            Ok("completed after deadline")
        }).await;
        assert!(settled.load(Ordering::SeqCst));
        assert!(result.is_err());
        assert_eq!(budget.stages().last().unwrap().elapsed, Duration::from_millis(120));
    }
}
