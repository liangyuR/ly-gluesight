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
    Preconditions {
        busy: false,
        lyflow: Ok(()),
    }
}

#[test]
fn entering_offline_is_refused_when_preconditions_fail() {
    let set = manifest(serde_json::json!([{"id": "cam7", "views": [1]}]));
    let partial = manifest(serde_json::json!([{"id": "cam7", "views": [1, 2]}]));
    let cases: [(&str, Derivation, Preconditions, bool); 5] = [
        ("在途事务", Derivation::Synthetic, Preconditions { busy: true, ..ready() }, false),
        ("样本集缺 lyFlow", Derivation::SampleSet(&set), Preconditions { lyflow: Err("未找到核心库".into()), ..ready() }, false),
        ("样本集视角不完整", Derivation::SampleSet(&partial), ready(), false),
        ("合成图不需要 lyFlow", Derivation::Synthetic, Preconditions { lyflow: Err("未找到核心库".into()), ..ready() }, true),
        ("样本集条件齐全", Derivation::SampleSet(&set), ready(), true),
    ];
    for (name, derivation, pre, accepted) in cases {
        assert_eq!(OfflineSession::enter(real(), derivation, 50102, pre).is_ok(), accepted, "{name}");
    }
}

#[test]
fn work_in_progress_or_unwritten_records_then_project_switch_is_refused() {
    let idle = || ProjectSwitch {
        block: None,
        busy: false,
        audit_pending: false,
    };
    let cases = [
        (
            "在途件或未结事务",
            ProjectSwitch {
                block: Some("检测仍有在途工件，完成握手或复位故障后再切换站点模式".into()),
                ..idle()
            },
        ),
        (
            "节拍未回到空闲",
            ProjectSwitch {
                busy: true,
                ..idle()
            },
        ),
        (
            "录制或追溯未写完",
            ProjectSwitch {
                audit_pending: true,
                ..idle()
            },
        ),
    ];
    for (name, pre) in cases {
        assert!(check_project_switch(&pre).is_err(), "{name}");
    }
    assert!(check_project_switch(&idle()).is_ok());
}
