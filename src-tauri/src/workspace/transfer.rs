//! 段配方候选的迁移包：把候选（配方文档、走廊示教帧、多轮标定特征、采集复核）连同它引用的示教采集轮次原图
//! 打成一个 ZIP，在另一台机器上作为新候选导入。
//!
//! - 包内 `manifest.json` 记格式版本、配方身份、导出时间、来源软件版本、包含的轮次与每个文件的字节数和 SHA-256；
//! - 导入先完整解到暂存目录：只接受认识的格式版本、清单内且路径在白名单结构里的文件，逐个核对字节数与校验和；
//! - 导入的候选从草稿开始（不带待生效版本、验证结论和本机历史记录样本），仍需在本机验证与发布；
//!   示教帧与标定轮次记下的相机配置照常与本机设备核对，不一致时现有检查让示教、标定失效；
//! - 本机已有同名采集轮次时，导入的轮次改用新编号，候选里对它的引用一并改名。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};

use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::*;

pub const TRANSFER_FORMAT: u32 = 1;
const MANIFEST: &str = "manifest.json";
const MANIFEST_MAX_BYTES: u64 = 64 << 20;
const CANDIDATE_FILE: &str = "workspace/workspace.json";
const RECIPE_FILE: &str = "recipe.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferKind {
    /// 完整候选：候选目录与引用的采集轮次
    Candidate,
    /// 只有已发布版本、没有候选：只含配方文档
    PublishedDoc,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferManifest {
    pub format: u32,
    pub kind: TransferKind,
    pub recipe_id: String,
    pub name: String,
    pub product_code: u16,
    pub version: u32,
    pub exported_at: i64,
    pub app_version: String,
    pub rounds: Vec<String>,
    pub notes: Vec<String>,
    pub files: Vec<TransferFile>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferExported {
    pub path: String,
    pub bytes: u64,
    pub kind: TransferKind,
    pub rounds: usize,
    pub files: usize,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferPreview {
    pub path: String,
    pub kind: TransferKind,
    pub recipe_id: String,
    pub name: String,
    pub product_code: u16,
    pub version: u32,
    pub exported_at: i64,
    pub app_version: String,
    pub rounds: usize,
    pub files: usize,
    pub bytes: u64,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferImported {
    pub id: String,
    pub kind: TransferKind,
    pub rounds: usize,
    pub renamed_rounds: usize,
    pub notes: Vec<String>,
}

enum Content {
    Disk(PathBuf),
    Bytes(Vec<u8>),
}

struct Entry {
    path: String,
    content: Content,
}

/// 待写进迁移包的内容（不含 manifest 的文件清单，写包时算出）。
struct Package {
    manifest: TransferManifest,
    entries: Vec<Entry>,
}

fn valid_round_id(id: &str) -> bool {
    id.starts_with("capture-")
        && id.len() <= 64
        && id[8..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        && id.len() > 8
}

fn valid_name(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 96
        && part != "."
        && part != ".."
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn frame_file(name: &str) -> bool {
    name.len() == 10 && name.ends_with(".pgm") && name[..6].bytes().all(|b| b.is_ascii_digit())
}

/// 包内路径只能是以下结构（分隔符 `/`，每段只含字母、数字、`-`、`_`、`.`）：
/// `recipe.json`、`workspace/workspace.json`、`workspace/images/<图>.pgm`、`workspace/calibration/<轮次>/<文件>`、
/// `rounds/<轮次>/round.json`、`rounds/<轮次>/frames.jsonl`、`rounds/<轮次>/<相机>/<seq:06>.pgm`。
fn allowed_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    if path.len() > 240 || !parts.iter().all(|p| valid_name(p)) {
        return false;
    }
    match parts.as_slice() {
        [file] => *file == RECIPE_FILE,
        ["workspace", "workspace.json"] => true,
        ["workspace", "images", image] => image.ends_with(".pgm"),
        ["workspace", "calibration", round, _] => valid_round_id(round),
        ["rounds", round, "round.json" | "frames.jsonl"] => valid_round_id(round),
        ["rounds", round, camera, frame] => {
            valid_round_id(round) && crate::recipe::valid_camera_id(camera) && frame_file(frame)
        }
        _ => false,
    }
}

/// 候选引用的采集轮次：走廊示教帧、多轮标定、验证样本库、采集复核与模板。
fn referenced_rounds(w: &Workspace) -> BTreeSet<String> {
    let mut rounds = BTreeSet::new();
    rounds.extend(w.camera_teaching.iter().map(|t| t.round.clone()));
    rounds.extend(w.calibration.rounds.iter().map(|r| r.round_id.clone()));
    rounds.extend(w.sample_bank.iter().filter_map(|s| s.capture_id.clone()));
    rounds.extend(w.capture_review.rounds.keys().cloned());
    rounds.extend(w.capture_review.template.iter().map(|t| t.round_id.clone()));
    rounds.extend(w.capture_id.clone());
    rounds.retain(|id| valid_round_id(id));
    rounds
}

fn plain_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn plain_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

fn sorted_entries(dir: &Path) -> Result<Vec<std::fs::DirEntry>, String> {
    let mut entries = std::fs::read_dir(dir)
        .map_err(|e| format!("读取 {} 失败：{e}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|e| e.file_name());
    Ok(entries)
}

/// 段配方候选的迁移内容。`dir` 为候选目录，`capture_root` 为 recipe-capture；`check_round` 核对一轮可以导出（完整的段配方轮次）。
fn collect_candidate(
    w: &Workspace,
    dir: &Path,
    capture_root: &Path,
    check_round: impl Fn(&str) -> Result<(), String>,
) -> Result<Package, String> {
    if !w.doc.is_burst() {
        return Err("旧拍照点配方已停用，不能导出".into());
    }
    let mut notes = Vec::new();
    let mut entries = vec![Entry {
        path: CANDIDATE_FILE.into(),
        content: Content::Bytes(serde_json::to_vec_pretty(w).map_err(|e| e.to_string())?),
    }];
    for teaching in &w.camera_teaching {
        let name = format!("{}.pgm", teaching.frame.id);
        let path = dir.join("images").join(&name);
        if valid_name(&name) && plain_file(&path) {
            entries.push(Entry {
                path: format!("workspace/images/{name}"),
                content: Content::Disk(path),
            });
        } else {
            notes.push(format!(
                "相机 {} 的示教帧原图缺失，导入后需重新示教",
                teaching.camera
            ));
        }
    }
    let mut rounds = Vec::new();
    for round in referenced_rounds(w) {
        let round_dir = capture_root.join(&round);
        if let Err(e) = check_round(&round).and_then(|_| {
            if plain_dir(&round_dir) {
                Ok(())
            } else {
                Err("轮次目录不存在".into())
            }
        }) {
            notes.push(format!("采集轮次 {round} 未导出：{e}"));
            continue;
        }
        for file in ["round.json", "frames.jsonl"] {
            entries.push(Entry {
                path: format!("rounds/{round}/{file}"),
                content: Content::Disk(round_dir.join(file)),
            });
        }
        for camera in sorted_entries(&round_dir)? {
            let camera_name = camera.file_name().to_string_lossy().into_owned();
            if !camera.file_type().is_ok_and(|t| t.is_dir())
                || !crate::recipe::valid_camera_id(&camera_name)
            {
                continue;
            }
            for frame in sorted_entries(&camera.path())? {
                let frame_name = frame.file_name().to_string_lossy().into_owned();
                if frame.file_type().is_ok_and(|t| t.is_file()) && frame_file(&frame_name) {
                    entries.push(Entry {
                        path: format!("rounds/{round}/{camera_name}/{frame_name}"),
                        content: Content::Disk(frame.path()),
                    });
                }
            }
        }
        let features = dir.join("calibration").join(&round);
        if plain_dir(&features) {
            for file in sorted_entries(&features)? {
                let name = file.file_name().to_string_lossy().into_owned();
                if file.file_type().is_ok_and(|t| t.is_file()) && valid_name(&name) {
                    entries.push(Entry {
                        path: format!("workspace/calibration/{round}/{name}"),
                        content: Content::Disk(file.path()),
                    });
                }
            }
        }
        rounds.push(round);
    }
    Ok(Package {
        manifest: manifest_head(TransferKind::Candidate, &w.doc, rounds, notes),
        entries,
    })
}

fn collect_published(doc: &RecipeDoc) -> Result<Package, String> {
    if !doc.is_burst() {
        return Err("旧拍照点配方已停用，不能导出".into());
    }
    let notes =
        vec!["本机只有已发布版本、没有候选：只导出配方文档，不含示教帧、多轮标定与采集原图".into()];
    Ok(Package {
        manifest: manifest_head(TransferKind::PublishedDoc, doc, Vec::new(), notes),
        entries: vec![Entry {
            path: RECIPE_FILE.into(),
            content: Content::Bytes(serde_json::to_vec_pretty(doc).map_err(|e| e.to_string())?),
        }],
    })
}

fn manifest_head(
    kind: TransferKind,
    doc: &RecipeDoc,
    rounds: Vec<String>,
    notes: Vec<String>,
) -> TransferManifest {
    TransferManifest {
        format: TRANSFER_FORMAT,
        kind,
        recipe_id: doc.id.clone(),
        name: doc.name.clone(),
        product_code: doc.product_code,
        version: doc.version,
        exported_at: ly_plc::now_ms(),
        app_version: String::new(),
        rounds,
        notes,
        files: Vec::new(),
    }
}

fn copy_hashed(mut from: impl Read, mut to: impl Write) -> std::io::Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = from.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        to.write_all(&buffer[..n])?;
        total += n as u64;
    }
    Ok((total, format!("{:x}", hasher.finalize())))
}

fn write_package<W: Write + Seek>(
    out: W,
    package: Package,
) -> Result<(W, TransferManifest), String> {
    let Package {
        mut manifest,
        entries,
    } = package;
    let mut zip = ZipWriter::new(out);
    for entry in entries {
        if !allowed_path(&entry.path) {
            return Err(format!("迁移包路径无效：{}", entry.path));
        }
        let (length, reader): (u64, Box<dyn Read>) = match entry.content {
            Content::Disk(path) => {
                let file =
                    File::open(&path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
                (
                    file.metadata().map_err(|e| e.to_string())?.len(),
                    Box::new(BufReader::new(file)),
                )
            }
            Content::Bytes(bytes) => (bytes.len() as u64, Box::new(std::io::Cursor::new(bytes))),
        };
        let level = if entry.path.ends_with(".pgm") { 1 } else { 6 };
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(level))
            .large_file(length >= u32::MAX as u64);
        zip.start_file(entry.path.as_str(), options)
            .map_err(|e| e.to_string())?;
        let (bytes, sha256) =
            copy_hashed(reader, &mut zip).map_err(|e| format!("写入 {} 失败：{e}", entry.path))?;
        manifest.files.push(TransferFile {
            path: entry.path,
            bytes,
            sha256,
        });
    }
    zip.start_file(
        MANIFEST,
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
    )
    .map_err(|e| e.to_string())?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let out = zip.finish().map_err(|e| e.to_string())?;
    Ok((out, manifest))
}

fn read_manifest<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<TransferManifest, String> {
    let mut entry = zip
        .by_name(MANIFEST)
        .map_err(|_| "不是配方迁移包：缺少 manifest.json".to_string())?;
    if entry.size() > MANIFEST_MAX_BYTES {
        return Err("迁移包清单过大".into());
    }
    let mut text = String::new();
    entry
        .by_ref()
        .take(MANIFEST_MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("迁移包清单无法读取：{e}"))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("迁移包清单格式无效：{e}"))?;
    let format = value.get("format").and_then(Value::as_u64);
    if format != Some(TRANSFER_FORMAT as u64) {
        return Err(format!(
            "迁移包格式 {} 不受本机软件支持（支持 {TRANSFER_FORMAT}）",
            format.map_or("未知".into(), |f| f.to_string())
        ));
    }
    let manifest: TransferManifest =
        serde_json::from_value(value).map_err(|e| format!("迁移包清单格式无效：{e}"))?;
    safe_id(&manifest.recipe_id)?;
    if manifest.rounds.iter().any(|r| !valid_round_id(r))
        || manifest.rounds.iter().collect::<BTreeSet<_>>().len() != manifest.rounds.len()
    {
        return Err("迁移包清单的采集轮次编号无效".into());
    }
    let mut seen = HashSet::new();
    for file in &manifest.files {
        if !allowed_path(&file.path) {
            return Err(format!("迁移包含不允许的路径：{}", file.path));
        }
        if !seen.insert(file.path.to_ascii_lowercase()) {
            return Err(format!("迁移包清单重复列出 {}", file.path));
        }
        if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("迁移包清单的校验和无效：{}", file.path));
        }
        if let Some(round) = file
            .path
            .strip_prefix("rounds/")
            .or_else(|| file.path.strip_prefix("workspace/calibration/"))
            .and_then(|r| r.split('/').next())
        {
            if !manifest.rounds.iter().any(|r| r == round) {
                return Err(format!("迁移包文件 {} 属于清单之外的采集轮次", file.path));
            }
        }
    }
    let required = match manifest.kind {
        TransferKind::Candidate => CANDIDATE_FILE,
        TransferKind::PublishedDoc => RECIPE_FILE,
    };
    if !manifest.files.iter().any(|f| f.path == required) {
        return Err(format!("迁移包缺少 {required}"));
    }
    if manifest.kind == TransferKind::PublishedDoc && manifest.files.len() != 1 {
        return Err("只含配方文档的迁移包不能带其他文件".into());
    }
    Ok(manifest)
}

fn open_archive(path: &Path) -> Result<ZipArchive<BufReader<File>>, String> {
    let file = File::open(path).map_err(|e| format!("打开迁移包失败：{e}"))?;
    ZipArchive::new(BufReader::new(file)).map_err(|e| format!("迁移包不是有效的 ZIP 文件：{e}"))
}

fn inspect(path: &Path) -> Result<(TransferManifest, u64), String> {
    let manifest = read_manifest(&mut open_archive(path)?)?;
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok((manifest, bytes))
}

/// 把迁移包完整解到空的暂存目录 `staging`：只写清单列出、路径合规的普通文件，逐个核对字节数与 SHA-256，
/// 清单外的条目、目录、符号链接、重复条目与缺少的文件都拒绝。
fn extract(path: &Path, staging: &Path) -> Result<TransferManifest, String> {
    let mut zip = open_archive(path)?;
    let manifest = read_manifest(&mut zip)?;
    let listed: BTreeMap<&str, &TransferFile> = manifest
        .files
        .iter()
        .map(|f| (f.path.as_str(), f))
        .collect();
    let mut seen = HashSet::new();
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|e| format!("迁移包条目无法读取：{e}"))?;
        let name = entry
            .name()
            .map_err(|e| format!("迁移包条目名称无效：{e}"))?
            .into_owned();
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(format!("迁移包含重复条目：{name}"));
        }
        if name == MANIFEST {
            continue;
        }
        let expected = listed
            .get(name.as_str())
            .filter(|_| allowed_path(&name))
            .ok_or_else(|| format!("迁移包含清单之外或不允许的条目：{name}"))?;
        if !entry.is_file() || entry.encrypted() {
            return Err(format!("迁移包条目不是普通文件：{name}"));
        }
        if entry.size() != expected.bytes {
            return Err(format!("迁移包文件 {name} 的大小与清单不符"));
        }
        let target = name
            .split('/')
            .fold(staging.to_path_buf(), |path, part| path.join(part));
        std::fs::create_dir_all(target.parent().ok_or("暂存路径无效")?)
            .map_err(|e| e.to_string())?;
        let out =
            File::create_new(&target).map_err(|e| format!("写入暂存文件 {name} 失败：{e}"))?;
        let mut out = BufWriter::new(out);
        let (bytes, sha256) = copy_hashed(entry.take(expected.bytes + 1), &mut out)
            .map_err(|e| format!("解出 {name} 失败：{e}"))?;
        out.flush().map_err(|e| e.to_string())?;
        if bytes != expected.bytes || sha256 != expected.sha256.to_ascii_lowercase() {
            return Err(format!("迁移包文件 {name} 校验和不符，文件已损坏或被改动"));
        }
    }
    if seen.len() != manifest.files.len() + 1 {
        return Err("迁移包缺少清单列出的文件".into());
    }
    Ok(manifest)
}

