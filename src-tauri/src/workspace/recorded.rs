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

fn original_bytes(root: &Path, relative: &Path) -> Result<Vec<u8>, String> {
    let source = root.join(relative);
    crate::replay::checked_source_path(&source)?;
    let path = source.canonicalize().map_err(|_| "原图未保留或已按保留策略清理".to_string())?;
    if !path.starts_with(root) || !path.is_file() { return Err("原图路径越过录制目录或不是文件".into()); }
    let bytes = std::fs::read(path).map_err(|error| format!("原图读取失败：{error}"))?;
    Ok(bytes)
}

fn expected_dimensions(root: &Path, cycle_id: &str, shot: &PartShot, raw: &ShotRawFile) -> Result<Option<[u32; 2]>, String> {
    let stored = match (raw.width, raw.height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => Some([width, height]),
        (None, None) => None,
        _ => return Err("历史原图的录制尺寸无效".into()),
    };
    let relative = relative_path(cycle_id, raw)?;
    let metadata_path = root.join(relative).parent().ok_or("原图目录无效")?.join("part.json");
    match std::fs::symlink_metadata(&metadata_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err("录制元数据缺失，不能核验历史原图完整身份".into()),
        Err(error) => return Err(format!("录制元数据无法读取：{error}")),
        Ok(_) => crate::replay::checked_source_path(&metadata_path)?,
    }
    let metadata: serde_json::Value = serde_json::from_slice(&std::fs::read(&metadata_path)
        .map_err(|error| format!("录制元数据读取失败：{error}"))?)
        .map_err(|error| format!("录制元数据损坏：{error}"))?;
    if metadata["cycleId"].as_str() != Some(cycle_id) { return Err("录制元数据的 cycleId 与历史原图不一致".into()); }
    let file = relative.file_name().and_then(|name| name.to_str()).ok_or("原图文件名无效")?;
    let frames = metadata["frames"].as_array().ok_or("录制元数据缺少原图引用")?;
    let mut matching = frames.iter().filter(|frame| frame["file"].as_str() == Some(file) && frame["view"].as_u64() == Some(u64::from(raw.view)));
    let frame = matching.next().ok_or("录制元数据缺少对应原图")?;
    if matching.next().is_some() || frame["cycleId"].as_str() != Some(cycle_id) || frame["available"].as_bool() != Some(true) {
        return Err("录制元数据的原图身份重复、不一致或未落盘".into());
    }
    if shot.shot_id.is_empty() || shot.camera.is_empty() || !(1..=3).contains(&shot.view)
        || frame["k"].as_u64() != Some(shot.k as u64)
        || frame["shotId"].as_str() != Some(shot.shot_id.as_str())
        || frame["camera"].as_str() != Some(shot.camera.as_str())
        || frame["selectedView"].as_u64() != Some(u64::from(shot.view)) {
        return Err("录制原图的拍照点、相机或选定视角与历史身份不一致".into());
    }
    for (field, expected) in [("session", shot.session), ("ordinal", shot.ordinal),
        ("frameCounter", shot.frame_counter), ("triggerCounter", shot.trigger_counter)] {
        let expected = expected.ok_or_else(|| format!("历史原图缺少 {field} 身份，不能核验"))?;
        if frame[field].as_u64() != Some(expected) {
            return Err(format!("录制原图的 {field} 与历史身份不一致"));
        }
    }
    let dimension = |field: &str| frame[field].as_u64().and_then(|value| u32::try_from(value).ok()).filter(|value| *value > 0);
    let size = [dimension("width").ok_or("录制原图宽度无效")?, dimension("height").ok_or("录制原图高度无效")?];
    if stored.is_some_and(|expected| expected != size) { return Err("录制元数据尺寸与历史原图引用不一致".into()); }
    Ok(Some(size))
}

