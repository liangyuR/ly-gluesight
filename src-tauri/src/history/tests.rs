use super::*;

use crate::operation::{Operation, Operations, CANCELLED_MESSAGE};
use crate::recipe::RecipeDoc;
use crate::segmentation::Region;
use crate::store::burst_fixture;

/// 段配方：直线段 + 弯道段（N_exp 待分段算法），整件标定 6 个帧组。
fn burst_doc() -> RecipeDoc {
    let mut doc = crate::recipe::samples_burst();
    let defaults = doc.segment_defaults.clone();
    doc.segments = vec![
        crate::plan::new_segment(1, SegmentKind::Straight, &defaults),
        crate::plan::new_segment(2, SegmentKind::Corner, &defaults),
    ];
    doc.part_calib = Some(crate::plan::PartCalib {
        expected_total: 6,
        calib: crate::plan::SegmentCalib {
            rounds: 10,
            min: 5,
            max: 7,
            mean: 6.0,
        },
    });
    doc
}

/// 一件段配方记录：直线区域 seq 1–3、弯道区域 seq 4–6；相机 1 第 5 帧整帧胶宽 2.0 mm（低于默认下限 2.5 mm），
/// 相机 3 第 2 帧缺帧（检测不完整）。原结论按配方算出，为胶宽 NG。
fn burst_row(recipe: &Recipe) -> (StoredMeasurement, Vec<FrameRecord>) {
    let mut frames = burst_fixture::frames(6, &[(2, 2)]);
    frames
        .iter_mut()
        .find(|frame| (frame.camera, frame.seq) == (0, 5))
        .unwrap()
        .metrics = Some(Arc::new(burst_fixture::measured(0, 5, 2.0)));
    let regions = vec![
        Region {
            kind: RegionKind::Straight { segment: 1 },
            first_seq: 1,
            last_seq: 3,
        },
        Region {
            kind: RegionKind::Corner { segment: 2 },
            first_seq: 4,
            last_seq: 6,
        },
    ];
    let (_, burst) = burst_fixture::record(recipe, frames, Some(regions), Some(6));
    let row = StoredMeasurement {
        id: 7,
        project_id: "default".into(),
        ts: 1234,
        sn: "SN-B7".into(),
        recipe_id: Some(recipe.id.clone()),
        recipe_revision: Some(recipe.revision_id.clone()),
        verdict: burst.judgement.verdict,
        burst: Some(burst.judgement),
    };
    (row, burst.frames)
}

/// 规格 §1 计划身份：段序列 / 类型 / 预期帧数、相机绑定变了，已存指标的区域口径就变了，用当前候选重判时跳过并记 LayoutChanged；
/// 走廊变了每帧指标要原图复测，同样跳过。限值变化沿用计划身份，可以直接重判。
#[test]
fn rejudge_skips_when_plan_identity_changed_but_allows_limit_only_change() {
    type Edit = fn(&mut RecipeDoc);
    let cases: [(&str, Edit, Option<Verdict>); 5] = [
        (
            "弯道段改为直线段",
            |doc| {
                doc.segments[1] = crate::plan::new_segment(
                    2,
                    SegmentKind::Straight,
                    &doc.segment_defaults.clone(),
                )
            },
            None,
        ),
        ("相机顺序", |doc| doc.inspect.swap(0, 1), None),
        (
            "整件重新标定为 7 个帧组",
            |doc| doc.part_calib.as_mut().unwrap().expected_total = 7,
            None,
        ),
        (
            "相机 1 第 1 段的走廊",
            |doc| doc.inspect[0].segments[0].corridor[0][0] += 5.0,
            None,
        ),
        (
            "弯道段胶宽下允差",
            |doc| doc.segments[1].checks.width.tol_lower = 3.0,
            Some(Verdict::Ok),
        ),
    ];
    let original = Arc::new(burst_doc().build().unwrap());
    let request = RejudgeRequest {
        use_current_recipe: true,
        ..Default::default()
    };
    for (case, edit, rejudged) in cases {
        let mut doc = burst_doc();
        edit(&mut doc);
        let candidate = Arc::new(doc.build().unwrap());
        let (row, frames) = burst_row(&original);
        let result = rejudge_rows(
            std::iter::once(Ok(row)),
            &request,
            "default",
            |_, _| Ok(Some(original.clone())),
            |_| Some(candidate.clone()),
            |_| Ok(frames.clone()),
            None,
        )
        .unwrap();
        match rejudged {
            None => assert_eq!(
                result
                    .skip_reasons
                    .iter()
                    .map(|skip| skip.kind)
                    .collect::<Vec<_>>(),
                [SkipKind::LayoutChanged],
                "{case}"
            ),
            Some(verdict) => assert_eq!(
                result
                    .matrix
                    .iter()
                    .map(|cell| (cell.from, cell.to))
                    .collect::<Vec<_>>(),
                [(Verdict::NgWidth, verdict)],
                "{case}"
            ),
        }
    }
}

