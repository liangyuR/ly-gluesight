use std::collections::BTreeSet;
use std::fs;
use std::io::BufWriter;
use std::path::{Component, Path};

use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::applog;
use crate::judge::Verdict;
use crate::store::{HistoryQuery, HistoryScope, PartDetail, Store};
use crate::ziparchive::ZipWriter;

const MAX_RANGE_MS: i64 = 31 * 86_400_000;
const MAX_PARTS: usize = 5000;
const MAX_IMAGE_PARTS: usize = 200;
const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_PLC_LOGS: i64 = 200_000;
const MAX_CONFIG_FILE: u64 = 8 * 1024 * 1024;

const README: &str = r#"# 诊断包说明

在 `manifest.json` 查看导出时段、软件版本和配置摘要，在 `notes` 查看缺失或截断的内容。`files` 给出各文件的大小和 SHA-256 校验值。

## 包结构

| 路径 | 内容 |
|---|---|
| `logs/runtime-*.log` | 带本地时间和时区的纯文本运行日志 |
| `logs/gluesight-*` | 导出时段内仍保留的旧版运行日志，使用原文件名和格式 |
| `logs/plc.jsonl` | PLC 通讯日志，含工件通讯事件、连接、点位值变化、写入、边沿、错误与配置事件，按 `category` 区分。仅导出所选时间段内的记录，按时间正序排列 |
| `project.json` | 当前项目的编号、名称与描述 |
| `config/` | 当前项目的配置文件（PLC、相机、检测设置等） |
| `machine/site-offline.json` | 本机的站点模式与离线选择（没有时不打包） |
| `state/` | 运行状态文件（S7 握手日志等） |
| `records/parts.csv`、`records/parts.jsonl` | 检测记录摘要与完整明细 |
| `recipes/<projectId>/<revisionId>.json` | 检测记录所用的配方快照 |
| `records/<记录号>_<cycleId>/` | 原图、`part.json` 与对照结果（按导出选项附带） |

## 排查工件

在运行日志中搜索 SN 或故障发生时间，对照 `logs/plc.jsonl` 里的信号变化。PLC 日志的 `t` 是带时区的本地时间，`ts` 及清单中的时间范围均为 Unix 毫秒时间戳。需要复核检测结论时，按工件的 `cycleId` 查找检测记录、配方快照和原图。
"#;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageMode {
    None,
    #[default]
    Abnormal,
    All,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub from: i64,
    pub to: i64,
    #[serde(default)]
    pub scope: Option<HistoryScope>,
    #[serde(default)]
    pub history_ids: Vec<i64>,
    #[serde(default)]
    pub images: ImageMode,
    #[serde(default = "default_image_parts")]
    pub max_image_parts: usize,
}

fn default_image_parts() -> usize {
    20
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: String,
    pub bytes: u64,
    pub logs: u64,
    pub plc_logs: u64,
    pub parts: usize,
    pub image_parts: usize,
    pub images: usize,
    pub notes: Vec<String>,
}

fn ok_class(verdict: Verdict) -> bool {
    matches!(verdict, Verdict::Ok | Verdict::OkWithExcursion)
}

/// 选择要附原图的工件：指定记录时只取指定的；否则按模式取异常件或全部，新的在前，不超过上限。
pub fn image_parts(
    parts: &[(i64, Verdict)],
    explicit: bool,
    mode: ImageMode,
    max: usize,
) -> Vec<i64> {
    if mode == ImageMode::None {
        return Vec::new();
    }
    let mut chosen: Vec<_> = parts
        .iter()
        .filter(|(_, v)| explicit || mode == ImageMode::All || !ok_class(*v))
        .map(|(id, _)| *id)
        .collect();
    chosen.sort_unstable_by(|a, b| b.cmp(a));
    chosen.truncate(max);
    chosen
}

fn relative(file: &str) -> Option<&Path> {
    let path = Path::new(file);
    (!file.is_empty()
        && !file.contains(':')
        && path.components().all(|c| matches!(c, Component::Normal(_))))
    .then_some(path)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn compressible(name: &str) -> bool {
    !matches!(
        Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "zip")
    )
}

#[derive(Clone, Debug, Serialize)]
struct PackedFile {
    path: String,
    bytes: u64,
    sha256: String,
}

struct Package<W: std::io::Write> {
    zip: ZipWriter<W>,
    notes: Vec<String>,
    files: Vec<PackedFile>,
}

impl<W: std::io::Write> Package<W> {
    fn new(out: W) -> Self {
        Self {
            zip: ZipWriter::new(out),
            notes: Vec::new(),
            files: Vec::new(),
        }
    }

    fn add(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        self.zip
            .add(name, data, compressible(name))
            .map_err(|e| e.to_string())?;
        self.files.push(PackedFile {
            path: name.to_string(),
            bytes: data.len() as u64,
            sha256: format!("{:x}", Sha256::digest(data)),
        });
        Ok(())
    }

