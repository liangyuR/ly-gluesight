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

/// 按来源派生离线环境：样本集按清单的相机与视角；合成图沿用真实相机组的编号与视角数（旧配方照常引用），
/// 单视角相机不足 3 台时补足（段配方需要三台独立单视角相机共用触发）。
#[test]
fn entering_offline_derives_temporary_environment_by_source() {
    let single = manifest(serde_json::json!([{"id": "cam7", "views": [1]}]));
    let triple = manifest(serde_json::json!([{"id": "cam7", "views": [3, 1, 2]}]));
    let three_single = Environment { cameras: ["cam1", "cam2", "cam3"].map(|id| CameraConfig { id: id.into(), view_count: 1, ..real().cameras[0].clone() }).to_vec(), ..real() };
    let cases: [(&str, Environment, Derivation, SourceKind, Vec<(&str, CameraSource, u8)>); 4] = [
        ("单视角样本集", real(), Derivation::SampleSet(&single), SourceKind::SampleSet, vec![("cam7", CameraSource::SampleSet, 1)]),
        ("三视角样本集", real(), Derivation::SampleSet(&triple), SourceKind::SampleSet, vec![("cam7", CameraSource::SampleSet, 3)]),
        ("合成图：现场一台单视角、一台三视角 → 沿用并补两台单视角", real(), Derivation::Synthetic, SourceKind::Synthetic,
            vec![("cam1", CameraSource::Sim, 1), ("cam2", CameraSource::Sim, 3), ("cam3", CameraSource::Sim, 1), ("cam4", CameraSource::Sim, 1)]),
        ("合成图：现场已有三台单视角 → 不补", three_single, Derivation::Synthetic, SourceKind::Synthetic,
            vec![("cam1", CameraSource::Sim, 1), ("cam2", CameraSource::Sim, 1), ("cam3", CameraSource::Sim, 1)]),
    ];
    for (name, real, derivation, kind, cameras) in cases {
        let session = OfflineSession::enter(real, derivation, 50102, ready()).unwrap();
        let applied = session.applied();
        assert_eq!(session.kind(), kind, "{name}");
        assert_eq!((applied.plc.connection.host.as_str(), applied.plc.connection.port, applied.plc.auto_connect), ("127.0.0.1", 50102, false), "{name}");
        assert_eq!(applied.plc.points, crate::inspection::default_plc_config().points, "{name}");
        let derived: Vec<_> = applied.cameras.iter().map(|c| (c.id.as_str(), c.source, c.view_count)).collect();
        assert_eq!(derived, cameras, "{name}");
        let planned: Vec<String> = applied.cameras.iter().filter(|c| c.view_count == 1).map(|c| c.id.clone()).collect();
        crate::camera::check_burst_cameras(&applied.cameras, &planned).expect("离线单视角相机应可参加整轮采集");
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
