use std::path::{Component, Path};

use crate::frame::FrameImage;
use crate::store::{PartShot, ShotRawFile};

use super::RawFrame;

fn relative_path<'a>(cycle_id: &str, raw: &'a ShotRawFile) -> Result<&'a Path, String> {
    let relative = Path::new(&raw.file);
    if cycle_id.is_empty() || cycle_id.len() > 128
        || !cycle_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || !(1..=3).contains(&raw.view)
        || relative.is_absolute() || relative.components().any(|part| !matches!(part, Component::Normal(_)))
        || raw.file.contains(':') || raw.file.contains('\\')
        || !relative.parent().and_then(Path::file_name).and_then(|name| name.to_str())
            .and_then(|name| name.split_once("_cycle_")).is_some_and(|(_, id)| id == cycle_id) {
        return Err("历史原图路径与 cycleId 不一致，拒绝读取".into());
    }
    Ok(relative)
}

fn verified_bytes(root: &Path, relative: &Path, raw: &ShotRawFile) -> Result<Vec<u8>, String> {
    let path = root.join(relative).canonicalize().map_err(|_| "原图未保留或已按保留策略清理".to_string())?;
    if !path.starts_with(root) || !path.is_file() { return Err("原图路径越过录制目录或不是文件".into()); }
    let hash = raw.hash.as_deref().ok_or("历史原图没有落盘校验值")?;
    let bytes = std::fs::read(path).map_err(|error| format!("原图读取失败：{error}"))?;
    if hash != format!("fnv1a64:{}", crate::release::fnv_hex(&bytes)) { return Err("原图校验值不一致，文件已改变".into()); }
    Ok(bytes)
}

pub(super) fn load_verified(root: &Path, cycle_id: &str, raw: &ShotRawFile) -> Result<FrameImage, String> {
    let root = root.canonicalize().map_err(|_| "原图目录不存在或已清理".to_string())?;
    let bytes = verified_bytes(&root, relative_path(cycle_id, raw)?, raw)?;
    let image = image::load_from_memory(&bytes).map_err(|error| format!("原图 {} 解码失败：{error}", raw.file))?.into_luma8();
    let (width, height) = image.dimensions();
    Ok(FrameImage::new(width, height, image.into_raw()))
}

