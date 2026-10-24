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
use crate::cycle::CycleHost;
use crate::judge::Verdict;
use crate::store::{HistoryQuery, HistoryScope, PartDetail, RecordSource, Store};
use crate::ziparchive::ZipWriter;

const MAX_RANGE_MS: i64 = 31 * 86_400_000;
const MAX_PARTS: usize = 5000;
const MAX_IMAGE_PARTS: usize = 200;
const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_PLC_LOGS: i64 = 200_000;
const MAX_CONFIG_FILE: u64 = 8 * 1024 * 1024;
const AUDIT_FILE: &str = "plc-handshake.audit.jsonl";
const AUDIT_ACTIVE: &str = "state/plc-handshake.audit.jsonl";

const README: &str = r#"# 诊断包说明

本包由 GlueSight 诊断导出生成，供人工或 Agent 离线分析。导出参数、构建信息与配置摘要见 `manifest.json`。

## 包结构

| 路径 | 内容 |
|---|---|
| `manifest.json` | 导出时间段与参数；`build`（version、gitSha、buildTime）、`utcOffset`、`logSchema`、`configSummary`（与运行日志 `app.config` 的 data 相同）、`logLimits`、`summary`；`notes` 列出未打包或被截断的内容；`files` 列出除 manifest.json 本身外每个文件的 path、bytes、sha256 |
| `README.md` | 本说明 |
| `logs/gluesight-*.jsonl` | 运行日志，按天分文件，只含导出时间段内的行 |
| `logs/plc.jsonl` | PLC 日志（连接、点位值变化、写入、边沿、错误与配置事件，按 `category` 区分），只含导出时间段内的行，时间正序 |
| `project.json` | 当前项目的编号、名称与描述 |
| `config/` | 当前项目的配置文件（PLC、相机、检测设置等） |
| `machine/site-offline.json` | 本机的站点模式与离线选择（没有时不打包） |
| `state/` | 运行状态文件（S7 握手事务、示教握手等）；`state/plc-handshake.audit.jsonl` 只含导出时间段内的握手审计行 |
| `state/plc-handshake-audit/` | 与导出时间段有交集的已滚动握手审计分段（整段，可能含时间段外的行） |
| `records/parts.csv`、`records/parts.jsonl` | 检测记录摘要与完整明细 |
| `recipes/<revisionId>.json` | 检测记录所用的配方快照 |
| `records/<记录号>_<cycleId>/` | 原图、`part.json` 与对照结果（按导出选项附带） |

## 时间口径

- `ts`：毫秒 Unix 时间戳，各文件通用；排序与跨文件对齐以它为准。`manifest.json` 的 `createdAt`、`range.from`、`range.to` 同此口径。
- `t`：本地时间，RFC 3339 带时区偏移、毫秒精度，如 `2026-10-24T18:18:12.059+08:00`；运行日志与 `logs/plc.jsonl` 每行都有。现场时区见 `manifest.json` 的 `utcOffset`。

## 事件编码

- 运行日志 schema 版本：{logSchema}（即 `manifest.json` 的 `logSchema`）。
- 每行的 `code` 是稳定的事件编码，含义与 `data` 字段见源码仓库 `docs/architecture/agent-logging-codes.md`（与 `src-tauri/src/applog/codes.rs` 逐条一致）；对应源码版本见 `build.gitSha`。
- 没有 `code` 的行来自未改造的调用点（不做全量迁移），按 `ev` 与 `msg` 理解。

## 推荐分析顺序

1. 读 `manifest.json`：确认构建版本、时间段与 `notes`，按 `files` 校验文件完整性。
2. 在运行日志中先找 `fault.enter` 与 `s7.fault_output`，确定故障时刻、来源与原因；没有这些编码时，找 `ev` 为“故障”的行。
3. 按 `teachId`（示教握手）或 `cycleId`（工件）串起同一事务的事件，对照同一时间窗口内 `logs/plc.jsonl` 的信号变化与 `state/plc-handshake.audit.jsonl`。
4. 涉及检测结论时，按 `cycleId` 对照 `records/parts.jsonl`、配方快照与原图。
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

fn default_image_parts() -> usize { 20 }

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
pub fn image_parts(parts: &[(i64, Verdict)], explicit: bool, mode: ImageMode, max: usize) -> Vec<i64> {
    if mode == ImageMode::None { return Vec::new(); }
    let mut chosen: Vec<_> = parts.iter().filter(|(_, v)| explicit || mode == ImageMode::All || !ok_class(*v)).map(|(id, _)| *id).collect();
    chosen.sort_unstable_by(|a, b| b.cmp(a));
    chosen.truncate(max);
    chosen
}

