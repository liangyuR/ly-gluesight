use std::path::Path;

use crate::frame::FrameImage;
use crate::offline::sample_set::{SampleRef, SampleSet};
use crate::recipe::Recipe;
use crate::store::{PartDetail, RecordSource};

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
