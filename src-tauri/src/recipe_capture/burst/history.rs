//! 历史件导入为采集轮次（docs/architecture/online-commissioning.md 5.2）：维护模式或生产录制的完整件物化成一轮已完成的
//! 采集，目录结构与示教实拍的轮次相同，原图是指向录制原图的硬链接（同一卷，不复制）；之后录制被清理，轮次原图仍可读。

use std::path::PathBuf;

use super::*;

/// 一帧历史原图：本件相机下标、帧组序号与录制时的帧号、尺寸，`source` 是录制目录里的原图。
pub struct HistoryFrame {
    pub camera: usize,
    pub seq: u64,
    pub frame_counter: u64,
    pub width: u32,
    pub height: u32,
    pub ts: i64,
    pub source: PathBuf,
}

/// 要物化的一件历史件：相机按录制的计划相机顺序，每台相机 1..=group_count 的帧都要齐。
pub struct HistoryRound {
    pub recipe_id: String,
    pub origin: RoundOrigin,
    pub cameras: Vec<CaptureCamera>,
    pub group_count: u64,
    pub frames: Vec<HistoryFrame>,
}

pub(super) fn valid_cycle_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// 录制原图是普通文件（不是链接或 reparse point），长度等于这一尺寸的 PGM。
fn check_source(path: &Path, width: u32, height: u32) -> Result<(), String> {
    crate::image_file::checked_source_path(path)?;
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("历史原图 {} 不可用：{e}", path.display()))?;
    let expected =
        format!("P5\n{width} {height}\n255\n").len() as u64 + u64::from(width) * u64::from(height);
    if !meta.is_file() || meta.len() != expected {
        return Err(format!(
            "历史原图 {} 不是普通文件或长度与录制尺寸不符",
            path.display()
        ));
    }
    Ok(())
}

impl RecipeCaptureHost {
    /// 历史件物化成一轮：逐帧建硬链接、写 frames.jsonl，round.json 最后写（没有它的目录不算轮次）；任一步失败删掉本轮目录，
    /// 删的只是链接，录制原图不受影响。
    pub fn import_history(&self, input: HistoryRound) -> Result<CaptureRoundV2, String> {
        let (round_id, created_at) = self.new_round_dir()?;
        let dir = self.root.join(&round_id);
        let result = self.materialize(round_id, &dir, created_at, input);
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        result
    }

    fn materialize(
        &self,
        round_id: String,
        dir: &Path,
        created_at: i64,
        input: HistoryRound,
    ) -> Result<CaptureRoundV2, String> {
        let HistoryRound {
            recipe_id,
            origin,
            cameras,
            group_count,
            frames,
        } = input;
        if !(1..=MAX_SEQ).contains(&group_count) {
            return Err("历史件没有帧组或帧组数超出上限".into());
        }
        for camera in &cameras {
            if !crate::recipe::valid_camera_id(&camera.camera_id) {
                return Err(format!("相机编号 {:?} 无效", camera.camera_id));
            }
            std::fs::create_dir(dir.join(&camera.camera_id)).map_err(|e| e.to_string())?;
        }
        let mut saved = vec![BTreeSet::new(); cameras.len()];
        let mut lines = Vec::with_capacity(frames.len());
        for frame in frames {
            let camera = cameras
                .get(frame.camera)
                .ok_or("历史帧引用了本件之外的相机")?;
            if !(1..=group_count).contains(&frame.seq) || !saved[frame.camera].insert(frame.seq) {
                return Err("历史帧的帧组序号越界或重复".into());
            }
            check_source(&frame.source, frame.width, frame.height)?;
            std::fs::hard_link(
                &frame.source,
                dir.join(&camera.camera_id).join(frame_file(frame.seq)),
            )
            .map_err(|e| {
                format!(
                    "为历史原图 {} 建硬链接失败（录制目录须与采集目录在同一磁盘卷）：{e}",
                    frame.source.display()
                )
            })?;
            lines.push(FrameLine {
                camera: frame.camera,
                seq: frame.seq,
                frame_counter: frame.frame_counter,
                trigger_counter: None,
                path: frame_rel_path(&camera.camera_id, frame.seq),
                width: frame.width,
                height: frame.height,
                ts: frame.ts,
            });
        }
        let summary = summarize(group_count, &saved);
        if !summary.missing_frames.is_empty() {
            return Err("历史件缺帧，不能导入为采集轮次".into());
        }
        let mut journal = String::new();
        for line in &lines {
            journal.push_str(&serde_json::to_string(line).map_err(|e| e.to_string())?);
            journal.push('\n');
        }
        crate::fsio::write_atomic(&dir.join(FRAMES_FILE), &journal)?;
        let mut round = CaptureRoundV2 {
            format: FORMAT,
            round_id,
            recipe_id,
            cameras,
            state: CaptureState::Complete,
            plc_shot_count: None,
            plc_plan_version: None,
            teaching_round_id: None,
            device_triggers: None,
            group_count: 0,
            camera_frames: Vec::new(),
            incomplete_groups: Vec::new(),
            missing_frames: Vec::new(),
            loss_alarms: Vec::new(),
            alarms: Vec::new(),
            created_at,
            ended_at: Some(created_at),
            error: None,
            failure: None,
            simulated: false,
            origin: Some(origin),
        };
        apply(&mut round, summary);
        validate_record(&round.round_id, &round, Some(&lines))?;
        self.save_burst(&round)?;
        Ok(round)
    }
}
