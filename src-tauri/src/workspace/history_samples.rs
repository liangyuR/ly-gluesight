use std::collections::HashSet;
use std::path::Path;

use crate::frame::FrameImage;
use crate::offline::sample_set::{SampleRef, SampleSet};
use crate::recipe::Recipe;
use crate::store::{PartDetail, PartShot, RecordSource};

use super::RawFrame;

pub(super) struct HistorySamples {
    set: SampleSet,
    reference: SampleRef,
    sample_shots: Vec<String>,
    shots: Vec<PartShot>,
    ts: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycle::FrameStatus;
    use crate::judge::{self, PointState, Verdict};
    use crate::offline::sample_set::fixture::Fixture;
    use crate::store::{PartSummary, PlcDelivery, RecordingEvidence, RecordingState};

    fn history(set: &SampleSet, view: u8) -> (Recipe, PartDetail) {
        let mut doc = crate::recipe::samples().remove(1);
        for shot in &mut doc.shots { shot.view = view; }
        let mut recipe = doc.build().unwrap();
        recipe.revision_id = format!("candidate-{}-r17", recipe.id);
        let table = vec![PointState::Pending; recipe.point_count()];
        let detail = PartDetail {
            summary: PartSummary { id: 42, ts: 1, sn: "DBG-42".into(), recipe_id: Some(recipe.id.clone()),
                recipe_version: Some(recipe.version), recipe_revision: Some(recipe.revision_id.clone()), trigger_mode: Some("fly".into()),
                verdict: Verdict::ErrInspect, plc_code: 90, fault_code: 0, reason: "测试历史".into(), drain_ms: None,
                frames_expected: 4, frames_received: 4, retest_of: None, cycle_id: Some("offline-candidate-cycle".into()),
                bundle_id: None, delivery: PlcDelivery::default(), source: RecordSource::OfflineCandidate,
                sample: set.reference("p02"), flags: None, burst: None },
            judgement: judge::judge(&recipe, &table), frames: Vec::new(), triggers: 4, software_version: "test".into(),
            points: None, retests: Vec::new(), shots: recipe.shots.iter().enumerate().map(|(k, shot)| PartShot {
                k, shot_id: shot.id.clone(), camera: shot.camera.clone(), view: shot.view, session: None, ordinal: None,
                frame_counter: None, trigger_counter: None, status: FrameStatus::Done, error: None, score: None, ms: None, raw_files: Vec::new(),
            }).collect(),
            recording: RecordingEvidence { state: RecordingState::Off, ..Default::default() },
            burst: None,
        };
        (recipe, detail)
    }

    #[test]
    fn candidate_history_with_recording_off_reads_its_managed_part_and_selected_view() {
        let source = Fixture::new(&[1, 2]);
        let root = Fixture::new(&[1]);
        let managed = crate::managed_samples::import(&root.0, &source.0).unwrap();
        let set = SampleSet::load(&managed).unwrap();
        let (recipe, detail) = history(&set, 2);
        drop(source);
        let images = HistorySamples::open(&root.0, &detail, &recipe).unwrap();
        assert_eq!(images.load(2, 2).unwrap().pixels, [Fixture::pixel(2, 3, 2); 4]);
        assert_eq!(images.frames().len(), 8);
        assert!(images.frames().iter().all(|frame| frame.available));
    }

    #[test]
    fn missing_sample_view_never_substitutes_another_view_or_part() {
        let source = Fixture::new(&[1, 2]);
        let root = Fixture::new(&[1]);
        let managed = crate::managed_samples::import(&root.0, &source.0).unwrap();
        let (recipe, detail) = history(&SampleSet::load(&managed).unwrap(), 2);
        std::fs::remove_file(managed.join("parts/p02/S003_cam1_v1.png")).unwrap();
        let images = HistorySamples::open(&root.0, &detail, &recipe).unwrap();
        assert_eq!(images.load(2, 2).unwrap().pixels, [Fixture::pixel(2, 3, 2); 4]);
        assert!(!images.frames().iter().find(|frame| frame.k == 2 && frame.view == 1).unwrap().available);
        std::fs::remove_file(managed.join("parts/p02/S003_cam1_v2.png")).unwrap();
        let images = HistorySamples::open(&root.0, &detail, &recipe).unwrap();
        assert!(images.load(2, 2).is_err());
        assert!(!images.frames().iter().find(|frame| frame.k == 2 && frame.view == 2).unwrap().available);
    }

