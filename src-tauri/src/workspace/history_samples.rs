use std::path::Path;

use crate::frame::FrameImage;
use crate::offline::sample_set::{SampleRef, SampleSet};
use crate::recipe::Recipe;
use crate::store::{PartDetail, RecordSource};


#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::{Judgement, Verdict};
    use crate::offline::sample_set::fixture::Fixture;
    use crate::store::{PartSummary, PlcDelivery, RecordingEvidence, RecordingState};

    fn history(set: &SampleSet, recipe: &Recipe) -> PartDetail {
        PartDetail {
            summary: PartSummary {
                id: 42,
                ts: 1,
                sn: "DBG-42".into(),
                recipe_id: Some(recipe.id.clone()),
                recipe_version: Some(recipe.version),
                recipe_revision: Some(recipe.revision_id.clone()),
                trigger_mode: Some("fly".into()),
                verdict: Verdict::ErrInspect,
                plc_code: 90,
                fault_code: 0,
                reason: "测试历史".into(),
                drain_ms: None,
                frames_expected: 4,
                frames_received: 4,
                retest_of: None,
                cycle_id: Some("offline-candidate-cycle".into()),
                bundle_id: None,
                delivery: PlcDelivery::default(),
                source: RecordSource::OfflineCandidate,
                sample: set.reference("p02"),
                flags: None,
                burst: None,
                mode: Default::default(),
            },
            judgement: Judgement::error(0, "测试历史"),
            frames: Vec::new(),
            triggers: 4,
            software_version: "test".into(),
            points: None,
            retests: Vec::new(),
            shots: Vec::new(),
            recording: RecordingEvidence {
                state: RecordingState::Off,
                ..Default::default()
            },
            burst: None,
        }
    }

    #[test]
    fn burst_candidate_history_reads_camera_seq_from_that_cameras_own_shot_sequence() {
        // 防的回归（D-SB-28 样本集 v1 的段配方解释）：按清单全局顺序或拍照点编号取图，读到别的相机的帧；某台相机帧数不够时
        // 拿别的帧顶替；历史计划相机与配方快照不一致时仍关联原图
        let source = Fixture::new(&[1]);
        // 三台相机轮流拍：S001 cam1、S002 cam2、S003 cam3、S004 cam1、S005 cam2、S006 cam3；像素 = 拍照点序号 × 10 + 件序号
        let mut manifest = source.manifest();
        manifest["cameras"] = serde_json::json!((1..=3).map(|c| serde_json::json!({"id": format!("cam{c}"), "views": [1]})).collect::<Vec<_>>());
        manifest["shots"] = serde_json::json!((1..=6).map(|k| serde_json::json!({"shotId": format!("S{k:03}"), "cameraId": format!("cam{}", (k - 1) % 3 + 1), "views": [1]})).collect::<Vec<_>>());
        source.write_manifest(&manifest);
        for part in 1..=2u8 {
            let dir = source.0.join("parts").join(format!("p{part:02}"));
            for entry in std::fs::read_dir(&dir).unwrap() { std::fs::remove_file(entry.unwrap().path()).unwrap(); }
            for k in 1..=6u8 {
                image::save_buffer(dir.join(format!("S{k:03}_cam{}_v1.png", (k - 1) % 3 + 1)), &[k * 10 + part; 4], 2, 2, image::ColorType::L8).unwrap();
            }
        }
        let root = Fixture::new(&[1]);
        let managed = crate::managed_samples::import(&root.0, &source.0).unwrap();
        let set = SampleSet::load(&managed).unwrap();
        let mut recipe = crate::recipe::samples_burst().build().unwrap();
        recipe.revision_id = format!("candidate-{}-r17", recipe.id);
        let mut detail = history(&set, &recipe);
        detail.burst = Some(crate::store::PartDetailBurst { segments: recipe.path_segments.clone(), cameras: recipe.plan_cameras(),
            expected_total: recipe.expected_total(), tolerance_total: recipe.tolerance_total(), device_triggers: None, camera_frames: vec![2; 3],
            judgement: crate::burst_judge::BurstJudgement::error(0, "测试历史"), frames: Vec::new() });
        let images = BurstSamples::open(&root.0, &detail, &recipe).unwrap();
        for (camera, seq, shot) in [(0, 1, 1u8), (1, 2, 5), (2, 1, 3), (2, 2, 6)] {
            assert_eq!(images.load(camera, seq).unwrap().pixels, [shot * 10 + 2; 4], "相机 {camera} 第 {seq} 帧组应为 S{shot:03}");
        }
        assert!(images.load(0, 3).is_err(), "相机只有 2 帧时第 3 帧组不能拿别的帧顶替");
        detail.burst.as_mut().unwrap().cameras.swap(0, 1);
        assert!(BurstSamples::open(&root.0, &detail, &recipe).is_err(), "历史计划相机与配方快照不一致");
    }

}

