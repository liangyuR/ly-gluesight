//! 帧录制目录列表（测试脚本用来定位录制根目录）。

use serde::Serialize;
use tauri::State;

use crate::cycle::CycleHost;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordEntry {
    pub path: String,
    pub name: String,
    pub frames: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordListing {
    pub root: String,
    pub items: Vec<RecordEntry>,
}

/// 录制目录里的原图帧数：未压缩的 `.pgm` 与空闲压缩过的 `.gsr`。
fn frame_count(part: &std::path::Path) -> usize {
    let raw = |path: std::path::PathBuf| {
        path.extension()
            .is_some_and(|x| x == "pgm" || x == crate::image_file::gsr::EXTENSION)
    };
    std::fs::read_dir(part)
        .map(|entries| entries.flatten().filter(|e| raw(e.path())).count())
        .unwrap_or(0)
}

/// 帧录制目录，新的在前，最多 200 条。
#[tauri::command]
pub fn records_list(cycle: State<'_, CycleHost>) -> RecordListing {
    let roots = cycle.recorders.roots(crate::store::RecordSource::Real);
    let mut items = Vec::new();
    for days in roots.iter().filter_map(|root| std::fs::read_dir(root).ok()) {
        for day in days.flatten().map(|d| d.path()).filter(|p| p.is_dir() && !p.ends_with("_pending")) {
            for part in std::fs::read_dir(&day).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
                let frames = frame_count(&part);
                let name = part.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                items.push(RecordEntry { path: part.display().to_string(), name, frames });
            }
        }
    }
    items.sort_by(|a, b| b.name.cmp(&a.name));
    items.truncate(200);
    RecordListing {
        root: roots
            .iter()
            .map(|root| root.display().to_string())
            .collect::<Vec<_>>()
            .join("；"),
        items,
    }
}
