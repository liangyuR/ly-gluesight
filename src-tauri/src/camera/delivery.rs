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
            frame.manual = advanced && manual.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::camera::FRAME_QUEUE;
    use crate::frame::FrameImage;
    use crate::judge::fault;
    use crate::shot_router::{ArmCam, Plan, Policy, Route, ShotRouter};

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

    fn router() -> ShotRouter {
        let plan = Plan::new([("P1", "cam1"), ("P2", "cam1"), ("P3", "cam1"), ("P4", "cam1")]);
        let camera = ArmCam { cam: 0, camera: "cam1".into(), session: 11, source: Some(CounterSource::ChunkTrigger), counter_after_open: Some(0) };
        ShotRouter::arm(&plan, &Ledgers::default(), &Policy::production(None), &[camera]).unwrap()
    }

    #[test]
    fn full_frame_channel_advances_callback_ledger_counts_drops_and_recovers() {
        let (tx, mut rx) = channel::<Frame>(FRAME_QUEUE);
        let identity = Mutex::new(Ledgers::default());
        let manual = AtomicU32::new(0);
        let dropped = AtomicU64::new(0);
        for counter in 1..=FRAME_QUEUE as u64 {
            callback(&identity, &manual, &tx, &dropped, frame(0, counter)).unwrap();
        }
        assert_eq!(rx.len(), FRAME_QUEUE);
        for counter in [65, 66] {
            let Err(TrySendError::Full(rejected)) = callback(&identity, &manual, &tx, &dropped, frame(0, counter)) else {
                panic!("满帧通道必须拒绝完整设备帧");
            };
            assert_eq!(rejected.trigger_counter, counter);
            assert_eq!(rejected.images.len(), 3);
        }
        {
            let ledgers = identity.lock().unwrap();
            let ledger = ledgers.get(0).unwrap();
            assert_eq!(ledger.last, Some(66));
            assert_eq!(ledger.frames, 66);
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        for counter in 1..=FRAME_QUEUE as u64 {
            let accepted = rx.try_recv().unwrap();
            assert_eq!(accepted.trigger_counter, counter);
            assert_eq!(accepted.images.len(), 3);
        }
        callback(&identity, &manual, &tx, &dropped, frame(0, 67)).unwrap();
        assert_eq!(rx.try_recv().unwrap().trigger_counter, 67);
        assert!(rx.try_recv().is_err());
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        assert_eq!(identity.lock().unwrap().get(0).unwrap().last, Some(67));
        assert_eq!(identity.lock().unwrap().get(0).unwrap().frames, 67);
    }

    #[test]
    fn closed_frame_channel_still_advances_callback_ledger_and_counts_each_drop() {
        let (tx, rx) = channel(FRAME_QUEUE);
        drop(rx);
        let identity = Mutex::new(Ledgers::default());
        let manual = AtomicU32::new(2);
        let dropped = AtomicU64::new(0);
        for counter in 1..=2 {
            let Err(TrySendError::Closed(rejected)) = callback(&identity, &manual, &tx, &dropped, frame(0, counter)) else {
                panic!("关闭帧通道必须返回 Closed");
            };
            assert!(rejected.manual);
            assert_eq!(rejected.trigger_counter, counter);
            assert_eq!(dropped.load(Ordering::Relaxed), counter);
        }
        assert_eq!(manual.load(Ordering::SeqCst), 0);
        assert_eq!(identity.lock().unwrap().get(0).unwrap().last, Some(2));
        assert_eq!(identity.lock().unwrap().get(0).unwrap().frames, 2);
    }

    #[test]
    fn observation_without_an_offer_keeps_idle_baseline_and_manual_marking() {
        let (tx, mut rx) = channel::<Frame>(FRAME_QUEUE);
        let identity = Mutex::new(Ledgers::default());
        let manual = AtomicU32::new(2);
        let dropped = AtomicU64::new(0);
        for counter in 1..=2 {
            let mut observed = frame(0, counter);
            observe(&identity, &manual, &mut observed, true);
            assert!(observed.manual);
        }
        assert!(rx.try_recv().is_err());
        assert_eq!(tx.capacity(), FRAME_QUEUE);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert_eq!(manual.load(Ordering::SeqCst), 0);
        assert_eq!(identity.lock().unwrap().get(0).unwrap().last, Some(2));
        assert_eq!(identity.lock().unwrap().get(0).unwrap().frames, 2);
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

    #[test]
    fn full_callback_channel_keeps_first_middle_and_last_missing_shots_in_place() {
        for missing in [0, 1, 3] {
            let (tx, mut rx) = channel::<Frame>(FRAME_QUEUE);
            let identity = Mutex::new(Ledgers::default());
            let manual = AtomicU32::new(0);
            let dropped = AtomicU64::new(0);
            let mut router = router();
            let mut bound = Vec::new();
            for shot in 0..4 {
                if shot == missing {
                    for counter in 1..=FRAME_QUEUE as u64 {
                        callback(&identity, &manual, &tx, &dropped, frame(1, counter)).unwrap();
                    }
                    assert_eq!(rx.len(), FRAME_QUEUE);
                }
                let result = callback(&identity, &manual, &tx, &dropped, frame(0, shot as u64 + 1));
                if shot == missing {
                    assert!(matches!(result, Err(TrySendError::Full(_))));
                } else {
                    result.unwrap();
                }
                while let Ok(received) = rx.try_recv() {
                    let route = router.route(&FrameMeta::from(&received));
                    if received.cam == 0 {
                        assert_eq!(route, Route::Bound { shot, ordinal: shot as u64 + 1 });
                        bound.push(shot);
                    } else {
                        assert_eq!(route, Route::UnknownCamera);
                    }
                }
            }
            assert_eq!(bound, (0..4).filter(|shot| *shot != missing).collect::<Vec<_>>());
            assert_eq!(router.missing(), [missing]);
            assert!(!router.complete());
            assert_eq!(dropped.load(Ordering::Relaxed), 1);
            let ledgers = identity.lock().unwrap();
            let ledger = ledgers.get(0).unwrap();
            assert_eq!(ledger.last, Some(4));
            assert_eq!(ledger.frames, 4);
            assert!(router.check_observed(0, ledger).is_ok());
        }
    }

    #[test]
    fn extra_callback_lost_to_full_channel_is_still_rejected_by_router_summary() {
        let (tx, mut rx) = channel::<Frame>(FRAME_QUEUE);
        let identity = Mutex::new(Ledgers::default());
        let manual = AtomicU32::new(0);
        let dropped = AtomicU64::new(0);
        let mut router = router();
        for shot in 0..4 {
            callback(&identity, &manual, &tx, &dropped, frame(0, shot as u64 + 1)).unwrap();
            let received = rx.try_recv().unwrap();
            assert_eq!(router.route(&FrameMeta::from(&received)), Route::Bound { shot, ordinal: shot as u64 + 1 });
        }
        assert!(router.complete());
        for counter in 1..=FRAME_QUEUE as u64 {
            callback(&identity, &manual, &tx, &dropped, frame(1, counter)).unwrap();
        }
        assert!(matches!(callback(&identity, &manual, &tx, &dropped, frame(0, 5)), Err(TrySendError::Full(_))));
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        let ledgers = identity.lock().unwrap();
        let ledger = ledgers.get(0).unwrap();
        assert_eq!(ledger.last, Some(5));
        assert_eq!(ledger.frames, 5);
        let (code, reason) = router.check_observed(0, ledger).unwrap_err();
        assert_eq!(code, fault::EXTRA_FRAME);
        assert!(reason.contains('5') && reason.contains('4'));
        assert!(router.complete());
    }

    #[test]
    fn full_callback_channel_finishes_missing_first_middle_and_last_as_plc_error() {
        for missing in [0, 1, 3] {
            let (tx, mut rx) = channel::<Frame>(FRAME_QUEUE);
            let identity = Mutex::new(Ledgers::default());
            let manual = AtomicU32::new(0);
            let dropped = AtomicU64::new(0);
            for counter in 1..=FRAME_QUEUE as u64 {
                callback(&identity, &manual, &tx, &dropped, frame(0, counter)).unwrap();
            }
            let baseline = identity.lock().unwrap().clone();
            let mut received = Vec::new();
            if missing != 0 {
                while let Ok(stale) = rx.try_recv() { received.push(stale); }
            }
            for shot in 0..4 {
                if shot == missing && shot != 0 {
                    for counter in 1..=FRAME_QUEUE as u64 {
                        callback(&identity, &manual, &tx, &dropped, frame(1, counter)).unwrap();
                    }
                }
                let result = callback(&identity, &manual, &tx, &dropped, frame(0, 65 + shot));
                if shot == missing {
                    assert!(matches!(result, Err(TrySendError::Full(_))));
                } else {
                    result.unwrap();
                }
                while let Ok(accepted) = rx.try_recv() {
                    if accepted.cam == 0 { received.push(accepted); }
                }
            }
            assert_eq!(dropped.load(Ordering::Relaxed), 1);
            let (judgement, shots) = crate::cycle::tests::judge_callback_channel(
                &baseline, received, &identity.lock().unwrap());
            assert_eq!(judgement.verdict, crate::judge::Verdict::ErrInspect);
            assert_eq!(judgement.plc_code, 90);
            assert_eq!(judgement.fault_code, fault::MISSING_FRAME);
            for (k, shot) in shots.iter().enumerate() {
                assert_eq!(shot.shot_id, format!("P{}", k + 1));
                assert_eq!(shot.status, if k == missing as usize {
                    crate::cycle::FrameStatus::Missing
                } else { crate::cycle::FrameStatus::Done });
                assert_eq!(shot.trigger_counter, (k != missing as usize).then_some(65 + k as u64));
            }
            let recovered_baseline = identity.lock().unwrap().clone();
            let mut recovered = Vec::new();
            for counter in 69..=72 {
                callback(&identity, &manual, &tx, &dropped, frame(0, counter)).unwrap();
                recovered.push(rx.try_recv().unwrap());
            }
            let (judgement, shots) = crate::cycle::tests::judge_callback_channel(
                &recovered_baseline, recovered, &identity.lock().unwrap());
            assert_eq!(judgement.verdict, crate::judge::Verdict::Ok);
            assert_eq!(judgement.plc_code, 1);
            assert_eq!(judgement.fault_code, 0);
            assert!(shots.iter().all(|shot| shot.status == crate::cycle::FrameStatus::Done));
        }
    }

    #[test]
    fn extra_callback_dropped_by_full_channel_overrides_complete_ok_with_plc_error() {
        let (tx, mut rx) = channel::<Frame>(FRAME_QUEUE);
        let identity = Mutex::new(Ledgers::default());
        let manual = AtomicU32::new(0);
        let dropped = AtomicU64::new(0);
        for counter in 1..=FRAME_QUEUE as u64 {
            callback(&identity, &manual, &tx, &dropped, frame(0, counter)).unwrap();
        }
        let baseline = identity.lock().unwrap().clone();
        while rx.try_recv().is_ok() {}
        let mut received = Vec::new();
        for counter in 65..=68 {
            callback(&identity, &manual, &tx, &dropped, frame(0, counter)).unwrap();
            received.push(rx.try_recv().unwrap());
        }
        let (normal, shots) = crate::cycle::tests::judge_callback_channel(
            &baseline, received.clone(), &identity.lock().unwrap());
        assert_eq!(normal.verdict, crate::judge::Verdict::Ok);
        assert!(shots.iter().all(|shot| shot.status == crate::cycle::FrameStatus::Done));
        for counter in 1..=FRAME_QUEUE as u64 {
            callback(&identity, &manual, &tx, &dropped, frame(1, counter)).unwrap();
        }
        assert!(matches!(callback(&identity, &manual, &tx, &dropped, frame(0, 69)), Err(TrySendError::Full(_))));
        let (judgement, shots) = crate::cycle::tests::judge_callback_channel(
            &baseline, received, &identity.lock().unwrap());
        assert!(shots.iter().all(|shot| shot.status == crate::cycle::FrameStatus::Done));
        assert_eq!(judgement.verdict, crate::judge::Verdict::ErrInspect);
        assert_eq!(judgement.plc_code, 90);
        assert_eq!(judgement.fault_code, fault::EXTRA_FRAME);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        while rx.try_recv().is_ok() {}
        let recovered_baseline = identity.lock().unwrap().clone();
        let mut recovered = Vec::new();
        for counter in 70..=73 {
            callback(&identity, &manual, &tx, &dropped, frame(0, counter)).unwrap();
            recovered.push(rx.try_recv().unwrap());
        }
        let (judgement, shots) = crate::cycle::tests::judge_callback_channel(
            &recovered_baseline, recovered, &identity.lock().unwrap());
        assert_eq!(judgement.verdict, crate::judge::Verdict::Ok);
        assert_eq!(judgement.plc_code, 1);
        assert_eq!(judgement.fault_code, 0);
        assert!(shots.iter().all(|shot| shot.status == crate::cycle::FrameStatus::Done));
    }

}
