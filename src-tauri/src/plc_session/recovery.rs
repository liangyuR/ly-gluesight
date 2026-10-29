use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};

use serde::Deserialize;

use super::{validate_pending, Pending, PlcSession, Request, ResultEnvelope};

const ERROR_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AckReceipt {
    pub cycle_id: String,
    pub sn: String,
    pub request_seq: u32,
    pub ts: i64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct AckRecovery {
    pub receipts: Vec<AckReceipt>,
    pub errors: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditRecord {
    version: u8,
    ts: i64,
    action: String,
    last_request_seq: u32,
    pending: Option<Pending>,
}

struct Candidate {
    receipt: AckReceipt,
    request: Request,
    result: ResultEnvelope,
}

struct Recovery<'a> {
    unresolved: &'a BTreeSet<String>,
    receipts: BTreeMap<String, Option<Candidate>>,
    errors: Vec<String>,
    error_count: usize,
}

impl<'a> Recovery<'a> {
    fn new(unresolved: &'a BTreeSet<String>) -> Self {
        Self { unresolved, receipts: BTreeMap::new(), errors: Vec::new(), error_count: 0 }
    }

    fn error(&mut self, error: String) {
        self.error_count = self.error_count.saturating_add(1);
        if self.errors.len() < ERROR_LIMIT { self.errors.push(error); }
    }

    fn pending(&mut self, pending: &Pending, last_request_seq: u32, ts: i64, source: &str) {
        if !pending.cycle_id.as_ref().is_some_and(|cycle| self.unresolved.contains(cycle)) { return; }
        // 与启动时归档握手日志同样处理：旧协议（升级前的协议 5）的事务按 D-0 不再用于恢复，也不当作日志错误
        if pending.request.protocol_version != crate::handshake::PROTOCOL_VERSION { return; }
        if let Err(error) = validate_pending(pending, last_request_seq) {
            self.error(format!("{source}：{error}"));
            return;
        }
        if !pending.acknowledged { return; }
        let Some(cycle_id) = pending.cycle_id.as_deref() else { return };
        let result = pending.result.as_ref().unwrap();
        let candidate = Candidate {
            receipt: AckReceipt { cycle_id: cycle_id.into(), sn: result.sn.clone(), request_seq: result.request_seq, ts },
            request: pending.request.clone(), result: result.clone(),
        };
        let mut conflict = false;
        match self.receipts.entry(cycle_id.into()) {
            Entry::Vacant(entry) => { entry.insert(Some(candidate)); }
            Entry::Occupied(mut entry) => {
                if entry.get().as_ref().is_some_and(|previous|
                    previous.request != candidate.request || previous.result != candidate.result) {
                    entry.insert(None);
                    conflict = true;
                } else if let Some(previous) = entry.get_mut() {
                    previous.receipt.ts = previous.receipt.ts.max(ts);
                }
            },
        }
        if conflict { self.error(format!("{source}：cycleId={cycle_id} 的 ACK 请求或结果身份冲突，拒绝恢复该工件")); }
    }

    fn finish(mut self) -> AckRecovery {
        if self.error_count > self.errors.len() {
            self.errors.push(format!("ACK 恢复共发现 {} 条诊断，另 {} 条已省略", self.error_count, self.error_count - self.errors.len()));
        }
        AckRecovery { receipts: self.receipts.into_values().filter_map(|candidate| candidate.map(|candidate| candidate.receipt)).collect(),
            errors: self.errors }
    }
}

pub(super) fn read(session: &PlcSession, unresolved: &BTreeSet<String>) -> AckRecovery {
    let mut recovery = Recovery::new(unresolved);
    if let Some(error) = &session.load_error {
        recovery.error(format!("当前握手日志不能恢复 ACK：{error}"));
    } else {
        let durable = PlcSession::open(session.path.clone());
        if let Some(error) = durable.load_error {
            recovery.error(format!("当前握手日志不能恢复 ACK：{error}"));
        } else if let Some(pending) = &durable.journal.pending {
            recovery.pending(pending, durable.journal.last_request_seq, pending.started_at.max(0), "当前握手日志");
        }
    }
    if unresolved.is_empty() { return recovery.finish(); }
    for path in crate::handshake_audit::files(&session.path.with_extension("audit.jsonl")) {
        read_audit(&mut recovery, &path);
    }
    recovery.finish()
}

fn read_audit(recovery: &mut Recovery, path: &std::path::Path) {
    match std::fs::File::open(path) {
        Ok(file) => {
            let mut reader = BufReader::new(file);
            let mut bytes = Vec::new();
            let mut line_number = 0usize;
            loop {
                bytes.clear();
                let count = match reader.read_until(b'\n', &mut bytes) {
                    Ok(count) => count,
                    Err(error) => {
                        recovery.error(format!("读取 ACK 审计 {} 第 {} 行失败：{error}", path.display(), line_number + 1));
                        break;
                    }
                };
                if count == 0 { break; }
                line_number += 1;
                let source = format!("ACK 审计 {} 第 {line_number} 行", path.display());
                if bytes.last() != Some(&b'\n') {
                    recovery.error(format!("{source} 未完整换行提交，忽略末尾半写记录"));
                    break;
                }
                let line = if line_number == 1 { bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes) } else { &bytes };
                if line.iter().all(u8::is_ascii_whitespace) { continue; }
                let record: AuditRecord = match serde_json::from_slice(line) {
                    Ok(record) => record,
                    Err(error) => {
                        recovery.error(format!("{source} 损坏：{error}"));
                        continue;
                    }
                };
                if record.version != 1 {
                    recovery.error(format!("{source} 版本 {} 不受支持", record.version));
                    continue;
                }
                if record.ts <= 0 || record.action.is_empty() {
                    recovery.error(format!("{source} 时间戳或审计动作无效"));
                    continue;
                }
                if let Some(pending) = record.pending {
                    recovery.pending(&pending, record.last_request_seq, record.ts, &source);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => recovery.error(format!("无法读取 ACK 审计 {}：{error}", path.display())),
    }
}
