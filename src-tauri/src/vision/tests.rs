use super::*;

#[test]
fn invalid_frame_is_rejected_before_ffi() {
    assert!(image_input(&FrameImage::new(0, 1, vec![])).is_err());
    assert!(image_input(&FrameImage::new(2, 2, vec![1, 2, 3])).is_err());
    assert!(image_input(&FrameImage::new(2, 2, vec![1, 2, 3, 4, 5])).is_err());
}

#[test]
fn calibration_keys_are_distinct_for_each_view_of_one_device() {
    assert_eq!(station_view_key("device1", 1).unwrap(), "device1-v1");
    assert_eq!(station_view_key("device1", 3).unwrap(), "device1-v3");
    assert!(station_view_key("device1", 0).is_err());
    assert!(station_view_key("device1", 4).is_err());
}

#[test]
fn unloaded_core_status_reuses_the_failure_within_backoff_and_reloads_after_it() {
    let dir = std::env::temp_dir().join(format!("gluesight-core-backoff-{}-{}", std::process::id(), ly_plc::now_ms()));
    std::fs::create_dir_all(&dir).unwrap();
    let core = dir.join("lyflow_core.dll");
    let runtime = || Some(dir.to_str().unwrap().to_string());
    let host = VisionHost::default();
    let now = Instant::now();
    let first = host.status_at(runtime(), now);
    std::fs::write(&core, b"not a dll").unwrap();
    let within = host.status_at(runtime(), now);
    let after = host.status_at(runtime(), now + STATUS_RELOAD_BACKOFF + Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!first.loaded && !within.loaded && !after.loaded);
    assert_eq!(within.message, first.message);
    assert_ne!(after.message, first.message);
}

#[test]
fn runtime_outside_the_locked_series_or_not_a_formal_build_is_refused() {
    let dir = std::env::temp_dir().join(format!("gluesight-runtime-descriptor-{}-{}", std::process::id(), ly_plc::now_ms()));
    std::fs::create_dir_all(&dir).unwrap();
    let descriptor = |engine: &str, abi: &str, arch: &str, local: bool| std::fs::write(dir.join("lyflow-runtime.json"),
        json!({"engineVersion": engine, "abiVersion": abi, "architecture": arch, "localValidationOnly": local}).to_string()).unwrap();
    let missing = check_runtime(&dir, "1.1");
    let mut refused = Vec::new();
    for (engine, abi, arch, local) in [("1.2.0", "1.2.0", "x64", false), ("1.1.3", "1.2", "x64", false), ("1.1.3", "1.1", "x86", false), ("1.1.3", "1.1", "x64", true)] {
        descriptor(engine, abi, arch, local);
        refused.push(check_runtime(&dir, "1.1").is_err());
    }
    descriptor("1.1.7", "1.1", "x64", false);
    let patch = check_runtime(&dir, "1.1");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(missing.unwrap_err().contains("缺少描述文件"));
    assert_eq!(refused, [true; 4]);
    assert!(patch.is_ok());
}
