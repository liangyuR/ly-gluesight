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
    let path = core.to_str().unwrap();
    let host = VisionHost::default();
    let now = Instant::now();
    let first = host.status_at(Some(path), now);
    std::fs::write(&core, b"not a dll").unwrap();
    let within = host.status_at(Some(path), now);
    let after = host.status_at(Some(path), now + STATUS_RELOAD_BACKOFF + Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!first.loaded && !within.loaded && !after.loaded);
    assert_eq!(within.message, first.message);
    assert_ne!(after.message, first.message);
}