/// 候选离线调试记录引用的受管样本件：记录来源、配方快照身份、样本件 ID / SN / 标签都要对上。
fn managed_part(root: &Path, detail: &PartDetail, recipe: &Recipe) -> Result<(SampleSet, SampleRef), String> {
    if detail.summary.source != RecordSource::OfflineCandidate {
        return Err("只有候选离线调试记录可关联离线样本原图".into());
    }
    if detail.summary.recipe_id.as_deref() != Some(recipe.id.as_str())
        || detail.summary.recipe_revision.as_deref() != Some(recipe.revision_id.as_str()) {
        return Err("历史配方快照与记录身份不一致，不能关联样本原图".into());
    }
    let reference = detail.summary.sample.as_ref().ok_or("候选调试历史没有样本件引用，不能确定离线样本原图")?;
    let set = crate::managed_samples::load_by_id(root, &reference.set_id)?;
    if set.reference(&reference.part_id).as_ref() != Some(reference) {
        return Err("受管样本件的 ID、SN 或标签与历史引用不一致，拒绝关联原图".into());
    }
    Ok((set, reference.clone()))
}

/// 段配方的候选离线调试记录（决策记录 D-SB-28）：样本集格式 v1 不变，每台计划相机在清单里按顺序的拍照点就是它的帧序列，
/// 第 seq 帧组取这台相机的第 seq 个拍照点（单视角，图 1）。帧数不够时报错，不拿别的相机或别的帧顶替。
pub(super) struct BurstSamples {
    set: SampleSet,
    part_id: String,
    /// 按计划相机顺序：(相机编号, 这台相机按清单顺序的拍照点编号)
    frames: Vec<(String, Vec<String>)>,
}

impl BurstSamples {
    pub(super) fn open(root: &Path, detail: &PartDetail, recipe: &Recipe) -> Result<Self, String> {
        let burst = detail.burst.as_ref().ok_or("这条候选调试记录不是段配方记录")?;
        if !recipe.is_burst() || burst.cameras != recipe.plan_cameras() {
            return Err("历史记录的计划相机与冻结配方快照不一致，不能确定样本原图".into());
        }
        let (set, reference) = managed_part(root, detail, recipe)?;
        let frames = burst.cameras.iter().map(|camera| {
            (camera.clone(), set.manifest().shots.iter().filter(|shot| &shot.camera_id == camera).map(|shot| shot.shot_id.clone()).collect())
        }).collect();
        Ok(Self { set, part_id: reference.part_id, frames })
    }

    /// 第 camera 台计划相机第 seq 帧组（从 1 起）的样本原图。
    pub(super) fn load(&self, camera: usize, seq: u64) -> Result<FrameImage, String> {
        let (camera_id, shots) = self.frames.get(camera).ok_or("相机下标超出本件计划相机")?;
        let shot = usize::try_from(seq).ok().and_then(|seq| seq.checked_sub(1)).and_then(|index| shots.get(index))
            .ok_or_else(|| format!("样本集里相机 {camera_id} 只有 {} 帧，没有第 {seq} 帧组", shots.len()))?;
        self.set.load_view(&self.part_id, shot, camera_id, 1)
    }
}
