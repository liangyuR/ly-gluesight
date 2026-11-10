use super::*;
use std::collections::{BTreeSet, HashMap};

fn json(path: &Path) -> Result<Value, String> {
    crate::image_file::checked_source_path(path)?;
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("读取资源引用 {} 失败：{e}", path.display()))
}

fn strings(value: &Value, key: &str, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(fields) => {
            if let Some(Value::String(id)) = fields.get(key) {
                out.insert(id.clone());
            }
            for child in fields.values() {
                strings(child, key, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                strings(child, key, out);
            }
        }
        _ => {}
    }
}

#[derive(Default)]
struct Candidate {
    images: BTreeSet<String>,
    rounds: BTreeSet<String>,
    navigation: BTreeSet<String>,
}

pub(super) fn select(
    root: &Path,
    entries: Vec<(String, PathBuf)>,
    history: bool,
    notes: &mut Vec<String>,
) -> Result<Vec<(String, PathBuf)>, String> {
    let mut candidates = HashMap::<String, Candidate>::new();
    let mut rounds = BTreeSet::new();
    let mut archives = BTreeSet::new();
    let mut bundles = BTreeSet::new();
    let mut supports = BTreeSet::new();
    for (name, path) in &entries {
        let parts: Vec<_> = name.split('/').collect();
        if parts.len() == 3 && parts[1] == "recipes" && name.ends_with(".json") {
            let doc = json(path)?;
            if let (Some(id), Some(version)) = (doc["id"].as_str(), doc["version"].as_u64()) {
                archives.insert(format!(
                    "project/workspaces/{id}/published/{id}-v{version}.json"
                ));
            }
        }
        if parts.len() != 4 || parts[1] != "workspaces" || parts[3] != "workspace.json" {
            continue;
        }
        let value = json(path)?;
        let mut candidate = Candidate::default();
        strings(&value, "roundId", &mut candidate.rounds);
        strings(&value["cameraTeaching"], "round", &mut candidate.rounds);
        if let Some(id) = value["captureId"].as_str() {
            candidate.rounds.insert(id.into());
        }
        if let Some(reviews) = value["captureReview"]["rounds"].as_object() {
            candidate.rounds.extend(reviews.keys().cloned());
        }
        for teaching in value["cameraTeaching"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(
                value["passAlignment"]["source"]["teaching"]
                    .as_array()
                    .into_iter()
                    .flatten(),
            )
        {
            if let Some(id) = teaching["frame"]["id"].as_str() {
                candidate.images.insert(format!("{id}.pgm"));
            }
        }
        if let Some(id) = value["navigationId"].as_str() {
            let map = format!("maps/{id}.json");
            if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("导航资源编号无效".into());
            }
            let nav = json(
                &root
                    .join("workspaces")
                    .join(parts[2])
                    .join("navigation")
                    .join(&map),
            )?;
            let image = nav["image"]["id"].as_str().ok_or("导航图缺少图像编号")?;
            if image.len() != 64 || !image.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("导航图像编号无效".into());
            }
            candidate.navigation.insert(map);
            candidate.navigation.insert(format!("images/{image}.png"));
        }
        if let Some(bundle) = value["pending"]["bundleId"].as_str() {
            bundles.insert(format!("{}/{bundle}", parts[2]));
            if let Some(id) = value["pending"]["navigation"]["bundleId"].as_str() {
                supports.insert(format!("{}/{id}", parts[2]));
            }
        }
        rounds.extend(candidate.rounds.iter().cloned());
        candidates.insert(parts[2].into(), candidate);
    }
    for (name, path) in &entries {
        if archives.contains(name) {
            let archive = json(path)?;
            let parts: Vec<_> = name.split('/').collect();
            if let Some(id) = archive["bundleId"].as_str() {
                bundles.insert(format!("{}/{id}", parts[2]));
            }
            if let Some(id) = archive["navigation"]["bundleId"].as_str() {
                supports.insert(format!("{}/{id}", parts[2]));
            }
        }
    }
    let present: BTreeSet<_> = entries.iter().map(|(name, _)| name.as_str()).collect();
    for bundle in &bundles {
        if !present.contains(format!("project/vision/releases/{bundle}/manifest.json").as_str()) {
            return Err(format!(
                "当前或待生效配方引用的发布包 {bundle} 缺失，不能生成完整项目"
            ));
        }
    }
    for (id, candidate) in &candidates {
        for resource in &candidate.navigation {
            if !present.contains(format!("project/workspaces/{id}/navigation/{resource}").as_str())
            {
                return Err(format!(
                    "候选 {id} 引用的导航资源 {resource} 缺失，不能生成完整项目"
                ));
            }
        }
        for image in &candidate.images {
            if !present.contains(format!("project/workspaces/{id}/images/{image}").as_str()) {
                return Err(format!(
                    "候选 {id} 引用的示教图 {image} 缺失，不能生成完整项目"
                ));
            }
        }
        for round in &candidate.rounds {
            if !present.contains(format!("project/recipe-capture/{round}/round.json").as_str()) {
                return Err(format!(
                    "候选 {id} 引用的示教轮次 {round} 缺失，不能生成完整项目"
                ));
            }
        }
    }
    let preserve: BTreeSet<_> = archives
        .iter()
        .filter(|name| !present.contains(name.as_str()))
        .filter_map(|name| name.split('/').nth(2).map(str::to_owned))
        .collect();
    if !preserve.is_empty() {
        notes.push("部分生产配方缺少当前发布快照，保守保留其发布与导航资源".into());
    }
    let before = entries.len();
    let out: Vec<_> = entries
        .into_iter()
        .filter(|(name, _)| {
            let parts: Vec<_> = name.split('/').collect();
            match parts.as_slice() {
                ["project", "vision", "releases", recipe, bundle, ..] => {
                    history
                        || preserve.contains(*recipe)
                        || bundles.contains(&format!("{recipe}/{bundle}"))
                }
                ["project", "recipe-support", recipe, bundle, ..] => {
                    history
                        || preserve.contains(*recipe)
                        || supports.contains(&format!("{recipe}/{bundle}"))
                }
                ["project", "recipe-capture", round, ..] => {
                    candidates.is_empty() || rounds.contains(*round)
                }
                ["project", "workspaces", id, "published", ..] => {
                    history || preserve.contains(*id) || archives.contains(name)
                }
                ["project", "workspaces", id, "images", image] => candidates
                    .get(*id)
                    .is_none_or(|c| c.images.contains(*image)),
                ["project", "workspaces", id, "calibration", round, ..] => candidates
                    .get(*id)
                    .is_none_or(|c| c.rounds.contains(*round)),
                ["project", "workspaces", id, "navigation", kind, file] => candidates
                    .get(*id)
                    .is_none_or(|c| c.navigation.contains(&format!("{kind}/{file}"))),
                _ => true,
            }
        })
        .collect();
    if before != out.len() {
        notes.push(format!(
            "按当前配方引用裁剪了 {} 个旧版本或未引用资源文件",
            before - out.len()
        ));
    }
    Ok(out)
}