pub(super) fn frames(root: &Path, cycle_id: &str, shots: &[PartShot], ts: i64) -> Result<Vec<RawFrame>, String> {
    let root = root.canonicalize().map_err(|_| "原图目录不存在或已清理".to_string())?;
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for shot in shots {
        for raw in &shot.raw_files {
            if !(1..=3).contains(&raw.view) || !seen.insert((shot.k, raw.view)) {
                return Err("历史原图的拍照点或视角重复，不能确定对应图像".into());
            }
            let checked = verified_bytes(&root, relative_path(cycle_id, raw)?, raw).map(|_| ());
            result.push(RawFrame { k: shot.k, view: raw.view, camera: shot.camera.clone(), file: raw.file.clone(), ts,
                available: checked.is_ok(), error: checked.err(), cam: None, frame_counter: shot.frame_counter,
                trigger_counter: shot.trigger_counter });
        }
    }
    result.sort_by_key(|frame| (frame.k, frame.view));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycle::FrameStatus;

    #[test]
    fn original_files_require_exact_cycle_and_hash_and_keep_all_views() {
        let root = super::super::tests::ImportTestDir::new();
        let directory = root.0.join("20261010/part_cycle_first");
        std::fs::create_dir_all(&directory).unwrap();
        let mut shots = Vec::new();
        for k in 0..4 {
            let mut raw_files = Vec::new();
            for view in 1..=3 {
                let file = format!("20261010/part_cycle_first/k{k}_v{view}.pgm");
                let bytes = format!("raw-{k}-{view}").into_bytes();
                std::fs::write(root.0.join(&file), &bytes).unwrap();
                raw_files.push(ShotRawFile { view, file, hash: Some(format!("fnv1a64:{}", crate::release::fnv_hex(&bytes))) });
            }
            shots.push(PartShot { k, shot_id: format!("P{k}"), camera: "cam1".into(), view: [1,2,3,1][k],
                session: Some(7), ordinal: Some(k as u64 + 1), frame_counter: Some(k as u64 + 1),
                trigger_counter: Some(k as u64 + 1), status: FrameStatus::Done, error: None, score: Some(1.0), ms: Some(2), raw_files });
        }
        let files = frames(&root.0, "first", &shots, 1).unwrap();
        assert_eq!(files.len(), 12);
        assert!(files.iter().all(|frame| frame.available));
        assert!(load_verified(&root.0, "first", &shots[0].raw_files[0]).unwrap_err().contains("解码"));
        assert!(frames(&root.0, "same-sn-second", &shots, 1).unwrap_err().contains("cycleId"));
        std::fs::write(root.0.join(&shots[1].raw_files[1].file), b"changed").unwrap();
        let changed = frames(&root.0, "first", &shots, 1).unwrap();
        assert!(!changed[4].available);
        assert!(changed[4].error.as_ref().unwrap().contains("校验"));
        std::fs::remove_file(root.0.join(&shots[2].raw_files[2].file)).unwrap();
        assert!(!frames(&root.0, "first", &shots, 1).unwrap()[8].available);
        shots[0].raw_files[0].file = "../part_cycle_first/external.pgm".into();
        assert!(frames(&root.0, "first", &shots, 1).is_err());
    }

    #[test]
    fn recorder_outcome_round_trips_all_views_with_verified_pixels_and_cycle_identity() {
        use std::sync::{mpsc::channel, Arc};
        use std::time::Duration;

        use crate::frame::{CounterSource, Frame};
        use crate::judge::Verdict;
        use crate::recorder::{Recorder, RecordingState};
        use crate::settings::RecordMode;

        let root = super::super::tests::ImportTestDir::new();
        let (tx, rx) = channel();
        let recorder = Recorder::new(root.0.join("records"), Some(Arc::new(move |outcome| tx.send(outcome).unwrap())));
        let mut doc = crate::recipe::samples().remove(1);
        doc.shots.truncate(1);
        doc.shots[0].view = 2;
        let recipe = Arc::new(doc.build().unwrap());
        let cycle_id = "recorder_cycle_history-roundtrip";
        let frame = Frame { cam: 0, session: 7, counter: CounterSource::Synthetic, frame_counter: 13,
            trigger_counter: 11, lost_packets: 0, ts: 777, manual: false,
            images: (0..3).map(|view| Arc::new(FrameImage::new(2, 1, vec![17 + view, 71 + view]))).collect() };
        let mut recording = recorder.begin(RecordMode::All, 42, recipe.clone(), cycle_id, Some("original-bundle")).unwrap();
        recorder.frame(&mut recording, &frame, &recipe.shots[0].camera, 0, 9);
        recorder.finish(recording, Verdict::NgWidth, "保留原始判定", 10, u64::MAX, Vec::new());
        let outcome = rx.recv_timeout(Duration::from_secs(10)).expect("真实 Recorder 必须完成收尾");
        assert!(outcome.available, "{:?}", outcome.errors);
        assert_eq!(outcome.state, RecordingState::Complete);
        assert_eq!(outcome.files.len(), 3);
        assert!(outcome.directory.as_ref().unwrap().file_name().unwrap().to_string_lossy()
            .ends_with(&format!("_NG_WIDTH_cycle_{cycle_id}")));
        let shots = vec![PartShot { k: 0, shot_id: recipe.shots[0].id.clone(), camera: recipe.shots[0].camera.clone(),
            view: 2, session: Some(frame.session), ordinal: Some(9), frame_counter: Some(frame.frame_counter),
            trigger_counter: Some(frame.trigger_counter), status: FrameStatus::Done, error: None, score: Some(1.0), ms: Some(2),
            raw_files: outcome.files.iter().map(|raw| ShotRawFile { view: raw.view, file: raw.file.clone(), hash: Some(raw.hash.clone()) }).collect() }];
        let files = frames(recorder.root(), cycle_id, &shots, frame.ts).unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|file| file.available));
        for raw in &shots[0].raw_files {
            let loaded = load_verified(recorder.root(), cycle_id, raw).unwrap();
            assert_eq!((loaded.width, loaded.height), (2, 1));
            assert_eq!(loaded.pixels, frame.images[raw.view as usize - 1].pixels);
            assert!(load_verified(recorder.root(), "same-sn-different-cycle", raw).unwrap_err().contains("cycleId"));
            assert!(load_verified(recorder.root(), "history-roundtrip", raw).unwrap_err().contains("cycleId"));
        }
        let selected = &shots[0].raw_files[1];
        let mut no_hash = selected.clone();
        no_hash.hash = None;
        assert!(load_verified(recorder.root(), cycle_id, &no_hash).unwrap_err().contains("校验值"));
        std::fs::write(recorder.root().join(&selected.file), [b"P5\n2 1\n255\n".as_slice(), &[91, 92]].concat()).unwrap();
        assert!(load_verified(recorder.root(), cycle_id, selected).unwrap_err().contains("校验值不一致"));
        let changed = frames(recorder.root(), cycle_id, &shots, frame.ts).unwrap();
        assert!(!changed[1].available);
        assert!(changed[0].available && changed[2].available);
    }
}
