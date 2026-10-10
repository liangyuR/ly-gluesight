use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};

use serde::Deserialize;

use super::{validate_pending, Pending, PlcSession, Request, ResultEnvelope};

const ERROR_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AckReceipt {
    pub cycle_id: String,
    pub sn: u32,
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
        if let Err(error) = validate_pending(pending, last_request_seq) {
            self.error(format!("{source}：{error}"));
            return;
        }
        if !pending.acknowledged { return; }
        let Some(cycle_id) = pending.cycle_id.as_deref() else { return };
        let result = pending.result.as_ref().unwrap();
        let candidate = Candidate {
            receipt: AckReceipt { cycle_id: cycle_id.into(), sn: result.sn, request_seq: result.request_seq, ts },
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
    let path = session.path.with_extension("audit.jsonl");
    match std::fs::File::open(&path) {
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
    recovery.finish()
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::{json, Value};

    use super::*;
    use crate::plc_session::{Journal, SessionPhase};

    const CYCLE_A: &str = "11111111111111111111111111111111";
    const CYCLE_B: &str = "22222222222222222222222222222222";

    struct Files { directory: PathBuf, path: PathBuf }

    impl Files {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!("gluesight-ack-recovery-{}-{}-{}", std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(), SEQUENCE.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&directory).unwrap();
            Self { path: directory.join("session.json"), directory }
        }

        fn journal(&self, pending: Option<Pending>) {
            let journal = Journal { version: 1, last_request_seq: pending.as_ref().map_or(0, |pending| pending.request.request_seq),
                pending, last_resolution: None };
            self.write(&serde_json::to_vec(&journal).unwrap());
        }

        fn write(&self, bytes: &[u8]) {
            let mut file = std::fs::File::create(&self.path).unwrap();
            file.write_all(bytes).unwrap();
            file.sync_all().unwrap();
        }

        fn audit_bytes(&self, bytes: &[u8]) {
            let mut file = std::fs::OpenOptions::new().create(true).append(true)
                .open(self.path.with_extension("audit.jsonl")).unwrap();
            file.write_all(bytes).unwrap();
            file.sync_all().unwrap();
        }

        fn audit(&self, record: Value) {
            let mut bytes = serde_json::to_vec(&record).unwrap();
            bytes.push(b'\n');
            self.audit_bytes(&bytes);
        }

        fn open(&self) -> PlcSession { PlcSession::open(self.path.clone()) }
    }

    impl Drop for Files {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.directory); }
    }

    fn pending(cycle_id: &str, request_seq: u32, sn: u32) -> Pending {
        Pending { capture_id: None, request: Request { protocol_version: 1, request_seq, sn, product_code: 1,
            shot_count: 4, plan_version: 7, camera_shots: [2, 1, 1] },
            result: Some(ResultEnvelope { request_seq, sn, result_code: 1, fault_code: 0 }),
            phase: SessionPhase::Releasing, cycle_id: Some(cycle_id.into()), acknowledged: true, started_at: 1000 }
    }

    fn record(pending: &Pending, ts: i64) -> Value {
        json!({"version":1,"ts":ts,"action":"acknowledged","lastRequestSeq":pending.request.request_seq,"pending":pending})
    }

    fn targets() -> BTreeSet<String> {
        [CYCLE_A, CYCLE_B].into_iter().map(str::to_owned).collect()
    }

    fn receipt(cycle_id: &str, request_seq: u32, sn: u32, ts: i64) -> AckReceipt {
        AckReceipt { cycle_id: cycle_id.into(), request_seq, sn, ts }
    }

    #[test]
    fn persisted_ack_is_recovered_after_close_and_reopen_idempotently() {
        let files = Files::new();
        files.journal(Some(pending(CYCLE_A, 7, 50)));
        let session = files.open();
        assert_eq!(session.cycle_id(), Some(CYCLE_A));
        assert_eq!(session.phase(), SessionPhase::ResetRequired);
        assert!(session.acknowledged());
        drop(session);
        let reopened = files.open();
        let recovered = reopened.recover_acknowledgements(&targets());
        assert_eq!(recovered.receipts, vec![receipt(CYCLE_A, 7, 50, 1000)]);
        assert!(recovered.errors.is_empty(), "{:?}", recovered.errors);
        assert_eq!(reopened.recover_acknowledgements(&targets()), recovered);
    }

    #[test]
    fn cycle_binding_is_immutable_and_does_not_write_on_arm_path() {
        let files = Files::new();
        let mut request = pending(CYCLE_A, 7, 50);
        request.cycle_id = None;
        request.acknowledged = false;
        request.result = None;
        request.phase = SessionPhase::Validating;
        files.journal(Some(request));
        let original = std::fs::read(&files.path).unwrap();
        let mut session = files.open();
        assert!(session.bind_cycle_id("bad-id").is_err());
        session.bind_cycle_id(CYCLE_A).unwrap();
        session.bind_cycle_id(CYCLE_A).unwrap();
        assert!(session.bind_cycle_id(CYCLE_B).is_err());
        assert_eq!(session.cycle_id(), Some(CYCLE_A));
        assert_eq!(std::fs::read(&files.path).unwrap(), original);
        files.write(&serde_json::to_vec(&session.journal).unwrap());
        drop(session);
        assert_eq!(files.open().cycle_id(), Some(CYCLE_A));
        files.journal(None);
        assert!(files.open().bind_cycle_id(CYCLE_A).is_err());
    }

    #[test]
    fn released_pending_recovers_from_audit_and_merges_duplicate_receipts() {
        let files = Files::new();
        files.journal(Some(pending(CYCLE_A, 7, 50)));
        let original = files.open();
        drop(original);
        let acknowledged = pending(CYCLE_A, 7, 50);
        files.audit(record(&acknowledged, 2000));
        let mut reset = record(&acknowledged, 3000);
        reset["action"] = json!("resetCompleted");
        files.audit(reset);
        files.journal(None);
        let reopened = files.open();
        assert!(!reopened.pending());
        let recovered = reopened.recover_acknowledgements(&targets());
        assert_eq!(recovered.receipts, vec![receipt(CYCLE_A, 7, 50, 3000)]);
        assert!(recovered.errors.is_empty(), "{:?}", recovered.errors);
        assert_eq!(reopened.recover_acknowledgements(&targets()), recovered);
    }

    #[test]
    fn repeated_sn_and_sequence_never_merge_different_cycles() {
        let files = Files::new();
        let previous = pending(CYCLE_A, 7, 50);
        files.audit(record(&previous, 2000));
        files.journal(Some(pending(CYCLE_B, 7, 50)));
        let recovered = files.open().recover_acknowledgements(&targets());
        assert_eq!(recovered.receipts, vec![receipt(CYCLE_A, 7, 50, 2000), receipt(CYCLE_B, 7, 50, 1000)]);
        assert!(recovered.errors.is_empty(), "{:?}", recovered.errors);
    }

    #[test]
    fn memory_only_ack_and_invalid_current_journal_cannot_supply_receipts() {
        let files = Files::new();
        let mut unacknowledged = pending(CYCLE_A, 7, 50);
        unacknowledged.acknowledged = false;
        unacknowledged.phase = SessionPhase::AwaitAck;
        files.journal(Some(unacknowledged));
        let mut session = files.open();
        session.journal.pending.as_mut().unwrap().acknowledged = true;
        assert!(session.recover_acknowledgements(&targets()).receipts.is_empty());
        files.audit(record(&pending(CYCLE_B, 8, 50), 2000));
        files.write(b"{\"version\":1,\"pending\":broken");
        let corrupt = files.open();
        assert!(corrupt.load_error.is_some());
        files.journal(Some(pending(CYCLE_A, 7, 50)));
        let recovered = corrupt.recover_acknowledgements(&targets());
        assert_eq!(recovered.receipts, vec![receipt(CYCLE_B, 8, 50, 2000)]);
        assert_eq!(recovered.errors.len(), 1);
        std::fs::write(files.path.with_extension("pending.tmp"), b"uncommitted").unwrap();
        let unfinished = files.open().recover_acknowledgements(&targets());
        assert_eq!(unfinished.receipts, recovered.receipts);
        assert_eq!(unfinished.errors.len(), 1);
    }

    #[test]
    fn malformed_identities_and_unknown_versions_are_diagnosed_without_guessing() {
        let files = Files::new();
        files.journal(None);
        let valid = pending(CYCLE_A, 7, 50);
        let mut cases = Vec::new();
        let mut wrong_version = record(&valid, 2000);
        wrong_version["version"] = json!(2);
        cases.push(wrong_version);
        let mut wrong_id = record(&valid, 2000);
        wrong_id["pending"]["cycleId"] = json!("gggggggggggggggggggggggggggggggg");
        cases.push(wrong_id);
        let mut wrong_sn = record(&valid, 2000);
        wrong_sn["pending"]["result"]["sn"] = json!(51);
        cases.push(wrong_sn);
        let mut wrong_seq = record(&valid, 2000);
        wrong_seq["pending"]["result"]["requestSeq"] = json!(8);
        cases.push(wrong_seq);
        let mut wrong_last_seq = record(&valid, 2000);
        wrong_last_seq["lastRequestSeq"] = json!(8);
        cases.push(wrong_last_seq);
        let mut missing_result = record(&valid, 2000);
        missing_result["pending"]["result"] = Value::Null;
        cases.push(missing_result);
        let mut wrong_phase = record(&valid, 2000);
        wrong_phase["pending"]["phase"] = json!("acquiring");
        cases.push(wrong_phase);
        let mut wrong_protocol = record(&valid, 2000);
        wrong_protocol["pending"]["request"]["protocolVersion"] = json!(2);
        cases.push(wrong_protocol);
        for invalid in &cases { files.audit(invalid.clone()); }
        let mut unresolved = targets();
        unresolved.insert("gggggggggggggggggggggggggggggggg".into());
        let recovered = files.open().recover_acknowledgements(&unresolved);
        assert!(recovered.receipts.is_empty());
        assert_eq!(recovered.errors.len(), cases.len(), "{:?}", recovered.errors);
        for (index, error) in recovered.errors.iter().enumerate() { assert!(error.contains(&format!("第 {} 行", index + 1)), "{error}"); }
    }

    #[test]
    fn old_valid_records_without_cycle_identity_are_quietly_skipped() {
        let files = Files::new();
        let mut old = pending(CYCLE_A, 7, 50);
        old.cycle_id = None;
        files.journal(Some(old.clone()));
        files.audit(record(&old, 2000));
        let mut missing = record(&old, 3000);
        missing["pending"].as_object_mut().unwrap().remove("cycleId");
        files.audit(missing);
        let recovered = files.open().recover_acknowledgements(&targets());
        assert!(recovered.receipts.is_empty());
        assert!(recovered.errors.is_empty(), "{:?}", recovered.errors);
    }

    #[test]
    fn conflicting_cycle_identity_blocks_all_duplicates_but_preserves_other_cycles() {
        let files = Files::new();
        files.journal(Some(pending(CYCLE_A, 7, 50)));
        let mut conflict = pending(CYCLE_A, 7, 50);
        conflict.request.plan_version = 456;
        files.audit(record(&conflict, 2000));
        files.audit(record(&pending(CYCLE_A, 7, 50), 3000));
        files.audit(record(&pending(CYCLE_B, 8, 50), 4000));
        let recovered = files.open().recover_acknowledgements(&targets());
        assert_eq!(recovered.receipts, vec![receipt(CYCLE_B, 8, 50, 4000)]);
        assert_eq!(recovered.errors.len(), 1);
        assert!(recovered.errors[0].contains("身份冲突"));
    }

    #[test]
    fn audit_stream_continues_after_corruption_and_rejects_unterminated_receipt() {
        let files = Files::new();
        files.journal(None);
        files.audit_bytes(b"{broken}\n\xff\n");
        files.audit(record(&pending(CYCLE_A, 7, 50), 2000));
        files.audit_bytes(&serde_json::to_vec(&record(&pending(CYCLE_B, 8, 50), 3000)).unwrap());
        let recovered = files.open().recover_acknowledgements(&targets());
        assert_eq!(recovered.receipts, vec![receipt(CYCLE_A, 7, 50, 2000)]);
        assert_eq!(recovered.errors.len(), 3);
        assert!(recovered.errors[2].contains("末尾半写"));
    }

    #[test]
    fn audit_diagnostics_are_bounded_and_report_total_error_count() {
        let files = Files::new();
        files.journal(None);
        let bytes = b"{broken}\n".repeat(100);
        files.audit_bytes(&bytes);
        files.audit(record(&pending(CYCLE_A, 7, 50), 2000));
        let recovered = files.open().recover_acknowledgements(&targets());
        assert_eq!(recovered.receipts, vec![receipt(CYCLE_A, 7, 50, 2000)]);
        assert_eq!(recovered.errors.len(), ERROR_LIMIT + 1);
        assert!(recovered.errors[ERROR_LIMIT].contains("100 条诊断"));
        assert!(recovered.errors[ERROR_LIMIT].contains("36 条已省略"));
    }

    #[test]
    fn no_unresolved_delivery_skips_audit_io_without_hiding_journal_failure() {
        let files = Files::new();
        files.journal(Some(pending(CYCLE_A, 7, 50)));
        std::fs::create_dir(files.path.with_extension("audit.jsonl")).unwrap();
        let session = files.open();
        let recovered = session.recover_acknowledgements(&BTreeSet::new());
        assert!(recovered.receipts.is_empty() && recovered.errors.is_empty());
        assert!(session.pending() && session.acknowledged());
        assert_eq!(session.phase(), SessionPhase::ResetRequired);
        files.write(b"{broken}");
        let corrupt = files.open().recover_acknowledgements(&BTreeSet::new());
        assert!(corrupt.receipts.is_empty());
        assert_eq!(corrupt.errors.len(), 1);
    }

    #[test]
    fn lifecycle_audit_only_retains_unresolved_candidates_and_their_identity_conflicts() {
        let files = Files::new();
        files.journal(None);
        let unresolved = [CYCLE_A.to_owned(), CYCLE_B.to_owned()].into_iter().collect();
        let mut recovery = Recovery::new(&unresolved);
        let mut bytes = Vec::new();
        for index in 100..10_100u32 {
            let cycle = format!("{index:032x}");
            let old = pending(&cycle, 7, 50);
            recovery.pending(&old, 7, 2000, "old history");
            bytes.extend(serde_json::to_vec(&record(&old, 2000)).unwrap());
            bytes.push(b'\n');
        }
        assert!(recovery.receipts.is_empty());
        files.audit_bytes(&bytes);
        let selected = pending(CYCLE_A, 7, 50);
        files.audit(record(&selected, 3000));
        let mut conflict = selected.clone();
        conflict.request.plan_version += 1;
        files.audit(record(&conflict, 4000));
        files.audit(record(&pending(CYCLE_B, 8, 50), 5000));
        let recovered = files.open().recover_acknowledgements(&unresolved);
        assert_eq!(recovered.receipts, vec![receipt(CYCLE_B, 8, 50, 5000)]);
        assert_eq!(recovered.errors.len(), 1);
        assert!(recovered.errors[0].contains("身份冲突"));
    }

}
