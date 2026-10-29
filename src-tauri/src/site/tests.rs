use super::*;

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