    fn copy_file(&mut self, path: &Path, name: &str) -> Result<(), String> {
        if fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_CONFIG_FILE {
            self.notes.push(format!("{name} 超过 8 MB，未打包"));
            return Ok(());
        }
        match fs::read(path) {
            Ok(bytes) => self.add(name, &bytes),
            Err(error) => {
                self.notes.push(format!("{name} 读取失败：{error}"));
                Ok(())
            }
        }
    }

    fn copy_dir(&mut self, dir: &Path, prefix: &str, skip: &[&str]) -> Result<(), String> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Ok(());
        };
        let mut files: Vec<_> = entries
            .flatten()
            .filter(|e| {
                e.file_type().is_ok_and(|t| t.is_file())
                    && !skip.iter().any(|name| e.file_name() == *name)
            })
            .collect();
        files.sort_by_key(|e| e.file_name());
        for entry in files {
            self.copy_file(
                &entry.path(),
                &format!("{prefix}/{}", entry.file_name().to_string_lossy()),
            )?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct PlcLine<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    t: Option<String>,
    #[serde(flatten)]
    entry: &'a ly_plc::LogEntry,
}

/// PLC 日志按时间正序逐行写出，每行补本地时间 t。
fn plc_jsonl(mut entries: Vec<ly_plc::LogEntry>) -> Vec<u8> {
    entries.sort_by_key(|e| (e.ts, e.id));
    let mut out = Vec::new();
    for entry in &entries {
        if let Ok(line) = serde_json::to_string(&PlcLine {
            t: applog::local_time(entry.ts),
            entry,
        }) {
            out.extend_from_slice(line.as_bytes());
            out.push(b'\n');
        }
    }
    out
}

fn collect_parts(
    store: &Store,
    request: &ExportRequest,
    notes: &mut Vec<String>,
) -> Result<Vec<PartDetail>, String> {
    if !request.history_ids.is_empty() {
        let mut parts = Vec::new();
        for &id in request.history_ids.iter().take(MAX_PARTS) {
            match store.detail(id) {
                Ok(detail) => parts.push(detail),
                Err(error) => notes.push(format!("记录 #{id}：{error}")),
            }
        }
        return Ok(parts);
    }
    let scopes = request
        .scope
        .map_or_else(|| vec![HistoryScope::Real], |s| vec![s]);
    let mut ids = Vec::new();
    for scope in scopes {
        let mut query = HistoryQuery {
            from: Some(request.from),
            to: Some(request.to.saturating_add(1)),
            scope,
            limit: 500,
            ..HistoryQuery::default()
        };
        loop {
            let page = store.query(&query)?;
            ids.extend(page.items.iter().map(|p| p.id));
            if page.items.len() < 500 || ids.len() > MAX_PARTS {
                break;
            }
            query.offset += 500;
        }
    }
    ids.sort_unstable_by(|a, b| b.cmp(a));
    if ids.len() > MAX_PARTS {
        notes.push(format!(
            "时间段内记录超过 {MAX_PARTS} 件，只打包最新的 {MAX_PARTS} 件"
        ));
        ids.truncate(MAX_PARTS);
    }
    ids.into_iter().map(|id| store.detail(id)).collect()
}

fn plc_logs(app: &AppHandle, from: i64, to: i64, notes: &mut Vec<String>) -> Vec<ly_plc::LogEntry> {
    let host = app.state::<crate::plc::PlcHost>();
    let mut offset = 0u32;
    let mut entries = Vec::new();
    loop {
        let query = ly_plc::LogQuery {
            start: Some(from),
            end: Some(to),
            levels: Vec::new(),
            categories: Vec::new(),
            point_id: None,
            context: None,
            keyword: None,
            limit: Some(1000),
            offset: Some(offset),
        };
        match host.engine().logbook().query(&query) {
            Ok(page) => {
                let full = page.items.len() == 1000;
                entries.extend(page.items);
                if !full {
                    break;
                }
                if entries.len() as i64 >= MAX_PLC_LOGS {
                    notes.push(format!(
                        "PLC 日志超过 {MAX_PLC_LOGS} 条，只打包最新的 {MAX_PLC_LOGS} 条"
                    ));
                    break;
                }
                offset += 1000;
            }
            Err(error) => {
                notes.push(format!("PLC 日志读取失败：{error}"));
                break;
            }
        }
    }
    entries
}

