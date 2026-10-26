use std::path::{Component, Path};

use crate::burst_judge::FrameState;
use crate::frame::FrameImage;
use crate::store::{FrameRecord, PartShot, ShotRawFile};

use super::RawFrame;

fn relative_path<'a>(cycle_id: &str, raw: &'a ShotRawFile) -> Result<&'a Path, String> {
    if !(1..=3).contains(&raw.view) {
        return Err("历史原图路径与 cycleId 不一致，拒绝读取".into());
    }
    cycle_relative(cycle_id, &raw.file)
}

/// 录制根内的相对路径，且所在目录是本件的录制目录（`*_cycle_{cycleId}`）。
fn cycle_relative<'a>(cycle_id: &str, file: &'a str) -> Result<&'a Path, String> {
    let relative = Path::new(file);
    if cycle_id.is_empty() || cycle_id.len() > 128
        || !cycle_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || relative.is_absolute() || relative.components().any(|part| !matches!(part, Component::Normal(_)))
        || file.contains(':') || file.contains('\\')
        || !relative.parent().and_then(Path::file_name).and_then(|name| name.to_str())
            .and_then(|name| name.split_once("_cycle_")).is_some_and(|(_, id)| id == cycle_id) {
        return Err("历史原图路径与 cycleId 不一致，拒绝读取".into());
    }
    Ok(relative)
}

/// 读录制目录里的 part.json，要求它属于本件（cycleId 相同）。
fn cycle_metadata(root: &Path, relative: &Path, cycle_id: &str) -> Result<serde_json::Value, String> {
    let metadata_path = root.join(relative).parent().ok_or("原图目录无效")?.join("part.json");
    match std::fs::symlink_metadata(&metadata_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err("录制元数据缺失，不能核验历史原图完整身份".into()),
        Err(error) => return Err(format!("录制元数据无法读取：{error}")),
        Ok(_) => crate::image_file::checked_source_path(&metadata_path)?,
    }
    let metadata: serde_json::Value = serde_json::from_slice(&std::fs::read(&metadata_path)
        .map_err(|error| format!("录制元数据读取失败：{error}"))?)
        .map_err(|error| format!("录制元数据损坏：{error}"))?;
    if metadata["cycleId"].as_str() != Some(cycle_id) { return Err("录制元数据的 cycleId 与历史原图不一致".into()); }
    Ok(metadata)
}

/// 段配方一帧的录制原图，按 (计划相机下标, 帧组序号) 读（决策记录 D-SB-29，录制文件名 `{相机}_s{seq:06}.pgm`）：
/// 路径在本件录制目录内、文件名由相机与 seq 推出；part.json 里这一帧的 cycleId、相机下标与编号、seq、帧号
/// （历史记有触发计数时还有触发计数）都与历史记录一致且已落盘，解码尺寸等于录制尺寸。缺帧、未录制或任一项对不上都报错，
/// 不拿别的帧顶替。
pub(super) fn load_frame_verified(root: &Path, cycle_id: &str, camera_id: &str, frame: &FrameRecord) -> Result<FrameImage, String> {
    if frame.status == FrameState::Missing { return Err(format!("相机 {camera_id} 第 {} 帧组缺帧，没有原图", frame.seq)); }
    let file = frame.raw_file.as_deref().ok_or_else(|| format!("相机 {camera_id} 第 {} 帧组的原图未录制或录制队满未保存", frame.seq))?;
    crate::image_file::checked_source_path(root)?;
    let root = root.canonicalize().map_err(|_| "原图目录不存在或已清理".to_string())?;
    let relative = cycle_relative(cycle_id, file)?;
    let name = format!("{camera_id}_s{:06}.pgm", frame.seq);
    if relative.file_name().and_then(|n| n.to_str()) != Some(name.as_str()) {
        return Err(format!("历史原图 {file} 与相机 {camera_id} 第 {} 帧组的身份不一致，拒绝读取", frame.seq));
    }
    let metadata = cycle_metadata(&root, relative, cycle_id)?;
    let frames = metadata["frames"].as_array().ok_or("录制元数据缺少原图引用")?;
    let mut matching = frames.iter().filter(|entry| entry["file"].as_str() == Some(name.as_str()));
    let entry = matching.next().ok_or("录制元数据缺少对应原图")?;
    if matching.next().is_some() || entry["cycleId"].as_str() != Some(cycle_id) || entry["available"].as_bool() != Some(true) {
        return Err("录制元数据的原图身份重复、不一致或未落盘".into());
    }
    let frame_counter = frame.frame_counter.ok_or("历史帧记录缺少帧号，不能核验原图身份")?;
    if entry["camera"].as_u64() != Some(frame.camera as u64) || entry["cameraId"].as_str() != Some(camera_id)
        || entry["seq"].as_u64() != Some(frame.seq) || entry["frameCounter"].as_u64() != Some(frame_counter)
        || frame.trigger_counter.is_some_and(|trigger| entry["triggerCounter"].as_u64() != Some(trigger)) {
        return Err("录制原图的相机、帧组序号或帧号与历史记录不一致".into());
    }
    let dimension = |field: &str| entry[field].as_u64().and_then(|value| u32::try_from(value).ok()).filter(|value| *value > 0);
    let size = [dimension("width").ok_or("录制原图宽度无效")?, dimension("height").ok_or("录制原图高度无效")?];
    let bytes = original_bytes(&root, relative)?;
    checked_dimensions(&bytes, Some(size), file)?;
    let image = image::load_from_memory(&bytes).map_err(|error| format!("原图 {file} 解码失败：{error}"))?.into_luma8();
    let (width, height) = image.dimensions();
    Ok(FrameImage::new(width, height, image.into_raw()))
}

