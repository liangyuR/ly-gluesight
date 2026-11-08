use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::Event;

const MAX_EVENTS: usize = 4096;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EVENT_BYTES: u64 = 4 * 1024 * 1024;
const PROBE_TEMPORARY: &str = ".health-create.tmp";
const PROBE_COMMITTED: &str = ".health-commit";
/// spool 文件格式。2：文件为 {"format":2,"event":…}，测点位置可空（无位置基准）、新增超出检测范围、帧去掉 locateFailed。
/// 3：帧的各视图结果加特征定位位姿（viewResults[].pose）。
/// 4：检测记录加段配方的帧与区域记录（burst），原图引用加段配方帧身份（files[].frame）。
/// 其他版本或没有 format 的是旧软件留下的文件：不兼容（D-0），移到未恢复目录留待人工核对，不猜测迁移。
const FORMAT: u32 = 4;
/// 恢复不了的件和读不了的文件：原样移到这里留待人工核对，不阻止启动与接件。
const UNRECOVERED: &str = ".unrecovered";
const INTERRUPTED: &str = ".interrupted-preinsert";

#[derive(serde::Serialize)]
struct Envelope<'a> {
    format: u32,
    event: &'a Event,
}

#[derive(serde::Deserialize)]
struct Header {
    format: Option<u32>,
}

#[derive(serde::Deserialize)]
struct Stored {
    event: Event,
}

struct Receipt {
    cycle: String,
    bytes: u64,
}

pub(super) struct Spool {
    root: PathBuf,
    receipts: BTreeMap<u64, Receipt>,
    interrupted: BTreeMap<u64, Event>,
    interrupted_bytes: u64,
    /// 已隔离到 .unrecovered 的事件：人工核对前仍保护其引用的原图目录
    quarantined: Vec<Event>,
    next: u64,
    bytes: u64,
    limit: usize,
    byte_limit: u64,
}

fn entry(path: &Path) -> Result<std::fs::Metadata, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|error| format!("审计路径 {} 不可用：{error}", path.display()))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 { return Err("审计路径不能包含 reparse point".into()); }
    }
    if meta.file_type().is_symlink() { return Err(format!("审计路径 {} 不能是链接", path.display())); }
    Ok(meta)
}

fn plain(path: &Path, directory: bool) -> Result<(), String> {
    let meta = entry(path)?;
    if (directory && !meta.is_dir()) || (!directory && !meta.is_file()) {
        return Err(format!("审计路径 {} 不是普通{}", path.display(), if directory { "目录" } else { "文件" }));
    }
    Ok(())
}

fn ancestors(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => plain(ancestor, true)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(format!("审计路径祖先不可用：{error}")),
        }
    }
    Ok(())
}

pub(super) fn validate(event: &Event) -> Result<(), String> {
    let cycle = event.cycle_id();
    if cycle.is_empty() || cycle.len() > 128 { return Err("审计事件 cycleId 长度必须为 1–128".into()); }
    if let Event::Recording(outcome) = event {
        for file in &outcome.files { super::raw_file(file.clone())?; }
    }
    Ok(())
}

fn sequence(name: &str, suffix: &str) -> Option<u64> {
    name.strip_suffix(suffix).filter(|name| name.len() == 20 && name.bytes().all(|byte| byte.is_ascii_digit()))?.parse().ok()
}

/// 探针残留只应为空或单个零字节；其他内容是中断探针留下的，删掉即可。
fn probe_leftover(path: &Path, meta: &std::fs::Metadata) -> Option<String> {
    let mut content = Vec::new();
    let read = std::fs::File::open(path).and_then(|file| file.take(2).read_to_end(&mut content));
    if meta.len() <= 1 && read.is_ok() && (content.is_empty() || content == [0]) { return None; }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    Some(match std::fs::remove_file(path) {
        Ok(()) => format!("审计健康探针 {name} 含残留数据，已删除"),
        Err(error) => format!("审计健康探针 {name} 含残留数据，删除失败（{error}），健康检查会重试"),
    })
}

