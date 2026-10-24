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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::frame::FrameImage;

    fn frame(cam: u8, counter: u64) -> Frame {
        Frame {
            cam, session: 11 + cam as u64, counter: CounterSource::ChunkTrigger,
            frame_counter: counter, trigger_counter: counter, lost_packets: 0, ts: 0, manual: false,
            images: (1..=3).map(|view| Arc::new(FrameImage::new(1, 1, vec![view]))).collect(),
        }
    }

    fn callback(identity: &Mutex<Ledgers>, manual: &AtomicU32, tx: &Sender<Frame>, dropped: &AtomicU64, mut frame: Frame) -> Result<(), TrySendError<Frame>> {
        observe(identity, manual, &mut frame, true);
        offer(tx, dropped, frame)
    }

    #[test]
    fn duplicate_or_stale_callbacks_do_not_consume_another_manual_trigger() {
        let identity = Mutex::new(Ledgers::default());
        let manual = AtomicU32::new(2);
        let mut first = frame(0, 1);
        observe(&identity, &manual, &mut first, true);
        assert!(first.manual);
        assert_eq!(manual.load(Ordering::SeqCst), 1);
        let mut duplicate = frame(0, 1);
        observe(&identity, &manual, &mut duplicate, true);
        assert!(!duplicate.manual);
        let mut stale = frame(0, 2);
        stale.manual = true;
        observe(&identity, &manual, &mut stale, false);
        assert!(!stale.manual);
        assert_eq!(manual.load(Ordering::SeqCst), 1);
        assert_eq!(identity.lock().unwrap().get(0).unwrap().last, Some(1));
        let mut synthetic = frame(1, 1);
        synthetic.counter = CounterSource::Synthetic;
        synthetic.manual = true;
        observe(&identity, &manual, &mut synthetic, true);
        assert!(synthetic.manual);
        assert_eq!(manual.load(Ordering::SeqCst), 1);
    }

    /// 规格 §3 丢帧只告警（段配方）：回调通道满或图像内存预算满时这一帧不交给节拍，该组标检测不完整，
    /// 其余相机照常判，整件不判 ERR。
    #[test]
    fn dropped_callback_frame_becomes_incomplete_group_not_err() {
        use crate::cycle::tests::{burst_frame, judge_burst_delivered};
        for (case, budget_full) in [("回调通道满", false), ("图像内存预算满", true)] {
            let (tx, mut rx) = channel::<Frame>(1);
            let identity = Mutex::new(Ledgers::default());
            let (manual, dropped, budget_dropped) = (AtomicU32::new(0), AtomicU64::new(0), AtomicU64::new(0));
            let mut received = Vec::new();
            for counter in 1..=4 {
                for cam in 0..3u8 {
                    let mut frame = burst_frame(cam, counter);
                    if (cam, counter) != (1, 2) {
                        callback(&identity, &manual, &tx, &dropped, frame).unwrap();
                        received.push(rx.try_recv().unwrap());
                    } else if budget_full {
                        discard_over_budget(&identity, &manual, &budget_dropped, &mut frame, true);
                    } else {
                        // 节拍来不及取走上一帧，通道占满
                        tx.try_send(burst_frame(0, 999)).unwrap();
                        assert!(matches!(callback(&identity, &manual, &tx, &dropped, frame), Err(TrySendError::Full(_))), "{case}");
                        rx.try_recv().unwrap();
                    }
                }
            }
            let judgement = judge_burst_delivered(received, Some(4));
            assert_eq!((judgement.verdict, judgement.fault_code), (crate::judge::Verdict::Ok, 0), "{case}");
            assert!(judgement.flags.incomplete, "{case}");
            assert_eq!(judgement.regions[0].incomplete_groups, vec![2], "{case}");
        }
    }
}
