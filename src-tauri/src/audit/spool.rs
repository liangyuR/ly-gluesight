use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::Event;

const MAX_EVENTS: usize = 4096;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EVENT_BYTES: u64 = 4 * 1024 * 1024;
const PROBE_TEMPORARY: &str = ".health-create.tmp";
const PROBE_COMMITTED: &str = ".health-commit";

struct Receipt {
    cycle: String,
    bytes: u64,
}

pub(super) struct Spool {
    root: PathBuf,
    receipts: BTreeMap<u64, Receipt>,
    interrupted: BTreeMap<u64, Event>,
    interrupted_bytes: u64,
    next: u64,
    bytes: u64,
    limit: usize,
    byte_limit: u64,
}

fn plain(path: &Path, directory: bool) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|error| format!("审计路径 {} 不可用：{error}", path.display()))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 { return Err("审计路径不能包含 reparse point".into()); }
    }
    if meta.file_type().is_symlink() || (directory && !meta.is_dir()) || (!directory && !meta.is_file()) {
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

fn validate_probe(path: &Path) -> Result<(), String> {
    let mut content = Vec::new();
    std::fs::File::open(path).map_err(|error| error.to_string())?.take(2).read_to_end(&mut content).map_err(|error| error.to_string())?;
    if content.is_empty() || content == [0] { Ok(()) }
    else { Err("审计健康探针含未知数据，保留并拒绝就绪".into()) }
}

impl Spool {
    pub(super) fn open(root: PathBuf) -> Result<Self, String> {
        Self::with_limits(root, MAX_EVENTS, MAX_BYTES)
    }

    pub(super) fn with_limits(root: PathBuf, limit: usize, byte_limit: u64) -> Result<Self, String> {
        if limit == 0 || byte_limit == 0 { return Err("审计 spool 容量必须大于零".into()); }
        ancestors(&root)?;
        std::fs::create_dir_all(&root).map_err(|error| format!("创建审计 spool 失败：{error}"))?;
        ancestors(&root)?;
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        let mut spool = Self { root, receipts: BTreeMap::new(), interrupted: BTreeMap::new(), interrupted_bytes: 0, next: 1, bytes: 0, limit, byte_limit };
        for item in std::fs::read_dir(&spool.root).map_err(|error| error.to_string())? {
            let path = item.map_err(|error| error.to_string())?.path();
            let name = path.file_name().and_then(|name| name.to_str()).ok_or("审计 spool 文件名无效")?;
            if name == ".interrupted-preinsert" {
                plain(&path, true)?;
                for item in std::fs::read_dir(&path).map_err(|error| error.to_string())? {
                    let path = item.map_err(|error| error.to_string())?.path();
                    let name = path.file_name().and_then(|name| name.to_str()).ok_or("中断录制归档文件名无效")?;
                    let sequence = name.strip_suffix(".json").filter(|name| name.len() == 20 && name.bytes().all(|byte| byte.is_ascii_digit()))
                        .ok_or("中断录制归档含未知文件，保留并拒绝就绪")?.parse::<u64>().map_err(|error| error.to_string())?;
                    let event = spool.read_path(&path)?;
                    if !matches!(&event, Event::Recording(_)) { return Err("中断录制归档含非录制事件".into()); }
                    spool.interrupted_bytes = spool.interrupted_bytes.saturating_add(path.metadata().map_err(|error| error.to_string())?.len());
                    if spool.interrupted.len() >= MAX_EVENTS || spool.interrupted_bytes > MAX_BYTES { return Err("中断录制归档超过容量，保留并拒绝就绪".into()); }
                    spool.interrupted.insert(sequence, event);
                }
                continue;
            }
            plain(&path, false)?;
            if matches!(name, ".health" | PROBE_TEMPORARY | PROBE_COMMITTED) { validate_probe(&path)?; continue; }
            let sequence = name.strip_suffix(".json").filter(|name| name.len() == 20 && name.bytes().all(|byte| byte.is_ascii_digit()))
                .ok_or("审计 spool 含未知或未完成文件，保留证据并拒绝就绪")?.parse::<u64>().map_err(|error| error.to_string())?;
            let bytes = path.metadata().map_err(|error| error.to_string())?.len();
            if spool.receipts.len() >= limit || bytes > MAX_EVENT_BYTES || spool.bytes.saturating_add(bytes) > byte_limit {
                return Err("审计 spool 超过容量，保留全部文件并拒绝就绪".into());
            }
            let event = spool.read_file(sequence)?;
            spool.bytes += bytes;
            spool.next = spool.next.max(sequence.checked_add(1).ok_or("审计 spool 序号已耗尽")?);
            spool.receipts.insert(sequence, Receipt { cycle: event.cycle_id().into(), bytes });
        }
        spool.probe()?;
        Ok(spool)
    }

    pub(super) fn probe(&self) -> Result<(), String> {
        ancestors(&self.root)?;
        let path = self.root.join(".health");
        if path.try_exists().map_err(|error| error.to_string())? {
            plain(&path, false)?;
            validate_probe(&path)?;
        }
        let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(path).map_err(|error| format!("审计写入健康检查失败：{error}"))?;
        file.write_all(&[0]).and_then(|_| file.set_len(0)).and_then(|_| file.sync_all()).map_err(|error| format!("审计持久写入健康检查失败：{error}"))?;
        drop(file);
        for name in [PROBE_TEMPORARY, PROBE_COMMITTED] {
            let marker = self.root.join(name);
            match std::fs::symlink_metadata(&marker) {
                Ok(_) => {
                    plain(&marker, false)?;
                    validate_probe(&marker)?;
                    std::fs::remove_file(&marker).map_err(|error| format!("恢复中断审计探针失败，保留探针：{error}"))?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(format!("审计探针路径不可用：{error}")),
            }
        }
        let temporary = self.root.join(PROBE_TEMPORARY);
        let committed = self.root.join(PROBE_COMMITTED);
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&temporary)
            .map_err(|error| format!("审计探针新建失败：{error}"))?;
        file.write_all(&[0]).and_then(|_| file.sync_all()).map_err(|error| format!("审计探针持久写入失败，保留探针：{error}"))?;
        drop(file);
        std::fs::rename(&temporary, &committed).map_err(|error| format!("审计探针重命名失败，保留探针：{error}"))?;
        std::fs::remove_file(&committed).map_err(|error| format!("审计探针清理失败，保留探针：{error}"))
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
        if content.len() as u64 > MAX_EVENT_BYTES { return Err("审计事件读取超过大小限制，保留并拒绝就绪".into()); }
        let event = serde_json::from_slice::<Event>(&content)
            .map_err(|error| format!("审计 spool {} 无法解析，保留原文件：{error}", path.display()))?;
        validate(&event)?;
        Ok(event)
    }

    pub(super) fn append(&mut self, event: &Event) -> Result<(), String> {
        validate(event)?;
        let encoded = serde_json::to_vec(event).map_err(|error| error.to_string())?;
        if encoded.len() as u64 > MAX_EVENT_BYTES || self.receipts.len() >= self.limit || self.bytes.saturating_add(encoded.len() as u64) > self.byte_limit {
            return Err("审计 spool 容量不足，未丢弃任何待写证据，禁止接收下一件".into());
        }
        ancestors(&self.root)?;
        let sequence = self.next;
        self.next = self.next.checked_add(1).ok_or("审计 spool 序号已耗尽")?;
        let path = self.path(sequence);
        if path.try_exists().map_err(|error| error.to_string())? { return Err("审计事件序号已存在，拒绝覆盖证据".into()); }
        let temporary = path.with_extension("tmp");
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&temporary)
            .map_err(|error| format!("创建审计暂存失败：{error}"))?;
        file.write_all(&encoded).and_then(|_| file.sync_all()).map_err(|error| format!("审计暂存持久写入失败，保留暂存：{error}"))?;
        drop(file);
        std::fs::rename(&temporary, &path).map_err(|error| format!("提交审计事件失败，保留暂存：{error}"))?;
        self.bytes += encoded.len() as u64;
        self.receipts.insert(sequence, Receipt { cycle: event.cycle_id().into(), bytes: encoded.len() as u64 });
        Ok(())
    }

    pub(super) fn archive_preinsert(&mut self, receipts: &[u64]) -> Result<(), String> {
        let root = self.root.join(".interrupted-preinsert");
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

    pub(super) fn protected_directories(&self, root: &Path) -> Result<Vec<PathBuf>, String> {
        if self.empty() && self.interrupted.is_empty() { return Ok(Vec::new()); }
        ancestors(root)?;
        let checked_root = root.canonicalize().map_err(|error| format!("录制根目录不可用，暂停清理：{error}"))?;
        let mut paths = BTreeSet::new();
        let mut cycles = self.cycles();
        cycles.extend(self.interrupted.values().map(|event| event.cycle_id().to_string()));
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
        let mut events = self.interrupted.values().cloned().collect::<Vec<_>>();
        for cycle in self.cycles() { events.extend(self.events(&cycle)?.into_iter().map(|(_, event)| event)); }
        for event in events {
                if let Event::Recording(outcome) = event {
                    let mut candidates = Vec::new();
                    if let Some(directory) = outcome.directory { candidates.push(directory); }
                    for file in outcome.files {
                        let path = root.join(file.file);
                        if let Some(parent) = path.parent() { candidates.push(parent.to_path_buf()); }
                    }
                    for path in candidates {
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