fn part_images<W: std::io::Write>(
    app: &AppHandle,
    package: &mut Package<W>,
    detail: &PartDetail,
    budget: &mut u64,
) -> Result<usize, String> {
    let id = detail.summary.id;
    let Some(cycle_id) = detail.summary.cycle_id.as_deref() else {
        package
            .notes
            .push(format!("记录 #{id} 没有 cycleId，无法确定原图"));
        return Ok(0);
    };
    let root = match crate::project::records::record_root_of(
        &crate::storage::root(app),
        &detail.summary.project_id,
        detail.recording.directory.as_deref(),
    ) {
        Ok(root) => root,
        Err(error) => {
            package
                .notes
                .push(format!("记录 #{id} 的原图未打包：{error}"));
            return Ok(0);
        }
    };
    let folder = format!("records/{id}_{cycle_id}");
    let mut count = 0;
    let mut directory = None;
    // 原图按 (相机, seq) 记在帧记录里（FrameRecord.raw_file）
    let frame_files = detail.burst.iter().flat_map(|burst| {
        burst.frames.iter().filter_map(move |frame| {
            let camera = burst.cameras.get(frame.camera).map_or("?", String::as_str);
            frame
                .raw_file
                .as_deref()
                .map(|file| (format!("相机 {camera} 第 {} 帧", frame.seq), file))
        })
    });
    let files: Vec<(String, &str)> = frame_files.collect();
    for (label, file) in &files {
        let Some(path) = relative(file) else {
            package
                .notes
                .push(format!("记录 #{id} {label} 的原图路径无效，未打包"));
            continue;
        };
        let source = root.join(path);
        directory.get_or_insert_with(|| source.parent().map(Path::to_path_buf));
        // 空闲压缩过的原图（同名 .gsr）按原始字节数计、还原成原始 PGM 打包
        let size = crate::image_file::read_original_info(&source).map_or(0, |(_, length)| length);
        if size > *budget {
            package.notes.push(format!(
                "原图总量超过 {} GB，记录 #{id} 起的其余原图未打包",
                MAX_IMAGE_BYTES >> 30
            ));
            *budget = 0;
            return Ok(count);
        }
        match crate::image_file::read_original(&source) {
            Ok(bytes) => {
                *budget -= bytes.len() as u64;
                package.add(&format!("{folder}/{}", file_name(path)), &bytes)?;
                count += 1;
            }
            Err(_) => package
                .notes
                .push(format!("记录 #{id} {label} 的原图未保留或已清理")),
        }
    }
    if let Some(Some(dir)) = directory {
        if let Ok(bytes) = fs::read(dir.join("part.json")) {
            package.add(&format!("{folder}/part.json"), &bytes)?;
        }
    }
    if count == 0 && files.is_empty() {
        package.notes.push(format!(
            "记录 #{id} 没有保留原图（录制状态：{:?}）",
            detail.recording.state
        ));
    }
    let comparisons = crate::storage::project(app)
        .join("workspaces")
        .join("comparisons")
        .join(id.to_string());
    package.copy_dir(&comparisons, &format!("{folder}/comparisons"), &[])?;
    Ok(count)
}

