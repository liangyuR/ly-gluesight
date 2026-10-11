use super::*;
use crate::judge::{Judgement, Verdict};
use crate::store::{PartRecord, PlcDelivery};
use std::sync::atomic::{AtomicU64, Ordering};

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
        software_version: "comparison-business-test", cycle_id: Some(cycle), bundle_id: None,
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
fn comparisons_keep_candidates_separate_from_original_history() {
    let fixture = Fixture::new(); let store = fixture.store(); let root = fixture.root();
    let id = history(&store, "cycle", 100);
    let before = serde_json::to_value(store.detail(id).unwrap()).unwrap();
    seed(&root, &comparison(id, "cycle", 3));
    seed(&root, &comparison(id, "cycle", 1));
    let comparisons = Comparisons::default();
    comparisons.save(&root, &store, &comparison(id, "cycle", 2)).unwrap();
    assert_eq!(count(&root, id), 3);
    let a = comparisons.list(&root, &store, "A", id).unwrap();
    assert!(a.iter().all(|saved| saved.source == "original" || saved.candidate_id == "A"));
    assert!(a.iter().any(|saved| saved.source == "original" && saved.candidate_id == "B"));
    assert_eq!(serde_json::to_value(store.detail(id).unwrap()).unwrap(), before);
}