fn checked_dimensions(bytes: &[u8], expected: Option<[u32; 2]>, file: &str) -> Result<(), String> {
    if let Some(expected) = expected {
        let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()
            .map_err(|error| format!("原图 {file} 解码失败：{error}"))?;
        let (width, height) = reader.into_dimensions().map_err(|error| format!("原图 {file} 解码失败：{error}"))?;
        if [width, height] != expected { return Err(format!("原图 {file} 的尺寸与录制尺寸不符")); }
    }
    Ok(())
}

pub(super) fn load_verified(root: &Path, cycle_id: &str, shot: &PartShot, raw: &ShotRawFile) -> Result<FrameImage, String> {
    crate::replay::checked_source_path(root)?;
    let root = root.canonicalize().map_err(|_| "原图目录不存在或已清理".to_string())?;
    let bytes = original_bytes(&root, relative_path(cycle_id, raw)?)?;
    let expected = expected_dimensions(&root, cycle_id, shot, raw)?;
    checked_dimensions(&bytes, expected, &raw.file)?;
    let image = image::load_from_memory(&bytes).map_err(|error| format!("原图 {} 解码失败：{error}", raw.file))?.into_luma8();
    let (width, height) = image.dimensions();
    Ok(FrameImage::new(width, height, image.into_raw()))
}

