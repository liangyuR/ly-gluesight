use std::collections::BTreeSet;
use std::fs;
use std::io::BufWriter;
use std::path::{Component, Path};

use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::json;
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

struct Package<W: std::io::Write> {
    zip: ZipWriter<W>,
    notes: Vec<String>,
}

impl<W: std::io::Write> Package<W> {
    fn add(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        self.zip.add(name, data, compressible(name)).map_err(|e| e.to_string())
    }

    fn copy_dir(&mut self, dir: &Path, prefix: &str) -> Result<(), String> {
        let Ok(entries) = fs::read_dir(dir) else { return Ok(()) };
        let mut files: Vec<_> = entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())).collect();
        files.sort_by_key(|e| e.file_name());
        for entry in files {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX) > MAX_CONFIG_FILE {
                self.notes.push(format!("{prefix}/{name} 超过 8 MB，未打包"));
                continue;
            }
            match fs::read(entry.path()) {
                Ok(bytes) => self.add(&format!("{prefix}/{name}"), &bytes)?,
                Err(error) => self.notes.push(format!("{prefix}/{name} 读取失败：{error}")),
            }
        }
        Ok(())
    }
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

fn plc_logs(app: &AppHandle, from: i64, to: i64, out: &mut Vec<u8>, notes: &mut Vec<String>) -> u64 {
    let host = app.state::<crate::plc::PlcHost>();
    let mut offset = 0u32;
    let mut count = 0u64;
    loop {
        let query = ly_plc::LogQuery { start: Some(from), end: Some(to), levels: Vec::new(), categories: Vec::new(), point_id: None,
            keyword: None, limit: Some(1000), offset: Some(offset) };
        match host.engine().logbook().query(&query) {
            Ok(page) => {
                for entry in &page.items {
                    if let Ok(line) = serde_json::to_string(entry) {
                        out.extend_from_slice(line.as_bytes());
                        out.push(b'\n');
                        count += 1;
                    }
                }
                if page.items.len() < 1000 { break; }
                if count as i64 >= MAX_PLC_LOGS {
                    notes.push(format!("PLC 日志超过 {MAX_PLC_LOGS} 条，只打包最新的 {MAX_PLC_LOGS} 条"));
                    break;
                }
                offset += 1000;
            }
            Err(error) => { notes.push(format!("PLC 日志读取失败：{error}")); break; }
        }
    }
    count
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
    for shot in &detail.shots {
        for raw in &shot.raw_files {
            let Some(path) = relative(&raw.file) else {
                package.notes.push(format!("记录 #{id} k{} 视角 {} 的原图路径无效，未打包", shot.k + 1, raw.view));
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
                Err(_) => package.notes.push(format!("记录 #{id} k{} 视角 {} 的原图未保留或已清理", shot.k + 1, raw.view)),
            }
        }
    }
    if let Some(Some(dir)) = directory {
        if let Ok(bytes) = fs::read(dir.join("part.json")) { package.add(&format!("{folder}/part.json"), &bytes)?; }
    }
    if count == 0 && detail.shots.iter().all(|s| s.raw_files.is_empty()) {
        package.notes.push(format!("记录 #{id} 没有保留原图（录制状态：{:?}）", detail.recording.state));
    }
    let comparisons = crate::storage::root(app).join("workspaces").join("comparisons").join(id.to_string());
    package.copy_dir(&comparisons, &format!("{folder}/comparisons"))?;
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
    let mut package = Package { zip: ZipWriter::new(BufWriter::new(file)), notes: Vec::new() };
    let built = build(app, &request, &mut package);
    let result = built.and_then(|summary| {
        let manifest = json!({
            "schema": 1, "createdAt": ly_plc::now_ms(), "app": {"name": app.package_info().name, "version": app.package_info().version.to_string()},
            "dataRoot": crate::storage::root(app).display().to_string(), "range": {"from": request.from, "to": request.to},
            "scope": request.scope, "historyIds": request.history_ids, "images": request.images, "maxImageParts": request.max_image_parts,
            "logLimits": applog::usage(&applog::dir(app)?), "summary": summary, "notes": package.notes,
        });
        package.add("manifest.json", &serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?)?;
        Ok(summary)
    });
    let finished = result.and_then(|summary| {
        let Package { zip, notes } = package;
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
    let mut out = Vec::new();
    let plc_logs = plc_logs(app, request.from, request.to, &mut out, &mut package.notes);
    package.add("logs/plc.jsonl", &out)?;
    package.copy_dir(&crate::storage::config(app), "config")?;
    package.copy_dir(&crate::storage::state(app), "state")?;
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

    #[test]
    fn image_parts_take_abnormal_newest_first_unless_records_are_named() {
        let parts = [(1, Verdict::Ok), (2, Verdict::NgGap), (3, Verdict::OkWithExcursion), (4, Verdict::ErrInspect), (5, Verdict::NgWidth)];
        assert_eq!(image_parts(&parts, false, ImageMode::Abnormal, 2), [5, 4]);
        assert_eq!(image_parts(&parts, false, ImageMode::All, 3), [5, 4, 3]);
        assert_eq!(image_parts(&parts[..1], true, ImageMode::Abnormal, 20), [1]);
        assert!(image_parts(&parts, true, ImageMode::None, 20).is_empty());
    }
}
