//! 帧录制目录列表（回放相机选目录用）。

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

/// 帧录制目录，新的在前，最多 200 条。
#[tauri::command]
pub fn records_list(cycle: State<'_, CycleHost>) -> RecordListing {
    let root = cycle.recorder.root().to_path_buf();
    let mut items = Vec::new();
    if let Ok(days) = std::fs::read_dir(&root) {
        for day in days.flatten().map(|d| d.path()).filter(|p| p.is_dir() && !p.ends_with("_pending")) {
            for part in std::fs::read_dir(&day).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
                let frames = std::fs::read_dir(&part).map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "pgm")).count()).unwrap_or(0);
                let name = part.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                items.push(RecordEntry { path: part.display().to_string(), name, frames });
            }
        }
    }
    items.sort_by(|a, b| b.name.cmp(&a.name));
    items.truncate(200);
    RecordListing { root: root.display().to_string(), items }
}