    #[test]
    fn ambiguous_sample_or_mismatched_history_identity_refuses_originals() {
        let source = Fixture::new(&[1, 2]);
        let root = Fixture::new(&[1]);
        let managed = crate::managed_samples::import(&root.0, &source.0).unwrap();
        let (recipe, original) = history(&SampleSet::load(&managed).unwrap(), 2);
        let variants: [fn(&mut PartDetail); 10] = [
            |d| d.summary.sample = None,
            |d| d.summary.sample.as_mut().unwrap().set_id = "missing-set".into(),
            |d| d.summary.sample.as_mut().unwrap().part_id = "missing-part".into(),
            |d| d.summary.sample.as_mut().unwrap().sn = "other-part".into(),
            |d| d.summary.sample.as_mut().unwrap().label = crate::offline::sample_set::SampleLabel::Ok,
            |d| d.shots[0].shot_id = "other-shot".into(),
            |d| d.shots[0].camera = "cam2".into(),
            |d| d.shots[0].view = 1,
            |d| d.shots[0].k = 1,
            |d| d.summary.recipe_revision = Some("candidate-other-r17".into()),
        ];
        for edit in variants {
            let mut detail = original.clone();
            edit(&mut detail);
            assert!(HistorySamples::open(&root.0, &detail, &recipe).is_err());
        }
        let duplicate = root.0.join("samples/duplicate");
        std::fs::create_dir_all(duplicate.join("parts")).unwrap();
        std::fs::copy(managed.join("sample-set.json"), duplicate.join("sample-set.json")).unwrap();
        assert!(HistorySamples::open(&root.0, &original, &recipe).is_err());
    }
}

impl HistorySamples {
    pub(super) fn open(root: &Path, detail: &PartDetail, recipe: &Recipe) -> Result<Self, String> {
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
        let sample_shots = set.manifest().recipe_shots(recipe)?.iter().map(|shot| shot.shot_id.clone()).collect();
        let mut seen = HashSet::new();
        for shot in &detail.shots {
            let original = recipe.shots.get(shot.k).ok_or("历史拍照点不在冻结配方快照中")?;
            if !seen.insert(shot.k) || shot.shot_id != original.id || shot.camera != original.camera || shot.view != original.view {
                return Err("历史拍照点、相机或选定视角与冻结配方快照不一致".into());
            }
        }
        if seen.len() != recipe.shot_count() { return Err("历史拍照点记录不完整，不能确定整圈样本原图".into()); }
        Ok(Self { set, reference: reference.clone(), sample_shots, shots: detail.shots.clone(), ts: detail.summary.ts })
    }

    pub(super) fn load(&self, k: usize, view: u8) -> Result<FrameImage, String> {
        let shot = self.shots.iter().find(|shot| shot.k == k).ok_or("历史拍照点不存在")?;
        self.set.load_view(&self.reference.part_id, &self.sample_shots[k], &shot.camera, view)
    }

    pub(super) fn frames(&self) -> Vec<RawFrame> {
        let mut frames = Vec::new();
        for shot in &self.shots {
            let sample_shot = &self.sample_shots[shot.k];
            let views = &self.set.manifest().shots.iter().find(|sample| sample.shot_id == *sample_shot).unwrap().views;
            for &view in views {
                let checked = self.load(shot.k, view);
                let file = self.set.image_path(&self.reference.part_id, sample_shot, &shot.camera, view)
                    .map(|path| path.display().to_string()).unwrap_or_else(|_| format!("{}/{sample_shot}_{}_v{view}", self.reference.part_id, shot.camera));
                let available = checked.is_ok();
                frames.push(RawFrame { error: checked.err(), k: shot.k, view, camera: shot.camera.clone(), file, ts: self.ts,
                    available, cam: None, frame_counter: shot.frame_counter, trigger_counter: shot.trigger_counter });
            }
        }
        frames.sort_by_key(|frame| (frame.k, frame.view));
        frames
    }
}