pub fn export(app: &AppHandle, request: ExportRequest) -> Result<ExportResult, String> {
    if request.to <= request.from {
        return Err("结束时间必须晚于开始时间".into());
    }
    if request.to.saturating_sub(request.from) > MAX_RANGE_MS {
        return Err("诊断包时间范围最多 31 天，请缩小范围".into());
    }
    let dir = crate::storage::root(app).join("exports");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = Local::now().format("%Y%m%d_%H%M%S_%3f");
    let path = dir.join(format!("诊断包_{stamp}.zip"));
    let temporary = dir.join(format!("诊断包_{stamp}.zip.tmp"));
    let file = fs::File::create_new(&temporary).map_err(|e| e.to_string())?;
    let mut package = Package::new(BufWriter::new(file));
    let built = build(app, &request, &mut package);
    let result = built.and_then(|summary| {
        package.add("README.md", README.as_bytes())?;
        let version = app.package_info().version.to_string();
        let identity = applog::build_identity();
        let manifest = json!({
            "schema": 3, "createdAt": ly_plc::now_ms(), "app": {"name": app.package_info().name, "version": version},
            "build": {"version": version, "gitSha": identity.git_sha, "buildTime": identity.build_time},
            "utcOffset": Local::now().format("%:z").to_string(),
            "dataRoot": crate::storage::root(app).display().to_string(), "range": {"from": request.from, "to": request.to},
            "scope": request.scope, "historyIds": request.history_ids, "images": request.images, "maxImageParts": request.max_image_parts,
            "configSummary": crate::commands::config_summary(app),
            "summary": summary, "notes": package.notes, "files": package.files,
        });
        package.add("manifest.json", &serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?)?;
        Ok(summary)
    });
    let finished = result.and_then(|summary| {
        let Package { zip, notes, .. } = package;
        zip.finish()
            .map_err(|e| e.to_string())?
            .into_inner()
            .map_err(|e| e.to_string())?
            .sync_all()
            .map_err(|e| e.to_string())?;
        Ok((summary, notes))
    });
    let (summary, notes) = match finished {
        Ok(done) => done,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    fs::rename(&temporary, &path).map_err(|e| {
        let _ = fs::remove_file(&temporary);
        e.to_string()
    })?;
    let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    tracing::info!(path = %path.display(), bytes, "已导出诊断包");
    Ok(ExportResult {
        path: path.display().to_string(),
        bytes,
        notes,
        ..summary
    })
}

fn build<W: std::io::Write>(
    app: &AppHandle,
    request: &ExportRequest,
    package: &mut Package<W>,
) -> Result<ExportResult, String> {
    let mut logs = 0;
    let files = applog::files_in_range(&applog::dir(app)?, request.from, request.to)
        .unwrap_or_else(|error| {
            package.notes.push(format!("运行日志目录读取失败：{error}"));
            Vec::new()
        });
    for file in files {
        let name = file_name(&file);
        let mut out = Vec::new();
        match applog::export_file(&file, request.from, request.to, &mut out) {
            Ok(count) => {
                if !out.is_empty() {
                    package.add(&format!("logs/{name}"), &out)?;
                }
                logs += count;
            }
            Err(error) => package
                .notes
                .push(format!("运行日志 {name} 未打包：{error}")),
        }
    }
    let entries = plc_logs(app, request.from, request.to, &mut package.notes);
    let plc_logs = entries.len() as u64;
    package.add("logs/plc.jsonl", &plc_jsonl(entries))?;
    let project = crate::storage::project(app);
    package.copy_file(&project.join("project.json"), "project.json")?;
    package.copy_dir(&crate::storage::project_config(app), "config", &[])?;
    let machine = crate::storage::config(app).join("site-offline.json");
    if machine.is_file() {
        package.copy_file(&machine, "machine/site-offline.json")?;
    }
    let state = crate::storage::state(app);
    package.copy_dir(&state, "state", &[])?;
    let store = app.state::<Store>();
    let parts = collect_parts(&store, request, &mut package.notes)?;
    let mut csv = crate::history::CSV_HEADER.as_bytes().to_vec();
    let mut details = Vec::new();
    let mut revisions = BTreeSet::new();
    for detail in &parts {
        csv.extend_from_slice(crate::history::csv_row(&detail.summary).as_bytes());
        details.extend_from_slice(&serde_json::to_vec(detail).map_err(|e| e.to_string())?);
        details.push(b'\n');
        if let Some(revision) = &detail.summary.recipe_revision {
            revisions.insert((detail.summary.project_id.clone(), revision.clone()));
        }
    }
    package.add("records/parts.csv", &csv)?;
    package.add("records/parts.jsonl", &details)?;
    for (project, revision) in &revisions {
        match store.recipe_snapshot(project, revision) {
            Ok(Some(recipe)) => package.add(
                &format!("recipes/{project}/{revision}.json"),
                &serde_json::to_vec_pretty(&recipe).map_err(|e| e.to_string())?,
            )?,
            Ok(None) => package.notes.push(format!("配方快照 {revision} 缺失")),
            Err(error) => package
                .notes
                .push(format!("配方快照 {revision} 读取失败：{error}")),
        }
    }
    let chosen = image_parts(
        &parts
            .iter()
            .map(|p| (p.summary.id, p.summary.verdict))
            .collect::<Vec<_>>(),
        !request.history_ids.is_empty(),
        request.images,
        request.max_image_parts.min(MAX_IMAGE_PARTS),
    );
    let mut budget = MAX_IMAGE_BYTES;
    let mut images = 0;
    let mut image_parts = 0;
    for detail in parts.iter().filter(|p| chosen.contains(&p.summary.id)) {
        if budget == 0 {
            break;
        }
        let added = part_images(app, package, detail, &mut budget)?;
        if added > 0 {
            image_parts += 1;
        }
        images += added;
    }
    Ok(ExportResult {
        path: String::new(),
        bytes: 0,
        logs,
        plc_logs,
        parts: parts.len(),
        image_parts,
        images,
        notes: Vec::new(),
    })
}

#[tauri::command]
pub async fn diagnostics_export(
    app: AppHandle,
    request: Option<ExportRequest>,
) -> Result<ExportResult, String> {
    let request = request.unwrap_or_else(|| {
        let to = ly_plc::now_ms();
        ExportRequest {
            from: to - 86_400_000,
            to,
            scope: None,
            history_ids: Vec::new(),
            images: ImageMode::Abnormal,
            max_image_parts: default_image_parts(),
        }
    });
    tauri::async_runtime::spawn_blocking(move || {
        let _busy = crate::raw_compress::activity().busy();
        export(&app, request)
    })
    .await
    .map_err(|e| e.to_string())?
}
