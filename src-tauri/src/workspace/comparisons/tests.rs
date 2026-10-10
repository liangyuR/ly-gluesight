use super::*;
use crate::judge::{Judgement, Verdict};
use crate::store::{PartRecord, PlcDelivery};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("gluesight-comparisons-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn root(&self) -> PathBuf { self.0.join("workspaces") }
    fn store(&self) -> Store { Store::open(&self.0.join("parts.sqlite")).unwrap() }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

fn history(store: &Store, cycle: &str, ts: i64) -> i64 {
    store.insert(&PartRecord {
        ts, sn: 42, recipe: None, judgement: &Judgement::error(1, "original"), drain_ms: None,
        frames: &[], frames_expected: 0, frames_received: 0, triggers: 0, table: None,
        software_version: "comparison-retention-test", cycle_id: Some(cycle), bundle_id: None,
        delivery: &PlcDelivery::default(), shots: &[],
    }).unwrap()
}
fn comparison(id: i64, cycle: &str, sequence: i64) -> Comparison {
    Comparison {
        id: format!("{id}-{sequence}-0"), history_id: id, cycle_id: cycle.into(),
        source: if sequence % 3 == 0 { "original" } else { "rules" }.into(), bundle_id: None,
        candidate_id: if sequence % 2 == 0 { "A" } else { "B" }.into(), candidate_revision: 1,
        candidate_recipe: crate::recipe::samples().remove(0).build().unwrap(),
        original_verdict: Verdict::ErrInspect, judgement: Judgement::error(1, "comparison"),
        measurements: vec![serde_json::json!({"sequence": sequence})], created_at: sequence,
    }
}
fn seed(root: &Path, saved: &Comparison) -> PathBuf {
    let dir = directory(root, saved.history_id, true).unwrap();
    let file = dir.join(format!("{}.json", saved.id));
    fs::write(&file, serde_json::to_vec(saved).unwrap()).unwrap();
    file
}
fn count(root: &Path, id: i64) -> usize { fs::read_dir(root.join("comparisons").join(id.to_string())).unwrap().count() }

#[test]
fn latest_hundred_on_disk_are_shared_by_original_and_all_candidates() {
    let fixture = Fixture::new(); let store = fixture.store(); let root = fixture.root();
    let id = history(&store, "cycle", 100);
    let before = serde_json::to_value(store.detail(id).unwrap()).unwrap();
    for sequence in 0..105 { seed(&root, &comparison(id, "cycle", sequence)); }
    let comparisons = Comparisons::default();
    comparisons.save(&root, &store, &comparison(id, "cycle", 105)).unwrap();
    assert_eq!(count(&root, id), LIMIT);
    let mut retained = inventory(&directory(&root, id, false).unwrap(), id).unwrap();
    newest_first(&mut retained);
    assert_eq!(retained.first().unwrap().1.created_at, 105);
    assert_eq!(retained.last().unwrap().1.created_at, 6);
    let a = comparisons.list(&root, &store, "A", id).unwrap();
    assert!(a.iter().all(|saved| saved.source == "original" || saved.candidate_id == "A"));
    assert!(a.iter().any(|saved| saved.source == "original" && saved.candidate_id == "B"));
    assert_eq!(serde_json::to_value(store.detail(id).unwrap()).unwrap(), before);
}

#[test]
fn history_purge_reconciles_old_orphans_and_keeps_live_latest_window() {
    let fixture = Fixture::new(); let store = fixture.store(); let root = fixture.root();
    let old = history(&store, "old", 10); let live = history(&store, "live", 100);
    seed(&root, &comparison(old, "old", 1));
    seed(&root, &comparison(999, "already-purged", 1));
    for sequence in 0..103 { seed(&root, &comparison(live, "live", sequence)); }
    let (deleted, warnings) = Comparisons::default().purge(&root, &store, 50).unwrap();
    assert_eq!(deleted, 1); assert!(warnings.is_empty(), "{warnings:?}");
    assert!(!root.join("comparisons").join(old.to_string()).exists());
    assert!(!root.join("comparisons/999").exists());
    assert_eq!(count(&root, live), LIMIT);
    assert!(store.detail(live).is_ok());
    assert!(!root.join("comparisons").join(live.to_string()).join(format!("{live}-2-0.json")).exists());
}

#[test]
fn late_retest_cannot_resurrect_purged_history_and_reused_id_is_cycle_scoped() {
    let fixture = Fixture::new(); let store = Arc::new(fixture.store()); let root = fixture.root();
    let old = history(&store, "old", 10); let pending = comparison(old, "old", 2);
    seed(&root, &comparison(old, "old", 1));
    let comparisons = Arc::new(Comparisons::default()); let barrier = Arc::new(Barrier::new(2));
    let worker = { let comparisons = comparisons.clone(); let store = store.clone(); let root = root.clone(); let barrier = barrier.clone();
        std::thread::spawn(move || { barrier.wait(); comparisons.save(&root, &store, &pending) }) };
    comparisons.purge(&root, &store, 50).unwrap(); barrier.wait();
    assert!(worker.join().unwrap().is_err());
    assert!(!root.join("comparisons").join(old.to_string()).exists());
    seed(&root, &comparison(old, "old", 3));
    let reused = history(&store, "new", 100); assert_eq!(reused, old);
    assert!(comparisons.list(&root, &store, "A", reused).unwrap().is_empty());
    assert!(comparisons.save(&root, &store, &comparison(old, "old", 4)).is_err());
    comparisons.save(&root, &store, &comparison(reused, "new", 5)).unwrap();
    assert_eq!(count(&root, reused), 1);
    assert_eq!(comparisons.list(&root, &store, "B", reused).unwrap()[0].cycle_id, "new");
}

#[test]
fn concurrent_retests_keep_one_shared_hundred_file_window() {
    let fixture = Fixture::new(); let store = Arc::new(fixture.store()); let root = fixture.root();
    let id = history(&store, "cycle", 100);
    for sequence in 0..98 { seed(&root, &comparison(id, "cycle", sequence)); }
    let comparisons = Arc::new(Comparisons::default()); let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for offset in [98, 104] {
        let comparisons = comparisons.clone(); let store = store.clone(); let root = root.clone(); let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || { barrier.wait();
            for sequence in offset..offset+6 { comparisons.save(&root, &store, &comparison(id, "cycle", sequence)).unwrap(); }
        }));
    }
    barrier.wait(); for worker in workers { worker.join().unwrap(); }
    assert_eq!(count(&root, id), LIMIT);
    let files = inventory(&directory(&root, id, false).unwrap(), id).unwrap();
    assert!(files.iter().all(|(_, saved)| saved.created_at >= 10));
}

