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
    debug_recipe.revision_id = format!("candidate-{}-r355", recipe.id);
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

/// 段配方记录入库：结论摘要按区域判定、计划帧数取 ΣN_exp、收到帧取帧组数。
fn save_burst(store: &Store, recipe: &Recipe, cycle: &str, judgement: &Judgement, burst: &BurstRecord) -> Result<i64, String> {
    store.insert(&burst_part(recipe, cycle, judgement, burst))
}

fn burst_part<'a>(recipe: &'a Recipe, cycle: &'a str, judgement: &'a Judgement, burst: &'a BurstRecord) -> PartRecord<'a> {
    PartRecord {
        ts: 1234, sn: "SN-B1", recipe: Some(recipe), judgement, drain_ms: None, frames: &[],
        frames_expected: burst.expected_total.unwrap_or(0) as usize, frames_received: burst.judgement.group_count as usize,
        triggers: burst.device_triggers.unwrap_or(0).into(), table: None, software_version: "test-burst", cycle_id: Some(cycle),
        bundle_id: None, delivery: &PlcDelivery { state: PlcDeliveryState::NotRequired, updated_at: 0, message: None }, shots: &[],
        source: RecordSource::Real, sample: None, burst: Some(burst),
    }
}

/// 规格 §3/§6、决策 D-SB-29：段配方按每相机每帧落库，详情原样带回帧（含缺帧）、区域判定、标记与 deviceTriggers，
/// 列表带整件标记。
#[test]
fn burst_record_round_trips_frames_flags_regions_and_device_triggers() {
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    let recipe = burst_fixture::recipe();
    let (judgement, mut burst) = burst_fixture::record(&recipe, burst_fixture::frames(5, &[(1, 2), (1, 3)]), None, Some(7));
    assert!(burst.judgement.flags.incomplete && !burst.judgement.alarms.is_empty(), "夹具要带缺帧标记和丢帧告警");
    burst.frames.reverse();
    let id = save_burst(&store, &recipe, "burst-cycle", &judgement, &burst).unwrap();

    let detail = store.detail(id).unwrap();
    let stored = detail.burst.as_ref().expect("段配方记录要带 burst");
    let mut frames = burst.frames.clone();
    frames.sort_by_key(|frame| (frame.camera, frame.seq));
    assert_eq!(stored.frames, frames);
    assert_eq!(stored.judgement, burst.judgement);
    assert_eq!((stored.expected_total, stored.tolerance_total, stored.device_triggers, &stored.camera_frames),
        (Some(4), 1, Some(7), &vec![5, 3, 5]));
    assert_eq!((&stored.segments, &stored.cameras), (&recipe.path_segments, &recipe.plan_cameras()));
    assert!(detail.shots.is_empty() && detail.points.is_none() && detail.frames.is_empty());

    let summary = store.query(&HistoryQuery::default()).unwrap().items.remove(0);
    assert_eq!(summary.flags.as_ref(), Some(&burst.judgement.flags));
    assert_eq!((summary.frames_expected, summary.frames_received), (4, 5));
    assert_eq!(store.measurements(&HistoryQuery::default(), &[id], 1).unwrap()[0].burst.as_ref(), Some(&burst.judgement));
}

/// 规格 §2：段配方帧数可以波动，不要求等于预期帧组数；§3：帧身份 (camera, seq) 唯一、相机属于计划相机；
/// D-SB-1：帧记录只属于段配方。不合格的记录整条不落库。
#[test]
fn insert_accepts_any_frame_count_but_rejects_duplicate_camera_seq_or_foreign_camera() {
    type Edit = fn(&mut Recipe, &mut Vec<FrameRecord>);
    let cases: [(&str, u64, Edit, bool); 4] = [
        ("帧组数 7 多于预期 4", 7, |_, _| {}, true),
        (
            "同一相机同一 seq 两条",
            5,
            |_, frames| {
                let duplicate = frames[1].clone();
                frames.push(duplicate);
            },
            false,
        ),
        (
            "相机下标 3 不在计划相机内",
            5,
            |_, frames| frames[0].camera = 3,
            false,
        ),
        (
            "旧拍照点配方带帧记录",
            5,
            |legacy, _| *legacy = crate::recipe::samples().remove(1).build().unwrap(),
            false,
        ),
    ];
    let db = TestDb::new();
    let store = Store::open(&db.path()).unwrap();
    for (index, (case, groups, edit, accepted)) in cases.into_iter().enumerate() {
        let mut recipe = burst_fixture::recipe();
        let (judgement, mut burst) = burst_fixture::record(&recipe, burst_fixture::frames(groups, &[]), None, None);
        edit(&mut recipe, &mut burst.frames);
        let result = save_burst(&store, &recipe, &format!("cycle-{index}"), &judgement, &burst);
        assert_eq!(result.is_ok(), accepted, "{case}: {result:?}");
    }
    let conn = store.conn.lock().unwrap();
    let rows = |table: &str| conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get::<_, i64>(0)).unwrap();
    assert_eq!((rows("parts"), rows("part_frames")), (1, 21), "被拒的记录不留任何行");
}
