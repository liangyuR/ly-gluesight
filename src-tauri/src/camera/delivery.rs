use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use tokio::sync::mpsc::{error::TrySendError, Sender};

use crate::frame::{CounterSource, Frame};
use crate::shot_router::{FrameMeta, Ledgers};

pub(super) fn observe(identity: &Mutex<Ledgers>, manual: &AtomicU32, frame: &mut Frame, current_session: bool) {
    if current_session {
        let mut ledger = identity.lock().unwrap();
        let advanced = ledger.observe(&FrameMeta::from(&*frame));
        if frame.counter != CounterSource::Synthetic {
            frame.manual = advanced && manual.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
        }
    } else if frame.counter != CounterSource::Synthetic {
        frame.manual = false;
    }
}

pub(super) fn offer(tx: &Sender<Frame>, dropped: &AtomicU64, frame: Frame) -> Result<(), TrySendError<Frame>> {
    let result = tx.try_send(frame);
    if result.is_err() {
        dropped.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// 图像内存预算满、回调没拷图的帧（D-SB-32）：与回调通道满一样，台账照常前进、帧不交付，节拍据台账按缺帧处理；
/// 单独计入“内存预算满”丢帧。
pub(super) fn discard_over_budget(identity: &Mutex<Ledgers>, manual: &AtomicU32, budget_dropped: &AtomicU64, frame: &mut Frame, current_session: bool) {
    observe(identity, manual, frame, current_session);
    budget_dropped.fetch_add(1, Ordering::Relaxed);
}