fn recording(root: &Path, cycle: &str) -> Result<PathBuf, String> {
    use rusqlite::OptionalExtension;
    let data = root
        .parent()
        .and_then(Path::parent)
        .ok_or("项目目录缺少资料根")?;
    let db = super::super::records::database(data);
    crate::image_file::checked_source_path(&db)?;
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
    let directory: Option<Option<String>> = conn
        .query_row(
            "SELECT recording_directory FROM parts WHERE cycle_id=?1",
            [cycle],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    directory
        .flatten()
        .map(PathBuf::from)
        .ok_or_else(|| format!("示教件 {cycle} 的来源原图不在本机，不能生成完整项目"))
}

pub(super) fn materialize(
    root: &Path,
    entries: &mut Vec<(String, PathBuf)>,
    scratch: &Path,
) -> Result<(), String> {
    let sources: Vec<_> = entries
        .iter()
        .filter(|(name, _)| {
            name.starts_with("project/recipe-capture/") && name.ends_with("/round.json")
        })
        .cloned()
        .collect();
    for (name, path) in sources {
        let mut round = json(&path)?;
        let Some(format) = round["format"].as_u64() else {
            continue;
        };
        if ![2, 3, 4].contains(&format) {
            return Err("不能导出未知格式的示教轮次".into());
        }
        let frame_name = name.trim_end_matches("round.json").to_owned() + "frames.jsonl";
        let frames = path.with_file_name("frames.jsonl");
        crate::image_file::checked_source_path(&frames)?;
        let text = std::fs::read_to_string(frames).map_err(|e| format!("读取示教索引失败：{e}"))?;
        let mut lines = Vec::new();
        let mut additions = Vec::new();
        let mut recording_dir = None;
        let mut changed = format == 4;
        for line in text.lines().filter(|s| !s.trim().is_empty()) {
            let mut frame: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
            let camera = frame["camera"].as_u64().ok_or("示教帧缺少相机索引")? as usize;
            let id = round["cameras"]
                .get(camera)
                .and_then(|c| c["cameraId"].as_str())
                .ok_or("示教帧相机不存在")?;
            if id.is_empty()
                || id.contains(['/', '\\', ':'])
                || id.chars().any(char::is_control)
                || id == "."
                || id == ".."
            {
                return Err("示教相机编号无效".into());
            }
            let seq = frame["seq"].as_u64().ok_or("示教帧序号无效")?;
            let relative = format!("{id}/{seq:06}.pgm");
            let local = path.parent().unwrap().join(&relative);
            if format == 4 || !local.is_file() {
                if recording_dir.is_none() {
                    let cycle = round["origin"]["cycleId"]
                        .as_str()
                        .ok_or("示教轮次缺少来源，无法恢复原图")?;
                    recording_dir = Some(recording(root, cycle)?);
                }
                let source = recording_dir
                    .as_ref()
                    .unwrap()
                    .join(format!("{id}_s{seq:06}.pgm"));
                // 空闲压缩过的原图在盘上是同名的 .gsr，写包或复制时还原
                let source = crate::image_file::stored_path(&source)
                    .ok_or_else(|| format!("示教原图 {} 缺失", source.display()))?;
                crate::image_file::checked_source_path(&source)?;
                additions.push((
                    name.trim_end_matches("round.json").to_owned() + &relative,
                    source,
                ));
                changed = true;
            }
            frame["path"] = Value::String(relative);
            lines.push(serde_json::to_string(&frame).map_err(|e| e.to_string())?);
        }
        if !changed {
            continue;
        }
        round["format"] = Value::from(3);
        let dir = scratch.join("teaching").join(
            path.parent()
                .and_then(Path::file_name)
                .ok_or("示教目录无效")?,
        );
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let round_file = dir.join("round.json");
        let frames_file = dir.join("frames.jsonl");
        std::fs::write(
            &round_file,
            serde_json::to_vec(&round).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(&frames_file, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
        entries.retain(|(entry, _)| {
            entry != &name
                && entry != &frame_name
                && !additions.iter().any(|(added, _)| added == entry)
        });
        entries.push((name, round_file));
        entries.push((frame_name, frames_file));
        entries.extend(additions);
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(())
}