fn original_bytes(root: &Path, relative: &Path) -> Result<Vec<u8>, String> {
    let source = root.join(relative);
    crate::image_file::checked_source_path(&source)?;
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
    let metadata = cycle_metadata(root, relative, cycle_id)?;
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
    crate::image_file::checked_source_path(root)?;
    let root = root.canonicalize().map_err(|_| "原图目录不存在或已清理".to_string())?;
    let bytes = original_bytes(&root, relative_path(cycle_id, raw)?)?;
    let expected = expected_dimensions(&root, cycle_id, shot, raw)?;
    checked_dimensions(&bytes, expected, &raw.file)?;
    let image = image::load_from_memory(&bytes).map_err(|error| format!("原图 {} 解码失败：{error}", raw.file))?.into_luma8();
    let (width, height) = image.dimensions();
    Ok(FrameImage::new(width, height, image.into_raw()))
}

pub(super) fn frames(root: &Path, cycle_id: &str, shots: &[PartShot], ts: i64) -> Result<Vec<RawFrame>, String> {
    crate::image_file::checked_source_path(root)?;
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

    #[test]
    fn recorded_burst_frame_is_read_by_camera_and_seq_and_identity_mismatch_is_refused() {
        // 防的回归（D-SB-29 原图引用按 (camera, seq)、历史复测按帧组读原图）：按到达顺序或文件列表位置取图，读到别的相机或
        // 别的帧组的原图；历史帧记录与录制元数据（帧号、相机、seq）对不上、跨件 cycleId、缺帧时仍读出一张图
        use std::sync::{mpsc::channel, Arc};
        use std::time::Duration;

        use crate::frame::{CounterSource, Frame};
        use crate::judge::Verdict;
        use crate::recorder::Recorder;
        use crate::settings::RecordMode;
        use crate::store::FrameRecord;

        let root = super::super::tests::ImportTestDir::new();
        let (tx, rx) = channel();
        let recorder = Recorder::new(root.0.join("records"), Some(Arc::new(move |outcome| tx.send(outcome).unwrap())));
        let recipe = Arc::new(crate::recipe::samples_burst().build().unwrap());
        let cameras = recipe.plan_cameras();
        let cycle_id = "burst_cycle_history-frames";
        // 帧 (camera, seq) 的像素为 [10 × seq + camera, 1]，帧号 100 + 10 × camera + seq；相机 2 第 2 帧组缺帧
        let delivered = [(0, 1), (1, 1), (2, 1), (0, 2), (2, 2)];
        let frame = |camera: usize, seq: u64| Frame { cam: camera as u8, session: 7, counter: CounterSource::Synthetic,
            frame_counter: 100 + 10 * camera as u64 + seq, trigger_counter: 50 + seq, lost_packets: 0, ts: 777, manual: false,
            images: vec![Arc::new(FrameImage::new(2, 1, vec![10 * seq as u8 + camera as u8, 1]))] };
        let mut recording = recorder.begin(RecordMode::All, "SN-B7", recipe.clone(), cycle_id, Some("bundle")).unwrap();
        for &(camera, seq) in &delivered { recorder.burst_frame(&mut recording, &frame(camera, seq), camera, seq); }
        recorder.finish(recording, Verdict::NgWidth, "原始判定", 10, u64::MAX, Vec::new());
        let outcome = rx.recv_timeout(Duration::from_secs(10)).expect("真实 Recorder 必须完成收尾");
        let record = |camera: usize, seq: u64| {
            let delivered = delivered.contains(&(camera, seq));
            let raw_file = outcome.files.iter().find(|f| (f.frame.camera, f.frame.seq) == (camera, seq)).map(|f| f.file.clone());
            FrameRecord { camera, seq, status: if delivered { FrameState::Measured } else { FrameState::Missing },
                frame_counter: delivered.then(|| frame(camera, seq).frame_counter), trigger_counter: delivered.then_some(50 + seq),
                region: Some(0), metrics: None, raw_file }
        };
        let records = recorder.root();
        for &(camera, seq) in &delivered {
            let loaded = load_frame_verified(records, cycle_id, &cameras[camera], &record(camera, seq)).unwrap();
            assert_eq!(loaded.pixels, [10 * seq as u8 + camera as u8, 1], "相机 {camera} 第 {seq} 帧组");
        }
        type Edit = fn(&mut FrameRecord);
        let refused: [(&str, Edit); 4] = [
            ("帧号与录制不符", |f| f.frame_counter = Some(999)),
            ("触发计数与录制不符", |f| f.trigger_counter = Some(999)),
            ("记录指向同目录里另一台相机的原图", |f| f.raw_file = f.raw_file.as_ref().map(|p| p.replace("cam1_s000002", "cam3_s000002"))),
            ("缺帧", |f| f.status = FrameState::Missing),
        ];
        for (case, edit) in refused {
            let mut changed = record(0, 2);
            edit(&mut changed);
            assert!(load_frame_verified(records, cycle_id, &cameras[0], &changed).is_err(), "{case}");
        }
        assert!(load_frame_verified(records, cycle_id, &cameras[1], &record(1, 2)).is_err(), "缺帧没有原图");
        assert!(load_frame_verified(records, "history-frames", &cameras[0], &record(0, 1)).is_err(), "cycleId 不同");
        assert!(load_frame_verified(records, cycle_id, &cameras[1], &record(0, 1)).is_err(), "相机编号与下标不符");
    }

}
