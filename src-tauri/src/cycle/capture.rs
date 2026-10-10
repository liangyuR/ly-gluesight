use super::*;
use crate::plc_plan::{PlanShot, PlcPlan};
use crate::recipe_capture::CaptureState;

impl Machine {
    pub(super) fn capture_devices_ready(&self) -> Result<(), String> {
        let rig = &host(&self.app).camera;
        let round = rig.recipe_capture().current().ok_or("没有正在执行的示教采集")?;
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
            let configs = rig.configs();
            let slots: [String; 3] = std::array::from_fn(|i| configs.get(i).map(|c| c.id.clone()).unwrap_or_default());
            let slot = slots.iter().position(|id| id == &round.camera_id).ok_or("采集 device 未映射到 PLC 槽")?;
            if request.camera_shots.iter().enumerate().any(|(i, &n)| (i == slot && n != request.shot_count) || (i != slot && n != 0)) {
                return Err("示教采集只允许所选 device 对应的一个 PLC 槽触发，数量必须等于计划总数".into());
            }
            self.capture_devices_ready()?;
            let shots = (0..request.shot_count).map(|k| PlanShot {
                shot_id: format!("S{:03}", k + 1), pose_id: format!("采集序号{}", k + 1), camera_id: round.camera_id.clone(),
            }).collect();
            let plan = PlcPlan::compile(round.recipe_id.clone(), request.plan_version, slots, shots)?;
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
            let slot = rig.index_of(&round.camera_id).ok_or("采集设备不存在")? as usize;
            if slot >= 3 { return Err("采集设备没有 PLC 触发计数槽".into()); }
            let counts = tag::CAMERA_TRIGGERS.iter().map(|tag| read_tag_u32(plc(&self.app), tag).ok_or_else(|| format!("PLC 缺少实际触发计数 {tag}"))).collect::<Result<Vec<_>, _>>()?;
            if counts.iter().enumerate().any(|(i, &count)| i != slot && count != 0) {
                return Err("非采集 device 的 PLC 槽也发生了触发，请核对现场程序".into());
            }
            capture.plc_ended(counts[slot])
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

    pub(super) async fn capture_edge(&mut self, edge: &EdgeEvent) {
        let capture = host(&self.app).camera.recipe_capture().clone();
        if edge.tags.iter().any(|tag| tag == tag::PART_START) {
            if let Some(version) = read_tag_u32(plc(&self.app), tag::PLAN_VERSION).filter(|v| *v > 0) {
                if let Err(error) = capture.set_plan_version(version) { capture.fail(error); return; }
            }
            let result = read_tag_u32(plc(&self.app), tag::SHOT_COUNT).ok_or_else(|| "PLC 缺少计划拍照数量".to_owned())
                .and_then(|count| capture.plc_started(count));
            if let Err(error) = result { capture.fail(error); }
        } else if edge.tags.iter().any(|tag| tag == tag::PART_END) {
            self.end_capture();
        }
    }
}
