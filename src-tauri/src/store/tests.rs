use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDb(PathBuf);

impl TestDb {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("gluesight-store-{}-{}-{}", std::process::id(), ly_plc::now_ms(), NEXT_DIR.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> PathBuf {
        self.0.join("parts.sqlite")
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn offline_parts_stay_out_of_real_history_statistics_and_rejudge_and_vice_versa() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let mut frames = burst_fixture::frames(4, &[(1, 2)]);
    frames[0].metrics = Some(burst_fixture::measured(0, 1, 6.5));
    let (judgement, burst) = burst_fixture::record(&recipe, frames, None, Some(0));
    let save_source = |recipe: &Recipe, cycle: &str, source| {
        let mut part = burst_part(recipe, cycle, &judgement, &burst);
        part.source = source;
        store.insert(&part)
    };
    let real = save_source(&recipe, "real-part", RecordSource::Real).unwrap();
    let synthetic = save_source(&recipe, "synthetic-part", RecordSource::OfflineSynthetic).unwrap();
    let sample = save_source(&recipe, "sample-part", RecordSource::OfflineSample).unwrap();
    let mut debug_recipe = recipe.clone();
    debug_recipe.revision_id = format!("candidate-{}-i1-r355", recipe.id);
    let debug = save_source(&debug_recipe, "debug-part", RecordSource::OfflineCandidate).unwrap();
    for (scope, expected) in [(HistoryScope::Real, vec![real]), (HistoryScope::Offline, vec![sample, synthetic]), (HistoryScope::Debug, vec![debug])] {
        let query = HistoryQuery { scope, ..Default::default() };
        let page = store.query(&query).unwrap();
        assert_eq!(
            page.items.iter().map(|p| p.id).collect::<Vec<_>>(),
            expected,
            "{scope:?}"
        );
        assert_eq!(
            (page.total, page.counts.ng),
            (expected.len() as u64, expected.len() as u64),
            "{scope:?}"
        );
        assert_eq!(
            store.counts_since(0, scope).unwrap().ng,
            expected.len() as u64,
            "{scope:?}"
        );
        let measurements = store.measurements(&query, &[], 10).unwrap();
        assert_eq!(
            measurements.iter().map(|m| m.id).collect::<Vec<_>>(),
            expected,
            "{scope:?}"
        );
        assert!(measurements
            .iter()
            .all(|measurement| measurement.burst.as_ref() == Some(&burst.judgement)));
    }
}


fn burst_part<'a>(recipe: &'a Recipe, cycle: &'a str, judgement: &'a Judgement, burst: &'a BurstRecord) -> PartRecord<'a> {
    PartRecord {
        mode: PartMode::Production,
        ts: 1234, sn: "SN-B1", recipe: Some(recipe), judgement, drain_ms: None, frames: &[],
        frames_expected: burst.expected_total.unwrap_or(0) as usize, frames_received: burst.judgement.group_count as usize,
        triggers: burst.device_triggers.unwrap_or(0).into(), table: None, software_version: "test-burst", cycle_id: Some(cycle),
        bundle_id: None, delivery: &PlcDelivery { state: PlcDeliveryState::NotRequired, updated_at: 0, message: None }, shots: &[],
        source: RecordSource::Real, sample: None, burst: Some(burst),
    }
}