pub(super) fn frames(root: &Path, cycle_id: &str, shots: &[PartShot], ts: i64) -> Result<Vec<RawFrame>, String> {
    crate::replay::checked_source_path(root)?;
    let root = root.canonicalize().map_err(|_| "原图目录不存在或已清理".to_string())?;
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for shot in shots {
        for raw in &shot.raw_files {
            if !(1..=3).contains(&raw.view) || !seen.insert((shot.k, raw.view)) {
                return Err("历史原图的拍照点或视角重复，不能确定对应图像".into());
            }
            let checked = original_bytes(&root, relative_path(cycle_id, raw)?).and_then(|bytes| {
                checked_dimensions(&bytes, expected_dimensions(&root, cycle_id, shot, raw)?, &raw.file)
            });
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

    fn write_identity_metadata(root: &Path, cycle_id: &str, shots: &[PartShot]) -> std::path::PathBuf {
        let directory = root.join(&shots[0].raw_files[0].file).parent().unwrap().to_path_buf();
        let frames = shots.iter().flat_map(|shot| shot.raw_files.iter().map(move |raw| serde_json::json!({
            "cycleId": cycle_id, "file": Path::new(&raw.file).file_name().unwrap().to_str().unwrap(),
            "view": raw.view, "available": true, "k": shot.k, "shotId": shot.shot_id,
            "camera": shot.camera, "selectedView": shot.view, "session": shot.session,
            "ordinal": shot.ordinal, "frameCounter": shot.frame_counter, "triggerCounter": shot.trigger_counter,
            "width": raw.width.unwrap_or(2), "height": raw.height.unwrap_or(1)
        }))).collect::<Vec<_>>();
        let path = directory.join("part.json");
        std::fs::write(&path, serde_json::to_vec(&serde_json::json!({"cycleId": cycle_id, "frames": frames})).unwrap()).unwrap();
        path
    }

    #[cfg(windows)]
    #[test]
    fn internal_cycle_junction_and_linked_root_are_rejected() {
        let fixture = super::super::tests::ImportTestDir::new();
        let root = fixture.0.join("records");
        let target = root.join("20261010").join("part_cycle_second");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("k0_v1.pgm"), b"P5\n1 1\n255\n\x80").unwrap();
        let link = root.join("20261010").join("part_cycle_first");
        let linked_root = fixture.0.join("linked-records");
        for (alias, actual) in [(&link, &target), (&linked_root, &root)] {
            let status = std::process::Command::new("cmd").args(["/C", "mklink", "/J"])
                .arg(alias).arg(actual).output().unwrap();
            assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
        }
        let raw = ShotRawFile { view: 1, file: "20261010/part_cycle_first/k0_v1.pgm".into(), width: None, height: None };
        let shot = PartShot { k: 0, shot_id: "P0".into(), camera: "cam1".into(), view: 1,
            session: None, ordinal: None, frame_counter: None, trigger_counter: None,
            status: FrameStatus::Done, error: None, score: None, ms: None, raw_files: vec![raw.clone()] };
        assert!(load_verified(&root, "first", &shot, &raw).unwrap_err().contains("reparse"));
        let listed = frames(&root, "first", std::slice::from_ref(&shot), 1).unwrap();
        assert!(!listed[0].available && listed[0].error.as_ref().unwrap().contains("reparse"));
        let direct = ShotRawFile { view: 1, file: "20261010/part_cycle_second/k0_v1.pgm".into(), width: None, height: None };
        assert!(load_verified(&linked_root, "second", &shot, &direct).unwrap_err().contains("reparse"));
        assert!(load_verified(&root, "second", &shot, &direct).unwrap_err().contains("元数据缺失"));
        std::fs::remove_dir(&linked_root).unwrap();
        std::fs::remove_dir(&link).unwrap();
    }

    #[test]
    fn original_files_require_exact_cycle_and_keep_all_views() {
        let root = super::super::tests::ImportTestDir::new();
        let directory = root.0.join("20261010/part_cycle_first");
        std::fs::create_dir_all(&directory).unwrap();
        let mut shots = Vec::new();
        for k in 0..4 {
            let mut raw_files = Vec::new();
            for view in 1..=3 {
                let file = format!("20261010/part_cycle_first/k{k}_v{view}.pgm");
                let bytes = [b"P5\n2 1\n255\n".as_slice(), &[k as u8, view]].concat();
                std::fs::write(root.0.join(&file), &bytes).unwrap();
                raw_files.push(ShotRawFile { view, file, width: None, height: None });
            }
            shots.push(PartShot { k, shot_id: format!("P{k}"), camera: "cam1".into(), view: [1,2,3,1][k],
                session: Some(7), ordinal: Some(k as u64 + 1), frame_counter: Some(k as u64 + 1),
                trigger_counter: Some(k as u64 + 1), status: FrameStatus::Done, error: None, score: Some(1.0), ms: Some(2), raw_files });
        }
        write_identity_metadata(&root.0, "first", &shots);
        let files = frames(&root.0, "first", &shots, 1).unwrap();
        assert_eq!(files.len(), 12);
        assert!(files.iter().all(|frame| frame.available));
        assert_eq!(load_verified(&root.0, "first", &shots[0], &shots[0].raw_files[0]).unwrap().pixels, [0, 1]);
        assert!(frames(&root.0, "same-sn-second", &shots, 1).unwrap_err().contains("cycleId"));
        std::fs::write(root.0.join(&shots[1].raw_files[1].file), b"P5\n2 1\n255\n\x07\x08").unwrap();
        let changed = frames(&root.0, "first", &shots, 1).unwrap();
        assert!(changed[4].available);
        assert_eq!(load_verified(&root.0, "first", &shots[1], &shots[1].raw_files[1]).unwrap().pixels, [7, 8]);
        std::fs::remove_file(root.0.join(&shots[2].raw_files[2].file)).unwrap();
        assert!(!frames(&root.0, "first", &shots, 1).unwrap()[8].available);
        shots[0].raw_files[0].file = "../part_cycle_first/external.pgm".into();
        assert!(frames(&root.0, "first", &shots, 1).is_err());
    }

    #[test]
    fn same_cycle_same_view_same_size_other_shot_reference_is_rejected() {
        let root = super::super::tests::ImportTestDir::new();
        let directory = root.0.join("20261010/part_cycle_shared");
        std::fs::create_dir_all(&directory).unwrap();
        let shots = (0..2).map(|k| {
            let file = format!("20261010/part_cycle_shared/k{k}_v1.pgm");
            std::fs::write(root.0.join(&file), [b"P5\n2 1\n255\n".as_slice(), &[k as u8, 99]].concat()).unwrap();
            PartShot { k, shot_id: format!("P{k}"), camera: "cam1".into(), view: 1,
                session: Some(7), ordinal: Some(k as u64 + 1), frame_counter: Some(k as u64 + 13),
                trigger_counter: Some(k as u64 + 11), status: FrameStatus::Done, error: None,
                score: Some(1.0), ms: Some(2), raw_files: vec![ShotRawFile { view: 1, file, width: Some(2), height: Some(1) }] }
        }).collect::<Vec<_>>();
        write_identity_metadata(&root.0, "shared", &shots);
        assert_eq!(load_verified(&root.0, "shared", &shots[0], &shots[0].raw_files[0]).unwrap().pixels, [0, 99]);
        assert_eq!(load_verified(&root.0, "shared", &shots[1], &shots[1].raw_files[0]).unwrap().pixels, [1, 99]);
        let mut redirected = shots[0].clone();
        redirected.raw_files[0] = shots[1].raw_files[0].clone();
        assert!(load_verified(&root.0, "shared", &redirected, &redirected.raw_files[0]).unwrap_err().contains("历史身份不一致"));
        let listed = frames(&root.0, "shared", &[redirected, shots[1].clone()], 1).unwrap();
        assert!(!listed[0].available && listed[0].error.as_ref().unwrap().contains("历史身份不一致"));
        assert!(listed[1].available);
    }

    #[test]
    fn recorded_metadata_requires_every_shot_identity_field_and_survives_valid_restore() {
        let root = super::super::tests::ImportTestDir::new();
        let file = "20261010/part_cycle_identity/k0_v1.pgm";
        std::fs::create_dir_all(root.0.join(file).parent().unwrap()).unwrap();
        std::fs::write(root.0.join(file), b"P5\n2 1\n255\n\x11\x22").unwrap();
        let shot = PartShot { k: 0, shot_id: "P0".into(), camera: "cam1".into(), view: 2,
            session: Some(7), ordinal: Some(9), frame_counter: Some(13), trigger_counter: Some(11),
            status: FrameStatus::Done, error: None, score: Some(1.0), ms: Some(2),
            raw_files: vec![ShotRawFile { view: 1, file: file.into(), width: Some(2), height: Some(1) }] };
        let path = write_identity_metadata(&root.0, "identity", std::slice::from_ref(&shot));
        let original: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let raw = &shot.raw_files[0];
        let variants: [(&str, serde_json::Value); 8] = [("k", 1.into()), ("shotId", "other-pose".into()), ("camera", "cam2".into()),
            ("selectedView", 1.into()), ("session", 8.into()), ("ordinal", 10.into()),
            ("frameCounter", 14.into()), ("triggerCounter", 12.into())];
        for (field, value) in variants {
            for missing in [false, true] {
                let mut changed = original.clone();
                if missing { changed["frames"][0].as_object_mut().unwrap().remove(field); }
                else { changed["frames"][0][field] = value.clone(); }
                std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
                assert!(load_verified(&root.0, "identity", &shot, raw).is_err(), "{field} missing={missing}");
                let listed = frames(&root.0, "identity", std::slice::from_ref(&shot), 1).unwrap();
                assert!(!listed[0].available && listed[0].error.is_some(), "{field} missing={missing}");
            }
        }
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        assert_eq!(load_verified(&root.0, "identity", &shot, raw).unwrap().pixels, [17, 34]);
        for field in ["session", "ordinal", "frameCounter", "triggerCounter"] {
            let mut value = serde_json::to_value(&shot).unwrap();
            value[field] = serde_json::Value::Null;
            let missing: PartShot = serde_json::from_value(value).unwrap();
            assert!(load_verified(&root.0, "identity", &missing, raw).unwrap_err().contains("缺少"), "{field}");
        }
        std::fs::remove_file(path).unwrap();
        assert!(load_verified(&root.0, "identity", &shot, raw).unwrap_err().contains("元数据缺失"));
        assert!(!frames(&root.0, "identity", std::slice::from_ref(&shot), 1).unwrap()[0].available);
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
            raw_files: outcome.files.iter().map(|raw| ShotRawFile { view: raw.view, file: raw.file.clone(), width: Some(raw.width), height: Some(raw.height) }).collect() }];
        let files = frames(recorder.root(), cycle_id, &shots, frame.ts).unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|file| file.available));
        for raw in &shots[0].raw_files {
            let loaded = load_verified(recorder.root(), cycle_id, &shots[0], raw).unwrap();
            assert_eq!((loaded.width, loaded.height), (2, 1));
            assert_eq!(loaded.pixels, frame.images[raw.view as usize - 1].pixels);
            assert!(load_verified(recorder.root(), "same-sn-different-cycle", &shots[0], raw).unwrap_err().contains("cycleId"));
            assert!(load_verified(recorder.root(), "history-roundtrip", &shots[0], raw).unwrap_err().contains("cycleId"));
        }
        let selected = &shots[0].raw_files[1];
        std::fs::write(recorder.root().join(&selected.file), [b"P5\n2 1\n255\n".as_slice(), &[91, 92]].concat()).unwrap();
        assert_eq!(load_verified(recorder.root(), cycle_id, &shots[0], selected).unwrap().pixels, [91, 92]);
        let changed = frames(recorder.root(), cycle_id, &shots, frame.ts).unwrap();
        assert!(changed[1].available);
        assert!(changed[0].available && changed[2].available);
        let metadata_path = outcome.directory.as_ref().unwrap().join("part.json");
        let original_metadata = std::fs::read(&metadata_path).unwrap();
        let mut legacy = selected.clone();
        legacy.width = None;
        legacy.height = None;
        assert_eq!(load_verified(recorder.root(), cycle_id, &shots[0], &legacy).unwrap().pixels, [91, 92]);
        std::fs::write(recorder.root().join(&selected.file), b"P5\n3 1\n255\n\x01\x02\x03").unwrap();
        for reference in [selected, &legacy] {
            assert!(load_verified(recorder.root(), cycle_id, &shots[0], reference).unwrap_err().contains("尺寸"));
        }
        let changed = frames(recorder.root(), cycle_id, &shots, frame.ts).unwrap();
        assert!(!changed[1].available && changed[1].error.as_ref().unwrap().contains("尺寸"));
        assert!(changed[0].available && changed[2].available);
        std::fs::write(recorder.root().join(&selected.file), b"P5\n2 1\n255\n\x5b\x5c").unwrap();
        let original: serde_json::Value = serde_json::from_slice(&original_metadata).unwrap();
        for invalid in ["cycle", "dimensions", "missing-dimensions", "duplicate", "malformed"] {
            let mut metadata = original.clone();
            match invalid {
                "cycle" => metadata["cycleId"] = "another-cycle".into(),
                "dimensions" => metadata["frames"][1]["width"] = 3.into(),
                "missing-dimensions" => { metadata["frames"][1].as_object_mut().unwrap().remove("width"); },
                "duplicate" => {
                    let duplicate = metadata["frames"][1].clone();
                    metadata["frames"].as_array_mut().unwrap().push(duplicate);
                },
                _ => {},
            }
            let bytes = if invalid == "malformed" { b"broken".to_vec() } else { serde_json::to_vec(&metadata).unwrap() };
            std::fs::write(&metadata_path, bytes).unwrap();
            assert!(load_verified(recorder.root(), cycle_id, &shots[0], &legacy).is_err(), "{invalid}");
        }
        std::fs::write(&metadata_path, &original_metadata).unwrap();
        std::fs::remove_file(&metadata_path).unwrap();
        std::fs::write(recorder.root().join(&selected.file), b"P5\n3 1\n255\n\x01\x02\x03").unwrap();
        for raw in [selected, &legacy] {
            assert!(load_verified(recorder.root(), cycle_id, &shots[0], raw).unwrap_err().contains("元数据缺失"));
        }
        assert!(frames(recorder.root(), cycle_id, &shots, frame.ts).unwrap().iter().all(|raw| !raw.available));
    }
}
