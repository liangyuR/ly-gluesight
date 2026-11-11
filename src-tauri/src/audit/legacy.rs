//! 旧版本留下的追溯暂存 `history/audit-spool/`（格式 4）：启动时按件补录进记录库，补录完的文件删除；
//! 补不进的原地保留，原图继续保护，待人工核对。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{apply_legacy, Event, Notice, Replay, Sink};

const FORMAT: u64 = 4;
const SUBDIRECTORIES: [&str; 2] = [".unrecovered", ".interrupted-preinsert"];
const PROBES: [&str; 3] = [".health", ".health-create.tmp", ".health-commit"];
/// 原图保护扫描的目录项上限，超过就暂停清理。
const SCAN_LIMIT: usize = 4096;

#[derive(Default)]
pub(super) struct Leftovers {
    pub(super) cycles: BTreeSet<String>,
    pub(super) directories: Vec<PathBuf>,
    /// 旧暂存目录读不了：不知道哪些原图还没入库，原图清理暂停到修复后重启
    pub(super) unreadable: Option<String>,
}

enum Read {
    Event(Box<Event>),
    Retired,
}

fn read(path: &Path) -> Result<Read, String> {
    let content = std::fs::read(path).map_err(|error| error.to_string())?;
    let mut value: serde_json::Value =
        serde_json::from_slice(&content).map_err(|error| format!("无法解析：{error}"))?;
    match value.get("format").and_then(serde_json::Value::as_u64) {
        Some(FORMAT) => (),
        format => {
            return Err(format!(
                "格式 {} 不是 {FORMAT}",
                format.map_or("缺失".into(), |format| format.to_string())
            ))
        }
    }
    if matches!(
        value
            .pointer("/event/Insert/source")
            .and_then(serde_json::Value::as_str),
        Some("offlineSynthetic" | "offlineSample" | "offlineCandidate")
    ) {
        return Ok(Read::Retired);
    }
    serde_json::from_value(value["event"].take())
        .map(|event| Read::Event(Box::new(event)))
        .map_err(|error| format!("无法解析：{error}"))
}

fn files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for directory in
        std::iter::once(root.to_path_buf()).chain(SUBDIRECTORIES.iter().map(|name| root.join(name)))
    {
        let unreadable =
            |error: std::io::Error| format!("读取旧追溯暂存 {} 失败：{error}", directory.display());
        let items = match std::fs::read_dir(&directory) {
            Ok(items) => items,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(unreadable(error)),
        };
        let mut paths = items
            .map(|item| item.map(|item| item.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(unreadable)?;
        paths.sort();
        for path in paths {
            if std::fs::symlink_metadata(&path)
                .map_err(unreadable)?
                .is_file()
            {
                files.push(path);
            }
        }
    }
    Ok(files)
}

/// 返回没补录的件、各件的补录日志，和给人看的未补录汇总。
pub(super) fn import(
    root: &Path,
    sink: &mut impl Sink,
    now: i64,
) -> (Leftovers, Vec<Notice>, Option<String>) {
    let mut leftovers = Leftovers::default();
    let mut notices = Vec::new();
    let mut unreadable = Vec::new();
    let mut cycles = BTreeMap::<String, Vec<(PathBuf, Event)>>::new();
    let files = match files(root) {
        Ok(files) => files,
        Err(error) => {
            let summary = format!("{error}；不知道哪些原图还没入库，原图滚动清理暂停，修复后重启");
            leftovers.unreadable = Some(error);
            return (leftovers, notices, Some(summary));
        }
    };
    for path in files {
        if path.parent() == Some(root)
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| PROBES.contains(&name))
        {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        match read(&path) {
            Ok(Read::Event(event)) => cycles
                .entry(event.cycle_id().to_string())
                .or_default()
                .push((path, *event)),
            Ok(Read::Retired) => {
                let _ = std::fs::remove_file(&path);
            }
            Err(error) => unreadable.push(format!("{}（{error}）", path.display())),
        }
    }
    for (cycle, mut items) in cycles {
        items.sort_by_key(|(_, event)| !matches!(event, Event::Insert(_)));
        let (paths, events): (Vec<_>, Vec<_>) = items.into_iter().unzip();
        let directories = events
            .iter()
            .filter_map(|event| match event {
                Event::Recording(outcome) => outcome.directory.clone(),
                _ => None,
            })
            .collect::<Vec<_>>();
        let (replay, applied) = apply_legacy(events, &cycle, sink, now);
        notices.extend(applied);
        if replay != Replay::Complete {
            leftovers.cycles.insert(cycle);
            leftovers.directories.extend(directories);
            continue;
        }
        let kept = paths
            .iter()
            .filter(|path| std::fs::remove_file(path).is_err())
            .count();
        let message = if kept == 0 {
            format!("{} 条旧暂存事件已补录进记录库", paths.len())
        } else {
            format!(
                "{} 条旧暂存事件已补录进记录库，其中 {kept} 个文件删除失败，下次启动按已入库核对",
                paths.len()
            )
        };
        super::log(&mut notices, "info", "旧追溯暂存补录", &cycle, message);
    }
    for directory in SUBDIRECTORIES
        .iter()
        .map(|name| root.join(name))
        .chain(std::iter::once(root.to_path_buf()))
    {
        let _ = std::fs::remove_dir(directory);
    }
    let mut summary = Vec::new();
    if !leftovers.cycles.is_empty() {
        let cycles = leftovers
            .cycles
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("、");
        summary.push(format!(
            "{} 件没能补录（cycleId {cycles}），原图继续保护",
            leftovers.cycles.len()
        ));
    }
    if !unreadable.is_empty() {
        summary.push(format!(
            "{} 个文件读不了：{}",
            unreadable.len(),
            unreadable.join("；")
        ));
    }
    let summary = (!summary.is_empty()).then(|| {
        format!(
            "旧追溯暂存 {}：{}；文件原地保留，待人工核对",
            root.display(),
            summary.join("；")
        )
    });
    (leftovers, notices, summary)
}

fn plain_directory(path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|error| format!("录制路径 {} 不可用：{error}", path.display()))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(format!("录制路径 {} 不能是 reparse point", path.display()));
        }
    }
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(format!("录制路径 {} 不是普通目录", path.display()));
    }
    Ok(())
}

