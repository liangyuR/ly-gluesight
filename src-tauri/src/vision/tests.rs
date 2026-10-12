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