pub(super) struct ImportTarget {
    pub id: String,
    pub name: String,
    pub product_code: u16,
}

/// 把字符串按 [字母数字-] 切成词，整词等于旧轮次编号的换成新编号（`capture-1-1` 不会误换 `capture-1-10` 的前缀）。
fn replace_tokens(text: &str, renames: &BTreeMap<String, String>) -> String {
    let token = |c: char| c.is_ascii_alphanumeric() || c == '-';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(token) {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find(|c: char| !token(c))
            .map_or(rest.len(), |e| start + e);
        let word = &rest[start..end];
        out.push_str(renames.get(word).map_or(word, String::as_str));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn rename_values(value: &mut Value, renames: &BTreeMap<String, String>) {
    match value {
        Value::String(text) => *text = replace_tokens(text, renames),
        Value::Array(items) => items.iter_mut().for_each(|v| rename_values(v, renames)),
        Value::Object(map) => {
            for (key, mut item) in std::mem::take(map) {
                rename_values(&mut item, renames);
                map.insert(replace_tokens(&key, renames), item);
            }
        }
        _ => {}
    }
}

/// 给与本机冲突的轮次分配新编号：沿用原轮次的时间戳，后缀递增到本机与本次都没用过为止。
fn assign_round_ids(rounds: &[String], taken: impl Fn(&str) -> bool) -> BTreeMap<String, String> {
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut out = BTreeMap::new();
    for round in rounds {
        let mut id = round.clone();
        if taken(&id) || used.contains(&id) {
            let stem = round
                .rsplit_once('-')
                .map_or(round.as_str(), |(stem, _)| stem);
            let mut suffix = round
                .rsplit_once('-')
                .and_then(|(_, n)| n.parse::<u32>().ok())
                .unwrap_or(0);
            loop {
                suffix += 1;
                id = format!("{stem}-{suffix}");
                if !taken(&id) && !used.contains(&id) {
                    break;
                }
            }
        }
        used.insert(id.clone());
        out.insert(round.clone(), id);
    }
    out
}

/// 由暂存的迁移包得出要导入的新候选：换成操作员给的编号、名称与产品代码并核对与本机配方不冲突，
/// 冲突的采集轮次改编号（候选引用、暂存的 round.json 与标定特征目录一起改），轮次归属改成新编号。
/// 候选回到草稿：清掉待生效版本、验证结论、发布状态、本机历史记录样本与基准版本。返回候选、轮次改名表与提示。
fn prepare_import<'a>(
    staging: &Path,
    manifest: &TransferManifest,
    target: &ImportTarget,
    existing: impl Iterator<Item = (&'a str, u16)>,
    round_taken: impl Fn(&str) -> bool,
) -> Result<(Workspace, BTreeMap<String, String>, Vec<String>), String> {
    safe_id(&target.id)?;
    let name = target.name.trim();
    if name.is_empty() {
        return Err("请填写配方名称".into());
    }
    let mut notes = Vec::new();
    let renames = assign_round_ids(&manifest.rounds, round_taken);
    let mut w = match manifest.kind {
        TransferKind::Candidate => {
            let text =
                crate::fsio::read_text(&staging.join(CANDIDATE_FILE)).map_err(|e| e.to_string())?;
            let mut value: Value =
                serde_json::from_str(&text).map_err(|e| format!("迁移包的候选无法读取：{e}"))?;
            rename_values(
                &mut value,
                &renames
                    .iter()
                    .filter(|(a, b)| a != b)
                    .map(|(a, b)| (a.clone(), b.clone()))
                    .collect(),
            );
            serde_json::from_value::<Workspace>(value)
                .map_err(|e| format!("迁移包的候选格式与本机软件不兼容：{e}"))?
        }
        TransferKind::PublishedDoc => {
            let text =
                crate::fsio::read_text(&staging.join(RECIPE_FILE)).map_err(|e| e.to_string())?;
            let mut doc: RecipeDoc = serde_json::from_str(&text)
                .map_err(|e| format!("迁移包的配方文档格式与本机软件不兼容：{e}"))?;
            burst::prepare_new_doc(&mut doc);
            Workspace::new(doc, None)
        }
    };
    if !w.doc.is_burst() {
        return Err("旧拍照点配方已停用，不能导入".into());
    }
    w.doc.id = target.id.clone();
    w.doc.name = name.to_string();
    w.doc.product_code = target.product_code;
    w.doc.teaching_id = None;
    w.doc.plan_version = 0;
    check_new_identity(&w.doc, existing)?;
    w.doc.build()?;
    w.base_revision = None;
    w.instance_id = 0;
    w.pending = None;
    w.publish = PublishState::Draft;
    w.last_position = None;
    let before = w.samples.len();
    w.samples.retain(|s| s.history_id.is_none());
    if w.samples.len() < before {
        notes.push(format!(
            "{} 个来自来源机检测历史的验证样本未导入",
            before - w.samples.len()
        ));
    }
    w.changed();
    for (old, new) in &renames {
        let round_file = staging.join("rounds").join(old).join("round.json");
        let mut round: Value =
            serde_json::from_str(&crate::fsio::read_text(&round_file).map_err(|e| e.to_string())?)
                .map_err(|e| format!("采集轮次 {old} 的记录无法读取：{e}"))?;
        let object = round
            .as_object_mut()
            .ok_or_else(|| format!("采集轮次 {old} 的记录格式无效"))?;
        object.insert("roundId".into(), json!(new));
        object.insert("recipeId".into(), json!(target.id));
        crate::fsio::write_atomic(
            &round_file,
            &serde_json::to_string_pretty(&round).map_err(|e| e.to_string())?,
        )?;
        let features = staging.join("workspace").join("calibration");
        if old != new && plain_dir(&features.join(old)) {
            std::fs::rename(features.join(old), features.join(new)).map_err(|e| e.to_string())?;
        }
        if old != new {
            notes.push(format!("本机已有采集轮次 {old}，导入后改为 {new}"));
        }
    }
    Ok((w, renames, notes))
}

fn capture_root(app: &AppHandle) -> PathBuf {
    crate::storage::root(app).join("recipe-capture")
}

fn collect(app: &AppHandle, id: &str) -> Result<Package, String> {
    let host = app.state::<WorkspaceHost>();
    let candidate = host.items.lock().unwrap().get(id).cloned();
    let mut package = match candidate {
        Some(w) => {
            let capture = app.state::<CycleHost>().camera.recipe_capture().clone();
            collect_candidate(&w, &host.dir(id), &capture_root(app), |round| {
                if capture.get_burst(round)?.state != crate::recipe_capture::CaptureState::Complete
                {
                    return Err("采集未完整完成".into());
                }
                Ok(())
            })?
        }
        None => collect_published(
            &app.state::<CycleHost>()
                .recipes
                .doc(id)
                .ok_or("配方不存在")?,
        )?,
    };
    package.manifest.app_version = app.package_info().version.to_string();
    Ok(package)
}

#[tauri::command]
pub async fn workspace_transfer_export(
    window: tauri::WebviewWindow,
    app: AppHandle,
    id: String,
) -> Result<Option<TransferExported>, String> {
    safe_id(&id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let package = collect(&app, &id)?;
        let exports = crate::storage::root(&app).join("exports");
        std::fs::create_dir_all(&exports).map_err(|e| e.to_string())?;
        let file_name = format!(
            "配方迁移_{id}_{}.zip",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        let Some(path) = rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("导出配方迁移包")
            .set_directory(&exports)
            .set_file_name(&file_name)
            .add_filter("配方迁移包", &["zip"])
            .save_file()
        else {
            return Ok(None);
        };
        let mut temporary = path.clone().into_os_string();
        temporary.push(".tmp");
        let temporary = PathBuf::from(temporary);
        let written = File::create(&temporary)
            .map_err(|e| format!("创建导出文件失败：{e}"))
            .and_then(|file| {
                let (out, manifest) = write_package(BufWriter::new(file), package)?;
                out.into_inner()
                    .map_err(|e| e.to_string())?
                    .sync_all()
                    .map_err(|e| e.to_string())?;
                Ok(manifest)
            });
        let manifest = match written {
            Ok(manifest) => manifest,
            Err(error) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        };
        if std::fs::symlink_metadata(&path).is_ok() {
            std::fs::remove_file(&path).map_err(|e| {
                let _ = std::fs::remove_file(&temporary);
                format!("覆盖导出文件失败：{e}")
            })?;
        }
        std::fs::rename(&temporary, &path).map_err(|e| {
            let _ = std::fs::remove_file(&temporary);
            e.to_string()
        })?;
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        crate::cycle::log(
            &app,
            "info",
            "配方迁移",
            format!(
                "已导出 {id} 到 {}（{} 轮采集，{:.1} MB）",
                path.display(),
                manifest.rounds.len(),
                bytes as f64 / 1048576.0
            ),
        );
        Ok(Some(TransferExported {
            path: path.display().to_string(),
            bytes,
            kind: manifest.kind,
            rounds: manifest.rounds.len(),
            files: manifest.files.len(),
            notes: manifest.notes,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn workspace_transfer_pick(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<Option<TransferPreview>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_parent(&window)
            .set_title("选择配方迁移包")
            .add_filter("配方迁移包", &["zip"]);
        let exports = crate::storage::root(&app).join("exports");
        if exports.is_dir() {
            dialog = dialog.set_directory(exports);
        }
        let Some(path) = dialog.pick_file() else {
            return Ok(None);
        };
        let (m, bytes) = inspect(&path)?;
        Ok(Some(TransferPreview {
            path: path.display().to_string(),
            kind: m.kind,
            recipe_id: m.recipe_id,
            name: m.name,
            product_code: m.product_code,
            version: m.version,
            exported_at: m.exported_at,
            app_version: m.app_version,
            rounds: m.rounds.len(),
            files: m.files.len(),
            bytes,
            notes: m.notes,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn staging_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let cache = crate::storage::root(app).join("cache");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let stamp = ly_plc::now_ms();
    for suffix in 0..1000 {
        let dir = cache.join(format!("transfer-import-{stamp}-{suffix}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("无法创建导入暂存目录".into())
}

/// 暂存内容移进资料目录：采集轮次到 recipe-capture/（逐轮按采集记录核对），候选目录到 workspaces/<编号>/。失败时撤回已移入的目录。
fn install(
    app: &AppHandle,
    staging: &Path,
    manifest: &TransferManifest,
    target: ImportTarget,
) -> Result<TransferImported, String> {
    let host = app.state::<WorkspaceHost>();
    let cycle = app.state::<CycleHost>();
    let capture = cycle.camera.recipe_capture().clone();
    let captures = capture_root(app);
    let mut items = host.items.lock().unwrap();
    let production = cycle.recipes.list();
    let existing = items
        .values()
        .map(|w| (w.doc.id.as_str(), w.doc.product_code))
        .chain(production.iter().map(|r| (r.id.as_str(), r.product_code)));
    let (mut w, renames, mut notes) = prepare_import(staging, manifest, &target, existing, |id| {
        std::fs::symlink_metadata(captures.join(id)).is_ok()
    })?;
    let dir = host.dir(&w.doc.id);
    if std::fs::symlink_metadata(&dir).is_ok() {
        return Err("配方目录已经存在，请换一个编号".into());
    }
    w.doc.version = cycle
        .recipes
        .next_version(&w.doc.id)?
        .max(w.doc.version.max(1));
    let mut moved: Vec<PathBuf> = Vec::new();
    let result = (|| {
        for (old, new) in &renames {
            let to = captures.join(new);
            std::fs::rename(staging.join("rounds").join(old), &to)
                .map_err(|e| format!("移入采集轮次 {new} 失败：{e}"))?;
            moved.push(to);
            capture
                .get_burst(new)
                .map_err(|e| format!("采集轮次 {new} 核对失败：{e}"))?;
        }
        let candidate = staging.join("workspace");
        if plain_dir(&candidate) {
            std::fs::rename(&candidate, &dir).map_err(|e| format!("移入候选目录失败：{e}"))?;
        } else {
            std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
        }
        moved.push(dir.clone());
        w.ensure_instance(&app.state::<Store>())?;
        host.save(&w)
    })();
    if let Err(error) = result {
        for path in moved.iter().rev() {
            let _ = std::fs::remove_dir_all(path);
        }
        return Err(error);
    }
    if manifest.kind == TransferKind::PublishedDoc {
        notes.push("包内只有配方文档：示教帧与多轮标定需在本机重新采集".into());
    }
    let renamed = renames.iter().filter(|(a, b)| a != b).count();
    let id = w.doc.id.clone();
    items.insert(id.clone(), w);
    Ok(TransferImported {
        id,
        kind: manifest.kind,
        rounds: renames.len(),
        renamed_rounds: renamed,
        notes,
    })
}

#[tauri::command]
pub async fn workspace_transfer_import(
    app: AppHandle,
    path: String,
    id: String,
    name: String,
    product_code: u16,
) -> Result<TransferImported, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let staging = staging_dir(&app)?;
        let result = extract(Path::new(&path), &staging).and_then(|manifest| {
            install(
                &app,
                &staging,
                &manifest,
                ImportTarget {
                    id,
                    name,
                    product_code,
                },
            )
        });
        let _ = std::fs::remove_dir_all(&staging);
        if let Ok(imported) = &result {
            crate::cycle::log(
                &app,
                "info",
                "配方迁移",
                format!(
                    "已从 {path} 导入候选 {}（{} 轮采集）",
                    imported.id, imported.rounds
                ),
            );
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests;
