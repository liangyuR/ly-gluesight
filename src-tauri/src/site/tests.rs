use super::*;

fn real() -> Environment {
    let mut plc = crate::inspection::default_plc_config();
    plc.connection.host = "192.168.0.10".into();
    plc.auto_connect = true;
    let camera = |id: &str, view_count| CameraConfig {
        id: id.into(), name: format!("现场 {id}"), source: CameraSource::Mvs, view_count, serial: format!("SN-{id}"), ..CameraConfig::default()
    };
    Environment { plc, cameras: vec![camera("cam1", 1), camera("cam2", 3)], record: RecordMode::Off }
}

fn manifest(cameras: serde_json::Value) -> Manifest {
    serde_json::from_value(serde_json::json!({
        "schemaVersion": 1, "id": "set", "name": "样本集", "cameras": cameras,
        "shots": [{"shotId": "S001", "cameraId": "cam7", "views": [1]}],
        "parts": [{"partId": "p01", "sn": "A01", "label": "ok"}],
    })).unwrap()
}

fn ready() -> Preconditions {
    Preconditions { busy: false, capturing: false, lyflow: Ok(()) }
}

#[test]
fn entering_offline_derives_temporary_environment_by_source() {
    let single = manifest(serde_json::json!([{"id": "cam7", "views": [1]}]));
    let triple = manifest(serde_json::json!([{"id": "cam7", "views": [3, 1, 2]}]));
    let cases: [(&str, Derivation, SourceKind, Vec<(&str, CameraSource, u8)>); 3] = [
        ("单视角样本集", Derivation::SampleSet(&single), SourceKind::SampleSet, vec![("cam7", CameraSource::SampleSet, 1)]),
        ("三视角样本集", Derivation::SampleSet(&triple), SourceKind::SampleSet, vec![("cam7", CameraSource::SampleSet, 3)]),
        ("合成图", Derivation::Synthetic, SourceKind::Synthetic, vec![("cam1", CameraSource::Sim, 1), ("cam2", CameraSource::Sim, 3)]),
    ];
    for (name, derivation, kind, cameras) in cases {
        let session = OfflineSession::enter(real(), derivation, 50102, ready()).unwrap();
        let applied = session.applied();
        assert_eq!(session.kind(), kind, "{name}");
        assert_eq!((applied.plc.connection.host.as_str(), applied.plc.connection.port, applied.plc.auto_connect), ("127.0.0.1", 50102, false), "{name}");
        assert_eq!(applied.plc.points, crate::inspection::default_plc_config().points, "{name}");
        let derived: Vec<_> = applied.cameras.iter().map(|c| (c.id.as_str(), c.source, c.view_count)).collect();
        assert_eq!(derived, cameras, "{name}");
        assert_eq!(applied.record, RecordMode::All, "{name}");
    }
}

#[test]
fn entering_offline_is_refused_when_preconditions_fail() {
    let set = manifest(serde_json::json!([{"id": "cam7", "views": [1]}]));
    let partial = manifest(serde_json::json!([{"id": "cam7", "views": [1, 2]}]));
    let cases: [(&str, Derivation, Preconditions, bool); 6] = [
        ("在途事务", Derivation::Synthetic, Preconditions { busy: true, ..ready() }, false),
        ("整圈采集中", Derivation::SampleSet(&set), Preconditions { capturing: true, ..ready() }, false),
        ("样本集缺 lyFlow", Derivation::SampleSet(&set), Preconditions { lyflow: Err("未找到核心库".into()), ..ready() }, false),
        ("样本集视角不完整", Derivation::SampleSet(&partial), ready(), false),
        ("合成图不需要 lyFlow", Derivation::Synthetic, Preconditions { lyflow: Err("未找到核心库".into()), ..ready() }, true),
        ("样本集条件齐全", Derivation::SampleSet(&set), ready(), true),
    ];
    for (name, derivation, pre, accepted) in cases {
        assert_eq!(OfflineSession::enter(real(), derivation, 50102, pre).is_ok(), accepted, "{name}");
    }
}
