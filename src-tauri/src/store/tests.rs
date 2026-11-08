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
    frames[0].metrics = Some(burst_fixture::measured(0, 1, 2.0));
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

#[test]
fn overview_keeps_verdicts_and_single_frame_loads_only_selected_metrics() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let mut frames = burst_fixture::frames(4, &[]);
    for frame in &mut frames {
        frame.metrics = Some(burst_fixture::measured(frame.camera, frame.seq, 2.0));
    }
    let metrics = frames[0].metrics.as_mut().unwrap();
    metrics.widths[0] = None;
    metrics.measurement_evidence = Some(crate::burst_judge::MeasurementEvidence {
        version: 1,
        stations: (0..metrics.widths.len())
            .map(|i| crate::burst_judge::StationEvidence {
                state: if i == 0 {
                    crate::burst_judge::StationEvidenceState::Clipped
                } else {
                    crate::burst_judge::StationEvidenceState::Exact
                },
                width_lower_bound_mm: (i == 0).then_some(9.0),
                ..Default::default()
            })
            .collect(),
        exact_stations: metrics.widths.len() as u32 - 1,
        clipped_stations: 1,
        max_width_lower_bound_mm: Some(9.0),
        present_ratio: Some(1.0),
        ..Default::default()
    });
    let (judgement, burst) = burst_fixture::record(&recipe, frames, None, Some(0));
    let id = store
        .insert(&burst_part(&recipe, "overview-part", &judgement, &burst))
        .unwrap();
    let full = store.detail(id).unwrap().burst.unwrap();
    let overview = store.detail_overview(id).unwrap().burst.unwrap();
    let full_evidence = full.frames[0]
        .metrics
        .as_ref()
        .unwrap()
        .measurement_evidence
        .as_ref()
        .unwrap();
    let summary_evidence = overview.frames[0]
        .metrics
        .as_ref()
        .unwrap()
        .measurement_evidence
        .as_ref()
        .unwrap();
    assert!(!full_evidence.stations.is_empty());
    assert!(summary_evidence.stations.is_empty());
    let mut expected_summary = full_evidence.clone();
    expected_summary.stations.clear();
    assert_eq!(summary_evidence, &expected_summary);
    assert_eq!(
        serde_json::to_value(&overview.frame_verdicts).unwrap(),
        serde_json::to_value(&full.frame_verdicts).unwrap()
    );
    assert!(overview
        .frames
        .iter()
        .all(|frame| frame.metrics.as_ref().unwrap().widths.is_empty()));
    let expected = full
        .frames
        .iter()
        .find(|frame| frame.camera == 0 && frame.seq == 1)
        .unwrap();
    assert!(!expected.metrics.as_ref().unwrap().widths.is_empty());
    rusqlite::Connection::open(db.path())
        .unwrap()
        .execute(
            "UPDATE part_frames SET metrics='broken' WHERE part_id=?1 AND camera=1 AND seq=2",
            [id],
        )
        .unwrap();
    let selected = store.detail_frame(id, 0, 1).unwrap().burst.unwrap();
    assert_eq!(selected.frames, vec![expected.clone()]);
    assert!(selected.frame_verdicts.is_empty());
    assert!(store.detail_frame(id, 0, 100).is_err());
    assert!(store.detail(id).is_err());
}
