use super::*;
use crate::plc_plan::{PlanShot, PlcPlan};
use crate::recipe_capture::CaptureState;

impl Machine {
    pub(super) fn capture_devices_ready(&self) -> Result<(), String> {
        let rig = &host(&self.app).camera;
        let round = rig.recipe_capture().current().ok_or("没有正在执行的示教采集")?;
        let ids = capture_device_ids(&rig.configs(), &round.camera_ids())?;
        let cams = ids.iter().map(|id| rig.index_of(id).ok_or_else(|| format!("采集相机 {id} 已从设备配置移除"))).collect::<Result<Vec<_>, String>>()?;
        rig.check_ready_at(&cams)
    }

    pub(super) async fn start_capture_s7(&mut self, request: Request) {
        self.part = None;
        self.current_cycle_id = None;
        self.done_at = None;
        self.ack_alarmed = false;
        let app = self.app.clone();
        let rig = &host(&app).camera;
        let capture = rig.recipe_capture().clone();
        let attempt = async {
            let round = capture.current().ok_or("没有正在执行的示教采集")?;
            let device_ids = capture_device_ids(&rig.configs(), &round.camera_ids())?;
            if request.device_shots != request.shot_count {
                return Err("示教采集设备触发数量必须等于计划总数".into());
            }
            self.capture_devices_ready()?;
            let shots = (0..request.shot_count).map(|k| PlanShot {
                shot_id: format!("S{:03}", k + 1), pose_id: format!("采集序号{}", k + 1), camera_ids: device_ids.clone(),
            }).collect();
            let plan = PlcPlan::compile(round.recipe_id.clone(), request.plan_version, device_ids, shots)?;
            self.s7.bind_capture_id(&round.round_id)?;
            self.s7.validate_plan(&plan)?;
            capture.set_plan_version(request.plan_version)?;
            capture.plc_started(u32::from(request.shot_count))?;
            self.s7.arm(plc(&app), &plan).await
        }.await;
        match attempt {
            Ok(()) => {
                self.set_phase(Phase::Acquire);
                log(&app, "info", "示教采集布防", "已接受 PLC 采集事务，等待各相机硬触发图像；本轮不产生生产检测结果");
            }
            Err(error) => {
                capture.fail(error.clone());
                self.s7.fault(plc(&app), error.clone()).await;
                self.enter_fault(error);
            }
        }
    }

    pub(super) fn end_capture(&self) {
        let rig = &host(&self.app).camera;
        let capture = rig.recipe_capture();
        let result = (|| {
            let round = capture.current().ok_or("采集轮次不存在")?;
            capture_device_ids(&rig.configs(), &round.camera_ids())?;
            let count = read_tag_u32(plc(&self.app), tag::DEVICE_TRIGGERS).ok_or("PLC 缺少设备实际触发计数")?;
            capture.plc_ended(count)
        })();
        if let Err(error) = result { capture.fail(error); }
    }

    pub(super) async fn finish_capture_s7(&mut self) {
        if !matches!(self.s7.phase(), SessionPhase::Acquiring | SessionPhase::Draining) { return; }
        let app = self.app.clone();
        let capture = host(&app).camera.recipe_capture();
        let Some(round) = capture.current().filter(|round| Some(round.round_id.as_str()) == self.s7.capture_id()) else { return; };
        let (result, code) = match round.state {
            CaptureState::Complete => (1, 0),
            CaptureState::Failed => (90, fault::SHOT_COUNT_MISMATCH),
            _ => return,
        };
        match self.s7.report(plc(&app), result, code).await {
            Ok(()) => {
                self.done_at = Some(Instant::now());
                self.set_phase(Phase::Report);
                log(&app, if result == 1 { "ok" } else { "err" }, "示教采集交付", format!("采集轮次 {} 完成码 {result}；等待 PLC 确认，不计入生产 OK/NG", round.round_id));
            }
            Err(error) => self.enter_fault(error),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teaching_capture_arms_every_selected_camera_but_rejects_a_removed_one() {
        let device = |id: &str| crate::camera::CameraConfig { id: id.into(), view_count: 1, ..Default::default() };
        let configs = [device("cam1"), device("cam2"), device("cam3")];
        let selected = ["cam1".to_string(), "cam3".to_string()];
        assert_eq!(capture_device_ids(&configs, &selected).unwrap(), selected);
        assert!(capture_device_ids(&configs[..2], &selected).unwrap_err().contains("cam3"));
        assert!(capture_device_ids(&configs, &[]).is_err());
    }
}

/// 示教采集期间的复位 / 就绪判定：只放宽"没有可开工配方"，生产生效版本、工件身份、测量线程与采集相机仍须就绪。
pub(super) fn capture_readiness(service: Result<(), String>, device: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    service.and_then(|()| device())
}