fn ancestors(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => plain_directory(ancestor)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(format!("录制路径祖先不可用：{error}")),
        }
    }
    Ok(())
}

/// 录制根 `root` 下属于 `cycles` 的录制目录（按目录名 `…_cycle_<cycleId>` 认），加上 `directories` 里在根内的目录。
pub(super) fn protected_directories(
    root: &Path,
    cycles: &BTreeSet<String>,
    directories: &[PathBuf],
) -> Result<Vec<PathBuf>, String> {
    if cycles.is_empty() && directories.is_empty() {
        return Ok(Vec::new());
    }
    ancestors(root)?;
    let checked_root = root
        .canonicalize()
        .map_err(|error| format!("录制根目录不可用，暂停清理：{error}"))?;
    let mut paths = BTreeSet::new();
    let mut scanned = 0usize;
    if !cycles.is_empty() {
        for item in std::fs::read_dir(root).map_err(|error| error.to_string())? {
            scanned += 1;
            if scanned > SCAN_LIMIT {
                return Err("录制保护扫描达到上限，暂停清理".into());
            }
            let path = item.map_err(|error| error.to_string())?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if name != "_pending"
                && !(name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_digit()))
            {
                continue;
            }
            plain_directory(&path)?;
            for child in std::fs::read_dir(&path).map_err(|error| error.to_string())? {
                scanned += 1;
                if scanned > SCAN_LIMIT {
                    return Err("录制保护扫描达到上限，暂停清理".into());
                }
                let child = child.map_err(|error| error.to_string())?.path();
                let name = child
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                if name
                    .split_once("_cycle_")
                    .is_some_and(|(_, cycle)| cycles.contains(cycle))
                {
                    plain_directory(&child)?;
                    let checked = child.canonicalize().map_err(|error| error.to_string())?;
                    if !checked.starts_with(&checked_root) {
                        return Err("待入库录制目录越界，暂停清理".into());
                    }
                    paths.insert(checked);
                }
            }
        }
    }
    for directory in directories
        .iter()
        .filter(|directory| directory.starts_with(root) && directory.exists())
    {
        ancestors(directory)?;
        let checked = directory
            .canonicalize()
            .map_err(|error| format!("待入库录制目录不可核对，暂停清理：{error}"))?;
        if !checked.starts_with(&checked_root) || checked == checked_root {
            return Err("待入库录制引用越过 records 子目录，暂停清理".into());
        }
        paths.insert(checked);
    }
    Ok(paths.into_iter().collect())
}
