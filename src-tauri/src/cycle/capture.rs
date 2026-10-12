use super::*;
use crate::plc_plan::{PlanShot, PlcPlan};
use crate::recipe_capture::CaptureState;

impl Machine {
    pub(super) fn capture_devices_ready(&self) -> Result<(), String> {
        let rig = &host(&self.app).camera;
        let round = rig.recipe_capture().current().ok_or("没有正在执行的示教采集")?;
        capture_device_id(&rig.configs(), &round.camera_id)?;
        let index = rig.index_of(&round.camera_id).ok_or("采集 device 已从设备配置移除")?;
        rig.check_ready_at(&[index])
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
            let device_id = capture_device_id(&rig.configs(), &round.camera_id)?;
            if request.device_shots != request.shot_count {
                return Err("示教采集设备触发数量必须等于计划总数".into());
            }
            self.capture_devices_ready()?;
            let shots = (0..request.shot_count).map(|k| PlanShot {
                shot_id: format!("S{:03}", k + 1), pose_id: format!("采集序号{}", k + 1), camera_id: round.camera_id.clone(),
            }).collect();
            let plan = PlcPlan::compile(round.recipe_id.clone(), request.plan_version, device_id, shots)?;
            self.s7.bind_capture_id(&round.round_id)?;
            self.s7.validate_plan(&plan)?;
            capture.set_plan_version(request.plan_version)?;
            capture.plc_started(u32::from(request.shot_count))?;
            self.s7.arm(plc(&app), &plan).await
        }.await;
        match attempt {
            Ok(()) => {
                self.set_phase(Phase::Acquire);
                log(&app, "info", "示教采集布防", "已接受 PLC 采集事务，等待硬触发拼接图；本轮不产生生产检测结果");
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
            capture_device_id(&rig.configs(), &round.camera_id)?;
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

fn capture_device_id(configs: &[crate::camera::CameraConfig], selected_id: &str) -> Result<String, String> {
    let device_id = crate::plc::sole_device_id(configs)?;
    if device_id != selected_id { return Err("采集设备与唯一相机设备不一致".into()); }
    Ok(device_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teaching_capture_rejects_multiple_or_mismatched_devices() {
        let device = crate::camera::CameraConfig { id: "device1".into(), view_count: 3, ..Default::default() };
        assert_eq!(capture_device_id(&[device.clone()], "device1").unwrap(), "device1");
        assert!(capture_device_id(&[device.clone()], "device2").is_err());
        assert!(capture_device_id(&[device.clone(), crate::camera::CameraConfig { id: "device2".into(), ..device }], "device1").is_err());
        assert!(capture_device_id(&[], "device1").is_err());
    }
}
