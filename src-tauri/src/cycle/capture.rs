use super::*;

impl Machine {
    pub(super) fn capture_devices_ready(&self) -> Result<(), String> {
        let rig = &host(&self.app).camera;
        let round = rig.recipe_capture().current_any().ok_or("没有正在执行的示教采集")?;
        let ids = capture_device_ids(&rig.configs(), &round.camera_ids())?;
        let cams = ids.iter().map(|id| rig.index_of(id).ok_or_else(|| format!("采集相机 {id} 已从设备配置移除"))).collect::<Result<Vec<_>, String>>()?;
        rig.check_ready_at(&cams)
    }

    pub(super) async fn poll_teaching(&mut self, reset: bool, base_ready: bool) {
        use crate::teaching::TeachEvent;
        let app = self.app.clone();
        let cycle = host(&app);
        let capture = cycle.camera.recipe_capture();
        let mut teaching = cycle.teaching.lock().await;
        if reset && teaching.busy() {
            capture.interrupt("示教事务复位，请整圈重采");
        }
        teaching.set_origin(crate::offline::enabled(&app));
        crate::offline::service_teaching_idle(&app, teaching.reset_pending() && base_ready);
        let was_busy = teaching.busy();
        if was_busy && self.s7.request().is_none() {
            self.close_routing();
            self.part = None;
            self.current_cycle_id = None;
            self.done_at = None;
            self.ack_alarmed = false;
        }
        let event = match teaching.poll(plc(&app), reset, base_ready).await {
            Ok(event) => event,
            Err(error) => teaching.force_fault(error),
        };
        match event {
            TeachEvent::Accepted => {
                self.set_phase(Phase::Acquire);
            }
            TeachEvent::Ended => {
                if capture.active() {
                    if let Err(error) = capture.burst_plc_ended(None) {
                        capture.fail(error);
                    }
                }
                self.set_phase(Phase::Drain);
            }
            TeachEvent::Rejected(reason) => {
                capture.fail(reason);
                if production_start_pending(plc(&app)).await.unwrap_or(false) {
                    cycle.teaching_preempted.store(true, Ordering::SeqCst);
                }
                if self.s7.request().is_none() {
                    self.set_phase(Phase::Release);
                }
            }
            TeachEvent::Released => {
                cycle.teaching_preempted.store(false, Ordering::SeqCst);
                if self.s7.request().is_none() {
                    self.set_phase(Phase::Idle);
                }
            }
            TeachEvent::Fault(reason) if was_busy => {
                capture.fail(reason.clone());
                // S7 已在故障时不改写它的原因，否则两边原因每拍互相覆盖、反复报故障
                if self.s7.phase() != SessionPhase::Fault {
                    self.s7.fault(plc(&app), reason.clone()).await;
                }
                self.enter_fault("teach", reason);
            }
            _ => {}
        }
        if teaching.busy()
            && !reset
            && matches!(self.s7.phase(), SessionPhase::Fault)
            && teaching.view().phase != "fault"
        {
            let reason = self
                .s7
                .view()
                .message
                .unwrap_or_else(|| "生产基础握手故障，示教需要复位".into());
            teaching.force_fault(reason.clone());
            capture.fail(reason);
        }
        let view = teaching.view();
        if view.phase == "fault" && teaching.busy() && self.s7.phase() != SessionPhase::Fault {
            self.s7.fault(plc(&app), view.message.clone()).await;
            self.enter_fault("teach", view.message.clone());
        }
        if matches!(
            view.phase.as_str(),
            "cancelling" | "fault" | "releasing" | "reporting"
        ) && capture.active()
        {
            capture.interrupt(view.message.clone());
        }
        if matches!(view.phase.as_str(), "requested" | "accepted" | "draining") {
            if let Some(round) = capture
                .current_any()
                .filter(|round| Some(round.round_id()) == teaching.bound_round_id())
            {
                if let Some((result, _)) = round.plc_report() {
                    let reason = match &round {
                        crate::recipe_capture::AnyRound::Burst(round) => match round
                            .failure
                            .as_ref()
                        {
                            Some(
                                crate::recipe_capture::CaptureFailureV2::Camera { .. }
                                | crate::recipe_capture::CaptureFailureV2::SessionChanged { .. },
                            ) => 3,
                            Some(crate::recipe_capture::CaptureFailureV2::WriteTimeout {
                                ..
                            }) => 5,
                            Some(crate::recipe_capture::CaptureFailureV2::Plc) => 6,
                            Some(crate::recipe_capture::CaptureFailureV2::StartTimeout) => 7,
                            _ => 4,
                        },
                    };
                    if let Err(error) = teaching.finish(plc(&app), result == 1, reason).await {
                        teaching.force_fault(error.clone());
                        self.s7.fault(plc(&app), error.clone()).await;
                        self.enter_fault("teach", error);
                    } else {
                        self.set_phase(Phase::Report);
                    }
                }
            }
        }
        cycle.store_teaching_busy(teaching.busy(), teaching.blocks_site_switch());
    }
}

/// 本轮采集的相机都还在设备配置里；PLC 每次触发同时触发这些相机。
fn capture_device_ids(configs: &[crate::camera::CameraConfig], selected: &[String]) -> Result<Vec<String>, String> {
    if selected.is_empty() { return Err("采集轮次没有相机".into()); }
    if let Some(missing) = selected.iter().find(|id| !configs.iter().any(|c| &c.id == *id)) {
        return Err(format!("采集相机 {missing} 已从设备配置移除"));
    }
    Ok(selected.to_vec())
}



/// 示教采集期间的复位 / 就绪判定：只放宽"没有可开工配方"，生产生效版本、工件身份、测量线程与采集相机仍须就绪。
pub(super) fn capture_readiness(service: Result<(), String>, device: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    service.and_then(|()| device())
}