#[test]
fn malformed_files_and_sqlite_errors_preserve_unconfirmed_evidence() {
    let fixture = Fixture::new(); let store = fixture.store(); let root = fixture.root();
    let id = history(&store, "cycle", 10);
    let file = seed(&root, &comparison(id, "cycle", 1));
    let bad = file.parent().unwrap().join("unconfirmed.json"); fs::write(&bad, b"not-json").unwrap();
    let comparisons = Comparisons::default();
    assert!(comparisons.save(&root, &store, &comparison(id, "cycle", 2)).is_err());
    let (deleted, warnings) = comparisons.purge(&root, &store, 50).unwrap();
    assert_eq!(deleted, 1); assert_eq!(warnings.len(), 1); assert!(file.exists()); assert!(bad.exists());
    let live = history(&store, "live", 100); let live_file = seed(&root, &comparison(live, "live", 3));
    rusqlite::Connection::open(fixture.0.join("parts.sqlite")).unwrap().execute_batch("DROP TABLE parts").unwrap();
    assert!(comparisons.purge(&root, &store, 200).is_err());
    assert!(comparisons.save(&root, &store, &comparison(live, "live", 4)).is_err());
    assert!(comparisons.list(&root, &store, "A", live).is_err());
    assert!(live_file.exists());
}

#[cfg(windows)]
#[test]
fn locked_retention_victim_rejects_new_writes_without_growing_or_poisoning_history() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new(); let store = fixture.store(); let root = fixture.root();
    let id = history(&store, "cycle", 100);
    let victim = seed(&root, &comparison(id, "cycle", 0));
    for sequence in 1..100 { seed(&root, &comparison(id, "cycle", sequence)); }
    let locked = OpenOptions::new().read(true).share_mode(1).open(&victim).unwrap();
    let comparisons = Comparisons::default();
    for sequence in 100..103 {
        assert!(comparisons.save(&root, &store, &comparison(id, "cycle", sequence)).is_err());
        assert_eq!(count(&root, id), LIMIT);
    }
    assert!(store.detail(id).is_ok());
    drop(locked);
    comparisons.save(&root, &store, &comparison(id, "cycle", 103)).unwrap();
    assert_eq!(count(&root, id), LIMIT); assert!(!victim.exists());
}

#[cfg(windows)]
#[test]
fn root_history_and_file_reparse_points_never_follow_outside_evidence() {
    for placement in 0..3 {
        let fixture = Fixture::new(); let store = fixture.store(); let root = fixture.root();
        let id = history(&store, "cycle", 10);
        let outside = fixture.0.join("outside"); fs::create_dir(&outside).unwrap();
        let sentinel = outside.join("preserve.json"); fs::write(&sentinel, b"preserve").unwrap();
        let link = match placement {
            0 => { fs::create_dir(&root).unwrap(); root.join("comparisons") },
            1 => { fs::create_dir_all(root.join("comparisons")).unwrap(); root.join("comparisons").join(id.to_string()) },
            _ => { directory(&root, id, true).unwrap().join(format!("{id}-1-0.json")) },
        };
        let status = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&outside).output().unwrap();
        assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
        let comparisons = Comparisons::default();
        assert!(comparisons.save(&root, &store, &comparison(id, "cycle", 2)).is_err());
        assert!(comparisons.list(&root, &store, "A", id).is_err());
        let (_, warnings) = comparisons.purge(&root, &store, 50).unwrap(); assert!(!warnings.is_empty());
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve");
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
        fs::remove_dir(&link).unwrap();
    }
}