fn read_names(path: &Path) -> Result<Vec<std::ffi::OsString>, String> {
    let mut names = std::fs::read_dir(path).map_err(|error| format!("读取审计目录 {} 失败：{error}", path.display()))?
        .map(|item| item.map(|item| item.file_name()).map_err(|error| format!("读取审计目录 {} 失败：{error}", path.display())))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

impl Spool {
    /// 只有 spool 目录本身不可用（建不了、读不了、路径含链接）才失败；读不了的文件移到未恢复目录，返回给人看的说明。
    pub(super) fn open(root: PathBuf) -> Result<(Self, Vec<String>), String> {
        Self::with_limits(root, MAX_EVENTS, MAX_BYTES)
    }

    pub(super) fn with_limits(root: PathBuf, limit: usize, byte_limit: u64) -> Result<(Self, Vec<String>), String> {
        if limit == 0 || byte_limit == 0 { return Err("审计 spool 容量必须大于零".into()); }
        ancestors(&root)?;
        std::fs::create_dir_all(&root).map_err(|error| format!("创建审计 spool 失败：{error}"))?;
        ancestors(&root)?;
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        let mut spool = Self {
            root,
            receipts: BTreeMap::new(),
            interrupted: BTreeMap::new(),
            interrupted_bytes: 0,
            quarantined: Vec::new(),
            next: 1,
            bytes: 0,
            limit,
            byte_limit,
        };
        let mut notes = Vec::new();
        for name in read_names(&spool.root)? {
            let path = spool.root.join(&name);
            let meta = entry(&path)?;
            let Some(name) = name.to_str() else {
                spool.set_aside(&path, &name.to_string_lossy(), "文件名不是有效 UTF-8", &mut notes);
                continue;
            };
            if let Some(next) = sequence(name, ".json").or_else(|| sequence(name, ".tmp")).and_then(|sequence| sequence.checked_add(1)) {
                spool.next = spool.next.max(next);
            }
            match name {
                UNRECOVERED if meta.is_dir() => {
                    spool.load_quarantined(&path);
                    continue;
                }
                INTERRUPTED if meta.is_dir() => { spool.load_interrupted(&path, &mut notes)?; continue; }
                ".health" | PROBE_TEMPORARY | PROBE_COMMITTED if meta.is_file() => { notes.extend(probe_leftover(&path, &meta)); continue; }
                _ => (),
            }
            let Some(sequence) = sequence(name, ".json").filter(|_| meta.is_file()) else {
                spool.set_aside(&path, name, "未知或未完成的文件（如写入中断留下的暂存）", &mut notes);
                continue;
            };
            let bytes = meta.len();
            if spool.receipts.len() >= limit || bytes > MAX_EVENT_BYTES || spool.bytes.saturating_add(bytes) > byte_limit {
                spool.set_aside(&path, name, "超过 spool 容量", &mut notes);
                continue;
            }
            match spool.read_path(&path) {
                Ok(event) => {
                    spool.bytes += bytes;
                    spool.receipts.insert(sequence, Receipt { cycle: event.cycle_id().into(), bytes });
                }
                Err(error) => spool.set_aside(&path, name, &error, &mut notes),
            }
        }
        Ok((spool, notes))
    }

    fn load_interrupted(&mut self, directory: &Path, notes: &mut Vec<String>) -> Result<(), String> {
        for name in read_names(directory)? {
            let path = directory.join(&name);
            let meta = entry(&path)?;
            let label = format!("{INTERRUPTED}-{}", name.to_string_lossy());
            let Some(sequence) = name.to_str().and_then(|name| sequence(name, ".json")).filter(|_| meta.is_file()) else {
                self.set_aside(&path, &label, "中断录制归档含未知文件", notes);
                continue;
            };
            if self.interrupted.len() >= MAX_EVENTS || self.interrupted_bytes.saturating_add(meta.len()) > MAX_BYTES {
                self.set_aside(&path, &label, "中断录制归档超过容量", notes);
                continue;
            }
            match self.read_path(&path) {
                Ok(event @ Event::Recording(_)) => {
                    self.interrupted_bytes += meta.len();
                    self.interrupted.insert(sequence, event);
                }
                Ok(_) => self.set_aside(&path, &label, "中断录制归档含非录制事件", notes),
                Err(error) => self.set_aside(&path, &label, &error, notes),
            }
        }
        Ok(())
    }

    fn set_aside(&self, path: &Path, name: &str, reason: &str, notes: &mut Vec<String>) {
        notes.push(match self.move_aside(path, name) {
            Ok(target) => format!("审计 spool 文件 {name}：{reason}；已移到 {}，待人工核对", target.display()),
            Err(error) => format!("审计 spool 文件 {name}：{reason}；移到未恢复目录失败（{error}），本次不读取"),
        });
    }

    /// 原样移到未恢复目录，保留原文件名；重名时加序号前缀。
    fn move_aside(&self, path: &Path, name: &str) -> Result<PathBuf, String> {
        let root = self.root.join(UNRECOVERED);
        ancestors(&root)?;
        std::fs::create_dir_all(&root).map_err(|error| format!("创建未恢复追溯目录失败：{error}"))?;
        ancestors(&root)?;
        let target = std::iter::once(root.join(name)).chain((1u32..10_000).map(|n| root.join(format!("{n}-{name}"))))
            .find(|path| std::fs::symlink_metadata(path).is_err()).ok_or("未恢复追溯文件名耗尽")?;
        std::fs::rename(path, &target).map_err(|error| format!("移出 {} 失败：{error}", path.display()))?;
        Ok(target)
    }

    pub(super) fn probe(&self) -> Result<(), String> {
        ancestors(&self.root)?;
        let path = self.root.join(".health");
        if path.try_exists().map_err(|error| error.to_string())? { plain(&path, false)?; }
        let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(path).map_err(|error| format!("审计写入健康检查失败：{error}"))?;
        file.write_all(&[0]).and_then(|_| file.set_len(0)).and_then(|_| file.sync_all()).map_err(|error| format!("审计持久写入健康检查失败：{error}"))?;
        drop(file);
        for name in [PROBE_TEMPORARY, PROBE_COMMITTED] {
            let marker = self.root.join(name);
            match std::fs::symlink_metadata(&marker) {
                Ok(_) => {
                    plain(&marker, false)?;
                    std::fs::remove_file(&marker).map_err(|error| format!("清理中断的审计探针失败：{error}"))?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(format!("审计探针路径不可用：{error}")),
            }
        }
        let temporary = self.root.join(PROBE_TEMPORARY);
        let committed = self.root.join(PROBE_COMMITTED);
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&temporary)
            .map_err(|error| format!("审计探针新建失败：{error}"))?;
        file.write_all(&[0]).and_then(|_| file.sync_all()).map_err(|error| format!("审计探针持久写入失败：{error}"))?;
        drop(file);
        std::fs::rename(&temporary, &committed).map_err(|error| format!("审计探针重命名失败：{error}"))?;
        std::fs::remove_file(&committed).map_err(|error| format!("审计探针清理失败：{error}"))
    }

    fn path(&self, sequence: u64) -> PathBuf { self.root.join(format!("{sequence:020}.json")) }

    fn read_file(&self, sequence: u64) -> Result<Event, String> {
        ancestors(&self.root)?;
        self.read_path(&self.path(sequence))
    }

    fn read_path(&self, path: &Path) -> Result<Event, String> {
        ancestors(path.parent().ok_or("审计文件缺少父目录")?)?;
        plain(path, false)?;
        let bytes = path.metadata().map_err(|error| error.to_string())?.len();
        if bytes > MAX_EVENT_BYTES { return Err("审计事件超过文件大小限制".into()); }
        let mut content = Vec::new();
        std::fs::File::open(&path).map_err(|error| error.to_string())?.take(MAX_EVENT_BYTES + 1)
            .read_to_end(&mut content).map_err(|error| error.to_string())?;
        if content.len() as u64 > MAX_EVENT_BYTES { return Err("审计事件读取超过大小限制".into()); }
        let unreadable = |error: serde_json::Error| format!("审计 spool {} 无法解析：{error}", path.display());
        let header = serde_json::from_slice::<Header>(&content).map_err(unreadable)?;
        if header.format != Some(FORMAT) {
            return Err(format!(
                "审计 spool {} 是旧版本软件留下的格式（format {}），当前版本不读取也不迁移",
                path.display(), header.format.map_or("缺失".into(), |format| format.to_string())));
        }
        let event = serde_json::from_slice::<Stored>(&content).map_err(unreadable)?.event;
        validate(&event)?;
        Ok(event)
    }

    pub(super) fn append(&mut self, event: &Event) -> Result<(), String> {
        validate(event)?;
        let encoded = serde_json::to_vec(&Envelope { format: FORMAT, event }).map_err(|error| error.to_string())?;
        if encoded.len() as u64 > MAX_EVENT_BYTES || self.receipts.len() >= self.limit || self.bytes.saturating_add(encoded.len() as u64) > self.byte_limit {
            return Err("审计 spool 容量已满".into());
        }
        ancestors(&self.root)?;
        let sequence = self.next;
        self.next = self.next.checked_add(1).ok_or("审计 spool 序号已耗尽")?;
        let path = self.path(sequence);
        if path.try_exists().map_err(|error| error.to_string())? { return Err("审计事件序号已存在，拒绝覆盖证据".into()); }
        let temporary = path.with_extension("tmp");
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&temporary)
            .map_err(|error| format!("创建审计暂存失败：{error}"))?;
        let written = file.write_all(&encoded).and_then(|_| file.sync_all()).map_err(|error| format!("审计暂存持久写入失败：{error}"));
        drop(file);
        if let Err(error) = written.and_then(|()| std::fs::rename(&temporary, &path).map_err(|error| format!("提交审计事件失败：{error}"))) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        self.bytes += encoded.len() as u64;
        self.receipts.insert(sequence, Receipt { cycle: event.cycle_id().into(), bytes: encoded.len() as u64 });
        Ok(())
    }

    pub(super) fn archive_preinsert(&mut self, receipts: &[u64]) -> Result<(), String> {
        let root = self.root.join(INTERRUPTED);
        ancestors(&root)?;
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        ancestors(&root)?;
        let mut next = self.interrupted.last_key_value().map_or(Ok(1), |(&id, _)| id.checked_add(1).ok_or("中断录制归档序号耗尽"))?;
        for &receipt in receipts {
            if !self.receipts.contains_key(&receipt) { return Err("中断录制收据不存在".into()); }
            let event = self.read_file(receipt)?;
            if !matches!(&event, Event::Recording(_)) { return Err("仅允许归档未生成主记录的录制事件".into()); }
            let source = self.path(receipt);
            let mut content = Vec::new();
            std::fs::File::open(&source).map_err(|error| error.to_string())?.take(MAX_EVENT_BYTES + 1).read_to_end(&mut content).map_err(|error| error.to_string())?;
            if content.len() as u64 > MAX_EVENT_BYTES || self.interrupted.len() >= MAX_EVENTS || self.interrupted_bytes.saturating_add(content.len() as u64) > MAX_BYTES { return Err("中断录制归档容量不足，保留原始 spool".into()); }
            let target = root.join(format!("{next:020}.json"));
            let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(target).map_err(|error| error.to_string())?;
            file.write_all(&content).and_then(|_| file.sync_all()).map_err(|error| error.to_string())?;
            drop(file);
            self.interrupted.insert(next, event);
            self.interrupted_bytes += content.len() as u64;
            next = next.checked_add(1).ok_or("中断录制归档序号耗尽")?;
            self.remove(&[receipt])?;
        }
        Ok(())
    }

    /// 把一件的全部待写事件原样移到未恢复目录，返回存放目录。
    pub(super) fn quarantine(&mut self, cycle: &str) -> Result<PathBuf, String> {
        let sequences = self.receipts.iter().filter(|(_, receipt)| receipt.cycle == cycle).map(|(&sequence, _)| sequence).collect::<Vec<_>>();
        for sequence in sequences {
            let source = self.path(sequence);
            plain(&source, false)?;
            let event = self.read_file(sequence).ok();
            self.move_aside(&source, &format!("{sequence:020}.json"))?;
            self.quarantined.extend(event);
            let receipt = self.receipts.remove(&sequence).unwrap();
            self.bytes -= receipt.bytes;
        }
        Ok(self.root.join(UNRECOVERED))
    }

    fn load_quarantined(&mut self, directory: &Path) {
        let Ok(items) = std::fs::read_dir(directory) else {
            return;
        };
        for item in items.flatten() {
            if let Ok(event) = self.read_path(&item.path()) {
                self.quarantined.push(event);
            }
        }
    }

    pub(super) fn empty(&self) -> bool { self.receipts.is_empty() }

    pub(super) fn cycles(&self) -> BTreeSet<String> { self.receipts.values().map(|receipt| receipt.cycle.clone()).collect() }

    pub(super) fn events(&self, cycle: &str) -> Result<Vec<(u64, Event)>, String> {
        self.receipts.iter().filter(|(_, receipt)| receipt.cycle == cycle).map(|(&sequence, _)| {
            let event = self.read_file(sequence)?;
            if event.cycle_id() != cycle { return Err("审计 spool 文件 cycleId 与接收记录不一致".into()); }
            Ok((sequence, event))
        }).collect()
    }

    pub(super) fn remove(&mut self, sequences: &[u64]) -> Result<(), String> {
        ancestors(&self.root)?;
        for &sequence in sequences {
            if !self.receipts.contains_key(&sequence) { return Err("审计 spool 收据不存在".into()); }
            let path = self.path(sequence);
            plain(&path, false)?;
            std::fs::remove_file(&path).map_err(|error| format!("已入库审计事件暂未移除，将幂等重试：{error}"))?;
            let receipt = self.receipts.remove(&sequence).unwrap();
            self.bytes -= receipt.bytes;
        }
        Ok(())
    }

    /// backlog：还在内存暂存、未落盘的事件，同样受保护。
    pub(super) fn protected_directories(&self, root: &Path, backlog: &VecDeque<Event>) -> Result<Vec<PathBuf>, String> {
        if self.empty()
            && self.interrupted.is_empty()
            && backlog.is_empty()
            && self.quarantined.is_empty()
        {
            return Ok(Vec::new());
        }
        ancestors(root)?;
        let checked_root = root.canonicalize().map_err(|error| format!("录制根目录不可用，暂停清理：{error}"))?;
        let mut paths = BTreeSet::new();
        let mut cycles = self.cycles();
        cycles.extend(
            self.interrupted
                .values()
                .chain(backlog)
                .chain(&self.quarantined)
                .map(|event| event.cycle_id().to_string()),
        );
        let mut scanned = 0usize;
        for item in std::fs::read_dir(root).map_err(|error| error.to_string())? {
            scanned += 1;
            if scanned > MAX_EVENTS { return Err("录制保护扫描达到上限，暂停清理".into()); }
            let path = item.map_err(|error| error.to_string())?.path();
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            if name != "_pending" && !(name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_digit())) { continue; }
            plain(&path, true)?;
            for child in std::fs::read_dir(&path).map_err(|error| error.to_string())? {
                scanned += 1;
                if scanned > MAX_EVENTS { return Err("录制保护扫描达到上限，暂停清理".into()); }
                let child = child.map_err(|error| error.to_string())?.path();
                let name = child.file_name().and_then(|name| name.to_str()).unwrap_or_default();
                if cycles.iter().any(|cycle| name.ends_with(&format!("_cycle_{cycle}"))) {
                    plain(&child, true)?;
                    let checked = child.canonicalize().map_err(|error| error.to_string())?;
                    if !checked.starts_with(&checked_root) { return Err("待入库录制目录越界，暂停清理".into()); }
                    paths.insert(checked);
                }
            }
        }
        let mut events = self
            .interrupted
            .values()
            .chain(backlog)
            .filter(|event| matches!(event, Event::Recording(_)))
            .cloned()
            .map(|event| (event, false))
            .collect::<Vec<_>>();
        events.extend(self.quarantined.iter().cloned().map(|event| (event, true)));
        for cycle in self.cycles() {
            events.extend(
                self.events(&cycle)?
                    .into_iter()
                    .map(|(_, event)| (event, false)),
            );
        }
        for (event, quarantined) in events {
                if let Event::Recording(outcome) = event {
                    if outcome.directory.as_deref().is_some_and(|directory| !directory.starts_with(root)) { continue; }
                    let mut candidates = Vec::new();
                    if let Some(directory) = outcome.directory { candidates.push(directory); }
                    for file in outcome.files {
                        let path = root.join(file.file);
                        if let Some(parent) = path.parent() { candidates.push(parent.to_path_buf()); }
                    }
                    for path in candidates {
                        if quarantined && !path.exists() {
                            continue;
                        }
                        ancestors(&path)?;
                        let checked = path.canonicalize().map_err(|error| format!("待入库录制目录不可核对，暂停清理：{error}"))?;
                        if !checked.starts_with(&checked_root) || checked == checked_root { return Err("待入库录制引用越过 records 子目录，暂停清理".into()); }
                        paths.insert(checked);
                    }
                }
        }
        Ok(paths.into_iter().collect())
    }
}
