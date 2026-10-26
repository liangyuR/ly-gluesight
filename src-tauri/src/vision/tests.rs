use super::*;

#[test]
fn calibration_page_view_on_single_view_camera_also_writes_the_key_burst_recipe_reads() {
    // 防的回归（D-SB-30 前后端契约）：标定页（CalibPanel / FlyshotCalibrationPage）总带图编号、单视角相机为 1，标定只存成
    // 旧配方的 {device}-v1，段配方按设备标定读不到——走廊示教的当量缺省与发布包标定 source 恒为空；或三目拼接相机的
    // 图 N 写进段配方的设备标定；第一个键是标定页读回的键，必须仍是 {device}-v{view}
    use crate::camera::CameraConfig;
    let single = CameraConfig {
        id: "cam1".into(),
        ..CameraConfig::default()
    };
    let triple = CameraConfig {
        id: "cam2".into(),
        view_count: 3,
        ..CameraConfig::default()
    };
    let burst_reads = camera_calib_key(&CameraInspect {
        camera: "cam1".into(),
        corridor: Vec::new(),
        mm_per_px: None,
        calib: None,
        detect: None,
        segments: Vec::new(),
    })
    .unwrap();
    let station = |key: &str| CalibKey::Station(key.into());
    // （相机，标定页传的图编号，要写的键：第一个也是读回的键）
    let cases = [(&single, 1, vec![station("cam1-v1"), burst_reads]), (&triple, 1, vec![station("cam2-v1")]), (&triple, 2, vec![station("cam2-v2")])];
    for (config, view, keys) in cases {
        assert_eq!(calib_keys(config, Some(view)).unwrap(), keys, "{} 图 {view}", config.id);
    }
}