#[test]
fn rejudge_counts_arrived_but_uninspected_frame_as_present() {
    let recipe = Arc::new(burst_doc().build().unwrap());
    let (row, mut frames) = burst_row(&recipe);
    let frame = frames
        .iter_mut()
        .find(|frame| (frame.camera, frame.seq) == (2, 2))
        .unwrap();
    frame.status = FrameState::NotInspected;
    frame.frame_counter = Some(2);
    frame.trigger_counter = Some(2);
    let mut metrics = burst_fixture::measured(frame.camera, frame.seq, 4.0);
    metrics.state = frame.status;
    frame.metrics = Some(Arc::new(metrics));
    let request = RejudgeRequest {
        overrides: Overrides {
            corner: KindOverride {
                tol_lower: Some(3.0),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let result = rejudge_rows(
        std::iter::once(Ok(row)),
        &request,
        "default",
        |_, _| Ok(Some(recipe.clone())),
        |_| None,
        |_| Ok(frames.clone()),
        None,
    )
    .unwrap();
    let [change] = result.changes.as_slice() else {
        panic!("弯道允差放宽后应有一件判定变化");
    };
    assert_eq!(change.to, Verdict::Ok);
    assert!(change.flags.as_ref().is_some_and(|flags| !flags.incomplete));
}

#[test]
fn rejudge_skips_reversed_region_and_processes_the_rest_of_the_batch() {
    let recipe = Arc::new(burst_doc().build().unwrap());
    let (healthy, frames) = burst_row(&recipe);
    let healthy_id = healthy.id;
    let (mut damaged, _) = burst_row(&recipe);
    damaged.id = healthy.id + 1;
    let damaged_id = damaged.id;
    let region = &mut damaged.burst.as_mut().unwrap().regions[0].region;
    std::mem::swap(&mut region.first_seq, &mut region.last_seq);
    let mut frame_reads = Vec::new();

    let result = rejudge_rows(
        [damaged, healthy].into_iter().map(Ok),
        &RejudgeRequest::default(),
        "default",
        |_, _| Ok(Some(recipe.clone())),
        |_| None,
        |id| {
            frame_reads.push(id);
            Ok(frames.clone())
        },
        None,
    )
    .unwrap();

    assert_eq!((result.total, result.skipped), (1, 1));
    assert_eq!(frame_reads, [healthy_id]);
    let [skip] = result.skip_reasons.as_slice() else {
        panic!("只应跳过区域倒置的工件");
    };
    assert_eq!((skip.id, skip.kind), (damaged_id, SkipKind::LayoutMismatch));
    let [cell] = result.matrix.as_slice() else {
        panic!("其余工件应照常完成重判");
    };
    assert_eq!(
        (cell.from, cell.to, cell.count),
        (Verdict::NgWidth, Verdict::NgWidth, 1)
    );
}

#[test]
fn cancelling_rejudge_during_a_read_counts_finished_and_skipped_records_then_stops() {
    let recipe = Arc::new(burst_doc().build().unwrap());
    let (row, frames) = burst_row(&recipe);
    let id = row.id;
    let (mut skipped, _) = burst_row(&recipe);
    skipped.id = id - 1;
    skipped.verdict = Verdict::ErrInspect;
    let (mut remaining, _) = burst_row(&recipe);
    remaining.id = id + 1;

    let operations = Operations::default();
    let task_id = "cancel-history-rejudge";
    let operation =
        Operation::with_events(operations.clone(), Some(task_id.into()), |_| {}).unwrap();
    let mut candidates_read = Vec::new();
    let mut reads = Vec::new();
    let result = operation.run(|operation| {
        rejudge_rows(
            [skipped, row, remaining].into_iter().map(|row| {
                candidates_read.push(row.id);
                Ok(row)
            }),
            &RejudgeRequest::default(),
            "default",
            |_, _| Ok(Some(recipe.clone())),
            |_| None,
            |record_id| {
                reads.push(record_id);
                if record_id == id {
                    assert!(operations.cancel(task_id));
                }
                Ok(frames.clone())
            },
            Some(operation),
        )
    });
    assert_eq!(result.unwrap_err(), CANCELLED_MESSAGE);
    assert_eq!(candidates_read, [id - 1, id]);
    assert_eq!(reads, [id]);
    let progress = serde_json::to_value(operations.status(task_id).unwrap()).unwrap();
    assert_eq!(progress["stage"], "cancelled");
    assert_eq!(progress["completed"], 2);
    assert_eq!(progress["skipped"], 1);
    assert_eq!(progress["total"], 3);
}

#[test]
fn rejudge_limit_counts_the_oldest_row_without_decoding_it() {
    struct TestDir(std::path::PathBuf);
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    let directory = std::env::temp_dir().join(format!(
        "gluesight-rejudge-limit-{}-{}",
        std::process::id(),
        ly_plc::now_ms()
    ));
    std::fs::create_dir(&directory).unwrap();
    let directory = TestDir(directory);
    let path = directory.0.join("parts.sqlite");
    let store = Store::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let last = (REJUDGE_LIMIT + 1) as i64;
    let judgement =
        r#"{"verdict":"OK","plcCode":1,"faultCode":0,"reason":"合格","segments":[],"gaps":[]}"#;
    conn.execute(
        "WITH RECURSIVE ids(id) AS (
             VALUES(1) UNION ALL SELECT id + 1 FROM ids WHERE id < ?1
         )
         INSERT INTO parts (id, project_id, ts, sn, verdict, plc_code, fault_code, reason,
             frames_expected, frames_received, triggers, software_version, judgement, frames,
             cycle_id, delivery_state, delivery_updated_at, source, burst_judgement)
         SELECT id, ?2, 1234, printf('SN-%d', id), 'OK', 1, 0, '合格', 0, 0, 0, 'fixture',
             ?3, '[]', printf('rejudge-limit-%d', id), 'notRequired', 0, 'real',
             CASE WHEN id = 1 THEN '[' ELSE NULL END
         FROM ids",
        rusqlite::params![last, store.project(), judgement],
    )
    .unwrap();

    let request = RejudgeRequest::default();
    let result = rejudge_history(&store, &request, |_| None, None).unwrap();
    assert!(result.limit_hit);
    assert_eq!(result.total, 0);
    assert_eq!(result.skipped, REJUDGE_LIMIT as u64);
    assert!(result.matrix.is_empty());
    let skipped_ids: Vec<_> = result.skip_reasons.iter().map(|row| row.id).collect();
    assert_eq!(skipped_ids, (last - 299..=last).rev().collect::<Vec<_>>());
    assert!(result
        .skip_reasons
        .iter()
        .all(|row| row.kind == SkipKind::NoTable));

    conn.execute("DELETE FROM parts WHERE id = ?1", [last])
        .unwrap();
    assert!(rejudge_history(&store, &request, |_| None, None).is_err());
}