fn relative(file: &str) -> Option<&Path> {
    let path = Path::new(file);
    (!file.is_empty() && !file.contains(':') && path.components().all(|c| matches!(c, Component::Normal(_)))).then_some(path)
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn compressible(name: &str) -> bool {
    !matches!(Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("jpg" | "jpeg" | "png" | "zip"))
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
            self.copy_file(&entry.path(), &format!("{prefix}/{}", entry.file_name().to_string_lossy()))?;
        }
        Ok(())
    }

    fn copy_handshake_audit(&mut self, active: &Path, from: i64, to: i64) -> Result<(), String> {
        match fs::read(active) {
            Ok(bytes) => {
                let (out, untimed) = audit_lines_in_range(&bytes, from, to);
                if untimed > 0 {
                    self.notes
                        .push(format!("{AUDIT_ACTIVE} 有 {untimed} 行无法解析 ts，未打包"));
                }
                if !out.is_empty() {
                    self.add(AUDIT_ACTIVE, &out)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => self.notes.push(format!("{AUDIT_ACTIVE} 读取失败：{error}")),
        }
        let (chosen, skipped) = crate::handshake_audit::overlapping(active, from, to);
        for path in &chosen {
            self.copy_file(path, &format!("state/plc-handshake-audit/{}", file_name(path)))?;
        }
        if skipped > 0 {
            self.notes.push(format!("握手审计另有 {skipped} 个分段不在导出时间段内，未打包（现场 state/plc-handshake-audit/）"));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct AuditStamp {
    ts: i64,
}

/// 握手审计当前文件里 ts 落在 [from, to] 的行，及无法解析 ts 的行数。
fn audit_lines_in_range(bytes: &[u8], from: i64, to: i64) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut untimed = 0;
    for line in bytes
        .split(|b| *b == b'\n')
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
        .filter(|l| !l.is_empty())
    {
        match serde_json::from_slice::<AuditStamp>(line) {
            Ok(stamp) if (from..=to).contains(&stamp.ts) => {
                out.extend_from_slice(line);
                out.push(b'\n');
            }
            Ok(_) => {}
            Err(_) => untimed += 1,
        }
    }
    (out, untimed)
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

fn collect_parts(store: &Store, request: &ExportRequest, notes: &mut Vec<String>) -> Result<Vec<PartDetail>, String> {
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
    let scopes = request.scope.map_or_else(|| vec![HistoryScope::Real, HistoryScope::Offline, HistoryScope::Debug], |s| vec![s]);
    let mut ids = Vec::new();
    for scope in scopes {
        let mut query = HistoryQuery { from: Some(request.from), to: Some(request.to + 1), scope, limit: 500, ..HistoryQuery::default() };
        loop {
            let page = store.query(&query)?;
            ids.extend(page.items.iter().map(|p| p.id));
            if page.items.len() < 500 || ids.len() > MAX_PARTS { break; }
            query.offset += 500;
        }
    }
    ids.sort_unstable_by(|a, b| b.cmp(a));
    if ids.len() > MAX_PARTS {
        notes.push(format!("时间段内记录超过 {MAX_PARTS} 件，只打包最新的 {MAX_PARTS} 件"));
        ids.truncate(MAX_PARTS);
    }
    ids.into_iter().map(|id| store.detail(id)).collect()
}

fn plc_logs(app: &AppHandle, from: i64, to: i64, notes: &mut Vec<String>) -> Vec<ly_plc::LogEntry> {
    let host = app.state::<crate::plc::PlcHost>();
    let mut offset = 0u32;
    let mut entries = Vec::new();
    loop {
        let query = ly_plc::LogQuery { start: Some(from), end: Some(to), levels: Vec::new(), categories: Vec::new(), point_id: None, context: None,
            keyword: None, limit: Some(1000), offset: Some(offset) };
        match host.engine().logbook().query(&query) {
            Ok(page) => {
                let full = page.items.len() == 1000;
                entries.extend(page.items);
                if !full {
                    break;
                }
                if entries.len() as i64 >= MAX_PLC_LOGS {
                    notes.push(format!("PLC 日志超过 {MAX_PLC_LOGS} 条，只打包最新的 {MAX_PLC_LOGS} 条"));
                    break;
                }
                offset += 1000;
            }
            Err(error) => { notes.push(format!("PLC 日志读取失败：{error}")); break; }
        }
    }
    entries
}

fn part_images<W: std::io::Write>(app: &AppHandle, package: &mut Package<W>, detail: &PartDetail, budget: &mut u64) -> Result<usize, String> {
    let id = detail.summary.id;
    if detail.summary.source == RecordSource::OfflineCandidate {
        package.notes.push(format!("记录 #{id} 为候选调试件，原图在受管样本 samples/ 中，未重复打包"));
        return Ok(0);
    }
    let Some(cycle_id) = detail.summary.cycle_id.as_deref() else {
        package.notes.push(format!("记录 #{id} 没有 cycleId，无法确定原图"));
        return Ok(0);
    };
    let root = app.state::<CycleHost>().recorders.root(detail.summary.source).to_path_buf();
    let folder = format!("records/{id}_{cycle_id}");
    let mut count = 0;
    let mut directory = None;
    // 旧拍照点配方按拍照点与视角记原图；段配方按 (相机, seq) 记在帧记录里（FrameRecord.raw_file）
    let shot_files = detail.shots.iter().flat_map(|shot| shot.raw_files.iter().map(move |raw| (format!("k{} 视角 {}", shot.k + 1, raw.view), raw.file.as_str())));
    let frame_files = detail.burst.iter().flat_map(|burst| burst.frames.iter().filter_map(move |frame| {
        let camera = burst.cameras.get(frame.camera).map_or("?", String::as_str);
        frame.raw_file.as_deref().map(|file| (format!("相机 {camera} 第 {} 帧", frame.seq), file))
    }));
    let files: Vec<(String, &str)> = shot_files.chain(frame_files).collect();
    for (label, file) in &files {
        let Some(path) = relative(file) else {
            package.notes.push(format!("记录 #{id} {label} 的原图路径无效，未打包"));
            continue;
        };
        let source = root.join(path);
        directory.get_or_insert_with(|| source.parent().map(Path::to_path_buf));
        let size = fs::metadata(&source).map(|m| m.len()).unwrap_or(0);
        if size > *budget {
            package.notes.push(format!("原图总量超过 {} GB，记录 #{id} 起的其余原图未打包", MAX_IMAGE_BYTES >> 30));
            *budget = 0;
            return Ok(count);
        }
        match fs::read(&source) {
            Ok(bytes) => {
                *budget -= bytes.len() as u64;
                package.add(&format!("{folder}/{}", file_name(path)), &bytes)?;
                count += 1;
            }
            Err(_) => package.notes.push(format!("记录 #{id} {label} 的原图未保留或已清理")),
        }
    }
    if let Some(Some(dir)) = directory {
        if let Ok(bytes) = fs::read(dir.join("part.json")) { package.add(&format!("{folder}/part.json"), &bytes)?; }
    }
    if count == 0 && files.is_empty() {
        package.notes.push(format!("记录 #{id} 没有保留原图（录制状态：{:?}）", detail.recording.state));
    }
    let comparisons = crate::storage::project(app).join("workspaces").join("comparisons").join(id.to_string());
    package.copy_dir(&comparisons, &format!("{folder}/comparisons"), &[])?;
    Ok(count)
}

pub fn export(app: &AppHandle, request: ExportRequest) -> Result<ExportResult, String> {
    if request.to <= request.from { return Err("结束时间必须晚于开始时间".into()); }
    if request.to - request.from > MAX_RANGE_MS { return Err("诊断包时间范围最多 31 天，请缩小范围".into()); }
    let dir = crate::storage::root(app).join("exports");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = Local::now().format("%Y%m%d_%H%M%S");
    let path = dir.join(format!("诊断包_{stamp}.zip"));
    let temporary = dir.join(format!("诊断包_{stamp}.zip.tmp"));
    let file = fs::File::create(&temporary).map_err(|e| e.to_string())?;
    let mut package = Package::new(BufWriter::new(file));
    let built = build(app, &request, &mut package);
    let result = built.and_then(|summary| {
        package.add("README.md", README.replace("{logSchema}", &applog::SCHEMA.to_string()).as_bytes())?;
        let version = app.package_info().version.to_string();
        let identity = applog::build_identity();
        let manifest = json!({
            "schema": 2, "createdAt": ly_plc::now_ms(), "app": {"name": app.package_info().name, "version": version},
            "build": {"version": version, "gitSha": identity.git_sha, "buildTime": identity.build_time},
            "utcOffset": Local::now().format("%:z").to_string(), "logSchema": applog::SCHEMA,
            "dataRoot": crate::storage::root(app).display().to_string(), "range": {"from": request.from, "to": request.to},
            "scope": request.scope, "historyIds": request.history_ids, "images": request.images, "maxImageParts": request.max_image_parts,
            "configSummary": crate::commands::config_summary(app),
            "logLimits": applog::usage(&applog::dir(app)?), "summary": summary, "notes": package.notes, "files": package.files,
        });
        package.add("manifest.json", &serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?)?;
        Ok(summary)
    });
    let finished = result.and_then(|summary| {
        let Package { zip, notes, .. } = package;
        zip.finish().map_err(|e| e.to_string())?.into_inner().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())?;
        Ok((summary, notes))
    });
    let (summary, notes) = match finished {
        Ok(done) => done,
        Err(error) => { let _ = fs::remove_file(&temporary); return Err(error); }
    };
    fs::rename(&temporary, &path).map_err(|e| { let _ = fs::remove_file(&temporary); e.to_string() })?;
    let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    crate::cycle::log(app, "info", "诊断包", format!("已导出 {}（{:.1} MB）", path.display(), bytes as f64 / 1048576.0));
    Ok(ExportResult { path: path.display().to_string(), bytes, notes, ..summary })
}

fn build<W: std::io::Write>(app: &AppHandle, request: &ExportRequest, package: &mut Package<W>) -> Result<ExportResult, String> {
    let mut logs = 0;
    for file in applog::files_in_range(&applog::dir(app)?, request.from, request.to) {
        let mut out = Vec::new();
        logs += applog::export_file(&file, request.from, request.to, &mut out);
        if !out.is_empty() { package.add(&format!("logs/{}", file_name(&file)), &out)?; }
    }
    let entries = plc_logs(app, request.from, request.to, &mut package.notes);
    let plc_logs = entries.len() as u64;
    package.add("logs/plc.jsonl", &plc_jsonl(entries))?;
    let project = crate::storage::project(app);
    package.copy_file(&project.join("project.json"), "project.json")?;
    package.copy_dir(&crate::storage::project_config(app), "config", &[])?;
    let machine = crate::storage::config(app).join("site-offline.json");
    if machine.is_file() { package.copy_file(&machine, "machine/site-offline.json")?; }
    let state = crate::storage::state(app);
    package.copy_dir(&state, "state", &[AUDIT_FILE])?;
    package.copy_handshake_audit(&state.join(AUDIT_FILE), request.from, request.to)?;
    let store = app.state::<Store>();
    let parts = collect_parts(&store, request, &mut package.notes)?;
    let mut csv = crate::history::CSV_HEADER.as_bytes().to_vec();
    let mut details = Vec::new();
    let mut revisions = BTreeSet::new();
    for detail in &parts {
        csv.extend_from_slice(crate::history::csv_row(&detail.summary).as_bytes());
        details.extend_from_slice(&serde_json::to_vec(detail).map_err(|e| e.to_string())?);
        details.push(b'\n');
        if let Some(revision) = &detail.summary.recipe_revision { revisions.insert(revision.clone()); }
    }
    package.add("records/parts.csv", &csv)?;
    package.add("records/parts.jsonl", &details)?;
    for revision in &revisions {
        match store.recipe_snapshot(revision) {
            Ok(Some(recipe)) => package.add(&format!("recipes/{revision}.json"), &serde_json::to_vec_pretty(&recipe).map_err(|e| e.to_string())?)?,
            Ok(None) => package.notes.push(format!("配方快照 {revision} 缺失")),
            Err(error) => package.notes.push(format!("配方快照 {revision} 读取失败：{error}")),
        }
    }
    let chosen = image_parts(&parts.iter().map(|p| (p.summary.id, p.summary.verdict)).collect::<Vec<_>>(),
        !request.history_ids.is_empty(), request.images, request.max_image_parts.min(MAX_IMAGE_PARTS));
    let mut budget = MAX_IMAGE_BYTES;
    let mut images = 0;
    let mut image_parts = 0;
    for detail in parts.iter().filter(|p| chosen.contains(&p.summary.id)) {
        if budget == 0 { break; }
        let added = part_images(app, package, detail, &mut budget)?;
        if added > 0 { image_parts += 1; }
        images += added;
    }
    Ok(ExportResult { path: String::new(), bytes: 0, logs, plc_logs, parts: parts.len(), image_parts, images, notes: Vec::new() })
}

#[tauri::command]
pub async fn diagnostics_export(app: AppHandle, request: ExportRequest) -> Result<ExportResult, String> {
    tauri::async_runtime::spawn_blocking(move || export(&app, request)).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PLC 日志库按 ts 倒序分页返回；logs/plc.jsonl 要求时间正序，且每行 t 与 ts 是同一时刻，Agent 才能与运行日志按时间对齐。
    #[test]
    fn plc_logs_fetched_newest_first_are_written_oldest_first_with_matching_t() {
        let entry = |id, ts| ly_plc::LogEntry {
            id,
            ts,
            ..ly_plc::LogEntry::new(ly_plc::Level::Info, ly_plc::Category::Value, "")
        };
        let out = plc_jsonl(vec![
            entry(3, 1_792_837_092_059),
            entry(2, 1_792_837_092_059),
            entry(1, 1_792_837_091_000),
        ]);
        let lines: Vec<serde_json::Value> = out
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).unwrap())
            .collect();
        assert_eq!(
            lines
                .iter()
                .map(|l| l["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        for line in &lines {
            let t = chrono::DateTime::parse_from_rfc3339(line["t"].as_str().unwrap()).unwrap();
            assert_eq!(t.timestamp_millis(), line["ts"].as_i64().unwrap());
        }
    }
}
